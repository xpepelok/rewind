mod error;
#[cfg(windows)]
mod windows;

pub use error::{HotkeyError, Result};
#[cfg(windows)]
pub use windows::{HotkeyEvent, HotkeyService, KeyCatcher, is_held};
