use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{ClipboardItem, Context};
use parking_lot::Mutex;

use super::Rewind;
use crate::assets::Icon;
use crate::platform::{clipboard, http};
use crate::uploads::Upload;
use crate::{gallery, share, watchdog};

const POLL: Duration = Duration::from_millis(100);
const FILE_WAIT: Duration = Duration::from_secs(10);
const FILE_POLL: Duration = Duration::from_millis(50);
const PNG_SIGNATURE: [u8; 4] = [0x89, b'P', b'N', b'G'];
const PNG_END: [u8; 8] = [0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];
const SHARE_LOG: &str = "uploads.log";
const EDITED_SUFFIX: &str = "изменено";
const SHARED_DIR: &str = "shared";
const PREVIEWS_DIR: &str = "previews";
const PREVIEWED: usize = 100;
const HTTP_OK: u32 = 200;

mod msg {
    pub const COPIED: &str = "Скопировано в буфер обмена";
    pub const COPY_FAILED: &str = "Не удалось скопировать картинку";
    pub const SAVED: &str = "Снимок сохранён";
    pub const COPY_SAVED: &str = "Копия снимка сохранена";
    pub const SAVE_FAILED: &str = "Не удалось сохранить снимок";
    pub const LINK_COPIED: &str = "ссылка скопирована";

    pub fn album_started(count: usize, host: &str) -> String {
        format!("Публикую снимки одним альбомом на {host}: {count}")
    }

    pub fn album_done(host: &str) -> String {
        format!("Альбом опубликован на {host}")
    }

    pub fn album_failed(host: &str) -> String {
        format!("Альбом не опубликован на {host}")
    }

    pub const IMGBB_FALLBACK: &str =
        "ImgBB не умеет альбомы без аккаунта, поэтому альбом собран на Catbox";
}

pub fn edited_path(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let directory = source.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    (1..)
        .map(|index| {
            let name = if index == 1 {
                format!("{stem} ({EDITED_SUFFIX}).png")
            } else {
                format!("{stem} ({EDITED_SUFFIX} {index}).png")
            };
            directory.join(name)
        })
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| directory.join(format!("{stem} ({EDITED_SUFFIX}).png")))
}

fn split_link(link: &str) -> Option<(&str, &str)> {
    let rest = link.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash..]))
}

fn cache_name(link: &str) -> String {
    let tail = split_link(link).map_or(link, |(_, path)| path);
    tail.trim_start_matches('/')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn download(link: &str, destination: &Path) -> Result<(), String> {
    let (host, path) = split_link(link).ok_or_else(|| link.to_owned())?;
    let response = http::Session::new(http::BROWSER_AGENT)?.get(host, path)?;
    if response.status != HTTP_OK || response.bytes.is_empty() {
        return Err(format!("HTTP {}", response.status));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(destination, &response.bytes).map_err(|error| error.to_string())
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "image.png".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn is_complete(bytes: &[u8]) -> bool {
    !bytes.is_empty() && (!bytes.starts_with(&PNG_SIGNATURE) || bytes.ends_with(&PNG_END))
}

fn bytes_of(path: &Path, png: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    if let Some(png) = png {
        return Ok(png);
    }
    let deadline = Instant::now() + FILE_WAIT;
    loop {
        let late = Instant::now() >= deadline;
        match std::fs::read(path) {
            Ok(bytes) if late || is_complete(&bytes) => return Ok(bytes),
            Err(error) if late => return Err(error.to_string()),
            _ => std::thread::sleep(FILE_POLL),
        }
    }
}

fn percent(value: f32) -> u32 {
    (value.clamp(0.0, 1.0) * 100.0).round() as u32
}

impl Rewind {
    pub fn share_progress(&self, path: &Path) -> Option<f32> {
        self.shares.get(path).copied()
    }

    fn shared_cache(&self, link: &str) -> PathBuf {
        self.paths.data.join(SHARED_DIR).join(cache_name(link))
    }

    fn upload_preview(&self, link: &str) -> PathBuf {
        self.paths
            .data
            .join(SHARED_DIR)
            .join(PREVIEWS_DIR)
            .join(format!("{}.png", cache_name(link)))
    }

    fn upload_source(&self, upload: &Upload) -> Option<PathBuf> {
        Some(upload.file.clone())
            .filter(|file| file.exists())
            .or_else(|| Some(self.shared_cache(&upload.link)).filter(|cache| cache.exists()))
    }

    pub fn previewing_uploads(&self) -> bool {
        self.previewing
    }

    pub fn upload_image(&self, upload: &Upload) -> Option<PathBuf> {
        Some(self.upload_preview(&upload.link)).filter(|preview| preview.exists())
    }

    pub fn ensure_upload_previews(&mut self, cx: &mut Context<Self>) {
        if self.previewing {
            return;
        }
        let missing: Vec<(String, Option<PathBuf>, PathBuf, PathBuf)> = self
            .uploads
            .entries
            .iter()
            .take(PREVIEWED)
            .filter(|upload| {
                !self.previews_tried.contains(&upload.link) && self.upload_image(upload).is_none()
            })
            .map(|upload| {
                (
                    upload.link.clone(),
                    self.upload_source(upload),
                    self.shared_cache(&upload.link),
                    self.upload_preview(&upload.link),
                )
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        self.previewing = true;
        self.previews_tried
            .extend(missing.iter().map(|(link, ..)| link.clone()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    for (link, source, cache, preview) in missing {
                        let source = match source {
                            Some(source) => Some(source),
                            None => download(&link, &cache).ok().map(|()| cache),
                        };
                        if let Some(source) = source {
                            gallery::write_preview(&source, &preview).ok();
                        }
                    }
                })
                .await;
            this.update(cx, |rewind, cx| {
                rewind.previewing = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn save_uploads(&self) {
        if let Err(error) = self.uploads.save(&self.paths.uploads()) {
            self.share_log(&format!("ОШИБКА истории: {error}"));
        }
    }

    pub fn forget_upload(&mut self, link: &str, cx: &mut Context<Self>) {
        if self.uploads.remove(link) {
            std::fs::remove_file(self.upload_preview(link)).ok();
            self.save_uploads();
            cx.notify();
        }
    }

    fn share_log(&self, line: &str) {
        watchdog::log_to(&self.paths.logs(), SHARE_LOG, line);
    }

    pub fn copy_image(&mut self, path: PathBuf, png: Option<Vec<u8>>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    bytes_of(&path, png).and_then(|bytes| clipboard::copy_image(&bytes))
                })
                .await;
            this.update(cx, |rewind, cx| match outcome {
                Ok(()) => rewind.toast(Icon::Photo, msg::COPIED, None, false, cx),
                Err(error) => rewind.toast(Icon::Photo, msg::COPY_FAILED, Some(error), true, cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn save_image(
        &mut self,
        source: PathBuf,
        png: Vec<u8>,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) -> PathBuf {
        let target = if overwrite {
            source.with_extension("png")
        } else {
            edited_path(&source)
        };
        let written = target.clone();
        cx.spawn(async move |this, cx| {
            let destination = written.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    std::fs::write(&destination, png).map_err(|error| error.to_string())
                })
                .await;
            this.update(cx, |rewind, cx| match outcome {
                Ok(()) => {
                    rewind.forget_thumbnail(&written);
                    let title = if overwrite {
                        msg::SAVED
                    } else {
                        msg::COPY_SAVED
                    };
                    rewind.toast(Icon::Photo, title, None, false, cx);
                    rewind.refresh_gallery_later(cx);
                }
                Err(error) => rewind.toast(Icon::Photo, msg::SAVE_FAILED, Some(error), true, cx),
            })
            .ok();
        })
        .detach();
        target
    }

    pub fn publish_album(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.len() == 1 {
            if let Some(path) = paths.into_iter().next() {
                self.publish_image(path, None, cx);
            }
            return;
        }
        if paths.is_empty() {
            return;
        }
        let wanted = self.settings.sharing.host;
        let host = share::album_host(wanted);
        let title = format!("Rewind {}", youtube::iso_date(youtube::now_unix()));
        self.toast(
            Icon::Upload,
            msg::album_started(paths.len(), host.label()),
            (host != wanted).then(|| msg::IMGBB_FALLBACK.to_owned()),
            false,
            cx,
        );
        let sources = paths.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let files = sources
                        .iter()
                        .map(|path| bytes_of(path, None).map(|bytes| (file_name(path), bytes)))
                        .collect::<Result<Vec<_>, String>>()?;
                    share::upload_album(wanted, &title, &files, &|_| {})
                })
                .await;
            this.update(cx, |rewind, cx| {
                match outcome {
                    Ok(album) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(album.link.clone()));
                        for (path, item) in paths.iter().zip(&album.items) {
                            rewind.uploads.add(Upload {
                                file: path.clone(),
                                host: album.host,
                                link: item.link.clone(),
                                delete: item.delete.clone(),
                                uploaded_at: youtube::now_unix(),
                            });
                        }
                        rewind.save_uploads();
                        rewind.ensure_upload_previews(cx);
                        rewind.share_log(&format!(
                            "ОК альбом {} {} снимков: {}",
                            album.host.label(),
                            album.items.len(),
                            album.link
                        ));
                        rewind.toast(
                            Icon::Upload,
                            msg::album_done(album.host.label()),
                            Some(format!("{} — {}", album.link, msg::LINK_COPIED)),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        rewind.share_log(&format!("ОШИБКА альбом {}: {error}", host.label()));
                        rewind.toast(
                            Icon::Upload,
                            msg::album_failed(host.label()),
                            Some(error),
                            true,
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn publish_image(&mut self, path: PathBuf, png: Option<Vec<u8>>, cx: &mut Context<Self>) {
        if self.shares.contains_key(&path) {
            return;
        }
        let host = self.settings.sharing.host;
        let name = file_name(&path);
        let published = format!("Скриншот опубликован на {}", host.label());
        let not_published = format!("Скриншот не опубликован на {}", host.label());
        self.shares.insert(path.clone(), 0.0);
        cx.notify();
        let progress = Arc::new(Mutex::new(0.0f32));
        let result: Arc<Mutex<Option<Result<share::Uploaded, String>>>> =
            Arc::new(Mutex::new(None));
        let (worker_progress, worker_result) = (progress.clone(), result.clone());
        let source = path.clone();
        let upload_name = name.clone();
        cx.background_executor()
            .spawn(async move {
                let outcome = bytes_of(&source, png).and_then(|bytes| {
                    share::upload(host, &upload_name, &bytes, &|value| {
                        *worker_progress.lock() = value;
                    })
                });
                *worker_result.lock() = Some(outcome);
            })
            .detach();
        cx.spawn(async move |this, cx| {
            let mut shown = percent(0.0);
            let outcome = loop {
                cx.background_executor().timer(POLL).await;
                if let Some(outcome) = result.lock().take() {
                    break outcome;
                }
                let value = *progress.lock();
                if percent(value) == shown {
                    continue;
                }
                shown = percent(value);
                let alive = this.update(cx, |rewind, cx| {
                    rewind.shares.insert(path.clone(), value);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            };
            this.update(cx, |rewind, cx| {
                rewind.shares.remove(&path);
                match outcome {
                    Ok(uploaded) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(uploaded.link.clone()));
                        rewind.uploads.add(Upload {
                            file: path.clone(),
                            host,
                            link: uploaded.link.clone(),
                            delete: uploaded.delete.clone(),
                            uploaded_at: youtube::now_unix(),
                        });
                        rewind.save_uploads();
                        rewind.ensure_upload_previews(cx);
                        rewind.share_log(&format!(
                            "ОК {} {name} {} удаление={}",
                            host.label(),
                            uploaded.link,
                            uploaded.delete.as_deref().unwrap_or("-")
                        ));
                        rewind.toast(
                            Icon::Upload,
                            published,
                            Some(format!("{} — {}", uploaded.link, msg::LINK_COPIED)),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        rewind.share_log(&format!("ОШИБКА {} {name}: {error}", host.label()));
                        rewind.toast(Icon::Upload, not_published, Some(error), true, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_progress_is_compared_in_whole_percent() {
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(0.4214), percent(0.4249));
        assert_ne!(percent(0.42), percent(0.43));
        assert_eq!(percent(1.7), 100);
        assert_eq!(percent(-0.2), 0);
    }

    #[test]
    fn shared_links_map_to_safe_cache_names() {
        assert_eq!(
            split_link("https://files.catbox.moe/ab12.png"),
            Some(("files.catbox.moe", "/ab12.png"))
        );
        assert_eq!(cache_name("https://i.ibb.co/vx/a b.png"), "vx_a_b.png");
        assert_eq!(split_link("ftp://x/y"), None);
    }

    #[test]
    fn a_half_written_png_is_not_complete() {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&[0; 16]);
        assert!(!is_complete(&png));
        png.extend_from_slice(&PNG_END);
        assert!(is_complete(&png));
        assert!(is_complete(b"JFIF"));
        assert!(!is_complete(b""));
    }

    #[test]
    fn edited_copies_never_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("shot.png");
        let first = edited_path(&source);
        assert_eq!(first.file_name().unwrap(), "shot (изменено).png");
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(
            edited_path(&source).file_name().unwrap(),
            "shot (изменено 2).png"
        );
    }
}
