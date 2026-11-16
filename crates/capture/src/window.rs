use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::Interface;

use crate::duplication::Acquired;
use crate::error::Result;

const BUFFERS: i32 = 2;

pub struct WindowCapture {
    window: HWND,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    held: Option<Direct3D11CaptureFrame>,
    pub width: u32,
    pub height: u32,
}

impl WindowCapture {
    pub fn new(device: &ID3D11Device, window: HWND) -> Result<Self> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }?;
        let dxgi: IDXGIDevice = device.cast()?;
        let direct: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?.cast()?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            BUFFERS,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(true);
        session.StartCapture()?;
        Ok(Self {
            window,
            pool,
            session,
            held: None,
            width: size.Width.max(2) as u32,
            height: size.Height.max(2) as u32,
        })
    }

    pub fn is_open(&self) -> bool {
        unsafe { IsWindow(Some(self.window)) }.as_bool()
    }

    pub fn resized(&self, frame: &Direct3D11CaptureFrame) -> bool {
        frame.ContentSize().is_ok_and(|size| {
            size.Width.max(2) as u32 != self.width || size.Height.max(2) as u32 != self.height
        })
    }

    pub fn acquire(&mut self) -> Result<Option<Acquired>> {
        self.release();
        let Ok(frame) = self.pool.TryGetNextFrame() else {
            return Ok(Some(Acquired::Timeout));
        };
        if self.resized(&frame) {
            return Ok(None);
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
        self.held = Some(frame);
        Ok(Some(Acquired::Frame {
            texture,
            presents: 1,
        }))
    }

    pub fn release(&mut self) {
        if let Some(frame) = self.held.take() {
            let _ = frame.Close();
        }
    }
}

impl Drop for WindowCapture {
    fn drop(&mut self) {
        self.release();
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::Runtime;
    use crate::convert::{Converter, fit};
    use crate::device::Gpu;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::w;

    #[test]
    #[ignore = "needs a real desktop session"]
    fn a_window_frame_reaches_the_converter() {
        let _runtime = Runtime::enter().unwrap();
        let gpu = Gpu::open(None).unwrap();
        let window = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.unwrap();
        let mut capture = WindowCapture::new(&gpu.device, window).unwrap();
        assert!(capture.is_open());
        let size = (capture.width, capture.height);
        let mut converter =
            Converter::new(&gpu.device, &gpu.context, size, fit(size, None), 60).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "no frame from the window"
            );
            if let Some(Acquired::Frame { texture, .. }) = capture.acquire().unwrap() {
                converter.convert(&texture).unwrap();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
