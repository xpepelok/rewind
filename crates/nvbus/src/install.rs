use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{PCWSTR, s, w};

use crate::Error;

const BUS_KEY: PCWSTR = w!("SOFTWARE\\NVIDIA Corporation\\NvContainer\\MessageBus");
const INSTALL_PATH: PCWSTR = w!("InstallPath");
const CONTAINER: &str = "NVIDIA Corporation\\NvContainer\\nvcontainer.exe";
pub const CLIENT_LIBRARY: &str = "MessageBus.dll";
pub const BROADCASTER: &str = "NvMessageBusBroadcast.dll";
pub const SETTINGS: &str = "messagebus.conf";
const DRIVER_STORE: &str = "DriverStore\\FileRepository";
const DRIVER_CONTAINER: &str = "Display.NvContainer";
const DRIVER_CONFIG: &str = "messagebus_client.conf";
const DRIVER_CAMERA: &str = "NvCamera\\NvCamera64.dll";
const CAMERA_KEY: PCWSTR = w!("SOFTWARE\\NVIDIA Corporation\\Ansel");
const REMOTE_UI: PCWSTR = w!("IPCenabled");
const FILTERS: PCWSTR = w!("FreestyleEnabled");
const PATH_LIMIT: usize = 1024;

pub fn bus_directory() -> Result<PathBuf, Error> {
    let mut buffer = [0u16; PATH_LIMIT];
    let mut size = (PATH_LIMIT * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            BUS_KEY,
            INSTALL_PATH,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return Err(Error::Missing("путь установки канала сообщений"));
    }
    let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(0);
    let path = PathBuf::from(OsString::from_wide(&buffer[..length]));
    if path.join(CLIENT_LIBRARY).is_file() {
        Ok(path)
    } else {
        Err(Error::Missing("библиотека канала сообщений"))
    }
}

pub fn container() -> Result<PathBuf, Error> {
    let root = std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .ok_or(Error::Missing("папка Program Files"))?;
    let path = PathBuf::from(root).join(CONTAINER);
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error::Missing("контейнер NVIDIA"))
    }
}

fn driver_file(relative: &str) -> Result<PathBuf, Error> {
    let store =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32")
            .join(DRIVER_STORE);
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&store)?.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("nv_disp") {
            continue;
        }
        let candidate = entry.path().join(relative);
        let Ok(stamp) = std::fs::metadata(&candidate).and_then(|data| data.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(best, _)| stamp >= *best) {
            newest = Some((stamp, candidate));
        }
    }
    newest
        .map(|(_, path)| path)
        .ok_or(Error::Missing("файл драйвера NVIDIA"))
}

pub fn driver_configs() -> Result<Vec<(&'static str, String)>, Error> {
    let path = driver_file(&format!("{DRIVER_CONTAINER}\\{DRIVER_CONFIG}"))?;
    let driver = String::from_utf8_lossy(&std::fs::read(&path)?).into_owned();
    let application = bus_directory()
        .ok()
        .and_then(|directory| std::fs::read(directory.join(SETTINGS)).ok())
        .map(|raw| String::from_utf8_lossy(&raw).into_owned());
    let mut configs = crate::config::candidates(&driver, application.as_deref());
    configs.push((
        crate::config::DRIVER_PATH,
        path.to_string_lossy().into_owned(),
    ));
    Ok(configs)
}

fn camera_text(name: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 64];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            CAMERA_KEY,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buffer[..length]))
}

fn set_camera_text(name: PCWSTR, value: &str) -> Result<(), Error> {
    let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            CAMERA_KEY,
            name,
            REG_SZ.0,
            Some(wide.as_ptr().cast()),
            (wide.len() * 2) as u32,
        )
    };
    if status.is_err() {
        return Err(Error::Missing("настройки камеры NVIDIA"));
    }
    Ok(())
}

pub fn filters_enabled() -> bool {
    let on = |name| {
        camera_text(name).is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    };
    on(REMOTE_UI) && on(FILTERS)
}

pub fn set_filters_enabled(enabled: bool) -> Result<(), Error> {
    set_camera_text(REMOTE_UI, if enabled { "1" } else { "0" })?;
    let path: Vec<u16> = driver_file(DRIVER_CAMERA)?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let library =
        unsafe { LoadLibraryExW(PCWSTR(path.as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH) }
            .map_err(|_| Error::Library("модуль камеры не загрузился"))?;
    let switched = unsafe { GetProcAddress(library, s!("SetFreeStyleStatus")) }.map(|address| {
        let switch: unsafe extern "C" fn(u8) -> i32 = unsafe { std::mem::transmute(address) };
        unsafe { switch(u8::from(enabled)) }
    });
    let _ = unsafe { FreeLibrary(library) };
    match switched {
        Some(_) => Ok(()),
        None => Err(Error::Library("в модуле камеры нет переключателя фильтров")),
    }
}
