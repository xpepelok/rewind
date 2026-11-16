pub mod ansel;
pub mod config;
pub mod desk;
pub mod highlights;
pub mod session;
pub mod wire;

#[cfg(windows)]
mod camera;
#[cfg(windows)]
mod client;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod moments;

#[cfg(windows)]
pub use camera::Camera;
#[cfg(windows)]
pub use client::{Bus, Message, Transport};
#[cfg(windows)]
pub use host::Host;
#[cfg(windows)]
pub use install::{filters_enabled, set_filters_enabled};
#[cfg(windows)]
pub use moments::Moments;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Address {
    pub system: String,
    pub module: String,
}

impl Address {
    pub fn new(system: &str, module: &str) -> Self {
        Self {
            system: system.to_owned(),
            module: module.to_owned(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("компоненты NVIDIA не найдены: {0}")]
    Missing(&'static str),
    #[error("библиотека канала NVIDIA не подходит: {0}")]
    Library(&'static str),
    #[error("в папке канала лежит посторонний файл")]
    Foreign,
    #[error("имя участника канала содержит нулевой символ")]
    Name,
    #[error("канал NVIDIA не принял сообщение")]
    Rejected,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
