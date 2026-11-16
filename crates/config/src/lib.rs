mod error;
mod filters;
mod hotkey;
mod moments;
mod paths;
mod settings;
mod store;

pub use error::{ConfigError, Result};
pub use filters::{Amount, FilterLayer, FilterValue, Filters, GameFilters, STYLE_SLOTS, game_key};
pub use hotkey::{Bindings, Command, Hotkey, Modifiers, key_name};
pub use moments::{GameMoments, Highlights, MomentKind};
pub use paths::Paths;
pub use settings::{
    Audio, BITRATE_MAX_MBPS, BITRATE_MIN_MBPS, COPY_SECONDS_MAX, COPY_SECONDS_MIN, Captured, Codec,
    Corner, DISK_LIMIT_MAX_GB, DISK_LIMIT_MIN_GB, Files, FontColor, FontSize, FrameRate, Gallery,
    GalleryKind, GallerySort, GameProfile, Interface, MARK_SPAN_MAX_SECONDS, MARK_SPAN_MIN_SECONDS,
    MicMode, Notifications, OverlayPart, Profiles, Quality, QuickShare, REPLAY_MAX_SECONDS,
    REPLAY_MIN_SECONDS, REPLAY_STEPS, ReplaySaving, Resolution, Screenshots, Settings, ShareHost,
    Sharing, Specs, StatsLayout, Theme, Tracks, UploadMode, UploadPrivacy, VOLUME_MAX, Video,
    YouTube, replay_step,
};
pub use store::Store;
