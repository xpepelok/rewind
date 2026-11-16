use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};
use windows::core::BOOL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Area {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x + self.width as i32
            && y < self.y + self.height as i32
    }

    pub fn clamp_to(&self, width: u32, height: u32) -> Option<Self> {
        let left = self.x.max(0);
        let top = self.y.max(0);
        let right = (self.x + self.width as i32).min(width as i32);
        let bottom = (self.y + self.height as i32).min(height as i32);
        (right > left && bottom > top).then(|| Self {
            x: left,
            y: top,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }
}

pub struct Frozen {
    pub monitor: RECT,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub windows: Vec<Area>,
}

impl Frozen {
    pub fn crop(&self, area: Area) -> Option<(u32, u32, Vec<u8>)> {
        let area = area.clamp_to(self.width, self.height)?;
        let stride = self.width as usize * 4;
        let row = area.width as usize * 4;
        let mut pixels = Vec::with_capacity(row * area.height as usize);
        for line in area.y as usize..area.y as usize + area.height as usize {
            let start = line * stride + area.x as usize * 4;
            pixels.extend_from_slice(&self.bgra[start..start + row]);
        }
        Some((area.width, area.height, pixels))
    }
}

struct Search {
    monitor: RECT,
    own: u32,
    found: Vec<Area>,
}

unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
    let search = unsafe { &mut *(data.0 as *mut Search) };
    if !unsafe { IsWindowVisible(window) }.as_bool() || unsafe { IsIconic(window) }.as_bool() {
        return true.into();
    }
    let mut process = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };
    if process == search.own {
        return true.into();
    }
    let mut cloaked = 0u32;
    let cloak = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    if cloak.is_ok() && cloaked != 0 {
        return true.into();
    }
    let mut rect = RECT::default();
    let framed = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
    };
    if framed.is_err() {
        return true.into();
    }
    let monitor = search.monitor;
    let area = Area {
        x: rect.left - monitor.left,
        y: rect.top - monitor.top,
        width: (rect.right - rect.left).max(0) as u32,
        height: (rect.bottom - rect.top).max(0) as u32,
    };
    let size = (
        (monitor.right - monitor.left) as u32,
        (monitor.bottom - monitor.top) as u32,
    );
    if let Some(visible) = area.clamp_to(size.0, size.1)
        && visible.width > 1
        && visible.height > 1
    {
        search.found.push(visible);
    }
    true.into()
}

fn cursor_monitor() -> Option<RECT> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.ok()?;
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then_some(info.rcMonitor)
}

fn grab(monitor: RECT) -> Option<(u32, u32, Vec<u8>)> {
    let width = (monitor.right - monitor.left).max(1);
    let height = (monitor.bottom - monitor.top).max(1);
    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        return None;
    }
    let memory = unsafe { CreateCompatibleDC(Some(screen)) };
    let bitmap = unsafe { CreateCompatibleBitmap(screen, width, height) };
    let previous = unsafe { SelectObject(memory, bitmap.into()) };
    let copied = unsafe {
        BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            monitor.left,
            monitor.top,
            SRCCOPY | CAPTUREBLT,
        )
    }
    .is_ok();
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let lines = unsafe {
        GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    (copied && lines == height).then_some((width as u32, height as u32, pixels))
}

fn freeze_monitor(monitor: RECT, primary: bool) -> Option<Frozen> {
    let mut search = Search {
        monitor,
        own: std::process::id(),
        found: Vec::new(),
    };
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut search as *mut Search as isize)) };
    let (width, height, bgra) = grab(monitor)?;
    Some(Frozen {
        monitor,
        primary,
        width,
        height,
        bgra,
        windows: search.found,
    })
}

unsafe extern "system" fn add_monitor(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    _: windows::Win32::Graphics::Gdi::HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let found = unsafe { &mut *(data.0 as *mut Vec<(RECT, bool)>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        found.push((info.rcMonitor, info.dwFlags & MONITOR_PRIMARY != 0));
    }
    true.into()
}

const MONITOR_PRIMARY: u32 = 1;

pub fn freeze_all() -> Vec<Frozen> {
    let mut monitors: Vec<(RECT, bool)> = Vec::new();
    let _ = unsafe {
        windows::Win32::Graphics::Gdi::EnumDisplayMonitors(
            None,
            None,
            Some(add_monitor),
            LPARAM(&mut monitors as *mut Vec<(RECT, bool)> as isize),
        )
    };
    if monitors.is_empty()
        && let Some(monitor) = cursor_monitor()
    {
        monitors.push((monitor, true));
    }
    let mut frozen: Vec<Frozen> = monitors
        .into_iter()
        .filter_map(|(monitor, primary)| freeze_monitor(monitor, primary))
        .collect();
    frozen.sort_by_key(|monitor| monitor.primary);
    frozen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_clip_to_the_screen() {
        let area = Area {
            x: -10,
            y: 5,
            width: 50,
            height: 2000,
        };
        assert_eq!(
            area.clamp_to(100, 100),
            Some(Area {
                x: 0,
                y: 5,
                width: 40,
                height: 95
            })
        );
        assert!(
            Area {
                x: 200,
                y: 0,
                width: 5,
                height: 5
            }
            .clamp_to(100, 100)
            .is_none()
        );
        assert!(area.contains(0, 10));
        assert!(!area.contains(41, 10));
    }

    #[test]
    fn cropping_takes_the_right_rows() {
        let frozen = Frozen {
            monitor: RECT::default(),
            primary: true,
            width: 2,
            height: 2,
            bgra: (0..16).collect(),
            windows: Vec::new(),
        };
        let (width, height, pixels) = frozen
            .crop(Area {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            })
            .unwrap();
        assert_eq!((width, height), (1, 1));
        assert_eq!(pixels, vec![12, 13, 14, 15]);
    }

    #[test]
    #[ignore = "needs a real desktop session"]
    fn the_cursor_monitor_freezes() {
        let all = freeze_all();
        assert!(!all.is_empty());
        assert!(all.last().is_some_and(|monitor| monitor.primary));
        let frozen = all.into_iter().next().unwrap();
        assert_eq!(
            frozen.bgra.len(),
            frozen.width as usize * frozen.height as usize * 4
        );
        assert!(
            frozen
                .bgra
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3] != [0, 0, 0])
        );
    }
}
