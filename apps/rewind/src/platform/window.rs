use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, LWA_ALPHA, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

const OPAQUE: u8 = 255;
const VISIBLE_IN_CAPTURE: &str = "REWIND_VISIBLE_IN_CAPTURE";

pub fn show_in_capture(window: &Window, shown: bool) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let affinity = if shown || std::env::var_os(VISIBLE_IN_CAPTURE).is_some() {
        WDA_NONE
    } else {
        WDA_EXCLUDEFROMCAPTURE
    };
    let _ = unsafe { SetWindowDisplayAffinity(hwnd, affinity) };
}

fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut core::ffi::c_void)),
        _ => None,
    }
}

pub type Extent = windows::Win32::Foundation::RECT;

const FOLDED: i32 = 1;

fn place(window: &Window, extent: Extent, cx: &gpui::App) {
    use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let handle = hwnd.0 as isize;
    cx.foreground_executor()
        .spawn(async move {
            let _ = unsafe {
                SetWindowPos(
                    HWND(handle as *mut core::ffi::c_void),
                    None,
                    extent.left,
                    extent.top,
                    extent.right - extent.left,
                    extent.bottom - extent.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
        })
        .detach();
}

const PARKED: i32 = -32000;

const fn folded() -> Extent {
    Extent {
        left: PARKED,
        top: PARKED,
        right: PARKED + FOLDED,
        bottom: PARKED + FOLDED,
    }
}

pub fn pin(window: &Window, extent: Extent, accent: u32, cx: &gpui::App) {
    use windows::Win32::Graphics::Dwm::{
        DWM_WINDOW_CORNER_PREFERENCE, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_DONOTROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW;
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    add_ex_style(hwnd, (WS_EX_TOPMOST | WS_EX_TOOLWINDOW).0);
    let corner = DWMWCP_DONOTROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    };
    let border = ((accent & 0xff) << 16) | (accent & 0xff00) | ((accent >> 16) & 0xff);
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&border as *const u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    let handle = hwnd.0 as isize;
    cx.foreground_executor()
        .spawn(async move {
            let hwnd = HWND(handle as *mut core::ffi::c_void);
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    extent.left,
                    extent.top,
                    extent.right - extent.left,
                    extent.bottom - extent.top,
                    SWP_SHOWWINDOW,
                )
            };
            let _ = unsafe { SetForegroundWindow(hwnd) };
        })
        .detach();
}

pub fn place_pinned(window: &Window, extent: Extent, cx: &gpui::App) {
    place(window, extent, cx);
}

pub fn extent(window: &Window) -> Option<Extent> {
    let mut rect = Extent::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd(window)?, &mut rect) }
        .ok()?;
    Some(rect)
}

pub fn cursor() -> Option<(i32, i32)> {
    let mut point = windows::Win32::Foundation::POINT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}

pub fn fold(window: &Window, cx: &gpui::App) -> Option<Extent> {
    let mut full = Extent::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd(window)?, &mut full) }
        .ok()?;
    place(window, folded(), cx);
    Some(full)
}

pub fn unfold(window: &Window, full: Extent, cx: &gpui::App) {
    place(window, full, cx);
}

fn topmost(hwnd: HWND) {
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
}

fn add_ex_style(hwnd: HWND, style: u32) {
    let current = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, current | style as isize) };
}

pub fn prepare_overlay(window: &Window, captured: bool) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    show_in_capture(window, captured);
    add_ex_style(hwnd, (WS_EX_TOPMOST | WS_EX_TOOLWINDOW).0);
}

pub fn prepare_hud(window: &Window, captured: bool) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    show_in_capture(window, captured);
    add_ex_style(
        hwnd,
        (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW).0,
    );
    let _ = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), OPAQUE, LWA_ALPHA) };
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    topmost(hwnd);
}

pub fn show_overlay(window: &mut Window) {
    if let Some(hwnd) = hwnd(window) {
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
        topmost(hwnd);
        let _ = unsafe { SetForegroundWindow(hwnd) };
    }
    window.activate_window();
}

pub fn inner_placement(
    window: &Window,
    width: f32,
    height: f32,
    header: f32,
) -> Option<youtube::window::Placement> {
    use youtube::window::{Placement, Rect, centred};
    let hwnd = hwnd(window)?;
    let mut inner = windows::Win32::Foundation::RECT::default();
    let mut origin = windows::Win32::Foundation::POINT::default();
    let measured = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut inner).is_ok()
            && windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut origin).as_bool()
    };
    if !measured {
        return None;
    }
    let scale = window.scale_factor();
    let physical = |length: f32| (length * scale).round() as i32;
    let area = Rect::new(origin.x, origin.y, inner.right, inner.bottom);
    let card = centred(area, physical(width), physical(height + header));
    let top = physical(header).min(card.height);
    Some(Placement {
        owner: hwnd.0 as isize,
        content: Rect::new(card.x, card.y + top, card.width, card.height - top),
    })
}

pub fn hide(window: &Window) {
    if let Some(hwnd) = hwnd(window) {
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

pub fn keep_on_top(window: &Window) {
    if let Some(hwnd) = hwnd(window) {
        topmost(hwnd);
    }
}

pub fn cover(window: &Window, monitor: windows::Win32::Foundation::RECT) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOPMOST, SWP_SHOWWINDOW, SetWindowPos};
    if let Some(hwnd) = hwnd(window) {
        use windows::Win32::Graphics::Dwm::{
            DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute,
        };
        let disabled = windows::core::BOOL::from(true);
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED,
                (&disabled as *const windows::core::BOOL).cast(),
                size_of::<windows::core::BOOL>() as u32,
            )
        };
        add_ex_style(hwnd, (WS_EX_TOPMOST | WS_EX_TOOLWINDOW).0);
        let place = |left: i32, top: i32, width: i32, height: i32| {
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    left,
                    top,
                    width,
                    height,
                    SWP_SHOWWINDOW,
                )
            };
        };
        let (width, height) = (monitor.right - monitor.left, monitor.bottom - monitor.top);
        place(monitor.left, monitor.top, width, height);
        let mut outer = windows::Win32::Foundation::RECT::default();
        let mut inner = windows::Win32::Foundation::RECT::default();
        let mut origin = windows::Win32::Foundation::POINT::default();
        let measured = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut outer).is_ok()
                && windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut inner).is_ok()
                && windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut origin).as_bool()
        };
        if measured {
            let left = origin.x - outer.left;
            let top = origin.y - outer.top;
            let right = outer.right - (origin.x + inner.right);
            let bottom = outer.bottom - (origin.y + inner.bottom);
            place(
                monitor.left - left,
                monitor.top - top,
                width + left + right,
                height + top + bottom,
            );
        }
        let _ = unsafe { SetForegroundWindow(hwnd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folded_window_is_a_dot_far_outside_every_screen() {
        const SMALLEST_FRAME: i32 = 400;
        let small = folded();
        assert_eq!((small.right - small.left, small.bottom - small.top), (1, 1));
        assert!(small.left + SMALLEST_FRAME < -16000);
        assert!(small.top + SMALLEST_FRAME < -16000);
    }
}
