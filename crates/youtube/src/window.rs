use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub const COMPACT_WIDTH: i32 = 480;
pub const COMPACT_HEIGHT: i32 = 680;

const LOOKING: Duration = Duration::from_millis(25);
const KEEPING: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn right(self) -> i32 {
        self.x + self.width
    }

    pub const fn bottom(self) -> i32 {
        self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub owner: isize,
    pub content: Rect,
}

pub fn centred(area: Rect, width: i32, height: i32) -> Rect {
    let width = width.min(area.width).max(0);
    let height = height.min(area.height).max(0);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

pub fn scaled(length: i32, dpi: u32) -> i32 {
    let dpi = i64::from(if dpi == 0 { 96 } else { dpi });
    i32::try_from((i64::from(length) * dpi + 48) / 96).unwrap_or(length)
}

pub fn window_for(target: Rect, window: Rect, content: Rect) -> Rect {
    let left = content.x - window.x;
    let top = content.y - window.y;
    let right = window.right() - content.right();
    let bottom = window.bottom() - content.bottom();
    Rect::new(
        target.x - left,
        target.y - top,
        target.width + left + right,
        target.height + top + bottom,
    )
}

pub fn clip_for(window: Rect, content: Rect) -> Rect {
    Rect::new(
        content.x - window.x,
        content.y - window.y,
        content.width,
        content.height,
    )
}

pub fn window_arguments(bounds: Rect, dpi: u32) -> [String; 2] {
    let unscale = |length: i32| {
        let dpi = i64::from(if dpi == 0 { 96 } else { dpi });
        i32::try_from(i64::from(length) * 96 / dpi).unwrap_or(length)
    };
    [
        format!(
            "--window-size={},{}",
            unscale(bounds.width),
            unscale(bounds.height)
        ),
        format!(
            "--window-position={},{}",
            unscale(bounds.x),
            unscale(bounds.y)
        ),
    ]
}

#[derive(Default)]
struct State {
    placement: Option<Placement>,
    hidden: bool,
    cancelled: bool,
    running: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone, Default)]
pub struct SignInWindow {
    shared: Arc<Shared>,
}

impl SignInWindow {
    pub fn new(placement: Option<Placement>) -> Self {
        let window = Self::default();
        window.state().placement = placement;
        window
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn change(&self, edit: impl FnOnce(&mut State)) {
        edit(&mut self.state());
        self.shared.changed.notify_all();
    }

    pub fn placement(&self) -> Option<Placement> {
        self.state().placement
    }

    pub fn reposition(&self, content: Rect) {
        self.change(|state| {
            if let Some(placement) = state.placement.as_mut() {
                placement.content = content;
            }
        });
    }

    pub fn hide(&self) {
        self.change(|state| state.hidden = true);
    }

    pub fn show(&self) {
        self.change(|state| state.hidden = false);
    }

    pub fn cancel(&self) {
        self.change(|state| {
            state.cancelled = true;
            state.hidden = true;
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.state().cancelled
    }

    pub fn is_hidden(&self) -> bool {
        self.state().hidden
    }

    pub(crate) fn launch_bounds(&self) -> (Rect, u32) {
        native::launch_bounds(self.placement())
    }

    pub(crate) fn keep(&self, process: u32) -> Keeper {
        self.change(|state| state.running = true);
        let shared = Arc::clone(&self.shared);
        let thread = std::thread::Builder::new()
            .name("youtube-sign-in-window".to_string())
            .spawn(move || native::keep(&shared, process))
            .ok();
        Keeper {
            window: self.clone(),
            thread,
        }
    }
}

pub(crate) struct Keeper {
    window: SignInWindow,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Keeper {
    fn drop(&mut self) {
        self.window.change(|state| state.running = false);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
mod native {
    use super::{COMPACT_HEIGHT, COMPACT_WIDTH, KEEPING, LOOKING, Placement, Rect, Shared};
    use std::sync::PoisonError;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY,
        DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateRectRgn, GetMonitorInfoW, GetWindowRgnBox, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
        MonitorFromPoint, SIMPLEREGION, SetWindowRgn,
    };
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, GetDpiForWindow,
        MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GW_HWNDPREV, GWL_EXSTYLE, GetClassNameW, GetCursorPos,
        GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
        GetWindowThreadProcessId, HWND_TOPMOST, IsWindow, IsWindowVisible, SW_HIDE,
        SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST,
    };

    const FRAME_CLASS: &str = "Chrome_WidgetWin_1";
    const TOP_TRIM: i32 = 4;
    const CONTENT_CLASS: &str = "Chrome_RenderWidgetHostHWND";

    fn rect_of(hwnd: HWND) -> Option<Rect> {
        let mut raw = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        (unsafe { GetWindowRect(hwnd, &raw mut raw) } != 0).then(|| {
            Rect::new(
                raw.left,
                raw.top,
                raw.right - raw.left,
                raw.bottom - raw.top,
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn rect_for_tests(frame: isize) -> Option<Rect> {
        rect_of(frame as HWND)
    }

    fn class_of(hwnd: HWND) -> String {
        let mut buffer = [0u16; 64];
        let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), 64) };
        String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
    }

    unsafe extern "system" fn collect_frame(hwnd: HWND, found: LPARAM) -> i32 {
        let (process, windows) = unsafe { &mut *(found as *mut (u32, Vec<isize>)) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &raw mut owner) };
        if owner == *process
            && class_of(hwnd) == FRAME_CLASS
            && unsafe { GetWindowTextLengthW(hwnd) } > 0
        {
            windows.push(hwnd as isize);
        }
        1
    }

    pub(crate) fn frames(process: u32) -> Vec<isize> {
        let mut found: (u32, Vec<isize>) = (process, Vec::new());
        unsafe { EnumWindows(Some(collect_frame), &raw mut found as LPARAM) };
        found.1
    }

    unsafe extern "system" fn collect_content(hwnd: HWND, found: LPARAM) -> i32 {
        let best = unsafe { &mut *(found as *mut Option<Rect>) };
        if class_of(hwnd) == CONTENT_CLASS
            && unsafe { IsWindowVisible(hwnd) } != 0
            && let Some(rect) = rect_of(hwnd)
            && best.is_none_or(|known| rect.width * rect.height > known.width * known.height)
        {
            *best = Some(rect);
        }
        1
    }

    pub(crate) fn content_of(frame: isize) -> Option<Rect> {
        let mut best: Option<Rect> = None;
        unsafe {
            EnumChildWindows(
                frame as HWND,
                Some(collect_content),
                &raw mut best as LPARAM,
            )
        };
        best.filter(|rect| rect.width > 0 && rect.height > 0)
    }

    fn is_topmost(hwnd: isize) -> bool {
        let style = unsafe { GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE) };
        style as u32 & WS_EX_TOPMOST != 0
    }

    fn plain(hwnd: HWND) {
        let attributes = [
            (DWMWA_NCRENDERING_POLICY as u32, DWMNCRP_DISABLED as u32),
            (
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                DWMWCP_DONOTROUND as u32,
            ),
            (DWMWA_BORDER_COLOR as u32, DWMWA_COLOR_NONE),
            (DWMWA_TRANSITIONS_FORCEDISABLED as u32, 1),
        ];
        for (attribute, value) in attributes {
            unsafe {
                DwmSetWindowAttribute(hwnd, attribute, (&raw const value).cast(), 4);
            }
        }
    }

    const ABOVE_SEARCH: usize = 256;

    fn dress(frame: isize, main: bool) {
        let hwnd = frame as HWND;
        if main {
            plain(hwnd);
        }
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let wanted = (style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
            if wanted != style {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted as isize);
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn above(frame: isize, owner: isize) -> bool {
        let mut higher = owner as HWND;
        for _ in 0..ABOVE_SEARCH {
            higher = unsafe { GetWindow(higher, GW_HWNDPREV) };
            if higher.is_null() {
                return false;
            }
            if higher as isize == frame {
                return true;
            }
        }
        false
    }

    fn raise(frame: isize, topmost: bool) {
        unsafe {
            SetWindowPos(
                frame as HWND,
                if topmost {
                    HWND_TOPMOST
                } else {
                    std::ptr::null_mut()
                },
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    fn place(frame: isize, bounds: Rect, topmost: bool) {
        let mut flags = SWP_NOACTIVATE;
        if !topmost {
            flags |= SWP_NOZORDER;
        }
        unsafe {
            SetWindowPos(
                frame as HWND,
                if topmost {
                    HWND_TOPMOST
                } else {
                    std::ptr::null_mut()
                },
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                flags,
            );
        }
    }

    fn clip(frame: isize, region: Rect) {
        unsafe {
            let shape = CreateRectRgn(region.x, region.y, region.right(), region.bottom());
            if !shape.is_null() {
                SetWindowRgn(frame as HWND, shape, 1);
            }
        }
    }

    fn clipped(frame: isize) -> Option<Rect> {
        let mut raw = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let kind = unsafe { GetWindowRgnBox(frame as HWND, &raw mut raw) };
        (kind == SIMPLEREGION).then(|| {
            Rect::new(
                raw.left,
                raw.top,
                raw.right - raw.left,
                raw.bottom - raw.top,
            )
        })
    }

    fn set_shown(frame: isize, shown: bool) {
        let hwnd = frame as HWND;
        let visible = unsafe { IsWindowVisible(hwnd) } != 0;
        if visible != shown {
            unsafe { ShowWindow(hwnd, if shown { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        }
    }

    fn work_area_under_cursor() -> (Rect, u32) {
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&raw mut cursor) };
        let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY) };
        let mut info = MONITORINFO {
            cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(40),
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };
        let known = unsafe { GetMonitorInfoW(monitor, &raw mut info) } != 0;
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) };
        let work = info.rcWork;
        let area = if known {
            Rect::new(
                work.left,
                work.top,
                work.right - work.left,
                work.bottom - work.top,
            )
        } else {
            Rect::new(0, 0, 1280, 720)
        };
        (area, dpi_x)
    }

    fn aware() {
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    pub(super) fn compact_bounds() -> (Rect, u32) {
        aware();
        let (area, dpi) = work_area_under_cursor();
        (
            super::centred(
                area,
                super::scaled(COMPACT_WIDTH, dpi),
                super::scaled(COMPACT_HEIGHT, dpi),
            ),
            dpi,
        )
    }

    pub(super) fn launch_bounds(placement: Option<Placement>) -> (Rect, u32) {
        let (compact, dpi) = compact_bounds();
        (
            placement.map_or(compact, |placement| placement.content),
            dpi,
        )
    }

    #[derive(Default)]
    struct Kept {
        frame: Option<isize>,
        owned: Vec<isize>,
        centred: bool,
        placed: Option<Rect>,
        blanked: bool,
    }

    pub(super) fn keep(shared: &Shared, process: u32) {
        aware();
        let (compact, _) = compact_bounds();
        let mut kept = Kept::default();
        let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.running {
            let placement = state.placement;
            let hidden = state.hidden;
            drop(state);
            tick(&mut kept, process, placement, hidden, compact);
            let pause = if kept.frame.is_some() {
                KEEPING
            } else {
                LOOKING
            };
            state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.running {
                state = shared
                    .changed
                    .wait_timeout(state, pause)
                    .map_or_else(|poisoned| poisoned.into_inner().0, |(state, _)| state);
            }
        }
    }

    fn tick(
        kept: &mut Kept,
        process: u32,
        placement: Option<Placement>,
        hidden: bool,
        compact: Rect,
    ) {
        if kept
            .frame
            .is_some_and(|frame| unsafe { IsWindow(frame as HWND) } == 0)
        {
            *kept = Kept {
                centred: kept.centred,
                ..Kept::default()
            };
        }
        let frames = frames(process);
        if kept.frame.is_none() {
            kept.frame = frames
                .iter()
                .copied()
                .find(|frame| unsafe { IsWindowVisible(*frame as HWND) } != 0);
        }
        let Some(frame) = kept.frame else {
            return;
        };
        let owner = placement.map(|placement| placement.owner);
        let topmost = owner.is_some_and(is_topmost);
        let hidden =
            hidden || owner.is_some_and(|owner| unsafe { IsWindowVisible(owner as HWND) } == 0);
        for other in frames {
            if placement.is_some() && !kept.owned.contains(&other) {
                dress(other, other == frame);
                kept.owned.push(other);
            }
            set_shown(other, !hidden);
            if !hidden
                && let Some(owner) = owner
                && !above(other, owner)
            {
                raise(other, topmost);
            }
        }
        if hidden {
            return;
        }
        let Some(placement) = placement else {
            if !kept.centred {
                place(frame, compact, false);
                kept.centred = true;
            }
            return;
        };
        let (Some(window), Some(content)) = (rect_of(frame as HWND), content_of(frame)) else {
            if !kept.blanked {
                clip(frame, Rect::default());
                kept.blanked = true;
            }
            return;
        };
        let trim = super::scaled(TOP_TRIM, unsafe { GetDpiForWindow(frame as HWND) });
        let shown = placement.content;
        let page = Rect::new(shown.x, shown.y - trim, shown.width, shown.height + trim);
        let wanted = super::window_for(page, window, content);
        if wanted != window || kept.placed != Some(wanted) {
            place(frame, wanted, topmost);
            kept.placed = Some(wanted);
        }
        let region = super::clip_for(
            wanted,
            Rect::new(
                wanted.x + (content.x - window.x),
                wanted.y + (content.y - window.y) + trim,
                shown.width,
                shown.height,
            ),
        );
        if clipped(frame) != Some(region) {
            clip(frame, region);
        }
    }
}

#[cfg(not(windows))]
mod native {
    use super::{COMPACT_HEIGHT, COMPACT_WIDTH, Placement, Rect, Shared};

    pub(super) fn launch_bounds(placement: Option<Placement>) -> (Rect, u32) {
        let compact = super::centred(Rect::new(0, 0, 1280, 800), COMPACT_WIDTH, COMPACT_HEIGHT);
        (placement.map_or(compact, |placement| placement.content), 96)
    }

    pub(super) fn keep(_shared: &Shared, _process: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compact_window_is_centred_in_the_work_area_and_never_spills_out() {
        let area = Rect::new(0, 0, 1920, 1040);
        assert_eq!(
            centred(area, COMPACT_WIDTH, COMPACT_HEIGHT),
            Rect::new(720, 180, 480, 680)
        );
        let second = Rect::new(-1280, 200, 1280, 984);
        let placed = centred(second, 480, 680);
        assert_eq!(placed, Rect::new(-880, 352, 480, 680));
        let tiny = Rect::new(10, 10, 300, 400);
        assert_eq!(centred(tiny, 480, 680), tiny);
    }

    #[test]
    fn sizes_follow_the_monitor_scale() {
        assert_eq!(scaled(480, 96), 480);
        assert_eq!(scaled(480, 144), 720);
        assert_eq!(scaled(680, 120), 850);
        assert_eq!(scaled(480, 0), 480);
    }

    #[test]
    fn the_window_is_moved_so_the_page_lands_exactly_on_the_target() {
        let window = Rect::new(100, 100, 500, 700);
        let content = Rect::new(108, 140, 484, 652);
        let target = Rect::new(600, 300, 460, 640);
        let moved = window_for(target, window, content);
        assert_eq!(moved, Rect::new(592, 260, 476, 688));
        let landed = Rect::new(
            moved.x + (content.x - window.x),
            moved.y + (content.y - window.y),
            moved.width - (window.width - content.width),
            moved.height - (window.height - content.height),
        );
        assert_eq!(landed, target);
        assert_eq!(window_for(content, window, content), window);
    }

    #[test]
    fn only_the_page_is_left_visible_by_the_clip() {
        let window = Rect::new(592, 260, 476, 688);
        let content = Rect::new(600, 300, 460, 640);
        assert_eq!(clip_for(window, content), Rect::new(8, 40, 460, 640));
    }

    #[test]
    fn the_launch_flags_are_given_in_device_independent_pixels() {
        let bounds = Rect::new(1080, 270, 720, 1020);
        assert_eq!(
            window_arguments(bounds, 144),
            [
                "--window-size=480,680".to_string(),
                "--window-position=720,180".to_string()
            ]
        );
        assert_eq!(
            window_arguments(Rect::new(-100, 5, 480, 680), 96)[1],
            "--window-position=-100,5"
        );
    }

    #[test]
    fn the_handle_carries_the_owner_and_follows_every_request() {
        let window = SignInWindow::new(Some(Placement {
            owner: 42,
            content: Rect::new(1, 2, 3, 4),
        }));
        let app = window.clone();
        app.reposition(Rect::new(5, 6, 7, 8));
        assert_eq!(
            window.placement(),
            Some(Placement {
                owner: 42,
                content: Rect::new(5, 6, 7, 8)
            })
        );
        app.hide();
        assert!(window.is_hidden());
        app.show();
        assert!(!window.is_hidden());
        assert!(!window.is_cancelled());
        app.cancel();
        assert!(window.is_cancelled());
        assert!(window.is_hidden());

        let compact = SignInWindow::new(None);
        compact.reposition(Rect::new(5, 6, 7, 8));
        assert_eq!(compact.placement(), None);
    }
}

#[cfg(all(test, windows))]
mod live {
    use super::*;
    use crate::chrome::Browser;
    use std::path::PathBuf;
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, GetDC, GetPixel, ReleaseDC};
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE,
        GWLP_HWNDPARENT, GetWindowLongPtrW, HWND_TOPMOST, IsWindowVisible, MSG, PM_REMOVE,
        PeekMessageW, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, WNDCLASSW,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

    const OWNER_COLOUR: u32 = 0x0020_c020;
    const PAGE_COLOUR: u32 = 0x00cc_6633;
    const PAGE: &str = "data:text/html,<body style='margin:0;background:%233366cc'></body>";

    unsafe extern "system" fn owner_procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    struct Owner {
        hwnd: isize,
        bounds: Rect,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Owner {
        fn open(bounds: Rect) -> Self {
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (sender, receiver) = std::sync::mpsc::channel();
            let halt = std::sync::Arc::clone(&stop);
            let thread = std::thread::spawn(move || unsafe {
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                let name: Vec<u16> = "RewindTestOwner\0".encode_utf16().collect();
                let class = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(owner_procedure),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: std::ptr::null_mut(),
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: CreateSolidBrush(OWNER_COLOUR),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: name.as_ptr(),
                };
                RegisterClassW(&raw const class);
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST,
                    name.as_ptr(),
                    name.as_ptr(),
                    WS_POPUP | WS_VISIBLE,
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                sender.send(hwnd as isize).unwrap();
                let mut message: MSG = std::mem::zeroed();
                while !halt.load(std::sync::atomic::Ordering::Relaxed) {
                    while PeekMessageW(&raw mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0
                    {
                        TranslateMessage(&raw const message);
                        DispatchMessageW(&raw const message);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                DestroyWindow(hwnd);
            });
            let hwnd = receiver.recv().unwrap();
            Self {
                hwnd,
                bounds,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn pixel(x: i32, y: i32) -> u32 {
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let colour = GetPixel(screen, x, y);
            ReleaseDC(std::ptr::null_mut(), screen);
            colour
        }
    }

    fn near(colour: u32, wanted: u32) -> bool {
        (0..3).all(|shift| {
            let a = (colour >> (shift * 8)) & 0xff;
            let b = (wanted >> (shift * 8)) & 0xff;
            a.abs_diff(b) <= 12
        })
    }

    fn snapshot(area: Rect, name: &str) {
        let to = std::env::var("REWIND_SHOTS")
            .map_or_else(|_| std::env::temp_dir(), PathBuf::from)
            .join(name);
        let script = format!(
            "Add-Type -AssemblyName System.Drawing; $b = New-Object System.Drawing.Bitmap {w},{h}; $g = [System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen({x},{y},0,0,$b.Size); $b.Save('{path}')",
            w = area.width,
            h = area.height,
            x = area.x,
            y = area.y,
            path = to.display()
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status();
        println!("screenshot {}", to.display());
    }

    fn until(what: &str, timeout_seconds: u64, mut check: impl FnMut() -> bool) -> f64 {
        let started = Instant::now();
        while !check() {
            assert!(
                started.elapsed().as_secs() < timeout_seconds,
                "timed out waiting for {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let took = started.elapsed().as_secs_f64();
        println!("{what}: {took:.2}s");
        took
    }

    fn scratch_profile(name: &str) -> PathBuf {
        let path = PathBuf::from(
            std::env::var("REWIND_SCRATCH")
                .unwrap_or_else(|_| std::env::temp_dir().display().to_string()),
        )
        .join(format!("rewind-window-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn shut_down(browser: &mut Browser) {
        if let Ok(mut connection) = crate::cdp::Connection::connect(&browser.websocket_url) {
            connection.close_browser();
        }
        browser.wait_for_exit(crate::session::SHUTDOWN_TIMEOUT);
    }

    fn lands_on(content: Option<Rect>, target: Rect) -> bool {
        content.is_some_and(|content| {
            content.x == target.x
                && content.width == target.width
                && content.bottom() == target.bottom()
                && (0..=8).contains(&(target.y - content.y))
        })
    }

    fn check_glued(frame: isize, target: Rect) {
        assert_eq!(
            unsafe { GetWindowLongPtrW(frame as HWND, GWLP_HWNDPARENT) },
            0,
            "the browser window is never owned across processes"
        );
        let style = unsafe { GetWindowLongPtrW(frame as HWND, GWL_EXSTYLE) } as u32;
        assert!(style & WS_EX_TOOLWINDOW != 0, "no taskbar button");
        assert!(style & WS_EX_TOPMOST != 0, "above a topmost owner");
        assert!(
            lands_on(native::content_of(frame), target),
            "{:?}",
            native::content_of(frame)
        );
        let inside = [
            (target.x + 3, target.y + 3),
            (target.right() - 4, target.y + 3),
            (target.x + target.width / 2, target.y + target.height / 2),
            (target.x + 3, target.bottom() - 4),
        ];
        for (x, y) in inside {
            assert!(
                near(pixel(x, y), PAGE_COLOUR),
                "page at {x},{y}: {:06x}",
                pixel(x, y)
            );
        }
        let outside = [
            (target.x + target.width / 2, target.y - 6),
            (target.x + target.width / 2, target.y - 30),
            (target.x - 4, target.y + target.height / 2),
            (target.right() + 3, target.y + target.height / 2),
            (target.x + target.width / 2, target.bottom() + 3),
        ];
        for (x, y) in outside {
            assert!(
                near(pixel(x, y), OWNER_COLOUR),
                "owner at {x},{y}: {:06x}",
                pixel(x, y)
            );
        }
    }

    #[test]
    #[ignore = "opens a visible browser window over a test owner window; set REWIND_SHOTS"]
    fn a_placed_window_shows_only_the_page_glued_inside_its_owner() {
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let owner = Owner::open(Rect::new(200, 120, 1000, 820));
        let target = centred(owner.bounds, 460, 640);
        let window = SignInWindow::new(Some(Placement {
            owner: owner.hwnd,
            content: target,
        }));
        let profile = scratch_profile("placed");
        let (bounds, dpi) = window.launch_bounds();
        let started = Instant::now();
        let mut browser = Browser::launch_app(&profile, PAGE, bounds, dpi).unwrap();
        let keeper = window.keep(browser.pid());
        let mut frame = 0;
        until("window found", 20, || {
            frame = native::frames(browser.pid()).first().copied().unwrap_or(0);
            frame != 0
        });
        until("page glued to the target", 20, || {
            lands_on(native::content_of(frame), target)
                && near(pixel(target.x + 3, target.y + 3), PAGE_COLOUR)
        });
        println!("launch to glued: {:.2}s", started.elapsed().as_secs_f64());
        std::thread::sleep(std::time::Duration::from_millis(500));
        snapshot(owner.bounds, "window-placed.png");
        check_glued(frame, target);

        let moved = Rect::new(
            target.x + 60,
            target.y + 40,
            target.width,
            target.height - 40,
        );
        window.reposition(moved);
        until("page follows a reposition", 5, || {
            lands_on(native::content_of(frame), moved)
                && near(pixel(moved.x + 3, moved.y + 3), PAGE_COLOUR)
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        snapshot(owner.bounds, "window-moved.png");
        check_glued(frame, moved);

        window.hide();
        until(
            "hidden",
            5,
            || unsafe { IsWindowVisible(frame as HWND) } == 0,
        );
        window.show();
        until(
            "shown again",
            5,
            || unsafe { IsWindowVisible(frame as HWND) } != 0,
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        check_glued(frame, moved);

        unsafe {
            SetWindowPos(
                owner.hwnd as HWND,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE,
            );
            SetForegroundWindow(owner.hwnd as HWND);
        }
        until("page back above a raised owner", 5, || {
            near(pixel(moved.x + 3, moved.y + 3), PAGE_COLOUR)
        });
        check_glued(frame, moved);

        unsafe { ShowWindow(owner.hwnd as HWND, SW_HIDE) };
        until(
            "hidden with its owner",
            5,
            || unsafe { IsWindowVisible(frame as HWND) } == 0,
        );
        unsafe { ShowWindow(owner.hwnd as HWND, SW_SHOWNOACTIVATE) };
        until(
            "shown with its owner",
            5,
            || unsafe { IsWindowVisible(frame as HWND) } != 0,
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        check_glued(frame, moved);

        drop(keeper);
        shut_down(&mut browser);
        drop(owner);
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[test]
    #[ignore = "opens a compact visible browser window on a scratch profile; set REWIND_SHOTS"]
    fn an_unplaced_window_is_compact_and_centred_under_the_cursor() {
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let window = SignInWindow::new(None);
        let (expected, dpi) = window.launch_bounds();
        let profile = scratch_profile("compact");
        let mut browser = Browser::launch_app(&profile, PAGE, expected, dpi).unwrap();
        let keeper = window.keep(browser.pid());
        let mut frame = 0;
        until("window found", 20, || {
            frame = native::frames(browser.pid()).first().copied().unwrap_or(0);
            frame != 0
        });
        let mut placed = None;
        until("window centred", 10, || {
            placed = native::rect_for_tests(frame);
            placed == Some(expected)
        });
        let content = native::content_of(frame);
        println!("window {placed:?}, page {content:?}, dpi {dpi}");
        snapshot(expected, "window-compact.png");
        assert_eq!(
            unsafe { GetWindowLongPtrW(frame as HWND, GWLP_HWNDPARENT) },
            0
        );
        drop(keeper);
        shut_down(&mut browser);
        let _ = std::fs::remove_dir_all(&profile);
    }

    fn chrome_on(profile: &std::path::Path) -> usize {
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name='chrome.exe'\" | Where-Object {{ $_.CommandLine -like '*{}*' }} | Measure-Object | ForEach-Object Count",
            profile.display()
        );
        let listing = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .unwrap();
        String::from_utf8_lossy(&listing.stdout)
            .trim()
            .parse()
            .unwrap_or(0)
    }

    fn until_closed(profile: &std::path::Path) {
        until("browser gone", 20, || chrome_on(profile) == 0);
    }

    #[test]
    #[ignore = "signs in with an already signed-in profile inside a test owner window; set REWIND_YOUTUBE_DATA and REWIND_ACCOUNT"]
    fn a_signed_in_profile_is_recognised_and_the_window_goes_away_by_itself() {
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let directory = PathBuf::from(std::env::var("REWIND_YOUTUBE_DATA").unwrap());
        let account = std::env::var("REWIND_ACCOUNT").unwrap();
        let owner = Owner::open(Rect::new(200, 120, 1000, 820));
        let target = centred(owner.bounds, 460, 640);
        let window = SignInWindow::new(Some(Placement {
            owner: owner.hwnd,
            content: target,
        }));
        let started = Instant::now();
        let outcome =
            crate::session::sign_in_within(&directory, &account, 0, &window, &mut || true);
        println!(
            "sign-in returned in {:.2}s: {:?}",
            started.elapsed().as_secs_f64(),
            outcome.as_ref().map(|(found, avatar)| (
                found.id.clone(),
                found.title.clone(),
                found.handle.clone(),
                avatar.as_ref().map(Vec::len)
            ))
        );
        let profile = crate::chrome::profile_directory(&directory, &account);
        until_closed(&profile);
        drop(owner);
        assert_eq!(outcome.map(|(found, _)| found.id).ok(), Some(account));
    }

    fn scratch_sign_in(
        name: &str,
        then: impl FnOnce(&SignInWindow, &std::path::Path) + Send + 'static,
    ) -> crate::Failure {
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let directory = scratch_profile(name);
        let owner = Owner::open(Rect::new(200, 120, 1000, 820));
        let window = SignInWindow::new(Some(Placement {
            owner: owner.hwnd,
            content: centred(owner.bounds, 460, 640),
        }));
        let profile = crate::chrome::profile_directory(&directory, "scratch");
        let (app, at) = (window.clone(), profile.clone());
        let helper = std::thread::spawn(move || then(&app, &at));
        let started = Instant::now();
        let outcome =
            crate::session::sign_in_within(&directory, "scratch", 0, &window, &mut || true);
        println!(
            "sign-in returned in {:.2}s",
            started.elapsed().as_secs_f64()
        );
        helper.join().unwrap();
        until_closed(&profile);
        drop(owner);
        let _ = std::fs::remove_dir_all(&directory);
        outcome.err().unwrap()
    }

    fn wait_for_page(profile: &std::path::Path) -> u16 {
        let mut port = 0;
        until("sign-in page up", 30, || {
            port = std::fs::read_to_string(profile.join("DevToolsActivePort"))
                .ok()
                .and_then(|text| crate::chrome::parse_port_file(&text))
                .map_or(0, |(port, _)| port);
            port != 0
                && crate::devtools::targets(port).is_ok_and(|targets| {
                    targets.iter().any(|target| {
                        target.is_page() && target.url.contains("accounts.google.com")
                    })
                })
        });
        std::thread::sleep(std::time::Duration::from_secs(2));
        port
    }

    #[test]
    #[ignore = "opens Google's sign-in page in a scratch profile and cancels; set REWIND_SHOTS"]
    fn cancelling_from_the_app_closes_the_browser_and_reports_a_cancel() {
        let failure = scratch_sign_in("cancel", |window, profile| {
            wait_for_page(profile);
            let target = window.placement().unwrap().content;
            snapshot(
                Rect::new(
                    target.x - 40,
                    target.y - 40,
                    target.width + 80,
                    target.height + 80,
                ),
                "window-sign-in.png",
            );
            window.cancel();
        });
        assert_eq!(failure, crate::Failure::Cancelled);
    }

    #[test]
    #[ignore = "opens Google's sign-in page in a scratch profile and closes its window; set REWIND_SHOTS"]
    fn closing_the_window_reports_an_abandoned_sign_in() {
        let failure = scratch_sign_in("close", |_, profile| {
            let port = wait_for_page(profile);
            let url = std::fs::read_to_string(profile.join("DevToolsActivePort"))
                .ok()
                .and_then(|text| crate::chrome::parse_port_file(&text))
                .map(|(_, url)| url)
                .unwrap();
            assert!(port != 0);
            let mut connection = crate::cdp::Connection::connect(&url).unwrap();
            let page = connection.adopt_first_page().unwrap();
            let _ = page.close(&mut connection);
        });
        assert_eq!(failure, crate::Failure::SignInAbandoned);
    }
}
