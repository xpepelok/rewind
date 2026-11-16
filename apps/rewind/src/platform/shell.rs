use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING,
    SHFILEOPSTRUCTW, SHFileOperationW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};

const EXPLORER: &str = "explorer.exe";

fn launch(target: String, arguments: Option<String>) {
    std::thread::Builder::new()
        .name("rewind-shell".to_owned())
        .spawn(move || {
            let com =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
                    .is_ok();
            let target = HSTRING::from(target);
            let arguments = arguments.map(HSTRING::from);
            let _ = unsafe {
                ShellExecuteW(
                    None,
                    &HSTRING::from("open"),
                    &target,
                    arguments
                        .as_ref()
                        .map_or(PCWSTR::null(), |arguments| PCWSTR(arguments.as_ptr())),
                    PCWSTR::null(),
                    SW_SHOWNORMAL,
                )
            };
            if com {
                unsafe { CoUninitialize() };
            }
        })
        .ok();
}

pub fn open(path: &Path) {
    launch(path.display().to_string(), None);
}

pub fn open_url(url: &str) {
    launch(url.to_owned(), None);
}

pub fn select(path: &Path) {
    launch(
        EXPLORER.to_owned(),
        Some(format!("/select,\"{}\"", path.display())),
    );
}

pub fn reveal(path: &Path) {
    std::fs::create_dir_all(path).ok();
    open(path);
}

pub fn trash(paths: &[PathBuf]) -> bool {
    if trash_together(paths) {
        return true;
    }
    let mut all = true;
    for path in paths.iter().filter(|path| path.exists()) {
        all &= trash_together(std::slice::from_ref(path));
    }
    all
}

fn trash_together(paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return true;
    }
    let mut from: Vec<u16> = Vec::new();
    for path in paths {
        from.extend(path.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let flags = FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0;
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (flags | FOF_WANTNUKEWARNING.0) as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut operation) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_held_by_another_program_does_not_keep_the_rest() {
        use std::os::windows::fs::OpenOptionsExt;
        let folder = std::env::temp_dir().join(format!("rewind-held-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("mkdir");
        let files: Vec<PathBuf> = (0..3)
            .map(|index| {
                let file = folder.join(format!("clip {index}.mp4"));
                std::fs::write(&file, b"x").expect("write");
                file
            })
            .collect();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&files[0])
            .expect("hold");
        assert!(!trash(&files));
        assert!(files[0].exists());
        assert!(files[1..].iter().all(|file| !file.exists()));
        drop(held);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn trashed_files_leave_their_folder() {
        let folder = std::env::temp_dir().join(format!("rewind-trash-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("mkdir");
        let files: Vec<PathBuf> = (0..2)
            .map(|index| {
                let file = folder.join(format!("clip {index}.mp4"));
                std::fs::write(&file, b"x").expect("write");
                file
            })
            .collect();
        assert!(trash(&files));
        assert!(files.iter().all(|file| !file.exists()));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
