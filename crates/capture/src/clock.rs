use std::sync::OnceLock;

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::format::HNS_PER_SECOND;

fn frequency() -> i64 {
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut value = 0;
        let _ = unsafe { QueryPerformanceFrequency(&mut value) };
        value.max(1)
    })
}

pub fn now() -> i64 {
    let mut counter = 0;
    let _ = unsafe { QueryPerformanceCounter(&mut counter) };
    (i128::from(counter) * i128::from(HNS_PER_SECOND) / i128::from(frequency())) as i64
}
