use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use util::ResultExt;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    System::Performance::QueryPerformanceFrequency,
};

static QPC_TICKS_PER_SECOND: LazyLock<u64> = LazyLock::new(|| {
    let mut frequency = 0;
    // On systems that run Windows XP or later, the function will always succeed and
    // will thus never return zero.
    unsafe { QueryPerformanceFrequency(&mut frequency).unwrap() };
    frequency as u64
});

static FRAME_REQUESTS: LazyLock<parking_lot::Mutex<HashMap<isize, Arc<crate::FrameRequest>>>> =
    LazyLock::new(Default::default);

static FRAME_SIGNAL: (parking_lot::Mutex<bool>, parking_lot::Condvar) =
    (parking_lot::Mutex::new(true), parking_lot::Condvar::new());

fn signal_frame_request() {
    let mut pending = FRAME_SIGNAL.0.lock();
    *pending = true;
    FRAME_SIGNAL.1.notify_one();
}

pub(crate) fn wait_for_frame_request(timeout: Duration) -> bool {
    let mut pending = FRAME_SIGNAL.0.lock();
    if !*pending {
        FRAME_SIGNAL.1.wait_for(&mut pending, timeout);
    }
    std::mem::replace(&mut *pending, false)
}

pub(crate) fn frame_request_flag(hwnd: HWND) -> Arc<crate::FrameRequest> {
    let flag = FRAME_REQUESTS
        .lock()
        .entry(hwnd.0 as isize)
        .or_insert_with(|| Arc::new(crate::FrameRequest::new(signal_frame_request)))
        .clone();
    signal_frame_request();
    flag
}

pub(crate) fn take_frame_request(hwnd: HWND) -> bool {
    FRAME_REQUESTS
        .lock()
        .get(&(hwnd.0 as isize))
        .is_none_or(|flag| flag.take())
}

pub(crate) fn forget_frame_request(hwnd: HWND) {
    FRAME_REQUESTS.lock().remove(&(hwnd.0 as isize));
}

const VSYNC_INTERVAL_THRESHOLD: Duration = Duration::from_millis(1);
const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_666); // ~60Hz

pub(crate) struct VSyncProvider {
    interval: Duration,
    f: Box<dyn Fn() -> bool>,
}

impl VSyncProvider {
    pub(crate) fn new() -> Self {
        let interval = get_dwm_interval()
            .context("Failed to get DWM interval")
            .log_err()
            .unwrap_or(DEFAULT_VSYNC_INTERVAL);
        let f = Box::new(|| unsafe { DwmFlush().is_ok() });
        Self { interval, f }
    }

    pub(crate) fn wait_for_vsync(&self) {
        let vsync_start = Instant::now();
        let wait_succeeded = (self.f)();
        let elapsed = vsync_start.elapsed();
        // DwmFlush and DCompositionWaitForCompositorClock returns very early
        // instead of waiting until vblank when the monitor goes to sleep or is
        // unplugged (nothing to present due to desktop occlusion). We use 1ms as
        // a threshold for the duration of the wait functions and fallback to
        // Sleep() if it returns before that. This could happen during normal
        // operation for the first call after the vsync thread becomes non-idle,
        // but it shouldn't happen often.
        if !wait_succeeded || elapsed < VSYNC_INTERVAL_THRESHOLD {
            log::trace!("VSyncProvider::wait_for_vsync() took less time than expected");
            std::thread::sleep(self.interval);
        }
    }
}

fn get_dwm_interval() -> Result<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing_info) }?;
    let interval = retrieve_duration(timing_info.qpcRefreshPeriod, *QPC_TICKS_PER_SECOND);
    // Check for interval values that are impossibly low. A 29 microsecond
    // interval was seen (from a qpcRefreshPeriod of 60).
    if interval < VSYNC_INTERVAL_THRESHOLD {
        Ok(retrieve_duration(
            timing_info.rateRefresh.uiDenominator as u64,
            timing_info.rateRefresh.uiNumerator as u64,
        ))
    } else {
        Ok(interval)
    }
}

#[inline]
fn retrieve_duration(counts: u64, ticks_per_second: u64) -> Duration {
    let ticks_per_microsecond = ticks_per_second / 1_000_000;
    Duration::from_micros(counts / ticks_per_microsecond)
}
