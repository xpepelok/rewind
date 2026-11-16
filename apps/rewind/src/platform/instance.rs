use std::thread;
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForSingleObject,
};
use windows::core::w;

use crate::state::Signal;

const READY_WAIT: Duration = Duration::from_secs(10);
const READY_POLL: Duration = Duration::from_millis(100);

fn show_event() -> Option<HANDLE> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:(A;;GA;;;AU)S:(ML;;NW;;;ME)"),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .ok()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let event = unsafe { CreateEventW(Some(&attributes), false, false, w!("Local\\Rewind.Show")) };
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    event.ok()
}

pub struct Instance {
    mutex: HANDLE,
    event: HANDLE,
}

unsafe impl Send for Instance {}

impl Instance {
    pub fn acquire() -> Option<Self> {
        let mutex = unsafe { CreateMutexW(None, true, w!("Local\\Rewind.Instance")) }.ok()?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            let _ = unsafe { CloseHandle(mutex) };
            return None;
        }
        let event = show_event()?;
        Some(Self { mutex, event })
    }

    pub fn signal_running() -> bool {
        let Ok(event) =
            (unsafe { OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\Rewind.Show")) })
        else {
            return false;
        };
        let _ = unsafe { SetEvent(event) };
        let _ = unsafe { CloseHandle(event) };
        true
    }

    pub fn signal_when_ready() {
        let deadline = Instant::now() + READY_WAIT;
        while !Self::signal_running() && Instant::now() < deadline {
            thread::sleep(READY_POLL);
        }
    }

    pub fn listen(&self, signals: UnboundedSender<Signal>) {
        let event = self.event.0 as usize;
        thread::Builder::new()
            .name("rewind-instance".to_owned())
            .spawn(move || {
                let event = HANDLE(event as *mut core::ffi::c_void);
                while unsafe { WaitForSingleObject(event, INFINITE) }.0 == 0 {
                    if signals.unbounded_send(Signal::Show).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.event) };
        let _ = unsafe { CloseHandle(self.mutex) };
    }
}
