use windows::Win32::Media::MediaFoundation::{MFSTARTUP_FULL, MFShutdown, MFStartup};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::error::Result;

const MF_VERSION: u32 = 0x0002_0070;

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
