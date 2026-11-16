use std::path::Path;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};

use rewind_media::{Bgra, Runtime, write_png};

use crate::error::{CaptureError, Result};

pub struct Image {
    width: u32,
    height: u32,
    stride: u32,
    pixels: Vec<u8>,
}

impl Image {
    pub fn capture(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
    ) -> Result<Self> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { source.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        desc.MipLevels = 1;
        desc.ArraySize = 1;
        let mut staging = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }?;
        let staging = staging.ok_or(CaptureError::NoOutput)?;
        unsafe { context.CopyResource(&staging, source) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }?;
        let length = (mapped.RowPitch * desc.Height) as usize;
        let pixels =
            unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, length) }.to_vec();
        unsafe { context.Unmap(&staging, 0) };
        Ok(Self {
            width: desc.Width,
            height: desc.Height,
            stride: mapped.RowPitch,
            pixels,
        })
    }

    pub fn save(&self, targets: &[(&Path, Option<u32>)]) -> Result<()> {
        let _runtime = Runtime::enter().map_err(|_| CaptureError::NoOutput)?;
        let image = Bgra {
            width: self.width,
            height: self.height,
            stride: self.stride,
            pixels: &self.pixels,
        };
        for (path, max_width) in targets {
            write_png(path, &image, *max_width).map_err(|_| CaptureError::NoOutput)?;
        }
        Ok(())
    }
}
