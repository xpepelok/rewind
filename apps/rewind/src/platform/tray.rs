use futures::channel::mpsc::UnboundedSender;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::state::Signal;

const SIZE: u32 = 32;
const ACCENT: [u8; 3] = [0x76, 0xb9, 0x00];
const RING_OUTER: f32 = 15.0;
const RING_INNER: f32 = 11.5;
const DOT: f32 = 6.5;
const ICON_RESOURCE: u16 = 1;
const OPEN_ID: &str = "open";
const QUIT_ID: &str = "quit";

pub struct Tray {
    _icon: TrayIcon,
}

fn coverage(distance: f32, edge: f32) -> f32 {
    (edge - distance + 0.5).clamp(0.0, 1.0)
}

fn drawn_icon() -> Option<Icon> {
    let center = (SIZE as f32 - 1.0) / 2.0;
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let distance = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            let ring = coverage(distance, RING_OUTER) * (1.0 - coverage(distance, RING_INNER));
            let alpha = ring.max(coverage(distance, DOT));
            pixels.extend_from_slice(&ACCENT);
            pixels.push((alpha * 255.0) as u8);
        }
    }
    Icon::from_rgba(pixels, SIZE, SIZE).ok()
}

fn icon() -> Option<Icon> {
    Icon::from_resource(ICON_RESOURCE, None)
        .ok()
        .or_else(drawn_icon)
}

impl Tray {
    pub fn install(signals: UnboundedSender<Signal>) -> Option<Self> {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id(OPEN_ID, "Открыть оверлей", true, None))
            .ok()?;
        menu.append(&MenuItem::with_id(QUIT_ID, "Выход", true, None))
            .ok()?;
        let menu_signals = signals.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let signal = if event.id == MenuId::new(QUIT_ID) {
                Signal::Quit
            } else {
                Signal::Show
            };
            menu_signals.unbounded_send(signal).ok();
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                signals.unbounded_send(Signal::Show).ok();
            }
        }));
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Rewind")
            .with_icon(icon()?)
            .build()
            .ok()?;
        Some(Self { _icon: icon })
    }
}
