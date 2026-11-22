use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::hotkey::Bindings;
use crate::paths::Paths;

pub const REPLAY_MIN_SECONDS: u32 = 15;
pub const REPLAY_MAX_SECONDS: u32 = 24 * 60 * 60;
pub const REPLAY_STEPS: [u32; 26] = [
    15,
    30,
    45,
    60,
    90,
    2 * 60,
    3 * 60,
    4 * 60,
    5 * 60,
    7 * 60,
    10 * 60,
    15 * 60,
    20 * 60,
    30 * 60,
    45 * 60,
    60 * 60,
    90 * 60,
    2 * 60 * 60,
    3 * 60 * 60,
    4 * 60 * 60,
    6 * 60 * 60,
    8 * 60 * 60,
    10 * 60 * 60,
    12 * 60 * 60,
    18 * 60 * 60,
    24 * 60 * 60,
];

pub fn replay_step(seconds: u32) -> usize {
    REPLAY_STEPS
        .iter()
        .position(|step| *step >= seconds)
        .unwrap_or(REPLAY_STEPS.len() - 1)
}
pub const BITRATE_MIN_MBPS: u32 = 1;
pub const BITRATE_MAX_MBPS: u32 = 130;
pub const VOLUME_MAX: u8 = 100;
pub const DISK_LIMIT_MIN_GB: u32 = 5;
pub const DISK_LIMIT_MAX_GB: u32 = 2000;

macro_rules! cycle {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label),+
                }
            }

            pub fn step(self, forward: bool) -> Self {
                let index = Self::ALL.iter().position(|item| *item == self).unwrap_or(0);
                let count = Self::ALL.len();
                let next = if forward { (index + 1) % count } else { (index + count - 1) % count };
                Self::ALL[next]
            }
        }
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    Low,
    #[default]
    Medium,
    High,
    Custom,
}

cycle!(Quality { Low => "Низкое", Medium => "Среднее", High => "Высокое", Custom => "Пользовательское" });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Native,
    P720,
    #[default]
    P1080,
    P1440,
    P2160,
}

cycle!(Resolution { Native => "В игре", P720 => "720p HD", P1080 => "1080p HD", P1440 => "1440p QHD", P2160 => "2160p 4K" });

impl Resolution {
    pub const fn height(self) -> Option<u32> {
        match self {
            Self::Native => None,
            Self::P720 => Some(720),
            Self::P1080 => Some(1080),
            Self::P1440 => Some(1440),
            Self::P2160 => Some(2160),
        }
    }

    const fn pixel_weight(self) -> u32 {
        match self {
            Self::P720 => 4,
            Self::Native | Self::P1080 => 9,
            Self::P1440 => 16,
            Self::P2160 => 36,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameRate {
    #[default]
    Fps30,
    Fps60,
    Fps120,
    Auto,
}

cycle!(FrameRate { Fps30 => "30 FPS", Fps60 => "60 FPS", Fps120 => "120 FPS", Auto => "Как в игре" });

impl FrameRate {
    pub const fn value(self) -> u32 {
        match self {
            Self::Fps30 => 30,
            Self::Fps60 => 60,
            Self::Fps120 | Self::Auto => 120,
        }
    }

    pub const fn bitrate_rate(self) -> u32 {
        match self {
            Self::Auto => 60,
            other => other.value(),
        }
    }

    pub const fn is_variable(self) -> bool {
        matches!(self, Self::Auto)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    #[default]
    H264,
    Hevc,
    Av1,
}

cycle!(Codec { H264 => "H.264", Hevc => "HEVC", Av1 => "AV1" });

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub capture_desktop: bool,
    pub window_only: bool,
    pub replay_seconds: u32,
    pub quality: Quality,
    pub resolution: Resolution,
    pub frame_rate: FrameRate,
    pub codec: Codec,
    pub custom_bitrate_mbps: u32,
}

impl Default for Video {
    fn default() -> Self {
        Self {
            capture_desktop: true,
            window_only: false,
            replay_seconds: 5 * 60,
            quality: Quality::Medium,
            resolution: Resolution::P1080,
            frame_rate: FrameRate::Fps30,
            codec: Codec::H264,
            custom_bitrate_mbps: 9,
        }
    }
}

impl Video {
    pub fn bitrate_mbps(&self) -> u32 {
        let base = self.resolution.pixel_weight() * self.frame_rate.bitrate_rate();
        let scaled = match self.quality {
            Quality::Low => base / 54,
            Quality::Medium => base / 30,
            Quality::High => base / 12,
            Quality::Custom => self.custom_bitrate_mbps,
        };
        scaled.clamp(BITRATE_MIN_MBPS, BITRATE_MAX_MBPS)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicMode {
    #[default]
    AlwaysOn,
    PushToTalk,
    Off,
}

cycle!(MicMode { AlwaysOn => "Всегда вкл.", PushToTalk => "Push-to-talk", Off => "Выкл." });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tracks {
    #[default]
    Single,
    Separate,
}

cycle!(Tracks { Single => "Одна дорожка", Separate => "Раздельные дорожки" });

impl Tracks {
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Single => {
                "Создайте одну звуковую дорожку, объединяющую системные звуки и звук микрофона."
            }
            Self::Separate => {
                "Системные звуки, микрофон и Discord записываются на разные дорожки — голоса из Discord не попадают в звук игры."
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub system_volume: u8,
    pub mic_volume: u8,
    pub mic_boost: u8,
    pub mic_device: Option<String>,
    pub mic_mode: MicMode,
    pub tracks: Tracks,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            system_volume: VOLUME_MAX,
            mic_volume: VOLUME_MAX,
            mic_boost: 0,
            mic_device: None,
            mic_mode: MicMode::AlwaysOn,
            tracks: Tracks::Single,
        }
    }
}

impl Audio {
    pub fn mic_gain(&self) -> f32 {
        let volume = f32::from(self.mic_volume) / f32::from(VOLUME_MAX);
        let boost = 1.0 + 3.0 * f32::from(self.mic_boost) / f32::from(VOLUME_MAX);
        volume * boost
    }

    pub fn system_gain(&self) -> f32 {
        f32::from(self.system_volume) / f32::from(VOLUME_MAX)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    Off,
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

cycle!(Corner { Off => "Выкл.", TopLeft => "Вверху слева", TopRight => "Верхний правый", BottomLeft => "Внизу слева", BottomRight => "Внизу справа" });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatsLayout {
    #[default]
    Row,
    Column,
}

cycle!(StatsLayout { Row => "В одну строку", Column => "В столбец" });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Carbon,
    Ruby,
    Light,
    #[default]
    #[serde(other)]
    Classic,
}

cycle!(Theme { Classic => "Классика", Carbon => "Карбон", Ruby => "Рубин", Light => "Светлая" });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontSize {
    Small,
    #[default]
    Standard,
    Large,
}

cycle!(FontSize { Small => "Маленький", Standard => "Стандартный", Large => "Крупный" });

impl FontSize {
    pub const fn pixels(self) -> f32 {
        match self {
            Self::Small => 12.0,
            Self::Standard => 15.0,
            Self::Large => 20.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontColor {
    #[default]
    White,
    Green,
    Yellow,
}

cycle!(FontColor { White => "Белый", Green => "Зелёный", Yellow => "Жёлтый" });

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interface {
    pub stats_corner: Corner,
    pub stats_x: u8,
    pub stats_y: u8,
    pub stats_layout: StatsLayout,
    pub stats_opacity: u8,
    pub stats_font_size: FontSize,
    pub stats_font_color: FontColor,
    pub recording_indicator: Corner,
    pub replay_indicator: Corner,
    pub mic_indicator: Corner,
    pub theme: Theme,
}

impl Default for Interface {
    fn default() -> Self {
        Self {
            stats_corner: Corner::TopRight,
            stats_x: 100,
            stats_y: 0,
            stats_layout: StatsLayout::Row,
            stats_opacity: 0,
            stats_font_size: FontSize::Standard,
            stats_font_color: FontColor::White,
            recording_indicator: Corner::BottomRight,
            replay_indicator: Corner::BottomRight,
            mic_indicator: Corner::BottomRight,
            theme: Theme::Classic,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub enabled: bool,
    pub overlay_toggled: bool,
    pub recording_saved: bool,
    pub replay_saved: bool,
    pub screenshot_saved: bool,
    pub replay_toggled: bool,
    pub recording_started: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            overlay_toggled: true,
            recording_saved: true,
            replay_saved: true,
            screenshot_saved: true,
            replay_toggled: true,
            recording_started: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayPart {
    Panel,
    Notifications,
    Indicators,
    Stats,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Captured {
    pub enabled: bool,
    pub panel: bool,
    pub notifications: bool,
    pub indicators: bool,
    pub stats: bool,
}

impl Default for Captured {
    fn default() -> Self {
        Self {
            enabled: true,
            panel: true,
            notifications: true,
            indicators: true,
            stats: true,
        }
    }
}

impl Captured {
    pub fn shows(&self, part: OverlayPart) -> bool {
        self.enabled
            && match part {
                OverlayPart::Panel => self.panel,
                OverlayPart::Notifications => self.notifications,
                OverlayPart::Indicators => self.indicators,
                OverlayPart::Stats => self.stats,
            }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Files {
    pub gallery: PathBuf,
    pub temp: PathBuf,
    pub disk_limit: bool,
    pub disk_limit_gb: u32,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            gallery: Paths::default_gallery(),
            temp: Paths::default_temp(),
            disk_limit: false,
            disk_limit_gb: 100,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Specs {
    pub enabled: bool,
    pub gpu: bool,
    pub cpu: bool,
    pub memory: bool,
    pub display: bool,
    pub system: bool,
}

impl Default for Specs {
    fn default() -> Self {
        Self {
            enabled: false,
            gpu: true,
            cpu: true,
            memory: true,
            display: true,
            system: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShareHost {
    #[default]
    Catbox,
    ImgBb,
    Pixhost,
}

cycle!(ShareHost { Catbox => "Catbox", ImgBb => "ImgBB", Pixhost => "Pixhost" });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuickShare {
    #[default]
    Discord,
    DiscordNitro,
    Telegram,
}

cycle!(QuickShare { Discord => "Discord · 20 МБ", DiscordNitro => "Discord Nitro · 1 ГБ", Telegram => "Telegram · 2 ГБ" });

impl QuickShare {
    pub const fn limit_bytes(self) -> u64 {
        match self {
            Self::Discord => 20_000_000,
            Self::DiscordNitro => 1_000_000_000,
            Self::Telegram => 2_000_000_000,
        }
    }

    pub const fn app(self) -> &'static str {
        match self {
            Self::Discord | Self::DiscordNitro => "Discord",
            Self::Telegram => "Telegram",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharing {
    #[serde(deserialize_with = "lenient_host")]
    pub host: ShareHost,
    pub quick: QuickShare,
    #[serde(default = "default_copy_seconds")]
    pub copy_seconds: u32,
}

pub const COPY_SECONDS_MIN: u32 = 5;
pub const COPY_SECONDS_MAX: u32 = 120;

const fn default_copy_seconds() -> u32 {
    30
}

impl Default for Sharing {
    fn default() -> Self {
        Self {
            host: ShareHost::default(),
            quick: QuickShare::default(),
            copy_seconds: default_copy_seconds(),
        }
    }
}

fn lenient_host<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<ShareHost, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct YouTube {
    pub specs: Specs,
    pub offer_upload: bool,
    pub upload_mode: UploadMode,
    pub upload_privacy: UploadPrivacy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadMode {
    #[default]
    Review,
    Direct,
}

cycle!(UploadMode { Review => "Открыть окно заливки", Direct => "Сразу залить" });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadPrivacy {
    #[default]
    Unlisted,
    Private,
}

cycle!(UploadPrivacy { Unlisted => "По ссылке", Private => "Закрытый" });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GallerySort {
    #[default]
    Newest,
    Oldest,
    Largest,
    Smallest,
    Longest,
    Shortest,
    Name,
}

cycle!(GallerySort {
    Newest => "Сначала новые",
    Oldest => "Сначала старые",
    Largest => "Сначала большие",
    Smallest => "Сначала маленькие",
    Longest => "Сначала длинные",
    Shortest => "Сначала короткие",
    Name => "По названию",
});

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GalleryKind {
    #[default]
    All,
    Videos,
    Replays,
    Recordings,
    Screenshots,
}

cycle!(GalleryKind {
    All => "Всё",
    Videos => "Все видео",
    Replays => "Мгновенные повторы",
    Recordings => "Записи",
    Screenshots => "Снимки экрана",
});

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gallery {
    pub sort: GallerySort,
    pub kind: GalleryKind,
    pub game: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameProfile {
    pub name: String,
    pub executable: String,
    pub video: Video,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profiles {
    pub enabled: bool,
    pub games: Vec<GameProfile>,
}

impl Profiles {
    pub fn executables(&self) -> Vec<String> {
        self.games
            .iter()
            .filter(|_| self.enabled)
            .map(|game| game.executable.clone())
            .collect()
    }

    pub fn running(
        &self,
        running: &std::collections::HashMap<String, u32>,
    ) -> Option<(&GameProfile, u32)> {
        if !self.enabled {
            return None;
        }
        self.games.iter().find_map(|game| {
            running
                .get(&game.executable.to_lowercase())
                .map(|process| (game, *process))
        })
    }

    pub fn by_executable(&self, executable: &str) -> Option<&GameProfile> {
        self.games
            .iter()
            .filter(|_| self.enabled)
            .find(|game| game.executable.eq_ignore_ascii_case(executable))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Screenshots {
    pub custom: bool,
    pub copy_on_select: bool,
    pub save_on_select: bool,
}

impl Screenshots {
    pub const fn saves_selection(&self) -> bool {
        self.copy_on_select && self.save_on_select
    }
}

pub const MARK_SPAN_MIN_SECONDS: u32 = 5;
pub const MARK_SPAN_MAX_SECONDS: u32 = 5 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplaySaving {
    pub choose_range: bool,
    pub markers: bool,
    pub before_mark: u32,
    pub after_mark: u32,
}

impl Default for ReplaySaving {
    fn default() -> Self {
        Self {
            choose_range: false,
            markers: false,
            before_mark: 30,
            after_mark: 10,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub bindings: Bindings,
    pub video: Video,
    pub replay: ReplaySaving,
    pub profiles: Profiles,
    pub screenshots: Screenshots,
    pub audio: Audio,
    pub interface: Interface,
    pub notifications: Notifications,
    pub captured: Captured,
    pub files: Files,
    pub youtube: YouTube,
    pub sharing: Sharing,
    pub gallery: Gallery,
    pub filters: crate::filters::Filters,
    pub highlights: crate::moments::Highlights,
    pub replay_enabled: bool,
    pub stats_visible: bool,
    pub imported_nvidia: bool,
    pub choose_screen: bool,
    pub verbose_logging: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_enabled_profiles_name_their_games() {
        let mut profiles = Profiles {
            enabled: false,
            games: vec![GameProfile {
                name: "Cristalix".to_owned(),
                executable: "java.exe".to_owned(),
                video: Video::default(),
            }],
        };
        assert!(profiles.executables().is_empty());
        profiles.enabled = true;
        assert_eq!(profiles.executables(), ["java.exe"]);
    }

    #[test]
    fn a_selection_is_saved_only_together_with_copying_it() {
        let mut shots = Screenshots::default();
        assert!(!shots.saves_selection());
        shots.save_on_select = true;
        assert!(!shots.saves_selection());
        shots.copy_on_select = true;
        assert!(shots.saves_selection());
        let old: Screenshots =
            serde_json::from_str(r#"{"custom":true,"copy_on_select":true}"#).unwrap();
        assert!(!old.saves_selection());
    }

    #[test]
    fn a_theme_this_build_does_not_know_falls_back_to_the_classic_one() {
        let known: Theme = serde_json::from_str("\"Carbon\"").unwrap();
        let unknown: Theme = serde_json::from_str("\"Graphite\"").unwrap();
        assert_eq!(known, Theme::Carbon);
        assert_eq!(unknown, Theme::Classic);
    }

    #[test]
    fn nothing_of_the_overlay_is_captured_while_the_master_switch_is_off() {
        let parts = [
            OverlayPart::Panel,
            OverlayPart::Notifications,
            OverlayPart::Indicators,
            OverlayPart::Stats,
        ];
        let mut captured = Captured::default();
        assert!(parts.iter().all(|part| captured.shows(*part)));
        captured.enabled = false;
        assert!(parts.iter().all(|part| !captured.shows(*part)));
        captured.enabled = true;
        captured.stats = false;
        assert!(!captured.shows(OverlayPart::Stats));
        assert!(captured.shows(OverlayPart::Panel));
    }

    #[test]
    fn medium_1080p30_matches_shadowplay_default() {
        assert_eq!(Video::default().bitrate_mbps(), 9);
    }

    #[test]
    fn replay_steps_cover_the_whole_range_in_order() {
        assert_eq!(REPLAY_STEPS[0], REPLAY_MIN_SECONDS);
        assert_eq!(REPLAY_STEPS[REPLAY_STEPS.len() - 1], REPLAY_MAX_SECONDS);
        assert!(REPLAY_STEPS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(REPLAY_STEPS[replay_step(5 * 60)], 5 * 60);
        assert_eq!(REPLAY_STEPS[replay_step(100)], 2 * 60);
        assert_eq!(replay_step(u32::MAX), REPLAY_STEPS.len() - 1);
    }

    #[test]
    fn a_running_game_picks_its_profile_only_when_enabled() {
        let mut profiles = Profiles {
            enabled: false,
            games: vec![GameProfile {
                name: "Game".to_owned(),
                executable: "Game.exe".to_owned(),
                video: Video::default(),
            }],
        };
        let running = std::collections::HashMap::from([("game.exe".to_owned(), 42)]);
        assert!(profiles.running(&running).is_none());
        profiles.enabled = true;
        assert_eq!(
            profiles
                .running(&running)
                .map(|(game, process)| (game.name.as_str(), process)),
            Some(("Game", 42))
        );
        assert!(profiles.by_executable("GAME.EXE").is_some());
        assert!(
            profiles
                .running(&std::collections::HashMap::new())
                .is_none()
        );
    }

    #[test]
    fn bitrate_stays_in_bounds() {
        let video = Video {
            resolution: Resolution::P2160,
            frame_rate: FrameRate::Fps120,
            quality: Quality::High,
            ..Video::default()
        };
        assert_eq!(video.bitrate_mbps(), BITRATE_MAX_MBPS);
    }

    #[test]
    fn a_retired_share_host_falls_back_without_losing_settings() {
        let settings: Settings = serde_json::from_str(
            r#"{"replay_enabled":true,"sharing":{"host":"Imgur","imgur_client_id":"x"}}"#,
        )
        .unwrap();
        assert!(settings.replay_enabled);
        assert_eq!(settings.sharing.host, ShareHost::default());
    }

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(Quality::Low.step(false), Quality::Custom);
        assert_eq!(Quality::Custom.step(true), Quality::Low);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let settings: Settings = serde_json::from_str(r#"{"replay_enabled":true}"#).unwrap();
        assert!(settings.replay_enabled);
        assert_eq!(settings.video, Video::default());
    }
}
