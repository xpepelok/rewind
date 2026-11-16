use std::sync::Arc;

use gpui::{
    App, Bounds, Context, CursorStyle, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ObjectFit, Render, RenderImage, ScrollWheelEvent, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, img, prelude::*, px,
};

use crate::platform::window::{self, Extent};
use crate::theme::{FONT_FAMILY, ThemeExt, alpha};
use crate::views::annotate::render_image;
use crate::views::picture::encode_png;

const ZOOM_STEP: f32 = 1.1;
const ZOOM_MIN: f32 = 0.2;
const ZOOM_MAX: f32 = 4.0;
const MIN_SIDE: i32 = 24;
const MENU_WIDTH: f32 = 176.0;
const MENU_ITEM: f32 = 34.0;
const MENU_PAD: f32 = 5.0;

pub fn zoomed(natural: (i32, i32), zoom: f32) -> (i32, i32) {
    let side = |length: i32| ((length as f32 * zoom).round() as i32).max(MIN_SIDE);
    (side(natural.0), side(natural.1))
}

pub fn dragged(origin: Extent, from: (i32, i32), to: (i32, i32)) -> Extent {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    Extent {
        left: origin.left + dx,
        top: origin.top + dy,
        right: origin.right + dx,
        bottom: origin.bottom + dy,
    }
}

fn clamp(at: f32, span: f32, extent: f32) -> f32 {
    at.min(extent - span).max(0.0)
}

fn options(bounds: Bounds<gpui::Pixels>) -> WindowOptions {
    WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        kind: WindowKind::PopUp,
        window_background: WindowBackgroundAppearance::Opaque,
        focus: true,
        show: false,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        ..WindowOptions::default()
    }
}

pub fn open(width: u32, height: u32, bgra: Vec<u8>, at: (i32, i32), scale: f32, cx: &mut App) {
    let accent = cx.palette().accent;
    let Some(image) = render_image(width, height, bgra.clone()) else {
        return;
    };
    let natural = (width as i32, height as i32);
    let extent = Extent {
        left: at.0,
        top: at.1,
        right: at.0 + natural.0,
        bottom: at.1 + natural.1,
    };
    // The window is placed in physical screen pixels, but gpui sizes its content
    // in logical pixels. Divide by the display scale so the fragment renders 1:1
    // at its native resolution instead of being upscaled (and blurred).
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let bounds = Bounds::new(
        gpui::point(px(at.0 as f32 / scale), px(at.1 as f32 / scale)),
        gpui::size(px(width as f32 / scale), px(height as f32 / scale)),
    );
    let accent_rgb = {
        let rgba = gpui::Rgba::from(accent);
        ((rgba.r * 255.0) as u32) << 16
            | ((rgba.g * 255.0) as u32) << 8
            | (rgba.b * 255.0) as u32
    };
    cx.open_window(options(bounds), |window, cx| {
        window::pin(window, extent, accent_rgb, cx);
        cx.new(|cx| {
            let focus = cx.focus_handle();
            window.focus(&focus);
            Pin {
                image: Some(image),
                bytes: bgra,
                natural,
                zoom: 1.0,
                drag: None,
                menu: None,
                focus,
            }
        })
    })
    .ok();
}

pub struct Pin {
    image: Option<Arc<RenderImage>>,
    bytes: Vec<u8>,
    natural: (i32, i32),
    zoom: f32,
    drag: Option<((i32, i32), Extent)>,
    menu: Option<(f32, f32)>,
    focus: FocusHandle,
}

impl Pin {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(image) = self.image.take() {
            cx.drop_image(image, Some(window));
        }
        window.remove_window();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let mut rgba = self.bytes.clone();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        if let Ok(png) = encode_png(rgba, self.natural.0 as u32, self.natural.1 as u32) {
            let _ = crate::platform::clipboard::copy_image(&png);
        }
        self.menu = None;
        cx.notify();
    }

    fn on_down(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
            return;
        }
        self.drag = window::cursor().zip(window::extent(_window));
    }

    fn on_right(&mut self, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        self.menu = Some((f32::from(event.position.x), f32::from(event.position.y)));
        cx.notify();
    }

    fn on_move(&mut self, event: &MouseMoveEvent, window: &mut Window, _cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let (Some((from, origin)), Some(to)) = (self.drag, window::cursor()) else {
            return;
        };
        window::place_pinned(window, dragged(origin, from, to), _cx);
    }

    fn on_wheel(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = f32::from(event.delta.pixel_delta(px(20.0)).y);
        if delta == 0.0 || self.drag.is_some() || self.menu.is_some() {
            return;
        }
        let factor = if delta > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let Some(current) = window::extent(window) else {
            return;
        };
        let (width, height) = zoomed(self.natural, self.zoom);
        window::place_pinned(
            window,
            Extent {
                left: current.left,
                top: current.top,
                right: current.left + width,
                bottom: current.top + height,
            },
            cx,
        );
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            if self.menu.take().is_some() {
                cx.notify();
            } else {
                self.close(window, cx);
            }
        }
    }

    fn context_menu(&self, at: (f32, f32), window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = cx.palette();
        let viewport = window.viewport_size();
        let height = MENU_ITEM * 2.0 + MENU_PAD * 2.0;
        let x = clamp(at.0, MENU_WIDTH, f32::from(viewport.width));
        let y = clamp(at.1, height, f32::from(viewport.height));
        let item = |id: &'static str, label: &'static str, danger: bool| {
            let color = if danger { p.record } else { p.text };
            div()
                .id(id)
                .w_full()
                .h(px(MENU_ITEM))
                .px(px(12.0))
                .flex()
                .items_center()
                .rounded(px(5.0))
                .text_size(px(13.0))
                .text_color(color)
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .child(label)
        };
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(MENU_WIDTH))
            .p(px(MENU_PAD))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .rounded(px(8.0))
            .bg(p.panel)
            .border_1()
            .border_color(alpha(p.white, 0.12))
            .shadow(p.toast_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                item("pin-copy", "Копировать", false)
                    .on_click(cx.listener(|pin, _, _, cx| pin.copy(cx))),
            )
            .child(
                item("pin-close", "Закрыть", true)
                    .on_click(cx.listener(|pin, _, window, cx| pin.close(window, cx))),
            )
            .into_any_element()
    }
}

impl Render for Pin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        let menu = self.menu.map(|at| self.context_menu(at, window, cx));
        div()
            .id("pin")
            .size_full()
            .relative()
            .font_family(FONT_FAMILY)
            .bg(p.stage)
            .cursor(CursorStyle::OpenHand)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|pin, _, _, _| pin.drag = None))
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .when_some(self.image.clone(), |pin, image| {
                pin.child(img(image).size_full().object_fit(ObjectFit::Fill))
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .size_full()
                    .border_1()
                    .border_color(p.accent),
            )
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_moves_the_pin_by_the_cursor_travel_and_keeps_its_size() {
        let origin = Extent {
            left: 100,
            top: 50,
            right: 400,
            bottom: 250,
        };
        let moved = dragged(origin, (10, 10), (-30, 25));
        assert_eq!((moved.left, moved.top), (60, 65));
        assert_eq!((moved.right - moved.left, moved.bottom - moved.top), (300, 200));
    }

    #[test]
    fn zooming_scales_both_sides_and_never_collapses() {
        assert_eq!(zoomed((300, 200), 1.0), (300, 200));
        assert_eq!(zoomed((300, 200), 2.0), (600, 400));
        assert_eq!(zoomed((300, 200), 0.01), (MIN_SIDE, MIN_SIDE));
    }

    #[test]
    fn the_menu_stays_inside_the_window() {
        assert_eq!(clamp(10.0, 176.0, 400.0), 10.0);
        assert_eq!(clamp(380.0, 176.0, 400.0), 224.0);
        assert_eq!(clamp(5.0, 176.0, 100.0), 0.0);
    }
}
