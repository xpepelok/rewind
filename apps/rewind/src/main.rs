#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod channels;
mod clip_share;
mod editor_files;
mod gallery;
mod motion;
mod platform;
mod share;
mod state;
mod theme;
mod ui;
mod update;
mod uploads;
mod views;
mod watchdog;

use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::{
    App, AppContext, Bounds, Pixels, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, px, size,
};

use rewind_config::{Paths, Theme};

use crate::assets::Assets;
use crate::platform::instance::Instance;
use crate::platform::system;
use crate::platform::tray::Tray;
use crate::state::Signal;
use crate::views::hud::Hud;
use crate::views::installer::Installer;
use crate::views::overlay::Overlay;

const APP_ID: &str = "Rewind";
const SHOW_FLAG: &str = "--show";
const FAILURE_CODE: i32 = 1;

fn wants_overlay() -> bool {
    std::env::args()
        .skip(1)
        .any(|arg| arg == SHOW_FLAG || arg == system::NVIDIA_FLAG)
}

fn wants_installer() -> bool {
    let flagged = std::env::args()
        .skip(1)
        .any(|arg| arg == SHOW_FLAG || arg == system::NVIDIA_FLAG || arg == system::ELEVATED_FLAG);
    !flagged && !system::running_installed_copy()
}

fn run_installer() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            theme::install(cx);
            theme::apply(Theme::Classic, cx);
            let bounds = Bounds::centered(None, size(px(1100.0), px(992.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Rewind".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                kind: WindowKind::Normal,
                focus: true,
                show: true,
                is_movable: true,
                is_resizable: false,
                is_minimizable: true,
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some(APP_ID.to_owned()),
                ..WindowOptions::default()
            };
            if cx
                .open_window(options, |_window, cx| cx.new(|_| Installer::new()))
                .is_err()
            {
                cx.quit();
                return;
            }
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
        });
}

fn system_command() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let elevated = args.iter().any(|arg| arg == system::ELEVATED_FLAG);
    let (flag, job): (&str, fn() -> std::io::Result<()>) =
        if args.iter().any(|arg| arg == system::INSTALL_FLAG) {
            (system::INSTALL_FLAG, system::install)
        } else if args.iter().any(|arg| arg == system::UNINSTALL_FLAG) {
            (system::UNINSTALL_FLAG, system::uninstall)
        } else {
            return None;
        };
    let succeeded = if elevated || system::is_admin() {
        job().is_ok()
    } else {
        system::run_elevated(flag).unwrap_or(false)
    };
    if succeeded && args.iter().any(|arg| arg == system::RELAUNCH_FLAG) {
        system::start_elevated();
    }
    Some(if succeeded { 0 } else { FAILURE_CODE })
}

fn layer_options(
    bounds: Bounds<Pixels>,
    display: Option<gpui::DisplayId>,
    show: bool,
) -> WindowOptions {
    WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        kind: WindowKind::PopUp,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
        show,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display,
        app_id: Some(APP_ID.to_owned()),
        ..WindowOptions::default()
    }
}

fn main() -> anyhow::Result<()> {
    if let Some(code) = system_command() {
        std::process::exit(code);
    }
    if wants_installer() {
        if !system::is_admin() && system::relaunch_elevated() {
            return Ok(());
        }
        run_installer();
        return Ok(());
    }
    let show_now = wants_overlay();
    if system::needs_elevation() {
        if Instance::signal_running() {
            return Ok(());
        }
        if system::start_elevated() {
            if show_now {
                Instance::signal_when_ready();
            }
            return Ok(());
        }
    }
    let Some(instance) = Instance::acquire() else {
        Instance::signal_running();
        return Ok(());
    };
    let (signals, mut receiver) = unbounded::<Signal>();
    instance.listen(signals.clone());

    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            theme::install(cx);
            ui::input::bind_keys(cx);
            let rewind = state::create(Paths::resolve(), signals.clone(), cx);
            theme::apply(rewind.read(cx).settings.interface.theme, cx);

            let display = cx.primary_display();
            let bounds = display
                .as_ref()
                .map(|display| display.bounds())
                .unwrap_or_else(|| {
                    Bounds::centered(None, gpui::size(gpui::px(1920.0), gpui::px(1080.0)), cx)
                });
            let display_id = display.map(|display| display.id());

            let overlay_state = rewind.clone();
            let overlay = cx.open_window(layer_options(bounds, display_id, false), |window, cx| {
                cx.new(|cx| Overlay::new(overlay_state, window, cx))
            });
            let huds = crate::views::hud::PARTS.map(|part| {
                let hud_state = rewind.clone();
                cx.open_window(layer_options(bounds, display_id, true), |window, cx| {
                    cx.new(|cx| Hud::new(hud_state, part, window, cx))
                })
            });
            if overlay.is_err() || huds.iter().any(Result::is_err) {
                cx.quit();
                return;
            }

            let tray = Tray::install(signals.clone());
            let pump = rewind.clone();
            cx.spawn(async move |cx| {
                let _tray = tray;
                let _instance = instance;
                while let Some(signal) = receiver.next().await {
                    let quit = matches!(signal, Signal::Quit);
                    pump.update(cx, |rewind, cx| rewind.handle(signal, cx));
                    if quit {
                        break;
                    }
                }
            })
            .detach();

            if show_now {
                signals.unbounded_send(Signal::Show).ok();
            }
        });
    Ok(())
}
