use std::time::Duration;

use gpui::{
    AnyElement, Bounds, Context, Hsla, MouseButton, MouseDownEvent, PathBuilder, Pixels, Point,
    SharedString, Window, canvas, div, fill, point, prelude::*, px, size,
};

use rewind_media::{Area, Blur, GAIN_MAX, Mix, SPEEDS, Stroke, Window as Range};

use super::{Editor, Picker, seconds};
use crate::assets::{Icon, icon};
use crate::theme::{CONTROL_TEXT, GUTTER, ICON, Palette, SMALL_TEXT, ThemeExt, alpha};
use crate::ui::{ButtonKind, Dropdown, Slider, button, label, section, value_box};
use crate::views::annotate::widgets::square_button;

pub(super) const INKS: [[u8; 3]; 5] = [
    [0xff, 0x3b, 0x30],
    [0x76, 0xb9, 0x00],
    [0xff, 0xd6, 0x0a],
    [0x0a, 0x84, 0xff],
    [0xff, 0xff, 0xff],
];
const INK_WIDTHS: [f32; 8] = [0.003, 0.006, 0.01, 0.015, 0.022, 0.03, 0.04, 0.055];
pub(super) const DEFAULT_INK_SIZE: usize = 1;
const INK_SIZE_WIDTH: f32 = 22.0;
const EFFECT_LENGTH: Duration = Duration::from_secs(3);
const MUTE_LENGTH: Duration = Duration::from_secs(2);
const BLUR_SIZE: f32 = 0.3;
const CORNER_GRAB: f32 = 14.0;
const CORNER_SIZE: f32 = 10.0;
const POINT_STEP: f32 = 0.002;
const LANE_ROW: f32 = 14.0;
const LANE_GAP: f32 = 3.0;
const LANE_ROWS: usize = 3;
const EDGE_GRAB: f32 = 8.0;
const SHADE: f32 = 0.6;
const PERCENT: f32 = 100.0;
const GAIN_STEP: u32 = 5;
const VALUE_WIDTH: f32 = 56.0;
const SWATCH: f32 = 18.0;
const DEFAULT_SPEED: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Pointer,
    Crop,
    Draw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Item {
    Mute(usize),
    Blur(usize),
    Stroke(usize),
}

impl Item {
    const fn row(self) -> usize {
        match self {
            Self::Mute(_) => 0,
            Self::Blur(_) => 1,
            Self::Stroke(_) => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const CORNERS: [Corner; 4] = [
    Corner::TopLeft,
    Corner::TopRight,
    Corner::BottomLeft,
    Corner::BottomRight,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum StageDrag {
    Press,
    Crop(Corner),
    CropMove {
        from: (f32, f32),
        area: Area,
    },
    Blur {
        index: usize,
        corner: Corner,
    },
    BlurMove {
        index: usize,
        from: (f32, f32),
        area: Area,
    },
    Draw(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LaneDrag {
    Start(Item),
    End(Item),
    Move { item: Item, grab: Duration },
}

pub(super) fn picture_rect(stage: Bounds<Pixels>, frame: (u32, u32)) -> Bounds<Pixels> {
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

pub(super) fn to_unit(rect: Bounds<Pixels>, at: Point<Pixels>) -> (f32, f32) {
    let x = f32::from(at.x - rect.origin.x) / f32::from(rect.size.width).max(1.0);
    let y = f32::from(at.y - rect.origin.y) / f32::from(rect.size.height).max(1.0);
    (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

fn to_screen(rect: Bounds<Pixels>, unit: (f32, f32)) -> Point<Pixels> {
    point(
        rect.origin.x + rect.size.width * unit.0,
        rect.origin.y + rect.size.height * unit.1,
    )
}

const fn corner_unit(area: Area, corner: Corner) -> (f32, f32) {
    match corner {
        Corner::TopLeft => (area.x, area.y),
        Corner::TopRight => (area.x + area.width, area.y),
        Corner::BottomLeft => (area.x, area.y + area.height),
        Corner::BottomRight => (area.x + area.width, area.y + area.height),
    }
}

const fn opposite(corner: Corner) -> Corner {
    match corner {
        Corner::TopLeft => Corner::BottomRight,
        Corner::TopRight => Corner::BottomLeft,
        Corner::BottomLeft => Corner::TopRight,
        Corner::BottomRight => Corner::TopLeft,
    }
}

pub(super) fn corner_at(area: Area, rect: Bounds<Pixels>, at: Point<Pixels>) -> Option<Corner> {
    CORNERS.into_iter().find(|corner| {
        let spot = to_screen(rect, corner_unit(area, *corner));
        f32::from(spot.x - at.x).abs() <= CORNER_GRAB
            && f32::from(spot.y - at.y).abs() <= CORNER_GRAB
    })
}

pub(super) fn drag_corner(area: Area, corner: Corner, unit: (f32, f32)) -> (Area, Corner) {
    let anchor = corner_unit(area, opposite(corner));
    let corner = match (unit.0 >= anchor.0, unit.1 >= anchor.1) {
        (true, true) => Corner::BottomRight,
        (true, false) => Corner::TopRight,
        (false, true) => Corner::BottomLeft,
        (false, false) => Corner::TopLeft,
    };
    (Area::from_corners(anchor, unit), corner)
}

fn ink_width(size: usize) -> f32 {
    INK_WIDTHS[size.min(INK_WIDTHS.len() - 1)]
}

fn ink_size_after(size: usize, thicker: bool) -> usize {
    if thicker {
        (size + 1).min(INK_WIDTHS.len() - 1)
    } else {
        size.saturating_sub(1)
    }
}

fn ink(color: [u8; 3]) -> Hsla {
    gpui::rgb(u32::from(color[0]) << 16 | u32::from(color[1]) << 8 | u32::from(color[2])).into()
}

fn outline(window: &mut Window, rect: Bounds<Pixels>, color: Hsla, thickness: f32) {
    let t = px(thickness);
    let o = rect.origin;
    let s = rect.size;
    for edge in [
        Bounds::new(o, size(s.width, t)),
        Bounds::new(point(o.x, o.y + s.height - t), size(s.width, t)),
        Bounds::new(o, size(t, s.height)),
        Bounds::new(point(o.x + s.width - t, o.y), size(t, s.height)),
    ] {
        window.paint_quad(fill(edge, color));
    }
}

fn area_rect(picture: Bounds<Pixels>, area: Area) -> Bounds<Pixels> {
    Bounds::new(
        to_screen(picture, (area.x, area.y)),
        size(
            picture.size.width * area.width,
            picture.size.height * area.height,
        ),
    )
}

fn handles(window: &mut Window, picture: Bounds<Pixels>, area: Area, color: Hsla) {
    for corner in CORNERS {
        let spot = to_screen(picture, corner_unit(area, corner));
        window.paint_quad(fill(
            Bounds::new(
                point(
                    spot.x - px(CORNER_SIZE / 2.0),
                    spot.y - px(CORNER_SIZE / 2.0),
                ),
                size(px(CORNER_SIZE), px(CORNER_SIZE)),
            ),
            color,
        ));
    }
}

struct Fader {
    id: &'static str,
    title: &'static str,
    value: f32,
    muted: bool,
    enabled: bool,
    set: fn(&mut Mix, f32),
    mute: fn(&mut Mix),
}

struct StageView {
    frame: (u32, u32),
    crop: Option<Area>,
    cropping: bool,
    blurs: Vec<(Area, bool)>,
    strokes: Vec<Stroke>,
}

fn paint_stage(view: &StageView, stage: Bounds<Pixels>, window: &mut Window, p: &Palette) {
    let picture = picture_rect(stage, view.frame);
    for (area, selected) in &view.blurs {
        let rect = area_rect(picture, *area);
        window.paint_quad(fill(rect, alpha(p.white, 0.22)));
        outline(
            window,
            rect,
            if *selected { p.accent } else { p.white },
            2.0,
        );
        if *selected {
            handles(window, picture, *area, p.accent);
        }
    }
    for stroke in &view.strokes {
        let width = (stroke.width * f32::from(picture.size.height)).max(1.0);
        let mut path = PathBuilder::stroke(px(width));
        let mut points = stroke.points.iter();
        if let Some(first) = points.next() {
            path.move_to(to_screen(picture, *first));
            if stroke.points.len() == 1 {
                path.line_to(to_screen(picture, (first.0 + 0.0005, first.1)));
            }
            for next in points {
                path.line_to(to_screen(picture, *next));
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, ink(stroke.color));
        }
    }
    let Some(crop) = view.crop.filter(|area| view.cropping || !area.is_full()) else {
        return;
    };
    let inner = area_rect(picture, crop);
    let shade = alpha(p.stage, SHADE);
    let (top, bottom) = (inner.origin.y, inner.origin.y + inner.size.height);
    let (left, right) = (inner.origin.x, inner.origin.x + inner.size.width);
    let edge = picture.origin.y + picture.size.height;
    let end = picture.origin.x + picture.size.width;
    for block in [
        Bounds::new(
            picture.origin,
            size(picture.size.width, top - picture.origin.y),
        ),
        Bounds::new(
            point(picture.origin.x, bottom),
            size(picture.size.width, edge - bottom),
        ),
        Bounds::new(
            point(picture.origin.x, top),
            size(left - picture.origin.x, bottom - top),
        ),
        Bounds::new(point(right, top), size(end - right, bottom - top)),
    ] {
        window.paint_quad(fill(block, shade));
    }
    outline(window, inner, p.white, 2.0);
    if view.cropping {
        handles(window, picture, crop, p.white);
    }
}

impl Editor {
    pub(super) fn now(&self) -> Duration {
        Duration::from_secs_f64(self.player_state().position.max(0.0))
    }

    fn frame_size(&self) -> (u32, u32) {
        self.info.map_or((16, 9), |info| (info.width, info.height))
    }

    fn effect_window(&self, length: Duration) -> Range {
        Range::around(self.now(), length, self.duration())
    }

    pub(super) fn item_window(&self, item: Item) -> Option<Range> {
        match item {
            Item::Mute(index) => self.edit.mutes.get(index).copied(),
            Item::Blur(index) => self.edit.blurs.get(index).map(|blur| blur.window),
            Item::Stroke(index) => self.edit.strokes.get(index).map(|stroke| stroke.window),
        }
    }

    fn set_item_window(&mut self, item: Item, window: Range) {
        match item {
            Item::Mute(index) => {
                if let Some(slot) = self.edit.mutes.get_mut(index) {
                    *slot = window;
                }
            }
            Item::Blur(index) => {
                if let Some(blur) = self.edit.blurs.get_mut(index) {
                    blur.window = window;
                }
            }
            Item::Stroke(index) => {
                if let Some(stroke) = self.edit.strokes.get_mut(index) {
                    stroke.window = window;
                }
            }
        }
    }

    fn items(&self) -> Vec<Item> {
        (0..self.edit.mutes.len())
            .map(Item::Mute)
            .chain((0..self.edit.blurs.len()).map(Item::Blur))
            .chain((0..self.edit.strokes.len()).map(Item::Stroke))
            .collect()
    }

    pub(super) fn toggle_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = if self.tool == tool {
            Tool::Pointer
        } else {
            tool
        };
        if self.tool == Tool::Crop && self.edit.crop.is_none() {
            self.edit.crop = Some(Area::FULL);
        }
        if self.tool == Tool::Crop
            && let Some(player) = &self.player
        {
            player.pause();
        }
        cx.notify();
    }

    fn add_blur(&mut self, cx: &mut Context<Self>) {
        self.edit.blurs.push(Blur {
            area: Area::centered(BLUR_SIZE, BLUR_SIZE),
            window: self.effect_window(EFFECT_LENGTH),
        });
        self.selected = Some(Item::Blur(self.edit.blurs.len() - 1));
        self.tool = Tool::Pointer;
        if let Some(player) = &self.player {
            player.pause();
        }
        cx.notify();
    }

    fn add_mute(&mut self, cx: &mut Context<Self>) {
        self.edit.mutes.push(self.effect_window(MUTE_LENGTH));
        self.selected = Some(Item::Mute(self.edit.mutes.len() - 1));
        cx.notify();
    }

    pub(super) fn delete_selected(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = self.selected.take() else {
            return false;
        };
        match item {
            Item::Mute(index) if index < self.edit.mutes.len() => {
                self.edit.mutes.remove(index);
            }
            Item::Blur(index) if index < self.edit.blurs.len() => {
                self.edit.blurs.remove(index);
            }
            Item::Stroke(index) if index < self.edit.strokes.len() => {
                self.edit.strokes.remove(index);
            }
            _ => {}
        }
        cx.notify();
        true
    }

    fn step_ink_size(&mut self, thicker: bool, cx: &mut Context<Self>) {
        self.ink_size = ink_size_after(self.ink_size, thicker);
        if let Some(Item::Stroke(index)) = self.selected
            && let Some(stroke) = self.edit.strokes.get_mut(index)
        {
            stroke.width = ink_width(self.ink_size);
        }
        cx.notify();
    }

    fn reset_crop(&mut self, cx: &mut Context<Self>) {
        self.edit.crop = None;
        self.tool = Tool::Pointer;
        cx.notify();
    }

    fn set_speed(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(speed) = SPEEDS.get(index) {
            self.edit.speed = *speed;
            if let Some(player) = &self.player {
                player.set_rate(f64::from(*speed));
            }
        }
        cx.notify();
    }

    pub(super) fn sync_audio(&mut self) {
        let Some(player) = &self.player else {
            return;
        };
        let gain = if self.audio_tracks > 1 {
            self.edit.mix.master_gain()
        } else {
            self.edit.mix.gains().0
        };
        let silenced = self.edit.muted_at(self.now());
        let muted = gain <= 0.0 || silenced;
        let volume = gain.min(1.0);
        let mix = self.edit.mix;
        if self.preview_audio != Some((mix, silenced)) {
            player.set_volume(f64::from(volume));
            player.set_muted(muted);
            player.set_tracks(
                (0..self.audio_tracks)
                    .map(|track| {
                        if silenced {
                            0.0
                        } else {
                            f64::from(mix.track_gain(track))
                        }
                    })
                    .collect(),
            );
            self.preview_audio = Some((mix, silenced));
        }
    }

    pub(super) fn on_stage_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(picture) = self.picture else {
            self.stage_drag = Some(StageDrag::Press);
            return;
        };
        let unit = to_unit(picture, event.position);
        let now = self.now();
        self.stage_drag = Some(match self.tool {
            Tool::Draw => {
                let color = INKS[self.ink % INKS.len()];
                self.edit.strokes.push(Stroke {
                    points: vec![unit],
                    width: ink_width(self.ink_size),
                    color,
                    window: self.effect_window(EFFECT_LENGTH),
                });
                let index = self.edit.strokes.len() - 1;
                self.selected = Some(Item::Stroke(index));
                StageDrag::Draw(index)
            }
            Tool::Crop => {
                let area = self.edit.crop.unwrap_or(Area::FULL);
                match corner_at(area, picture, event.position) {
                    Some(corner) => StageDrag::Crop(corner),
                    None if !area.is_full() && area.contains(unit) => {
                        StageDrag::CropMove { from: unit, area }
                    }
                    None => {
                        self.edit.crop = Some(Area::from_corners(unit, unit));
                        StageDrag::Crop(Corner::BottomRight)
                    }
                }
            }
            Tool::Pointer => {
                let selected_blur = match self.selected {
                    Some(Item::Blur(index)) => self
                        .edit
                        .blurs
                        .get(index)
                        .filter(|blur| blur.window.contains(now))
                        .map(|blur| (index, blur.area)),
                    _ => None,
                };
                let corner = selected_blur.and_then(|(index, area)| {
                    corner_at(area, picture, event.position).map(|c| (index, c))
                });
                if let Some((index, corner)) = corner {
                    StageDrag::Blur { index, corner }
                } else if let Some(index) = self
                    .edit
                    .blurs
                    .iter()
                    .rposition(|blur| blur.window.contains(now) && blur.area.contains(unit))
                {
                    self.selected = Some(Item::Blur(index));
                    StageDrag::BlurMove {
                        index,
                        from: unit,
                        area: self.edit.blurs[index].area,
                    }
                } else {
                    StageDrag::Press
                }
            }
        });
        cx.notify();
    }

    pub(super) fn on_stage_move(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let (Some(drag), Some(picture)) = (self.stage_drag, self.picture) else {
            return false;
        };
        let unit = to_unit(picture, at);
        match drag {
            StageDrag::Press => return true,
            StageDrag::Draw(index) => {
                if let Some(stroke) = self.edit.strokes.get_mut(index) {
                    let far = stroke.points.last().is_none_or(|last| {
                        (last.0 - unit.0).abs() + (last.1 - unit.1).abs() > POINT_STEP
                    });
                    if far {
                        stroke.points.push(unit);
                    }
                }
            }
            StageDrag::Crop(corner) => {
                let area = self.edit.crop.unwrap_or(Area::FULL);
                let (next, flipped) = drag_corner(area, corner, unit);
                self.edit.crop = Some(next);
                self.stage_drag = Some(StageDrag::Crop(flipped));
            }
            StageDrag::CropMove { from, area } => {
                self.edit.crop = Some(area.moved(unit.0 - from.0, unit.1 - from.1));
            }
            StageDrag::Blur { index, corner } => {
                if let Some(blur) = self.edit.blurs.get_mut(index) {
                    let (next, flipped) = drag_corner(blur.area, corner, unit);
                    blur.area = next;
                    self.stage_drag = Some(StageDrag::Blur {
                        index,
                        corner: flipped,
                    });
                }
            }
            StageDrag::BlurMove { index, from, area } => {
                if let Some(blur) = self.edit.blurs.get_mut(index) {
                    blur.area = area.moved(unit.0 - from.0, unit.1 - from.1);
                }
            }
        }
        cx.notify();
        true
    }

    pub(super) fn on_stage_up(&mut self, cx: &mut Context<Self>) {
        if self.stage_drag.take() == Some(StageDrag::Press) {
            self.selected = None;
            self.toggle_play(cx);
        }
    }

    fn lane_row_of(&self, y: Pixels) -> Option<usize> {
        let lane = self.lane?;
        let local = f32::from(y - lane.origin.y);
        let row = (local / (LANE_ROW + LANE_GAP)) as usize;
        (local >= 0.0 && row < LANE_ROWS).then_some(row)
    }

    pub(super) fn on_lane_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(row) = self.lane_row_of(event.position.y) else {
            return;
        };
        let x = f32::from(event.position.x - self.track.map_or(px(0.0), |track| track.origin.x));
        let at = self.time_at(event.position.x);
        let hit = self.items().into_iter().rev().find(|item| {
            item.row() == row
                && self.item_window(*item).is_some_and(|window| {
                    x >= self.x_of(window.start) - EDGE_GRAB
                        && x <= self.x_of(window.end) + EDGE_GRAB
                })
        });
        let Some(item) = hit else {
            self.selected = None;
            cx.notify();
            return;
        };
        let Some(window) = self.item_window(item) else {
            return;
        };
        self.selected = Some(item);
        self.lane_drag = Some(if (x - self.x_of(window.start)).abs() <= EDGE_GRAB {
            LaneDrag::Start(item)
        } else if (x - self.x_of(window.end)).abs() <= EDGE_GRAB {
            LaneDrag::End(item)
        } else {
            LaneDrag::Move {
                item,
                grab: at.saturating_sub(window.start),
            }
        });
        if let Some(player) = &self.player {
            player.seek(seconds(window.start.max(self.edit.span.start)));
        }
        cx.notify();
    }

    pub(super) fn on_lane_move(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.lane_drag else {
            return false;
        };
        let time = self.time_at(at.x);
        let limit = self.duration();
        let (item, next) = match drag {
            LaneDrag::Start(item) => (item, self.item_window(item).map(|w| w.with_start(time))),
            LaneDrag::End(item) => (
                item,
                self.item_window(item).map(|w| w.with_end(time, limit)),
            ),
            LaneDrag::Move { item, grab } => (
                item,
                self.item_window(item)
                    .map(|w| w.shifted(time.saturating_sub(grab), limit)),
            ),
        };
        if let Some(window) = next {
            self.set_item_window(item, window);
        }
        cx.notify();
        true
    }

    pub(super) fn stage_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let now = self.now();
        let view = StageView {
            frame: self.frame_size(),
            crop: self.edit.crop,
            cropping: self.tool == Tool::Crop,
            blurs: self
                .edit
                .blurs
                .iter()
                .enumerate()
                .filter(|(_, blur)| blur.window.contains(now))
                .map(|(index, blur)| (blur.area, self.selected == Some(Item::Blur(index))))
                .collect(),
            strokes: self
                .edit
                .strokes
                .iter()
                .filter(|stroke| stroke.window.contains(now))
                .cloned()
                .collect(),
        };
        let entity = cx.entity().downgrade();
        let frame = view.frame;
        canvas(
            move |bounds, window, cx| {
                let scale = window.scale_factor();
                entity
                    .update(cx, |editor, _| {
                        editor.picture = Some(picture_rect(bounds, frame));
                        editor.fit_player(bounds.size, scale);
                    })
                    .ok();
                bounds
            },
            move |_, bounds, window, _| paint_stage(&view, bounds, window, &p),
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    pub(super) fn lane(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let entity = cx.entity().downgrade();
        let duration = seconds(self.duration()).max(f64::EPSILON);
        let segments: Vec<(usize, f32, f32, Hsla, bool)> = self
            .items()
            .into_iter()
            .filter_map(|item| {
                let window = self.item_window(item)?;
                let color = match item {
                    Item::Mute(_) => p.record,
                    Item::Blur(_) => p.white,
                    Item::Stroke(index) => ink(self.edit.strokes[index].color),
                };
                Some((
                    item.row(),
                    (seconds(window.start) / duration) as f32,
                    (seconds(window.end) / duration) as f32,
                    color,
                    self.selected == Some(item),
                ))
            })
            .collect();
        let height = LANE_ROWS as f32 * (LANE_ROW + LANE_GAP);
        let painter = canvas(
            move |bounds, _, cx| {
                entity
                    .update(cx, |editor, _| editor.lane = Some(bounds))
                    .ok();
                bounds
            },
            move |_, bounds, window, _| {
                let width = f32::from(bounds.size.width);
                for row in 0..LANE_ROWS {
                    let top = bounds.origin.y + px(row as f32 * (LANE_ROW + LANE_GAP));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.origin.x, top),
                            size(bounds.size.width, px(LANE_ROW)),
                        ),
                        p.track,
                    ));
                }
                for (row, from, to, color, selected) in &segments {
                    let top = bounds.origin.y + px(*row as f32 * (LANE_ROW + LANE_GAP));
                    let rect = Bounds::new(
                        point(bounds.origin.x + px(width * from), top),
                        size(px((width * (to - from)).max(3.0)), px(LANE_ROW)),
                    );
                    window.paint_quad(fill(rect, alpha(*color, if *selected { 0.9 } else { 0.5 })));
                    if *selected {
                        outline(window, rect, p.white, 1.0);
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id("effects-lane")
            .relative()
            .flex_none()
            .mx(px(GUTTER))
            .mb(px(GUTTER))
            .h(px(height))
            .cursor_pointer()
            .child(painter)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_lane_down))
            .into_any_element()
    }

    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let tool_button = |id: &'static str, text: &'static str, active: bool| {
            button(
                id,
                text,
                if active {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                },
                true,
                &p,
            )
        };
        let speed_index = SPEEDS
            .iter()
            .position(|speed| (*speed - self.edit.speed).abs() < f32::EPSILON)
            .unwrap_or(DEFAULT_SPEED);
        let names = SPEEDS
            .iter()
            .map(|speed| SharedString::from(format!("{speed}x")))
            .collect();
        let entity = cx.entity();
        let speed = Dropdown::new("speed", names, speed_index)
            .shown(self.shown(Picker::Speed, cx))
            .on_toggle({
                let entity = entity.clone();
                move |window, cx| {
                    entity.update(cx, |editor, cx| {
                        editor.toggle_picker(Picker::Speed, window, cx)
                    })
                }
            })
            .on_select(move |index, window, cx| {
                entity.update(cx, |editor, cx| {
                    editor.set_speed(index, cx);
                    editor.close_picker(window, cx);
                })
            });
        let cropping = self.tool == Tool::Crop;
        let drawing = self.tool == Tool::Draw;
        let swatches = drawing.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .children(INKS.iter().enumerate().map(|(index, color)| {
                    let chosen = self.ink % INKS.len() == index;
                    div()
                        .id(("ink", index))
                        .size(px(SWATCH))
                        .rounded_full()
                        .bg(ink(*color))
                        .border_2()
                        .border_color(if chosen { p.white } else { p.stage })
                        .cursor_pointer()
                        .on_click(cx.listener(move |editor, _, _, cx| {
                            editor.ink = index;
                            cx.notify();
                        }))
                }))
        });
        let sizes = drawing.then(|| {
            let step = |id: &'static str, glyph: Icon, thicker: bool| {
                square_button(id, glyph, false, &p).on_click(
                    cx.listener(move |editor, _, _, cx| editor.step_ink_size(thicker, cx)),
                )
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(step("ink-thinner", Icon::Minus, false))
                .child(
                    div()
                        .w(px(INK_SIZE_WIDTH))
                        .text_center()
                        .text_size(px(CONTROL_TEXT))
                        .text_color(p.text)
                        .child((self.ink_size + 1).to_string()),
                )
                .child(step("ink-thicker", Icon::Plus, true))
        });
        let selected = self.selected.is_some();
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .px(px(GUTTER))
            .pt(px(10.0))
            .child(
                tool_button("tool-crop", "Кадр", cropping)
                    .on_click(cx.listener(|editor, _, _, cx| editor.toggle_tool(Tool::Crop, cx))),
            )
            .when(self.edit.crop.is_some(), |bar| {
                bar.child(
                    tool_button("tool-crop-reset", "Сбросить кадр", false)
                        .on_click(cx.listener(|editor, _, _, cx| editor.reset_crop(cx))),
                )
            })
            .child(
                tool_button("tool-blur", "Размыть", false)
                    .on_click(cx.listener(|editor, _, _, cx| editor.add_blur(cx))),
            )
            .child(
                tool_button("tool-draw", "Рисовать", drawing)
                    .on_click(cx.listener(|editor, _, _, cx| editor.toggle_tool(Tool::Draw, cx))),
            )
            .children(swatches)
            .children(sizes)
            .child(
                tool_button("tool-mute", "Заглушить", false)
                    .on_click(cx.listener(|editor, _, _, cx| editor.add_mute(cx))),
            )
            .when(selected, |bar| {
                bar.child(
                    button(
                        "tool-delete",
                        "Удалить эффект",
                        ButtonKind::Danger,
                        true,
                        &p,
                    )
                    .on_click(cx.listener(|editor, _, _, cx| {
                        editor.delete_selected(cx);
                    })),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon(Icon::Play, ICON, p.icon))
                    .child(speed),
            )
            .into_any_element()
    }

    fn fader(&self, fader: Fader, cx: &mut Context<Self>) -> AnyElement {
        let Fader {
            id,
            title,
            value,
            muted,
            enabled,
            set,
            mute,
        } = fader;
        let p = cx.palette();
        let entity = cx.entity();
        let percent = (value * PERCENT).round() as u32;
        let slider = Slider::new(id, percent, 0, (GAIN_MAX * PERCENT) as u32, GAIN_STEP)
            .disabled(!enabled || muted)
            .on_input(move |value, _, cx| {
                entity.update(cx, |editor, cx| {
                    set(&mut editor.edit.mix, value as f32 / PERCENT);
                    editor.sync_audio();
                    cx.notify();
                })
            });
        div()
            .flex()
            .flex_col()
            .px(px(GUTTER))
            .py(px(6.0))
            .child(label(title, if enabled { p.text } else { p.disabled }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .pt(px(6.0))
                    .child(
                        div()
                            .id(id)
                            .flex_none()
                            .cursor_pointer()
                            .child(icon(
                                if muted { Icon::MicOff } else { Icon::Audio },
                                ICON,
                                if muted { p.record } else { p.icon },
                            ))
                            .when(enabled, |toggle| {
                                toggle.on_click(cx.listener(move |editor, _, _, cx| {
                                    mute(&mut editor.edit.mix);
                                    editor.sync_audio();
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(slider)
                    .child(value_box(
                        if muted {
                            "выкл".to_owned()
                        } else {
                            format!("{percent}%")
                        },
                        &p,
                        VALUE_WIDTH,
                    )),
            )
            .into_any_element()
    }

    pub(super) fn mixer_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let mix = self.edit.mix;
        let separate = self.audio_tracks > 1;
        let master = self.fader(
            Fader {
                id: "mix-master",
                title: "Общая громкость",
                value: mix.master,
                muted: mix.master_muted,
                enabled: self.audio_tracks > 0,
                set: |mix, value| mix.master = value,
                mute: |mix| mix.master_muted = !mix.master_muted,
            },
            cx,
        );
        let game = self.fader(
            Fader {
                id: "mix-game",
                title: "Звук игры",
                value: mix.game,
                muted: mix.game_muted,
                enabled: separate,
                set: |mix, value| mix.game = value,
                mute: |mix| mix.game_muted = !mix.game_muted,
            },
            cx,
        );
        let mic = self.fader(
            Fader {
                id: "mix-mic",
                title: "Микрофон",
                value: mix.mic,
                muted: mix.mic_muted,
                enabled: separate,
                set: |mix, value| mix.mic = value,
                mute: |mix| mix.mic_muted = !mix.mic_muted,
            },
            cx,
        );
        let discord = self.fader(
            Fader {
                id: "mix-discord",
                title: "Discord",
                value: mix.discord,
                muted: mix.discord_muted,
                enabled: self.audio_tracks > 2,
                set: |mix, value| mix.discord = value,
                mute: |mix| mix.discord_muted = !mix.discord_muted,
            },
            cx,
        );
        let note = if self.audio_tracks == 0 {
            Some("В клипе нет звука.")
        } else if !separate {
            Some(
                "Клип записан одной дорожкой: игра и микрофон уже смешаны. Раздельные дорожки включаются в настройках аудио.",
            )
        } else {
            None
        };
        let mutes = self.edit.mutes.len();
        div()
            .flex()
            .flex_col()
            .child(section("Звук", &p))
            .child(master)
            .child(game)
            .child(mic)
            .child(discord)
            .when_some(note, |column, note| {
                column.child(
                    div()
                        .px(px(GUTTER))
                        .pt(px(4.0))
                        .text_size(px(SMALL_TEXT))
                        .text_color(p.muted)
                        .child(note),
                )
            })
            .when(mutes > 0, |column| {
                column.child(
                    div()
                        .px(px(GUTTER))
                        .pt(px(6.0))
                        .text_size(px(CONTROL_TEXT))
                        .text_color(p.text)
                        .child(format!("Заглушённых участков: {mutes}")),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brush_size_stays_within_its_steps_and_each_step_is_thicker() {
        assert_eq!(ink_size_after(0, false), 0);
        assert_eq!(ink_size_after(0, true), 1);
        let last = INK_WIDTHS.len() - 1;
        assert_eq!(ink_size_after(last, true), last);
        assert!(INK_WIDTHS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(ink_width(99), INK_WIDTHS[last]);
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn the_picture_is_letterboxed_inside_the_stage() {
        let picture = picture_rect(rect(0.0, 0.0, 1000.0, 1000.0), (1920, 1080));
        assert_eq!(f32::from(picture.size.width), 1000.0);
        assert!((f32::from(picture.size.height) - 562.5).abs() < 0.01);
        assert!((f32::from(picture.origin.y) - 218.75).abs() < 0.01);
    }

    #[test]
    fn points_map_to_frame_units_and_back() {
        let picture = rect(100.0, 50.0, 400.0, 200.0);
        let unit = to_unit(picture, point(px(300.0), px(150.0)));
        assert_eq!(unit, (0.5, 0.5));
        assert_eq!(to_unit(picture, point(px(0.0), px(999.0))), (0.0, 1.0));
    }

    #[test]
    fn dragging_a_new_frame_keeps_the_starting_corner() {
        let start = (0.2, 0.3);
        let mut area = Area::from_corners(start, start);
        let mut corner = Corner::BottomRight;
        for step in 1..=10 {
            let unit = (0.2 + step as f32 * 0.05, 0.3 + step as f32 * 0.04);
            let (next, flipped) = drag_corner(area, corner, unit);
            area = next;
            corner = flipped;
        }
        assert!(
            (area.x - 0.2).abs() < 1e-4 && (area.y - 0.3).abs() < 1e-4,
            "{area:?}"
        );
        assert!((area.right() - 0.7).abs() < 1e-4 && (area.bottom() - 0.7).abs() < 1e-4);
    }

    #[test]
    fn corners_are_grabbed_and_resize_from_the_opposite_side() {
        let picture = rect(0.0, 0.0, 1000.0, 1000.0);
        let area = Area {
            x: 0.2,
            y: 0.2,
            width: 0.4,
            height: 0.4,
        };
        assert_eq!(
            corner_at(area, picture, point(px(605.0), px(598.0))),
            Some(Corner::BottomRight)
        );
        assert_eq!(corner_at(area, picture, point(px(400.0), px(400.0))), None);
        let (grown, corner) = drag_corner(area, Corner::BottomRight, (0.9, 0.8));
        assert_eq!(corner, Corner::BottomRight);
        assert!((grown.x - 0.2).abs() < 1e-6 && (grown.right() - 0.9).abs() < 1e-6);
        assert!((grown.bottom() - 0.8).abs() < 1e-6);
    }
}
