use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteKeyW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW,
    RegSetValueExW,
};
use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_TYPE, OpenSCManagerW,
    OpenServiceW, QueryServiceStatus, SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_CHANGE_CONFIG,
    SERVICE_CONTROL_STOP, SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_START_TYPE, SERVICE_STATUS,
    SERVICE_STOP, StartServiceW,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, w};

pub const INSTALL_FLAG: &str = "--install-system";
pub const UNINSTALL_FLAG: &str = "--uninstall-system";
pub const NVIDIA_FLAG: &str = "--nvidia";
pub const ELEVATED_FLAG: &str = "--elevated";
pub const RELAUNCH_FLAG: &str = "--relaunch";

const APP_NAME: &str = "Rewind";
const EXE_NAME: &str = "Rewind.exe";
const NVIDIA_SERVICE: &str = "NvContainerLocalSystem";
const TASK_PREFIX: &str = "\\NVIDIA App SelfUpdate";
const REDIRECTED: [&str; 2] = ["NVIDIA Overlay.exe", "NVIDIA App.exe"];
const LOGON_TASK: &str = "Rewind";
const TASK_FILE: &str = "rewind-task.xml";
const STATE_KEY: &str = "SOFTWARE\\Rewind";
const IFEO_KEY: &str =
    "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const UNINSTALL_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Rewind";
const SERVICE_KEY: &str = "SYSTEM\\CurrentControlSet\\Services\\NvContainerLocalSystem";
const INSTALLED_VALUE: &str = "Installed";
const START_VALUE: &str = "ServiceStart";
const RUNNING_VALUE: &str = "ServiceWasRunning";
const TASKS_VALUE: &str = "DisabledTasks";
const DEBUGGER_VALUE: &str = "Debugger";
const START_DISABLED: u32 = 4;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const STOP_WAIT: Duration = Duration::from_millis(250);
const STOP_ATTEMPTS: u32 = 40;
const SUCCESS_CODE: u32 = 0;

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(Some(0)).collect()
}

fn error(code: windows::Win32::Foundation::WIN32_ERROR) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code.0 as i32))
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn create_key(root: HKEY, path: &str) -> io::Result<Key> {
    let mut key = HKEY::default();
    error(unsafe {
        RegCreateKeyExW(
            root,
            &HSTRING::from(path),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            None,
        )
    })?;
    Ok(Key(key))
}

fn set_string(root: HKEY, path: &str, name: &str, value: &str) -> io::Result<()> {
    let key = create_key(root, path)?;
    let data = wide(value);
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
    error(unsafe { RegSetValueExW(key.0, &HSTRING::from(name), None, REG_SZ, Some(bytes)) })
}

fn set_dword(root: HKEY, path: &str, name: &str, value: u32) -> io::Result<()> {
    let key = create_key(root, path)?;
    error(unsafe {
        RegSetValueExW(
            key.0,
            &HSTRING::from(name),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    })
}

fn get_dword(root: HKEY, path: &str, name: &str) -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(path),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

pub(crate) fn get_string(root: HKEY, path: &str, name: &str) -> Option<String> {
    let mut size = 0u32;
    let (path, name) = (HSTRING::from(path), HSTRING::from(name));
    let status = unsafe {
        RegGetValueW(
            root,
            &path,
            &name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            root,
            &path,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then(|| {
        let length = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..length])
    })
}

fn delete_value(root: HKEY, path: &str, name: &str) {
    let mut key = HKEY::default();
    let opened = unsafe {
        RegOpenKeyExW(
            root,
            &HSTRING::from(path),
            None,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == ERROR_SUCCESS {
        let key = Key(key);
        let _ = unsafe { RegDeleteValueW(key.0, &HSTRING::from(name)) };
    }
}

fn is_empty_key(root: HKEY, path: &str) -> bool {
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(root, &HSTRING::from(path), None, KEY_READ, &mut key) }
        != ERROR_SUCCESS
    {
        return false;
    }
    let key = Key(key);
    let mut subkeys = 0u32;
    let mut values = 0u32;
    let status = unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
    };
    status == ERROR_SUCCESS && subkeys == 0 && values == 0
}

struct Service(SC_HANDLE);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn open_service(access: u32) -> io::Result<(Service, Service)> {
    let manager = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map_err(io::Error::other)?;
    let manager = Service(manager);
    let service = unsafe { OpenServiceW(manager.0, &HSTRING::from(NVIDIA_SERVICE), access) }
        .map_err(io::Error::other)?;
    Ok((manager, Service(service)))
}

fn service_running(service: &Service) -> bool {
    let mut status = SERVICE_STATUS::default();
    unsafe { QueryServiceStatus(service.0, &mut status) }.is_ok()
        && status.dwCurrentState == SERVICE_RUNNING
}

fn set_service_start(service: &Service, start: u32) -> io::Result<()> {
    unsafe {
        ChangeServiceConfigW(
            service.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            SERVICE_START_TYPE(start),
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    }
    .map_err(io::Error::other)
}

fn stop_service(service: &Service) {
    let mut status = SERVICE_STATUS::default();
    let _ = unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) };
    for _ in 0..STOP_ATTEMPTS {
        if !service_running(service) {
            break;
        }
        std::thread::sleep(STOP_WAIT);
    }
}

fn hidden(program: &str) -> Command {
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn nvidia_tasks() -> Vec<String> {
    let Ok(output) = hidden("schtasks.exe")
        .args(["/Query", "/FO", "CSV", "/NH"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names: Vec<String> = text
        .lines()
        .filter_map(|line| line.split("\",\"").next())
        .map(|field| field.trim_matches('"').to_owned())
        .filter(|name| name.starts_with(TASK_PREFIX))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn set_task(name: &str, enabled: bool) {
    let toggle = if enabled { "/Enable" } else { "/Disable" };
    hidden("schtasks.exe")
        .args(["/Change", "/TN", name, toggle])
        .output()
        .ok();
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn task_xml(user: &str, target: &Path) -> String {
    let user = escaped(user);
    let command = escaped(&target.display().to_string());
    let directory = escaped(
        &target
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_default(),
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{ELEVATED_FLAG}</Arguments>
      <WorkingDirectory>{directory}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn current_user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    }
}

fn register_logon_task(target: &Path) -> io::Result<()> {
    let file = std::env::temp_dir().join(TASK_FILE);
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(
        task_xml(&current_user(), target)
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    std::fs::write(&file, bytes)?;
    let created = hidden("schtasks.exe")
        .args(["/Create", "/TN", LOGON_TASK, "/XML"])
        .arg(&file)
        .arg("/F")
        .output();
    let _ = std::fs::remove_file(&file);
    if created?.status.success() {
        Ok(())
    } else {
        Err(io::Error::other("logon task was not registered"))
    }
}

fn remove_logon_task() {
    hidden("schtasks.exe")
        .args(["/Delete", "/TN", LOGON_TASK, "/F"])
        .output()
        .ok();
}

pub fn needs_elevation() -> bool {
    !is_admin()
        && !std::env::args().any(|arg| arg == ELEVATED_FLAG)
        && std::env::current_exe().is_ok_and(|current| current == install_dir().join(EXE_NAME))
}

pub fn start_elevated() -> bool {
    hidden("schtasks.exe")
        .args(["/Run", "/TN", LOGON_TASK])
        .output()
        .is_ok_and(|output| output.status.success())
}

pub fn apply_update(installer: &Path) -> io::Result<()> {
    hidden(&installer.display().to_string())
        .args([INSTALL_FLAG, RELAUNCH_FLAG])
        .spawn()
        .map(drop)
}

pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from("C:\\Program Files"), PathBuf::from);
    base.join(APP_NAME)
}

pub fn is_installed() -> bool {
    get_dword(HKEY_LOCAL_MACHINE, STATE_KEY, INSTALLED_VALUE).is_some_and(|value| value == 1)
}

pub fn installed_version() -> Option<String> {
    get_string(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, "DisplayVersion")
}

pub fn running_installed_copy() -> bool {
    std::env::current_exe().is_ok_and(|current| current == install_dir().join(EXE_NAME))
}

fn backup_service() -> io::Result<()> {
    if get_dword(HKEY_LOCAL_MACHINE, STATE_KEY, START_VALUE).is_some() {
        return Ok(());
    }
    let start = get_dword(HKEY_LOCAL_MACHINE, SERVICE_KEY, "Start").unwrap_or(2);
    let (_manager, service) = open_service(SERVICE_QUERY_STATUS)?;
    set_dword(HKEY_LOCAL_MACHINE, STATE_KEY, START_VALUE, start)?;
    set_dword(
        HKEY_LOCAL_MACHINE,
        STATE_KEY,
        RUNNING_VALUE,
        u32::from(service_running(&service)),
    )
}

pub fn install() -> io::Result<()> {
    let directory = install_dir();
    std::fs::create_dir_all(&directory)?;
    let target = directory.join(EXE_NAME);
    let current = std::env::current_exe()?;
    stop_running();
    sweep_stale(&directory);
    if current != target && std::fs::copy(&current, &target).is_err() {
        let stale = directory.join(format!("{EXE_NAME}.{}.old", std::process::id()));
        std::fs::rename(&target, &stale)?;
        schedule_removal(&stale);
        std::fs::copy(&current, &target)?;
    }
    let launch = format!("\"{}\"", target.display());

    backup_service()?;
    let (_manager, service) = open_service(
        SERVICE_CHANGE_CONFIG | SERVICE_STOP | SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG,
    )?;
    set_service_start(&service, START_DISABLED)?;
    stop_service(&service);

    if get_string(HKEY_LOCAL_MACHINE, STATE_KEY, TASKS_VALUE).is_none() {
        let tasks = nvidia_tasks();
        for task in &tasks {
            set_task(task, false);
        }
        set_string(
            HKEY_LOCAL_MACHINE,
            STATE_KEY,
            TASKS_VALUE,
            &tasks.join("\n"),
        )?;
    }

    for image in REDIRECTED {
        set_string(
            HKEY_LOCAL_MACHINE,
            &format!("{IFEO_KEY}\\{image}"),
            DEBUGGER_VALUE,
            &format!("{launch} {NVIDIA_FLAG}"),
        )?;
    }
    stop_nvidia_leftovers();
    register_logon_task(&target)?;
    delete_value(HKEY_CURRENT_USER, RUN_KEY, APP_NAME);
    set_string(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, "DisplayName", APP_NAME)?;
    set_string(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, "Publisher", APP_NAME)?;
    set_string(
        HKEY_LOCAL_MACHINE,
        UNINSTALL_KEY,
        "DisplayVersion",
        env!("CARGO_PKG_VERSION"),
    )?;
    set_string(
        HKEY_LOCAL_MACHINE,
        UNINSTALL_KEY,
        "DisplayIcon",
        &target.display().to_string(),
    )?;
    set_string(
        HKEY_LOCAL_MACHINE,
        UNINSTALL_KEY,
        "UninstallString",
        &format!("{launch} {UNINSTALL_FLAG}"),
    )?;
    set_dword(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, "NoModify", 1)?;
    set_dword(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, "NoRepair", 1)?;
    set_dword(HKEY_LOCAL_MACHINE, STATE_KEY, INSTALLED_VALUE, 1)
}

const EXIT_WAIT_MS: u32 = 5000;
const LEFTOVER_WAIT_MS: u32 = 2000;
const NVIDIA_CONTAINER: &str = "nvcontainer.exe";
const NVIDIA_LEFTOVERS: [&str; 6] = [
    "NVIDIA Overlay.exe",
    "NVIDIA App.exe",
    "NVIDIA Share.exe",
    "NVIDIA Web Helper.exe",
    "nvsphelper64.exe",
    NVIDIA_CONTAINER,
];

fn nvidia_leftover(name: &str, started_by_rewind: bool) -> bool {
    let container = name.eq_ignore_ascii_case(NVIDIA_CONTAINER);
    NVIDIA_LEFTOVERS
        .iter()
        .any(|leftover| name.eq_ignore_ascii_case(leftover))
        && !(container && started_by_rewind)
}

fn stop_nvidia_leftovers() {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };

    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        processes.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..length]),
        ));
        more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { CloseHandle(snapshot) };

    let ours: Vec<u32> = processes
        .iter()
        .filter(|(_, _, name)| name.eq_ignore_ascii_case(EXE_NAME))
        .map(|(pid, _, _)| *pid)
        .collect();
    for (pid, parent, name) in &processes {
        if *pid == 0 || !nvidia_leftover(name, ours.contains(parent)) {
            continue;
        }
        if let Ok(handle) =
            unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, *pid) }
        {
            let _ = unsafe { TerminateProcess(handle, 1) };
            let _ = unsafe { WaitForSingleObject(handle, LEFTOVER_WAIT_MS) };
            let _ = unsafe { CloseHandle(handle) };
        }
    }
}

fn installed_image(image: &Path, directory: &Path) -> bool {
    let same_folder = image.parent().is_some_and(|parent| {
        parent
            .as_os_str()
            .eq_ignore_ascii_case(directory.as_os_str())
    });
    let ours = image
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case(EXE_NAME)
                || (name.len() > EXE_NAME.len()
                    && name.is_char_boundary(EXE_NAME.len())
                    && name[..EXE_NAME.len()].eq_ignore_ascii_case(EXE_NAME)
                    && name.ends_with(".old"))
        });
    same_folder && ours
}

fn sweep_stale(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "old")
            && installed_image(&path, directory)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn stop_running() {
    use windows::Win32::Foundation::MAX_PATH;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    };
    use windows::core::PWSTR;

    let directory = install_dir();
    let current = std::process::id();
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while more {
        let pid = entry.th32ProcessID;
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if pid != current && pid != 0 && name.eq_ignore_ascii_case(EXE_NAME) {
            if let Ok(handle) = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
            } {
                let mut buffer = [0u16; MAX_PATH as usize];
                let mut size = buffer.len() as u32;
                let resolved = unsafe {
                    QueryFullProcessImageNameW(
                        handle,
                        PROCESS_NAME_WIN32,
                        PWSTR(buffer.as_mut_ptr()),
                        &mut size,
                    )
                };
                if resolved.is_ok()
                    && installed_image(
                        &PathBuf::from(String::from_utf16_lossy(&buffer[..size as usize])),
                        &directory,
                    )
                {
                    let _ = unsafe { TerminateProcess(handle, 1) };
                    let _ = unsafe { WaitForSingleObject(handle, EXIT_WAIT_MS) };
                }
                let _ = unsafe { CloseHandle(handle) };
            }
        }
        more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { CloseHandle(snapshot) };
}

fn schedule_removal(path: &Path) {
    let wide_path = HSTRING::from(path.as_os_str());
    let _ = unsafe { MoveFileExW(&wide_path, PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
}

pub fn uninstall() -> io::Result<()> {
    for image in REDIRECTED {
        let key = format!("{IFEO_KEY}\\{image}");
        delete_value(HKEY_LOCAL_MACHINE, &key, DEBUGGER_VALUE);
        if is_empty_key(HKEY_LOCAL_MACHINE, &key) {
            let _ = unsafe { RegDeleteKeyW(HKEY_LOCAL_MACHINE, &HSTRING::from(key.as_str())) };
        }
    }

    if let Some(start) = get_dword(HKEY_LOCAL_MACHINE, STATE_KEY, START_VALUE) {
        let was_running =
            get_dword(HKEY_LOCAL_MACHINE, STATE_KEY, RUNNING_VALUE).is_some_and(|value| value == 1);
        let (_manager, service) =
            open_service(SERVICE_CHANGE_CONFIG | SERVICE_START | SERVICE_QUERY_STATUS)?;
        set_service_start(&service, start)?;
        if was_running && !service_running(&service) {
            let _ = unsafe { StartServiceW(service.0, None) };
        }
    }
    if let Some(tasks) = get_string(HKEY_LOCAL_MACHINE, STATE_KEY, TASKS_VALUE) {
        for task in tasks.lines().filter(|task| !task.is_empty()) {
            set_task(task, true);
        }
    }

    remove_logon_task();
    stop_running();
    delete_value(HKEY_CURRENT_USER, RUN_KEY, APP_NAME);
    let _ = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(UNINSTALL_KEY)) };
    let _ = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(STATE_KEY)) };
    let _ = unsafe { RegDeleteKeyW(HKEY_LOCAL_MACHINE, &HSTRING::from(STATE_KEY)) };

    let directory = install_dir();
    let target = directory.join(EXE_NAME);
    if std::env::current_exe().is_ok_and(|current| current == target) {
        schedule_removal(&target);
        schedule_removal(&directory);
    } else {
        for attempt in 0..STOP_ATTEMPTS {
            if std::fs::remove_dir_all(&directory).is_ok() || !directory.exists() {
                break;
            }
            if attempt + 1 == STOP_ATTEMPTS {
                schedule_removal(&target);
                schedule_removal(&directory);
            } else {
                std::thread::sleep(STOP_WAIT);
            }
        }
    }
    Ok(())
}

pub fn trim_memory() {
    use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    let _ = unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() }.as_bool()
}

pub fn perform(flag: &str) -> bool {
    if is_admin() {
        let result = if flag == INSTALL_FLAG {
            install()
        } else {
            uninstall()
        };
        result.is_ok()
    } else {
        run_elevated(flag).unwrap_or(false)
    }
}

pub fn relaunch_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let file = HSTRING::from(exe.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.is_ok() && !info.hProcess.is_invalid()
}

pub fn run_elevated(flag: &str) -> io::Result<bool> {
    let exe = std::env::current_exe()?;
    let file = HSTRING::from(exe.as_os_str());
    let parameters = HSTRING::from(format!("{flag} {ELEVATED_FLAG}"));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(io::Error::other)?;
    if info.hProcess.is_invalid() {
        return Ok(false);
    }
    let finished = unsafe { WaitForSingleObject(info.hProcess, INFINITE) } == WAIT_OBJECT_0;
    let mut code = 1u32;
    let _ = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
    let _ = unsafe { CloseHandle(info.hProcess) };
    Ok(finished && code == SUCCESS_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logon_task_runs_the_app_with_the_highest_rights_of_its_user() {
        let xml = task_xml(
            "PC\\R&D",
            Path::new("C:\\Program Files\\Rewind\\Rewind.exe"),
        );
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<Arguments>--elevated</Arguments>"));
        assert_eq!(xml.matches("<UserId>PC\\R&amp;D</UserId>").count(), 2);
        assert!(xml.contains("<Command>C:\\Program Files\\Rewind\\Rewind.exe</Command>"));
        assert!(xml.contains("<WorkingDirectory>C:\\Program Files\\Rewind</WorkingDirectory>"));
    }

    #[test]
    fn only_the_overlay_stack_is_stopped_and_our_own_container_is_spared() {
        assert!(nvidia_leftover("NVIDIA Overlay.exe", false));
        assert!(nvidia_leftover("nvidia app.EXE", false));
        assert!(nvidia_leftover("nvsphelper64.exe", false));
        assert!(nvidia_leftover("nvcontainer.exe", false));
        assert!(!nvidia_leftover("nvcontainer.exe", true));
        assert!(nvidia_leftover("NVIDIA Overlay.exe", true));
        assert!(!nvidia_leftover("NVDisplay.Container.exe", false));
        assert!(!nvidia_leftover("NVIDIA Broadcast.exe", false));
        assert!(!nvidia_leftover("Rewind.exe", false));
    }

    #[test]
    fn a_renamed_old_copy_still_counts_as_the_installed_app() {
        let directory = Path::new(r"C:\Program Files\Rewind");
        let inside = |name: &str| installed_image(&directory.join(name), directory);
        assert!(inside("Rewind.exe"));
        assert!(inside("rewind.EXE"));
        assert!(inside("Rewind.exe.43772.old"));
        assert!(!inside("Rewind.exe.bak"));
        assert!(!inside("Other.exe"));
        assert!(!installed_image(
            Path::new(r"D:\kadr-build\Rewind.exe"),
            directory
        ));
        assert!(installed_image(
            Path::new(r"c:\program files\rewind\Rewind.exe"),
            directory
        ));
    }
}
