use std::mem::ManuallyDrop;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
    DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use crate::error::{CaptureError, Result};

const POOL_SIZE: usize = 8;
const INPUT_CACHE: usize = 8;

pub fn fit(source: (u32, u32), height: Option<u32>) -> (u32, u32) {
    let even = |value: u32| (value.max(2) + 1) & !1;
    let Some(height) = height.filter(|height| *height < source.1) else {
        return (even(source.0), even(source.1));
    };
    let width = (u64::from(source.0) * u64::from(height) / u64::from(source.1.max(1))) as u32;
    (even(width), even(height))
}

pub struct Converter {
    video: ID3D11VideoContext1,
    video_device: ID3D11VideoDevice,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    pool: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    inputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorInputView)>,
    next: usize,
}

impl Converter {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        source: (u32, u32),
        target: (u32, u32),
        fps: u32,
    ) -> Result<Self> {
        let video_device: ID3D11VideoDevice = device.cast()?;
        let video: ID3D11VideoContext1 = context.cast()?;
        let rate = DXGI_RATIONAL {
            Numerator: fps,
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: source.0,
            InputHeight: source.1,
            OutputFrameRate: rate,
            OutputWidth: target.0,
            OutputHeight: target.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content) }?;
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }?;
        unsafe {
            video.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            video.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            video.VideoProcessorSetStreamColorSpace1(
                &processor,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            video.VideoProcessorSetOutputColorSpace1(
                &processor,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            );
        }
        let texture_desc = D3D11_TEXTURE2D_DESC {
            Width: target.0,
            Height: target.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut pool = Vec::with_capacity(POOL_SIZE);
        for _ in 0..POOL_SIZE {
            let mut texture = None;
            unsafe { device.CreateTexture2D(&texture_desc, None, Some(&mut texture)) }?;
            let texture = texture.ok_or(CaptureError::NoOutput)?;
            let mut view = None;
            unsafe {
                video_device.CreateVideoProcessorOutputView(
                    &texture,
                    &enumerator,
                    &view_desc,
                    Some(&mut view),
                )
            }?;
            pool.push((texture, view.ok_or(CaptureError::NoOutput)?));
        }
        Ok(Self {
            video,
            video_device,
            enumerator,
            processor,
            pool,
            inputs: Vec::with_capacity(INPUT_CACHE),
            next: 0,
        })
    }

    fn input_view(&mut self, source: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorInputView> {
        if let Some((_, view)) = self
            .inputs
            .iter()
            .find(|(texture, _)| texture.as_raw() == source.as_raw())
        {
            return Ok(view.clone());
        }
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut view = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                source,
                &self.enumerator,
                &desc,
                Some(&mut view),
            )
        }?;
        let view = view.ok_or(CaptureError::NoOutput)?;
        if self.inputs.len() >= INPUT_CACHE {
            self.inputs.clear();
        }
        self.inputs.push((source.clone(), view.clone()));
        Ok(view)
    }

    pub fn convert(&mut self, source: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        let input = self.input_view(source)?;
        let (texture, output) = &self.pool[self.next];
        self.next = (self.next + 1) % self.pool.len();
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input)),
            ..Default::default()
        };
        let result = unsafe {
            self.video
                .VideoProcessorBlt(&self.processor, output, 0, std::slice::from_ref(&stream))
        };
        unsafe { ManuallyDrop::drop(&mut stream.pInputSurface) };
        result?;
        Ok(texture.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn native_keeps_size_even() {
        assert_eq!(fit((1921, 1081), None), (1922, 1082));
    }

    #[test]
    fn downscale_keeps_aspect() {
        assert_eq!(fit((2560, 1440), Some(1080)), (1920, 1080));
        assert_eq!(fit((3440, 1440), Some(1080)), (2580, 1080));
    }

    #[test]
    fn never_upscales() {
        assert_eq!(fit((1920, 1080), Some(2160)), (1920, 1080));
    }
}
