mod paint;
pub(crate) mod widgets;

use std::sync::Arc;

use gpui::{
    Bounds, Context, CursorStyle, Entity, FocusHandle, Focusable, Hsla, Modifiers, Pixels, Point,
    RenderImage, ScrollWheelEvent, Subscription, Window, point, prelude::*, px, size,
};
use image::{Frame as ImageFrame, RgbaImage};
use smallvec::smallvec;

use rewind_media::{Area, Mark, Shape, TextStyle, draw_text, render_marks};

use crate::assets::Icon;
use crate::platform::screen;
use crate::ui::input::{InputEvent, TextInput};

pub use paint::{crop_handles, overlay, texts, typing_field};
pub use widgets::{
    counter_hint, delete_button, eyedropper, palette_popover, redo_button, separator,
    size_controls, square_button, swatches, text_controls, tool_buttons, undo_button,
};

pub const PALETTE: [[u8; 3]; 12] = [
    [0x80, 0x00, 0x00],
    [0xff, 0x00, 0x00],
    [0xff, 0xff, 0x00],
    [0x00, 0xff, 0x00],
    [0x00, 0x80, 0x00],
    [0x00, 0xff, 0xff],
    [0x00, 0x00, 0xff],
    [0xff, 0x00, 0xff],
    [0x80, 0x00, 0x80],
    [0xff, 0xff, 0xff],
    [0x80, 0x80, 0x80],
    [0x00, 0x00, 0x00],
];
const DEFAULT_COLOR: [u8; 3] = [0xff, 0x00, 0x00];
pub const FONTS: [&str; 7] = [
    "Segoe UI",
    "Arial",
    "Calibri",
    "Consolas",
    "Times New Roman",
    "Comic Sans MS",
    "Impact",
];
const MIN_SIZE: u32 = 1;
const MAX_SIZE: u32 = 50;
const DEFAULT_SIZE: u32 = 3;
const MAX_COUNTER: u32 = 999;
const MARKER_EXTRA: f32 = 14.0;
const COUNTER_EXTRA: f32 = 15.0;
const TEXT_BASE: f32 = 12.0;
const HISTORY: usize = 100;
const POINT_STEP: f32 = 0.0015;
const MIN_SHAPE: f32 = 0.004;
const MIN_EFFECT: f32 = 0.005;
const HIT_SLOP: f32 = 0.008;
pub const TOOL_BUTTON: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Crop,
    Pen,
    Line,
    Arrow,
    Frame,
    Rect,
    Circle,
    Marker,
    Text,
    Pixelate,
    BlurBrush,
    Invert,
    Counter,
}

pub const TOOLS: [(Tool, Icon, &str, &str); 14] = [
    (
        Tool::Select,
        Icon::ToolSelect,
        "v",
        "Выбор и перемещение (V)",
    ),
    (Tool::Crop, Icon::ToolCrop, "x", "Обрезка (X)"),
    (Tool::Pen, Icon::ToolPen, "p", "Карандаш (P)"),
    (Tool::Line, Icon::ToolLine, "d", "Линия (D)"),
    (Tool::Arrow, Icon::ToolArrow, "a", "Стрелка (A)"),
    (Tool::Frame, Icon::ToolFrame, "s", "Рамка (S)"),
    (Tool::Rect, Icon::ToolRect, "r", "Прямоугольник (R)"),
    (Tool::Circle, Icon::ToolCircle, "c", "Эллипс (C)"),
    (Tool::Marker, Icon::ToolMarker, "m", "Маркер (M)"),
    (Tool::Text, Icon::ToolText, "t", "Текст (T)"),
    (
        Tool::Pixelate,
        Icon::ToolPixelate,
        "b",
        "Пикселизация, размер 1 — размытие (B)",
    ),
    (
        Tool::BlurBrush,
        Icon::ToolBlurBrush,
        "u",
        "Кисть размытия (U)",
    ),
    (Tool::Invert, Icon::ToolInvert, "i", "Инверсия (I)"),
    (Tool::Counter, Icon::ToolCounter, "n", "Счётчик (N)"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constraint {
    Free,
    Angle,
    Square,
}

impl Tool {
    const fn shape(self) -> Option<Shape> {
        match self {
            Self::Pen => Some(Shape::Pen),
            Self::Marker => Some(Shape::Marker),
            Self::Line => Some(Shape::Line),
            Self::Arrow => Some(Shape::Arrow),
            Self::Frame => Some(Shape::Frame),
            Self::Circle => Some(Shape::Ellipse),
            _ => None,
        }
    }

    const fn constraint(self) -> Constraint {
        match self {
            Self::Line | Self::Arrow | Self::Marker => Constraint::Angle,
            Self::Frame
            | Self::Rect
            | Self::Circle
            | Self::Crop
            | Self::Pixelate
            | Self::Invert => Constraint::Square,
            _ => Constraint::Free,
        }
    }

    fn pixels(self, size: u32) -> f32 {
        let size = size as f32;
        match self {
            Self::Marker => size + MARKER_EXTRA,
            Self::Counter => size + COUNTER_EXTRA,
            Self::Text => size * 2.0 + TEXT_BASE,
            Self::Pixelate => 2.0 * (size + 1.0),
            Self::BlurBrush => 2.0 * size + MARKER_EXTRA,
            _ => size,
        }
    }
}

const CROP_HANDLE: f32 = 10.0;
const CROP_GRAB: f32 = 12.0;
const CROP_LINE_GRAB: f32 = 6.0;
const MIN_CROP: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
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
    pub(crate) const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    pub(crate) fn anchor(self, area: Area) -> (f32, f32) {
        let (left, top) = (area.x, area.y);
        let (right, bottom) = (area.right(), area.bottom());
        let (mid_x, mid_y) = ((left + right) / 2.0, (top + bottom) / 2.0);
        match self {
            Self::Left => (left, mid_y),
            Self::Right => (right, mid_y),
            Self::Top => (mid_x, top),
            Self::Bottom => (mid_x, bottom),
            Self::TopLeft => (left, top),
            Self::TopRight => (right, top),
            Self::BottomLeft => (left, bottom),
            Self::BottomRight => (right, bottom),
        }
    }

    pub(crate) fn cursor(self) -> CursorStyle {
        match self {
            Self::Left | Self::Right => CursorStyle::ResizeLeftRight,
            Self::Top | Self::Bottom => CursorStyle::ResizeUpDown,
            Self::TopLeft | Self::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Self::TopRight | Self::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
    }
}

fn crop_edge_near(
    area: Area,
    unit: (f32, f32),
    grab: (f32, f32),
    line: (f32, f32),
) -> Option<Edge> {
    let handle = Edge::ALL.into_iter().find(|edge| {
        let (x, y) = edge.anchor(area);
        (x - unit.0).abs() <= grab.0 && (y - unit.1).abs() <= grab.1
    });
    if handle.is_some() {
        return handle;
    }
    let (left, top, right, bottom) = (area.x, area.y, area.right(), area.bottom());
    let along_x = unit.0 >= left - line.0 && unit.0 <= right + line.0;
    let along_y = unit.1 >= top - line.1 && unit.1 <= bottom + line.1;
    [
        (Edge::Left, (unit.0 - left).abs() / line.0, along_y),
        (Edge::Right, (unit.0 - right).abs() / line.0, along_y),
        (Edge::Top, (unit.1 - top).abs() / line.1, along_x),
        (Edge::Bottom, (unit.1 - bottom).abs() / line.1, along_x),
    ]
    .into_iter()
    .filter(|(_, distance, along)| *along && *distance <= 1.0)
    .min_by(|a, b| a.1.total_cmp(&b.1))
    .map(|(edge, _, _)| edge)
}

fn resize_crop(start: Area, edge: Edge, to: (f32, f32)) -> Area {
    let (mut left, mut top) = (start.x, start.y);
    let (mut right, mut bottom) = (start.right(), start.bottom());
    let (x, y) = (to.0.clamp(0.0, 1.0), to.1.clamp(0.0, 1.0));
    match edge {
        Edge::Left => left = x,
        Edge::Right => right = x,
        Edge::Top => top = y,
        Edge::Bottom => bottom = y,
        Edge::TopLeft => {
            left = x;
            top = y;
        }
        Edge::TopRight => {
            right = x;
            top = y;
        }
        Edge::BottomLeft => {
            left = x;
            bottom = y;
        }
        Edge::BottomRight => {
            right = x;
            bottom = y;
        }
    }
    Area::spanning((left, top), (right, bottom))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    Stroke {
        index: usize,
        anchor: (f32, f32),
        constraint: Constraint,
    },
    Area {
        index: usize,
        anchor: (f32, f32),
    },
    Crop {
        anchor: (f32, f32),
    },
    CropResize {
        edge: Edge,
        start: Area,
    },
    Tail(usize),
    Move {
        index: usize,
        from: (f32, f32),
        moved: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    marks: Vec<Mark>,
    crop: Option<Area>,
}

#[derive(Clone, Debug, PartialEq)]
struct Typing {
    at: (f32, f32),
    size: f32,
    replaced: Option<(usize, Mark)>,
}

pub trait Raster: Send + Sync {
    fn bgra(&self) -> &[u8];
}

impl Raster for Vec<u8> {
    fn bgra(&self) -> &[u8] {
        self
    }
}

impl Raster for screen::Frozen {
    fn bgra(&self) -> &[u8] {
        &self.bgra
    }
}

pub trait Annotate: Sized + 'static {
    fn board(&mut self) -> &mut Board;
}

struct Source {
    pixels: Arc<dyn Raster>,
    width: u32,
    height: u32,
}

struct Base {
    marks: Vec<Mark>,
    image: Arc<RenderImage>,
}

pub struct Board {
    source: Option<Source>,
    base: Option<Base>,
    base_pending: Option<Vec<Mark>>,
    pub tool: Tool,
    color: [u8; 3],
    size: u32,
    style: TextStyle,
    counter: u32,
    marks: Vec<Mark>,
    crop: Option<Area>,
    selected: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    drag: Option<Drag>,
    stage: Option<Bounds<Pixels>>,
    text: Entity<TextInput>,
    typing: Option<Typing>,
    pub picking: bool,
    pub palette: Option<Point<Pixels>>,
    focus: FocusHandle,
    can_crop: bool,
    _subscription: Subscription,
}

pub fn render_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    RgbaImage::from_raw(width, height, bgra)
        .map(|image| Arc::new(RenderImage::new(smallvec![ImageFrame::new(image)])))
}

pub fn picture_rect(stage: Bounds<Pixels>, frame: (u32, u32)) -> Bounds<Pixels> {
    let (width, height) = (f32::from(stage.size.width), f32::from(stage.size.height));
    let aspect = frame.0.max(1) as f32 / frame.1.max(1) as f32;
    let (w, h) = if width / height.max(1.0) > aspect {
        (height * aspect, height)
    } else {
        (width, width / aspect)
    };
    Bounds::new(
        point(
            stage.origin.x + px((width - w) / 2.0),
            stage.origin.y + px((height - h) / 2.0),
        ),
        size(px(w), px(h)),
    )
}

fn to_unit(rect: Bounds<Pixels>, at: Point<Pixels>) -> (f32, f32) {
    let x = f32::from(at.x - rect.origin.x) / f32::from(rect.size.width).max(1.0);
    let y = f32::from(at.y - rect.origin.y) / f32::from(rect.size.height).max(1.0);
    (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

fn to_screen(rect: Bounds<Pixels>, unit: (f32, f32)) -> (f32, f32) {
    (
        f32::from(rect.origin.x) + f32::from(rect.size.width) * unit.0,
        f32::from(rect.origin.y) + f32::from(rect.size.height) * unit.1,
    )
}

fn constrain(
    rect: Bounds<Pixels>,
    anchor: (f32, f32),
    unit: (f32, f32),
    constraint: Constraint,
) -> (f32, f32) {
    let (w, h) = (
        f32::from(rect.size.width).max(1.0),
        f32::from(rect.size.height).max(1.0),
    );
    let (dx, dy) = ((unit.0 - anchor.0) * w, (unit.1 - anchor.1) * h);
    let (x, y) = match constraint {
        Constraint::Free => return unit,
        Constraint::Square => {
            let side = dx.abs().max(dy.abs());
            (side * dx.signum(), side * dy.signum())
        }
        Constraint::Angle => {
            let step = std::f32::consts::FRAC_PI_4;
            let length = (dx * dx + dy * dy).sqrt();
            let angle = (dy.atan2(dx) / step).round() * step;
            (length * angle.cos(), length * angle.sin())
        }
    };
    (
        (anchor.0 + x / w).clamp(0.0, 1.0),
        (anchor.1 + y / h).clamp(0.0, 1.0),
    )
}

pub fn color_of(color: [u8; 3]) -> Hsla {
    gpui::rgb(u32::from(color[0]) << 16 | u32::from(color[1]) << 8 | u32::from(color[2])).into()
}

pub fn compose(
    bgra: &[u8],
    width: u32,
    height: u32,
    marks: &[Mark],
    crop: Option<Area>,
) -> (Vec<u8>, u32, u32) {
    render_marks(bgra.to_vec(), width, height, marks, crop, &draw_text)
}

fn extends(old: &Mark, new: &Mark) -> bool {
    match (old, new) {
        (
            Mark::BlurStroke {
                points: done,
                width: before,
            },
            Mark::BlurStroke { points, width },
        ) => before == width && points.starts_with(done),
        _ => old == new,
    }
}

fn unbaked(old: &Mark, new: &Mark) -> Option<Mark> {
    if old == new {
        return None;
    }
    match (old, new) {
        (
            Mark::BlurStroke {
                points: done,
                width: before,
            },
            Mark::BlurStroke { points, width },
        ) if before == width && !done.is_empty() && points.starts_with(done) => {
            Some(Mark::BlurStroke {
                points: points[done.len() - 1..].to_vec(),
                width: *width,
            })
        }
        _ => Some(new.clone()),
    }
}

fn baked_prefix(marks: &[Mark]) -> &[Mark] {
    let end = marks
        .iter()
        .rposition(Mark::is_effect)
        .map_or(0, |last| last + 1);
    &marks[..end]
}

fn covers(baked: &[Mark], marks: &[Mark], moving: Option<usize>) -> bool {
    baked.len() <= marks.len()
        && baked
            .iter()
            .zip(marks)
            .enumerate()
            .all(|(index, (old, new))| moving == Some(index) || extends(old, new))
}

fn loose(baked: &[Mark], marks: &[Mark]) -> Vec<Mark> {
    marks
        .iter()
        .enumerate()
        .filter_map(|(index, mark)| match baked.get(index) {
            Some(old) => unbaked(old, mark),
            None => Some(mark.clone()),
        })
        .collect()
}

fn clip(area: Area) -> Option<Area> {
    let (left, top) = (area.x.max(0.0), area.y.max(0.0));
    let (right, bottom) = (area.right().min(1.0), area.bottom().min(1.0));
    (right > left && bottom > top).then_some(Area {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

pub fn reframe(marks: &[Mark], frame: (u32, u32), crop: screen::Area) -> Vec<Mark> {
    let (frame_w, frame_h) = (frame.0.max(1) as f32, frame.1.max(1) as f32);
    let (crop_w, crop_h) = (crop.width.max(1) as f32, crop.height.max(1) as f32);
    let (left, top) = (crop.x as f32, crop.y as f32);
    let place = |unit: (f32, f32)| {
        (
            (unit.0 * frame_w - left) / crop_w,
            (unit.1 * frame_h - top) / crop_h,
        )
    };
    let height = |value: f32| value * frame_h / crop_h;
    let region = |area: Area| {
        let (x, y) = place((area.x, area.y));
        clip(Area {
            x,
            y,
            width: area.width * frame_w / crop_w,
            height: area.height * frame_h / crop_h,
        })
    };
    let points = |points: &[(f32, f32)]| points.iter().map(|point| place(*point)).collect();
    marks
        .iter()
        .filter_map(|mark| match mark {
            Mark::Stroke {
                shape,
                points: line,
                color,
                width,
            } => Some(Mark::Stroke {
                shape: *shape,
                points: points(line),
                color: *color,
                width: height(*width),
            }),
            Mark::BlurStroke {
                points: line,
                width,
            } => Some(Mark::BlurStroke {
                points: points(line),
                width: height(*width),
            }),
            Mark::Fill {
                area,
                color,
                radius,
            } => region(*area).map(|area| Mark::Fill {
                area,
                color: *color,
                radius: height(*radius),
            }),
            Mark::Blur(area) => region(*area).map(Mark::Blur),
            Mark::Invert(area) => region(*area).map(Mark::Invert),
            Mark::Pixelate { area, block } => region(*area).map(|area| Mark::Pixelate {
                area,
                block: height(*block),
            }),
            Mark::Text {
                at,
                text,
                size,
                style,
            } => Some(Mark::Text {
                at: place(*at),
                text: text.clone(),
                size: height(*size),
                style: style.clone(),
            }),
            Mark::Counter {
                center,
                tail,
                number,
                radius,
                color,
            } => Some(Mark::Counter {
                center: place(*center),
                tail: tail.map(place),
                number: *number,
                radius: height(*radius),
                color: *color,
            }),
        })
        .collect()
}

impl Board {
    pub fn new<T: Annotate>(
        focus: FocusHandle,
        can_crop: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        let text = cx.new(|cx| TextInput::new("Текст…", window, cx).multiline());
        let subscription = cx.subscribe_in(
            &text,
            window,
            |host: &mut T, _, event: &InputEvent, window, cx| match event {
                InputEvent::Submit | InputEvent::Blur => host.board().commit_text(window, cx),
                InputEvent::Changed => cx.notify(),
            },
        );
        Self {
            source: None,
            base: None,
            base_pending: None,
            tool: Tool::Pen,
            color: DEFAULT_COLOR,
            size: DEFAULT_SIZE,
            style: TextStyle::default(),
            counter: 1,
            marks: Vec::new(),
            crop: None,
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            drag: None,
            stage: None,
            text,
            typing: None,
            picking: false,
            palette: None,
            focus,
            can_crop,
            _subscription: subscription,
        }
    }

    pub fn set_source<T: 'static>(
        &mut self,
        pixels: Arc<dyn Raster>,
        width: u32,
        height: u32,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.drop_base(window, cx);
        self.source = Some(Source {
            pixels,
            width,
            height,
        });
    }

    pub fn source(&self) -> Option<(Arc<dyn Raster>, u32, u32)> {
        let source = self.source.as_ref()?;
        Some((source.pixels.clone(), source.width, source.height))
    }

    pub fn frame(&self) -> Option<(u32, u32)> {
        self.source
            .as_ref()
            .map(|source| (source.width, source.height))
    }

    pub fn drop_base<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        if let Some(base) = self.base.take() {
            cx.drop_image(base.image, Some(window));
        }
    }

    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    pub fn crop(&self) -> Option<Area> {
        self.crop
    }

    pub fn color(&self) -> [u8; 3] {
        self.color
    }

    pub fn typing(&self) -> bool {
        self.typing.is_some()
    }

    pub fn has_selected(&self) -> bool {
        self.selected.is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn deselect(&mut self) {
        self.selected = None;
    }

    fn moving(&self) -> Option<usize> {
        match self.drag {
            Some(Drag::Area { index, .. } | Drag::Move { index, .. } | Drag::Tail(index)) => {
                Some(index)
            }
            _ => None,
        }
    }

    fn moving_effect(&self) -> Option<usize> {
        self.moving()
            .filter(|index| self.marks.get(*index).is_some_and(Mark::is_effect))
    }

    fn moving_baked(&self) -> bool {
        self.moving()
            .is_some_and(|index| index < baked_prefix(&self.marks).len())
    }

    fn shown_base(&self) -> Option<&Base> {
        self.base
            .as_ref()
            .filter(|base| covers(&base.marks, &self.marks, self.moving_effect()))
    }

    pub fn backdrop(&self) -> Option<Arc<RenderImage>> {
        self.shown_base().map(|base| base.image.clone())
    }

    fn loose(&self) -> Vec<Mark> {
        let baked = self.shown_base().map_or(&[][..], |base| &base.marks);
        loose(baked, &self.marks)
    }

    pub fn refresh_base<T: Annotate>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        if self.moving_baked() || self.base_pending.is_some() {
            return;
        }
        let baked = baked_prefix(&self.marks).to_vec();
        if baked.is_empty() {
            self.drop_base(window, cx);
            return;
        }
        if self.base.as_ref().is_some_and(|base| base.marks == baked) {
            return;
        }
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let (pixels, width, height) = (source.pixels.clone(), source.width, source.height);
        self.base_pending = Some(baked.clone());
        cx.spawn_in(window, async move |this, cx| {
            let job = baked.clone();
            let (bgra, w, h) = cx
                .background_executor()
                .spawn(async move { compose(pixels.bgra(), width, height, &job, None) })
                .await;
            this.update_in(cx, |host, window, cx| {
                let board = host.board();
                board.base_pending = None;
                if let Some(image) = render_image(w, h, bgra) {
                    board.drop_base(window, cx);
                    board.base = Some(Base {
                        marks: baked,
                        image,
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn changed(&self) -> bool {
        !self.marks.is_empty() || self.crop.is_some_and(|area| !area.is_full())
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            marks: self.marks.clone(),
            crop: self.crop,
        }
    }

    fn remember(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.marks = snapshot.marks;
        self.crop = snapshot.crop;
        self.selected = None;
        self.counter = self.next_counter();
    }

    fn next_counter(&self) -> u32 {
        self.marks
            .iter()
            .filter_map(|mark| match mark {
                Mark::Counter { number, .. } => Some(*number + 1),
                _ => None,
            })
            .max()
            .unwrap_or(1)
            .min(MAX_COUNTER)
    }

    pub fn undo<T: 'static>(&mut self, cx: &mut Context<T>) {
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(snapshot);
            cx.notify();
        }
    }

    pub fn redo<T: 'static>(&mut self, cx: &mut Context<T>) {
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(snapshot);
            cx.notify();
        }
    }

    pub fn clear<T: 'static>(&mut self, cx: &mut Context<T>) {
        if self.changed() {
            self.remember();
            self.restore(Snapshot {
                marks: Vec::new(),
                crop: None,
            });
            cx.notify();
        }
    }

    pub fn delete_selected<T: 'static>(&mut self, cx: &mut Context<T>) {
        if let Some(index) = self.selected.take()
            && index < self.marks.len()
        {
            self.remember();
            self.marks.remove(index);
            cx.notify();
        }
    }

    pub fn discard_typing(&mut self) {
        self.typing = None;
    }

    pub fn reset(&mut self) {
        self.marks.clear();
        self.crop = None;
        self.selected = None;
        self.undo.clear();
        self.redo.clear();
        self.counter = 1;
        self.picking = false;
        self.palette = None;
    }

    fn unit_height(&self, pixels: f32) -> f32 {
        pixels
            / self
                .source
                .as_ref()
                .map_or(1080.0, |source| source.height as f32)
    }

    fn aspect(&self) -> f32 {
        self.source.as_ref().map_or(16.0 / 9.0, |source| {
            source.width as f32 / source.height.max(1) as f32
        })
    }

    pub fn set_color<T: 'static>(&mut self, color: [u8; 3], cx: &mut Context<T>) {
        self.color = color;
        if let Some(index) = self.selected
            && self.marks.get(index).and_then(Mark::color).is_some()
        {
            self.remember();
            if let Some(mark) = self.marks.get_mut(index) {
                mark.set_color(color);
            }
        }
        if self.typing.is_some() {
            self.style.color = color;
        }
        cx.notify();
    }

    pub fn set_size<T: 'static>(&mut self, size: u32, cx: &mut Context<T>) {
        let size = size.clamp(MIN_SIZE, MAX_SIZE);
        if size == self.size {
            return;
        }
        self.size = size;
        let text_size = self.unit_height(Tool::Text.pixels(size));
        if let Some(typing) = self.typing.as_mut() {
            typing.size = text_size;
        }
        if let Some(index) = self.selected {
            let tool = match self.marks.get(index) {
                Some(Mark::Stroke {
                    shape: Shape::Marker,
                    ..
                }) => Some(Tool::Marker),
                Some(Mark::Stroke { .. }) => Some(Tool::Pen),
                Some(Mark::BlurStroke { .. }) => Some(Tool::BlurBrush),
                Some(Mark::Text { .. }) => Some(Tool::Text),
                Some(Mark::Counter { .. }) => Some(Tool::Counter),
                _ => None,
            };
            if let Some(tool) = tool {
                let value = self.unit_height(tool.pixels(size));
                self.remember();
                match self.marks.get_mut(index) {
                    Some(Mark::Stroke { width, .. } | Mark::BlurStroke { width, .. }) => {
                        *width = value
                    }
                    Some(Mark::Text { size, .. }) => *size = value,
                    Some(Mark::Counter { radius, .. }) => *radius = value,
                    _ => {}
                }
            }
        }
        cx.notify();
    }

    fn set_counter<T: 'static>(&mut self, value: i64, cx: &mut Context<T>) {
        self.counter = value.clamp(1, i64::from(MAX_COUNTER)) as u32;
        cx.notify();
    }

    pub fn choose_tool<T: 'static>(
        &mut self,
        tool: Tool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.commit_text(window, cx);
        self.tool = tool;
        self.picking = false;
        if tool != Tool::Select {
            self.selected = None;
        }
        cx.notify();
    }

    fn begin_typing<T: 'static>(
        &mut self,
        at: (f32, f32),
        replaced: Option<(usize, Mark)>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let (content, size) = match &replaced {
            Some((
                _,
                Mark::Text {
                    text, size, style, ..
                },
            )) => {
                self.style = style.clone();
                self.color = style.color;
                (text.clone(), *size)
            }
            _ => (
                String::new(),
                self.unit_height(Tool::Text.pixels(self.size)),
            ),
        };
        self.typing = Some(Typing { at, size, replaced });
        self.selected = None;
        self.text.update(cx, |input, cx| {
            input.set_text(content, cx);
            input.select_all_text(cx);
        });
        let handle = self.text.focus_handle(cx);
        window.prevent_default();
        cx.stop_propagation();
        cx.defer_in(window, move |_, window, _| window.focus(&handle));
        cx.notify();
    }

    pub fn commit_text<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        let Some(typing) = self.typing.take() else {
            return;
        };
        let content = self.text.read(cx).text().trim_end().to_owned();
        let mark = (!content.trim().is_empty()).then(|| Mark::Text {
            at: typing.at,
            text: content,
            size: typing.size,
            style: TextStyle {
                color: self.color,
                ..self.style.clone()
            },
        });
        match (typing.replaced, mark) {
            (Some((index, old)), Some(mark)) if old != mark => {
                self.remember();
                self.marks.insert(index.min(self.marks.len()), mark);
            }
            (Some((index, old)), Some(_)) => self.marks.insert(index.min(self.marks.len()), old),
            (Some((index, old)), None) => {
                self.marks.insert(index.min(self.marks.len()), old);
                self.remember();
                self.marks.remove(index.min(self.marks.len() - 1));
            }
            (None, Some(mark)) => {
                self.remember();
                self.marks.push(mark);
            }
            (None, None) => {}
        }
        self.text.update(cx, |input, cx| input.set_text("", cx));
        window.focus(&self.focus);
        cx.notify();
    }

    fn cancel_typing<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        if let Some(typing) = self.typing.take()
            && let Some((index, old)) = typing.replaced
        {
            self.marks.insert(index.min(self.marks.len()), old);
        }
        self.text.update(cx, |input, cx| input.set_text("", cx));
        window.focus(&self.focus);
        cx.notify();
    }

    fn picture_bounds(&self) -> Option<Bounds<Pixels>> {
        let source = self.source.as_ref()?;
        Some(picture_rect(self.stage?, (source.width, source.height)))
    }

    fn crop_edge(&self, unit: (f32, f32)) -> Option<Edge> {
        let area = self.crop?;
        let picture = self.picture_bounds()?;
        let width = f32::from(picture.size.width).max(1.0);
        let height = f32::from(picture.size.height).max(1.0);
        crop_edge_near(
            area,
            unit,
            (CROP_GRAB / width, CROP_GRAB / height),
            (CROP_LINE_GRAB / width, CROP_LINE_GRAB / height),
        )
    }

    fn hit(&self, unit: (f32, f32)) -> Option<usize> {
        let aspect = self.aspect();
        let slop_x = HIT_SLOP / aspect;
        self.marks
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, mark)| {
                let (x0, y0, x1, y1) = mark.reach(aspect);
                (unit.0 >= x0 - slop_x
                    && unit.0 <= x1 + slop_x
                    && unit.1 >= y0 - HIT_SLOP
                    && unit.1 <= y1 + HIT_SLOP)
                    .then_some(index)
            })
    }

    pub fn hits(&self, position: Point<Pixels>) -> bool {
        self.picture_bounds()
            .is_some_and(|picture| self.hit(to_unit(picture, position)).is_some())
    }

    fn pick<T: 'static>(&mut self, unit: (f32, f32), cx: &mut Context<T>) {
        self.picking = false;
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let x = ((unit.0 * source.width as f32) as u32).min(source.width.saturating_sub(1));
        let y = ((unit.1 * source.height as f32) as u32).min(source.height.saturating_sub(1));
        let at = (y as usize * source.width as usize + x as usize) * 4;
        if let Some(pixel) = source.pixels.bgra().get(at..at + 3) {
            let color = [pixel[2], pixel[1], pixel[0]];
            self.set_color(color, cx);
        }
    }

    pub fn press<T: 'static>(
        &mut self,
        position: Point<Pixels>,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if self.typing.is_some() {
            self.commit_text(window, cx);
        }
        let Some(picture) = self.picture_bounds() else {
            return;
        };
        let unit = to_unit(picture, position);
        if self.picking {
            self.pick(unit, cx);
            return;
        }
        if click_count >= 2
            && let Some(index) = self.hit(unit)
            && let Some(Mark::Text { at, .. }) = self.marks.get(index)
        {
            let at = *at;
            let old = self.marks.remove(index);
            self.drag = None;
            self.begin_typing(at, Some((index, old)), window, cx);
            return;
        }
        let color = self.color;
        let pixels = self.tool.pixels(self.size);
        let value = self.unit_height(pixels);
        self.drag = match self.tool {
            Tool::Select => {
                self.selected = self.hit(unit);
                if self.selected.is_some() {
                    self.remember();
                }
                self.selected.map(|index| Drag::Move {
                    index,
                    from: unit,
                    moved: false,
                })
            }
            Tool::Crop => {
                let edge = self.crop_edge(unit);
                self.remember();
                match (edge, self.crop) {
                    (Some(edge), Some(start)) => Some(Drag::CropResize { edge, start }),
                    _ => {
                        self.crop = Some(Area::spanning(unit, unit));
                        Some(Drag::Crop { anchor: unit })
                    }
                }
            }
            Tool::Text => {
                self.begin_typing(unit, None, window, cx);
                None
            }
            Tool::Rect | Tool::Pixelate | Tool::Invert => {
                self.remember();
                let area = Area::spanning(unit, unit);
                self.marks.push(match self.tool {
                    Tool::Rect => Mark::Fill {
                        area,
                        color,
                        radius: self.unit_height(self.size as f32),
                    },
                    Tool::Pixelate if self.size <= MIN_SIZE => Mark::Blur(area),
                    Tool::Pixelate => Mark::Pixelate { area, block: value },
                    _ => Mark::Invert(area),
                });
                Some(Drag::Area {
                    index: self.marks.len() - 1,
                    anchor: unit,
                })
            }
            Tool::BlurBrush => {
                self.remember();
                self.marks.push(Mark::BlurStroke {
                    points: vec![unit],
                    width: value,
                });
                Some(Drag::Stroke {
                    index: self.marks.len() - 1,
                    anchor: unit,
                    constraint: Constraint::Free,
                })
            }
            Tool::Counter => {
                self.remember();
                self.marks.push(Mark::Counter {
                    center: unit,
                    tail: None,
                    number: self.counter,
                    radius: value,
                    color,
                });
                self.counter = (self.counter + 1).min(MAX_COUNTER);
                Some(Drag::Tail(self.marks.len() - 1))
            }
            tool => tool.shape().map(|shape| {
                self.remember();
                let points = if shape.is_freehand() {
                    vec![unit]
                } else {
                    vec![unit, unit]
                };
                self.marks.push(Mark::Stroke {
                    shape,
                    points,
                    color,
                    width: value,
                });
                Drag::Stroke {
                    index: self.marks.len() - 1,
                    anchor: unit,
                    constraint: tool.constraint(),
                }
            }),
        };
        cx.notify();
    }

    pub fn drag_to<T: 'static>(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<T>,
    ) {
        let (Some(drag), Some(picture)) = (self.drag, self.picture_bounds()) else {
            return;
        };
        let unit = to_unit(picture, position);
        let held = |constraint: Constraint, modifiers: Modifiers| {
            if modifiers.control || modifiers.shift {
                constraint
            } else {
                Constraint::Free
            }
        };
        let square = held(Constraint::Square, modifiers);
        match drag {
            Drag::Crop { anchor } => {
                self.crop = Some(Area::spanning(
                    anchor,
                    constrain(picture, anchor, unit, square),
                ))
            }
            Drag::CropResize { edge, start } => {
                self.crop = Some(resize_crop(start, edge, unit));
            }
            Drag::Area { index, anchor } => {
                let corner = constrain(picture, anchor, unit, square);
                if let Some(
                    Mark::Fill { area, .. }
                    | Mark::Blur(area)
                    | Mark::Pixelate { area, .. }
                    | Mark::Invert(area),
                ) = self.marks.get_mut(index)
                {
                    *area = Area::spanning(anchor, corner);
                }
            }
            Drag::Tail(index) => {
                if let Some(Mark::Counter { tail, .. }) = self.marks.get_mut(index) {
                    *tail = Some(unit);
                }
            }
            Drag::Move { index, from, .. } => {
                if let Some(mark) = self.marks.get_mut(index) {
                    mark.translate(unit.0 - from.0, unit.1 - from.1);
                }
                self.drag = Some(Drag::Move {
                    index,
                    from: unit,
                    moved: true,
                });
            }
            Drag::Stroke {
                index,
                anchor,
                constraint,
            } => {
                let end = constrain(picture, anchor, unit, held(constraint, modifiers));
                let far = |points: &[(f32, f32)]| {
                    points.last().is_none_or(|last| {
                        (last.0 - unit.0).abs() + (last.1 - unit.1).abs() > POINT_STEP
                    })
                };
                match self.marks.get_mut(index) {
                    Some(Mark::Stroke { shape, points, .. }) if !shape.is_freehand() => {
                        if let Some(last) = points.last_mut() {
                            *last = end;
                        }
                    }
                    Some(Mark::Stroke { points, .. } | Mark::BlurStroke { points, .. })
                        if far(points) =>
                    {
                        points.push(unit)
                    }
                    _ => {}
                }
            }
        }
        cx.notify();
    }

    pub fn release<T: 'static>(&mut self, cx: &mut Context<T>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        let tiny = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() + (a.1 - b.1).abs() < MIN_SHAPE;
        let discard = |board: &mut Self, index: usize| {
            board.marks.remove(index);
            board.undo.pop();
        };
        match drag {
            Drag::Stroke { index, .. } => {
                if let Some(Mark::Stroke { shape, points, .. }) = self.marks.get(index)
                    && !shape.is_freehand()
                    && tiny(points[0], points[points.len() - 1])
                {
                    discard(self, index);
                }
            }
            Drag::Area { index, .. } => {
                let small = match self.marks.get(index) {
                    Some(
                        Mark::Fill { area, .. }
                        | Mark::Blur(area)
                        | Mark::Pixelate { area, .. }
                        | Mark::Invert(area),
                    ) => area.width < MIN_EFFECT || area.height < MIN_EFFECT,
                    _ => false,
                };
                if small {
                    discard(self, index);
                }
            }
            Drag::Crop { .. } => {
                if self
                    .crop
                    .is_some_and(|area| area.width < MIN_CROP || area.height < MIN_CROP)
                {
                    self.crop = None;
                    self.undo.pop();
                }
            }
            Drag::CropResize { start, .. } => {
                if self
                    .crop
                    .is_some_and(|area| area.width < MIN_CROP || area.height < MIN_CROP)
                {
                    self.crop = Some(start);
                    self.undo.pop();
                }
            }
            Drag::Tail(_) => {}
            Drag::Move { moved, .. } => {
                if !moved {
                    self.undo.pop();
                }
            }
        }
        cx.notify();
    }

    pub fn wheel<T: 'static>(&mut self, event: &ScrollWheelEvent, cx: &mut Context<T>) {
        let delta = f32::from(event.delta.pixel_delta(px(20.0)).y);
        if delta == 0.0 {
            return;
        }
        let step: i64 = if delta > 0.0 { 1 } else { -1 };
        if event.modifiers.control {
            self.set_counter(i64::from(self.counter) + step, cx);
        } else {
            self.set_size((i64::from(self.size) + step).max(0) as u32, cx);
        }
        cx.stop_propagation();
    }

    pub fn typing_key<T: 'static>(
        &mut self,
        key: &str,
        control: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        match key {
            "escape" => self.cancel_typing(window, cx),
            "enter" if control => self.commit_text(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    pub fn edit_key<T: 'static>(
        &mut self,
        key: &str,
        control: bool,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> bool {
        match key {
            "z" if control && shift => self.redo(cx),
            "z" if control => self.undo(cx),
            "y" if control => self.redo(cx),
            "delete" | "backspace" if self.selected.is_some() => self.delete_selected(cx),
            "g" if !control => self.picking = !self.picking,
            "-" => self.set_size(self.size.saturating_sub(1), cx),
            "=" | "+" => self.set_size(self.size + 1, cx),
            other if !control => {
                let found = self
                    .tools()
                    .find(|(_, _, letter, _)| *letter == other)
                    .map(|(tool, ..)| *tool);
                match found {
                    Some(tool) => self.choose_tool(tool, window, cx),
                    None => return false,
                }
            }
            _ => return false,
        }
        true
    }

    pub fn tools(&self) -> impl Iterator<Item = &'static (Tool, Icon, &'static str, &'static str)> {
        let can_crop = self.can_crop;
        TOOLS
            .iter()
            .filter(move |(tool, ..)| can_crop || *tool != Tool::Crop)
    }

    pub fn cursor(&self) -> CursorStyle {
        if self.picking {
            CursorStyle::Crosshair
        } else {
            match self.tool {
                Tool::Text => CursorStyle::IBeam,
                Tool::Select => CursorStyle::OpenHand,
                _ => CursorStyle::Crosshair,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holding_ctrl_squares_boxes_and_snaps_lines() {
        let rect = Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(100.0)));
        let square = constrain(rect, (0.0, 0.0), (0.5, 0.3), Constraint::Square);
        assert!((square.0 * 200.0 - square.1 * 100.0).abs() < 1e-3);
        let snapped = constrain(rect, (0.0, 0.5), (0.5, 0.52), Constraint::Angle);
        assert!((snapped.1 - 0.5).abs() < 1e-4);
        let diagonal = constrain(rect, (0.0, 0.0), (0.3, 0.55), Constraint::Angle);
        assert!((diagonal.0 * 200.0 - diagonal.1 * 100.0).abs() < 1e-2);
    }

    #[test]
    fn only_the_unbaked_tail_of_a_blur_stroke_needs_a_placeholder() {
        let old = Mark::BlurStroke {
            points: vec![(0.1, 0.1), (0.2, 0.2)],
            width: 0.02,
        };
        let new = Mark::BlurStroke {
            points: vec![(0.1, 0.1), (0.2, 0.2), (0.3, 0.3)],
            width: 0.02,
        };
        assert_eq!(unbaked(&old, &old), None);
        assert_eq!(
            unbaked(&old, &new),
            Some(Mark::BlurStroke {
                points: vec![(0.2, 0.2), (0.3, 0.3)],
                width: 0.02,
            })
        );
        assert_eq!(unbaked(&new, &old), Some(old.clone()));
        let area = Mark::Blur(Area::FULL);
        assert_eq!(unbaked(&area, &old), Some(old));
        let order: Vec<Tool> = TOOLS.iter().map(|(tool, ..)| *tool).collect();
        let pixelate = order.iter().position(|tool| *tool == Tool::Pixelate);
        assert_eq!(
            pixelate.map(|index| order[index + 1]),
            Some(Tool::BlurBrush)
        );
    }

    fn pen(y: f32) -> Mark {
        Mark::Stroke {
            shape: Shape::Pen,
            points: vec![(0.1, y), (0.9, y)],
            color: [255, 0, 0],
            width: 0.01,
        }
    }

    fn brush(points: &[(f32, f32)]) -> Mark {
        Mark::BlurStroke {
            points: points.to_vec(),
            width: 0.1,
        }
    }

    #[test]
    fn everything_up_to_the_last_effect_is_baked() {
        let blur = Mark::Blur(Area::FULL);
        assert!(baked_prefix(&[pen(0.2), pen(0.4)]).is_empty());
        let marks = [pen(0.2), blur.clone(), pen(0.4), blur, pen(0.6)];
        assert_eq!(baked_prefix(&marks), &marks[..4]);
    }

    #[test]
    fn drawings_baked_under_an_effect_are_not_painted_again() {
        let blur = Mark::Blur(Area::FULL);
        let marks = [pen(0.2), blur, pen(0.4)];
        assert_eq!(loose(&marks[..2], &marks), vec![pen(0.4)]);
        assert_eq!(loose(&[], &marks), marks.to_vec());
    }

    #[test]
    fn a_growing_blur_stroke_keeps_its_bake_and_paints_only_its_tail() {
        let baked = [pen(0.2), brush(&[(0.1, 0.1), (0.2, 0.2)])];
        let marks = [pen(0.2), brush(&[(0.1, 0.1), (0.2, 0.2), (0.3, 0.3)])];
        assert!(covers(&baked, &marks, None));
        assert_eq!(
            loose(&baked, &marks),
            vec![brush(&[(0.2, 0.2), (0.3, 0.3)])]
        );
    }

    #[test]
    fn a_bake_is_stale_once_a_baked_mark_changes_unless_that_effect_is_being_moved() {
        let blur = Mark::Blur(Area::FULL);
        let baked = [pen(0.2), blur.clone()];
        assert!(covers(&baked, &[pen(0.2), blur.clone(), pen(0.4)], None));
        assert!(!covers(&baked, &[pen(0.3), blur.clone()], None));
        assert!(!covers(&baked, &[pen(0.2)], None));
        let moved = [pen(0.2), Mark::Invert(Area::FULL)];
        assert!(!covers(&baked, &moved, None));
        assert!(covers(&baked, &moved, Some(1)));
        assert_eq!(loose(&baked, &moved), vec![Mark::Invert(Area::FULL)]);
    }

    #[test]
    fn baking_a_stroke_under_a_blur_matches_the_final_picture() {
        let (width, height) = (40u32, 40u32);
        let source = vec![0u8; (width * height * 4) as usize];
        let brush = brush(&[(0.5, 0.2), (0.5, 0.8)]);
        let marks = [pen(0.5), brush, pen(0.3)];
        let (base, _, _) = compose(&source, width, height, baked_prefix(&marks), None);
        let centre = ((20 * width + 20) * 4 + 2) as usize;
        assert!(base[centre] > 0 && base[centre] < 255, "{}", base[centre]);
        let (whole, _, _) = compose(&source, width, height, &marks, None);
        let (rest, _, _) = compose(
            &base,
            width,
            height,
            &loose(baked_prefix(&marks), &marks),
            None,
        );
        assert_eq!(whole, rest);
    }

    #[test]
    fn tool_sizes_follow_the_flameshot_rules() {
        assert_eq!(Tool::Pen.pixels(3), 3.0);
        assert_eq!(Tool::Marker.pixels(3), 17.0);
        assert_eq!(Tool::Counter.pixels(3), 18.0);
        assert_eq!(Tool::Pixelate.pixels(3), 8.0);
        assert_eq!(Tool::BlurBrush.pixels(3), 20.0);
        let letters: Vec<&str> = TOOLS.iter().map(|(_, _, letter, _)| *letter).collect();
        let mut unique = letters.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(letters.len(), unique.len());
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
    }

    #[test]
    fn marks_are_translated_into_crop_coordinates() {
        let frame = (1000, 500);
        let crop = screen::Area {
            x: 200,
            y: 100,
            width: 400,
            height: 250,
        };
        let marks = [
            Mark::Stroke {
                shape: Shape::Line,
                points: vec![(0.2, 0.2), (0.6, 0.7)],
                color: [1, 2, 3],
                width: 0.01,
            },
            Mark::Counter {
                center: (0.4, 0.45),
                tail: Some((0.5, 0.5)),
                number: 2,
                radius: 0.02,
                color: [0, 0, 0],
            },
        ];
        let moved = reframe(&marks, frame, crop);
        let Mark::Stroke { points, width, .. } = &moved[0] else {
            panic!("stroke expected");
        };
        assert!(close(points[0], (0.0, 0.0)));
        assert!(close(points[1], (1.0, 1.0)));
        assert!((width - 0.02).abs() < 1e-6);
        let Mark::Counter {
            center,
            tail,
            radius,
            ..
        } = &moved[1]
        else {
            panic!("counter expected");
        };
        assert!(close(*center, (0.5, 0.5)));
        assert!(tail.is_some_and(|tail| close(tail, (0.75, 0.6))));
        assert!((radius - 0.04).abs() < 1e-6);
    }

    #[test]
    fn areas_are_clipped_to_the_crop_and_dropped_outside_it() {
        let frame = (100, 100);
        let crop = screen::Area {
            x: 50,
            y: 50,
            width: 50,
            height: 50,
        };
        let marks = [
            Mark::Invert(Area {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5,
            }),
            Mark::Blur(Area {
                x: 0.0,
                y: 0.0,
                width: 0.2,
                height: 0.2,
            }),
        ];
        let moved = reframe(&marks, frame, crop);
        assert_eq!(moved.len(), 1);
        let Mark::Invert(area) = moved[0] else {
            panic!("invert expected");
        };
        assert!(close((area.x, area.y), (0.0, 0.0)));
        assert!(close((area.width, area.height), (0.5, 0.5)));
    }

    #[test]
    fn rendering_reframed_marks_matches_rendering_then_cropping() {
        let (width, height) = (40u32, 20u32);
        let bgra = vec![0u8; (width * height * 4) as usize];
        let marks = [Mark::Fill {
            area: Area {
                x: 0.5,
                y: 0.25,
                width: 0.25,
                height: 0.5,
            },
            color: [0, 255, 0],
            radius: 0.0,
        }];
        let crop = screen::Area {
            x: 10,
            y: 0,
            width: 30,
            height: 20,
        };
        let whole = compose(
            &bgra,
            width,
            height,
            &marks,
            Some(Area {
                x: 0.25,
                y: 0.0,
                width: 0.75,
                height: 1.0,
            }),
        );
        let mut part = Vec::new();
        for row in 0..height as usize {
            let start = (row * width as usize + crop.x as usize) * 4;
            part.extend_from_slice(&bgra[start..start + crop.width as usize * 4]);
        }
        let framed = compose(
            &part,
            crop.width,
            crop.height,
            &reframe(&marks, (width, height), crop),
            None,
        );
        assert_eq!(whole, framed);
    }
}
