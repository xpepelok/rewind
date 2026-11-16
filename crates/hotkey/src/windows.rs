use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use parking_lot::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_HOTKEY, WM_QUIT,
    WM_USER,
};

use rewind_config::{Command, Hotkey};

use crate::error::{HotkeyError, Result};

const WM_REBIND: u32 = WM_APP + 1;

const THREAD_NAME: &str = "rewind-hotkeys";
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_SHIFT: i32 = 0x10;
const VK_LWIN: i32 = 0x5B;
const VK_RWIN: i32 = 0x5C;
const KEY_DOWN_MASK: i16 = i16::MIN;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed(Command),
    Registered { taken: Vec<Command> },
}

type Bindings = Arc<Mutex<Vec<(Command, Hotkey)>>>;

pub struct HotkeyService {
    thread_id: u32,
    bindings: Bindings,
    thread: Option<JoinHandle<()>>,
}

impl HotkeyService {
    pub fn start(
        bindings: Vec<(Command, Hotkey)>,
        on_event: impl Fn(HotkeyEvent) + Send + 'static,
    ) -> Result<Self> {
        let bindings: Bindings = Arc::new(Mutex::new(bindings));
        let shared = bindings.clone();
        let (ready, thread_id) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || run(shared, ready, on_event))?;
        let thread_id = thread_id.recv().map_err(|_| HotkeyError::NotReady)?;
        Ok(Self {
            thread_id,
            bindings,
            thread: Some(thread),
        })
    }

    pub fn rebind(&self, bindings: Vec<(Command, Hotkey)>) {
        *self.bindings.lock() = bindings;
        post(self.thread_id, WM_REBIND);
    }
}

impl Drop for HotkeyService {
    fn drop(&mut self) {
        post(self.thread_id, WM_QUIT);
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

fn post(thread_id: u32, message: u32) {
    let _ = unsafe { PostThreadMessageW(thread_id, message, WPARAM(0), LPARAM(0)) };
}

fn run(bindings: Bindings, ready: mpsc::Sender<u32>, on_event: impl Fn(HotkeyEvent)) {
    let mut message = MSG::default();
    let _ = unsafe { PeekMessageW(&mut message, None, WM_USER, WM_USER, PM_NOREMOVE) };
    if ready.send(unsafe { GetCurrentThreadId() }).is_err() {
        return;
    }
    let mut active = register(&bindings.lock(), &on_event);
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        match message.message {
            WM_HOTKEY => {
                if let Some(command) = active.get(message.wParam.0) {
                    on_event(HotkeyEvent::Pressed(*command));
                }
            }
            WM_REBIND => {
                unregister(active.len());
                active = register(&bindings.lock(), &on_event);
            }
            _ => {}
        }
    }
    unregister(active.len());
}

fn register(bindings: &[(Command, Hotkey)], on_event: &impl Fn(HotkeyEvent)) -> Vec<Command> {
    let mut taken = Vec::new();
    let active = bindings
        .iter()
        .enumerate()
        .map(|(index, (command, hotkey))| {
            let registered = unsafe {
                RegisterHotKey(
                    None,
                    index as i32,
                    modifiers(*hotkey) | MOD_NOREPEAT,
                    u32::from(hotkey.key),
                )
            };
            if registered.is_err() {
                taken.push(*command);
            }
            *command
        })
        .collect();
    on_event(HotkeyEvent::Registered { taken });
    active
}

fn unregister(count: usize) {
    for index in 0..count {
        let _ = unsafe { UnregisterHotKey(None, index as i32) };
    }
}

fn modifiers(hotkey: Hotkey) -> HOT_KEY_MODIFIERS {
    [
        (hotkey.modifiers.ctrl, MOD_CONTROL),
        (hotkey.modifiers.alt, MOD_ALT),
        (hotkey.modifiers.shift, MOD_SHIFT),
        (hotkey.modifiers.win, MOD_WIN),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(HOT_KEY_MODIFIERS(0), |all, (_, flag)| all | flag)
}

fn down(key: i32) -> bool {
    let state = unsafe { GetAsyncKeyState(key) };
    state & KEY_DOWN_MASK != 0
}

pub fn is_held(hotkey: Hotkey) -> bool {
    let modifiers = hotkey.modifiers;
    down(i32::from(hotkey.key))
        && (!modifiers.ctrl || down(VK_CONTROL))
        && (!modifiers.alt || down(VK_MENU))
        && (!modifiers.shift || down(VK_SHIFT))
        && (!modifiers.win || down(VK_LWIN) || down(VK_RWIN))
}

const VK_SNAPSHOT: u32 = 0x2C;
const VK_SCROLL: u32 = 0x91;
const CATCHER_NAME: &str = "rewind-key-catcher";

type Catch = Arc<dyn Fn(Hotkey) + Send + Sync>;

static CATCH: Mutex<Option<Catch>> = Mutex::new(None);

pub const fn invisible_to_windows(key: u32) -> bool {
    matches!(key, VK_SNAPSHOT | VK_SCROLL)
}

fn held_modifiers() -> rewind_config::Modifiers {
    rewind_config::Modifiers {
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        win: down(VK_LWIN) || down(VK_RWIN),
    }
}

unsafe extern "system" fn catch_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_SYSKEYDOWN,
    };
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if invisible_to_windows(info.vkCode) {
            let pressed = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            if pressed && let Some(catch) = CATCH.lock().clone() {
                catch(Hotkey::new(held_modifiers(), info.vkCode as u16));
            }
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub struct KeyCatcher {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyCatcher {
    pub fn start(on_key: impl Fn(Hotkey) + Send + Sync + 'static) -> Result<Self> {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
        };
        *CATCH.lock() = Some(Arc::new(on_key));
        let (ready, thread_id) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(CATCHER_NAME.to_owned())
            .spawn(move || {
                let mut message = MSG::default();
                let _ = unsafe { PeekMessageW(&mut message, None, WM_USER, WM_USER, PM_NOREMOVE) };
                let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(catch_key), None, 0) };
                if ready.send(unsafe { GetCurrentThreadId() }).is_err() {
                    return;
                }
                while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {}
                if let Ok(hook) = hook {
                    let _ = unsafe { UnhookWindowsHookEx(hook) };
                }
            })?;
        let thread_id = thread_id.recv().map_err(|_| HotkeyError::NotReady)?;
        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

impl Drop for KeyCatcher {
    fn drop(&mut self) {
        post(self.thread_id, WM_QUIT);
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
        CATCH.lock().take();
    }
}

#[cfg(test)]
mod catcher_tests {
    use super::*;

    #[test]
    fn only_keys_windows_hides_are_caught() {
        assert!(invisible_to_windows(0x2C));
        assert!(invisible_to_windows(0x91));
        assert!(!invisible_to_windows(0x41));
        assert!(!invisible_to_windows(0x79));
    }
}

#[cfg(test)]
mod catcher_live {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VIRTUAL_KEY,
    };

    fn key(flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(VK_SNAPSHOT as u16),
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        }
    }

    #[test]
    #[ignore = "needs a real desktop session"]
    fn print_screen_reaches_the_catcher() {
        let (sender, caught) = mpsc::channel();
        let sender = Mutex::new(sender);
        let catcher = KeyCatcher::start(move |hotkey| {
            sender.lock().send(hotkey).ok();
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
        unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        let hotkey = caught
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert_eq!(hotkey.key, 0x2C);
        drop(catcher);
    }
}
