use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIOutput1, IDXGIOutputDuplication,
};
use windows::core::Interface;

use crate::error::Result;

pub enum Acquired {
    Frame {
        texture: ID3D11Texture2D,
        presents: u32,
    },
    Timeout,
}

pub struct Duplicator {
    duplication: IDXGIOutputDuplication,
    held: bool,
    pub width: u32,
    pub height: u32,
}

impl Duplicator {
    pub fn new(device: &ID3D11Device, output: &IDXGIOutput1) -> Result<Self> {
        let duplication = unsafe { output.DuplicateOutput(device) }?;
        let desc = unsafe { duplication.GetDesc() };
        Ok(Self {
            duplication,
            held: false,
            width: desc.ModeDesc.Width,
            height: desc.ModeDesc.Height,
        })
    }

    pub fn acquire(&mut self, timeout_ms: u32) -> Result<Acquired> {
        self.release();
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        match unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut info, &mut resource)
        } {
            Ok(()) => {}
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Acquired::Timeout),
            Err(error) => return Err(error.into()),
        }
        self.held = true;
        let Some(resource) = resource else {
            return Ok(Acquired::Timeout);
        };
        if info.LastPresentTime == 0 && info.AccumulatedFrames == 0 {
            return Ok(Acquired::Timeout);
        }
        Ok(Acquired::Frame {
            texture: resource.cast()?,
            presents: info.AccumulatedFrames,
        })
    }

    pub fn release(&mut self) {
        if self.held {
            let _ = unsafe { self.duplication.ReleaseFrame() };
            self.held = false;
        }
    }
}

impl Drop for Duplicator {
    fn drop(&mut self) {
        self.release();
    }
}
