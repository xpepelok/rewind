use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use image::ImageFormat;
use rewind_config::{Gallery, GalleryKind, GallerySort};

const SCAN_DEPTH: usize = 2;
const MEDIA_EXTENSIONS: [&str; 2] = ["mp4", "png"];
const PREVIEW_AT: Duration = Duration::from_secs(1);
const THUMBNAIL_WIDTH: u32 = 320;
const MAX_WORKERS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GalleryItem {
    pub game: String,
    pub path: PathBuf,
    pub preview: Option<PathBuf>,
    pub duration: Option<Duration>,
    pub modified: SystemTime,
    pub size: u64,
}

fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MEDIA_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
}

fn collect(dir: &Path, depth: usize, files: &mut Vec<(PathBuf, SystemTime, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            if depth > 0 {
                collect(&path, depth - 1, files);
            }
        } else if is_media(&path) {
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((path, modified, metadata.len()));
        }
    }
}

fn media_files(gallery: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let mut files = Vec::with_capacity(64);
    collect(gallery, SCAN_DEPTH, &mut files);
    files.sort_by_key(|(_, modified, _)| std::cmp::Reverse(*modified));
    files
}

fn game_of(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn is_replay(path: &Path) -> bool {
    file_name(path).ends_with(".dvr.mp4")
}

pub fn format_size(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let value = bytes as f64;
    if value >= GB {
        let text = format!("{:.2}", value / GB);
        format!("{}GB", text.trim_end_matches('0').trim_end_matches('.'))
    } else if value >= MB {
        format!("{}MB", (value / MB).round() as u64)
    } else {
        format!("{}KB", (value / 1_000.0).round().max(1.0) as u64)
    }
}

impl GalleryItem {
    pub fn matches(&self, view: &Gallery) -> bool {
        let video = is_video(&self.path);
        let kind = match view.kind {
            GalleryKind::All => true,
            GalleryKind::Videos => video,
            GalleryKind::Replays => video && is_replay(&self.path),
            GalleryKind::Recordings => video && !is_replay(&self.path),
            GalleryKind::Screenshots => !video,
        };
        kind && view.game.as_ref().is_none_or(|game| *game == self.game)
    }
}

pub fn games(items: &[GalleryItem]) -> Vec<String> {
    let mut games: Vec<String> = items.iter().map(|item| item.game.clone()).collect();
    games.sort_by_cached_key(|game| (game.to_lowercase(), game.clone()));
    games.dedup();
    games
}

pub fn arrange(items: &[GalleryItem], view: &Gallery) -> Vec<GalleryItem> {
    let mut shown: Vec<GalleryItem> = items
        .iter()
        .filter(|item| item.matches(view))
        .cloned()
        .collect();
    let length = |item: &GalleryItem| item.duration.unwrap_or_default();
    match view.sort {
        GallerySort::Newest => shown.sort_by_key(|item| std::cmp::Reverse(item.modified)),
        GallerySort::Oldest => shown.sort_by_key(|item| item.modified),
        GallerySort::Largest => shown.sort_by_key(|item| std::cmp::Reverse(item.size)),
        GallerySort::Smallest => shown.sort_by_key(|item| item.size),
        GallerySort::Longest => shown.sort_by_key(|item| std::cmp::Reverse(length(item))),
        GallerySort::Shortest => shown.sort_by_key(length),
        GallerySort::Name => shown.sort_by_key(|item| file_name(&item.path)),
    }
    shown
}

type MediaFile = (PathBuf, SystemTime, u64);

#[derive(Clone, Debug)]
struct Entry {
    modified: SystemTime,
    size: u64,
    item: GalleryItem,
    failed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Job {
    Keep(GalleryItem),
    Preview { item: GalleryItem, retry: bool },
    Probe,
}

#[derive(Debug, Default)]
pub struct Cache {
    entries: HashMap<PathBuf, Entry>,
}

impl Cache {
    fn plan(&self, (path, modified, size): &MediaFile) -> Job {
        match self.entries.get(path) {
            Some(entry) if entry.modified == *modified && entry.size == *size => {
                if entry.item.preview.is_some() {
                    Job::Keep(entry.item.clone())
                } else {
                    Job::Preview {
                        item: entry.item.clone(),
                        retry: !entry.failed,
                    }
                }
            }
            _ => Job::Probe,
        }
    }
}

#[derive(Debug, Default)]
pub struct Scanner {
    running: bool,
    dirty: bool,
    limit: Option<u64>,
}

impl Scanner {
    pub fn request(&mut self, limit: Option<u64>) -> Option<Option<u64>> {
        self.limit = limit.or(self.limit);
        if self.running {
            self.dirty = true;
            return None;
        }
        self.running = true;
        Some(self.limit.take())
    }

    pub fn finish(&mut self) -> Option<Option<u64>> {
        if std::mem::take(&mut self.dirty) {
            return Some(self.limit.take());
        }
        self.running = false;
        None
    }
}

pub fn thumbnail_path(thumbnails: &Path, path: &Path) -> Option<PathBuf> {
    path.file_name()
        .map(|name| thumbnails.join(format!("{}.png", name.to_string_lossy())))
}

#[derive(Default)]
struct Worker {
    runtime: Option<Option<rewind_media::Runtime>>,
}

impl Worker {
    fn media_ready(&mut self) -> bool {
        self.runtime
            .get_or_insert_with(|| rewind_media::Runtime::enter().ok())
            .is_some()
    }

    fn preview(
        &mut self,
        path: &Path,
        thumbnails: &Path,
        generate: bool,
    ) -> (Option<PathBuf>, bool) {
        let video = is_video(path);
        if !video && image::image_dimensions(path).is_ok_and(|(width, _)| width <= THUMBNAIL_WIDTH)
        {
            return (Some(path.to_path_buf()), false);
        }
        let Some(thumbnail) = thumbnail_path(thumbnails, path) else {
            return (None, false);
        };
        if thumbnail.exists() {
            return (Some(thumbnail), false);
        }
        if !generate {
            return (None, true);
        }
        let made = if video {
            if !self.media_ready() {
                return (None, false);
            }
            rewind_media::thumbnail(path, PREVIEW_AT, THUMBNAIL_WIDTH, &thumbnail).is_ok()
        } else {
            write_preview(path, &thumbnail).is_ok()
        };
        if made {
            (Some(thumbnail), false)
        } else {
            (None, true)
        }
    }

    fn resolve(&mut self, file: &MediaFile, job: Job, thumbnails: &Path) -> Entry {
        let (path, modified, size) = file;
        let (item, retry) = match job {
            Job::Keep(item) => {
                return Entry {
                    modified: *modified,
                    size: *size,
                    item,
                    failed: false,
                };
            }
            Job::Preview { item, retry } => (item, retry),
            Job::Probe => (
                GalleryItem {
                    game: game_of(path),
                    path: path.clone(),
                    preview: None,
                    duration: is_video(path).then(|| mp4_duration(path)).flatten(),
                    modified: *modified,
                    size: *size,
                },
                true,
            ),
        };
        let (preview, failed) = self.preview(path, thumbnails, retry);
        Entry {
            modified: *modified,
            size: *size,
            item: GalleryItem { preview, ..item },
            failed,
        }
    }
}

pub fn write_preview(path: &Path, thumbnail: &Path) -> image::ImageResult<()> {
    let picture = image::open(path)?;
    let small = if picture.width() > THUMBNAIL_WIDTH {
        picture.thumbnail(THUMBNAIL_WIDTH, u32::MAX)
    } else {
        picture
    };
    if let Some(parent) = thumbnail.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = thumbnail.with_extension("part");
    small.save_with_format(&partial, ImageFormat::Png)?;
    fs::rename(&partial, thumbnail).inspect_err(|_| {
        fs::remove_file(&partial).ok();
    })?;
    Ok(())
}

pub fn scan(gallery: &Path, thumbnails: &Path, cache: &mut Cache) -> Vec<GalleryItem> {
    let files = media_files(gallery);
    let mut entries: Vec<Option<Entry>> = vec![None; files.len()];
    let mut pending = Vec::new();
    for (index, file) in files.iter().enumerate() {
        match cache.plan(file) {
            Job::Keep(item) => {
                entries[index] = Some(Entry {
                    modified: file.1,
                    size: file.2,
                    item,
                    failed: false,
                });
            }
            job => pending.push((index, job)),
        }
    }
    let workers = thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .clamp(1, MAX_WORKERS)
        .min(pending.len());
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut worker = Worker::default();
                    let mut done = Vec::new();
                    loop {
                        let slot = next.fetch_add(1, Ordering::Relaxed);
                        let Some((index, job)) = pending.get(slot) else {
                            break;
                        };
                        let entry = worker.resolve(&files[*index], job.clone(), thumbnails);
                        done.push((*index, entry));
                    }
                    done
                })
            })
            .collect();
        for handle in handles {
            for (index, entry) in handle.join().unwrap_or_default() {
                entries[index] = Some(entry);
            }
        }
    });
    let entries: Vec<Entry> = entries.into_iter().flatten().collect();
    cache.entries = entries
        .iter()
        .map(|entry| (entry.item.path.clone(), entry.clone()))
        .collect();
    entries.into_iter().map(|entry| entry.item).collect()
}

pub fn enforce_limit(gallery: &Path, thumbnails: &Path, limit_bytes: u64) {
    let files = media_files(gallery);
    let mut total: u64 = files.iter().map(|(_, _, size)| size).sum();
    for (path, _, size) in files.iter().rev() {
        if total <= limit_bytes {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(*size);
            if let Some(thumbnail) = thumbnail_path(thumbnails, path) {
                fs::remove_file(thumbnail).ok();
            }
        }
    }
}

fn read_box(file: &mut File, offset: u64) -> Option<(u64, [u8; 4], u64)> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header).ok()?;
    let size = u64::from(u32::from_be_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let kind = [header[4], header[5], header[6], header[7]];
    if size == 1 {
        let mut large = [0u8; 8];
        file.read_exact(&mut large).ok()?;
        return Some((u64::from_be_bytes(large), kind, 16));
    }
    Some((size, kind, 8))
}

fn read_u32(file: &mut File) -> Option<u32> {
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes).ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn read_u64(file: &mut File) -> Option<u64> {
    let mut bytes = [0u8; 8];
    file.read_exact(&mut bytes).ok()?;
    Some(u64::from_be_bytes(bytes))
}

fn movie_header(file: &mut File, start: u64, end: u64) -> Option<Duration> {
    let mut offset = start;
    while offset + 8 <= end {
        let (size, kind, header) = read_box(file, offset)?;
        if size < header {
            return None;
        }
        if &kind == b"mvhd" {
            let version = read_u32(file)? >> 24;
            let (timescale, duration) = if version == 1 {
                file.seek(SeekFrom::Current(16)).ok()?;
                (read_u32(file)?, read_u64(file)?)
            } else {
                file.seek(SeekFrom::Current(8)).ok()?;
                (read_u32(file)?, u64::from(read_u32(file)?))
            };
            return (timescale > 0)
                .then(|| Duration::from_secs_f64(duration as f64 / f64::from(timescale)));
        }
        offset += size;
    }
    None
}

pub fn mp4_duration(path: &Path) -> Option<Duration> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut offset = 0;
    while offset + 8 <= length {
        let (size, kind, header) = read_box(&mut file, offset)?;
        let size = if size == 0 { length - offset } else { size };
        if size < header {
            return None;
        }
        if &kind == b"moov" {
            return movie_header(&mut file, offset + header, offset + size);
        }
        offset += size;
    }
    None
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn sizes_read_like_a_person_would_write_them() {
        assert_eq!(format_size(1_000_000_000), "1GB");
        assert_eq!(format_size(1_324_000_000), "1.32GB");
        assert_eq!(format_size(1_300_000_000), "1.3GB");
        assert_eq!(format_size(398_400_000), "398MB");
        assert_eq!(format_size(679_000), "679KB");
    }

    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(body);
        bytes
    }

    fn clip(name: &str, game: &str, minutes: u64, size: u64) -> GalleryItem {
        GalleryItem {
            game: game.to_owned(),
            path: PathBuf::from(format!("C:/NVIDIA/{game}/{name}")),
            preview: None,
            duration: name
                .ends_with(".mp4")
                .then(|| Duration::from_secs(60 * minutes)),
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(minutes),
            size,
        }
    }

    #[test]
    fn the_gallery_filters_and_sorts_every_file() {
        let items = vec![
            clip("a.DVR.mp4", "Valorant", 3, 30),
            clip("b.mp4", "Valorant", 1, 90),
            clip("c.png", "Desktop", 2, 10),
        ];
        let names = |view: &Gallery| -> Vec<String> {
            arrange(&items, view)
                .iter()
                .map(|item| file_name(&item.path))
                .collect()
        };
        let mut view = Gallery::default();
        assert_eq!(names(&view), ["a.dvr.mp4", "c.png", "b.mp4"]);
        view.sort = GallerySort::Largest;
        assert_eq!(names(&view), ["b.mp4", "a.dvr.mp4", "c.png"]);
        view.kind = GalleryKind::Replays;
        assert_eq!(names(&view), ["a.dvr.mp4"]);
        view.kind = GalleryKind::Screenshots;
        assert_eq!(names(&view), ["c.png"]);
        view.kind = GalleryKind::All;
        view.game = Some("Valorant".to_owned());
        view.sort = GallerySort::Shortest;
        assert_eq!(names(&view), ["b.mp4", "a.dvr.mp4"]);
        assert_eq!(games(&items), ["Desktop", "Valorant"]);
    }

    #[test]
    fn reads_duration_from_moov_after_mdat() {
        let mut mvhd = vec![0u8; 4];
        mvhd.extend_from_slice(&[0u8; 8]);
        mvhd.extend_from_slice(&1000u32.to_be_bytes());
        mvhd.extend_from_slice(&33_500u32.to_be_bytes());
        let mut file_bytes = atom(b"ftyp", b"isom");
        file_bytes.extend(atom(b"mdat", &[7u8; 64]));
        file_bytes.extend(atom(b"moov", &atom(b"mvhd", &mvhd)));
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&file_bytes).unwrap();
        assert_eq!(
            mp4_duration(file.path()),
            Some(Duration::from_millis(33_500))
        );
    }

    #[test]
    fn parallel_scan_keeps_every_file_newest_first() {
        let gallery = tempfile::tempdir().unwrap();
        let thumbnails = tempfile::tempdir().unwrap();
        let count = 30;
        for index in 0..count {
            let folder = gallery.path().join(format!("game{}", index % 3));
            fs::create_dir_all(&folder).unwrap();
            let path = folder.join(format!("shot{index}.png"));
            let file = File::create(&path).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + index as u64))
                .unwrap();
        }
        let mut cache = Cache::default();
        let items = scan(gallery.path(), thumbnails.path(), &mut cache);
        assert_eq!(items.len(), count);
        let names: Vec<String> = items
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let expected: Vec<String> = (0..count)
            .rev()
            .map(|index| format!("shot{index}.png"))
            .collect();
        assert_eq!(names, expected);
        assert!(items.iter().all(|item| item.preview.is_none()));
        assert!(cache.entries.values().all(|entry| entry.failed));
    }

    fn png(path: &Path, width: u32) {
        image::RgbImage::new(width, 20).save(path).unwrap();
    }

    fn stamp(path: &Path, seconds: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn the_cache_reprobes_only_changed_files() {
        let path = PathBuf::from("C:/NVIDIA/Game/a.mp4");
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let mut item = clip("a.mp4", "Game", 1, 5);
        item.path = path.clone();
        item.preview = Some(PathBuf::from("C:/thumbs/a.mp4.png"));
        let mut cache = Cache::default();
        cache.entries.insert(
            path.clone(),
            Entry {
                modified,
                size: 5,
                item: item.clone(),
                failed: false,
            },
        );
        assert_eq!(
            cache.plan(&(path.clone(), modified, 5)),
            Job::Keep(item.clone())
        );
        assert_eq!(cache.plan(&(path.clone(), modified, 6)), Job::Probe);
        let later = modified + Duration::from_secs(1);
        assert_eq!(cache.plan(&(path.clone(), later, 5)), Job::Probe);
        assert_eq!(
            cache.plan(&(PathBuf::from("C:/NVIDIA/Game/b.mp4"), modified, 5)),
            Job::Probe
        );
        let blank = GalleryItem {
            preview: None,
            ..item
        };
        for failed in [true, false] {
            cache.entries.insert(
                path.clone(),
                Entry {
                    modified,
                    size: 5,
                    item: blank.clone(),
                    failed,
                },
            );
            assert_eq!(
                cache.plan(&(path.clone(), modified, 5)),
                Job::Preview {
                    item: blank.clone(),
                    retry: !failed
                }
            );
        }
    }

    #[test]
    fn screenshots_get_small_previews_and_failures_are_remembered() {
        let gallery = tempfile::tempdir().unwrap();
        let thumbnails = tempfile::tempdir().unwrap();
        let folder = gallery.path().join("Desktop");
        fs::create_dir_all(&folder).unwrap();
        let big = folder.join("big.png");
        let small = folder.join("small.png");
        let broken = folder.join("broken.png");
        png(&big, 1280);
        png(&small, 100);
        fs::write(&broken, b"not a png").unwrap();
        stamp(&big, 3_000);
        stamp(&small, 2_000);
        stamp(&broken, 1_000);
        let mut cache = Cache::default();
        let items = scan(gallery.path(), thumbnails.path(), &mut cache);
        let big_thumbnail = thumbnails.path().join("big.png.png");
        let previews: Vec<Option<PathBuf>> =
            items.iter().map(|item| item.preview.clone()).collect();
        assert_eq!(
            previews,
            [Some(big_thumbnail.clone()), Some(small.clone()), None]
        );
        assert_eq!(image::image_dimensions(&big_thumbnail).unwrap(), (320, 5));
        assert!(cache.entries[&broken].failed);
        let unchanged = (broken.clone(), cache.entries[&broken].modified, 9);
        assert!(matches!(
            cache.plan(&unchanged),
            Job::Preview { retry: false, .. }
        ));

        fs::remove_file(&big_thumbnail).unwrap();
        let again = scan(gallery.path(), thumbnails.path(), &mut cache);
        assert_eq!(again, items);
        assert!(!big_thumbnail.exists());

        png(&broken, 640);
        stamp(&broken, 4_000);
        fs::remove_file(&small).unwrap();
        let fixed = scan(gallery.path(), thumbnails.path(), &mut cache);
        assert_eq!(fixed.len(), 2);
        assert_eq!(
            fixed[0].preview,
            Some(thumbnails.path().join("broken.png.png"))
        );
        assert!(!cache.entries[&broken].failed);
        assert!(!cache.entries.contains_key(&small));
    }

    #[test]
    fn overlapping_refreshes_collapse_into_one_rerun() {
        let mut scanner = Scanner::default();
        assert_eq!(scanner.request(None), Some(None));
        assert_eq!(scanner.request(Some(7)), None);
        assert_eq!(scanner.request(None), None);
        assert_eq!(scanner.finish(), Some(Some(7)));
        assert_eq!(scanner.finish(), None);
        assert_eq!(scanner.request(None), Some(None));
        assert_eq!(scanner.finish(), None);
    }

    #[test]
    fn garbage_has_no_duration() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"not an mp4 file at all").unwrap();
        assert_eq!(mp4_duration(file.path()), None);
    }

    #[test]
    fn duration_formats_like_overlay_badge() {
        assert_eq!(format_duration(Duration::from_secs(300)), "05:00");
        assert_eq!(format_duration(Duration::from_secs(33)), "00:33");
    }
}
