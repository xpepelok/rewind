use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use gpui::{AppContext, Context, Entity, EventEmitter, Task};
use parking_lot::Mutex;

use rewind_capture::Screen;
use rewind_capture::{
    AudioControls, AudioLayout, Recorder, RecorderConfig, RecorderEvent, ReplayHealth, Stats,
    VideoCodec,
};
use rewind_config::{Codec, Command, Hotkey, MicMode, Paths, Settings, Store, Tracks};
use rewind_hotkey::{HotkeyEvent, HotkeyService, is_held};
use rewind_trace::{Traced, trace};

use crate::assets::Icon;
use crate::channels::YouTube;
use crate::gallery::{self, GalleryItem};
use crate::platform::{self, system};
use crate::uploads::Uploads;
use crate::watchdog::{self, Action, ReplayWatch};

const OFFER_LIFETIME: Duration = Duration::from_secs(10 * 60);
const TOAST_LIFETIME: Duration = Duration::from_secs(4);
pub const TOAST_EXIT: Duration = Duration::from_millis(500);
const TOAST_LIMIT: usize = 4;
const APPLY_DELAY: Duration = Duration::from_millis(400);
const TICK: Duration = Duration::from_secs(1);
const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

pub mod msg {
    use std::path::Path;

    pub const READY: &str = "Rewind готова к работе";
    pub const REPLAY_ON: &str = "Мгновенный повтор включен";
    pub const REPLAY_OFF: &str = "Мгновенный повтор выключен";
    pub const REPLAY_SAVED: &str = "Мгновенный повтор сохранен";
    pub const REPLAY_SAVING: &str = "Сохраняю мгновенный повтор…";
    pub const REPLAY_SAVE_FAILED: &str = "Не удалось сохранить повтор";
    pub const REPLAY_DISABLED: &str = "Сначала включите мгновенный повтор";
    pub const MARKED: &str = "Метка поставлена";
    pub const MERGED: &str = "Клипы склеены";
    pub const SNIP_FAILED: &str = "Не удалось сделать снимок экрана";
    pub const REPLAY_SAVED_OFFER: &str = "Повтор сохранён. Залить его на YouTube?";
    pub const NO_FRESH_REPLAY: &str = "Нет свежего повтора для заливки";
    pub const SIGN_IN_FIRST: &str = "Сначала войдите в YouTube в оверлее";

    pub fn offer_upload(hotkey: &str) -> String {
        format!("Нажмите {hotkey}, чтобы залить")
    }
    pub const MERGE_FAILED: &str = "Не удалось склеить клипы";

    pub fn merging(count: usize) -> String {
        format!("Склеиваю клипы: {count}")
    }

    pub fn marked(count: usize, save: Option<String>) -> String {
        match save {
            Some(save) => format!("Метка {count}. {save} — сохранить повтор и вырезать клип"),
            None => format!("Метка {count}"),
        }
    }
    pub const RECORDING_STARTED: &str = "Запись запущена";
    pub const RECORDING_SAVED: &str = "Запись сохранена";
    pub const RECORDING_STOPPED: &str = "Запись остановлена";
    pub const RECORDING_SAVING: &str = "Сохраняю в галерею…";
    pub const SCREENSHOT_SAVED: &str = "Снимок экрана сохранен";
    pub const MIC_ON: &str = "Микрофон включен";
    pub const MIC_OFF: &str = "Микрофон выключен";
    pub const FAILED: &str = "Не удалось выполнить запись";
    pub const DELETE_FAILED: &str = "Не удалось удалить";
    pub const HELD_ELSEWHERE: &str = "Файлы открыты в другой программе — закройте её и повторите";

    pub fn held_elsewhere(count: usize) -> String {
        format!(
            "Ещё {count} {} не удалось удалить — открыты в другой программе",
            files_word(count)
        )
    }

    pub fn files_word(count: usize) -> &'static str {
        let tens = count % 100;
        let ones = count % 10;
        if (11..=14).contains(&tens) {
            "файлов"
        } else if ones == 1 {
            "файл"
        } else if (2..=4).contains(&ones) {
            "файла"
        } else {
            "файлов"
        }
    }

    fn singular(count: usize) -> bool {
        count % 10 == 1 && count % 100 != 11
    }

    pub fn shots_word(count: usize) -> &'static str {
        match files_word(count) {
            "файл" => "снимок",
            "файла" => "снимка",
            _ => "снимков",
        }
    }

    pub fn deleted(count: usize) -> String {
        if count == 1 {
            "Файл удалён".to_owned()
        } else if singular(count) {
            format!("Был удалён {count} файл")
        } else {
            format!("Было удалено {count} {}", files_word(count))
        }
    }

    pub const fn restorable(count: usize) -> &'static str {
        if count == 1 {
            "Его можно вернуть из Корзины"
        } else {
            "Их можно вернуть из Корзины"
        }
    }

    pub fn kept_uploading(count: usize) -> String {
        let word = files_word(count);
        let verb = if word == "файл" {
            "загружается"
        } else {
            "загружаются"
        };
        let pronoun = if word == "файл" { "его" } else { "их" };
        format!("{count} {word} сейчас {verb} на YouTube — {pronoun} не тронул")
    }
    pub const REPLACED: &str = "Rewind заменила NVIDIA ShadowPlay";
    pub const RESTORED: &str = "NVIDIA ShadowPlay восстановлен";
    pub const REPLACE_FAILED: &str = "Не удалось изменить системные настройки";
    pub const REPLAY_FAILED: &str = "Не удалось включить мгновенный повтор";
    pub const REPLAY_RECOVERED: &str = "Мгновенный повтор снова работает";

    pub fn replay_attempt(attempt: u32, limit: u32, reason: &str) -> String {
        format!("Попытка {attempt}/{limit} включить мгновенный повтор: {reason}")
    }

    pub fn replay_gave_up(limit: u32, reason: &str) -> String {
        format!("Не удалось включить мгновенный повтор после {limit} попыток: {reason}")
    }

    pub fn capture_failed(reason: &str) -> String {
        format!("Ошибка захвата: {reason}")
    }

    pub const TAKEN: &str = "Сочетания клавиш заняты";

    pub fn taken(labels: &str) -> String {
        format!("{labels} — уже используются другой программой")
    }

    pub fn overlay_hint(hotkey: &str) -> String {
        format!("Нажмите {hotkey}, чтобы открыть оверлей")
    }

    pub fn save_hint(hotkey: &str, seconds: u32) -> String {
        format!(
            "{hotkey} — сохранить последние {} мин",
            seconds.div_ceil(60)
        )
    }

    pub fn stop_hint(hotkey: &str) -> String {
        format!("{hotkey} — остановить")
    }

    pub fn saved_to(path: &Path) -> Option<String> {
        let folder = path.parent()?.file_name()?.to_string_lossy().into_owned();
        Some(format!("Галерея · {folder}"))
    }
}

#[derive(Clone, Debug)]
pub enum Signal {
    Hotkey(HotkeyEvent),
    Recorder(RecorderEvent),
    Program(rewind_capture::ProgramEvent),
    Caught(Hotkey),
    Camera,
    Moment(rewind_nvbus::desk::Moment),
    Show,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    ToggleOverlay,
    ShowOverlay,
    OpenVideo(std::path::PathBuf),
    PickReplay(rewind_capture::ReplayWindow),
    EditPhoto(std::path::PathBuf),
    ShowFilters,
    ShowPhotoMode,
    ShowGallery,
    ShowSummary,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub icon: Icon,
    pub title: String,
    pub detail: Option<String>,
    pub error: bool,
    pub leaving: bool,
    pub stamp: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MicGate {
    mode: MicMode,
    push_to_talk: Option<Hotkey>,
}

pub struct Rewind {
    pub settings: Settings,
    store: Store,
    paths: Paths,
    recorder: Option<Recorder>,
    extras: Vec<(Screen, Recorder)>,
    pub screens: Vec<Screen>,
    recording_on: Option<Screen>,
    replay_target: Option<Screen>,
    controls: Arc<AudioControls>,
    gate: Arc<Mutex<MicGate>>,
    hotkeys: Option<HotkeyService>,
    signals: UnboundedSender<Signal>,
    pub taken: Vec<Command>,
    pub encoder: Option<String>,
    pub microphones: Vec<String>,
    pub gallery: Vec<GalleryItem>,
    gallery_cache: Arc<Mutex<gallery::Cache>>,
    gallery_scan: gallery::Scanner,
    pub gallery_generation: u64,
    pub toasts: Vec<Toast>,
    pub stats: Stats,
    pub recording: Option<Duration>,
    recording_toast: Option<u64>,
    replay_toast: Option<u64>,
    pub capturing: Option<Command>,
    last_error: Option<String>,
    replay_watch: ReplayWatch,
    toast_seq: u64,
    apply_task: Option<Task<()>>,
    pub youtube: YouTube,
    pub replaced: bool,
    pub replacement_busy: bool,
    pub shares: HashMap<PathBuf, f32>,
    pub uploads: Uploads,
    publish_next: bool,
    previewing: bool,
    previews_tried: std::collections::HashSet<String>,
    pub av1_encoder: bool,
    pub active_profile: Option<String>,
    active_process: Option<u32>,
    copy_pending: usize,
    last_replay: Option<(PathBuf, std::time::Instant)>,
    pub snipping: bool,
    catcher: Option<rewind_hotkey::KeyCatcher>,
    camera: Option<rewind_nvbus::Camera>,
    pub lens: std::rc::Rc<rewind_nvbus::session::State>,
    styled: Option<String>,
    style_task: Option<Task<()>>,
    moments: Option<rewind_nvbus::Moments>,
    pub moment_game: Option<String>,
    moment_pending: std::collections::VecDeque<String>,
    pub moment_clips: Vec<moments::MomentClip>,
    pub release: Option<crate::update::Release>,
    pub update_stage: crate::update::Stage,
}

pub mod filters;
pub mod moments;
mod pictures;

impl EventEmitter<UiEvent> for Rewind {}

const AREA: &str = "app";
const HEALTH_EVERY: u32 = 10;
static HEALTH_TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn describe_system() {
    if !rewind_trace::enabled() {
        return;
    }
    std::thread::Builder::new()
        .name("rewind-specs".to_owned())
        .spawn(|| {
            trace!(
                AREA,
                "system {:?}, needs elevation {}, microphones {:?}",
                platform::specs::collect(),
                system::needs_elevation(),
                rewind_capture::microphones()
            );
        })
        .ok();
}

fn recorder_config(settings: &Settings, paths: &Paths, profile: Option<&str>) -> RecorderConfig {
    let video = profile
        .and_then(|executable| settings.profiles.by_executable(executable))
        .map_or(&settings.video, |game| &game.video);
    RecorderConfig {
        codec: match video.codec {
            Codec::H264 => VideoCodec::H264,
            Codec::Hevc => VideoCodec::Hevc,
            Codec::Av1 => VideoCodec::Av1,
        },
        fps: video.frame_rate.value(),
        window_only: video.window_only,
        screen: None,
        bitrate: video.bitrate_mbps() * 1_000_000,
        height: video.resolution.height(),
        replay: Duration::from_secs(u64::from(video.replay_seconds)),
        audio: match settings.audio.tracks {
            Tracks::Single => AudioLayout::Single,
            Tracks::Separate => AudioLayout::Separate,
        },
        microphone: settings.audio.mic_device.clone(),
        capture_desktop: video.capture_desktop,
        gallery: settings.files.gallery.clone(),
        thumbnails: paths.thumbnails(),
        temp: settings.files.temp.clone(),
    }
}

fn shown_differs(stats: &Stats, other: &Stats) -> bool {
    (stats.display_fps, stats.encoded_fps) != (other.display_fps, other.encoded_fps)
}

fn gate_of(settings: &Settings) -> MicGate {
    MicGate {
        mode: settings.audio.mic_mode,
        push_to_talk: settings.bindings.get(Command::PushToTalk),
    }
}

impl Rewind {
    pub fn new(paths: Paths, signals: UnboundedSender<Signal>, cx: &mut Context<Self>) -> Self {
        let youtube = YouTube::load(paths.youtube());
        let paths_uploads = paths.uploads();
        let store = Store::new(paths.settings());
        rewind_trace::open(&paths.logs());
        let loaded = store.load();
        rewind_trace::set_enabled(
            loaded
                .as_ref()
                .is_ok_and(|settings| settings.verbose_logging),
        );
        let mut settings = loaded
            .traced(AREA, "load the settings, defaults are used instead")
            .unwrap_or_default();
        trace!(
            AREA,
            "Rewind {} started, settings {settings:?}",
            env!("CARGO_PKG_VERSION")
        );
        describe_system();
        if platform::nvidia::import(&mut settings) {
            trace!(AREA, "imported the NVIDIA settings");
            store
                .save(&settings)
                .traced(AREA, "save the imported settings")
                .ok();
        }
        let gate = Arc::new(Mutex::new(gate_of(&settings)));
        let gate_reader = gate.clone();
        let controls = Arc::new(AudioControls::new(
            settings.audio.system_gain(),
            settings.audio.mic_gain(),
            move || {
                let gate = *gate_reader.lock();
                match gate.mode {
                    MicMode::AlwaysOn => true,
                    MicMode::Off => false,
                    MicMode::PushToTalk => gate.push_to_talk.is_some_and(is_held),
                }
            },
        ));
        let hotkey_signals = signals.clone();
        let hotkeys = HotkeyService::start(global_hotkeys(&settings), move |event| {
            hotkey_signals.unbounded_send(Signal::Hotkey(event)).ok();
        })
        .traced(AREA, "start the global hotkeys")
        .ok();
        let mut rewind = Self {
            gallery: Vec::new(),
            gallery_cache: Arc::default(),
            gallery_scan: gallery::Scanner::default(),
            gallery_generation: 0,
            settings,
            store,
            paths,
            recorder: None,
            controls,
            gate,
            hotkeys,
            signals,
            taken: Vec::new(),
            encoder: None,
            microphones: Vec::new(),
            toasts: Vec::new(),
            stats: Stats::default(),
            recording: None,
            recording_toast: None,
            replay_toast: None,
            capturing: None,
            last_error: None,
            replay_watch: ReplayWatch::default(),
            toast_seq: 0,
            apply_task: None,
            youtube,
            replaced: system::is_installed(),
            replacement_busy: false,
            shares: HashMap::new(),
            uploads: Uploads::load(&paths_uploads),
            publish_next: false,
            previewing: false,
            previews_tried: std::collections::HashSet::new(),
            av1_encoder: false,
            active_profile: None,
            active_process: None,
            copy_pending: 0,
            last_replay: None,
            snipping: false,
            catcher: None,
            camera: None,
            lens: std::rc::Rc::default(),
            styled: None,
            style_task: None,
            moments: None,
            moment_game: None,
            moment_pending: std::collections::VecDeque::new(),
            moment_clips: Vec::new(),
            release: None,
            update_stage: crate::update::Stage::Idle,
            extras: Vec::new(),
            screens: Vec::new(),
            recording_on: None,
            replay_target: None,
        };
        rewind.restart_recorder();
        rewind.decorate_youtube_accounts(cx);
        rewind.start_ticker(cx);
        rewind.start_replay_watch(cx);
        rewind.start_camera();
        rewind.start_moments();
        rewind.start_profile_watch();
        rewind.scan_profiles(cx);
        rewind.startup_hint(cx);
        rewind.pump_uploads(cx);
        rewind.request_gallery_scan(None, Duration::ZERO, cx);
        rewind.probe_av1(cx);
        rewind.start_update_watch(cx);
        rewind
    }

    fn probe_av1(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let available = cx
                .background_executor()
                .spawn(async move { rewind_capture::hardware_encoder(VideoCodec::Av1) })
                .await;
            this.update(cx, |rewind, cx| {
                if rewind.av1_encoder != available {
                    rewind.av1_encoder = available;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn start_ticker(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this.update(cx, |rewind, cx| rewind.tick(cx));
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(recorder) = &self.recorder else {
            return;
        };
        let stats = recorder.stats();
        if rewind_trace::enabled()
            && HEALTH_TICKS
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .is_multiple_of(HEALTH_EVERY)
        {
            trace!(
                AREA,
                "health, replay wanted {}, replay {:?}, recording {}, saving {}, stats {stats:?}, profile {:?}, last error {:?}",
                self.settings.replay_enabled,
                recorder.replay_health(),
                recorder.is_recording(),
                recorder.is_saving(),
                self.active_profile,
                self.last_error
            );
        }
        let recording = self
            .recorder_on(self.recording_on)
            .and_then(Recorder::recording_elapsed);
        let recording_seconds = recording.map(|elapsed| elapsed.as_secs());
        let changed_recording =
            self.recording.map(|elapsed| elapsed.as_secs()) != recording_seconds;
        let changed_stats = self.settings.stats_visible && shown_differs(&stats, &self.stats);
        self.stats = stats;
        self.recording = recording;
        if changed_recording || changed_stats {
            cx.notify();
        }
    }

    fn start_profile_watch(&self) {
        let signals = self.signals.clone();
        rewind_capture::watch_programs(move |event| {
            signals.unbounded_send(Signal::Program(event)).ok();
        });
    }

    fn scan_profiles(&mut self, cx: &mut Context<Self>) {
        rewind_capture::set_games(self.settings.profiles.executables());
        if !self.settings.profiles.enabled {
            self.switch_profile(None, cx);
            return;
        }
        let running = rewind_capture::running_programs();
        let found = self
            .settings
            .profiles
            .running(&running)
            .map(|(game, process)| (game.executable.clone(), process));
        self.switch_profile(found, cx);
    }

    fn on_program(&mut self, event: rewind_capture::ProgramEvent, cx: &mut Context<Self>) {
        match event {
            rewind_capture::ProgramEvent::Focused {
                executable,
                process,
            } => {
                if self.active_process == Some(process) {
                    return;
                }
                let known = self
                    .settings
                    .profiles
                    .by_executable(&executable)
                    .map(|game| game.executable.clone());
                if let Some(executable) = known {
                    self.switch_profile(Some((executable, process)), cx);
                }
            }
            rewind_capture::ProgramEvent::Exited(process) => {
                if self.active_process == Some(process) {
                    self.active_process = None;
                    self.scan_profiles(cx);
                }
            }
        }
    }

    fn switch_profile(&mut self, next: Option<(String, u32)>, cx: &mut Context<Self>) {
        let process = next.as_ref().map(|(_, process)| *process);
        if process != self.active_process {
            self.active_process = process;
            if let Some(process) = process {
                rewind_capture::watch_exit(process);
            }
        }
        let executable = next.map(|(executable, _)| executable);
        if executable == self.active_profile {
            return;
        }
        let title = match executable
            .as_deref()
            .and_then(|executable| self.settings.profiles.by_executable(executable))
        {
            Some(game) => format!("Профиль «{}» включён", game.name),
            None => "Обычные настройки записи".to_owned(),
        };
        self.active_profile = executable;
        self.restart_recorder();
        if self.settings.notifications.enabled {
            self.toast(Icon::Settings, title, None, false, cx);
        }
        cx.notify();
    }

    pub fn active_profile_name(&self) -> Option<String> {
        self.active_profile
            .as_deref()
            .and_then(|executable| self.settings.profiles.by_executable(executable))
            .map(|game| game.name.clone())
    }

    pub fn profile_candidate(&self) -> Option<(String, String)> {
        let path = rewind_capture::application_path()?;
        let executable = path.file_name()?.to_string_lossy().into_owned();
        if self
            .settings
            .profiles
            .games
            .iter()
            .any(|game| game.executable.eq_ignore_ascii_case(&executable))
        {
            return None;
        }
        Some((rewind_capture::application_name(&path), executable))
    }

    pub fn add_profile(&mut self, name: String, executable: String, cx: &mut Context<Self>) {
        let video = self.settings.video.clone();
        self.change(
            |settings| {
                settings.profiles.enabled = true;
                settings.profiles.games.push(rewind_config::GameProfile {
                    name,
                    executable,
                    video,
                });
            },
            cx,
        );
    }

    fn start_replay_watch(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(watchdog::INTERVAL).await;
                let mut cycle_start = true;
                loop {
                    let Ok(again) =
                        this.update(cx, |rewind, cx| rewind.watch_replay(cycle_start, cx))
                    else {
                        return;
                    };
                    cycle_start = false;
                    if !again {
                        break;
                    }
                    cx.background_executor().timer(watchdog::RETRY_DELAY).await;
                }
            }
        })
        .detach();
    }

    fn watch_replay(&mut self, cycle_start: bool, cx: &mut Context<Self>) -> bool {
        if cycle_start {
            self.replay_watch.begin_cycle();
            if rewind_capture::screens() != self.screens {
                let config =
                    recorder_config(&self.settings, &self.paths, self.active_profile.as_deref());
                self.sync_extras(&config);
                cx.notify();
            }
        }
        let health = self
            .recorder
            .as_ref()
            .map_or(ReplayHealth::Stalled(None), Recorder::replay_health);
        match self
            .replay_watch
            .step(self.settings.replay_enabled, &health)
            .inspect(|action| {
                if *action != Action::Nothing {
                    trace!(
                        AREA,
                        "replay watchdog, wanted {}, health {health:?}, action {action:?}",
                        self.settings.replay_enabled
                    );
                }
            }) {
            Action::Nothing => false,
            Action::CheckSoon => true,
            Action::Recovered => {
                self.log(msg::REPLAY_RECOVERED);
                false
            }
            Action::Enable { attempt, reason } => {
                self.log(&msg::replay_attempt(
                    attempt,
                    watchdog::RETRY_LIMIT,
                    &reason,
                ));
                if let Some(recorder) = &self.recorder {
                    recorder.set_replay(true);
                }
                for (_, recorder) in &self.extras {
                    recorder.set_replay(true);
                }
                true
            }
            Action::Restart { attempt, reason } => {
                self.log(&msg::replay_attempt(
                    attempt,
                    watchdog::RETRY_LIMIT,
                    &reason,
                ));
                self.recorder = None;
                self.restart_recorder();
                true
            }
            Action::GiveUp(reason) => {
                self.log(&msg::replay_gave_up(watchdog::RETRY_LIMIT, &reason));
                self.toast(Icon::Replay, msg::REPLAY_FAILED, Some(reason), true, cx);
                false
            }
        }
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.paths.logs()
    }

    fn log(&self, line: &str) {
        watchdog::log(&self.paths.logs(), line);
    }

    fn restart_recorder(&mut self) {
        let config = recorder_config(&self.settings, &self.paths, self.active_profile.as_deref());
        self.sync_extras(&config);
        if self
            .recorder
            .as_ref()
            .is_some_and(|recorder| *recorder.config() == config)
        {
            return;
        }
        trace!(
            AREA,
            "restarting the recorder, profile {:?}, had a recorder {}",
            self.active_profile,
            self.recorder.is_some()
        );
        self.recorder = None;
        let signals = self.signals.clone();
        let recorder = Recorder::start(
            config,
            self.controls.clone(),
            self.settings.replay_enabled,
            move |event| {
                signals.unbounded_send(Signal::Recorder(event)).ok();
            },
        );
        recorder.set_monitor(self.settings.stats_visible);
        self.recorder = Some(recorder);
    }

    pub fn screen_choice(&self) -> bool {
        self.settings.choose_screen && self.screens.len() > 1
    }

    fn sync_extras(&mut self, base: &RecorderConfig) {
        self.screens = rewind_capture::screens();
        let wanted: Vec<Screen> = if self.screen_choice() {
            self.screens
                .iter()
                .copied()
                .filter(|screen| !screen.is_primary())
                .collect()
        } else {
            Vec::new()
        };
        let mut previous = std::mem::take(&mut self.extras);
        for screen in wanted {
            let config = RecorderConfig {
                screen: Some(screen),
                window_only: false,
                capture_desktop: true,
                ..base.clone()
            };
            let kept = previous
                .iter()
                .position(|(_, recorder)| *recorder.config() == config);
            let recorder = match kept {
                Some(index) => previous.swap_remove(index).1,
                None => {
                    let signals = self.signals.clone();
                    Recorder::start(
                        config,
                        self.controls.clone(),
                        self.settings.replay_enabled,
                        move |event| {
                            if matches!(
                                event,
                                RecorderEvent::ReplaySaved { .. }
                                    | RecorderEvent::ReplayFailed(_)
                                    | RecorderEvent::RecordingStarted
                                    | RecorderEvent::RecordingSaved { .. }
                            ) {
                                signals.unbounded_send(Signal::Recorder(event)).ok();
                            }
                        },
                    )
                }
            };
            self.extras.push((screen, recorder));
        }
        drop(previous);
        if self.recording_on.is_some() && self.extra(self.recording_on).is_none() {
            self.recording_on = None;
            self.recording = None;
        }
    }

    fn extra(&self, screen: Option<Screen>) -> Option<&Recorder> {
        let screen = screen?;
        self.extras
            .iter()
            .find(|(place, _)| *place == screen)
            .map(|(_, recorder)| recorder)
    }

    fn recorder_on(&self, screen: Option<Screen>) -> Option<&Recorder> {
        self.extra(screen).or(self.recorder.as_ref())
    }

    fn save(&self) {
        self.store
            .save(&self.settings)
            .traced(AREA, "save the settings")
            .ok();
    }

    pub fn change(&mut self, apply: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let before = (
            self.settings.bindings.clone(),
            self.settings.replay.markers,
            self.settings.youtube.offer_upload,
        );
        let profiles_before = self.settings.profiles.clone();
        let verbose_before = self.settings.verbose_logging;
        apply(&mut self.settings);
        if self.settings.verbose_logging != verbose_before {
            trace!(AREA, "verbose logging switched off");
            rewind_trace::set_enabled(self.settings.verbose_logging);
            trace!(
                AREA,
                "verbose logging switched on, Rewind {}, settings {:?}",
                env!("CARGO_PKG_VERSION"),
                self.settings
            );
            describe_system();
        } else {
            trace!(AREA, "settings changed");
        }
        self.controls.set_gains(
            self.settings.audio.system_gain(),
            self.settings.audio.mic_gain(),
        );
        *self.gate.lock() = gate_of(&self.settings);
        if (
            self.settings.bindings.clone(),
            self.settings.replay.markers,
            self.settings.youtube.offer_upload,
        ) != before
            && self.capturing.is_none()
        {
            self.rebind();
        }
        if self.settings.profiles != profiles_before {
            self.scan_profiles(cx);
        }
        self.apply_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(APPLY_DELAY).await;
            this.update(cx, |rewind, _| {
                rewind.save();
                rewind.restart_recorder();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn change_gallery_view(
        &mut self,
        apply: impl FnOnce(&mut rewind_config::Gallery),
        cx: &mut Context<Self>,
    ) {
        apply(&mut self.settings.gallery);
        self.save();
        cx.notify();
    }

    fn rebind(&self) {
        if let Some(hotkeys) = &self.hotkeys {
            hotkeys.rebind(global_hotkeys(&self.settings));
        }
    }

    pub fn begin_capture(&mut self, command: Command, cx: &mut Context<Self>) {
        self.capturing = Some(command);
        let signals = self.signals.clone();
        self.catcher = rewind_hotkey::KeyCatcher::start(move |hotkey| {
            signals.unbounded_send(Signal::Caught(hotkey)).ok();
        })
        .traced(AREA, "start catching a new hotkey")
        .ok();
        if let Some(hotkeys) = &self.hotkeys {
            hotkeys.rebind(Vec::new());
        }
        cx.notify();
    }

    pub fn finish_capture(&mut self, hotkey: Option<Option<Hotkey>>, cx: &mut Context<Self>) {
        self.catcher = None;
        let Some(command) = self.capturing.take() else {
            return;
        };
        if let Some(hotkey) = hotkey {
            self.settings.bindings.set(command, hotkey);
            self.save();
            *self.gate.lock() = gate_of(&self.settings);
        }
        self.rebind();
        cx.notify();
    }

    pub fn reset_bindings(&mut self, cx: &mut Context<Self>) {
        self.change(|settings| settings.bindings.reset(), cx);
    }

    pub fn replay_enabled(&self) -> bool {
        self.settings.replay_enabled
    }

    pub fn mic_enabled(&self) -> bool {
        self.controls.mic_enabled()
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn handle(&mut self, signal: Signal, cx: &mut Context<Self>) {
        trace!(AREA, "signal {signal:?}");
        match signal {
            Signal::Hotkey(HotkeyEvent::Pressed(command)) => self.run(command, cx),
            Signal::Hotkey(HotkeyEvent::Registered { taken }) => {
                if !taken.is_empty() && taken != self.taken {
                    let labels = taken
                        .iter()
                        .filter_map(|command| self.settings.bindings.get(*command))
                        .map(|hotkey| hotkey.label())
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.toast(
                        Icon::Keyboard,
                        msg::TAKEN,
                        Some(msg::taken(&labels)),
                        true,
                        cx,
                    );
                }
                self.taken = taken;
                cx.notify();
            }
            Signal::Recorder(event) => self.on_recorder(event, cx),
            Signal::Program(event) => self.on_program(event, cx),
            Signal::Caught(hotkey) => {
                if self.capturing.is_some() {
                    self.finish_capture(Some(Some(hotkey)), cx);
                }
            }
            Signal::Camera => self.camera_changed(cx),
            Signal::Moment(moment) => self.on_moment(moment, cx),
            Signal::Show => {
                rewind_capture::remember_application();
                cx.emit(UiEvent::ShowOverlay)
            }
            Signal::Quit => cx.quit(),
        }
    }

    pub fn run(&mut self, command: Command, cx: &mut Context<Self>) {
        match command {
            Command::ToggleOverlay => {
                rewind_capture::remember_application();
                cx.emit(UiEvent::ToggleOverlay)
            }
            Command::ToggleMic => self.toggle_mic(cx),
            Command::PushToTalk => {}
            Command::ToggleStats => self.toggle_stats(cx),
            Command::ToggleRecording => self.toggle_recording(cx),
            Command::ToggleReplay => self.toggle_replay(cx),
            Command::SaveReplay => self.save_replay(cx),
            Command::Screenshot if self.settings.screenshots.custom => self.start_snip(cx),
            Command::Screenshot => self.screenshot(),
            Command::PublishScreenshot => {
                self.publish_next = self.recorder.is_some();
                self.screenshot();
            }
            Command::Mark => self.mark(cx),
            Command::CopyLastClip => self.copy_last_clip(cx),
            Command::UploadReplay => self.upload_last_replay(cx),
            Command::Filters => {
                rewind_capture::remember_application();
                cx.emit(UiEvent::ShowFilters)
            }
            Command::PhotoMode => {
                rewind_capture::remember_application();
                cx.emit(UiEvent::ShowPhotoMode)
            }
            Command::StyleOff => self.hotkey_style(None, cx),
            Command::Style1 => self.hotkey_style(Some(0), cx),
            Command::Style2 => self.hotkey_style(Some(1), cx),
            Command::Style3 => self.hotkey_style(Some(2), cx),
        }
    }

    pub fn mark(&mut self, cx: &mut Context<Self>) {
        if !self.settings.replay.markers {
            return;
        }
        if !self.settings.replay_enabled {
            let detail = self.hotkey_label(Command::ToggleReplay);
            self.toast(Icon::Replay, msg::REPLAY_DISABLED, detail, false, cx);
            return;
        }
        let Some(recorder) = &self.recorder else {
            return;
        };
        if !recorder.mark() {
            return;
        }
        let count = recorder
            .replay_window()
            .map_or(1, |window| window.marks.len());
        let detail = msg::marked(count, self.hotkey_label(Command::SaveReplay));
        self.toast(Icon::Replay, msg::MARKED, Some(detail), false, cx);
    }

    pub fn start_snip(&mut self, cx: &mut Context<Self>) {
        if self.snipping {
            return;
        }
        self.snipping = true;
        rewind_capture::remember_application();
        cx.spawn(async move |this, cx| {
            let screens = cx
                .background_executor()
                .spawn(async move { crate::platform::screen::freeze_all() })
                .await;
            this.update(cx, |rewind, cx| {
                if screens.is_empty() {
                    rewind.snipping = false;
                    rewind.show_toast(Icon::Screenshot, msg::SNIP_FAILED, None, true, cx);
                    return;
                }
                let state = cx.entity();
                let session = cx.new(|_| crate::views::snip::SnipSession {
                    mode: crate::views::snip::SnipMode::Area,
                    owner: None,
                    windows: Vec::new(),
                    copy_on_select: rewind.settings.screenshots.copy_on_select,
                    host: rewind.settings.sharing.host.label(),
                });
                let mut windows = Vec::new();
                for (index, frozen) in screens.into_iter().enumerate() {
                    let (state, session) = (state.clone(), session.clone());
                    let monitor = frozen.monitor;
                    let place = cx
                        .displays()
                        .into_iter()
                        .map(|display| display.bounds())
                        .find(|candidate| {
                            (f32::from(candidate.origin.x) - monitor.left as f32).abs() < 2.0
                                && (f32::from(candidate.origin.y) - monitor.top as f32).abs() < 2.0
                        })
                        .unwrap_or_else(|| {
                            gpui::Bounds::new(
                                gpui::point(
                                    gpui::px(monitor.left as f32),
                                    gpui::px(monitor.top as f32),
                                ),
                                gpui::size(
                                    gpui::px((monitor.right - monitor.left) as f32),
                                    gpui::px((monitor.bottom - monitor.top) as f32),
                                ),
                            )
                        });
                    let frozen = Arc::new(frozen);
                    if let Ok(handle) =
                        cx.open_window(crate::views::snip::snip_options(place), |window, cx| {
                            cx.new(|cx| {
                                crate::views::snip::Snip::new(
                                    state, session, frozen, index, window, cx,
                                )
                            })
                        })
                    {
                        windows.push(handle.into());
                    }
                }
                if windows.is_empty() {
                    rewind.snipping = false;
                    return;
                }
                session.update(cx, |session, _| session.windows = windows);
            })
            .ok();
        })
        .detach();
    }

    pub fn keep_snip(
        &mut self,
        (width, height, pixels): (u32, u32, Vec<u8>),
        action: crate::views::snip::SnipAction,
        cx: &mut Context<Self>,
    ) {
        let copied = matches!(action, crate::views::snip::SnipAction::Copy);
        if copied && !self.settings.screenshots.saves_selection() {
            self.copy_snip(width, height, pixels, cx);
            return;
        }
        let path = rewind_capture::screenshot_path(&self.gallery_dir());
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let written = cx
                .background_executor()
                .spawn(async move {
                    let _runtime = rewind_media::Runtime::enter();
                    rewind_media::write_png(
                        &target,
                        &rewind_media::Bgra {
                            width,
                            height,
                            stride: width * 4,
                            pixels: &pixels,
                        },
                        None,
                    )
                    .is_ok()
                })
                .await;
            this.update(cx, |rewind, cx| {
                if !written {
                    rewind.show_toast(Icon::Screenshot, msg::SNIP_FAILED, None, true, cx);
                    return;
                }
                rewind.after_save(cx);
                match action {
                    crate::views::snip::SnipAction::Copy => rewind.copy_image(path, None, cx),
                    crate::views::snip::SnipAction::Upload => rewind.publish_image(path, None, cx),
                    crate::views::snip::SnipAction::Save => rewind.show_toast(
                        Icon::Screenshot,
                        msg::SCREENSHOT_SAVED,
                        msg::saved_to(&path),
                        false,
                        cx,
                    ),
                    crate::views::snip::SnipAction::Edit => {
                        rewind_capture::remember_application();
                        cx.emit(UiEvent::EditPhoto(path))
                    }
                    crate::views::snip::SnipAction::Pin => {}
                }
            })
            .ok();
        })
        .detach();
    }

    fn copy_snip(&mut self, width: u32, height: u32, mut pixels: Vec<u8>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let png = cx
                .background_executor()
                .spawn(async move {
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    crate::views::picture::encode_png(pixels, width, height)
                })
                .await;
            this.update(cx, |rewind, cx| match png {
                Ok(png) => rewind.copy_image(PathBuf::new(), Some(png), cx),
                Err(_) => rewind.show_toast(Icon::Screenshot, msg::SNIP_FAILED, None, true, cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn upload_last_replay(&mut self, cx: &mut Context<Self>) {
        if !self.settings.youtube.offer_upload {
            return;
        }
        let Some(path) = self
            .last_replay
            .take()
            .filter(|(path, saved)| saved.elapsed() < OFFER_LIFETIME && path.exists())
            .map(|(path, _)| path)
        else {
            self.show_toast(Icon::YouTube, msg::NO_FRESH_REPLAY, None, false, cx);
            return;
        };
        let youtube = &self.settings.youtube;
        if youtube.upload_mode == rewind_config::UploadMode::Review {
            rewind_capture::remember_application();
            cx.emit(UiEvent::ShowOverlay);
            cx.emit(UiEvent::OpenVideo(path));
            return;
        }
        let Some(account) = self
            .youtube
            .accounts
            .active()
            .map(|account| account.id.clone())
        else {
            self.show_toast(Icon::YouTube, msg::SIGN_IN_FIRST, None, true, cx);
            return;
        };
        let mut settings = crate::views::editor::bulk_settings(&path, &youtube.specs);
        settings.privacy = match youtube.upload_privacy {
            rewind_config::UploadPrivacy::Unlisted => youtube::Privacy::Unlisted,
            rewind_config::UploadPrivacy::Private => youtube::Privacy::Private,
        };
        self.publish(account, settings, path, cx);
    }

    pub fn copy_last_clip(&mut self, cx: &mut Context<Self>) {
        if !self.settings.replay_enabled {
            let detail = self.hotkey_label(Command::ToggleReplay);
            self.toast(Icon::Replay, msg::REPLAY_DISABLED, detail, false, cx);
            return;
        }
        let Some(window) = self
            .recorder
            .as_ref()
            .and_then(|recorder| recorder.replay_window())
        else {
            return;
        };
        let length = i64::from(self.settings.sharing.copy_seconds) * 10_000_000;
        let started = self.recorder.as_ref().is_some_and(|recorder| {
            recorder.save_replay_ranges(vec![(window.end - length, window.end)], false)
        });
        if !started {
            return;
        }
        self.copy_pending += 1;
        self.show_toast(
            Icon::Copy,
            crate::clip_share::text::PREPARING,
            None,
            false,
            cx,
        );
    }

    fn share_saved_clip(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let target = self.settings.sharing.quick;
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { crate::clip_share::share_clip(&path, None, target, &|_| {}) })
                .await;
            this.update(cx, |rewind, cx| {
                let (title, detail, error) = crate::clip_share::outcome_toast(target, &outcome);
                rewind.show_toast(Icon::Copy, title, detail, error, cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn merge_clips(&mut self, clips: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(first) = clips.first() else {
            return;
        };
        let destination = merged_path(first);
        let count = clips.len();
        self.toast(Icon::Videocam, msg::merging(count), None, false, cx);
        cx.spawn(async move |this, cx| {
            let target = destination.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let _runtime = rewind_media::Runtime::enter();
                    rewind_media::concat(&clips, &target, &|_| {})
                })
                .await;
            this.update(cx, |rewind, cx| {
                match outcome {
                    Ok(()) => rewind.toast(
                        Icon::Videocam,
                        msg::MERGED,
                        msg::saved_to(&destination),
                        false,
                        cx,
                    ),
                    Err(error) => {
                        trace!(AREA, "FAILED join clips into {destination:?}: {error:?}");
                        std::fs::remove_file(&destination).ok();
                        rewind.toast(
                            Icon::Videocam,
                            msg::MERGE_FAILED,
                            Some(error.to_string()),
                            true,
                            cx,
                        )
                    }
                }
                rewind.refresh_gallery_later(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn previews(&self) -> Option<rewind_capture::Previews> {
        self.recorder_on(self.replay_target)
            .map(rewind_capture::Recorder::previews)
    }

    pub fn save_replay_ranges(&mut self, ranges: Vec<(i64, i64)>, cx: &mut Context<Self>) {
        let started = self
            .recorder_on(self.replay_target)
            .is_some_and(|recorder| recorder.save_replay_ranges(ranges, true));
        if started {
            self.replay_save_started(cx);
        }
    }

    pub fn replay_saving(&self) -> bool {
        self.recorder
            .iter()
            .chain(self.extras.iter().map(|(_, recorder)| recorder))
            .any(rewind_capture::Recorder::is_saving)
    }

    fn replay_save_started(&mut self, cx: &mut Context<Self>) {
        if self.settings.notifications.enabled && self.settings.notifications.replay_saved {
            let slot = self.replay_toast.take();
            self.replay_toast =
                Some(self.replace_toast(slot, Icon::Replay, msg::REPLAY_SAVING, None, cx));
        }
        cx.notify();
    }

    pub fn toggle_mic(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.controls.mic_enabled();
        self.controls.set_mic_enabled(enabled);
        let (icon, text) = if enabled {
            (Icon::Mic, msg::MIC_ON)
        } else {
            (Icon::MicOff, msg::MIC_OFF)
        };
        let detail = self.hotkey_label(Command::ToggleMic);
        self.toast(icon, text, detail, false, cx);
        cx.notify();
    }

    pub fn toggle_stats(&mut self, cx: &mut Context<Self>) {
        let visible = !self.settings.stats_visible;
        self.change(|settings| settings.stats_visible = visible, cx);
        if let Some(recorder) = &self.recorder {
            recorder.set_monitor(visible);
        }
    }

    pub fn toggle_recording(&mut self, cx: &mut Context<Self>) {
        self.toggle_recording_on(None, cx);
    }

    pub fn toggle_recording_on(&mut self, screen: Option<Screen>, cx: &mut Context<Self>) {
        let stopping = self.recording.is_some();
        let target = if stopping {
            self.recording_on
        } else {
            screen.filter(|screen| self.extra(Some(*screen)).is_some())
        };
        if let Some(recorder) = self.recorder_on(target) {
            recorder.toggle_recording();
            let elapsed = recorder.recording_elapsed();
            self.recording = elapsed;
            self.recording_on = if stopping { None } else { target };
            if stopping
                && self.settings.notifications.enabled
                && self.settings.notifications.recording_saved
            {
                let slot = self.recording_toast.take();
                self.recording_toast = Some(self.replace_toast(
                    slot,
                    Icon::Record,
                    msg::RECORDING_STOPPED,
                    Some(msg::RECORDING_SAVING.to_owned()),
                    cx,
                ));
            }
            cx.notify();
        }
    }

    pub fn toggle_replay(&mut self, cx: &mut Context<Self>) {
        let enabled = !self.settings.replay_enabled;
        self.settings.replay_enabled = enabled;
        self.save();
        if let Some(recorder) = &self.recorder {
            recorder.set_replay(enabled);
        }
        for (_, recorder) in &self.extras {
            recorder.set_replay(enabled);
        }
        if self.settings.notifications.replay_toggled {
            let (text, detail) = if enabled {
                let seconds = self.settings.video.replay_seconds;
                let detail = self
                    .hotkey_label(Command::SaveReplay)
                    .map(|hotkey| msg::save_hint(&hotkey, seconds));
                (msg::REPLAY_ON, detail)
            } else {
                (msg::REPLAY_OFF, self.hotkey_label(Command::ToggleReplay))
            };
            self.toast(Icon::Replay, text, detail, false, cx);
        }
        cx.notify();
    }

    pub fn save_replay(&mut self, cx: &mut Context<Self>) {
        self.save_replay_on(None, cx);
    }

    pub fn save_replay_on(&mut self, screen: Option<Screen>, cx: &mut Context<Self>) {
        self.replay_target = screen.filter(|screen| self.extra(Some(*screen)).is_some());
        if !self.settings.replay_enabled {
            let detail = self.hotkey_label(Command::ToggleReplay);
            self.toast(Icon::Replay, msg::REPLAY_DISABLED, detail, false, cx);
            return;
        }
        let Some(recorder) = self.recorder_on(self.replay_target) else {
            return;
        };
        let saving = &self.settings.replay;
        if (saving.choose_range || saving.markers)
            && let Some(window) = recorder.replay_window()
            && (saving.choose_range || !window.marks.is_empty())
        {
            rewind_capture::remember_application();
            cx.emit(UiEvent::PickReplay(window));
            return;
        }
        if recorder.save_replay() {
            self.replay_save_started(cx);
        }
    }

    pub fn screenshot(&mut self) {
        if let Some(recorder) = &self.recorder {
            recorder.screenshot();
        }
    }

    fn on_recorder(&mut self, event: RecorderEvent, cx: &mut Context<Self>) {
        let notifications = self.settings.notifications.clone();
        match event {
            RecorderEvent::Encoder(name) => {
                self.encoder = Some(name);
                self.last_error = None;
            }
            RecorderEvent::ReplaySaved { path, thumbnail } if !self.moment_pending.is_empty() => {
                self.after_save(cx);
                self.moment_saved(path, thumbnail, cx);
            }
            RecorderEvent::ReplaySaved { path, .. } if self.copy_pending > 0 => {
                self.copy_pending -= 1;
                self.after_save(cx);
                self.share_saved_clip(path, cx);
            }
            RecorderEvent::ReplaySaved { path, .. } => {
                self.after_save(cx);
                let offer = self.settings.youtube.offer_upload;
                if offer {
                    self.last_replay = Some((path.clone(), std::time::Instant::now()));
                    let detail = self
                        .hotkey_label(Command::UploadReplay)
                        .map(|hotkey| msg::offer_upload(&hotkey));
                    let slot = self.replay_toast.take();
                    self.replace_toast(slot, Icon::YouTube, msg::REPLAY_SAVED_OFFER, detail, cx);
                } else if notifications.enabled && notifications.replay_saved {
                    let slot = self.replay_toast.take();
                    self.replace_toast(
                        slot,
                        Icon::Replay,
                        msg::REPLAY_SAVED,
                        msg::saved_to(&path),
                        cx,
                    );
                }
            }
            RecorderEvent::ReplayFailed(reason) => {
                self.copy_pending = 0;
                self.moment_pending.clear();
                self.replay_toast = None;
                self.log(&msg::capture_failed(&reason));
                self.toast(Icon::Close, msg::REPLAY_SAVE_FAILED, Some(reason), true, cx);
            }
            RecorderEvent::RecordingStarted => {
                if notifications.recording_started {
                    let detail = self
                        .hotkey_label(Command::ToggleRecording)
                        .map(|hotkey| msg::stop_hint(&hotkey));
                    self.recording_toast =
                        self.toast_id(Icon::Record, msg::RECORDING_STARTED, detail, false, cx);
                }
            }
            RecorderEvent::RecordingSaved { path, .. } => {
                self.recording = None;
                self.recording_on = None;
                self.after_save(cx);
                let slot = self.recording_toast.take();
                if notifications.enabled && notifications.recording_saved {
                    self.replace_toast(
                        slot,
                        Icon::Record,
                        msg::RECORDING_SAVED,
                        msg::saved_to(&path),
                        cx,
                    );
                }
            }
            RecorderEvent::ScreenshotSaved(path) => {
                self.refresh_gallery_later(cx);
                if std::mem::take(&mut self.publish_next) {
                    self.publish_image(path, None, cx);
                } else if notifications.screenshot_saved {
                    self.toast(
                        Icon::Screenshot,
                        msg::SCREENSHOT_SAVED,
                        msg::saved_to(&path),
                        false,
                        cx,
                    );
                }
            }
            RecorderEvent::Failed(reason) => {
                self.publish_next = false;
                if self.last_error.as_deref() != Some(reason.as_str()) {
                    self.log(&msg::capture_failed(&reason));
                    self.toast(Icon::Close, msg::FAILED, Some(reason.clone()), true, cx);
                    self.last_error = Some(reason);
                }
            }
        }
        cx.notify();
    }

    fn after_save(&mut self, cx: &mut Context<Self>) {
        let files = &self.settings.files;
        let limit = files
            .disk_limit
            .then(|| u64::from(files.disk_limit_gb) * BYTES_PER_GB);
        self.request_gallery_scan(limit, TICK, cx);
    }

    pub fn set_replacement(&mut self, install: bool, cx: &mut Context<Self>) {
        if self.replacement_busy {
            return;
        }
        self.replacement_busy = true;
        cx.notify();
        let flag = if install {
            system::INSTALL_FLAG
        } else {
            system::UNINSTALL_FLAG
        };
        cx.spawn(async move |this, cx| {
            let succeeded = cx
                .background_executor()
                .spawn(async move { system::run_elevated(flag).unwrap_or(false) })
                .await;
            this.update(cx, |rewind, cx| {
                rewind.replacement_busy = false;
                rewind.replaced = system::is_installed();
                let title = match (succeeded, install) {
                    (true, true) => msg::REPLACED,
                    (true, false) => msg::RESTORED,
                    (false, _) => msg::REPLACE_FAILED,
                };
                rewind.toast(Icon::Settings, title, None, !succeeded, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn forget_thumbnail(&self, clip: &Path) {
        if let Some(thumbnail) = gallery::thumbnail_path(&self.paths.thumbnails(), clip) {
            std::fs::remove_file(thumbnail).ok();
        }
    }

    pub fn uploading(&self, path: &Path) -> bool {
        self.youtube
            .queue
            .tasks
            .iter()
            .any(|task| task.state.is_active() && task.source == path)
    }

    pub fn delete_clips(&mut self, clips: Vec<PathBuf>, cx: &mut Context<Self>) {
        let (busy, doomed): (Vec<PathBuf>, Vec<PathBuf>) =
            clips.into_iter().partition(|clip| self.uploading(clip));
        let attempted = doomed.len();
        cx.spawn(async move |this, cx| {
            let gone = cx
                .background_executor()
                .spawn(async move {
                    platform::shell::trash(&doomed);
                    doomed
                        .into_iter()
                        .filter(|clip| !clip.exists())
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |rewind, cx| {
                for clip in &gone {
                    rewind.forget_thumbnail(clip);
                }
                rewind.gallery.retain(|item| !gone.contains(&item.path));
                rewind.gallery_generation += 1;
                let held = attempted - gone.len();
                let detail = Some(if held > 0 && !gone.is_empty() {
                    msg::held_elsewhere(held)
                } else if held > 0 {
                    msg::HELD_ELSEWHERE.to_owned()
                } else if busy.is_empty() {
                    msg::restorable(gone.len()).to_owned()
                } else {
                    msg::kept_uploading(busy.len())
                });
                if gone.is_empty() && attempted == 0 {
                    rewind.show_toast(
                        Icon::YouTube,
                        msg::kept_uploading(busy.len()),
                        None,
                        false,
                        cx,
                    );
                } else if gone.is_empty() {
                    rewind.show_toast(Icon::Delete, msg::DELETE_FAILED, detail, true, cx);
                } else {
                    rewind.toast(Icon::Delete, msg::deleted(gone.len()), detail, false, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_gallery_later(&mut self, cx: &mut Context<Self>) {
        self.request_gallery_scan(None, TICK, cx);
    }

    fn request_gallery_scan(
        &mut self,
        limit: Option<u64>,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        if let Some(limit) = self.gallery_scan.request(limit) {
            self.run_gallery_scan(limit, delay, cx);
        }
    }

    fn run_gallery_scan(&self, limit: Option<u64>, delay: Duration, cx: &mut Context<Self>) {
        let gallery_dir = self.settings.files.gallery.clone();
        let thumbnails = self.paths.thumbnails();
        let cache = self.gallery_cache.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let items = cx
                .background_executor()
                .spawn(async move {
                    if let Some(limit) = limit {
                        gallery::enforce_limit(&gallery_dir, &thumbnails, limit);
                    }
                    gallery::scan(&gallery_dir, &thumbnails, &mut cache.lock())
                })
                .await;
            this.update(cx, |rewind, cx| {
                rewind.gallery = items;
                rewind.gallery_generation += 1;
                cx.notify();
                if let Some(limit) = rewind.gallery_scan.finish() {
                    rewind.run_gallery_scan(limit, TICK, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_microphones(&mut self, cx: &mut Context<Self>) {
        self.microphones = rewind_capture::microphones();
        cx.notify();
    }

    pub fn gallery_dir(&self) -> PathBuf {
        self.settings.files.gallery.clone()
    }

    fn hotkey_label(&self, command: Command) -> Option<String> {
        self.settings
            .bindings
            .get(command)
            .map(|hotkey| hotkey.label())
    }

    pub fn toast(
        &mut self,
        icon: Icon,
        title: impl Into<String>,
        detail: Option<String>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.toast_id(icon, title, detail, error, cx);
    }

    fn toast_id(
        &mut self,
        icon: Icon,
        title: impl Into<String>,
        detail: Option<String>,
        error: bool,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        if !self.settings.notifications.enabled && !error {
            return None;
        }
        Some(self.show_toast_id(icon, title, detail, error, cx))
    }

    pub fn show_toast(
        &mut self,
        icon: Icon,
        title: impl Into<String>,
        detail: Option<String>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.show_toast_id(icon, title, detail, error, cx);
    }

    fn show_toast_id(
        &mut self,
        icon: Icon,
        title: impl Into<String>,
        detail: Option<String>,
        error: bool,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.toast_seq += 1;
        let id = self.toast_seq;
        let title = title.into();
        trace!(
            AREA,
            "notification shown, error {error}, title {title:?}, detail {detail:?}"
        );
        if self.toasts.len() >= TOAST_LIMIT {
            self.toasts.remove(0);
        }
        self.toasts.push(Toast {
            id,
            icon,
            title,
            detail,
            error,
            leaving: false,
            stamp: id,
        });
        cx.notify();
        self.expire_toast(id, id, cx);
        id
    }

    pub fn replace_toast(
        &mut self,
        slot: Option<u64>,
        icon: Icon,
        title: impl Into<String>,
        detail: Option<String>,
        cx: &mut Context<Self>,
    ) -> u64 {
        let shown = slot.and_then(|id| {
            self.toasts
                .iter()
                .position(|toast| toast.id == id && !toast.leaving)
        });
        let Some(index) = shown else {
            return self.show_toast_id(icon, title, detail, false, cx);
        };
        self.toast_seq += 1;
        let stamp = self.toast_seq;
        let toast = &mut self.toasts[index];
        toast.icon = icon;
        toast.title = title.into();
        toast.detail = detail;
        toast.error = false;
        toast.stamp = stamp;
        let id = toast.id;
        cx.notify();
        self.expire_toast(id, stamp, cx);
        id
    }

    fn expire_toast(&mut self, id: u64, stamp: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_LIFETIME).await;
            let marked = this.update(cx, |rewind, cx| {
                let toast = rewind
                    .toasts
                    .iter_mut()
                    .find(|toast| toast.id == id && toast.stamp == stamp);
                let found = toast.is_some();
                if let Some(toast) = toast {
                    toast.leaving = true;
                    cx.notify();
                }
                found
            });
            if !matches!(marked, Ok(true)) {
                return;
            }
            cx.background_executor().timer(TOAST_EXIT).await;
            this.update(cx, |rewind, cx| {
                rewind.toasts.retain(|toast| toast.id != id);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn startup_hint(&mut self, cx: &mut Context<Self>) {
        if self.settings.notifications.overlay_toggled {
            let detail = self
                .hotkey_label(Command::ToggleOverlay)
                .map(|hotkey| msg::overlay_hint(&hotkey));
            self.toast(Icon::Keyboard, msg::READY, detail, false, cx);
        }
    }
}

pub fn create(
    paths: Paths,
    signals: UnboundedSender<Signal>,
    cx: &mut gpui::App,
) -> Entity<Rewind> {
    cx.new(|cx| Rewind::new(paths, signals, cx))
}

fn merged_path(first: &Path) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim_end_matches(".DVR").to_owned())
        .unwrap_or_default();
    (1..)
        .map(|index| {
            let suffix = if index == 1 {
                String::new()
            } else {
                format!(" {index}")
            };
            first.with_file_name(format!("{stem} — склейка{suffix}.mp4"))
        })
        .find(|path| !path.exists())
        .unwrap_or_else(|| first.with_file_name(format!("{stem} — склейка.mp4")))
}

fn global_hotkeys(settings: &Settings) -> Vec<(Command, Hotkey)> {
    settings
        .bindings
        .global()
        .into_iter()
        .filter(|(command, _)| *command != Command::Mark || settings.replay.markers)
        .filter(|(command, _)| *command != Command::UploadReplay || settings.youtube.offer_upload)
        .filter(|(command, _)| {
            !matches!(
                command,
                Command::Filters
                    | Command::PhotoMode
                    | Command::StyleOff
                    | Command::Style1
                    | Command::Style2
                    | Command::Style3
            ) || settings.filters.enabled
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_frame_rates_on_screen_count_as_a_stats_change() {
        let shown = Stats {
            display_fps: 60,
            encoded_fps: 60,
            ..Stats::default()
        };
        let hidden = Stats {
            dropped: 3,
            buffered: Duration::from_secs(40),
            ..shown
        };
        assert!(!shown_differs(&shown, &hidden));
        let slower = Stats {
            display_fps: 59,
            ..shown
        };
        assert!(shown_differs(&shown, &slower));
        let stalled = Stats {
            encoded_fps: 0,
            ..shown
        };
        assert!(shown_differs(&shown, &stalled));
    }

    #[test]
    fn delete_messages_agree_with_the_count() {
        assert_eq!(msg::deleted(1), "Файл удалён");
        assert_eq!(msg::deleted(2), "Было удалено 2 файла");
        assert_eq!(msg::deleted(5), "Было удалено 5 файлов");
        assert_eq!(msg::deleted(11), "Было удалено 11 файлов");
        assert_eq!(msg::deleted(21), "Был удалён 21 файл");
        assert_eq!(msg::shots_word(3), "снимка");
        assert_eq!(msg::shots_word(21), "снимок");
        assert_eq!(msg::shots_word(12), "снимков");
        assert_eq!(
            msg::kept_uploading(1),
            "1 файл сейчас загружается на YouTube — его не тронул"
        );
        assert_eq!(
            msg::kept_uploading(3),
            "3 файла сейчас загружаются на YouTube — их не тронул"
        );
    }
}
