use std::path::{Path, PathBuf};

use rewind_config::ShareHost;
use serde::{Deserialize, Serialize};

const LIMIT: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upload {
    pub file: PathBuf,
    pub host: ShareHost,
    pub link: String,
    #[serde(default)]
    pub delete: Option<String>,
    pub uploaded_at: i64,
}

impl Upload {
    pub fn name(&self) -> String {
        self.file.file_name().map_or_else(
            || self.link.clone(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uploads {
    pub entries: Vec<Upload>,
}

impl Uploads {
    pub fn load(path: &Path) -> Self {
        let entries = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { entries }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(&self.entries).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, json)?;
        std::fs::rename(temporary, path)
    }

    pub fn add(&mut self, upload: Upload) {
        self.entries.insert(0, upload);
        self.entries.truncate(LIMIT);
    }

    pub fn remove(&mut self, link: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.link != link);
        before != self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(link: &str) -> Upload {
        Upload {
            file: PathBuf::from(r"C:\shots\a.png"),
            host: ShareHost::Catbox,
            link: link.to_owned(),
            delete: None,
            uploaded_at: 1,
        }
    }

    #[test]
    fn history_survives_a_restart_newest_first() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("uploads.json");
        let mut uploads = Uploads::default();
        uploads.add(upload("https://a"));
        uploads.add(upload("https://b"));
        uploads.save(&path).unwrap();
        let restored = Uploads::load(&path);
        assert_eq!(restored.entries[0].link, "https://b");
        assert_eq!(restored.entries.len(), 2);
        assert_eq!(restored.entries[1].name(), "a.png");
    }

    #[test]
    fn entries_can_be_forgotten_and_a_broken_file_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("uploads.json");
        std::fs::write(&path, b"{not json").unwrap();
        let mut uploads = Uploads::load(&path);
        assert!(uploads.entries.is_empty());
        uploads.add(upload("https://a"));
        assert!(uploads.remove("https://a"));
        assert!(!uploads.remove("https://a"));
    }
}
