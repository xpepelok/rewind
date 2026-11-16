use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] windows::core::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("no display output attached to the desktop")]
    NoOutput,
    #[error("no hardware video encoder on the capture adapter")]
    NoEncoder,
    #[error("no aac encoder available")]
    NoAudioEncoder,
    #[error("nothing buffered yet")]
    Empty,
    #[error("stream format is not known yet")]
    NoFormat,
}

pub type Result<T> = std::result::Result<T, CaptureError>;
