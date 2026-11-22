use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, Render,
    RenderImage, SharedString, Size, Subscription, Task, Window, canvas, div, fill, img, point,
    prelude::*, px, size,
};
use image::{Frame as ImageFrame, RgbaImage};
use smallvec::smallvec;
use youtube::{Privacy, PublishSettings};

use parking_lot::Mutex;
use rewind_media::{
    Edit, MediaInfo, Mix, Player, PlayerState, Span, audio_tracks, export, export_in_place, probe,
};
use rewind_trace::{Traced, trace};

mod tools;

use tools::{Item, LaneDrag, StageDrag, Tool};

use crate::assets::{Icon, icon};
use crate::channels::{default_settings, text as yt};
use crate::clip_share::{outcome_toast, share_clip};
use crate::editor_files::{copy_path, scratch_file};
use crate::gallery::format_duration;
use crate::motion::{self, DECELERATE, Key};
use crate::state::Rewind;
use crate::theme::{
    CONTROL_TEXT, FONT_FAMILY, GUTTER, HEADER_HEIGHT, ICON, PAGE_TITLE_TEXT, SMALL_TEXT, ThemeExt,
    alpha,
};
use crate::ui::input::TextInput;
use crate::ui::{
    Avatar, ButtonKind, Dropdown, Selector, Toggle, button, checkbox, divider, field, hint, label,
    section, setting_row, strong,
};

const SIDEBAR_WIDTH: f32 = 400.0;
const TRACK_HEIGHT: f32 = 56.0;
const HANDLE_WIDTH: f32 = 10.0;
const HANDLE_GRAB: f32 = 12.0;
const PLAYHEAD: f32 = 2.0;
const SEEK_STEP: f64 = 5.0;
const STRIP_FRAMES: u32 = 12;
const STRIP_HEIGHT: u32 = 90;
const STRIP_WORKERS: usize = 4;
const DROPDOWN_IN: Duration = Duration::from_millis(120);
const DROPDOWN_KEY: Key = Key::of("editor.dropdown");

mod msg {
    pub const COPY_SAVED: &str = "Копия сохранена";
    pub const TRIMMED: &str = "Клип обрезан";
    pub const TRIM_FAILED: &str = "Не удалось обрезать клип";
    pub const NO_TITLE: &str = "Введите название видео";
    pub const LOSSLESS: &str = "Без перекодирования: качество не теряется, начало подгоняется к ближайшему ключевому кадру.";
    pub const RENDERED: &str = "С эффектами, скоростью или звуком клип будет перекодирован.";
    pub const CONFIRM_OVERWRITE: &str = "Обрезать оригинал? Удалённые части нельзя будет вернуть.";
    pub const CONFIRM_DELETE: &str = "Удалить клип? Файл попадёт в Корзину.";
    pub const SIGNING_IN: &str = "Войдите в аккаунт в открывшемся окне браузера…";
    pub const NO_BROWSER: &str = "Для публикации нужен Chrome, Chromium или Edge.";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Start,
    End,
    Playhead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Picker {
    Account,
    Privacy,
    Speed,
    Quality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    Overwrite,
    Delete,
}

pub struct Editor {
    state: Entity<Rewind>,
    path: PathBuf,
    info: Option<MediaInfo>,
    player: Option<Player>,
    stage_pixels: Option<(u32, u32)>,
    _probe: Option<Task<()>>,
    frame: Option<Arc<RenderImage>>,
    strip: Vec<Arc<RenderImage>>,
    edit: Edit,
    audio_tracks: usize,
    tool: Tool,
    selected: Option<Item>,
    stage_drag: Option<StageDrag>,
    lane_drag: Option<LaneDrag>,
    picture: Option<Bounds<Pixels>>,
    lane: Option<Bounds<Pixels>>,
    ink: usize,
    ink_size: usize,
    preview_audio: Option<(Mix, bool)>,
    progress: Arc<Mutex<f32>>,
    dragging: Option<Handle>,
    track: Option<Bounds<Pixels>>,
    title: Entity<TextInput>,
    description: Entity<TextInput>,
    privacy: Privacy,
    quality: Option<u32>,
    flash: Option<(Icon, Instant)>,
    made_for_kids: bool,
    delete_source: bool,
    notify_subscribers: bool,
    busy: bool,
    confirm: Option<Confirm>,
    picker: Option<Picker>,
    error: Option<&'static str>,
    focus: FocusHandle,
    _subscription: Subscription,
}

pub enum Closed {
    Done,
    Deleted,
}

enum Update {
    Frame(Arc<RenderImage>),
    State,
}

impl EventEmitter<Closed> for Editor {}

fn strip_frames(path: &Path, times: &[Duration], size: (u32, u32)) -> Vec<rewind_media::Frame> {
    let workers = std::thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .clamp(1, STRIP_WORKERS);
    let chunk = times.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let parts: Vec<_> = times
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    let _runtime = rewind_media::Runtime::enter();
                    rewind_media::frames_at(path, part, Some(size)).ok()
                })
            })
            .collect();
        parts
            .into_iter()
            .map(|part| part.join().ok().flatten())
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.into_iter().flatten().collect())
            .unwrap_or_default()
    })
}

fn image_of(frame: rewind_media::Frame) -> Option<Arc<RenderImage>> {
    let image = RgbaImage::from_raw(frame.width, frame.height, frame.pixels)?;
    Some(Arc::new(RenderImage::new(smallvec![ImageFrame::new(
        image
    )])))
}

const SIGNATURE: &str = "uploaded via github.com/xpepelok/rewind";
const FLASH: Duration = Duration::from_millis(500);
const FLASH_SIZE: f32 = 64.0;
const FLASH_GROWTH: f32 = 0.5;
const FLASH_GLYPH: f32 = 0.5;
const QUALITIES: [u32; 8] = [2160, 1440, 1080, 720, 480, 360, 240, 144];

fn quality_choices(source_height: u32) -> Vec<Option<u32>> {
    std::iter::once(None)
        .chain(
            QUALITIES
                .iter()
                .filter(|height| **height < source_height)
                .map(|height| Some(*height)),
        )
        .collect()
}

fn quality_label(choice: Option<u32>, source_height: u32) -> String {
    match choice {
        Some(height) => format!("{height}p"),
        None => format!("Оригинал ({source_height}p)"),
    }
}
const SIGNATURE_GAP: &str = "\n\n";
const SPECS_RESERVE: usize = 400;
const DESCRIPTION_LIMIT: usize = youtube::publish::MAX_DESCRIPTION_CHARS
    - SIGNATURE.len()
    - 2 * SIGNATURE_GAP.len()
    - SPECS_RESERVE;

pub fn bulk_settings(path: &Path, specs: &rewind_config::Specs) -> PublishSettings {
    PublishSettings {
        description: signed(
            "",
            crate::platform::specs::describe(&crate::platform::specs::collect(), specs).as_deref(),
        ),
        ..default_settings(path)
    }
}

fn signed(text: &str, specs: Option<&str>) -> String {
    let text = text.trim_end();
    [Some(text), specs, Some(SIGNATURE)]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(SIGNATURE_GAP)
}

fn seconds(value: Duration) -> f64 {
    value.as_secs_f64()
}

fn precise(value: Duration) -> String {
    let total = value.as_millis();
    format!(
        "{:02}:{:02}.{}",
        total / 60_000,
        (total / 1000) % 60,
        (total % 1000) / 100
    )
}

impl Editor {
    pub fn new(
        state: Entity<Rewind>,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let defaults = default_settings(&path);
        let title = cx.new(|cx| {
            let mut input =
                TextInput::new("Название", window, cx).max_chars(youtube::publish::MAX_TITLE_CHARS);
            input.set_text(defaults.title.clone(), cx);
            input
        });
        let description = cx.new(|cx| {
            TextInput::new("Описание", window, cx)
                .multiline()
                .max_chars(DESCRIPTION_LIMIT)
        });
        let subscription = cx.observe(&state, |_, _, cx| cx.notify());
        let mut editor = Self {
            state,
            path,
            info: None,
            player: None,
            stage_pixels: None,
            _probe: None,
            frame: None,
            strip: Vec::new(),
            edit: Edit::new(Span::whole(Duration::ZERO)),
            audio_tracks: 0,
            tool: Tool::Pointer,
            selected: None,
            stage_drag: None,
            lane_drag: None,
            picture: None,
            lane: None,
            ink: 0,
            ink_size: tools::DEFAULT_INK_SIZE,
            preview_audio: None,
            progress: Arc::new(Mutex::new(0.0)),
            dragging: None,
            track: None,
            title,
            description,
            privacy: defaults.privacy,
            quality: None,
            flash: None,
            made_for_kids: false,
            delete_source: false,
            notify_subscribers: true,
            busy: false,
            confirm: None,
            picker: None,
            error: None,
            focus: cx.focus_handle(),
            _subscription: subscription,
        };
        editor.load(window, cx);
        window.focus(&editor.focus, cx);
        editor
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.info = None;
        self.edit = Edit::new(Span::whole(Duration::ZERO));
        self.audio_tracks = 0;
        self.tool = Tool::Pointer;
        self.selected = None;
        self.preview_audio = None;
        self.player = None;
        let path = self.path.clone();
        self._probe = Some(cx.spawn_in(window, async move |this, cx| {
            let (info, tracks) = cx
                .background_executor()
                .spawn(async move {
                    let _runtime = rewind_media::Runtime::enter();
                    (probe(&path).ok(), audio_tracks(&path).unwrap_or(0))
                })
                .await;
            this.update_in(cx, |editor, window, cx| {
                editor.loaded(info, tracks, window, cx)
            })
            .ok();
        }));
    }

    fn loaded(
        &mut self,
        info: Option<MediaInfo>,
        tracks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.info = info;
        let duration = info.map_or(Duration::ZERO, |info| info.duration);
        self.edit = Edit::new(Span::whole(duration));
        self.audio_tracks = tracks;
        let (sender, receiver) = unbounded();
        let bounds = self.stage_pixels.unwrap_or_else(|| {
            let viewport = window.viewport_size();
            let scale = window.scale_factor();
            (
                (f32::from(viewport.width) * scale).round() as u32,
                (f32::from(viewport.height) * scale).round() as u32,
            )
        });
        let wake = sender.clone();
        self.player = Some(Player::open_with_state(
            &self.path,
            bounds,
            move |frame| {
                if let Some(image) = image_of(frame) {
                    sender.unbounded_send(Update::Frame(image)).ok();
                }
            },
            move || {
                wake.unbounded_send(Update::State).ok();
            },
        ));
        self.receive(receiver, window, cx);
        self.load_strip(window, cx);
        cx.notify();
    }

    fn fit_player(&mut self, stage: Size<Pixels>, scale: f32) {
        let pixels = (
            (f32::from(stage.width) * scale).round() as u32,
            (f32::from(stage.height) * scale).round() as u32,
        );
        if pixels.0 == 0 || pixels.1 == 0 || self.stage_pixels == Some(pixels) {
            return;
        }
        self.stage_pixels = Some(pixels);
        if let Some(player) = &self.player {
            player.resize(pixels);
        }
    }

    fn load_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.info else {
            return;
        };
        let width = ((STRIP_HEIGHT * info.width / info.height.max(1)).max(2) + 1) & !1;
        let times: Vec<Duration> = (0..STRIP_FRAMES)
            .map(|index| {
                info.duration
                    .mul_f64((f64::from(index) + 0.5) / f64::from(STRIP_FRAMES))
            })
            .collect();
        let path = self.path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let frames = cx
                .background_executor()
                .spawn(async move { strip_frames(&path, &times, (width, STRIP_HEIGHT)) })
                .await;
            this.update_in(cx, |editor, window, cx| {
                for old in editor.strip.drain(..) {
                    cx.drop_image(old, Some(window));
                }
                editor.strip = frames.into_iter().filter_map(image_of).collect();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn receive(
        &self,
        mut receiver: UnboundedReceiver<Update>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = receiver.next().await {
                let mut latest = match first {
                    Update::Frame(image) => Some(image),
                    Update::State => None,
                };
                while let Ok(newer) = receiver.try_recv() {
                    if let Update::Frame(image) = newer {
                        latest = Some(image);
                    }
                }
                let alive = this.update_in(cx, |editor, window, cx| {
                    if let Some(latest) = latest
                        && let Some(old) = editor.frame.replace(latest)
                    {
                        cx.drop_image(old, Some(window));
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn duration(&self) -> Duration {
        self.info.map_or(Duration::ZERO, |info| info.duration)
    }

    fn player_state(&self) -> PlayerState {
        self.player.as_ref().map(Player::state).unwrap_or_default()
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(player) = &self.player else {
            return;
        };
        let state = player.state();
        self.flash = Some((
            if state.playing {
                Icon::Pause
            } else {
                Icon::Play
            },
            Instant::now(),
        ));
        if state.playing {
            player.pause();
        } else {
            let position = Duration::from_secs_f64(state.position.max(0.0));
            if position < self.edit.span.start || position >= self.edit.span.end {
                player.seek(seconds(self.edit.span.start));
            }
            player.play();
        }
        cx.notify();
    }

    fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(player) = &self.player {
            let target = (player.state().position + delta).clamp(0.0, seconds(self.duration()));
            player.seek(target);
            cx.notify();
        }
    }

    fn time_at(&self, x: Pixels) -> Duration {
        let Some(bounds) = self.track else {
            return Duration::ZERO;
        };
        let width = f32::from(bounds.size.width).max(1.0);
        let fraction = (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0);
        self.duration().mul_f32(fraction)
    }

    fn x_of(&self, time: Duration) -> f32 {
        let Some(bounds) = self.track else {
            return 0.0;
        };
        let total = seconds(self.duration()).max(f64::EPSILON);
        f32::from(bounds.size.width) * (seconds(time) / total) as f32
    }

    fn apply_drag(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(handle) = self.dragging else {
            return;
        };
        let time = self.time_at(x);
        match handle {
            Handle::Start => {
                self.edit.span = self.edit.span.with_start(time);
                if let Some(player) = &self.player {
                    player.seek(seconds(self.edit.span.start));
                }
            }
            Handle::End => {
                self.edit.span = self.edit.span.with_end(time, self.duration());
                if let Some(player) = &self.player {
                    player.seek(seconds(self.edit.span.end));
                }
            }
            Handle::Playhead => {
                if let Some(player) = &self.player {
                    player.seek(seconds(time));
                }
            }
        }
        cx.notify();
    }

    fn on_track_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.track else {
            return;
        };
        let local = f32::from(event.position.x - bounds.origin.x);
        let near = |time: Duration| (local - self.x_of(time)).abs() <= HANDLE_GRAB;
        self.dragging = Some(if near(self.edit.span.start) {
            Handle::Start
        } else if near(self.edit.span.end) {
            Handle::End
        } else {
            Handle::Playhead
        });
        self.apply_drag(event.position.x, cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.on_stage_move(event.position, cx) || self.on_lane_move(event.position, cx) {
            return;
        }
        if self.dragging.is_some() {
            self.apply_drag(event.position.x, cx);
        }
    }

    pub fn is_typing(&self, window: &Window, cx: &App) -> bool {
        [&self.title, &self.description]
            .iter()
            .any(|input| input.read(cx).is_focused(window))
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_typing(window, cx) {
            return;
        }
        if self.shortcut(&event.keystroke.key, cx) {
            cx.stop_propagation();
            return;
        }
        match event.keystroke.key.as_str() {
            "delete" | "backspace" => {
                if !self.delete_selected(cx) {
                    return;
                }
            }
            "escape" => {
                if self.confirm.is_some() || self.picker.is_some() {
                    self.confirm = None;
                    self.picker = None;
                } else if self.tool != Tool::Pointer {
                    self.tool = Tool::Pointer;
                } else if self.selected.is_some() {
                    self.selected = None;
                } else {
                    cx.emit(Closed::Done);
                }
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    pub fn shortcut(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "space" => self.toggle_play(cx),
            "left" => self.seek_by(-SEEK_STEP, cx),
            "right" => self.seek_by(SEEK_STEP, cx),
            _ => return false,
        }
        true
    }

    fn toggle_picker(&mut self, picker: Picker, window: &mut Window, cx: &mut Context<Self>) {
        motion::stop(DROPDOWN_KEY, window, cx);
        if self.picker == Some(picker) {
            self.picker = None;
        } else {
            self.picker = Some(picker);
            motion::drive(DROPDOWN_KEY, true, DROPDOWN_IN, DECELERATE, window, cx);
        }
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        motion::stop(DROPDOWN_KEY, window, cx);
        self.picker = None;
        cx.notify();
    }

    fn shown(&self, picker: Picker, cx: &App) -> Option<f32> {
        (self.picker == Some(picker)).then(|| motion::value(DROPDOWN_KEY, cx))
    }

    pub fn release_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._probe = None;
        self.player = None;
        if let Some(old) = self.frame.take() {
            cx.drop_image(old, Some(window));
        }
        for old in self.strip.drain(..) {
            cx.drop_image(old, Some(window));
        }
    }

    fn progress_reporter(&self) -> impl Fn(f32) + Send + 'static {
        let progress = self.progress.clone();
        *progress.lock() = 0.0;
        move |value| *progress.lock() = value
    }

    fn save_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (source, edit) = (self.path.clone(), self.edit.clone());
        let destination = copy_path(&source);
        let report = self.progress_reporter();
        self.run_trim(
            window,
            cx,
            move || export(&source, &edit, &destination, &report),
            false,
        );
    }

    fn quick_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.state.read(cx).settings.sharing.quick;
        let (source, edit) = (self.path.clone(), self.edit.clone());
        let report = self.progress_reporter();
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { share_clip(&source, Some(edit), target, &report) })
                .await;
            this.update(cx, |editor, cx| {
                editor.busy = false;
                let (title, detail, error) = outcome_toast(target, &outcome);
                editor.state.update(cx, |rewind, cx| {
                    rewind.show_toast(Icon::Copy, title, detail, error, cx)
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn overwrite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        self.release_player(window, cx);
        let (source, edit) = (self.path.clone(), self.edit.clone());
        let report = self.progress_reporter();
        self.run_trim(
            window,
            cx,
            move || export_in_place(&source, &edit, &report),
            true,
        );
    }

    fn run_trim(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl FnOnce() -> rewind_media::Result<()> + Send + 'static,
        reload: bool,
    ) {
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let _runtime = rewind_media::Runtime::enter();
                    job().traced("editor", "apply the edit to the clip")
                })
                .await;
            this.update_in(cx, |editor, window, cx| {
                editor.busy = false;
                let succeeded = outcome.is_ok();
                if reload {
                    editor.load(window, cx);
                }
                editor.state.update(cx, |rewind, cx| {
                    let title = match (succeeded, reload) {
                        (false, _) => msg::TRIM_FAILED,
                        (true, true) => msg::TRIMMED,
                        (true, false) => msg::COPY_SAVED,
                    };
                    rewind.toast(Icon::Videocam, title, None, !succeeded, cx);
                    rewind.forget_thumbnail(&editor.path);
                    rewind.refresh_gallery_later(cx);
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        self.release_player(window, cx);
        let path = self.path.clone();
        let busy = self.state.read(cx).uploading(&path);
        self.state
            .update(cx, |rewind, cx| rewind.delete_clips(vec![path], cx));
        if !busy {
            cx.emit(Closed::Deleted);
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn delete_after_publish(&mut self, cx: &mut Context<Self>) {
        self.delete_source = true;
        cx.notify();
    }

    fn settings(&self, cx: &App) -> PublishSettings {
        PublishSettings {
            title: self.title.read(cx).text().trim().to_owned(),
            description: signed(
                self.description.read(cx).text(),
                crate::platform::specs::describe(
                    &crate::platform::specs::collect(),
                    &self.state.read(cx).settings.youtube.specs,
                )
                .as_deref(),
            ),
            privacy: self.privacy,
            made_for_kids: self.made_for_kids,
            notify_subscribers: self.notify_subscribers && self.privacy == Privacy::Public,
            delete_source: self.delete_source.then(|| self.path.clone()),
            ..default_settings(&self.path)
        }
    }

    fn publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        if settings.title.is_empty() {
            self.error = Some(msg::NO_TITLE);
            cx.notify();
            return;
        }
        let Some(account) = self
            .state
            .read(cx)
            .youtube
            .accounts
            .active()
            .map(|account| account.id.clone())
        else {
            return;
        };
        self.error = None;
        let mut edit = self.edit.clone();
        edit.height = self.quality.filter(|height| *height < self.source_height());
        edit.merge_audio = true;
        if edit.span.is_whole(self.duration()) && !edit.needs_render() {
            let source = self.path.clone();
            self.state.update(cx, |rewind, cx| {
                rewind.publish(account, settings, source, cx)
            });
            return;
        }
        let (source, destination) = (self.path.clone(), scratch_file());
        let report = self.progress_reporter();
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let target = destination.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    let _runtime = rewind_media::Runtime::enter();
                    trace!("editor", "exporting {source:?} to {target:?} with {edit:?}");
                    export(&source, &edit, &target, &report)
                        .traced("editor", "export the clip for publishing")
                })
                .await;
            this.update(cx, |editor, cx| {
                editor.busy = false;
                editor.state.update(cx, |rewind, cx| match outcome {
                    Ok(()) => rewind.publish(account, settings, destination, cx),
                    Err(_) => rewind.toast(Icon::Videocam, msg::TRIM_FAILED, None, true, cx),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Editor {
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .gap(px(14.0))
            .pl(px(GUTTER + 2.0))
            .pr(px(GUTTER - 6.0))
            .bg(p.header)
            .child(strong("Rewind", PAGE_TITLE_TEXT, p.title))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(CONTROL_TEXT))
                    .text_color(p.muted)
                    .child(name),
            )
            .child(
                div()
                    .id("editor-close")
                    .flex_none()
                    .p(px(6.0))
                    .cursor_pointer()
                    .child(icon(Icon::Close, ICON, p.title))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Closed::Done))),
            )
            .into_any_element()
    }

    fn flash(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (glyph, started) = self.flash?;
        let progress = started.elapsed().as_secs_f32() / FLASH.as_secs_f32();
        if progress >= 1.0 {
            self.flash = None;
            return None;
        }
        window.request_animation_frame();
        let p = cx.palette();
        let eased = crate::motion::cubic_bezier(crate::motion::DECELERATE, progress);
        let side = FLASH_SIZE * (1.0 + FLASH_GROWTH * eased);
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(side))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(crate::theme::alpha(p.stage, 0.6))
                        .opacity(1.0 - eased)
                        .child(icon(glyph, side * FLASH_GLYPH, p.white)),
                )
                .into_any_element(),
        )
    }

    fn stage(&self, flash: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let layer = self.stage_layer(cx);
        div()
            .id("stage")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.stage)
            .when_some(self.frame.clone(), |stage, frame| {
                stage.child(img(frame).size_full().object_fit(ObjectFit::Contain))
            })
            .child(layer)
            .children(flash)
            .cursor(match self.tool {
                Tool::Pointer => gpui::CursorStyle::Arrow,
                Tool::Crop | Tool::Draw => gpui::CursorStyle::Crosshair,
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_stage_down))
            .into_any_element()
    }

    fn transport(&self, state: PlayerState, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let position = Duration::from_secs_f64(state.position.max(0.0));
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.0))
            .px(px(GUTTER))
            .py(px(10.0))
            .child(
                div()
                    .id("play")
                    .flex_none()
                    .size(px(40.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.accent)
                    .cursor_pointer()
                    .hover(|style| style.bg(p.accent_hover))
                    .child(icon(
                        if state.playing {
                            Icon::Pause
                        } else {
                            Icon::Play
                        },
                        24.0,
                        p.check_mark,
                    ))
                    .on_click(cx.listener(|editor, _, _, cx| editor.toggle_play(cx))),
            )
            .child(
                div()
                    .text_size(px(CONTROL_TEXT))
                    .text_color(p.text)
                    .child(format!(
                        "{} / {}",
                        precise(position),
                        precise(self.duration())
                    )),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(SMALL_TEXT))
                    .text_color(p.muted)
                    .child(format!(
                        "Фрагмент: {} — {} ({})",
                        precise(self.edit.span.start),
                        precise(self.edit.span.end),
                        format_duration(self.edit.span.length())
                    )),
            )
            .into_any_element()
    }

    fn timeline(&self, state: PlayerState, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let duration = seconds(self.duration()).max(f64::EPSILON);
        let fraction = |time: f64| (time / duration).clamp(0.0, 1.0) as f32;
        let start = fraction(seconds(self.edit.span.start));
        let end = fraction(seconds(self.edit.span.end));
        let head = fraction(state.position);
        let entity = cx.entity().downgrade();
        let filmstrip = !self.strip.is_empty();
        let painter = canvas(
            move |bounds, _, cx| {
                entity
                    .update(cx, |editor, _| editor.track = Some(bounds))
                    .ok();
                bounds
            },
            move |_, bounds: Bounds<Pixels>, window, _| {
                let width = f32::from(bounds.size.width);
                let at = |fraction: f32| bounds.origin.x + px(width * fraction);
                let height = bounds.size.height;
                if !filmstrip {
                    window.paint_quad(fill(bounds, p.track));
                }
                window.paint_quad(fill(
                    Bounds::new(
                        point(at(start), bounds.origin.y),
                        size(px(width * (end - start)), height),
                    ),
                    alpha(p.accent, 0.16),
                ));
                for (from, to) in [(0.0, start), (end, 1.0)] {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(at(from), bounds.origin.y),
                            size(px(width * (to - from)), height),
                        ),
                        alpha(p.stage, 0.45),
                    ));
                }
                for edge in [bounds.origin.y, bounds.origin.y + height - px(2.0)] {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(at(start), edge),
                            size(px(width * (end - start)), px(2.0)),
                        ),
                        p.accent,
                    ));
                }
                for (handle, offset) in [(start, 0.0), (end, -HANDLE_WIDTH)] {
                    let left = at(handle) + px(offset);
                    window.paint_quad(fill(
                        Bounds::new(point(left, bounds.origin.y), size(px(HANDLE_WIDTH), height)),
                        p.accent,
                    ));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                left + px(HANDLE_WIDTH / 2.0 - 1.0),
                                bounds.origin.y + height * 0.3,
                            ),
                            size(px(2.0), height * 0.4),
                        ),
                        p.check_mark,
                    ));
                }
                window.paint_quad(fill(
                    Bounds::new(
                        point(at(head) - px(PLAYHEAD / 2.0), bounds.origin.y - px(6.0)),
                        size(px(PLAYHEAD), height + px(12.0)),
                    ),
                    p.white,
                ));
            },
        )
        .absolute()
        .size_full();
        let frames = self.strip.iter().cloned().map(|frame| {
            img(frame)
                .flex_1()
                .min_w_0()
                .h_full()
                .object_fit(ObjectFit::Cover)
        });
        div()
            .id("timeline")
            .relative()
            .flex_none()
            .mx(px(GUTTER))
            .mb(px(GUTTER))
            .mt(px(8.0))
            .h(px(TRACK_HEIGHT))
            .cursor_pointer()
            .bg(p.track)
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .overflow_hidden()
                    .children(frames),
            )
            .child(painter)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_track_down))
            .into_any_element()
    }

    fn trim_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let whole = self.edit.span.is_whole(self.duration());
        let rendered = self.edit.needs_render();
        let enabled = (!whole || rendered) && !self.busy;
        let quick = self.state.read(cx).settings.sharing.quick;
        let status = if self.busy {
            format!(
                "Обработка… {}%",
                (*self.progress.lock() * 100.0).round() as u32
            )
        } else {
            String::new()
        };
        div()
            .flex()
            .flex_col()
            .child(section("Сохранение", &p))
            .child(
                div()
                    .px(px(GUTTER))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(label(
                        format!(
                            "{} — {}",
                            precise(self.edit.span.start),
                            precise(self.edit.span.end)
                        ),
                        p.text,
                    ))
                    .child(hint(
                        if rendered {
                            msg::RENDERED
                        } else {
                            msg::LOSSLESS
                        },
                        &p,
                    ))
                    .when(self.busy, |column| column.child(label(status, p.accent))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.0))
                    .px(px(GUTTER))
                    .pt(px(12.0))
                    .child(
                        button(
                            "save-copy",
                            "Сохранить копию",
                            ButtonKind::Secondary,
                            enabled,
                            &p,
                        )
                        .flex_1()
                        .when(enabled, |button| {
                            button.on_click(
                                cx.listener(|editor, _, window, cx| editor.save_copy(window, cx)),
                            )
                        }),
                    )
                    .child(
                        button(
                            "overwrite",
                            if rendered {
                                "Изменить оригинал"
                            } else {
                                "Обрезать оригинал"
                            },
                            ButtonKind::Primary,
                            enabled,
                            &p,
                        )
                        .flex_1()
                        .when(enabled, |button| {
                            button.on_click(cx.listener(|editor, _, _, cx| {
                                editor.confirm = Some(Confirm::Overwrite);
                                cx.notify();
                            }))
                        }),
                    ),
            )
            .child(
                div()
                    .px(px(GUTTER))
                    .pt(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(label("Копировать", p.text))
                    .child(hint(crate::clip_share::text::HINT, &p))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div().flex_1().child(
                                    Selector::new("quick-target", quick.label())
                                        .outlined()
                                        .on_step({
                                            let state = self.state.clone();
                                            move |forward, _, cx| {
                                                state.update(cx, |rewind, cx| {
                                                    rewind.change(
                                                        |settings| {
                                                            settings.sharing.quick =
                                                                settings.sharing.quick.step(forward)
                                                        },
                                                        cx,
                                                    )
                                                })
                                            }
                                        }),
                                ),
                            )
                            .child(
                                button(
                                    "quick-share",
                                    "Копировать",
                                    ButtonKind::Primary,
                                    !self.busy,
                                    &p,
                                )
                                .when(!self.busy, |button| {
                                    button.on_click(cx.listener(|editor, _, window, cx| {
                                        editor.quick_share(window, cx)
                                    }))
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn source_height(&self) -> u32 {
        self.info.as_ref().map_or(0, |info| {
            let mut whole = self.edit.clone();
            whole.height = None;
            whole.output_size(info.width, info.height).1.1
        })
    }

    fn quality_picker(&self, cx: &mut Context<Self>) -> Dropdown {
        let source_height = self.source_height();
        let choices = quality_choices(source_height);
        let names = choices
            .iter()
            .map(|choice| SharedString::from(quality_label(*choice, source_height)))
            .collect();
        let selected = choices
            .iter()
            .position(|choice| *choice == self.quality)
            .unwrap_or(0);
        let entity = cx.entity();
        Dropdown::new("quality", names, selected)
            .shown(self.shown(Picker::Quality, cx))
            .on_toggle({
                let entity = entity.clone();
                move |window, cx| {
                    entity.update(cx, |editor, cx| {
                        editor.toggle_picker(Picker::Quality, window, cx)
                    })
                }
            })
            .on_select(move |index, window, cx| {
                entity.update(cx, |editor, cx| {
                    if let Some(choice) = choices.get(index) {
                        editor.quality = *choice;
                    }
                    editor.close_picker(window, cx);
                })
            })
    }

    fn publish_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let channels = &self.state.read(cx).youtube;
        let supported = channels.supported;
        let signing_in = channels.signing_in;
        let accounts: Vec<(String, String)> = channels
            .accounts
            .accounts
            .iter()
            .map(|account| (account.id.clone(), account.display_name()))
            .collect();
        let faces: Vec<Avatar> = channels.accounts.accounts.iter().map(Avatar::of).collect();
        let active = channels.accounts.active().map(|account| account.id.clone());
        let running = channels.queue.running().map(|task| {
            (
                task.title().to_owned(),
                yt::stage(task.stage, task.percent),
                task.fraction(),
            )
        });
        let waiting = channels.queue.waiting_count()
            + channels
                .queue
                .tasks
                .iter()
                .filter(|task| task.state == youtube::TaskState::Running)
                .count()
                .saturating_sub(1);

        let mut column = div()
            .flex()
            .flex_col()
            .child(section("Публикация на YouTube", &p));
        if !supported {
            return column
                .child(div().px(px(GUTTER)).child(hint(msg::NO_BROWSER, &p)))
                .into_any_element();
        }
        if accounts.is_empty() {
            return column
                .child(
                    div()
                        .px(px(GUTTER))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(hint(
                            if signing_in {
                                msg::SIGNING_IN
                            } else {
                                "Добавьте аккаунт, чтобы публиковать клипы."
                            },
                            &p,
                        ))
                        .child(
                            button(
                                "add-account",
                                "Добавить аккаунт YouTube",
                                ButtonKind::Primary,
                                !signing_in,
                                &p,
                            )
                            .when(!signing_in, |button| {
                                button.on_click(cx.listener(|editor, _, window, cx| {
                                    let placement =
                                        crate::views::overlay::sign_in_placement(window);
                                    editor.state.update(cx, |rewind, cx| {
                                        rewind.add_youtube_account(placement, cx)
                                    })
                                }))
                            }),
                        ),
                )
                .into_any_element();
        }
        let selected = active
            .as_ref()
            .and_then(|id| accounts.iter().position(|(other, _)| other == id))
            .unwrap_or(0);
        let ids: Vec<String> = accounts.iter().map(|(id, _)| id.clone()).collect();
        let names = accounts
            .iter()
            .map(|(_, name)| SharedString::from(name.clone()))
            .collect();
        let entity = cx.entity();
        let state = self.state.clone();
        let account_picker = Dropdown::new("account", names, selected)
            .avatars(faces)
            .shown(self.shown(Picker::Account, cx))
            .on_toggle({
                let entity = entity.clone();
                move |window, cx| {
                    entity.update(cx, |editor, cx| {
                        editor.toggle_picker(Picker::Account, window, cx)
                    })
                }
            })
            .on_select({
                let entity = entity.clone();
                move |index, window, cx| {
                    if let Some(id) = ids.get(index) {
                        state.update(cx, |rewind, cx| rewind.select_youtube_account(id, cx));
                    }
                    entity.update(cx, |editor, cx| editor.close_picker(window, cx));
                }
            });
        let privacy_names = Privacy::ALL
            .iter()
            .map(|privacy| SharedString::from(yt::privacy(*privacy)))
            .collect();
        let privacy_index = Privacy::ALL
            .iter()
            .position(|item| *item == self.privacy)
            .unwrap_or(0);
        let privacy_picker = Dropdown::new("privacy", privacy_names, privacy_index)
            .shown(self.shown(Picker::Privacy, cx))
            .on_toggle({
                let entity = entity.clone();
                move |window, cx| {
                    entity.update(cx, |editor, cx| {
                        editor.toggle_picker(Picker::Privacy, window, cx)
                    })
                }
            })
            .on_select(move |index, window, cx| {
                entity.update(cx, |editor, cx| {
                    if let Some(privacy) = Privacy::ALL.get(index) {
                        editor.privacy = *privacy;
                    }
                    editor.close_picker(window, cx);
                })
            });
        column = column
            .child(
                setting_row(&p)
                    .child(label("Канал", p.text).flex_1())
                    .child(account_picker),
            )
            .child(
                div()
                    .px(px(GUTTER))
                    .pb(px(8.0))
                    .child(field(self.title.clone(), false, &p)),
            )
            .child(div().px(px(GUTTER)).pb(px(8.0)).child(field(
                self.description.clone(),
                true,
                &p,
            )))
            .child(
                setting_row(&p)
                    .child(label("Доступ", p.text).flex_1())
                    .child(privacy_picker),
            )
            .child(
                setting_row(&p)
                    .child(label("Качество", p.text).flex_1())
                    .child(self.quality_picker(cx)),
            )
            .child(
                setting_row(&p)
                    .child(label("Видео для детей", p.text).flex_1())
                    .child(Toggle::new("kids", self.made_for_kids).on_click({
                        let entity = cx.entity();
                        move |_, cx| {
                            entity.update(cx, |editor, cx| {
                                editor.made_for_kids = !editor.made_for_kids;
                                cx.notify();
                            })
                        }
                    })),
            )
            .child(
                setting_row(&p)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(label("Удалить с компьютера", p.text))
                            .child(hint("Когда видео загрузится на YouTube", &p)),
                    )
                    .child(Toggle::new("delete-source", self.delete_source).on_click({
                        let entity = cx.entity();
                        move |_, cx| {
                            entity.update(cx, |editor, cx| {
                                editor.delete_source = !editor.delete_source;
                                cx.notify();
                            })
                        }
                    })),
            )
            .when(self.privacy == Privacy::Public, |column| {
                column.child(
                    div()
                        .id("notify")
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .px(px(GUTTER))
                        .py(px(6.0))
                        .cursor_pointer()
                        .child(checkbox(self.notify_subscribers, &p))
                        .child(label("Уведомить подписчиков", p.text))
                        .on_click(cx.listener(|editor, _, _, cx| {
                            editor.notify_subscribers = !editor.notify_subscribers;
                            cx.notify();
                        })),
                )
            })
            .when_some(self.error, |column, error| {
                column.child(
                    div()
                        .px(px(GUTTER))
                        .pt(px(4.0))
                        .text_size(px(SMALL_TEXT))
                        .text_color(p.warning)
                        .child(error),
                )
            })
            .child(
                div().px(px(GUTTER)).pt(px(12.0)).child(
                    button(
                        "publish",
                        if self.delete_source {
                            "Опубликовать и удалить"
                        } else {
                            "Опубликовать"
                        },
                        ButtonKind::Primary,
                        !self.busy,
                        &p,
                    )
                    .w_full()
                    .when(!self.busy, |button| {
                        button.on_click(
                            cx.listener(|editor, _, window, cx| editor.publish(window, cx)),
                        )
                    }),
                ),
            );
        if let Some((title, stage, fraction)) = running {
            column = column.child(
                div()
                    .px(px(GUTTER))
                    .pt(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(label(title, p.text))
                    .child(hint(
                        if waiting > 0 {
                            format!("{stage} · ещё загрузок: {waiting}")
                        } else {
                            stage
                        },
                        &p,
                    ))
                    .child(
                        div()
                            .h(px(4.0))
                            .w_full()
                            .bg(p.track)
                            .child(div().h_full().w(gpui::relative(fraction)).bg(p.accent)),
                    ),
            );
        }
        column.into_any_element()
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        div()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .bg(p.panel)
            .child(
                div()
                    .id("editor-sidebar")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .child(self.trim_section(cx))
                    .child(divider(&p))
                    .child(self.mixer_section(cx))
                    .child(divider(&p))
                    .child(self.publish_section(cx)),
            )
            .child(divider(&p))
            .child(
                div().flex_none().px(px(GUTTER)).pb(px(GUTTER)).child(
                    button("delete", "Удалить клип", ButtonKind::Danger, !self.busy, &p)
                        .w_full()
                        .on_click(cx.listener(|editor, _, _, cx| {
                            editor.confirm = Some(Confirm::Delete);
                            cx.notify();
                        })),
                ),
            )
            .into_any_element()
    }

    fn dialog(&self, confirm: Confirm, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let (text, action) = match confirm {
            Confirm::Overwrite => (msg::CONFIRM_OVERWRITE, "Обрезать"),
            Confirm::Delete => (msg::CONFIRM_DELETE, "Удалить"),
        };
        let kind = match confirm {
            Confirm::Overwrite => ButtonKind::Primary,
            Confirm::Delete => ButtonKind::Danger,
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.scrim)
            .occlude()
            .child(
                div()
                    .w(px(420.0))
                    .flex()
                    .flex_col()
                    .gap(px(18.0))
                    .p(px(24.0))
                    .bg(p.panel)
                    .shadow(p.toast_shadow())
                    .child(label(text, p.text))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(10.0))
                            .child(
                                button("cancel", "Отмена", ButtonKind::Secondary, true, &p)
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.confirm = None;
                                        cx.notify();
                                    })),
                            )
                            .child(button("confirm", action, kind, true, &p).on_click(
                                cx.listener(move |editor, _, window, cx| match confirm {
                                    Confirm::Overwrite => editor.overwrite(window, cx),
                                    Confirm::Delete => editor.delete(window, cx),
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        motion::keep_alive(window, cx);
        let state = self.player_state();
        if state.playing {
            let position = Duration::from_secs_f64(state.position.max(0.0));
            if position >= self.edit.span.end
                && let Some(player) = &self.player
            {
                player.pause();
                player.seek(seconds(self.edit.span.end));
            }
        }
        self.sync_audio();
        if self.busy {
            window.request_animation_frame();
        }
        let header = self.header(cx);
        let flash = self.flash(window, cx);
        let stage = self.stage(flash, cx);
        let transport = self.transport(state, cx);
        let timeline = self.timeline(state, cx);
        let toolbar = self.toolbar(cx);
        let lane = self.lane(cx);
        let sidebar = self.sidebar(cx);
        let dialog = self.confirm.map(|confirm| self.dialog(confirm, cx));
        let backdrop = self.picker.map(|_| {
            div()
                .id("picker-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|editor, _, window, cx| editor.close_picker(window, cx)),
                )
        });
        div()
            .id("editor")
            .font_family(FONT_FAMILY)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(p.panel)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|editor, _, _, cx| {
                    editor.dragging = None;
                    editor.lane_drag = None;
                    editor.on_stage_up(cx);
                    cx.notify();
                }),
            )
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .bg(p.panel)
                            .child(stage)
                            .child(toolbar)
                            .child(transport)
                            .child(timeline)
                            .child(lane),
                    )
                    .child(div().w(px(1.0)).h_full().bg(p.header))
                    .child(sidebar),
            )
            .children(backdrop)
            .children(dialog)
    }
}

#[cfg(test)]
mod tests {
    use super::{SIGNATURE, signed};

    #[test]
    fn the_signature_follows_one_blank_line() {
        assert_eq!(
            signed("Мой клип\n", None),
            format!("Мой клип\n\n{SIGNATURE}")
        );
        assert_eq!(signed("  ", None), SIGNATURE);
        assert_eq!(
            signed("Клип", Some("Видеокарта: RTX")),
            format!("Клип\n\nВидеокарта: RTX\n\n{SIGNATURE}")
        );
    }
}
