use std::env;
use std::path::PathBuf;

const APP_DIR: &str = "Rewind";
const GALLERY_DIR: &str = "NVIDIA";
const SETTINGS_FILE: &str = "settings.json";
const THUMBNAILS_DIR: &str = "thumbnails";
const YOUTUBE_DIR: &str = "youtube";
const LOGS_DIR: &str = "logs";
const UPLOADS_FILE: &str = "uploads.json";
const CAMERA_DIR: &str = "nvbus";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        Self {
            data: base.join(APP_DIR),
        }
    }

    pub fn settings(&self) -> PathBuf {
        self.data.join(SETTINGS_FILE)
    }

    pub fn thumbnails(&self) -> PathBuf {
        self.data.join(THUMBNAILS_DIR)
    }

    pub fn youtube(&self) -> PathBuf {
        self.data.join(YOUTUBE_DIR)
    }

    pub fn uploads(&self) -> PathBuf {
        self.data.join(UPLOADS_FILE)
    }

    pub fn logs(&self) -> PathBuf {
        self.data.join(LOGS_DIR)
    }

    pub fn camera(&self) -> PathBuf {
        self.data.join(CAMERA_DIR)
    }

    pub fn default_gallery() -> PathBuf {
        let home = env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        home.join("Videos").join(GALLERY_DIR)
    }

    pub fn default_temp() -> PathBuf {
        env::temp_dir()
    }
}
