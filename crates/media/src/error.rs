use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] windows::core::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("the file has no video stream")]
    NoVideo,
    #[error("the selected range is empty")]
    EmptyRange,
    #[error("the decoder produced no frame")]
    NoFrame,
    #[error("Для склейки нужно выбрать хотя бы два клипа")]
    TooFewClips,
    #[error("{0}")]
    Incompatible(String),
}

pub type Result<T> = std::result::Result<T, MediaError>;
