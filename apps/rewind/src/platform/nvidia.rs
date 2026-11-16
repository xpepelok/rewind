use rewind_config::{REPLAY_MAX_SECONDS, REPLAY_MIN_SECONDS, Settings};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_ANY, RegGetValueW};
use windows::core::HSTRING;

const CAPTURE_KEY: &str = r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS";
const REPLAY_ENABLED: &str = "DwmDvrEnabledV1";
const REPLAY_LENGTH: &str = "DVRBufferLen";

fn read_u32(name: &str) -> Option<u32> {
    let mut bytes = [0u8; 4];
    let mut size = bytes.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(CAPTURE_KEY),
            &HSTRING::from(name),
            RRF_RT_ANY,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS && size == 4).then(|| u32::from_le_bytes(bytes))
}

pub fn import(settings: &mut Settings) -> bool {
    if settings.imported_nvidia {
        return false;
    }
    settings.imported_nvidia = true;
    if let Some(enabled) = read_u32(REPLAY_ENABLED) {
        settings.replay_enabled = enabled != 0;
    }
    if let Some(seconds) = read_u32(REPLAY_LENGTH).filter(|seconds| *seconds > 0) {
        settings.video.replay_seconds = seconds.clamp(REPLAY_MIN_SECONDS, REPLAY_MAX_SECONDS);
    }
    true
}
