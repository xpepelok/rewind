use std::path::{Path, PathBuf};

use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::{catalog, foreground};

const DESKTOP: &str = "Desktop";
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
const GENERIC_DIRS: [&str; 10] = [
    "bin", "binaries", "win64", "win32", "x64", "x86", "retail", "shipping", "release", "game",
];

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Replay,
    Recording,
    Screenshot,
}

impl Kind {
    const fn suffix(self) -> &'static str {
        match self {
            Self::Replay => ".DVR.mp4",
            Self::Recording => ".mp4",
            Self::Screenshot => ".png",
        }
    }
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            if FORBIDDEN.contains(&character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        DESKTOP.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn stamp() -> String {
    let time = unsafe { GetLocalTime() };
    format!(
        "{:04}.{:02}.{:02} - {:02}.{:02}.{:02}.{:02}",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        time.wMilliseconds / 10
    )
}

fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(characters.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn install_folder(executable: &Path) -> Option<String> {
    executable
        .ancestors()
        .skip(1)
        .filter_map(|directory| directory.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .find(|name| !GENERIC_DIRS.contains(&name.to_lowercase().as_str()))
}

fn folder_name(executable: Option<&Path>, known: impl Fn(&Path) -> Option<String>) -> String {
    let Some(executable) = executable else {
        return DESKTOP.to_owned();
    };
    let name = known(executable)
        .or_else(|| install_folder(executable))
        .unwrap_or_default();
    sanitize(&title_case(&name))
}

pub fn screenshot_path(gallery: &Path) -> PathBuf {
    next_path(gallery, Kind::Screenshot)
}

pub fn application_name(executable: &Path) -> String {
    folder_name(Some(executable), catalog::display_name)
}

pub fn next_path(gallery: &Path, kind: Kind) -> PathBuf {
    let application = folder_name(
        foreground::application_path().as_deref(),
        catalog::display_name,
    );
    gallery
        .join(&application)
        .join(format!("{application} {}{}", stamp(), kind.suffix()))
}

pub fn thumbnail_path(thumbnails: &Path, clip: &Path) -> PathBuf {
    let stem = clip
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    thumbnails.join(format!("{stem}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_forbidden_characters() {
        assert_eq!(sanitize("a:b*c"), "a_b_c");
        assert_eq!(sanitize("   "), "Desktop");
        assert_eq!(sanitize("game."), "game");
    }

    #[test]
    fn folders_are_named_the_way_nvidia_names_them() {
        let unknown = |_: &Path| None;
        let known = |_: &Path| Some("PEAK".to_owned());
        assert_eq!(folder_name(None, unknown), "Desktop");
        assert_eq!(
            folder_name(
                Some(Path::new(r"C:\Program Files\Java\jdk-23\bin\javaw.exe")),
                unknown
            ),
            "Jdk-23"
        );
        assert_eq!(
            folder_name(
                Some(Path::new(
                    r"D:\Steam\steamapps\common\YAPYAP Demo\YAPYAP.exe"
                )),
                unknown
            ),
            "Yapyap Demo"
        );
        assert_eq!(
            folder_name(
                Some(Path::new(r"D:\Games\REPO\Binaries\Win64\REPO.exe")),
                unknown
            ),
            "Repo"
        );
        assert_eq!(title_case("Counter-Strike 2"), "Counter-strike 2");
        assert_eq!(title_case("PEAK"), "Peak");
        assert_eq!(folder_name(Some(Path::new("PEAK.exe")), known), "Peak");
    }
}
