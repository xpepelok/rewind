use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, E_NOINTERFACE, E_POINTER, ERROR_TIMEOUT, S_OK, WAIT_TIMEOUT,
};
use windows::Win32::Media::Audio::{
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Vtbl, IAudioClient,
    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{BLOB, IAgileObject};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface};

use crate::error::Result;

const NAMES: [&str; 3] = ["discord.exe", "discordptb.exe", "discordcanary.exe"];
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capture {
    Only,
    Without,
}

fn is_discord(name: &str) -> bool {
    NAMES.iter().any(|known| name.eq_ignore_ascii_case(known))
}

pub fn root(processes: &[(u32, u32, String)]) -> Option<u32> {
    let members: HashSet<u32> = processes
        .iter()
        .filter(|(_, _, name)| is_discord(name))
        .map(|(id, _, _)| *id)
        .collect();
    let children = |root: u32| {
        processes
            .iter()
            .filter(|(id, parent, _)| *parent == root && members.contains(id))
            .count()
    };
    processes
        .iter()
        .filter(|(id, parent, name)| {
            is_discord(name) && (*parent == *id || !members.contains(parent))
        })
        .map(|(id, _, _)| (*id, children(*id)))
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(id, _)| id)
}

pub fn processes() -> Vec<(u32, u32, String)> {
    let mut list = Vec::new();
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return list;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while found {
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        list.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..length]),
        ));
        found = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { CloseHandle(snapshot) };
    list
}

pub fn running() -> Option<u32> {
    root(&processes())
}

pub fn alive(process: u32) -> Option<bool> {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, process) }.ok()?;
    let state = unsafe { WaitForSingleObject(handle, 0) };
    let _ = unsafe { CloseHandle(handle) };
    Some(state == WAIT_TIMEOUT)
}

#[repr(C)]
struct Completion {
    vtable: *const IActivateAudioInterfaceCompletionHandler_Vtbl,
    references: AtomicU32,
    done: mpsc::Sender<()>,
}

static VTABLE: IActivateAudioInterfaceCompletionHandler_Vtbl =
    IActivateAudioInterfaceCompletionHandler_Vtbl {
        base__: IUnknown_Vtbl {
            QueryInterface: query_interface,
            AddRef: add_ref,
            Release: release,
        },
        ActivateCompleted: activate_completed,
    };

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || interface.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { *iid };
    if iid == IUnknown::IID
        || iid == IActivateAudioInterfaceCompletionHandler::IID
        || iid == IAgileObject::IID
    {
        unsafe {
            add_ref(this);
            *interface = this;
        }
        S_OK
    } else {
        unsafe { *interface = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    let completion = unsafe { &*(this as *const Completion) };
    completion.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let remaining = {
        let completion = unsafe { &*(this as *const Completion) };
        completion.references.fetch_sub(1, Ordering::AcqRel) - 1
    };
    if remaining == 0 {
        drop(unsafe { Box::from_raw(this as *mut Completion) });
    }
    remaining
}

unsafe extern "system" fn activate_completed(this: *mut c_void, _: *mut c_void) -> HRESULT {
    let completion = unsafe { &*(this as *const Completion) };
    completion.done.send(()).ok();
    S_OK
}

fn completion() -> (IActivateAudioInterfaceCompletionHandler, mpsc::Receiver<()>) {
    let (done, finished) = mpsc::channel();
    let raw = Box::into_raw(Box::new(Completion {
        vtable: &VTABLE,
        references: AtomicU32::new(1),
        done,
    }));
    let handler = unsafe { IActivateAudioInterfaceCompletionHandler::from_raw(raw as *mut c_void) };
    (handler, finished)
}

pub fn client(process: u32, capture: Capture) -> Result<IAudioClient> {
    let mode = match capture {
        Capture::Only => PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        Capture::Without => PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    };
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: process,
                ProcessLoopbackMode: mode,
            },
        },
    };
    let variant = ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: &params as *const AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8,
                    },
                },
            }),
        },
    });
    let (handler, finished) = completion();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*variant),
            &handler,
        )
    }?;
    finished
        .recv_timeout(ACTIVATION_TIMEOUT)
        .map_err(|_| windows::core::Error::from(ERROR_TIMEOUT.to_hresult()))?;
    let mut status = S_OK;
    let mut activated: Option<IUnknown> = None;
    unsafe { operation.GetActivateResult(&mut status, &mut activated) }?;
    status.ok()?;
    let activated = activated.ok_or_else(|| windows::core::Error::from(E_NOINTERFACE))?;
    Ok(activated.cast()?)
}

#[cfg(test)]
mod tests {
    use super::root;

    fn list(entries: &[(u32, u32, &str)]) -> Vec<(u32, u32, String)> {
        entries
            .iter()
            .map(|(id, parent, name)| (*id, *parent, (*name).to_owned()))
            .collect()
    }

    #[test]
    fn the_root_is_the_discord_process_without_a_discord_parent() {
        let processes = list(&[
            (4, 0, "System"),
            (100, 4, "explorer.exe"),
            (210, 220, "Discord.exe"),
            (220, 100, "Discord.exe"),
            (230, 220, "Discord.exe"),
            (240, 220, "Discord.exe"),
            (300, 100, "chrome.exe"),
        ]);
        assert_eq!(root(&processes), Some(220));
    }

    #[test]
    fn other_channels_are_recognised_by_name() {
        let processes = list(&[(100, 4, "explorer.exe"), (500, 100, "DiscordCanary.exe")]);
        assert_eq!(root(&processes), Some(500));
        let processes = list(&[(100, 4, "explorer.exe"), (600, 100, "discordptb.exe")]);
        assert_eq!(root(&processes), Some(600));
    }

    #[test]
    fn without_discord_there_is_no_root() {
        let processes = list(&[
            (100, 4, "explorer.exe"),
            (200, 100, "Discordia.exe"),
            (300, 100, "notdiscord.exe"),
        ]);
        assert_eq!(root(&processes), None);
        assert_eq!(root(&[]), None);
    }

    #[test]
    fn the_busiest_tree_wins_when_two_clients_run() {
        let processes = list(&[
            (100, 4, "explorer.exe"),
            (700, 100, "DiscordPTB.exe"),
            (800, 100, "Discord.exe"),
            (810, 800, "Discord.exe"),
            (820, 800, "Discord.exe"),
            (710, 700, "DiscordPTB.exe"),
        ]);
        assert_eq!(root(&processes), Some(800));
    }

    #[test]
    fn an_orphaned_child_still_counts_as_a_root() {
        let processes = list(&[(100, 4, "explorer.exe"), (900, 12345, "Discord.exe")]);
        assert_eq!(root(&processes), Some(900));
    }
}
