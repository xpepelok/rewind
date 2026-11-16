use std::path::Path;

use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGR,
    IWICBitmapSource, IWICImagingFactory, WICBitmapEncoderNoCache, WICBitmapInterpolationModeFant,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{HSTRING, Interface};

use crate::error::{MediaError, Result};

pub struct Bgra<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: &'a [u8],
}

pub fn write_png(path: &Path, image: &Bgra, max_width: Option<u32>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }?;
    let bitmap = unsafe {
        factory.CreateBitmapFromMemory(
            image.width,
            image.height,
            &GUID_WICPixelFormat32bppBGR,
            image.stride,
            image.pixels,
        )
    }?;
    let bitmap: IWICBitmapSource = bitmap.cast()?;
    let (width, height, source) = match max_width.filter(|max| *max < image.width) {
        Some(max) => {
            let height =
                (u64::from(image.height) * u64::from(max) / u64::from(image.width)).max(1) as u32;
            let scaler = unsafe { factory.CreateBitmapScaler() }?;
            unsafe { scaler.Initialize(&bitmap, max, height, WICBitmapInterpolationModeFant) }?;
            (max, height, scaler.cast::<IWICBitmapSource>()?)
        }
        None => (image.width, image.height, bitmap),
    };
    let stream = unsafe { factory.CreateStream() }?;
    unsafe { stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0) }?;
    let encoder = unsafe { factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()) }?;
    unsafe { encoder.Initialize(&stream, WICBitmapEncoderNoCache) }?;
    let mut frame = None;
    unsafe { encoder.CreateNewFrame(&mut frame, std::ptr::null_mut()) }?;
    let frame = frame.ok_or(MediaError::NoFrame)?;
    let mut format = GUID_WICPixelFormat32bppBGR;
    unsafe {
        frame.Initialize(None)?;
        frame.SetSize(width, height)?;
        frame.SetPixelFormat(&mut format)?;
        frame.WriteSource(&source, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}
