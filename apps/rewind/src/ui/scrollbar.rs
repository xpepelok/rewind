use gpui::{
    AnyElement, App, DispatchPhase, Div, ElementId, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, RenderOnce, ScrollHandle, Window, canvas, div, point, prelude::*,
    px,
};

use crate::theme::{ThemeExt, alpha};

const TRACK: f32 = 14.0;
const THUMB: f32 = 6.0;
const INSET: f32 = 4.0;
const SHORTEST: f32 = 36.0;
const RESTING: f32 = 0.55;

fn thumb(view: f32, range: f32, offset: f32) -> Option<(f32, f32)> {
    if view <= 0.0 || range < 1.0 {
        return None;
    }
    let length = (view * view / (view + range)).max(SHORTEST).min(view);
    let travel = offset.clamp(0.0, range) / range;
    Some(((view - length) * travel, length))
}

fn offset_for(view: f32, range: f32, length: f32, top: f32) -> f32 {
    let free = view - length;
    if free <= 0.0 {
        return 0.0;
    }
    (top / free).clamp(0.0, 1.0) * range
}

#[derive(IntoElement)]
pub struct Scrollbar {
    id: ElementId,
    handle: ScrollHandle,
}

impl Scrollbar {
    pub fn new(id: impl Into<ElementId>, handle: ScrollHandle) -> Self {
        Self {
            id: id.into(),
            handle,
        }
    }
}

pub fn with_scrollbar(content: AnyElement, id: &'static str, handle: &ScrollHandle) -> Div {
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(content)
        .child(Scrollbar::new(id, handle.clone()))
}

impl RenderOnce for Scrollbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        let handle = self.handle;
        let area = handle.bounds();
        let view = f32::from(area.size.height);
        let range = f32::from(handle.max_offset().height);
        let scrolled = -f32::from(handle.offset().y);
        let Some((top, length)) = thumb(view, range, scrolled) else {
            return div().into_any_element();
        };
        let grab = window.use_keyed_state(self.id.clone(), cx, |_, _| None::<f32>);
        let held = grab.read(cx).is_some();
        let origin = f32::from(area.origin.y);
        let jump = {
            let handle = handle.clone();
            move |top: f32| {
                let target = offset_for(view, range, length, top);
                handle.set_offset(point(handle.offset().x, px(-target)));
            }
        };
        let press = {
            let (grab, jump) = (grab.clone(), jump.clone());
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                let at = f32::from(event.position.y) - origin;
                let inside = if (top..=top + length).contains(&at) {
                    at - top
                } else {
                    length / 2.0
                };
                jump(at - inside);
                grab.update(cx, |grab, _| *grab = Some(inside));
                cx.stop_propagation();
                window.refresh();
            }
        };
        let listeners = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let (moving, jump) = (grab.clone(), jump.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let Some(inside) = *moving.read(cx) else {
                        return;
                    };
                    if event.pressed_button == Some(MouseButton::Left) {
                        jump(f32::from(event.position.y) - origin - inside);
                    } else {
                        moving.update(cx, |grab, _| *grab = None);
                    }
                    window.refresh();
                });
                let releasing = grab.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && releasing.read(cx).is_some() {
                        releasing.update(cx, |grab, _| *grab = None);
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .size_0();
        let resting = alpha(p.muted, RESTING);
        div()
            .id(self.id)
            .group("scrollbar")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(TRACK))
            .occlude()
            .on_mouse_down(MouseButton::Left, press)
            .child(
                div()
                    .absolute()
                    .top(px(top))
                    .right(px(INSET))
                    .w(px(THUMB))
                    .h(px(length))
                    .rounded_full()
                    .bg(if held { p.text } else { resting })
                    .group_hover("scrollbar", |thumb| thumb.bg(p.text)),
            )
            .child(listeners)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_scroll_shows_no_thumb() {
        assert_eq!(thumb(600.0, 0.0, 0.0), None);
        assert_eq!(thumb(0.0, 500.0, 0.0), None);
    }

    #[test]
    fn the_thumb_spans_the_track_from_top_to_bottom() {
        let (top, length) = thumb(600.0, 600.0, 0.0).unwrap();
        assert_eq!((top, length), (0.0, 300.0));
        let (top, length) = thumb(600.0, 600.0, 600.0).unwrap();
        assert_eq!(top + length, 600.0);
        let (_, length) = thumb(600.0, 600_000.0, 0.0).unwrap();
        assert_eq!(length, SHORTEST);
    }

    #[test]
    fn dragging_the_thumb_maps_back_to_the_same_offset() {
        let (view, range) = (600.0, 2400.0);
        for offset in [0.0, 300.0, 1200.0, 2400.0] {
            let (top, length) = thumb(view, range, offset).unwrap();
            assert!((offset_for(view, range, length, top) - offset).abs() < 0.01);
        }
        assert_eq!(offset_for(view, range, 120.0, -50.0), 0.0);
        assert_eq!(offset_for(view, range, 120.0, 9000.0), range);
    }
}
