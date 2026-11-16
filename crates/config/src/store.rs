use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use crate::error::Result;
use crate::settings::Settings;

const TEMP_SUFFIX: &str = "tmp";
const CORRUPT_SUFFIX: &str = "corrupt";
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Settings> {
        let raw = match fs::read(&self.path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_slice(raw.strip_prefix(BOM).unwrap_or(&raw)) {
            Ok(settings) => Ok(settings),
            Err(_) => {
                fs::rename(&self.path, self.path.with_extension(CORRUPT_SUFFIX))?;
                Ok(Settings::default())
            }
        }
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = self.path.with_extension(TEMP_SUFFIX);
        fs::write(&temp, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(&temp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("settings.json"));
        assert_eq!(store.load().unwrap(), Settings::default());
    }

    #[test]
    fn saved_settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("nested").join("settings.json"));
        let mut settings = Settings {
            replay_enabled: true,
            ..Settings::default()
        };
        settings.video.replay_seconds = 120;
        store.save(&settings).unwrap();
        assert_eq!(store.load().unwrap(), settings);
    }

    #[test]
    fn byte_order_mark_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut raw = BOM.to_vec();
        raw.extend_from_slice(br#"{"replay_enabled": true}"#);
        fs::write(&path, raw).unwrap();
        assert!(Store::new(path).load().unwrap().replay_enabled);
    }

    #[test]
    fn corrupt_file_is_kept_aside_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{not json").unwrap();
        let store = Store::new(path.clone());
        assert_eq!(store.load().unwrap(), Settings::default());
        assert_eq!(
            fs::read(path.with_extension("corrupt")).unwrap(),
            b"{not json"
        );
    }
}
