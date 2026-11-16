use thiserror::Error;

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("failed to spawn hotkey thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("hotkey thread exited before it was ready")]
    NotReady,
}

pub type Result<T> = std::result::Result<T, HotkeyError>;
