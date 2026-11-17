use gpui::{
    AnyElement, Bounds, Context, FontWeight, Hsla, PathBuilder, Pixels, Window, canvas, div, fill,
    point, prelude::*, px, size,
};

use rewind_media::{
    Align, Area, LINE_HEIGHT, Mark, Shape, TextStyle, counter_outline, counter_size, ink_on,
    polylines, stroke_thickness,
};

use super::{
    Annotate, Board, CROP_HANDLE, CROP_LINE_GRAB, Edge, FONTS, Tool, color_of, picture_rect,
    to_screen,
};
use crate::theme::{FONT_FAMILY, Palette, SMALL_TEXT, alpha};

const SHADE: f32 = 0.6;
const FROST: f32 = 0.28;
const COUNTER_SEGMENTS: usize = 64;

fn area_rect(picture: Bounds<Pixels>, area: Area) -> Bounds<Pixels> {
    let (x, y) = to_screen(picture, (area.x, area.y));
    Bounds::new(
        point(px(x), px(y)),
        size(
            picture.size.width * area.width,
            picture.size.height * area.height,
        ),
    )
}

fn outline(window: &mut Window, rect: Bounds<Pixels>, color: Hsla, thickness: f32) {
    let t = px(thickness);
    let (o, s) = (rect.origin, rect.size);
    for edge in [
        Bounds::new(o, size(s.width, t)),
        Bounds::new(point(o.x, o.y + s.height - t), size(s.width, t)),
        Bounds::new(o, size(t, s.height)),
        Bounds::new(point(o.x + s.width - t, o.y), size(t, s.height)),
    ] {
        window.paint_quad(fill(edge, color));
    }
}

fn paint_polygon(window: &mut Window, points: &[(f32, f32)], color: Hsla) {
    let mut path = PathBuilder::fill();
    let mut iter = points.iter();
    let Some(first) = iter.next() else {
        return;
    };
    path.move_to(point(px(first.0), px(first.1)));
    for next in iter {
        path.line_to(point(px(next.0), px(next.1)));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn paint_counter(
    window: &mut Window,
    center: (f32, f32),
    tail: Option<(f32, f32)>,
    radius: f32,
    color: Hsla,
) {
    let circle: Vec<(f32, f32)> = (0..COUNTER_SEGMENTS)
        .map(|step| {
            let theta = std::f32::consts::TAU * step as f32 / COUNTER_SEGMENTS as f32;
            (
                center.0 + radius * theta.cos(),
                center.1 + radius * theta.sin(),
            )
        })
        .collect();
    paint_polygon(window, &circle, color);
    if let Some(wedge) = counter_outline(center, tail, radius) {
        paint_polygon(window, &wedge, color);
    }
}

fn paint_stroke(
    window: &mut Window,
    shape: Shape,
    points: &[(f32, f32)],
    thickness: f32,
    color: Hsla,
) {
    for line in polylines(shape, points, thickness) {
        let mut path = PathBuilder::stroke(px(thickness));
        let mut iter = line.iter();
        if let Some(first) = iter.next() {
            path.move_to(point(px(first.0), px(first.1)));
            if line.len() == 1 {
                path.line_to(point(px(first.0 + 0.5), px(first.1)));
            }
            for next in iter {
                path.line_to(point(px(next.0), px(next.1)));
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, alpha(color, shape.opacity()));
        }
    }
}

pub fn overlay<T: Annotate>(
    board: &Board,
    cropping: bool,
    p: Palette,
    cx: &mut Context<T>,
) -> AnyElement {
    let entity = cx.entity().downgrade();
    let frame = board.frame().unwrap_or((16, 9));
    let marks = board.loose();
    let crop = board.crop;
    let selected = board
        .selected
        .and_then(|index| board.marks.get(index).cloned());
    let aspect = board.aspect();
    canvas(
        move |bounds, _, cx| {
            entity
                .update(cx, |host, _| host.board().stage = Some(bounds))
                .ok();
            bounds
        },
        move |_, stage, window, _| {
            let picture = picture_rect(stage, frame);
            let height = f32::from(picture.size.height);
            let screen = |points: &[(f32, f32)]| -> Vec<(f32, f32)> {
                points
                    .iter()
                    .map(|unit| to_screen(picture, *unit))
                    .collect()
            };
            for mark in &marks {
                match mark {
                    Mark::Blur(area) | Mark::Pixelate { area, .. } | Mark::Invert(area) => {
                        let rect = area_rect(picture, *area);
                        window.paint_quad(fill(rect, alpha(p.white, 0.3)));
                        outline(window, rect, alpha(p.white, 0.8), 1.0);
                    }
                    Mark::BlurStroke { points, width } => paint_stroke(
                        window,
                        Shape::Pen,
                        &screen(points),
                        stroke_thickness(*width, height.round().max(1.0) as u32),
                        alpha(p.muted, FROST),
                    ),
                    Mark::Fill {
                        area,
                        color,
                        radius,
                    } => {
                        window.paint_quad(
                            fill(area_rect(picture, *area), color_of(*color))
                                .corner_radii(px(radius * height)),
                        );
                    }
                    Mark::Stroke {
                        shape,
                        points,
                        color,
                        width,
                    } => {
                        let thickness = stroke_thickness(*width, height.round().max(1.0) as u32);
                        paint_stroke(window, *shape, &screen(points), thickness, color_of(*color));
                    }
                    Mark::Counter {
                        center,
                        tail,
                        radius,
                        color,
                        ..
                    } => paint_counter(
                        window,
                        to_screen(picture, *center),
                        tail.map(|tail| to_screen(picture, tail)),
                        radius * height,
                        color_of(*color),
                    ),
                    _ => {}
                }
            }
            if let Some(mark) = &selected {
                let (x0, y0, x1, y1) = mark.reach(aspect);
                let (left, top) = to_screen(picture, (x0, y0));
                let (right, bottom) = to_screen(picture, (x1, y1));
                let rect = Bounds::new(
                    point(px(left - 3.0), px(top - 3.0)),
                    size(px(right - left + 6.0), px(bottom - top + 6.0)),
                );
                outline(window, rect, p.accent, 1.5);
            }
            let Some(area) = crop.filter(|area| cropping || !area.is_full()) else {
                return;
            };
            let inner = area_rect(picture, area);
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
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

pub fn texts(board: &Board) -> Vec<AnyElement> {
    let (Some(stage), Some(picture)) = (board.stage, board.picture_bounds()) else {
        return Vec::new();
    };
    let height = f32::from(picture.size.height);
    let origin = (f32::from(stage.origin.x), f32::from(stage.origin.y));
    board
        .loose()
        .iter()
        .filter_map(|mark| match mark {
            Mark::Text {
                at,
                text,
                size,
                style,
            } => {
                let (x, y) = to_screen(picture, *at);
                Some(
                    styled_text(div(), style, size * height)
                        .absolute()
                        .left(px(x - origin.0))
                        .top(px(y - origin.1))
                        .text_color(color_of(style.color))
                        .whitespace_nowrap()
                        .child(text.clone())
                        .into_any_element(),
                )
            }
            Mark::Counter {
                center,
                number,
                radius,
                color,
                ..
            } => {
                let (x, y) = to_screen(picture, *center);
                let r = radius * height;
                Some(
                    div()
                        .absolute()
                        .left(px(x - r - origin.0))
                        .top(px(y - r - origin.1))
                        .size(px(r * 2.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(FONTS[0])
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(counter_size(*number, r)))
                        .text_color(color_of(ink_on(*color)))
                        .child(number.to_string())
                        .into_any_element(),
                )
            }
            _ => None,
        })
        .collect()
}

pub fn typing_field(board: &Board, p: &Palette) -> Option<AnyElement> {
    let typing = board.typing.as_ref()?;
    let (stage, picture) = (board.stage?, board.picture_bounds()?);
    let (x, y) = to_screen(picture, typing.at);
    let height = f32::from(picture.size.height) * typing.size;
    let style = TextStyle {
        color: board.color,
        ..board.style.clone()
    };
    Some(
        styled_text(div(), &style, height.max(12.0))
            .absolute()
            .left(px(x - f32::from(stage.origin.x) - 4.0))
            .top(px(y - f32::from(stage.origin.y) - 2.0))
            .min_w(px((height * 6.0).max(200.0)))
            .px(px(4.0))
            .py(px(2.0))
            .bg(alpha(p.stage, 0.35))
            .border_1()
            .border_color(p.accent)
            .text_color(color_of(board.color))
            .child(board.text.clone())
            .child(
                div()
                    .font_family(FONT_FAMILY)
                    .font_weight(FontWeight::NORMAL)
                    .text_size(px(SMALL_TEXT))
                    .text_color(p.muted)
                    .child("Ctrl+Enter — готово, Esc — отмена"),
            )
            .into_any_element(),
    )
}

pub fn crop_handles(board: &Board, p: &Palette) -> Vec<AnyElement> {
    if board.tool != Tool::Crop {
        return Vec::new();
    }
    let (Some(stage), Some(picture), Some(area)) =
        (board.stage, board.picture_bounds(), board.crop)
    else {
        return Vec::new();
    };
    let origin = (f32::from(stage.origin.x), f32::from(stage.origin.y));
    let (left, top) = to_screen(picture, (area.x, area.y));
    let (right, bottom) = to_screen(picture, (area.right(), area.bottom()));
    let (left, top) = (left - origin.0, top - origin.1);
    let (wide, tall) = (right - origin.0 - left, bottom - origin.1 - top);
    let lines = [
        (Edge::Left, left, top, 0.0, tall),
        (Edge::Right, left + wide, top, 0.0, tall),
        (Edge::Top, left, top, wide, 0.0),
        (Edge::Bottom, left, top + tall, wide, 0.0),
    ]
    .into_iter()
    .map(|(edge, x, y, width, height)| {
        div()
            .absolute()
            .left(px(x - CROP_LINE_GRAB))
            .top(px(y - CROP_LINE_GRAB))
            .w(px(width + CROP_LINE_GRAB * 2.0))
            .h(px(height + CROP_LINE_GRAB * 2.0))
            .cursor(edge.cursor())
            .into_any_element()
    });
    let handles = Edge::ALL.into_iter().map(|edge| {
        let (ux, uy) = edge.anchor(area);
        let (x, y) = to_screen(picture, (ux, uy));
        div()
            .absolute()
            .left(px(x - origin.0 - CROP_HANDLE / 2.0))
            .top(px(y - origin.1 - CROP_HANDLE / 2.0))
            .size(px(CROP_HANDLE))
            .bg(p.accent)
            .border_1()
            .border_color(p.white)
            .cursor(edge.cursor())
            .into_any_element()
    });
    lines.chain(handles).collect()
}

pub fn styled_text(element: gpui::Div, style: &TextStyle, size: f32) -> gpui::Div {
    element
        .font_family(style.font.clone())
        .font_weight(if style.bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        })
        .text_size(px(size))
        .line_height(px(size * LINE_HEIGHT))
        .when(style.italic, |text| text.italic())
        .when(style.underline, |text| text.underline())
        .when(style.strike, |text| text.line_through())
        .map(|text| match style.align {
            Align::Left => text.text_left(),
            Align::Center => text.text_center(),
            Align::Right => text.text_right(),
        })
}
