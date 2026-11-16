pub mod images;
pub mod input;
pub mod scrollbar;

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, DispatchPhase, Div, ElementId, FontWeight, Hsla, IntoElement,
    MouseButton, ObjectFit, Pixels, Render, RenderOnce, ScrollHandle, ScrollWheelEvent,
    SharedString, Window, anchored, canvas, deferred, div, fill, img, point, prelude::*, px,
    relative, size,
};

use crate::assets::{Icon, icon};
use crate::motion::{self, Key};
use crate::theme::{
    BODY_TEXT, CONTROL_TEXT, DIVIDER_INSET, GUTTER, ICON, Palette, SMALL_TEXT, TEXT_INSET,
    THEMED_CONTENT_INSET, ThemeExt, alpha,
};

type StepHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type ValueHandler = Rc<dyn Fn(u32, &mut Window, &mut App)>;
type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

const SELECTOR_WIDTH: f32 = 210.0;
const DROPDOWN_ITEM: f32 = 36.0;
const DROPDOWN_AVATAR: f32 = 22.0;
const DROPDOWN_ROWS: usize = 8;
const THUMB_WIDTH: f32 = 80.0;
const SCROLLBAR_WIDTH: f32 = 4.0;
const SCROLLBAR_INSET: f32 = 3.0;
const SCROLLBAR_SPACE: f32 = 8.0;
const SCROLLBAR_MIN: f32 = 24.0;
const SLIDER_HEIGHT: f32 = 20.0;
const KNOB: f32 = 14.0;
const DOT: f32 = 2.0;
const DOT_GAP: f32 = 4.0;
const TOGGLE_WIDTH: f32 = 36.0;
const TOGGLE_TRACK: f32 = 14.0;
const TOGGLE_THUMB: f32 = 20.0;
const CHECKBOX: f32 = 20.0;
const KEYBOX_WIDTH: f32 = 120.0;
const KEYBOX_HEIGHT: f32 = 40.0;
const BUTTON_HEIGHT: f32 = 38.0;
const ROW_INSET: f32 = 8.0;
const CARD_INSET: f32 = 12.0;
const CARD_GAP: f32 = 4.0;
const CARD_ICON_GAP: f32 = 16.0;
const SOFT_RAIL: f32 = 4.0;
const SQUARE_RAIL_RADIUS: f32 = 1.0;
const ROUND_RAIL_RADIUS: f32 = SOFT_RAIL / 2.0;
const SOFT_KNOB: f32 = 16.0;
const SOFT_TOGGLE_WIDTH: f32 = 40.0;
const SOFT_TOGGLE_HEIGHT: f32 = 22.0;
const SOFT_TOGGLE_THUMB: f32 = 16.0;
const SOFT_VALUE_HEIGHT: f32 = 26.0;
const SOFT_SELECTOR_HEIGHT: f32 = 34.0;
const LIGHT_CHECKBOX_RADIUS: f32 = 4.0;

pub fn divider(p: &Palette) -> Div {
    div()
        .flex_none()
        .h(px(1.0))
        .mx(px(DIVIDER_INSET))
        .my(px(12.0))
        .bg(p.divider)
}

pub fn section(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .px(px(if p.soft { THEMED_CONTENT_INSET } else { GUTTER }))
        .pt(px(14.0))
        .pb(px(6.0))
        .text_size(px(SMALL_TEXT))
        .text_color(p.muted)
        .child(text.into())
}

pub fn label(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .min_w_0()
        .text_size(px(BODY_TEXT))
        .text_color(color)
        .child(text.into())
}

pub fn hint(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .min_w_0()
        .mt(px(2.0))
        .text_size(px(SMALL_TEXT))
        .text_color(p.muted)
        .child(text.into())
}

pub struct Tooltip(SharedString);

impl Tooltip {
    pub fn text(text: &'static str) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
        move |_, cx| cx.new(|_| Self(text.into())).into()
    }

    pub fn owned(text: SharedString) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
        move |_, cx| cx.new(|_| Self(text.clone())).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(p.toast)
            .rounded(px(p.radius()))
            .shadow(p.toast_shadow())
            .text_size(px(SMALL_TEXT))
            .text_color(p.title)
            .child(self.0.clone())
    }
}

pub fn row_icon(glyph: Icon, p: &Palette) -> Div {
    div()
        .flex_none()
        .size(px(ICON))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph, ICON, p.icon))
}

pub fn setting_row(p: &Palette) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .px(px(GUTTER))
        .py(px(10.0))
        .min_h(px(52.0))
        .when(p.soft, |row| {
            row.mx(px(ROW_INSET))
                .my(px(CARD_GAP))
                .px(px(CARD_INSET))
                .py(px(CARD_INSET))
                .rounded(px(p.radius()))
        })
        .when_some(p.card, |row, card| row.bg(card))
        .when_some(p.row_line, |row, line| row.border_1().border_color(line))
}

pub fn nav_row(id: impl Into<ElementId>, hover: Key, p: &Palette, cx: &App) -> gpui::Stateful<Div> {
    let t = motion::value(hover, cx);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(if p.soft {
            CARD_ICON_GAP
        } else {
            TEXT_INSET - GUTTER - ICON
        }))
        .pl(px(GUTTER))
        .pr(px(GUTTER))
        .py(px(12.0))
        .when(p.soft, |row| {
            row.mx(px(ROW_INSET))
                .px(px(CARD_INSET))
                .rounded(px(p.radius()))
        })
        .when(p.card.is_some() || p.row_line.is_some(), |row| {
            row.my(px(CARD_GAP))
        })
        .map(|row| match p.card {
            Some(card) => row.bg(crate::theme::blend(card, p.hover, t)),
            None => row.bg(alpha(p.hover, t)),
        })
        .when_some(p.row_line, |row, line| row.border_1().border_color(line))
        .cursor_pointer()
        .on_hover(move |hovered, window, cx| motion::hover(hover, *hovered, window, cx))
        .on_click(move |_, window, cx| motion::hover(hover, false, window, cx))
}

#[derive(IntoElement)]
pub struct Selector {
    id: ElementId,
    value: SharedString,
    disabled: bool,
    outlined: bool,
    on_step: Option<StepHandler>,
}

impl Selector {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            disabled: false,
            outlined: false,
            on_step: None,
        }
    }

    pub fn outlined(mut self) -> Self {
        self.outlined = true;
        self
    }

    pub fn on_step(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_step = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Selector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        let color = if self.disabled { p.disabled } else { p.text };
        let arrow = |glyph: Icon, forward: bool, handler: Option<StepHandler>| {
            div()
                .id(if forward { "next" } else { "previous" })
                .flex_none()
                .p(px(4.0))
                .cursor_pointer()
                .child(icon(glyph, 20.0, color))
                .when_some(handler, move |arrow, handler| {
                    arrow.on_click(move |_, window, cx| handler(forward, window, cx))
                })
        };
        let handler = (!self.disabled).then_some(self.on_step).flatten();
        div()
            .id(self.id)
            .flex_none()
            .w(px(SELECTOR_WIDTH))
            .flex()
            .items_center()
            .justify_between()
            .when(p.soft, |selector| {
                selector
                    .h(px(SOFT_SELECTOR_HEIGHT))
                    .px(px(4.0))
                    .rounded(px(p.radius()))
                    .bg(p.track)
                    .when(self.outlined, |selector| {
                        selector.border_1().border_color(p.field_line)
                    })
            })
            .child(arrow(Icon::ChevronLeft, false, handler.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .text_center()
                    .text_size(px(CONTROL_TEXT))
                    .text_color(color)
                    .child(self.value),
            )
            .child(arrow(Icon::ChevronRight, true, handler))
    }
}

type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(Clone)]
pub struct Avatar {
    pub file: Option<PathBuf>,
    pub initials: SharedString,
}

impl Avatar {
    pub fn of(account: &youtube::Account) -> Self {
        Self {
            file: account.avatar_file.clone().filter(|file| file.exists()),
            initials: account.initials().into(),
        }
    }
}

pub fn avatar(avatar: &Avatar, size_px: f32, p: &Palette) -> Div {
    let base = div()
        .flex_none()
        .size(px(size_px))
        .rounded_full()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(p.accent_dim);
    match avatar.file.clone() {
        Some(file) => base.child(
            img(file)
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover),
        ),
        None => base
            .text_size(px((size_px * 0.36).max(9.0)))
            .text_color(p.title)
            .child(avatar.initials.clone()),
    }
}

pub fn thumbnail(preview: Option<PathBuf>, fallback: Icon, p: &Palette) -> Div {
    let frame = div()
        .flex_none()
        .w(px(THUMB_WIDTH))
        .h(px(THUMB_WIDTH * 9.0 / 16.0))
        .rounded(px(p.radius().max(4.0)))
        .overflow_hidden()
        .bg(p.field)
        .flex()
        .items_center()
        .justify_center();
    match preview {
        Some(path) => frame.child(img(path).size_full().object_fit(ObjectFit::Cover)),
        None => frame.child(icon(fallback, ICON, p.icon)),
    }
}

#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    options: Vec<SharedString>,
    selected: usize,
    avatars: Vec<Avatar>,
    shown: Option<f32>,
    on_toggle: Option<ClickHandler>,
    on_select: Option<SelectHandler>,
}

impl Dropdown {
    pub fn new(id: impl Into<ElementId>, options: Vec<SharedString>, selected: usize) -> Self {
        Self {
            id: id.into(),
            options,
            selected,
            avatars: Vec::new(),
            shown: None,
            on_toggle: None,
            on_select: None,
        }
    }

    pub fn avatars(mut self, avatars: Vec<Avatar>) -> Self {
        self.avatars = avatars;
        self
    }

    pub fn shown(mut self, shown: Option<f32>) -> Self {
        self.shown = shown;
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Dropdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        let value = self.options.get(self.selected).cloned().unwrap_or_default();
        let open = self.shown.is_some();
        let current = self.avatars.get(self.selected).cloned();
        let trigger = div()
            .id("trigger")
            .h(px(DROPDOWN_ITEM))
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(6.0))
            .bg(p.field)
            .map(|trigger| {
                if p.soft {
                    trigger.border_1().rounded(px(p.radius()))
                } else {
                    trigger.border_b_1()
                }
            })
            .border_color(if open { p.accent } else { p.field_line })
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .when_some(current, |trigger, current| {
                trigger.child(avatar(&current, DROPDOWN_AVATAR, &p))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(CONTROL_TEXT))
                    .text_color(p.text)
                    .child(value),
            )
            .child(icon(Icon::Expand, 20.0, p.icon))
            .when_some(self.on_toggle, |trigger, handler| {
                trigger.on_click(move |_, window, cx| handler(window, cx))
            });
        let count = self.options.len();
        let scrolls = count > DROPDOWN_ROWS;
        let content = DROPDOWN_ITEM * count as f32;
        let visible = DROPDOWN_ITEM * count.min(DROPDOWN_ROWS) as f32;
        let selected = self.selected;
        let list = self.shown.map(|shown| {
            let scroll = window
                .use_keyed_state(self.id.clone(), cx, |_, _| {
                    let handle = ScrollHandle::new();
                    handle.scroll_to_item(selected);
                    handle
                })
                .read(cx)
                .clone();
            let items = self.options.into_iter().enumerate().map(|(index, option)| {
                let chosen = index == self.selected;
                let handler = self.on_select.clone();
                let face = self.avatars.get(index).cloned();
                div()
                    .id(("option", index))
                    .flex_none()
                    .h(px(DROPDOWN_ITEM))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .pl(px(10.0))
                    .pr(px(if scrolls {
                        10.0 + SCROLLBAR_SPACE
                    } else {
                        10.0
                    }))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.dots))
                    .when_some(face, |item, face| {
                        item.child(avatar(&face, DROPDOWN_AVATAR, &p))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(CONTROL_TEXT))
                            .text_color(if chosen { p.accent } else { p.text })
                            .child(option),
                    )
                    .when(chosen, |item| item.child(icon(Icon::Check, 18.0, p.accent)))
                    .when_some(handler, move |item, handler| {
                        item.on_click(move |_, window, cx| handler(index, window, cx))
                    })
            });
            let range = content - visible;
            let thumb = scrolls.then(|| {
                let length = (visible * visible / content).max(SCROLLBAR_MIN);
                let travel = (-f32::from(scroll.offset().y)).clamp(0.0, range) / range;
                div()
                    .absolute()
                    .top(px(4.0 + (visible - length) * travel))
                    .right(px(SCROLLBAR_INSET))
                    .w(px(SCROLLBAR_WIDTH))
                    .h(px(length))
                    .rounded_full()
                    .bg(p.muted)
            });
            let wheel = scrolls.then(|| {
                let scroll = scroll.clone();
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
                            if phase != DispatchPhase::Bubble
                                || scroll.bounds().contains(&event.position)
                            {
                                return;
                            }
                            let delta = event.delta.pixel_delta(px(DROPDOWN_ITEM)).y;
                            let offset = (scroll.offset().y + delta).clamp(px(-range), px(0.0));
                            scroll.set_offset(point(px(0.0), offset));
                            window.refresh();
                        })
                    },
                )
                .absolute()
                .size_0()
            });
            deferred(
                anchored()
                    .offset(point(px(0.0), px(DROPDOWN_ITEM)))
                    .snap_to_window()
                    .child(
                        div()
                            .id("options")
                            .relative()
                            .w(px(SELECTOR_WIDTH))
                            .mt(px(2.0 - 6.0 * (1.0 - shown)))
                            .py(px(4.0))
                            .opacity(shown)
                            .bg(p.card.unwrap_or(p.header))
                            .rounded(px(p.radius()))
                            .shadow(p.toast_shadow())
                            .occlude()
                            .on_scroll_wheel(|_, window, _| window.refresh())
                            .child(
                                div()
                                    .id("options-scroll")
                                    .h(px(visible))
                                    .flex()
                                    .flex_col()
                                    .overflow_y_scroll()
                                    .track_scroll(&scroll)
                                    .children(items),
                            )
                            .children(thumb)
                            .children(wheel),
                    ),
            )
            .with_priority(1)
        });
        div()
            .id(self.id)
            .flex_none()
            .w(px(SELECTOR_WIDTH))
            .flex()
            .flex_col()
            .child(trigger)
            .children(list)
    }
}

#[derive(Default)]
struct SliderState {
    bounds: Option<Bounds<Pixels>>,
}

struct SliderDrag(ElementId);

struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn slider_value(x: Pixels, bounds: Bounds<Pixels>, min: u32, max: u32, step: u32) -> u32 {
    let width = f32::from(bounds.size.width).max(1.0);
    let fraction = (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0);
    let raw = min as f32 + fraction * (max - min) as f32;
    let stepped = ((raw - min as f32) / step.max(1) as f32).round() as u32 * step.max(1) + min;
    stepped.clamp(min, max)
}

#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    value: u32,
    min: u32,
    max: u32,
    step: u32,
    disabled: bool,
    on_input: Option<ValueHandler>,
}

impl Slider {
    pub fn new(id: impl Into<ElementId>, value: u32, min: u32, max: u32, step: u32) -> Self {
        Self {
            id: id.into(),
            value,
            min,
            max,
            step,
            disabled: false,
            on_input: None,
        }
    }

    pub fn on_input(mut self, handler: impl Fn(u32, &mut Window, &mut App) + 'static) -> Self {
        self.on_input = Some(Rc::new(handler));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SliderState::default());
        let span = (self.max - self.min).max(1) as f32;
        let fraction = (self.value.clamp(self.min, self.max) - self.min) as f32 / span;
        let (fill_color, dot_color, knob_color) = if self.disabled {
            (p.disabled, p.disabled, p.disabled)
        } else {
            (p.accent, p.dots, p.accent)
        };
        let soft = p.soft;
        let knob = if soft { SOFT_KNOB } else { KNOB };
        let rail = soft.then(|| {
            div()
                .absolute()
                .top(px((SLIDER_HEIGHT - SOFT_RAIL) / 2.0))
                .left_0()
                .right_0()
                .h(px(SOFT_RAIL))
                .rounded(px(if p.square {
                    SQUARE_RAIL_RADIUS
                } else {
                    ROUND_RAIL_RADIUS
                }))
                .bg(p.dots)
                .when(p.square, |rail| rail.border_1().border_color(p.field_line))
                .child(
                    div()
                        .h_full()
                        .w(relative(fraction))
                        .rounded(px(if p.square {
                            SQUARE_RAIL_RADIUS
                        } else {
                            ROUND_RAIL_RADIUS
                        }))
                        .bg(fill_color),
                )
        });
        let painter = {
            let state = state.clone();
            canvas(
                move |bounds, _, cx| {
                    state.update(cx, |state, _| state.bounds = Some(bounds));
                    bounds
                },
                move |_, bounds, window, _| {
                    if soft {
                        return;
                    }
                    let middle = bounds.origin.y + bounds.size.height / 2.0 - px(1.0);
                    let width = f32::from(bounds.size.width);
                    let split = width * fraction;
                    window.paint_quad(fill(
                        Bounds::new(point(bounds.origin.x, middle), size(px(split), px(2.0))),
                        fill_color,
                    ));
                    let mut x = split + KNOB / 2.0 + DOT_GAP;
                    while x + DOT <= width {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(x), middle),
                                size(px(DOT), px(DOT)),
                            ),
                            dot_color,
                        ));
                        x += DOT + DOT_GAP;
                    }
                },
            )
            .absolute()
            .size_full()
        };
        let track = div()
            .id(self.id.clone())
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(SLIDER_HEIGHT))
            .child(painter)
            .children(rail)
            .child(
                div()
                    .absolute()
                    .top(px((SLIDER_HEIGHT - knob) / 2.0))
                    .left(relative(fraction))
                    .ml(px(-knob / 2.0))
                    .size(px(knob))
                    .rounded(px(p.knob_radius(knob)))
                    .bg(if p.square {
                        p.accent
                    } else if soft {
                        p.white
                    } else {
                        knob_color
                    })
                    .when(soft && !p.square, |knob| {
                        knob.border_2().border_color(knob_color)
                    }),
            );
        if self.disabled {
            return track;
        }
        let (min, max, step) = (self.min, self.max, self.step);
        let input = self.on_input;
        let own = self.id.clone();
        track
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                let input = input.clone();
                move |event, window, cx| {
                    let Some(bounds) = state.read(cx).bounds else {
                        return;
                    };
                    if let Some(input) = input.as_ref() {
                        input(
                            slider_value(event.position.x, bounds, min, max, step),
                            window,
                            cx,
                        );
                    }
                }
            })
            .on_drag(SliderDrag(self.id), |_, _, _, cx| cx.new(|_| DragGhost))
            .on_drag_move::<SliderDrag>(move |event, window, cx| {
                if event.drag(cx).0 != own {
                    return;
                }
                if let Some(input) = input.as_ref() {
                    input(
                        slider_value(event.event.position.x, event.bounds, min, max, step),
                        window,
                        cx,
                    );
                }
            })
    }
}

pub fn value_box(text: impl Into<SharedString>, p: &Palette, width: f32) -> Div {
    if p.soft {
        return div()
            .flex_none()
            .w(px(width))
            .h(px(SOFT_VALUE_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(p.radius()))
            .bg(p.field)
            .when(p.square || p.row_line.is_some(), |value| {
                value.border_1().border_color(p.field_line)
            })
            .text_size(px(CONTROL_TEXT))
            .text_color(p.text)
            .child(text.into());
    }
    div()
        .flex_none()
        .w(px(width))
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .pb(px(2.0))
                .text_size(px(CONTROL_TEXT))
                .text_color(p.icon)
                .child(text.into()),
        )
        .child(div().w_full().h(px(1.0)).bg(p.field_line))
}

#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    on: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        Self {
            id: id.into(),
            on,
            disabled: false,
            on_click: None,
        }
    }

    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = cx.palette();
        let disabled = self.disabled;
        let (track, thumb) = if self.on && !disabled {
            (p.accent_dim, p.accent)
        } else if disabled {
            (p.dots, p.disabled)
        } else {
            (p.dots, p.icon)
        };
        if p.soft {
            let inset = (SOFT_TOGGLE_HEIGHT - SOFT_TOGGLE_THUMB) / 2.0;
            let lit = self.on && !disabled;
            return div()
                .id(self.id)
                .relative()
                .flex_none()
                .w(px(SOFT_TOGGLE_WIDTH))
                .h(px(SOFT_TOGGLE_HEIGHT))
                .rounded(px(if p.square {
                    p.radius()
                } else {
                    p.knob_radius(SOFT_TOGGLE_HEIGHT) + 1.0
                }))
                .bg(if p.square && !lit {
                    p.panel
                } else if lit {
                    p.accent
                } else {
                    track
                })
                .when(p.square || p.row_line.is_some(), |toggle| {
                    toggle
                        .border_1()
                        .border_color(if lit { p.accent } else { p.field_line })
                })
                .when(!disabled, gpui::Styled::cursor_pointer)
                .child(
                    div()
                        .absolute()
                        .top(px(inset))
                        .left(px(if self.on {
                            SOFT_TOGGLE_WIDTH - SOFT_TOGGLE_THUMB - inset
                        } else {
                            inset
                        }))
                        .size(px(SOFT_TOGGLE_THUMB))
                        .rounded(px(p.knob_radius(SOFT_TOGGLE_THUMB)))
                        .bg(if lit { p.check_mark } else { thumb }),
                )
                .when_some(self.on_click.filter(|_| !disabled), |toggle, handler| {
                    toggle.on_click(move |_, window, cx| handler(window, cx))
                });
        }
        div()
            .id(self.id)
            .relative()
            .flex_none()
            .w(px(TOGGLE_WIDTH))
            .h(px(TOGGLE_THUMB))
            .when(!disabled, gpui::Styled::cursor_pointer)
            .child(
                div()
                    .absolute()
                    .top(px((TOGGLE_THUMB - TOGGLE_TRACK) / 2.0))
                    .left_0()
                    .w(px(TOGGLE_WIDTH))
                    .h(px(TOGGLE_TRACK))
                    .rounded_full()
                    .bg(track),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(if self.on {
                        TOGGLE_WIDTH - TOGGLE_THUMB
                    } else {
                        0.0
                    }))
                    .size(px(TOGGLE_THUMB))
                    .rounded_full()
                    .bg(thumb),
            )
            .when_some(self.on_click.filter(|_| !disabled), |toggle, handler| {
                toggle.on_click(move |_, window, cx| handler(window, cx))
            })
    }
}

pub fn checkbox(checked: bool, p: &Palette) -> Div {
    let base = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(CHECKBOX))
        .map(|base| {
            if p.soft {
                base.rounded(px(if p.row_line.is_some() && !p.square {
                    LIGHT_CHECKBOX_RADIUS
                } else {
                    p.radius().min(CHECKBOX / 2.0)
                }))
            } else {
                base.rounded(px(2.0))
            }
        });
    if checked {
        base.bg(p.accent)
            .child(icon(Icon::Check, 18.0, p.check_mark))
    } else {
        base.border_2()
            .border_color(if p.soft { p.field_line } else { p.icon })
    }
}

pub fn keybox(text: impl Into<SharedString>, active: bool, p: &Palette) -> Div {
    div()
        .flex_none()
        .min_w(px(KEYBOX_WIDTH))
        .h(px(KEYBOX_HEIGHT))
        .whitespace_nowrap()
        .flex()
        .items_center()
        .justify_center()
        .px(px(12.0))
        .bg(p.field)
        .rounded(px(p.radius()))
        .border_1()
        .border_color(if active {
            p.accent
        } else if p.soft {
            p.field_line
        } else {
            p.field
        })
        .text_size(px(CONTROL_TEXT))
        .text_color(if active { p.accent } else { p.text })
        .child(text.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

pub fn button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    kind: ButtonKind,
    enabled: bool,
    p: &Palette,
) -> gpui::Stateful<Div> {
    let (background, foreground, border) = match kind {
        ButtonKind::Primary if !enabled && p.soft => (p.accent_dim, p.text, p.field_line),
        ButtonKind::Primary => (p.accent, p.check_mark, p.accent),
        ButtonKind::Secondary if p.soft => (p.field, p.text, p.field_line),
        ButtonKind::Secondary => (p.panel, p.text, p.divider),
        ButtonKind::Danger => (p.panel, p.record, p.record),
    };
    let hover = match kind {
        ButtonKind::Primary => p.accent_hover,
        ButtonKind::Secondary | ButtonKind::Danger => p.hover,
    };
    div()
        .id(id)
        .flex_none()
        .h(px(BUTTON_HEIGHT))
        .px(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded(px(p.radius()))
        .text_size(px(CONTROL_TEXT))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(foreground)
        .when(enabled, |button| {
            button.cursor_pointer().hover(move |style| style.bg(hover))
        })
        .when(!enabled && !p.soft, |button| button.opacity(0.4))
        .child(text.into())
}

pub fn field(input: impl IntoElement, multiline: bool, p: &Palette) -> Div {
    div()
        .flex()
        .w_full()
        .min_h(px(if multiline { 88.0 } else { 38.0 }))
        .px(px(10.0))
        .py(px(8.0))
        .bg(p.field)
        .map(|field| {
            if p.soft {
                field.border_1().rounded(px(p.radius()))
            } else {
                field.border_b_1()
            }
        })
        .border_color(p.field_line)
        .text_size(px(CONTROL_TEXT))
        .text_color(p.text)
        .child(input)
}

pub fn strong(text: impl Into<SharedString>, size_px: f32, color: Hsla) -> Div {
    div()
        .text_size(px(size_px))
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_snaps_and_clamps() {
        let bounds = Bounds::new(point(px(10.0), px(0.0)), size(px(100.0), px(10.0)));
        assert_eq!(slider_value(px(10.0), bounds, 15, 1200, 15), 15);
        assert_eq!(slider_value(px(200.0), bounds, 15, 1200, 15), 1200);
        assert_eq!(slider_value(px(60.0), bounds, 0, 100, 1), 50);
    }
}
