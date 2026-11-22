use parking_lot::Mutex;
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

const SHELL_CLASSES: [&str; 3] = ["Progman", "WorkerW", "Shell_TrayWnd"];
const ENGINE_CLASSES: [&str; 5] = [
    "GLFW30",
    "LWJGL",
    "SDL_app",
    "UnityWndClass",
    "UnrealWindow",
];
const CLASS_CAPACITY: usize = 64;
const PATH_CAPACITY: usize = 1024;

fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; CLASS_CAPACITY];
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn covers(window: RECT, monitor: RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

fn foreground() -> Option<HWND> {
    let window = unsafe { GetForegroundWindow() };
    (!window.is_invalid()).then_some(window)
}

pub fn is_fullscreen_app() -> bool {
    foreground().is_some_and(covers_monitor)
}

static KNOWN_GAME: Mutex<Option<((isize, u32), bool)>> = Mutex::new(None);
static LISTED_GAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn set_games(executables: Vec<String>) {
    *LISTED_GAMES.lock() = executables;
    *KNOWN_GAME.lock() = None;
}

fn listed(games: &[String], path: &std::path::Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        games.iter().any(|game| game.eq_ignore_ascii_case(&name))
    })
}

pub fn game_window() -> Option<HWND> {
    let window = foreground()?;
    let mut process = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };
    if process == std::process::id() {
        return None;
    }
    let key = (window.0 as isize, process);
    let cached = *KNOWN_GAME.lock();
    let known = match cached {
        Some((seen, known)) if seen == key => known,
        _ => {
            let known = ENGINE_CLASSES.contains(&class_name(window).as_str())
                || image_path(process).is_some_and(|path| {
                    listed(&LISTED_GAMES.lock(), &path)
                        || crate::catalog::display_name(&path).is_some()
                });
            *KNOWN_GAME.lock() = Some((key, known));
            known
        }
    };
    (known && !SHELL_CLASSES.contains(&class_name(window).as_str()) || covers_monitor(window))
        .then_some(window)
}

fn covers_monitor(window: HWND) -> bool {
    if SHELL_CLASSES.contains(&class_name(window).as_str()) {
        return false;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(window, &mut rect) }.is_err() {
        return false;
    }
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() && covers(rect, info.rcMonitor)
}

static REMEMBERED: Mutex<Option<PathBuf>> = Mutex::new(None);

enum Owner {
    Shell,
    Itself,
    Other(PathBuf),
}

fn owner() -> Option<Owner> {
    let window = foreground()?;
    if SHELL_CLASSES.contains(&class_name(window).as_str()) {
        return Some(Owner::Shell);
    }
    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    if process_id == std::process::id() {
        return Some(Owner::Itself);
    }
    image_path(process_id).map(Owner::Other)
}

fn image_path(process_id: u32) -> Option<PathBuf> {
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    let mut buffer = [0u16; PATH_CAPACITY];
    let mut length = PATH_CAPACITY as u32;
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    queried.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

pub fn remember_application() {
    match owner() {
        Some(Owner::Itself) => {}
        Some(Owner::Other(path)) => *REMEMBERED.lock() = Some(path),
        Some(Owner::Shell) | None => *REMEMBERED.lock() = None,
    }
}

pub fn application_path() -> Option<PathBuf> {
    match owner()? {
        Owner::Shell => None,
        Owner::Itself => REMEMBERED.lock().clone(),
        Owner::Other(path) => Some(path),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramEvent {
    Focused { executable: String, process: u32 },
    Exited(u32),
}

type ProgramSink = std::sync::Arc<dyn Fn(ProgramEvent) + Send + Sync>;

static PROGRAM_SINK: Mutex<Option<ProgramSink>> = Mutex::new(None);
const SKIP_OWN_PROCESS: u32 = 2;

fn emit(event: ProgramEvent) {
    let sink = PROGRAM_SINK.lock().clone();
    if let Some(sink) = sink {
        sink(event);
    }
}

fn executable_of(path: &std::path::Path) -> Option<String> {
    Some(path.file_name()?.to_string_lossy().to_lowercase())
}

unsafe extern "system" fn on_foreground(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let mut process = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };
    if process == 0 {
        return;
    }
    if let Some(executable) = image_path(process).as_deref().and_then(executable_of) {
        emit(ProgramEvent::Focused {
            executable,
            process,
        });
    }
}

pub fn watch_programs(sink: impl Fn(ProgramEvent) + Send + Sync + 'static) {
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, MSG, WINEVENT_OUTOFCONTEXT,
    };
    *PROGRAM_SINK.lock() = Some(std::sync::Arc::new(sink));
    std::thread::Builder::new()
        .name("rewind-programs".to_owned())
        .spawn(|| {
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(on_foreground),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | SKIP_OWN_PROCESS,
                )
            };
            let mut message = MSG::default();
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
                unsafe { DispatchMessageW(&message) };
            }
            let _ = unsafe { UnhookWinEvent(hook) };
        })
        .ok();
}

pub fn watch_exit(process: u32) {
    use windows::Win32::System::Threading::{INFINITE, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    std::thread::Builder::new()
        .name("rewind-exit".to_owned())
        .spawn(move || {
            if let Ok(handle) = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, process) } {
                unsafe { WaitForSingleObject(handle, INFINITE) };
                let _ = unsafe { CloseHandle(handle) };
            }
            emit(ProgramEvent::Exited(process));
        })
        .ok();
}

pub fn running_programs() -> std::collections::HashMap<String, u32> {
    crate::discord::processes()
        .into_iter()
        .map(|(process, _, name)| (name.to_lowercase(), process))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::listed;
    use std::path::Path;

    #[test]
    fn a_listed_game_is_recognised_by_its_file_name_in_any_case() {
        let games = ["java.exe".to_owned(), "Game.exe".to_owned()];
        assert!(listed(
            &games,
            Path::new(r"C:\Games\Cristalix\runtime\bin\Java.EXE")
        ));
        assert!(listed(&games, Path::new(r"D:\game.exe")));
        assert!(!listed(&games, Path::new(r"C:\Program Files\javaw.exe")));
        assert!(!listed(&[], Path::new(r"D:\game.exe")));
    }
}
