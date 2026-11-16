use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_RIGHT,
    DeleteDC, DeleteObject, DrawTextW, FW_BOLD, FW_NORMAL, GdiFlush, HGDIOBJ, OUT_DEFAULT_PRECIS,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::core::HSTRING;

use crate::annotate::{Align, Anchor, TextStyle};
use crate::effects::Image;

const PADDING: i32 = 4;
const SHADOW_OFFSET: i32 = 2;
const SHADOW_OPACITY: f32 = 0.6;
const WHITE: u32 = 0x00ff_ffff;

struct Mask {
    values: Vec<f32>,
    width: i32,
    height: i32,
}

fn alignment(align: Align) -> DRAW_TEXT_FORMAT {
    match align {
        Align::Left => DT_LEFT,
        Align::Center => DT_CENTER,
        Align::Right => DT_RIGHT,
    }
}

fn coverage(text: &str, size: f32, style: &TextStyle) -> Option<Mask> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return None;
    }
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return None;
    }
    let font = unsafe {
        CreateFontW(
            -(size.round() as i32).max(1),
            0,
            0,
            0,
            if style.bold { FW_BOLD.0 } else { FW_NORMAL.0 } as i32,
            u32::from(style.italic),
            u32::from(style.underline),
            u32::from(style.strike),
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            DEFAULT_PITCH.0 as u32,
            &HSTRING::from(style.font.as_str()),
        )
    };
    let previous_font = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
    let format = alignment(style.align) | DT_NOPREFIX;
    let mut bounds = RECT::default();
    unsafe { DrawTextW(dc, &mut wide, &mut bounds, format | DT_CALCRECT) };
    let width = bounds.right - bounds.left + PADDING * 2;
    let height = bounds.bottom - bounds.top + PADDING * 2;
    let info = BITMAPINFO {
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
    let mut bits = std::ptr::null_mut();
    let bitmap =
        unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok();
    let result = bitmap.map(|bitmap| {
        let previous_bitmap = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };
        unsafe {
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(WHITE));
        }
        let mut target = RECT {
            left: PADDING,
            top: PADDING,
            right: width - PADDING,
            bottom: height - PADDING,
        };
        unsafe {
            DrawTextW(dc, &mut wide, &mut target, format);
            let _ = GdiFlush();
        }
        let pixels = (width * height) as usize;
        let data = unsafe { std::slice::from_raw_parts(bits as *const u8, pixels * 4) };
        let values = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| f32::from(pixel[1]) / 255.0)
            .collect();
        unsafe {
            SelectObject(dc, previous_bitmap);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
        }
        Mask {
            values,
            width,
            height,
        }
    });
    unsafe {
        SelectObject(dc, previous_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteDC(dc);
    }
    result
}

fn blend(image: &mut Image, mask: &Mask, origin: (i32, i32), color: [u8; 3], opacity: f32) {
    let stride = image.width as usize * 4;
    let [red, green, blue] = color;
    for y in 0..mask.height {
        let ty = origin.1 + y;
        if ty < 0 || ty >= image.height as i32 {
            continue;
        }
        for x in 0..mask.width {
            let tx = origin.0 + x;
            if tx < 0 || tx >= image.width as i32 {
                continue;
            }
            let amount = mask.values[(y * mask.width + x) as usize] * opacity;
            if amount <= 0.0 {
                continue;
            }
            let at = ty as usize * stride + tx as usize * 4;
            let pixel = &mut image.pixels[at..at + 3];
            for (channel, target) in pixel.iter_mut().zip([blue, green, red]) {
                *channel =
                    (f32::from(*channel) * (1.0 - amount) + f32::from(target) * amount) as u8;
            }
        }
    }
}

pub fn draw_text(
    image: &mut Image,
    text: &str,
    at: (f32, f32),
    size: f32,
    style: &TextStyle,
    anchor: Anchor,
) {
    let Some(mask) = coverage(text, size, style) else {
        return;
    };
    let origin = match anchor {
        Anchor::TopLeft => (at.0.round() as i32 - PADDING, at.1.round() as i32 - PADDING),
        Anchor::Center => (
            (at.0 - mask.width as f32 / 2.0).round() as i32,
            (at.1 - mask.height as f32 / 2.0).round() as i32,
        ),
    };
    if style.shadow {
        blend(
            image,
            &mask,
            (origin.0 + SHADOW_OFFSET, origin.1 + SHADOW_OFFSET),
            [0, 0, 0],
            SHADOW_OPACITY,
        );
    }
    blend(image, &mask, origin, style.color, 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> TextStyle {
        TextStyle {
            color: [255, 0, 0],
            ..TextStyle::default()
        }
    }

    fn red_pixels(pixels: &[u8]) -> usize {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[2] > 200 && pixel[0] < 40)
            .count()
    }

    #[test]
    fn text_is_painted_in_its_color_near_the_anchor() {
        let (width, height) = (200u32, 80u32);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let mut image = Image {
            pixels: &mut pixels,
            width,
            height,
        };
        draw_text(
            &mut image,
            "Rewind",
            (10.0, 10.0),
            32.0,
            &style(),
            Anchor::TopLeft,
        );
        assert!(red_pixels(&pixels) > 100);
        assert!(pixels[..4 * 5].iter().all(|value| *value == 0));
    }

    #[test]
    fn centered_text_lands_around_the_point() {
        let (width, height) = (100u32, 100u32);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let mut image = Image {
            pixels: &mut pixels,
            width,
            height,
        };
        draw_text(
            &mut image,
            "8",
            (50.0, 50.0),
            30.0,
            &style(),
            Anchor::Center,
        );
        let left: usize = (0..height)
            .flat_map(|y| (0..30).map(move |x| (y * width + x) as usize * 4))
            .filter(|at| pixels[at + 2] > 200)
            .count();
        assert_eq!(left, 0);
        assert!(red_pixels(&pixels) > 20);
    }
}
