use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFShutdown, IMFTransform, MFSTARTUP_FULL, MFShutdown, MFStartup,
    MFT_ENUM_FLAG, MFT_FRIENDLY_NAME_Attribute, MFT_REGISTER_TYPE_INFO, MFTEnum2,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::{GUID, Interface};

use crate::error::Result;

const MF_VERSION: u32 = 0x0002_0070;
const NAME_CAPACITY: usize = 128;

pub struct Runtime {
    com: bool,
}

impl Runtime {
    pub fn enter() -> Result<Self> {
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }?;
        Ok(Self { com })
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
        if self.com {
            unsafe { CoUninitialize() };
        }
    }
}

pub struct Transform {
    pub transform: IMFTransform,
    pub activate: IMFActivate,
    pub name: String,
}

pub fn shut_down(transform: &IMFTransform, activate: &IMFActivate) {
    if let Ok(shutdown) = transform.cast::<IMFShutdown>() {
        let _ = unsafe { shutdown.Shutdown() };
    }
    let _ = unsafe { activate.ShutdownObject() };
}

fn enumerate(
    category: GUID,
    flags: MFT_ENUM_FLAG,
    input: (GUID, GUID),
    output: (GUID, GUID),
    attributes: Option<&IMFAttributes>,
) -> Result<(*mut Option<IMFActivate>, u32)> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: input.0,
        guidSubtype: input.1,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: output.0,
        guidSubtype: output.1,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        MFTEnum2(
            category,
            flags,
            Some(&input),
            Some(&output),
            attributes,
            &mut activates,
            &mut count,
        )
    }?;
    Ok((activates, count))
}

pub fn transform_count(
    category: GUID,
    flags: MFT_ENUM_FLAG,
    input: (GUID, GUID),
    output: (GUID, GUID),
) -> Result<usize> {
    let (activates, count) = enumerate(category, flags, input, output, None)?;
    if activates.is_null() {
        return Ok(0);
    }
    let list = unsafe { std::slice::from_raw_parts_mut(activates, count as usize) };
    let found = list.iter_mut().filter_map(Option::take).count();
    unsafe { CoTaskMemFree(Some(activates as *const _)) };
    Ok(found)
}

pub fn first_transform(
    category: GUID,
    flags: MFT_ENUM_FLAG,
    input: (GUID, GUID),
    output: (GUID, GUID),
    attributes: Option<&IMFAttributes>,
) -> Result<Option<Transform>> {
    let (activates, count) = enumerate(category, flags, input, output, attributes)?;
    if activates.is_null() {
        return Ok(None);
    }
    let list = unsafe { std::slice::from_raw_parts_mut(activates, count as usize) };
    let mut found = None;
    for slot in list.iter_mut() {
        let Some(activate) = slot.take() else {
            continue;
        };
        if found.is_none()
            && let Ok(transform) = unsafe { activate.ActivateObject::<IMFTransform>() }
        {
            found = Some(Transform {
                transform,
                name: friendly_name(&activate),
                activate,
            });
        }
    }
    unsafe { CoTaskMemFree(Some(activates as *const _)) };
    Ok(found)
}

fn friendly_name(activate: &IMFActivate) -> String {
    let mut buffer = [0u16; NAME_CAPACITY];
    let mut length = 0u32;
    match unsafe {
        activate.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buffer, Some(&mut length))
    } {
        Ok(()) => String::from_utf16_lossy(&buffer[..length as usize]),
        Err(_) => String::new(),
    }
}
