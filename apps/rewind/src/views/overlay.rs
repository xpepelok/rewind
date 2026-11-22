use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, Render,
    Subscription, Window, div, prelude::*, px, relative,
};

use crate::assets::{Icon, icon};
use crate::gallery::GalleryItem;
use crate::motion::{self, ACCELERATE, DECELERATE, EASE, Key, cubic_bezier};
use crate::platform;
use crate::state::{Rewind, UiEvent};
use crate::theme::{
    FONT_FAMILY, GUTTER, HEADER_HEIGHT, ICON, PAGE_TITLE_TEXT, PANEL_WIDTH, TITLE_TEXT, ThemeExt,
    alpha,
};
use crate::ui::images::Images;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::strong;
use crate::views::editor::{Closed, Editor};
use crate::views::filters::{packed, steer_key};
use crate::views::gallery_pick::Picking;
use crate::views::keys::{Captured, capture};
use crate::views::library::{ClipMenu, PagePicker};
use crate::views::pages::{clock, parse_span};
use crate::views::picture::{Closed as PictureClosed, Picture};
use crate::views::published::{Published, PublishedEdit};
use crate::views::replay_pick::ReplayPick;
use rewind_nvbus::session::{Command as Lens, Mode};

pub const WM_KEYDOWN: i32 = 0x0100;
const WM_KEYUP: i32 = 0x0101;
const WM_MOUSEMOVE: i32 = 0x0200;
const WM_LBUTTONDOWN: i32 = 0x0201;
const WM_LBUTTONUP: i32 = 0x0202;
const WM_MOUSEWHEEL: i32 = 0x020A;
const MK_LBUTTON: i32 = 0x0001;
const WHEEL_NOTCH: f32 = 120.0;
const WHEEL_LINE: f32 = 20.0;
const PANEL_IN: Duration = Duration::from_millis(300);
const BACK_IN: Duration = Duration::from_millis(220);
const CONTENT_SHIFT: f32 = 10.0;
const BACK_SLOT: f32 = 44.0;
const TITLE_SHIFT: f32 = 8.0;
const PANEL: Key = Key::of("overlay.panel");
const ENTER: Key = Key::of("overlay.enter");
const LEAVE: Key = Key::of("overlay.leave");
const BACK: Key = Key::of("overlay.back");
const VIEWER: Key = Key::of("overlay.viewer");
const VIEWER_MARGIN: f32 = 32.0;
const STEP_SIZE: f32 = 28.0;
const APP_TITLE: &str = "Rewind";
const SIGN_IN_WIDTH: f32 = 460.0;
const SIGN_IN_HEIGHT: f32 = 640.0;
const SIGN_IN_HEADER: f32 = 44.0;

fn side_buttons(overlay: gpui::WeakEntity<Overlay>) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, window, cx| {
                let MouseButton::Navigate(direction) = event.button else {
                    return;
                };
                if phase != gpui::DispatchPhase::Capture {
                    return;
                }
                overlay
                    .update(cx, |overlay, cx| match direction {
                        gpui::NavigationDirection::Back => overlay.mouse_back(window, cx),
                        gpui::NavigationDirection::Forward => overlay.mouse_forward(window, cx),
                    })
                    .ok();
                cx.stop_propagation();
            });
        },
    )
    .absolute()
    .size_0()
}

fn shown_in_capture(rewind: &Rewind) -> bool {
    rewind
        .settings
        .captured
        .shows(rewind_config::OverlayPart::Panel)
}

pub fn sign_in_placement(window: &Window) -> Option<youtube::window::Placement> {
    platform::window::inner_placement(window, SIGN_IN_WIDTH, SIGN_IN_HEIGHT, SIGN_IN_HEADER)
}
const IMAGES_SHOWN: usize = 256;
const IMAGES_HIDDEN: usize = 24;
pub const PROBE_REFRESH: Duration = Duration::from_secs(3);

pub struct Memo<K, V> {
    entry: Option<(K, V, Instant)>,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<K: PartialEq, V> Memo<K, V> {
    pub fn get(
        &mut self,
        key: K,
        max_age: Option<Duration>,
        now: Instant,
        compute: impl FnOnce(&K) -> V,
    ) -> &V {
        let fresh = self.entry.as_ref().is_some_and(|(old, _, at)| {
            *old == key && max_age.is_none_or(|age| now.saturating_duration_since(*at) < age)
        });
        if !fresh {
            self.entry = None;
        }
        let (_, value, _) = self.entry.get_or_insert_with(|| {
            let value = compute(&key);
            (key, value, now)
        });
        value
    }

    pub fn clear(&mut self) {
        self.entry = None;
    }
}

pub type GalleryKey = (u64, usize, rewind_config::Gallery);
pub type GalleryView = (Rc<Vec<GalleryItem>>, Rc<Vec<Option<String>>>);
pub type HistoryKey = (u64, u64, usize, usize);
pub type HistoryRow = (youtube::HistoryEntry, Option<PathBuf>, bool);
pub type UploadKey = (usize, Option<String>, bool);
pub type UploadRow = (crate::uploads::Upload, Option<PathBuf>, Option<u64>);
pub type ShotFiles = (Option<PathBuf>, Option<PathBuf>, Option<u64>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenUse {
    Record,
    Replay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Home,
    Replay,
    Settings,
    Hotkeys,
    Interface,
    Notifications,
    Captured,
    Audio,
    Video,
    Profiles,
    Files,
    Gallery,
    YouTube,
    Sharing,
    Replace,
    Published,
    ScreensRecord,
    ScreensReplay,
    PhotoMode,
    Highlights,
    Filters,
    Summary,
    Update,
    Appearance,
    Misc,
}

impl Page {
    pub const COUNT: usize = Self::Misc as usize + 1;

    pub const fn bar(self) -> Option<Self> {
        match self {
            Self::Home
            | Self::Gallery
            | Self::YouTube
            | Self::Sharing
            | Self::Replace
            | Self::Update
            | Self::Published
            | Self::PhotoMode
            | Self::Filters => None,
            Self::ScreensReplay => Some(Self::ScreensRecord),
            other => Some(other),
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Home => APP_TITLE,
            Self::Replay => "Мгновенный повтор",
            Self::Settings => "Параметры",
            Self::Hotkeys => "Сочетания клавиш",
            Self::Interface => "Игровой интерфейс",
            Self::Notifications => "Уведомления",
            Self::Captured => "Оверлей в записи",
            Self::Audio => "Аудио",
            Self::Video => "Захват видео",
            Self::Profiles => "Профили игр",
            Self::Files => "Файлы и дисковое пространство",
            Self::Gallery => "Галерея",
            Self::YouTube => "YouTube",
            Self::Sharing => "Скриншоты",
            Self::Replace => "Замена NVIDIA ShadowPlay",
            Self::Published => "Публикация",
            Self::ScreensRecord | Self::ScreensReplay => "Выбор экрана",
            Self::PhotoMode => "Фоторежим",
            Self::Highlights => "Лучшие моменты",
            Self::Filters => "Игровые фильтры",
            Self::Summary => "Просмотреть лучшие моменты",
            Self::Update => "Обновление",
            Self::Appearance => "Внешний вид",
            Self::Misc => "Прочее",
        }
    }
}

const UPDATE_DOT: u32 = 0x2f8dff;
const TRIM_DELAY: Duration = Duration::from_secs(5);
const FOLD_SETTLE: Duration = Duration::from_millis(150);

pub struct Overlay {
    pub state: Entity<Rewind>,
    focus: FocusHandle,
    stack: Vec<Page>,
    forward: Vec<Page>,
    leaving: Option<Page>,
    open: bool,
    pub page_picker: Option<PagePicker>,
    pub gallery_offset: usize,
    pub menu: Option<ClipMenu>,
    pub confirm_delete: Option<Vec<PathBuf>>,
    pub picking: Picking,
    pub youtube_scroll: gpui::ScrollHandle,
    pub sharing_scroll: gpui::ScrollHandle,
    pub page_scrolls: [gpui::ScrollHandle; Page::COUNT],
    pub published: Option<Published>,
    pub published_edit: PublishedEdit,
    pub viewer_from_home: bool,
    viewer: Option<(Entity<Editor>, Subscription)>,
    photo: Option<(Entity<Picture>, Subscription)>,
    signing_in: bool,
    captured: bool,
    full: Option<platform::window::Extent>,
    settling: bool,
    pub replay_input: Entity<TextInput>,
    pub replay_pick: Option<ReplayPick>,
    pub folded: std::collections::BTreeSet<usize>,
    steer_from: Option<gpui::Point<gpui::Pixels>>,
    panel_hidden: bool,
    pub camera_folded: bool,
    pub photo_choice: crate::views::filters::PhotoChoice,
    pub selected: Vec<PathBuf>,
    pub gallery_view: Memo<GalleryKey, GalleryView>,
    pub history_rows: Memo<HistoryKey, Rc<Vec<HistoryRow>>>,
    pub upload_rows: Memo<UploadKey, Rc<Vec<UploadRow>>>,
    pub shot_files: Memo<(String, bool), ShotFiles>,
    pub free_space: Memo<PathBuf, Option<u64>>,
    pub profile_candidate: Memo<usize, Option<(String, String)>>,
    images: Entity<Images>,
    _subscriptions: Vec<Subscription>,
}

impl Overlay {
    pub fn new(state: Entity<Rewind>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let captured = shown_in_capture(state.read(cx));
        platform::window::prepare_overlay(window, captured);
        let replay_input = cx.new(|cx| {
            TextInput::new("45 мин", window, cx)
                .align_center()
                .max_chars(16)
        });
        let published_edit = PublishedEdit::new(window, cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &replay_input,
                window,
                |overlay, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Submit => {
                        overlay.commit_replay_input(cx);
                        window.focus(&overlay.focus, cx);
                    }
                    InputEvent::Blur => overlay.commit_replay_input(cx),
                    InputEvent::Changed => {}
                },
            ),
            cx.observe_in(&state, window, |overlay, state, window, cx| {
                overlay.sync_replay_input(window, cx);
                overlay.refresh_thumbnail(window, cx);
                let captured = shown_in_capture(state.read(cx));
                if captured != overlay.captured {
                    overlay.captured = captured;
                    platform::window::show_in_capture(window, captured);
                }
                let signing_in = state.read(cx).youtube.signing_in;
                let switched = signing_in != overlay.signing_in;
                if switched {
                    overlay.signing_in = signing_in;
                    let embedded = state.read(cx).youtube.sign_in.is_some();
                    if signing_in && !embedded {
                        platform::window::hide(window);
                    } else if overlay.open {
                        platform::window::show_overlay(window);
                    }
                }
                if switched || overlay.on_screen(cx) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &state,
                window,
                |overlay, _, event, window, cx| match event {
                    UiEvent::ToggleOverlay => {
                        if overlay.open {
                            overlay.close(window, cx);
                        } else {
                            overlay.show(window, cx);
                        }
                    }
                    UiEvent::ShowOverlay => overlay.show(window, cx),
                    UiEvent::OpenVideo(path) => {
                        overlay.viewer_from_home = true;
                        if overlay.open {
                            overlay.open_viewer(path.clone(), window, cx);
                        }
                    }
                    UiEvent::EditPhoto(path) => {
                        overlay.viewer_from_home = true;
                        overlay.show(window, cx);
                        overlay.open_photo(path.clone(), true, window, cx);
                    }
                    UiEvent::PickReplay(replay) => {
                        overlay.show(window, cx);
                        overlay.pick_replay(replay.clone(), cx);
                    }
                    UiEvent::ShowFilters => overlay.toggle_page(Page::Filters, window, cx),
                    UiEvent::ShowPhotoMode => overlay.toggle_page(Page::PhotoMode, window, cx),
                    UiEvent::ShowSummary => {
                        overlay.show(window, cx);
                        overlay.push(Page::Summary, window, cx);
                    }
                    UiEvent::ShowGallery => {
                        overlay.show(window, cx);
                        overlay.push(Page::Gallery, window, cx);
                    }
                },
            ),
        ];
        let seconds = state.read(cx).settings.video.replay_seconds;
        replay_input.update(cx, |input, cx| input.set_text(clock(seconds), cx));
        Self {
            replay_input,
            replay_pick: None,
            folded: std::collections::BTreeSet::new(),
            steer_from: None,
            panel_hidden: false,
            camera_folded: false,
            photo_choice: crate::views::filters::PhotoChoice::default(),
            selected: Vec::new(),
            state,
            focus: cx.focus_handle(),
            stack: vec![Page::Home],
            forward: Vec::new(),
            leaving: None,
            open: false,
            page_picker: None,
            gallery_offset: 0,
            menu: None,
            confirm_delete: None,
            picking: Picking::default(),
            youtube_scroll: gpui::ScrollHandle::new(),
            sharing_scroll: gpui::ScrollHandle::new(),
            page_scrolls: std::array::from_fn(|_| gpui::ScrollHandle::new()),
            published: None,
            published_edit,
            viewer_from_home: false,
            viewer: None,
            photo: None,
            signing_in: false,
            captured,
            full: None,
            settling: false,
            gallery_view: Memo::default(),
            history_rows: Memo::default(),
            upload_rows: Memo::default(),
            shot_files: Memo::default(),
            free_space: Memo::default(),
            profile_candidate: Memo::default(),
            images: Images::new(IMAGES_SHOWN, cx),
            _subscriptions: subscriptions,
        }
    }

    fn commit_replay_input(&mut self, cx: &mut Context<Self>) {
        if let Some(seconds) = parse_span(self.replay_input.read(cx).text()) {
            self.state.update(cx, |rewind, cx| {
                rewind.change(|settings| settings.video.replay_seconds = seconds, cx)
            });
        }
        let seconds = self.state.read(cx).settings.video.replay_seconds;
        self.replay_input
            .update(cx, |input, cx| input.set_text(clock(seconds), cx));
    }

    fn sync_replay_input(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.replay_input.read(cx).is_focused(window) {
            return;
        }
        let seconds = self.state.read(cx).settings.video.replay_seconds;
        self.replay_input
            .update(cx, |input, cx| input.set_text(clock(seconds), cx));
    }

    fn refresh_thumbnail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let youtube = &self.state.read(cx).youtube;
        let Some(video_id) = self
            .published_edit
            .new_thumbnail
            .take_if(|video_id| !youtube.managing.contains_key(video_id.as_str()))
        else {
            return;
        };
        let path = youtube.thumbnail_path(&video_id);
        self.images
            .update(cx, |images, cx| images.forget(&path, window, cx));
    }

    fn forget_views(&mut self) {
        self.gallery_view.clear();
        self.history_rows.clear();
        self.upload_rows.clear();
        self.shot_files.clear();
        self.free_space.clear();
        self.profile_candidate.clear();
        self.picking.forget();
    }

    fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settling = false;
        if !self.on_screen(cx) {
            platform::window::hide(window);
        }
    }

    fn on_screen(&self, cx: &App) -> bool {
        self.open || motion::running(PANEL, cx).is_some()
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            platform::window::show_overlay(window);
            return;
        }
        self.open = true;
        if let Some(full) = self.full.take() {
            platform::window::unfold(window, full, cx);
        }
        self.stack = vec![Page::Home];
        self.forward.clear();
        self.leaving = None;
        self.page_picker = None;
        platform::window::show_overlay(window);
        window.focus(&self.focus, cx);
        motion::stop(ENTER, window, cx);
        motion::stop(LEAVE, window, cx);
        motion::stop(BACK, window, cx);
        motion::restart(PANEL, PANEL_IN, window, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        if self.stack.last() == Some(&Page::Summary) {
            self.state
                .update(cx, |rewind, cx| rewind.discard_moments(cx));
        }
        self.panel_hidden = false;
        self.state.read(cx).close_camera();
        self.replay_pick = None;
        self.picking.cancel();
        self.clear_selection(cx);
        self.confirm_delete = None;
        self.close_viewer(window, cx);
        window.focus(&self.focus, cx);
        self.state.update(cx, |rewind, cx| {
            rewind.cancel_sign_in();
            if rewind.capturing.is_some() {
                rewind.finish_capture(None, cx);
            }
        });
        motion::restart(PANEL, PANEL_IN, window, cx);
        cx.notify();
    }

    pub fn publish_and_delete(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(window, cx);
        self.open_viewer(path, window, cx);
        if let Some((editor, _)) = &self.viewer {
            editor.update(cx, |editor, cx| editor.delete_after_publish(cx));
        }
    }

    pub fn open_viewer(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.close_viewer(window, cx);
        let state = self.state.clone();
        let editor = cx.new(|cx| Editor::new(state, path, window, cx));
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |overlay, editor, event: &Closed, window, cx| match event {
                Closed::Done => overlay.close_viewer(window, cx),
                Closed::Deleted => {
                    let gone = editor.read(cx).path().to_path_buf();
                    overlay.replace_viewed(&[gone], window, cx);
                }
            },
        );
        self.viewer = Some((editor, subscription));
        motion::restart(VIEWER, PANEL_IN, window, cx);
        cx.notify();
    }

    pub fn open_photo(
        &mut self,
        path: PathBuf,
        editing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_viewer(window, cx);
        let state = self.state.clone();
        let picture = cx.new(|cx| Picture::new(state, path, editing, window, cx));
        let subscription = cx.subscribe_in(
            &picture,
            window,
            |overlay, _, _: &PictureClosed, window, cx| overlay.close_viewer(window, cx),
        );
        self.photo = Some((picture, subscription));
        motion::restart(VIEWER, PANEL_IN, window, cx);
        cx.notify();
    }

    fn viewing(&self) -> bool {
        self.viewer.is_some() || self.photo.is_some()
    }

    pub fn viewed_path(&self, cx: &App) -> Option<PathBuf> {
        match (&self.viewer, &self.photo) {
            (Some((editor, _)), _) => Some(editor.read(cx).path().to_path_buf()),
            (None, Some((picture, _))) => Some(picture.read(cx).path().to_path_buf()),
            (None, None) => None,
        }
    }

    fn gallery_order(&mut self, cx: &App) -> Vec<PathBuf> {
        let paths = |items: &[GalleryItem]| items.iter().map(|item| item.path.clone()).collect();
        if self.viewer_from_home {
            return paths(&self.state.read(cx).gallery);
        }
        paths(&self.arranged_gallery(cx).0)
    }

    pub fn viewer_neighbors(&mut self, cx: &App) -> (Option<PathBuf>, Option<PathBuf>) {
        let Some(path) = self.viewed_path(cx) else {
            return (None, None);
        };
        if self.stack.last() == Some(&Page::Published) {
            return (None, None);
        }
        if self.viewer_from_home {
            return neighbors(&self.state.read(cx).gallery, &path);
        }
        neighbors(&self.arranged_gallery(cx).0, &path)
    }

    pub fn show_item(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewer_from_home {
            let gallery = &self.state.read(cx).gallery;
            if let Some(index) = gallery.iter().position(|item| item.path == path) {
                let visible = crate::views::pages::THUMBS_VISIBLE;
                if index < self.gallery_offset {
                    self.gallery_offset = index;
                } else if index >= self.gallery_offset + visible {
                    self.gallery_offset = index + 1 - visible;
                }
            }
        }
        if crate::gallery::is_video(&path) {
            self.open_viewer(path, window, cx);
        } else {
            self.open_photo(path, false, window, cx);
        }
    }

    pub fn step_viewer(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (before, after) = self.viewer_neighbors(cx);
        if let Some(next) = if forward { after } else { before } {
            self.show_item(next, window, cx);
        }
    }

    pub fn replace_viewed(
        &mut self,
        gone: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.viewed_path(cx).filter(|path| gone.contains(path)) else {
            return;
        };
        let order: Vec<PathBuf> = self.gallery_order(cx);
        let index = order.iter().position(|item| *item == path);
        let next = index.and_then(|index| {
            order[index + 1..]
                .iter()
                .find(|item| !gone.contains(item))
                .or_else(|| {
                    order[..index]
                        .iter()
                        .rev()
                        .find(|item| !gone.contains(item))
                })
                .cloned()
        });
        match next {
            Some(next) => self.show_item(next, window, cx),
            None => self.close_viewer(window, cx),
        }
    }

    pub fn close_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((picture, _)) = self.photo.take() {
            picture.update(cx, |picture, cx| picture.release(window, cx));
            window.focus(&self.focus, cx);
            cx.notify();
        }
        let Some((editor, _)) = self.viewer.take() else {
            return;
        };
        editor.update(cx, |editor, cx| editor.release_player(window, cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn push(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.forward.clear();
        self.enter(page, window, cx);
    }

    fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.replay_pick.is_some() {
            self.replay_pick = None;
            cx.notify();
            return true;
        }
        if self.page_picker.is_some() {
            self.close_page_picker(window, cx);
            return true;
        }
        if self.menu.is_some()
            || self.confirm_delete.is_some()
            || self.published_edit.confirm_delete
        {
            self.close_menu(window, cx);
            self.confirm_delete = None;
            self.published_edit.confirm_delete = false;
            cx.notify();
            return true;
        }
        false
    }

    fn mouse_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_modal(window, cx) {
            return;
        }
        if self.viewing() && self.viewer_neighbors(cx).0.is_some() {
            self.step_viewer(false, window, cx);
        } else {
            self.go_back(window, cx);
        }
    }

    fn mouse_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_modal(window, cx) {
            return;
        }
        if self.viewing() {
            self.step_viewer(true, window, cx);
        } else {
            self.go_forward(window, cx);
        }
    }

    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewing() {
            self.close_viewer(window, cx);
        } else if self.stack.len() > 1 {
            self.pop(window, cx);
        }
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewing() {
            return;
        }
        if let Some(page) = self.forward.pop() {
            self.enter(page, window, cx);
        }
    }

    fn enter(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.stack.push(page);
        self.leaving = None;
        self.page_picker = None;
        match page {
            Page::YouTube => self.history_rows.clear(),
            Page::Sharing => self.upload_rows.clear(),
            Page::Published => self.shot_files.clear(),
            Page::Video => self.free_space.clear(),
            Page::Profiles => self.profile_candidate.clear(),
            Page::Filters => self.state.read(cx).open_camera(Mode::Filters),
            Page::PhotoMode => self.state.read(cx).open_camera(Mode::Photo),
            _ => {}
        }
        if page == Page::YouTube {
            self.state
                .update(cx, |rewind, cx| rewind.ensure_youtube_thumbnails(cx));
        }
        if page == Page::Sharing {
            self.state
                .update(cx, |rewind, cx| rewind.ensure_upload_previews(cx));
        }
        if page == Page::Audio {
            self.state
                .update(cx, |rewind, cx| rewind.refresh_microphones(cx));
        }
        motion::stop(LEAVE, window, cx);
        motion::restart(ENTER, PANEL_IN, window, cx);
        motion::drive(BACK, page != Page::Home, BACK_IN, EASE, window, cx);
        cx.notify();
    }

    fn settle_camera(&mut self, cx: &mut Context<Self>) {
        self.panel_hidden = false;
        let rewind = self.state.read(cx);
        match self.stack.last() {
            Some(Page::Filters) => rewind.open_camera(Mode::Filters),
            Some(Page::PhotoMode) => rewind.open_camera(Mode::Photo),
            _ => rewind.close_camera(),
        }
    }

    pub fn toggle_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.open && self.stack.last() == Some(&page) {
            self.close(window, cx);
            return;
        }
        self.show(window, cx);
        self.push(page, window, cx);
    }

    pub fn pop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stack.len() <= 1 {
            self.close(window, cx);
            return;
        }
        self.leaving = self.stack.pop();
        if self.leaving == Some(Page::Summary) {
            self.state
                .update(cx, |rewind, cx| rewind.discard_moments(cx));
        } else {
            self.forward.extend(self.leaving);
        }
        self.settle_camera(cx);
        if self.leaving == Some(Page::Gallery) {
            self.picking.cancel();
            self.clear_selection(cx);
        }
        self.page_picker = None;
        motion::stop(LEAVE, window, cx);
        motion::restart(ENTER, PANEL_IN, window, cx);
        let top = *self.stack.last().unwrap_or(&Page::Home);
        motion::drive(BACK, top != Page::Home, BACK_IN, EASE, window, cx);
        cx.notify();
    }

    fn steers(&self, cx: &App) -> bool {
        self.open
            && !self.viewing()
            && self.stack.last() == Some(&Page::PhotoMode)
            && self.state.read(cx).lens.mode == Mode::Photo
    }

    fn steer(&self, message: i32, wparam: i32, lparam: i32, delta: bool, cx: &App) {
        self.state.read(cx).lens_send(Lens::Input {
            message,
            wparam,
            lparam,
            delta,
        });
    }

    fn on_key_up(&mut self, event: &gpui::KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.steers(cx)
            && let Some(key) = steer_key(&event.keystroke.key)
        {
            self.steer(WM_KEYUP, key, 0, false, cx);
            cx.stop_propagation();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.steers(cx) && matches!(event.keystroke.key.as_str(), "h" | "р") {
            self.panel_hidden ^= true;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.steers(cx)
            && let Some(key) = steer_key(&event.keystroke.key)
        {
            self.steer(WM_KEYDOWN, key, 0, false, cx);
            cx.stop_propagation();
            return;
        }
        let capturing = self.state.read(cx).capturing.is_some();
        if capturing {
            let result = match capture(&event.keystroke) {
                Captured::Cancel => Some(None),
                Captured::Clear => Some(Some(None)),
                Captured::Bind(hotkey) => Some(Some(Some(hotkey))),
                Captured::Ignore => None,
            };
            if let Some(result) = result {
                self.state
                    .update(cx, |rewind, cx| rewind.finish_capture(result, cx));
            }
            cx.stop_propagation();
            return;
        }
        if let Some((editor, _)) = &self.viewer {
            if editor.read(cx).is_typing(window, cx) {
                return;
            }
            if editor.update(cx, |editor, cx| editor.shortcut(&event.keystroke.key, cx)) {
                cx.stop_propagation();
                return;
            }
        }
        let on_gallery = self.stack.last() == Some(&Page::Gallery)
            && !self.viewing()
            && self.menu.is_none()
            && self.confirm_delete.is_none()
            && self.page_picker.is_none();
        let keys = &event.keystroke;
        if on_gallery && keys.key == "a" && keys.modifiers.control && !keys.modifiers.shift {
            self.select_all(cx);
            cx.stop_propagation();
            return;
        }
        if on_gallery && keys.key == "delete" && !self.selected.is_empty() {
            self.confirm_delete = Some(self.selected.clone());
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if on_gallery && keys.key == "escape" && self.clear_selection(cx) {
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" {
            if self.dismiss_modal(window, cx) {
                cx.stop_propagation();
                return;
            }
            if self.viewing() {
                self.close_viewer(window, cx);
                cx.stop_propagation();
                return;
            }
            self.pop(window, cx);
            cx.stop_propagation();
        }
    }

    fn header(&self, page: Page, back: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let home = page == Page::Home;
        let close = div()
            .id("close")
            .flex_none()
            .p(px(6.0))
            .cursor_pointer()
            .child(icon(Icon::Close, ICON, p.title))
            .on_click(cx.listener(|overlay, _, window, cx| overlay.close(window, cx)));
        let bar = div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .bg(if p.soft && p.row_line.is_some() && !p.square {
                p.header
            } else if p.soft {
                p.panel
            } else {
                p.header
            })
            .when(p.soft, |bar| bar.border_b_1().border_color(p.divider))
            .pr(px(GUTTER - 6.0));
        // The back control expands in / collapses out in place; the title text
        // shifts right to make room, mirroring NVIDIA App's header behaviour.
        let back_slot = div()
            .flex_none()
            .h_full()
            .w(px(BACK_SLOT * back))
            .overflow_hidden()
            .flex()
            .items_center()
            .opacity(back)
            .child(
                div()
                    .id("back")
                    .flex_none()
                    .w(px(BACK_SLOT))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .p(px(6.0))
                            .mr(px(8.0))
                            .cursor_pointer()
                            .child(icon(Icon::ArrowBack, ICON, p.title)),
                    )
                    .on_click(cx.listener(|overlay, _, window, cx| overlay.go_back(window, cx))),
            );
        let title_size = TITLE_TEXT + (PAGE_TITLE_TEXT - TITLE_TEXT) * back;
        let title = if home { APP_TITLE } else { page.title() };
        let mut bar = bar.pl(px(GUTTER - 6.0)).child(back_slot).child(
            strong(title, title_size, p.title)
                .flex_1()
                .min_w_0()
                .ml(px(TITLE_SHIFT * (1.0 - back))),
        );
        if home && self.state.read(cx).release.is_some() {
            bar =
                bar.child(
                    div()
                        .id("update-bell")
                        .flex_none()
                        .relative()
                        .p(px(6.0))
                        .mr(px(12.0))
                        .cursor_pointer()
                        .opacity(1.0 - back)
                        .child(icon(Icon::Bell, ICON, p.title))
                        .child(
                            div()
                                .absolute()
                                .top(px(4.0))
                                .right(px(4.0))
                                .size(px(9.0))
                                .rounded_full()
                                .bg(crate::theme::rgb(UPDATE_DOT))
                                .border_1()
                                .border_color(p.header),
                        )
                        .tooltip(crate::ui::Tooltip::text("Доступно обновление"))
                        .on_click(cx.listener(|overlay, _, window, cx| {
                            overlay.push(Page::Update, window, cx)
                        })),
                );
        }
        if home {
            bar = bar.child(
                div()
                    .id("settings")
                    .flex_none()
                    .p(px(6.0))
                    .mr(px(24.0))
                    .cursor_pointer()
                    .opacity(1.0 - back)
                    .child(icon(Icon::Settings, ICON, p.title))
                    .on_click(cx.listener(|overlay, _, window, cx| {
                        overlay.push(Page::Settings, window, cx)
                    })),
            );
        }
        bar.child(close).into_any_element()
    }

    fn layer(
        &mut self,
        page: Page,
        appear: f32,
        back: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = cx.palette();
        let header = self.header(page, back, cx);
        let body = self.page_body(page, window, cx);
        div()
            .id(("page", page as usize))
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(PANEL_WIDTH))
            .flex()
            .flex_col()
            .bg(p.panel)
            .child(header)
            .child(
                // The body fades and settles into place (translateX(-10px) -> 0),
                // so sections swap in-place instead of a full panel sliding across.
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .opacity(appear)
                    .ml(px(-CONTENT_SHIFT * (1.0 - appear)))
                    .child(body),
            )
            .into_any_element()
    }
}

impl Focusable for Overlay {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn neighbors(items: &[GalleryItem], path: &PathBuf) -> (Option<PathBuf>, Option<PathBuf>) {
    let Some(index) = items.iter().position(|item| item.path == *path) else {
        return (None, None);
    };
    (
        index
            .checked_sub(1)
            .map(|before| items[before].path.clone()),
        items.get(index + 1).map(|item| item.path.clone()),
    )
}

fn eased(progress: Option<f32>, curve: [f32; 4]) -> Option<f32> {
    progress.map(|progress| cubic_bezier(curve, progress))
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        let panel_progress = motion::running(PANEL, cx);
        let hidden = panel_progress.is_none() && !self.open;
        if hidden {
            if self.full.is_none() {
                self.full = platform::window::fold(window, cx);
                self.settling = self.full.is_some();
                cx.spawn_in(window, async move |overlay, cx| {
                    cx.background_executor().timer(FOLD_SETTLE).await;
                    overlay
                        .update_in(cx, |overlay, window, cx| overlay.settle(window, cx))
                        .ok();
                    cx.background_executor().timer(TRIM_DELAY).await;
                    platform::system::trim_memory();
                })
                .detach();
            }
            if !self.settling {
                platform::window::hide(window);
            }
            self.images
                .update(cx, |images, cx| images.trim(IMAGES_HIDDEN, window, cx));
        }
        let shown = match (
            eased(
                panel_progress,
                if self.open { DECELERATE } else { ACCELERATE },
            ),
            self.open,
        ) {
            (Some(t), true) => t,
            (Some(t), false) => 1.0 - t,
            (None, open) => f32::from(u8::from(open)),
        };
        self.leaving = None;

        // Sections swap in-place: a single layer whose body fades and settles
        // (no full panel sliding across the screen), with the header's back
        // control expanding in and the title shifting right — like NVIDIA App.
        let top = *self.stack.last().unwrap_or(&Page::Home);
        let appear = eased(motion::running(ENTER, cx), DECELERATE).unwrap_or(1.0);
        let back = motion::value(BACK, cx);
        let mut layers: Vec<AnyElement> = Vec::with_capacity(1);
        layers.push(self.layer(top, appear, back, window, cx));

        if hidden {
            self.forget_views();
        }

        let viewer_in = eased(motion::running(VIEWER, cx), DECELERATE).unwrap_or(1.0);
        let content = match (&self.viewer, &self.photo) {
            (Some((editor, _)), _) => Some(editor.clone().into_any_element()),
            (None, Some((picture, _))) => Some(picture.clone().into_any_element()),
            (None, None) => None,
        };
        let viewer = content.map(|content| {
            div()
                .id("viewer")
                .absolute()
                .top(px(VIEWER_MARGIN))
                .bottom(px(VIEWER_MARGIN))
                .left(px(PANEL_WIDTH + VIEWER_MARGIN))
                .right(px(VIEWER_MARGIN))
                .opacity(shown * viewer_in)
                .shadow(p.toast_shadow())
                .occlude()
                .child(content)
        });
        let (before, after) = if viewer.is_some() {
            self.viewer_neighbors(cx)
        } else {
            (None, None)
        };
        let step = |id: &'static str, glyph: Icon, forward: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .absolute()
                .top(relative(0.5))
                .mt(px(-STEP_SIZE / 2.0))
                .size(px(STEP_SIZE))
                .occlude()
                .rounded_full()
                .bg(alpha(p.panel, 0.9))
                .hover(|style| style.bg(p.hover))
                .cursor_pointer()
                .opacity(shown * viewer_in)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, 20.0, p.title))
                .on_click(cx.listener(move |overlay, _, window, cx| {
                    overlay.step_viewer(forward, window, cx)
                }))
        };
        let previous = before.map(|_| {
            step("viewer-previous", Icon::ChevronLeft, false, cx)
                .left(px(PANEL_WIDTH + (VIEWER_MARGIN - STEP_SIZE) / 2.0))
                .tooltip(crate::ui::Tooltip::text("Предыдущий"))
        });
        let next = after.map(|_| {
            step("viewer-next", Icon::ChevronRight, true, cx)
                .right(px((VIEWER_MARGIN - STEP_SIZE) / 2.0))
                .tooltip(crate::ui::Tooltip::text("Следующий"))
        });
        let menu = self.menu_layer(window, cx);
        let confirm = self.delete_layer(cx);
        let youtube_confirm = self.youtube_delete_layer(cx);
        let replay_pick = self.replay_pick_layer(cx);
        let sign_in = self.sign_in_layer(window, cx);
        let picker_backdrop = self.page_picker.map(|_| {
            div()
                .id("picker-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|overlay, _, window, cx| overlay.close_page_picker(window, cx)),
                )
        });
        motion::keep_alive(window, cx);
        div()
            .image_cache(self.images.clone())
            .id("overlay")
            .font_family(FONT_FAMILY)
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .child(side_buttons(cx.entity().downgrade()))
            .child(
                div()
                    .id("scrim")
                    .absolute()
                    .inset_0()
                    .bg(alpha(p.scrim, shown))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|overlay, event: &gpui::MouseDownEvent, window, cx| {
                            if overlay.steers(cx) {
                                overlay.steer_from = Some(event.position);
                                overlay.steer(WM_LBUTTONDOWN, MK_LBUTTON, 0, false, cx);
                            } else if overlay.viewing() {
                                overlay.close_viewer(window, cx);
                            } else {
                                overlay.close(window, cx);
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|overlay, event: &gpui::MouseMoveEvent, _, cx| {
                        let Some(from) = overlay.steer_from else {
                            return;
                        };
                        if event.pressed_button != Some(MouseButton::Left) {
                            overlay.steer_from = None;
                            overlay.steer(WM_LBUTTONUP, 0, 0, false, cx);
                            return;
                        }
                        let step = packed(
                            f32::from(event.position.x - from.x),
                            f32::from(event.position.y - from.y),
                        );
                        overlay.steer_from = Some(event.position);
                        overlay.steer(WM_MOUSEMOVE, MK_LBUTTON, step, true, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|overlay, _, _, cx| {
                            if overlay.steer_from.take().is_some() {
                                overlay.steer(WM_LBUTTONUP, 0, 0, false, cx);
                            }
                        }),
                    )
                    .on_scroll(cx.listener(|overlay, event: &gpui::ScrollEvent, _, cx| {
                        if !overlay.steers(cx) {
                            return;
                        }
                        let lines = match event.delta {
                            gpui::ScrollDelta::Lines(lines) => lines.y,
                            gpui::ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / WHEEL_LINE,
                        };
                        let turn = (lines * WHEEL_NOTCH).round() as i32;
                        overlay.steer(WM_MOUSEWHEEL, turn << 16, 0, false, cx);
                    })),
            )
            .children(viewer)
            .children(previous)
            .children(next)
            .child(
                div()
                    .id("panel")
                    .when(self.panel_hidden && self.steers(cx), |panel| {
                        panel.invisible()
                    })
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-PANEL_WIDTH * (1.0 - shown)))
                    .w(px(PANEL_WIDTH))
                    .opacity(shown)
                    .overflow_hidden()
                    .bg(p.panel)
                    .occlude()
                    .children(layers),
            )
            .children(picker_backdrop)
            .children(menu)
            .children(confirm)
            .children(youtube_confirm)
            .children(replay_pick)
            .children(sign_in)
    }
}

impl Overlay {
    fn sign_in_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = cx.palette();
        let shown = self.state.read(cx).youtube.sign_in.clone()?;
        if let Some(placement) = sign_in_placement(window) {
            shown.reposition(placement.content);
        }
        Some(
            div()
                .id("sign-in-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.stage, 0.55))
                .occlude()
                .child(
                    div()
                        .w(px(SIGN_IN_WIDTH))
                        .h(px(SIGN_IN_HEIGHT + SIGN_IN_HEADER))
                        .flex()
                        .flex_col()
                        .bg(p.panel)
                        .shadow(p.toast_shadow())
                        .child(
                            div()
                                .h(px(SIGN_IN_HEADER))
                                .flex_none()
                                .px(px(16.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(crate::ui::label("Вход в YouTube", p.text))
                                .child(
                                    div()
                                        .id("sign-in-close")
                                        .size(px(28.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(p.hover))
                                        .child(icon(Icon::Close, 18.0, p.title))
                                        .on_click(cx.listener(|overlay, _, _, cx| {
                                            overlay
                                                .state
                                                .update(cx, |rewind, _| rewind.cancel_sign_in());
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(crate::ui::hint("Открываем страницу входа…", &p)),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_recomputes_only_when_the_key_changes() {
        let mut memo: Memo<u64, u32> = Memo::default();
        let now = Instant::now();
        let mut runs = 0;
        let mut read = |memo: &mut Memo<u64, u32>, key: u64| {
            *memo.get(key, None, now, |key| {
                runs += 1;
                *key as u32 * 10
            })
        };
        assert_eq!(read(&mut memo, 1), 10);
        assert_eq!(read(&mut memo, 1), 10);
        assert_eq!(read(&mut memo, 2), 20);
        memo.clear();
        assert_eq!(read(&mut memo, 2), 20);
        assert_eq!(runs, 3);
    }

    #[test]
    fn memo_expires_after_max_age() {
        let mut memo: Memo<(), u32> = Memo::default();
        let start = Instant::now();
        let age = Some(PROBE_REFRESH);
        assert_eq!(*memo.get((), age, start, |_| 1), 1);
        let soon = start + Duration::from_secs(1);
        assert_eq!(*memo.get((), age, soon, |_| 2), 1);
        let late = start + PROBE_REFRESH;
        assert_eq!(*memo.get((), age, late, |_| 3), 3);
        assert_eq!(*memo.get((), None, late + PROBE_REFRESH * 10, |_| 4), 3);
    }
}
