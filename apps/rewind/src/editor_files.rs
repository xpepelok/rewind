use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rewind_config::Paths;

const SCRATCH_DIR: &str = "scratch";
const SHARE_DIR: &str = "Rewind Share";
const SHARE_KEEP: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const SIZE_MARGIN: f64 = 0.92;
const MIN_VIDEO_BITRATE: f64 = 200_000.0;
const HD_BITRATE: f64 = 1_500_000.0;
const SD_BITRATE: f64 = 700_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharePlan {
    Copy,
    Encode { bitrate: u32, height: Option<u32> },
    TooLong { max_seconds: u32 },
}

pub fn share_plan(
    limit: u64,
    file_size: u64,
    whole: std::time::Duration,
    span: std::time::Duration,
    source_height: u32,
    edited: bool,
    audio_bits: u32,
) -> SharePlan {
    let seconds = span.as_secs_f64().max(0.1);
    let fraction = (seconds / whole.as_secs_f64().max(0.1)).min(1.0);
    let budget = limit as f64 * SIZE_MARGIN;
    if !edited && file_size as f64 * fraction <= budget {
        return SharePlan::Copy;
    }
    let audio = f64::from(audio_bits);
    let video = budget * 8.0 / seconds - audio;
    if video < MIN_VIDEO_BITRATE {
        return SharePlan::TooLong {
            max_seconds: (budget * 8.0 / (MIN_VIDEO_BITRATE + audio)) as u32,
        };
    }
    let source = file_size as f64 * 8.0 / whole.as_secs_f64().max(0.1);
    let bitrate = video.min(source.max(MIN_VIDEO_BITRATE));
    let height = if bitrate < SD_BITRATE && source_height > 480 {
        Some(480)
    } else if bitrate < HD_BITRATE && source_height > 720 {
        Some(720)
    } else {
        None
    };
    SharePlan::Encode {
        bitrate: bitrate as u32,
        height,
    }
}

fn share_dir() -> PathBuf {
    let gallery = Paths::default_gallery();
    gallery.parent().unwrap_or(&gallery).join(SHARE_DIR)
}

pub fn share_file(source: &Path) -> PathBuf {
    let directory = share_dir();
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > SHARE_KEEP);
            if old {
                std::fs::remove_file(entry.path()).ok();
            }
        }
    }
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    directory.join(format!("{}.mp4", stem.trim_end_matches(".DVR")))
}
const COPY_SUFFIX: &str = "обрезка";

pub fn scratch_dir() -> PathBuf {
    Paths::resolve().data.join(SCRATCH_DIR)
}

pub fn scratch_file() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    scratch_dir().join(format!("upload-{stamp:x}.mp4"))
}

pub fn is_scratch(path: &Path) -> bool {
    path.starts_with(scratch_dir())
}

pub fn copy_path(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = stem.trim_end_matches(".DVR");
    (1..)
        .map(|index| {
            let suffix = if index == 1 {
                format!(" ({COPY_SUFFIX})")
            } else {
                format!(" ({COPY_SUFFIX} {index})")
            };
            source.with_file_name(format!("{stem}{suffix}.mp4"))
        })
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| source.with_extension("copy.mp4"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_path_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Game 2026.11.09 - 10.00.00.01.DVR.mp4");
        let first = copy_path(&source);
        assert!(
            first
                .to_string_lossy()
                .ends_with("10.00.00.01 (обрезка).mp4")
        );
        std::fs::write(&first, b"x").unwrap();
        let second = copy_path(&source);
        assert!(second.to_string_lossy().ends_with("(обрезка 2).mp4"));
    }

    #[test]
    fn share_plans_fit_the_limit() {
        use std::time::Duration;
        let minute = Duration::from_secs(60);
        assert_eq!(
            share_plan(10_000_000, 5_000_000, minute, minute, 1080, false, 192_000),
            SharePlan::Copy
        );
        let SharePlan::Encode { bitrate, height } =
            share_plan(10_000_000, 90_000_000, minute, minute, 1080, false, 192_000)
        else {
            panic!("expected an encode");
        };
        assert!(f64::from(bitrate + 192_000) * 60.0 / 8.0 <= 10_000_000.0);
        assert_eq!(height, Some(720));
        assert!(matches!(
            share_plan(10_000_000, 900_000_000, minute * 10, minute * 10, 1080, false, 192_000),
            SharePlan::TooLong { max_seconds } if max_seconds < 600 && max_seconds > 100
        ));
        assert!(matches!(
            share_plan(10_000_000, 5_000_000, minute, minute, 1080, true, 192_000),
            SharePlan::Encode { .. }
        ));
    }

    #[test]
    fn scratch_files_are_recognised() {
        assert!(is_scratch(&scratch_file()));
        assert!(!is_scratch(Path::new("C:/Videos/clip.mp4")));
    }
}
