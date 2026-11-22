use std::fmt::Debug;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const FILE: &str = "verbose.log";
const ROTATED_SUFFIX: &str = ".old";
const LIMIT: u64 = 8 * 1024 * 1024;
const UNNAMED_THREAD: &str = "unnamed";

static ENABLED: AtomicBool = AtomicBool::new(false);
static DIRECTORY: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn open(directory: &Path) {
    if let Ok(mut slot) = DIRECTORY.lock() {
        *slot = Some(directory.to_path_buf());
    }
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn write(area: &str, text: &str) {
    if !enabled() {
        return;
    }
    let Ok(slot) = DIRECTORY.lock() else {
        return;
    };
    let Some(directory) = slot.as_deref() else {
        return;
    };
    let thread = std::thread::current();
    let line = line(area, thread.name().unwrap_or(UNNAMED_THREAD), text);
    append(directory, &line);
}

fn line(area: &str, thread: &str, text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{} [{area}] ({thread}) {text}", timestamp())
}

fn append(directory: &Path, line: &str) {
    if fs::create_dir_all(directory).is_err() {
        return;
    }
    let file = directory.join(FILE);
    if fs::metadata(&file).is_ok_and(|meta| meta.len() > LIMIT) {
        fs::rename(&file, directory.join(format!("{FILE}{ROTATED_SUFFIX}"))).ok();
    }
    if let Ok(mut out) = OpenOptions::new().create(true).append(true).open(&file) {
        writeln!(out, "{line}").ok();
    }
}

#[macro_export]
macro_rules! trace {
    ($area:expr, $($text:tt)*) => {
        if $crate::enabled() {
            $crate::write($area, &format!($($text)*));
        }
    };
}

pub trait Traced: Sized {
    #[track_caller]
    fn traced(self, area: &str, step: &str) -> Self;
}

impl<T, E: Debug> Traced for Result<T, E> {
    #[track_caller]
    fn traced(self, area: &str, step: &str) -> Self {
        if let Err(error) = &self
            && enabled()
        {
            let place = Location::caller();
            write(
                area,
                &format!(
                    "FAILED {step} at {}:{}: {error:?}",
                    place.file(),
                    place.line()
                ),
            );
        }
        self
    }
}

impl<T> Traced for Option<T> {
    #[track_caller]
    fn traced(self, area: &str, step: &str) -> Self {
        if self.is_none() && enabled() {
            let place = Location::caller();
            write(
                area,
                &format!("NOTHING {step} at {}:{}", place.file(), place.line()),
            );
        }
        self
    }
}

#[cfg(windows)]
fn timestamp() -> String {
    let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds
    )
}

#[cfg(not(windows))]
fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_names_the_area_and_the_thread_and_stays_on_one_row() {
        let line = line("capture", "rewind-video", "open\n  the\tencoder");
        assert!(line.ends_with(" [capture] (rewind-video) open the encoder"));
    }

    #[test]
    fn nothing_is_written_until_logging_is_switched_on() {
        let directory = tempfile::tempdir().unwrap();
        open(directory.path());
        set_enabled(false);
        trace!("test", "quiet {}", 1);
        assert!(!directory.path().join(FILE).exists());
        set_enabled(true);
        trace!("test", "loud {}", 2);
        let failed: Result<(), &str> = Err("broken");
        failed.traced("test", "open the thing").ok();
        let missing: Option<u8> = None;
        missing.traced("test", "find the thing");
        set_enabled(false);
        let text = fs::read_to_string(directory.path().join(FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("loud 2"));
        assert!(lines[1].contains("FAILED open the thing at "));
        assert!(lines[1].ends_with(": \"broken\""));
        assert!(lines[2].contains("NOTHING find the thing at "));
    }

    #[test]
    fn a_full_log_is_rotated_once() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE);
        fs::write(&file, vec![b'x'; LIMIT as usize + 1]).unwrap();
        append(directory.path(), "fresh");
        assert_eq!(fs::read_to_string(&file).unwrap(), "fresh\n");
        assert!(
            directory
                .path()
                .join(format!("{FILE}{ROTATED_SUFFIX}"))
                .exists()
        );
    }
}
