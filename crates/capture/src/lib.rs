mod error;
mod format;
mod ring;

#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod catalog;
#[cfg(windows)]
mod clock;
#[cfg(windows)]
mod com;
#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod device;
#[cfg(windows)]
mod discord;
#[cfg(windows)]
mod duplication;
#[cfg(windows)]
mod encoder;
#[cfg(windows)]
mod foreground;
#[cfg(windows)]
mod mux;
#[cfg(windows)]
mod naming;
#[cfg(windows)]
mod recorder;
#[cfg(windows)]
mod snapshot;
#[cfg(windows)]
mod window;

pub use error::{CaptureError, Result};
pub use format::VideoCodec;

#[cfg(windows)]
pub use audio::{AudioControls, microphones};
#[cfg(windows)]
pub use device::{Screen, screens};
#[cfg(windows)]
pub use encoder::hardware_encoder;
#[cfg(windows)]
pub use foreground::{
    ProgramEvent, application_path, remember_application, running_programs, watch_exit,
    watch_programs,
};
#[cfg(windows)]
pub use naming::{application_name, screenshot_path};
#[cfg(windows)]
pub use recorder::{
    AudioLayout, Previews, Recorder, RecorderConfig, RecorderEvent, ReplayHealth, ReplayWindow,
    RingPlan, Stats, free_memory, free_space, replay_bytes, ring_plan,
};
