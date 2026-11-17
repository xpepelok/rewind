use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Context, CursorStyle, Entity, FocusHandle, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, ScrollEvent,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, img, prelude::*, px,
};

use rewind_media::{draw_text, render_marks};

use crate::assets::{Icon, icon};
use crate::platform::screen::{Area, Frozen};
use crate::state::Rewind;
use crate::theme::{CONTROL_TEXT, FONT_FAMILY, Palette, SMALL_TEXT, ThemeExt, alpha};
use crate::ui::Tooltip;
use crate::views::annotate::{
    self, Annotate, Board, Raster, TOOL_BUTTON, Tool, color_of, reframe, square_button,
};

const HANDLE: f32 = 10.0;
const GRAB: f32 = 12.0;
const LINE_GRAB: f32 = 6.0;
const MIN_SIDE: u32 = 4;
const BAR_GAP: f32 = 10.0;
const TOP_GAP: f32 = 16.0;
const MARGIN: f32 = 8.0;
const STRIP_PAD: f32 = 4.0;
const STRIP_GAP: f32 = 2.0;
const RULE: f32 = 9.0;
const SIZE_LABEL: f32 = 84.0;
const ACTIONS: usize = 6;
const ACTION_RULES: usize = 2;
const STRIP_EXTRAS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnipMode {
    Area,
    Window,
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnipAction {
    Copy,
    Save,
    Upload,
    Edit,
    Pin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Edge {
    const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    fn anchor(self, area: Area) -> (i32, i32) {
        let (left, top) = (area.x, area.y);
        let (right, bottom) = (area.x + area.width as i32, area.y + area.height as i32);
        let (middle_x, middle_y) = ((left + right) / 2, (top + bottom) / 2);
        match self {
            Self::Left => (left, middle_y),
            Self::Right => (right, middle_y),
            Self::Top => (middle_x, top),
            Self::Bottom => (middle_x, bottom),
            Self::TopLeft => (left, top),
            Self::TopRight => (right, top),
            Self::BottomLeft => (left, bottom),
            Self::BottomRight => (right, bottom),
        }
    }

    fn cursor(self) -> CursorStyle {
        match self {
            Self::Left | Self::Right => CursorStyle::ResizeLeftRight,
            Self::Top | Self::Bottom => CursorStyle::ResizeUpDown,
            Self::TopLeft | Self::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Self::TopRight | Self::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
    }
}

fn edge_near(area: Area, at: (i32, i32), grab: i32, line: i32) -> Option<Edge> {
    let handle = Edge::ALL.into_iter().find(|edge| {
        let (x, y) = edge.anchor(area);
        (x - at.0).abs() <= grab && (y - at.1).abs() <= grab
    });
    if handle.is_some() {
        return handle;
    }
    let (left, top) = (area.x, area.y);
    let (right, bottom) = (area.x + area.width as i32, area.y + area.height as i32);
    let along_x = at.0 >= left - line && at.0 <= right + line;
    let along_y = at.1 >= top - line && at.1 <= bottom + line;
    [
        (Edge::Left, (at.0 - left).abs(), along_y),
        (Edge::Right, (at.0 - right).abs(), along_y),
        (Edge::Top, (at.1 - top).abs(), along_x),
        (Edge::Bottom, (at.1 - bottom).abs(), along_x),
    ]
    .into_iter()
    .filter(|(_, distance, along)| *along && *distance <= line)
    .min_by_key(|(_, distance, _)| *distance)
    .map(|(edge, _, _)| edge)
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    Create((i32, i32)),
    Move((i32, i32), Area),
    Resize(Edge, Area),
}

pub fn span(from: (i32, i32), to: (i32, i32)) -> Area {
    Area {
        x: from.0.min(to.0),
        y: from.1.min(to.1),
        width: from.0.abs_diff(to.0),
        height: from.1.abs_diff(to.1),
    }
}

fn resize(start: Area, edge: Edge, to: (i32, i32)) -> Area {
    let (mut left, mut top) = (start.x, start.y);
    let (mut right, mut bottom) = (start.x + start.width as i32, start.y + start.height as i32);
    match edge {
        Edge::Left => left = to.0,
        Edge::Right => right = to.0,
        Edge::Top => top = to.1,
        Edge::Bottom => bottom = to.1,
        Edge::TopLeft => (left, top) = to,
        Edge::TopRight => (right, top) = to,
        Edge::BottomLeft => (left, bottom) = to,
        Edge::BottomRight => (right, bottom) = to,
    }
    span((left, top), (right, bottom))
}

fn shift(start: Area, grab: (i32, i32), at: (i32, i32), frame: (u32, u32)) -> Area {
    let x = (start.x + at.0 - grab.0).clamp(0, frame.0.saturating_sub(start.width) as i32);
    let y = (start.y + at.1 - grab.1).clamp(0, frame.1.saturating_sub(start.height) as i32);
    Area { x, y, ..start }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Rect {
    fn right(&self) -> f32 {
        self.x + self.width
    }

    fn bottom(&self) -> f32 {
        self.y + self.height
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

fn strip_length(buttons: usize, rules: usize) -> f32 {
    let items = buttons + rules;
    2.0 * STRIP_PAD
        + buttons as f32 * TOOL_BUTTON
        + rules as f32 * RULE
        + items.saturating_sub(1) as f32 * STRIP_GAP
}

fn fit(value: f32, length: f32, limit: f32) -> f32 {
    value.min(limit - length - MARGIN).max(MARGIN)
}

fn beside(selection: Rect, size: (f32, f32), viewport: (f32, f32)) -> Rect {
    let right = selection.right() + BAR_GAP;
    let left = selection.x - BAR_GAP - size.0;
    let x = if right + size.0 <= viewport.0 - MARGIN {
        right
    } else if left >= MARGIN {
        left
    } else {
        selection.right() - BAR_GAP - size.0
    };
    Rect {
        x: fit(x, size.0, viewport.0),
        y: fit(selection.y, size.1, viewport.1),
        width: size.0,
        height: size.1,
    }
}

fn under(selection: Rect, size: (f32, f32), viewport: (f32, f32), avoid: Rect) -> Rect {
    let below = selection.bottom() + BAR_GAP;
    let above = selection.y - BAR_GAP - size.1;
    let y = if below + size.1 <= viewport.1 - MARGIN {
        below
    } else if above >= MARGIN {
        above
    } else {
        selection.bottom() - BAR_GAP - size.1
    };
    let mut bar = Rect {
        x: fit(selection.x, size.0, viewport.0),
        y: fit(y, size.1, viewport.1),
        width: size.0,
        height: size.1,
    };
    if !bar.overlaps(&avoid) {
        return bar;
    }
    let left = avoid.x - BAR_GAP - size.0;
    let right = avoid.right() + BAR_GAP;
    let fits_left = left >= MARGIN;
    let fits_right = right + size.0 <= viewport.0 - MARGIN;
    let strip_on_right = avoid.x >= selection.x + selection.width / 2.0;
    if fits_left && (strip_on_right || !fits_right) {
        bar.x = left;
    } else if fits_right {
        bar.x = right;
    } else {
        let after = avoid.bottom() + BAR_GAP;
        bar.y = if after + size.1 <= viewport.1 - MARGIN {
            after
        } else {
            fit(avoid.y - BAR_GAP - size.1, size.1, viewport.1)
        };
    }
    bar
}

fn layout(selection: Rect, viewport: (f32, f32), tools: usize) -> (Rect, Rect) {
    let strip_size = (
        2.0 * STRIP_PAD + TOOL_BUTTON,
        strip_length(tools + STRIP_EXTRAS, 1),
    );
    let strip = beside(selection, strip_size, viewport);
    let bar_size = (
        strip_length(ACTIONS, ACTION_RULES) + SIZE_LABEL + STRIP_GAP,
        2.0 * STRIP_PAD + TOOL_BUTTON,
    );
    let bar = under(selection, bar_size, viewport, strip);
    (strip, bar)
}

pub fn snip_options(bounds: Bounds<Pixels>) -> WindowOptions {
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

pub struct SnipSession {
    pub mode: SnipMode,
    pub owner: Option<usize>,
    pub windows: Vec<gpui::AnyWindowHandle>,
    pub copy_on_select: bool,
    pub host: &'static str,
}

pub struct Snip {
    state: Entity<Rewind>,
    session: Entity<SnipSession>,
    index: usize,
    primary: bool,
    inside: bool,
    _watch: gpui::Subscription,
    frozen: Arc<Frozen>,
    image: Option<Arc<gpui::RenderImage>>,
    focus: FocusHandle,
    mode: SnipMode,
    selection: Option<Area>,
    drag: Option<Drag>,
    hover: Option<Area>,
    copy_on_select: bool,
    host: &'static str,
    board: Board,
}

impl Annotate for Snip {
    fn board(&mut self) -> &mut Board {
        &mut self.board
    }
}

impl Snip {
    pub fn new(
        state: Entity<Rewind>,
        session: Entity<SnipSession>,
        frozen: Arc<Frozen>,
        index: usize,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let monitor = frozen.monitor;
        cx.defer_in(window, move |_, window, _| {
            crate::platform::window::cover(window, monitor);
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let (copy_on_select, host) = {
            let shared = session.read(cx);
            (shared.copy_on_select, shared.host)
        };
        let watch = cx.observe(&session, |snip, session, cx| {
            let shared = session.read(cx);
            snip.mode = shared.mode;
            if shared.owner != Some(snip.index) {
                snip.selection = None;
                snip.drag = None;
                snip.board.discard_typing();
                snip.board.reset();
            }
            if snip.mode != SnipMode::Window {
                snip.hover = None;
            }
            cx.notify();
        });
        let mut board = Board::new(focus.clone(), false, window, cx);
        board.tool = Tool::Select;
        let pixels: Arc<dyn Raster> = frozen.clone();
        board.set_source(pixels, frozen.width, frozen.height, window, cx);
        let primary = frozen.primary;
        let image =
            crate::views::annotate::render_image(frozen.width, frozen.height, frozen.bgra.clone());
        Self {
            state,
            session,
            index,
            primary,
            inside: false,
            _watch: watch,
            frozen,
            image,
            focus,
            mode: SnipMode::Area,
            selection: None,
            drag: None,
            hover: None,
            copy_on_select,
            host,
            board,
        }
    }

    fn scale(&self, window: &gpui::Window) -> f32 {
        let width = f32::from(window.viewport_size().width).max(1.0);
        self.frozen.width as f32 / width
    }

    fn to_image(&self, point: Point<Pixels>, window: &gpui::Window) -> (i32, i32) {
        let scale = self.scale(window);
        (
            (f32::from(point.x) * scale).round() as i32,
            (f32::from(point.y) * scale).round() as i32,
        )
    }

    fn full(&self) -> Area {
        Area {
            x: 0,
            y: 0,
            width: self.frozen.width,
            height: self.frozen.height,
        }
    }

    fn set_mode(&mut self, mode: SnipMode, _: &mut gpui::Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.mode = mode;
            session.owner = None;
            cx.notify();
        });
    }

    fn claim(&mut self, mode: SnipMode, cx: &mut Context<Self>) {
        let index = self.index;
        self.mode = mode;
        self.session.update(cx, |session, cx| {
            session.owner = Some(index);
            session.mode = mode;
            cx.notify();
        });
    }

    fn edge_at(&self, at: (i32, i32), window: &gpui::Window) -> Option<Edge> {
        let area = self.selection?;
        let scale = self.scale(window);
        edge_near(area, at, (GRAB * scale) as i32, (LINE_GRAB * scale) as i32)
    }

    fn annotating(&self) -> bool {
        self.selection.is_some() && self.drag.is_none()
    }

    fn draws_at(&self, position: Point<Pixels>) -> bool {
        self.board.picking || self.board.tool != Tool::Select || self.board.hits(position)
    }

    fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.board.palette = None;
        let at = self.to_image(event.position, window);
        match self.mode {
            SnipMode::Window => {
                if let Some(area) = self.hover {
                    self.selection = Some(area);
                    self.claim(SnipMode::Area, cx);
                    if self.copy_on_select {
                        self.finish(SnipAction::Copy, window, cx);
                        return;
                    }
                }
            }
            SnipMode::Screen if self.selection.is_none() => {
                self.selection = Some(self.full());
                self.claim(SnipMode::Area, cx);
                if self.copy_on_select {
                    self.finish(SnipAction::Copy, window, cx);
                    return;
                }
            }
            SnipMode::Screen | SnipMode::Area => {
                let edge = self.edge_at(at, window);
                if edge.is_none() && self.annotating() && self.draws_at(event.position) {
                    self.board
                        .press(event.position, event.click_count, window, cx);
                    cx.notify();
                    return;
                }
                self.board.commit_text(window, cx);
                self.board.deselect();
                self.drag = Some(match (edge, self.selection) {
                    (Some(edge), Some(area)) => Drag::Resize(edge, area),
                    (None, Some(area)) if area.contains(at.0, at.1) => Drag::Move(at, area),
                    _ => {
                        self.selection = None;
                        self.claim(SnipMode::Area, cx);
                        Drag::Create(at)
                    }
                });
            }
        }
        cx.notify();
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.board.dragging() {
            self.board.drag_to(event.position, event.modifiers, cx);
            return;
        }
        let at = self.to_image(event.position, window);
        match self.drag {
            Some(Drag::Create(origin)) => self.selection = Some(span(origin, at)),
            Some(Drag::Move(grab, start)) => {
                self.selection = Some(shift(
                    start,
                    grab,
                    at,
                    (self.frozen.width, self.frozen.height),
                ))
            }
            Some(Drag::Resize(edge, start)) => self.selection = Some(resize(start, edge, at)),
            None if self.mode == SnipMode::Window => {
                let hover = self
                    .frozen
                    .windows
                    .iter()
                    .copied()
                    .find(|area| area.contains(at.0, at.1));
                if hover == self.hover {
                    return;
                }
                self.hover = hover;
            }
            None => return,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.board.dragging() {
            self.board.release(cx);
            return;
        }
        let Some(drag) = self.drag.take() else {
            return;
        };
        let small = self
            .selection
            .is_none_or(|area| area.width < MIN_SIDE || area.height < MIN_SIDE);
        if small {
            self.selection = None;
        } else if matches!(drag, Drag::Create(_)) && self.copy_on_select {
            self.finish(SnipAction::Copy, window, cx);
            return;
        }
        cx.notify();
    }

    fn right_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if !self.annotating() {
            return;
        }
        self.board.commit_text(window, cx);
        self.board.palette = Some(event.position);
        cx.stop_propagation();
        cx.notify();
    }

    fn wheel(&mut self, event: &ScrollEvent, _: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.annotating() {
            self.board.wheel(event, cx);
        }
    }

    fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let control = event.keystroke.modifiers.control;
        let shift = event.keystroke.modifiers.shift;
        let key = event.keystroke.key.as_str();
        if self.board.typing() {
            self.board.typing_key(key, control, window, cx);
            return;
        }
        match key {
            "escape" if self.board.palette.is_some() => {
                self.board.palette = None;
                cx.notify();
            }
            "escape" if self.board.picking => {
                self.board.picking = false;
                cx.notify();
            }
            "escape" => self.cancel(window, cx),
            "enter" => self.finish(SnipAction::Copy, window, cx),
            "c" if control => self.finish(SnipAction::Copy, window, cx),
            "s" if control => self.finish(SnipAction::Save, window, cx),
            "p" if control => self.finish(SnipAction::Pin, window, cx),
            _ => {
                if self.annotating() && self.board.edit_key(key, control, shift, window, cx) {
                    cx.notify();
                    cx.stop_propagation();
                }
            }
        }
    }

    fn close_all(&mut self, cx: &mut Context<Self>) {
        let windows = self.session.read(cx).windows.clone();
        self.state.update(cx, |rewind, _| rewind.snipping = false);
        cx.defer(move |cx| {
            for window in windows {
                window
                    .update(cx, |view, window, cx| {
                        if let Ok(snip) = view.downcast::<Snip>() {
                            snip.update(cx, |snip, cx| snip.release(window, cx));
                        }
                        window.remove_window();
                    })
                    .ok();
            }
        });
    }

    fn release(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.board.drop_base(window, cx);
        if let Some(image) = self.image.take() {
            cx.drop_image(image, Some(window));
        }
    }

    fn cancel(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.board.drop_base(window, cx);
        self.close_all(cx);
    }

    fn finish(&mut self, action: SnipAction, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let frame = (self.frozen.width, self.frozen.height);
        let Some(area) = self
            .selection
            .and_then(|area| area.clamp_to(frame.0, frame.1))
        else {
            return;
        };
        self.board.commit_text(window, cx);
        let Some(crop) = self.frozen.crop(area) else {
            return;
        };
        let marks = reframe(self.board.marks(), frame, area);
        self.board.drop_base(window, cx);
        let at = (
            self.frozen.monitor.left + area.x,
            self.frozen.monitor.top + area.y,
        );
        let pinned = action == SnipAction::Pin;
        let scale = self.scale(window);
        if marks.is_empty() && pinned {
            let (width, height, pixels) = crop;
            cx.defer(move |cx| crate::views::pin::open(width, height, pixels, at, scale, cx));
        } else if marks.is_empty() {
            self.state
                .update(cx, |rewind, cx| rewind.keep_snip(crop, action, cx));
        } else {
            let state = self.state.clone();
            cx.spawn(async move |_, cx| {
                let (width, height, pixels) = crop;
                let (pixels, width, height) =
                    cx.background_executor()
                        .spawn(async move {
                            render_marks(pixels, width, height, &marks, None, &draw_text)
                        })
                        .await;
                if pinned {
                    cx.update(|cx| crate::views::pin::open(width, height, pixels, at, scale, cx));
                    return;
                }
                state.update(cx, |rewind, cx| {
                    rewind.keep_snip((width, height, pixels), action, cx)
                });
            })
            .detach();
        }
        self.close_all(cx);
    }

    fn to_screen(&self, area: Area, window: &gpui::Window) -> Bounds<Pixels> {
        let scale = self.scale(window);
        Bounds::new(
            gpui::point(px(area.x as f32 / scale), px(area.y as f32 / scale)),
            gpui::size(
                px(area.width as f32 / scale),
                px(area.height as f32 / scale),
            ),
        )
    }

    fn mode_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let item = |id: &'static str, glyph: Icon, title: &'static str, mode: Option<SnipMode>| {
            let active = mode.is_some_and(|mode| mode == self.mode);
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(32.0))
                .px(px(12.0))
                .cursor_pointer()
                .when(active, |item| item.bg(p.accent_dim))
                .hover(|style| style.bg(p.hover))
                .child(icon(glyph, 18.0, if active { p.accent } else { p.text }))
                .when(!title.is_empty(), |item| {
                    item.child(div().text_color(p.text).child(title))
                })
        };
        div()
            .absolute()
            .top(px(TOP_GAP))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .p(px(4.0))
                    .bg(p.panel)
                    .shadow(p.toast_shadow())
                    .text_size(px(SMALL_TEXT + 1.0))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        item("snip-area", Icon::ToolRect, "Область", Some(SnipMode::Area))
                            .on_click(cx.listener(|snip, _, window, cx| {
                                snip.set_mode(SnipMode::Area, window, cx)
                            })),
                    )
                    .child(
                        item(
                            "snip-window",
                            Icon::ToolFrame,
                            "Окно",
                            Some(SnipMode::Window),
                        )
                        .on_click(cx.listener(|snip, _, window, cx| {
                            snip.set_mode(SnipMode::Window, window, cx)
                        })),
                    )
                    .child(
                        item(
                            "snip-screen",
                            Icon::Monitor,
                            "Весь экран",
                            Some(SnipMode::Screen),
                        )
                        .on_click(cx.listener(|snip, _, window, cx| {
                            snip.set_mode(SnipMode::Screen, window, cx)
                        })),
                    )
                    .child(div().w(px(1.0)).h(px(20.0)).mx(px(4.0)).bg(p.divider))
                    .child(
                        item("snip-close", Icon::Close, "", None)
                            .on_click(cx.listener(|snip, _, window, cx| snip.cancel(window, cx))),
                    ),
            )
            .into_any_element()
    }

    fn panel(rect: Rect, p: &Palette) -> gpui::Div {
        div()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .flex()
            .items_center()
            .gap(px(STRIP_GAP))
            .p(px(STRIP_PAD))
            .bg(p.panel)
            .shadow(p.toast_shadow())
            .text_size(px(SMALL_TEXT + 1.0))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
    }

    fn tool_strip(&self, rect: Rect, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let tools = annotate::tool_buttons(&self.board, cx);
        let undo = annotate::undo_button(&self.board, cx);
        let redo = annotate::redo_button(&self.board, cx);
        let popover = gpui::point(px(rect.right() + BAR_GAP), px(rect.y));
        let colour = div()
            .id("snip-style")
            .flex_none()
            .size(px(TOOL_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .cursor_pointer()
            .when(self.board.palette.is_some(), |item| {
                item.bg(alpha(p.accent, 0.22))
            })
            .hover(|style| style.bg(p.hover))
            .child(
                div()
                    .size(px(18.0))
                    .rounded_full()
                    .bg(color_of(self.board.color()))
                    .border_2()
                    .border_color(p.divider),
            )
            .tooltip(Tooltip::text("Цвет и толщина (правая кнопка мыши)"))
            .on_click(cx.listener(move |snip, _, window, cx| {
                snip.board.commit_text(window, cx);
                snip.board.palette = Some(popover);
                cx.notify();
            }));
        Self::panel(rect, &p)
            .flex_col()
            .children(tools)
            .child(
                div()
                    .flex_none()
                    .w(px(TOOL_BUTTON))
                    .h(px(1.0))
                    .my(px(4.0))
                    .bg(p.divider),
            )
            .child(colour)
            .child(undo)
            .child(redo)
            .into_any_element()
    }

    fn action_bar(&self, rect: Rect, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let action = |id: &'static str, glyph: Icon, which: SnipAction| {
            square_button(id, glyph, false, &p)
                .on_click(cx.listener(move |snip, _, window, cx| snip.finish(which, window, cx)))
        };
        let rule = || {
            div()
                .flex_none()
                .w(px(1.0))
                .h(px(22.0))
                .mx(px(4.0))
                .bg(p.divider)
        };
        let size = self
            .selection
            .map(|area| format!("{} × {}", area.width, area.height))
            .unwrap_or_default();
        let upload = format!("Загрузить в {}", self.host);
        Self::panel(rect, &p)
            .child(
                div()
                    .flex_none()
                    .w(px(SIZE_LABEL))
                    .text_center()
                    .text_size(px(CONTROL_TEXT))
                    .text_color(p.muted)
                    .child(size),
            )
            .child(rule())
            .child(
                action("snip-copy", Icon::Copy, SnipAction::Copy)
                    .tooltip(Tooltip::text("Копировать (Enter, Ctrl+C)")),
            )
            .child(
                action("snip-save", Icon::Save, SnipAction::Save)
                    .tooltip(Tooltip::text("Сохранить (Ctrl+S)")),
            )
            .child(
                action("snip-upload", Icon::Upload, SnipAction::Upload)
                    .tooltip(Tooltip::owned(upload.into())),
            )
            .child(
                action("snip-edit", Icon::Edit, SnipAction::Edit)
                    .tooltip(Tooltip::text("Открыть в редакторе")),
            )
            .child(
                action("snip-pin", Icon::Pin, SnipAction::Pin)
                    .tooltip(Tooltip::text("Закрепить на экране (Ctrl+P)")),
            )
            .child(rule())
            .child(
                square_button("snip-cancel", Icon::Close, false, &p)
                    .tooltip(Tooltip::text("Отмена (Esc)"))
                    .on_click(cx.listener(|snip, _, window, cx| snip.cancel(window, cx))),
            )
            .into_any_element()
    }

    fn style_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.board.palette?;
        let p = cx.palette();
        let mut extra = vec![
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(SMALL_TEXT))
                        .text_color(p.text)
                        .child("Толщина"),
                )
                .children(annotate::size_controls(&self.board, cx))
                .into_any_element(),
        ];
        if let Some(hint) = annotate::counter_hint(&self.board, &p) {
            extra.push(hint.into_any_element());
        }
        if self.board.tool == Tool::Text || self.board.typing() {
            extra.push(annotate::text_controls(&self.board, cx));
        }
        annotate::palette_popover(&self.board, "snip-palette", extra, cx)
    }
}

impl Render for Snip {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.board.refresh_base(window, cx);
        let p = cx.palette();
        let shade = alpha(gpui::black(), 0.45);
        let viewport = window.viewport_size();
        let whole = (self.mode == SnipMode::Screen && self.inside).then(|| self.full());
        let marked = self.selection.or(self.hover).or(whole);
        let shown = marked.map(|area| self.to_screen(area, window));
        let mut layers: Vec<AnyElement> = vec![annotate::overlay(&self.board, false, p, cx)];
        layers.extend(annotate::texts(&self.board));
        match shown {
            Some(area) => {
                let right = area.origin.x + area.size.width;
                let bottom = area.origin.y + area.size.height;
                for (left, top, width, height) in [
                    (px(0.0), px(0.0), viewport.width, area.origin.y),
                    (
                        px(0.0),
                        bottom,
                        viewport.width,
                        (viewport.height - bottom).max(px(0.0)),
                    ),
                    (px(0.0), area.origin.y, area.origin.x, area.size.height),
                    (
                        right,
                        area.origin.y,
                        (viewport.width - right).max(px(0.0)),
                        area.size.height,
                    ),
                ] {
                    layers.push(
                        div()
                            .absolute()
                            .left(left)
                            .top(top)
                            .w(width)
                            .h(height)
                            .bg(shade)
                            .into_any_element(),
                    );
                }
                layers.push(
                    div()
                        .absolute()
                        .left(area.origin.x)
                        .top(area.origin.y)
                        .w(area.size.width)
                        .h(area.size.height)
                        .border_2()
                        .border_color(p.accent)
                        .into_any_element(),
                );
                if let Some(selection) = self.selection {
                    let scale = self.scale(window);
                    let (left, top) = (f32::from(area.origin.x), f32::from(area.origin.y));
                    let (wide, tall) = (f32::from(area.size.width), f32::from(area.size.height));
                    for (edge, x, y, width, height) in [
                        (Edge::Left, left, top, 0.0, tall),
                        (Edge::Right, left + wide, top, 0.0, tall),
                        (Edge::Top, left, top, wide, 0.0),
                        (Edge::Bottom, left, top + tall, wide, 0.0),
                    ] {
                        layers.push(
                            div()
                                .absolute()
                                .left(px(x - LINE_GRAB))
                                .top(px(y - LINE_GRAB))
                                .w(px(width + LINE_GRAB * 2.0))
                                .h(px(height + LINE_GRAB * 2.0))
                                .cursor(edge.cursor())
                                .into_any_element(),
                        );
                    }
                    for edge in Edge::ALL {
                        let (x, y) = edge.anchor(selection);
                        layers.push(
                            div()
                                .absolute()
                                .left(px(x as f32 / scale - HANDLE / 2.0))
                                .top(px(y as f32 / scale - HANDLE / 2.0))
                                .size(px(HANDLE))
                                .bg(p.accent)
                                .border_1()
                                .border_color(p.title)
                                .into_any_element(),
                        );
                        layers.push(
                            div()
                                .absolute()
                                .left(px(x as f32 / scale - GRAB))
                                .top(px(y as f32 / scale - GRAB))
                                .size(px(GRAB * 2.0))
                                .cursor(edge.cursor())
                                .into_any_element(),
                        );
                    }
                    layers.extend(annotate::typing_field(&self.board, &p));
                    if self.drag.is_none() && !self.board.dragging() {
                        let (strip, bar) = layout(
                            Rect {
                                x: f32::from(area.origin.x),
                                y: f32::from(area.origin.y),
                                width: f32::from(area.size.width),
                                height: f32::from(area.size.height),
                            },
                            (f32::from(viewport.width), f32::from(viewport.height)),
                            self.board.tools().count(),
                        );
                        layers.push(self.tool_strip(strip, cx));
                        layers.push(self.action_bar(bar, cx));
                        layers.extend(self.style_popover(cx));
                    }
                }
            }
            None => layers.push(div().absolute().inset_0().bg(shade).into_any_element()),
        }
        let cursor = if let Some(Drag::Resize(edge, _)) = self.drag {
            edge.cursor()
        } else if self.annotating() && !self.board.picking && self.board.tool == Tool::Text {
            CursorStyle::IBeam
        } else {
            CursorStyle::Crosshair
        };
        let backdrop = self
            .board
            .backdrop()
            .or_else(|| self.image.clone())
            .map(img);
        div()
            .id("snip")
            .size_full()
            .relative()
            .font_family(FONT_FAMILY)
            .bg(p.stage)
            .cursor(cursor)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::right_down))
            .on_mouse_down(
                MouseButton::Navigate(gpui::NavigationDirection::Back),
                cx.listener(|view, _, _, cx| view.board.undo(cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(gpui::NavigationDirection::Forward),
                cx.listener(|view, _, _, cx| view.board.redo(cx)),
            )
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::wheel))
            .children(backdrop.map(|backdrop| {
                backdrop
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Fill)
            }))
            .children(layers)
            .on_hover(cx.listener(|snip, inside: &bool, _, cx| {
                snip.inside = *inside;
                cx.notify();
            }))
            .when(self.primary && !self.board.dragging(), |root| {
                root.child(self.mode_bar(cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use rewind_media::{Mark, Shape};

    use super::*;

    #[test]
    fn dragging_backwards_still_makes_a_positive_area() {
        assert_eq!(
            span((50, 40), (10, 20)),
            Area {
                x: 10,
                y: 20,
                width: 40,
                height: 20
            }
        );
    }

    #[test]
    fn pulling_an_edge_past_the_other_flips_cleanly() {
        let start = Area {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        assert_eq!(
            resize(start, Edge::Right, (5, 15)),
            Area {
                x: 5,
                y: 10,
                width: 5,
                height: 20
            }
        );
        assert_eq!(
            resize(start, Edge::BottomRight, (40, 50)),
            Area {
                x: 10,
                y: 10,
                width: 30,
                height: 40
            }
        );
        assert_eq!(Edge::TopLeft.anchor(start), (10, 10));
    }

    #[test]
    fn the_whole_frame_line_grabs_not_only_the_handles() {
        let area = Area {
            x: 100,
            y: 100,
            width: 400,
            height: 200,
        };
        assert_eq!(edge_near(area, (103, 130), 12, 6), Some(Edge::Left));
        assert_eq!(edge_near(area, (497, 270), 12, 6), Some(Edge::Right));
        assert_eq!(edge_near(area, (180, 96), 12, 6), Some(Edge::Top));
        assert_eq!(edge_near(area, (420, 305), 12, 6), Some(Edge::Bottom));
        assert_eq!(edge_near(area, (108, 108), 12, 6), Some(Edge::TopLeft));
        assert_eq!(edge_near(area, (300, 108), 12, 6), Some(Edge::Top));
        assert_eq!(edge_near(area, (300, 200), 12, 6), None);
        assert_eq!(edge_near(area, (90, 130), 12, 6), None);
        assert_eq!(edge_near(area, (103, 400), 12, 6), None);
    }

    fn inside(rect: Rect, viewport: (f32, f32)) -> bool {
        rect.x >= 0.0 && rect.y >= 0.0 && rect.right() <= viewport.0 && rect.bottom() <= viewport.1
    }

    #[test]
    fn the_strips_never_leave_the_screen() {
        let tools = 13;
        for viewport in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1440.0)] {
            for step_x in 0..=10 {
                for step_y in 0..=10 {
                    for (width, height) in [(4.0, 4.0), (120.0, 80.0), (600.0, 400.0)] {
                        let selection = Rect {
                            x: (viewport.0 - width) * step_x as f32 / 10.0,
                            y: (viewport.1 - height) * step_y as f32 / 10.0,
                            width,
                            height,
                        };
                        let (strip, bar) = layout(selection, viewport, tools);
                        assert!(inside(strip, viewport), "{selection:?} {strip:?}");
                        assert!(inside(bar, viewport), "{selection:?} {bar:?}");
                        assert!(!strip.overlaps(&bar), "{selection:?} {strip:?} {bar:?}");
                    }
                }
            }
            let full = Rect {
                x: 0.0,
                y: 0.0,
                width: viewport.0,
                height: viewport.1,
            };
            let (strip, bar) = layout(full, viewport, tools);
            assert!(inside(strip, viewport) && inside(bar, viewport));
            assert!(!strip.overlaps(&bar));
        }
    }

    #[test]
    fn the_strips_hug_the_selection_and_flip_when_there_is_no_room() {
        let viewport = (1920.0, 1080.0);
        let middle = Rect {
            x: 600.0,
            y: 200.0,
            width: 400.0,
            height: 300.0,
        };
        let (strip, bar) = layout(middle, viewport, 13);
        assert_eq!(strip.x, middle.right() + BAR_GAP);
        assert_eq!(strip.y, middle.y);
        assert_eq!(bar.y, middle.bottom() + BAR_GAP);
        assert_eq!(bar.x, middle.x);
        let corner = Rect {
            x: 1500.0,
            y: 900.0,
            width: 420.0,
            height: 180.0,
        };
        let (strip, bar) = layout(corner, viewport, 13);
        assert_eq!(strip.right(), corner.x - BAR_GAP);
        assert_eq!(bar.bottom(), corner.y - BAR_GAP);
    }

    #[test]
    fn moving_the_selection_keeps_marks_on_the_image() {
        let frame = (1000, 500);
        let marks = vec![Mark::Stroke {
            shape: Shape::Line,
            points: vec![(0.5, 0.6), (0.6, 0.6)],
            color: [255, 0, 0],
            width: 0.004,
        }];
        let start = Area {
            x: 400,
            y: 200,
            width: 200,
            height: 200,
        };
        let moved = shift(start, (450, 250), (500, 300), frame);
        assert_eq!(
            moved,
            Area {
                x: 450,
                y: 250,
                width: 200,
                height: 200
            }
        );
        let clamped = shift(start, (450, 250), (5000, 5000), frame);
        assert_eq!((clamped.x, clamped.y), (800, 300));
        let point = |area: Area| match &reframe(&marks, frame, area)[0] {
            Mark::Stroke { points, .. } => (
                points[0].0 * area.width as f32 + area.x as f32,
                points[0].1 * area.height as f32 + area.y as f32,
            ),
            _ => (f32::NAN, f32::NAN),
        };
        let (before, after) = (point(start), point(moved));
        assert!((before.0 - 500.0).abs() < 1e-3 && (before.1 - 300.0).abs() < 1e-3);
        assert!((after.0 - before.0).abs() < 1e-3 && (after.1 - before.1).abs() < 1e-3);
        assert_eq!(
            marks[0],
            Mark::Stroke {
                shape: Shape::Line,
                points: vec![(0.5, 0.6), (0.6, 0.6)],
                color: [255, 0, 0],
                width: 0.004,
            }
        );
    }
}
