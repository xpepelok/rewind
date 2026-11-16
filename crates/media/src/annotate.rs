use crate::edit::Area;
use crate::effects::{Image, blur, blur_polyline, crop, paint_polyline};

pub const MARKER_OPACITY: f32 = 0.35;
pub const LINE_HEIGHT: f32 = 1.33;
const GLYPH_WIDTH: f32 = 0.56;
const ELLIPSE_SEGMENTS: usize = 96;
const ARROW_HEAD: f32 = 4.5;
const ARROW_MIN_HEAD: f32 = 12.0;
const ARROW_ANGLE: f32 = 0.5;
const DARK_LUMA: f32 = 0.6;
const MIN_BLOCK: u32 = 2;

pub type TextPainter = dyn Fn(&mut Image, &str, (f32, f32), f32, &TextStyle, Anchor);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Pen,
    Marker,
    Line,
    Arrow,
    Frame,
    Ellipse,
}

impl Shape {
    pub const fn is_freehand(self) -> bool {
        matches!(self, Self::Pen)
    }

    pub const fn opacity(self) -> f32 {
        match self {
            Self::Marker => MARKER_OPACITY,
            _ => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    Center,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub font: String,
    pub color: [u8; 3],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub align: Align,
    pub shadow: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: "Segoe UI".to_owned(),
            color: [255, 255, 255],
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            align: Align::Left,
            shadow: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Stroke {
        shape: Shape,
        points: Vec<(f32, f32)>,
        color: [u8; 3],
        width: f32,
    },
    Fill {
        area: Area,
        color: [u8; 3],
        radius: f32,
    },
    Blur(Area),
    BlurStroke {
        points: Vec<(f32, f32)>,
        width: f32,
    },
    Pixelate {
        area: Area,
        block: f32,
    },
    Invert(Area),
    Text {
        at: (f32, f32),
        text: String,
        size: f32,
        style: TextStyle,
    },
    Counter {
        center: (f32, f32),
        tail: Option<(f32, f32)>,
        number: u32,
        radius: f32,
        color: [u8; 3],
    },
}

pub fn ink_on(color: [u8; 3]) -> [u8; 3] {
    let luma =
        (0.299 * f32::from(color[0]) + 0.587 * f32::from(color[1]) + 0.114 * f32::from(color[2]))
            / 255.0;
    if luma < DARK_LUMA {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    }
}

pub fn text_extent(text: &str, size: f32) -> (f32, f32) {
    let lines = text.split('\n').count().max(1) as f32;
    let longest = text
        .split('\n')
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0) as f32;
    (
        longest.max(1.0) * size * GLYPH_WIDTH,
        lines * size * LINE_HEIGHT,
    )
}

pub fn counter_size(number: u32, radius: f32) -> f32 {
    radius * if number >= 100 { 0.9 } else { 1.15 }
}

impl Mark {
    pub const fn is_effect(&self) -> bool {
        matches!(
            self,
            Self::Blur(_) | Self::BlurStroke { .. } | Self::Pixelate { .. } | Self::Invert(_)
        )
    }

    pub fn color(&self) -> Option<[u8; 3]> {
        match self {
            Self::Stroke { color, .. } | Self::Fill { color, .. } | Self::Counter { color, .. } => {
                Some(*color)
            }
            Self::Text { style, .. } => Some(style.color),
            Self::Blur(_) | Self::BlurStroke { .. } | Self::Pixelate { .. } | Self::Invert(_) => {
                None
            }
        }
    }

    pub fn set_color(&mut self, value: [u8; 3]) {
        match self {
            Self::Stroke { color, .. } | Self::Fill { color, .. } | Self::Counter { color, .. } => {
                *color = value
            }
            Self::Text { style, .. } => style.color = value,
            Self::Blur(_) | Self::BlurStroke { .. } | Self::Pixelate { .. } | Self::Invert(_) => {}
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let shift = |point: &mut (f32, f32)| {
            point.0 += dx;
            point.1 += dy;
        };
        match self {
            Self::Stroke { points, .. } | Self::BlurStroke { points, .. } => {
                points.iter_mut().for_each(shift)
            }
            Self::Text { at, .. } => shift(at),
            Self::Counter { center, tail, .. } => {
                shift(center);
                if let Some(tail) = tail {
                    shift(tail);
                }
            }
            Self::Fill { area, .. }
            | Self::Blur(area)
            | Self::Pixelate { area, .. }
            | Self::Invert(area) => {
                area.x += dx;
                area.y += dy;
            }
        }
    }

    pub fn reach(&self, aspect: f32) -> (f32, f32, f32, f32) {
        let span = |points: &[(f32, f32)], pad_x: f32, pad_y: f32| {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (x, y) in points {
                x0 = x0.min(*x);
                y0 = y0.min(*y);
                x1 = x1.max(*x);
                y1 = y1.max(*y);
            }
            (x0 - pad_x, y0 - pad_y, x1 + pad_x, y1 + pad_y)
        };
        let aspect = aspect.max(f32::EPSILON);
        match self {
            Self::Stroke { points, width, .. } | Self::BlurStroke { points, width } => {
                span(points, width / 2.0 / aspect, width / 2.0)
            }
            Self::Counter {
                center,
                tail,
                radius,
                ..
            } => {
                let mut points = vec![*center];
                points.extend(tail.iter().copied());
                span(&points, radius / aspect, *radius)
            }
            Self::Text { at, text, size, .. } => {
                let (width, height) = text_extent(text, *size);
                (at.0, at.1, at.0 + width / aspect, at.1 + height)
            }
            Self::Fill { area, .. }
            | Self::Blur(area)
            | Self::Pixelate { area, .. }
            | Self::Invert(area) => (area.x, area.y, area.right(), area.bottom()),
        }
    }
}

fn arrow_head(from: (f32, f32), to: (f32, f32), thickness: f32) -> Vec<(f32, f32)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f32::EPSILON {
        return Vec::new();
    }
    let head = (thickness * ARROW_HEAD)
        .max(ARROW_MIN_HEAD)
        .min(length * 0.6);
    let angle = dy.atan2(dx);
    let wing = |side: f32| {
        let theta = angle + std::f32::consts::PI - side * ARROW_ANGLE;
        (to.0 + head * theta.cos(), to.1 + head * theta.sin())
    };
    vec![wing(1.0), to, wing(-1.0)]
}

pub fn polylines(shape: Shape, points: &[(f32, f32)], thickness: f32) -> Vec<Vec<(f32, f32)>> {
    if shape.is_freehand() {
        return vec![points.to_vec()];
    }
    let (Some(from), Some(to)) = (points.first().copied(), points.last().copied()) else {
        return Vec::new();
    };
    match shape {
        Shape::Line | Shape::Marker => vec![vec![from, to]],
        Shape::Arrow => vec![vec![from, to], arrow_head(from, to, thickness)],
        Shape::Frame => vec![vec![from, (to.0, from.1), to, (from.0, to.1), from]],
        Shape::Ellipse => {
            let center = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
            let radius = ((to.0 - from.0).abs() / 2.0, (to.1 - from.1).abs() / 2.0);
            vec![
                (0..=ELLIPSE_SEGMENTS)
                    .map(|step| {
                        let theta = std::f32::consts::TAU * step as f32 / ELLIPSE_SEGMENTS as f32;
                        (
                            center.0 + radius.0 * theta.cos(),
                            center.1 + radius.1 * theta.sin(),
                        )
                    })
                    .collect(),
            ]
        }
        Shape::Pen => Vec::new(),
    }
}

pub fn stroke_thickness(width: f32, height: u32) -> f32 {
    (width * height as f32).max(1.0)
}

pub fn counter_outline(
    center: (f32, f32),
    tail: Option<(f32, f32)>,
    radius: f32,
) -> Option<[(f32, f32); 3]> {
    let tail = tail?;
    let distance = ((tail.0 - center.0).powi(2) + (tail.1 - center.1).powi(2)).sqrt();
    if distance <= radius {
        return None;
    }
    let angle = (tail.1 - center.1).atan2(tail.0 - center.0);
    let side = |offset: f32| {
        (
            center.0 + radius * (angle + offset).cos(),
            center.1 + radius * (angle + offset).sin(),
        )
    };
    Some([
        side(std::f32::consts::FRAC_PI_2),
        side(-std::f32::consts::FRAC_PI_2),
        tail,
    ])
}

fn blend_pixel(image: &mut Image, x: u32, y: u32, color: [u8; 3], amount: f32) {
    if amount <= 0.0 || x >= image.width || y >= image.height {
        return;
    }
    let at = (y as usize * image.width as usize + x as usize) * 4;
    let [red, green, blue] = color;
    for (channel, target) in image.pixels[at..at + 3].iter_mut().zip([blue, green, red]) {
        *channel = (f32::from(*channel) * (1.0 - amount) + f32::from(target) * amount) as u8;
    }
}

fn fill_rounded(image: &mut Image, area: Area, radius: f32, color: [u8; 3]) {
    let rect = area.pixels(image.width, image.height);
    let (left, top) = (rect.x as f32, rect.y as f32);
    let (right, bottom) = (left + rect.width as f32, top + rect.height as f32);
    let radius = radius
        .min(rect.width as f32 / 2.0)
        .min(rect.height as f32 / 2.0)
        .max(0.0);
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let coverage = if radius < 0.5 {
                1.0
            } else {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let cx = px.clamp(left + radius, right - radius);
                let cy = py.clamp(top + radius, bottom - radius);
                let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                (radius - distance + 0.5).clamp(0.0, 1.0)
            };
            blend_pixel(image, x, y, color, coverage);
        }
    }
}

fn pixelate(image: &mut Image, area: Area, block: u32) {
    let rect = area.pixels(image.width, image.height);
    let block = block.max(MIN_BLOCK);
    let stride = image.width as usize * 4;
    let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
    for y in (rect.y..bottom).step_by(block as usize) {
        let y1 = (y + block).min(bottom);
        for x in (rect.x..right).step_by(block as usize) {
            let x1 = (x + block).min(right);
            let mut sum = [0u32; 3];
            for yy in y..y1 {
                for xx in x..x1 {
                    let at = yy as usize * stride + xx as usize * 4;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(image.pixels[at + channel]);
                    }
                }
            }
            let count = ((x1 - x) * (y1 - y)).max(1);
            let average = sum.map(|total| (total / count) as u8);
            for yy in y..y1 {
                for xx in x..x1 {
                    let at = yy as usize * stride + xx as usize * 4;
                    image.pixels[at..at + 3].copy_from_slice(&average);
                }
            }
        }
    }
}

fn invert(image: &mut Image, area: Area) {
    let rect = area.pixels(image.width, image.height);
    let stride = image.width as usize * 4;
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let at = y as usize * stride + x as usize * 4;
            for channel in &mut image.pixels[at..at + 3] {
                *channel = 255 - *channel;
            }
        }
    }
}

fn inside_triangle(p: (f32, f32), [a, b, c]: [(f32, f32); 3]) -> bool {
    let side = |p1: (f32, f32), p2: (f32, f32)| {
        (p.0 - p2.0) * (p1.1 - p2.1) - (p1.0 - p2.0) * (p.1 - p2.1)
    };
    let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

fn counter(
    image: &mut Image,
    center: (f32, f32),
    tail: Option<(f32, f32)>,
    radius: f32,
    color: [u8; 3],
) {
    let wedge = counter_outline(center, tail, radius);
    let (mut x0, mut y0, mut x1, mut y1) = (
        center.0 - radius,
        center.1 - radius,
        center.0 + radius,
        center.1 + radius,
    );
    if let Some([_, _, tip]) = wedge {
        x0 = x0.min(tip.0);
        y0 = y0.min(tip.1);
        x1 = x1.max(tip.0);
        y1 = y1.max(tip.1);
    }
    let (width, height) = (image.width, image.height);
    let clamp_x = |value: f32| (value.max(0.0) as u32).min(width);
    let clamp_y = |value: f32| (value.max(0.0) as u32).min(height);
    for y in clamp_y(y0 - 1.0)..clamp_y(y1 + 2.0) {
        for x in clamp_x(x0 - 1.0)..clamp_x(x1 + 2.0) {
            let point = (x as f32 + 0.5, y as f32 + 0.5);
            let distance = ((point.0 - center.0).powi(2) + (point.1 - center.1).powi(2)).sqrt();
            let mut coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            if wedge.is_some_and(|wedge| inside_triangle(point, wedge)) {
                coverage = 1.0;
            }
            blend_pixel(image, x, y, color, coverage);
        }
    }
}

pub fn render(
    mut pixels: Vec<u8>,
    width: u32,
    height: u32,
    marks: &[Mark],
    frame: Option<Area>,
    text: &TextPainter,
) -> (Vec<u8>, u32, u32) {
    let mut image = Image {
        pixels: &mut pixels,
        width,
        height,
    };
    let h = height as f32;
    let scale = |unit: (f32, f32)| (unit.0 * width as f32, unit.1 * h);
    for mark in marks {
        match mark {
            Mark::Blur(area) => blur(&mut image, *area),
            Mark::BlurStroke {
                points,
                width: line,
            } => {
                let points: Vec<(f32, f32)> = points.iter().map(|point| scale(*point)).collect();
                blur_polyline(&mut image, &points, stroke_thickness(*line, height));
            }
            Mark::Pixelate { area, block } => {
                pixelate(&mut image, *area, (block * h).round() as u32)
            }
            Mark::Invert(area) => invert(&mut image, *area),
            Mark::Fill {
                area,
                color,
                radius,
            } => fill_rounded(&mut image, *area, radius * h, *color),
            Mark::Stroke {
                shape,
                points,
                color,
                width: line,
            } => {
                let thickness = stroke_thickness(*line, height);
                let points: Vec<(f32, f32)> = points.iter().map(|point| scale(*point)).collect();
                for polyline in polylines(*shape, &points, thickness) {
                    paint_polyline(&mut image, &polyline, thickness, *color, shape.opacity());
                }
            }
            Mark::Text {
                at,
                text: content,
                size,
                style,
            } => text(
                &mut image,
                content,
                scale(*at),
                size * h,
                style,
                Anchor::TopLeft,
            ),
            Mark::Counter {
                center,
                tail,
                number,
                radius,
                color,
            } => {
                let center = scale(*center);
                let radius = radius * h;
                counter(&mut image, center, tail.map(scale), radius, *color);
                let style = TextStyle {
                    color: ink_on(*color),
                    bold: true,
                    ..TextStyle::default()
                };
                text(
                    &mut image,
                    &number.to_string(),
                    center,
                    counter_size(*number, radius),
                    &style,
                    Anchor::Center,
                );
            }
        }
    }
    match frame.filter(|area| !area.is_full()) {
        Some(area) => {
            let rect = area.pixels(width, height);
            (crop(&pixels, width, rect), rect.width, rect.height)
        }
        None => (pixels, width, height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_text(_: &mut Image, _: &str, _: (f32, f32), _: f32, _: &TextStyle, _: Anchor) {}

    fn black(width: u32, height: u32) -> Vec<u8> {
        vec![0u8; (width * height * 4) as usize]
    }

    fn at(width: u32, x: u32, y: u32) -> usize {
        ((y * width + x) * 4) as usize
    }

    const SIDE: u32 = 40;

    const HALF: Area = Area {
        x: 0.0,
        y: 0.0,
        width: 0.5,
        height: 1.0,
    };

    #[test]
    fn arrows_get_a_two_winged_head_at_the_tip() {
        let lines = polylines(Shape::Arrow, &[(0.0, 0.0), (100.0, 0.0)], 4.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1][1], (100.0, 0.0));
        let (left, right) = (lines[1][0], lines[1][2]);
        assert!(left.0 < 100.0 && right.0 < 100.0);
        assert!(left.1 * right.1 < 0.0);
    }

    #[test]
    fn frames_and_ellipses_are_closed() {
        let frame = &polylines(Shape::Frame, &[(10.0, 10.0), (50.0, 30.0)], 2.0)[0];
        assert_eq!(frame.first(), frame.last());
        let ellipse = &polylines(Shape::Ellipse, &[(0.0, 0.0), (40.0, 20.0)], 2.0)[0];
        let (first, last) = (ellipse[0], ellipse[ellipse.len() - 1]);
        assert!((first.0 - last.0).abs() < 1e-3 && (first.1 - last.1).abs() < 1e-3);
    }

    #[test]
    fn the_marker_is_a_straight_stroke() {
        let lines = polylines(Shape::Marker, &[(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)], 4.0);
        assert_eq!(lines, vec![vec![(0.0, 0.0), (10.0, 0.0)]]);
    }

    #[test]
    fn rendering_draws_then_crops() {
        let (width, height) = (40u32, 20u32);
        let mut pixels = black(width, height);
        let marks = [Mark::Stroke {
            shape: Shape::Line,
            points: vec![(0.0, 0.5), (1.0, 0.5)],
            color: [0, 255, 0],
            width: 0.2,
        }];
        let frame = Area {
            x: 0.5,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        };
        let (out, w, h) = render(pixels.clone(), width, height, &marks, Some(frame), &no_text);
        assert_eq!((w, h), (20, 20));
        assert_eq!(out[at(w, 10, 10) + 1], 255);
        assert_eq!(out[1], 0);
    }

    #[test]
    fn the_marker_is_translucent() {
        let (width, height) = (20u32, 20u32);
        let mut pixels = black(width, height);
        let marks = [Mark::Stroke {
            shape: Shape::Marker,
            points: vec![(0.1, 0.5), (0.9, 0.5)],
            color: [255, 255, 255],
            width: 0.3,
        }];
        let (out, _, _) = render(pixels.clone(), width, height, &marks, None, &no_text);
        let value = out[at(width, 10, 10)];
        assert!(value > 70 && value < 110, "{value}");
    }

    #[test]
    fn inverting_and_filling_stay_inside_their_area() {
        let (width, height) = (20u32, 20u32);
        let mut pixels = black(width, height);
        let (out, _, _) = render(
            pixels.clone(),
            width,
            height,
            &[Mark::Invert(HALF)],
            None,
            &no_text,
        );
        assert_eq!(out[at(width, 2, 10)], 255);
        assert_eq!(out[at(width, 15, 10)], 0);
        let mut pixels = black(width, height);
        let fill = Mark::Fill {
            area: HALF,
            color: [0, 0, 255],
            radius: 0.0,
        };
        let (out, _, _) = render(pixels.clone(), width, height, &[fill], None, &no_text);
        assert_eq!(out[at(width, 5, 10)], 255);
        assert_eq!(out[at(width, 15, 10)], 0);
    }

    #[test]
    fn pixelating_averages_whole_blocks() {
        let (width, height) = (20u32, 20u32);
        let mut pixels: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let value = if index % 2 == 0 { 0 } else { 200 };
                [value, value, value, 255]
            })
            .collect();
        let blocky = Mark::Pixelate {
            area: HALF,
            block: 0.2,
        };
        let (out, _, _) = render(pixels.clone(), width, height, &[blocky], None, &no_text);
        assert_eq!(out[at(width, 0, 0)], out[at(width, 1, 0)]);
        assert_eq!(out[at(width, 0, 0)], 100);
        assert_ne!(out[at(width, 14, 0)], out[at(width, 15, 0)]);
    }

    #[test]
    fn blur_strokes_are_effects_that_move_and_smear_their_path() {
        let (width, height) = (40u32, 40u32);
        let mut pixels: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let value = if (index % width / 2 + index / width / 2) % 2 == 0 {
                    0
                } else {
                    250
                };
                [value, value, value, 255]
            })
            .collect();
        let original = pixels.clone();
        let mut mark = Mark::BlurStroke {
            points: vec![(0.1, 0.4), (0.9, 0.4)],
            width: 0.3,
        };
        mark.translate(0.0, 0.1);
        assert!(mark.is_effect());
        assert_eq!(mark.color(), None);
        let (_, y0, _, y1) = mark.reach(1.0);
        assert!((y0 - 0.35).abs() < 1e-5 && (y1 - 0.65).abs() < 1e-5);
        let (out, _, _) = render(pixels.clone(), width, height, &[mark], None, &no_text);
        assert_ne!(out[at(width, 20, 20)], original[at(width, 20, 20)]);
        assert_eq!(out[at(width, 20, 2)], original[at(width, 20, 2)]);
    }

    fn red_line() -> Mark {
        Mark::Stroke {
            shape: Shape::Line,
            points: vec![(0.1, 0.5), (0.9, 0.5)],
            color: [255, 0, 0],
            width: 0.1,
        }
    }

    fn rendered(marks: &[Mark]) -> Vec<u8> {
        let mut pixels = black(SIDE, SIDE);
        render(pixels.clone(), SIDE, SIDE, marks, None, &no_text).0
    }

    fn red(out: &[u8], x: u32, y: u32) -> u8 {
        out[at(SIDE, x, y) + 2]
    }

    #[test]
    fn effects_apply_to_the_drawings_made_before_them() {
        let marks = [
            Mark::Fill {
                area: HALF,
                color: [255, 255, 255],
                radius: 0.0,
            },
            Mark::Invert(Area::FULL),
        ];
        let out = rendered(&marks);
        assert_eq!(red(&out, 5, 20), 0);
        assert_eq!(red(&out, 30, 20), 255);
    }

    #[test]
    fn a_blur_brush_smears_a_stroke_drawn_before_it() {
        let brush = Mark::BlurStroke {
            points: vec![(0.5, 0.2), (0.5, 0.8)],
            width: 0.3,
        };
        let out = rendered(&[red_line(), brush]);
        let smeared = red(&out, 20, 20);
        assert!(smeared > 0 && smeared < 255, "{smeared}");
        assert_eq!(red(&out, 6, 20), 255);
    }

    #[test]
    fn blurring_and_pixelating_an_area_soften_a_stroke_drawn_before_them() {
        let pixelate = Mark::Pixelate {
            area: HALF,
            block: 0.4,
        };
        for effect in [Mark::Blur(HALF), pixelate] {
            let out = rendered(&[red_line(), effect]);
            let softened = red(&out, 10, 20);
            assert!(softened > 0 && softened < 255, "{softened}");
            assert_eq!(red(&out, 30, 20), 255);
        }
    }

    #[test]
    fn a_stroke_drawn_after_an_effect_stays_sharp() {
        let brush = Mark::BlurStroke {
            points: vec![(0.5, 0.2), (0.5, 0.8)],
            width: 0.3,
        };
        let pixelate = Mark::Pixelate {
            area: Area::FULL,
            block: 0.4,
        };
        let out = rendered(&[Mark::Blur(Area::FULL), brush, pixelate, red_line()]);
        assert_eq!(red(&out, 20, 20), 255);
        assert_eq!(red(&out, 20, 10), 0);
    }

    #[test]
    fn counters_fill_a_circle_and_pick_a_readable_digit_color() {
        let (width, height) = (40u32, 40u32);
        let mut pixels = black(width, height);
        let mark = Mark::Counter {
            center: (0.5, 0.5),
            tail: Some((0.95, 0.95)),
            number: 3,
            radius: 0.2,
            color: [255, 0, 0],
        };
        let (out, _, _) = render(pixels.clone(), width, height, &[mark], None, &no_text);
        assert_eq!(out[at(width, 20, 20) + 2], 255);
        assert_eq!(out[at(width, 2, 2) + 2], 0);
        assert_eq!(out[at(width, 35, 35) + 2], 255);
        assert_eq!(ink_on([255, 0, 0]), [255, 255, 255]);
        assert_eq!(ink_on([255, 255, 0]), [0, 0, 0]);
    }

    #[test]
    fn marks_move_recolor_and_report_their_reach() {
        let mut mark = Mark::Counter {
            center: (0.2, 0.2),
            tail: Some((0.4, 0.4)),
            number: 1,
            radius: 0.05,
            color: [0, 0, 0],
        };
        mark.translate(0.1, 0.2);
        mark.set_color([1, 2, 3]);
        assert_eq!(mark.color(), Some([1, 2, 3]));
        let (x0, y0, x1, y1) = mark.reach(1.0);
        assert!((x0 - 0.25).abs() < 1e-5 && (y0 - 0.35).abs() < 1e-5);
        assert!((x1 - 0.55).abs() < 1e-5 && (y1 - 0.65).abs() < 1e-5);
        assert_eq!(Mark::Invert(HALF).color(), None);
    }

    #[test]
    fn text_extent_grows_with_lines_and_length() {
        let (one_w, one_h) = text_extent("ab", 10.0);
        let (two_w, two_h) = text_extent("abcd\nx", 10.0);
        assert!(two_w > one_w && two_h > one_h);
    }
}
