use std::path::PathBuf;
use std::time::Duration;

use gpui::Context;

use crate::platform::http::Session;
use crate::platform::{shell, system};
use crate::state::Rewind;

const API_HOST: &str = "api.github.com";
const LATEST_PATH: &str = "/repos/xpepelok/rewind/releases/latest";
const DOWNLOAD_HOST: &str = "github.com";
const DOWNLOAD_ORIGIN: &str = "https://github.com";
const DOWNLOAD_ROOT: &str = "/xpepelok/rewind/releases/download/";
pub const RELEASES_URL: &str = "https://github.com/xpepelok/rewind/releases/latest";
const AGENT: &str = "Rewind-Updater";
const FIRST_CHECK: Duration = Duration::from_secs(20);
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);
const MIN_SIZE: usize = 1 << 20;
const FOLDER: &str = "Rewind-update";
const FILE: &str = "Rewind.exe";

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    asset: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Idle,
    Downloading,
    Failed(String),
}

fn numbers(version: &str) -> Vec<u32> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().unwrap_or(0)
        })
        .collect()
}

pub fn newer(current: &str, candidate: &str) -> bool {
    let (mut current, mut candidate) = (numbers(current), numbers(candidate));
    let length = current.len().max(candidate.len());
    current.resize(length, 0);
    candidate.resize(length, 0);
    candidate > current
}

pub fn parse(json: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let flag = |name: &str| value.get(name).and_then(serde_json::Value::as_bool) == Some(true);
    if flag("draft") || flag("prerelease") {
        return None;
    }
    let tag = value.get("tag_name")?.as_str()?;
    let asset = value
        .get("assets")?
        .as_array()?
        .iter()
        .filter(|asset| {
            asset
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(".exe"))
        })
        .filter_map(|asset| asset.get("browser_download_url")?.as_str())
        .filter_map(|url| url.strip_prefix(DOWNLOAD_ORIGIN))
        .find(|path| path.starts_with(DOWNLOAD_ROOT))?;
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_owned(),
        asset: asset.to_owned(),
    })
}

fn check() -> Option<Release> {
    let response = Session::new(AGENT).ok()?.get(API_HOST, LATEST_PATH).ok()?;
    if response.status != 200 {
        return None;
    }
    parse(&response.body).filter(|release| newer(CURRENT, &release.version))
}

fn download(release: &Release) -> Result<PathBuf, String> {
    let response = Session::new(AGENT)?.get(DOWNLOAD_HOST, &release.asset)?;
    if response.status != 200 {
        return Err(format!("сервер ответил кодом {}", response.status));
    }
    if response.bytes.len() < MIN_SIZE || !response.bytes.starts_with(b"MZ") {
        return Err("скачанный файл повреждён".to_owned());
    }
    let folder = std::env::temp_dir().join(FOLDER);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let path = folder.join(FILE);
    std::fs::write(&path, &response.bytes).map_err(|error| error.to_string())?;
    Ok(path)
}

impl Rewind {
    pub fn start_update_watch(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut wait = FIRST_CHECK;
            loop {
                cx.background_executor().timer(wait).await;
                wait = RECHECK;
                let found = cx.background_executor().spawn(async { check() }).await;
                let alive = this.update(cx, |rewind, cx| {
                    if found.is_some() && rewind.release != found {
                        rewind.release = found;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.release.clone() else {
            return;
        };
        if self.update_stage == Stage::Downloading {
            return;
        }
        if !system::running_installed_copy() {
            shell::open_url(RELEASES_URL);
            return;
        }
        self.update_stage = Stage::Downloading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let path = download(&release)?;
                    system::apply_update(&path).map_err(|error| error.to_string())
                })
                .await;
            if let Err(error) = result {
                this.update(cx, |rewind, cx| {
                    rewind.update_stage = Stage::Failed(error);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_higher_version_counts_as_an_update() {
        assert!(newer("0.1.0", "v0.1.1"));
        assert!(newer("0.1.0", "0.2"));
        assert!(newer("0.9.9", "1.0.0"));
        assert!(newer("0.1", "0.1.0.1"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "v0.1"));
        assert!(!newer("0.2.0", "0.1.9"));
        assert!(newer("0.1.0", "0.1.1-beta"));
        assert!(!newer("0.1.0", "garbage"));
    }

    #[test]
    fn a_release_needs_an_exe_from_the_projects_own_downloads() {
        let release = parse(
            r#"{"tag_name":"v0.2.0","draft":false,"prerelease":false,"assets":[
                {"name":"notes.txt","browser_download_url":"https://github.com/xpepelok/rewind/releases/download/v0.2.0/notes.txt"},
                {"name":"Rewind.exe","browser_download_url":"https://github.com/xpepelok/rewind/releases/download/v0.2.0/Rewind.exe"}]}"#,
        )
        .expect("release");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.asset, "/xpepelok/rewind/releases/download/v0.2.0/Rewind.exe");

        let foreign = r#"{"tag_name":"v9.0.0","assets":[
            {"name":"Rewind.exe","browser_download_url":"https://example.com/Rewind.exe"},
            {"name":"Rewind.exe","browser_download_url":"https://github.com/someone/else/releases/download/v9/Rewind.exe"}]}"#;
        assert_eq!(parse(foreign), None);

        let draft = r#"{"tag_name":"v9.0.0","draft":true,"assets":[
            {"name":"Rewind.exe","browser_download_url":"https://github.com/xpepelok/rewind/releases/download/v9/Rewind.exe"}]}"#;
        assert_eq!(parse(draft), None);
        assert_eq!(parse(r#"{"tag_name":"v9.0.0","assets":[]}"#), None);
        assert_eq!(parse("not json"), None);
    }
}
