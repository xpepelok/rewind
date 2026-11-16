use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::System::SystemInformation::GetPhysicallyInstalledSystemMemory;
use windows::core::PCWSTR;

use rewind_config::Specs as Choice;

use crate::platform::system::get_string;

const CPU_KEY: &str = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0";
const CPU_VALUE: &str = "ProcessorNameString";
const OS_KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
const WINDOWS_11_BUILD: u32 = 22_000;
const KIB_PER_GIB: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Specs {
    pub gpu: Option<String>,
    pub cpu: Option<String>,
    pub memory_gb: Option<u64>,
    pub display: Option<(u32, u32, u32)>,
    pub system: Option<String>,
}

fn gpu() -> Option<String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    (0..)
        .map_while(|index| unsafe { factory.EnumAdapters1(index) }.ok())
        .filter_map(|adapter| unsafe { adapter.GetDesc1() }.ok())
        .find(|desc| desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0)
        .map(|desc| {
            let length = desc
                .Description
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(desc.Description.len());
            String::from_utf16_lossy(&desc.Description[..length])
                .trim()
                .to_owned()
        })
        .filter(|name| !name.is_empty())
}

fn cpu() -> Option<String> {
    get_string(HKEY_LOCAL_MACHINE, CPU_KEY, CPU_VALUE)
        .map(|name| name.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|name| !name.is_empty())
}

fn memory_gb() -> Option<u64> {
    let mut kib = 0u64;
    unsafe { GetPhysicallyInstalledSystemMemory(&mut kib) }.ok()?;
    (kib > 0).then(|| (kib + KIB_PER_GIB / 2) / KIB_PER_GIB)
}

fn display() -> Option<(u32, u32, u32)> {
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    unsafe { EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut mode) }
        .as_bool()
        .then_some((mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency))
}

pub fn windows_name(product: &str, build: u32, version: Option<&str>) -> String {
    let product = if build >= WINDOWS_11_BUILD {
        product.replacen("Windows 10", "Windows 11", 1)
    } else {
        product.to_owned()
    };
    match version {
        Some(version) if !version.is_empty() => format!("{product} {version}"),
        _ => product,
    }
}

fn system() -> Option<String> {
    let product = get_string(HKEY_LOCAL_MACHINE, OS_KEY, "ProductName")?;
    let build = get_string(HKEY_LOCAL_MACHINE, OS_KEY, "CurrentBuildNumber")
        .and_then(|build| build.parse().ok())
        .unwrap_or(0);
    let version = get_string(HKEY_LOCAL_MACHINE, OS_KEY, "DisplayVersion");
    Some(windows_name(&product, build, version.as_deref()))
}

pub fn collect() -> Specs {
    Specs {
        gpu: gpu(),
        cpu: cpu(),
        memory_gb: memory_gb(),
        display: display(),
        system: system(),
    }
}

pub fn describe(specs: &Specs, choice: &Choice) -> Option<String> {
    if !choice.enabled {
        return None;
    }
    let lines: Vec<String> = [
        choice
            .gpu
            .then(|| specs.gpu.as_ref().map(|gpu| format!("Видеокарта: {gpu}")))
            .flatten(),
        choice
            .cpu
            .then(|| specs.cpu.as_ref().map(|cpu| format!("Процессор: {cpu}")))
            .flatten(),
        choice
            .memory
            .then(|| {
                specs
                    .memory_gb
                    .map(|gb| format!("Оперативная память: {gb} ГБ"))
            })
            .flatten(),
        choice
            .display
            .then(|| {
                specs
                    .display
                    .map(|(width, height, hertz)| format!("Монитор: {width}×{height}, {hertz} Гц"))
            })
            .flatten(),
        choice
            .system
            .then(|| {
                specs
                    .system
                    .as_ref()
                    .map(|system| format!("Система: {system}"))
            })
            .flatten(),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Specs {
        Specs {
            gpu: Some("NVIDIA GeForce RTX 3060".to_owned()),
            cpu: Some("12th Gen Intel(R) Core(TM) i5-12400F".to_owned()),
            memory_gb: Some(32),
            display: Some((2560, 1440, 165)),
            system: Some("Windows 11 Pro 24H2".to_owned()),
        }
    }

    #[test]
    fn nothing_is_shared_while_switched_off() {
        assert_eq!(describe(&sample(), &Choice::default()), None);
    }

    #[test]
    fn only_the_chosen_lines_are_shared() {
        let choice = Choice {
            enabled: true,
            cpu: false,
            display: false,
            ..Choice::default()
        };
        assert_eq!(
            describe(&sample(), &choice).as_deref(),
            Some(
                "Видеокарта: NVIDIA GeForce RTX 3060\nОперативная память: 32 ГБ\nСистема: Windows 11 Pro 24H2"
            )
        );
    }

    #[test]
    #[ignore = "reads this machine"]
    fn this_machine_is_described() {
        let specs = collect();
        let choice = Choice {
            enabled: true,
            ..Choice::default()
        };
        let text = describe(&specs, &choice).unwrap();
        println!("{text}");
        assert!(specs.gpu.is_some() && specs.cpu.is_some() && specs.system.is_some());
        assert!(specs.memory_gb.is_some() && specs.display.is_some());
    }

    #[test]
    fn windows_11_is_named_by_its_build() {
        assert_eq!(
            windows_name("Windows 10 Pro", 26_200, Some("25H2")),
            "Windows 11 Pro 25H2"
        );
        assert_eq!(
            windows_name("Windows 10 Pro", 19_045, None),
            "Windows 10 Pro"
        );
    }
}
