use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, IDXGIOutput1,
};
use windows::core::Interface;

use crate::error::{CaptureError, Result};

const GPU_THREAD_PRIORITY: i32 = 7;

pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub luid: LUID,
    pub output: IDXGIOutput1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl Screen {
    pub const fn is_primary(&self) -> bool {
        self.left == 0 && self.top == 0
    }

    const fn origin(&self) -> (i32, i32) {
        (self.left, self.top)
    }
}

type Output = (IDXGIAdapter1, IDXGIOutput1, LUID, Screen);

fn outputs() -> Result<Vec<Output>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
    let mut found = Vec::new();
    for adapter_index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(adapter_index) }) else {
            break;
        };
        let luid = unsafe { adapter.GetDesc1() }?.AdapterLuid;
        for output_index in 0.. {
            let Ok(output) = (unsafe { adapter.EnumOutputs(output_index) }) else {
                break;
            };
            let desc = unsafe { output.GetDesc() }?;
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let area = desc.DesktopCoordinates;
            let screen = Screen {
                left: area.left,
                top: area.top,
                width: (area.right - area.left).unsigned_abs(),
                height: (area.bottom - area.top).unsigned_abs(),
            };
            found.push((adapter.clone(), output.cast()?, luid, screen));
        }
    }
    Ok(found)
}

fn arrange(mut screens: Vec<Screen>) -> Vec<Screen> {
    screens.sort_by_key(|screen| (!screen.is_primary(), screen.left, screen.top));
    screens
}

fn pick(wanted: Option<Screen>, screens: &[Screen]) -> Option<usize> {
    wanted
        .and_then(|wanted| {
            screens
                .iter()
                .position(|screen| screen.origin() == wanted.origin())
        })
        .or_else(|| screens.iter().position(Screen::is_primary))
        .or(if screens.is_empty() { None } else { Some(0) })
}

pub fn screens() -> Vec<Screen> {
    arrange(
        outputs()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, _, _, screen)| screen)
            .collect(),
    )
}

impl Gpu {
    pub fn open(screen: Option<Screen>) -> Result<Self> {
        let mut outputs = outputs()?;
        let places: Vec<Screen> = outputs.iter().map(|(_, _, _, place)| *place).collect();
        let index = pick(screen, &places).ok_or(CaptureError::NoOutput)?;
        let (adapter, output, luid, _) = outputs.swap_remove(index);
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err(CaptureError::NoOutput);
        };
        let multithread: ID3D11Multithread = device.cast()?;
        let _ = unsafe { multithread.SetMultithreadProtected(true) };
        let dxgi: IDXGIDevice = device.cast()?;
        let _ = unsafe { dxgi.SetGPUThreadPriority(GPU_THREAD_PRIORITY) };
        Ok(Self {
            device,
            context,
            luid,
            output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn screen(left: i32, top: i32) -> Screen {
        Screen {
            left,
            top,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn the_primary_screen_comes_first_and_the_rest_go_left_to_right() {
        let arranged = arrange(vec![screen(2560, 0), screen(-1920, 360), screen(0, 0)]);
        assert_eq!(
            arranged,
            vec![screen(0, 0), screen(-1920, 360), screen(2560, 0)]
        );
    }

    #[test]
    fn a_missing_screen_falls_back_to_the_primary_one() {
        let screens = [screen(-1920, 360), screen(0, 0)];
        assert_eq!(pick(Some(screen(-1920, 360)), &screens), Some(0));
        assert_eq!(pick(Some(screen(5000, 0)), &screens), Some(1));
        assert_eq!(pick(None, &screens), Some(1));
        assert_eq!(pick(None, &[]), None);
    }
}
