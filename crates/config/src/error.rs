use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("corrupt settings file: {0}")]
    Corrupt(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ConfigError>;
