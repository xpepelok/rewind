use std::os::windows::ffi::OsStrExt;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::core::w;

const CF_DIB: u32 = 8;
const OPEN_ATTEMPTS: u32 = 10;
const OPEN_BACKOFF: Duration = Duration::from_millis(25);

fn global(bytes: &[u8]) -> Option<HGLOBAL> {
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.ok()?;
    let target = unsafe { GlobalLock(memory) };
    if target.is_null() {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return None;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(memory);
    }
    Some(memory)
}

pub fn dib(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: width * height * 4,
        ..Default::default()
    };
    let mut out = Vec::with_capacity(size_of::<BITMAPINFOHEADER>() + rgba.len());
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(
            (&header as *const BITMAPINFOHEADER).cast::<u8>(),
            size_of::<BITMAPINFOHEADER>(),
        )
    });
    let row = width as usize * 4;
    for line in (0..height as usize).rev() {
        for pixel in rgba[line * row..(line + 1) * row].as_chunks::<4>().0 {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    out
}

fn put(format: u32, bytes: &[u8]) -> bool {
    let Some(memory) = global(bytes) else {
        return false;
    };
    if unsafe { SetClipboardData(format, Some(HANDLE(memory.0))) }.is_ok() {
        true
    } else {
        let _ = unsafe { GlobalFree(Some(memory)) };
        false
    }
}

const CF_HDROP: u32 = 15;
const DROPFILES_SIZE: u32 = 20;
const DROPEFFECT_COPY: u32 = 1;

fn drop_files(path: &std::path::Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&DROPFILES_SIZE.to_le_bytes());
    bytes.extend_from_slice(&[0; 12]);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    for unit in path.as_os_str().encode_wide().chain([0, 0]) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn open() -> bool {
    for _ in 0..OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return true;
        }
        std::thread::sleep(OPEN_BACKOFF);
    }
    false
}

pub fn copy_file(path: &std::path::Path) -> Result<(), String> {
    if !open() {
        return Err("буфер обмена занят другой программой".to_owned());
    }
    let effect = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    let written = unsafe { EmptyClipboard() }.is_ok()
        && put(CF_HDROP, &drop_files(path))
        && (effect == 0 || put(effect, &DROPEFFECT_COPY.to_le_bytes()));
    let _ = unsafe { CloseClipboard() };
    if written {
        Ok(())
    } else {
        Err("не удалось положить файл в буфер обмена".to_owned())
    }
}

pub fn copy_image(png: &[u8]) -> Result<(), String> {
    let decoded = image::load_from_memory(png)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let bitmap = dib(decoded.as_raw(), width, height);
    let mut opened = false;
    for _ in 0..OPEN_ATTEMPTS {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(OPEN_BACKOFF);
    }
    if !opened {
        return Err("буфер обмена занят другой программой".to_owned());
    }
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    let written = unsafe { EmptyClipboard() }.is_ok()
        && put(CF_DIB, &bitmap)
        && (png_format == 0 || put(png_format, png));
    let _ = unsafe { CloseClipboard() };
    if written {
        Ok(())
    } else {
        Err("не удалось записать картинку в буфер обмена".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_is_bottom_up_bgra_after_the_header() {
        let rgba = [1, 2, 3, 4, 5, 6, 7, 8];
        let out = dib(&rgba, 1, 2);
        let header = size_of::<BITMAPINFOHEADER>();
        assert_eq!(out.len(), header + 8);
        assert_eq!(&out[header..], &[7, 6, 5, 8, 3, 2, 1, 4]);
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn dropped_files_are_wide_and_double_terminated() {
        let bytes = drop_files(std::path::Path::new("C:/a.mp4"));
        assert_eq!(&bytes[..4], &DROPFILES_SIZE.to_le_bytes());
        assert_eq!(&bytes[16..20], &1u32.to_le_bytes());
        let text: Vec<u16> = bytes[20..]
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(String::from_utf16_lossy(&text[..8]), "C:/a.mp4");
        assert_eq!(&text[8..], &[0, 0]);
    }
}
