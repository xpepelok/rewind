use crate::edit::{Area, PixelRect, Stroke};

const BYTES: usize = 4;
const BLUR_PASSES: usize = 3;
const BLUR_DIVISOR: u32 = 14;
const BLUR_MIN_RADIUS: u32 = 4;
const BLUR_MAX_RADIUS: u32 = 48;
const BLUR_COST: usize = 12;

pub struct Image<'a> {
    pub pixels: &'a mut [u8],
    pub width: u32,
    pub height: u32,
}

const RECIPROCAL_SHIFT: u32 = 23;

fn reciprocal(radius: usize) -> u32 {
    debug_assert!(radius <= BLUR_MAX_RADIUS as usize);
    (1u32 << RECIPROCAL_SHIFT) / (2 * radius as u32 + 1) + 1
}

fn box_pass(line: &mut [[u8; 3]], scratch: &mut Vec<[u8; 3]>, radius: usize) {
    let len = line.len();
    if len < 2 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(line);
    let last = len - 1;
    let scale = reciprocal(radius);
    let mut sum = [0u32; 3];
    for offset in -(radius as isize)..=(radius as isize) {
        let value = scratch[offset.clamp(0, last as isize) as usize];
        for channel in 0..3 {
            sum[channel] += u32::from(value[channel]);
        }
    }
    for (index, value) in line.iter_mut().enumerate() {
        for channel in 0..3 {
            value[channel] = ((sum[channel] * scale) >> RECIPROCAL_SHIFT) as u8;
        }
        let add = scratch[(index + radius + 1).min(last)];
        let remove = scratch[index.saturating_sub(radius)];
        for channel in 0..3 {
            sum[channel] = sum[channel] + u32::from(add[channel]) - u32::from(remove[channel]);
        }
    }
}

pub fn blur_radius(rect: PixelRect) -> u32 {
    (rect.width.max(rect.height) / BLUR_DIVISOR).clamp(BLUR_MIN_RADIUS, BLUR_MAX_RADIUS)
}

fn blurred_block(image: &Image, rect: PixelRect, radius: usize) -> Vec<[u8; 3]> {
    let stride = image.width as usize * BYTES;
    let (w, h) = (rect.width as usize, rect.height as usize);
    let mut block = vec![[0u8; 3]; w * h];
    if w == 0 || h == 0 {
        return block;
    }
    let pixels = &*image.pixels;
    let cost = block.len() * BLUR_COST;
    rows_parallel_with(&mut block, w, cost, |row, line| {
        let start = (rect.y as usize + row) * stride + rect.x as usize * BYTES;
        let source = &pixels[start..start + w * BYTES];
        for (value, pixel) in line.iter_mut().zip(source.as_chunks::<BYTES>().0) {
            *value = [pixel[0], pixel[1], pixel[2]];
        }
    });
    let mut copy = Vec::with_capacity(w * h);
    for _ in 0..BLUR_PASSES {
        blur_lines(&mut block, w, radius);
        blur_columns(&mut block, &mut copy, w, radius);
    }
    block
}

fn blur_lines(block: &mut [[u8; 3]], length: usize, radius: usize) {
    let lines = block.len() / length;
    let threads = workers(block.len() * BLUR_COST).min(lines);
    if threads <= 1 {
        let mut scratch = Vec::with_capacity(length);
        for line in block.chunks_mut(length) {
            box_pass(line, &mut scratch, radius);
        }
        return;
    }
    let per = lines.div_ceil(threads);
    std::thread::scope(|scope| {
        for part in block.chunks_mut(per * length) {
            scope.spawn(move || {
                let mut scratch = Vec::with_capacity(length);
                for line in part.chunks_mut(length) {
                    box_pass(line, &mut scratch, radius);
                }
            });
        }
    });
}

fn blur_columns(block: &mut [[u8; 3]], copy: &mut Vec<[u8; 3]>, width: usize, radius: usize) {
    let height = block.len() / width;
    if height < 2 {
        return;
    }
    copy.clear();
    copy.extend_from_slice(block);
    let source = &copy[..];
    let last = height - 1;
    let scale = reciprocal(radius);
    let row = |index: usize| &source[index * width..(index + 1) * width];
    let run = |first: usize, part: &mut [[u8; 3]]| {
        let mut sums = vec![[0u32; 3]; width];
        for offset in -(radius as isize)..=(radius as isize) {
            let line = row((first as isize + offset).clamp(0, last as isize) as usize);
            for (sum, value) in sums.iter_mut().zip(line) {
                for channel in 0..3 {
                    sum[channel] += u32::from(value[channel]);
                }
            }
        }
        for (step, target) in part.chunks_exact_mut(width).enumerate() {
            let index = first + step;
            for (value, sum) in target.iter_mut().zip(&sums) {
                for channel in 0..3 {
                    value[channel] = ((sum[channel] * scale) >> RECIPROCAL_SHIFT) as u8;
                }
            }
            let add = row((index + radius + 1).min(last));
            let remove = row(index.saturating_sub(radius));
            for ((sum, plus), minus) in sums.iter_mut().zip(add).zip(remove) {
                for channel in 0..3 {
                    sum[channel] =
                        sum[channel] + u32::from(plus[channel]) - u32::from(minus[channel]);
                }
            }
        }
    };
    let threads = workers(block.len() * BLUR_COST).min(height);
    if threads <= 1 {
        run(0, block);
        return;
    }
    let per = height.div_ceil(threads);
    let run = &run;
    std::thread::scope(|scope| {
        for (index, part) in block.chunks_mut(per * width).enumerate() {
            scope.spawn(move || run(index * per, part));
        }
    });
}

pub fn blur(image: &mut Image, area: Area) {
    let rect = area.pixels(image.width, image.height);
    let radius = blur_radius(rect) as usize;
    let block = blurred_block(image, rect, radius);
    let stride = image.width as usize * BYTES;
    let w = rect.width as usize;
    let (left, top) = (rect.x as usize * BYTES, rect.y as usize * stride);
    let rows = &mut image.pixels[top..top + rect.height as usize * stride];
    rows_parallel_with(rows, stride, block.len() * BYTES, |row, line| {
        let values = &block[row * w..(row + 1) * w];
        for (pixel, value) in line[left..left + w * BYTES]
            .as_chunks_mut::<BYTES>()
            .0
            .iter_mut()
            .zip(values)
        {
            pixel[..3].copy_from_slice(value);
        }
    });
}

pub fn brush_blur_radius(thickness: f32) -> u32 {
    ((thickness / 2.0) as u32).clamp(BLUR_MIN_RADIUS, BLUR_MAX_RADIUS)
}

pub fn blur_polyline(image: &mut Image, points: &[(f32, f32)], thickness: f32) {
    let Some(mask) = Coverage::of(points, thickness, image.width, image.height) else {
        return;
    };
    let radius = brush_blur_radius(thickness);
    let reach = radius * BLUR_PASSES as u32;
    let (x, y) = (mask.x0.saturating_sub(reach), mask.y0.saturating_sub(reach));
    let rect = PixelRect {
        x,
        y,
        width: (mask.x1 + reach).min(image.width) - x,
        height: (mask.y1 + reach).min(image.height) - y,
    };
    let block = blurred_block(image, rect, radius as usize);
    let stride = image.width as usize * BYTES;
    let w = rect.width as usize;
    let rows = &mut image.pixels[mask.y0 as usize * stride..mask.y1 as usize * stride];
    let cost = rows.len();
    rows_parallel_with(rows, stride, cost, |offset, line| {
        let row = mask.y0 + offset as u32;
        for column in mask.x0..mask.x1 {
            let amount = mask.at(column, row);
            if amount <= 0.0 {
                continue;
            }
            let value = block[(row - rect.y) as usize * w + (column - rect.x) as usize];
            let at = column as usize * BYTES;
            for (channel, target) in line[at..at + 3].iter_mut().zip(value) {
                *channel =
                    (f32::from(*channel) * (1.0 - amount) + f32::from(target) * amount) as u8;
            }
        }
    });
}

fn distance_to_segment(point: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length <= f32::EPSILON {
        0.0
    } else {
        (((point.0 - a.0) * dx + (point.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    ((point.0 - x).powi(2) + (point.1 - y).powi(2)).sqrt()
}

pub fn stroke_pixels(stroke: &Stroke, width: u32, height: u32) -> (Vec<(f32, f32)>, f32) {
    let points = stroke
        .points
        .iter()
        .map(|(x, y)| (x * width as f32, y * height as f32))
        .collect();
    (points, (stroke.width * height as f32).max(1.0))
}

pub(crate) fn stroke_mask(stroke: &Stroke, width: u32, height: u32) -> Option<Coverage> {
    let (points, thickness) = stroke_pixels(stroke, width, height);
    Coverage::of(&points, thickness, width, height)
}

pub fn draw(image: &mut Image, stroke: &Stroke) {
    if let Some(mask) = stroke_mask(stroke, image.width, image.height) {
        paint_mask(image, &mask, stroke.color, 1.0);
    }
}

pub(crate) struct Coverage {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    values: Vec<f32>,
}

impl Coverage {
    fn of(points: &[(f32, f32)], thickness: f32, width: u32, height: u32) -> Option<Self> {
        let radius = thickness.max(1.0) / 2.0;
        let segments: Vec<((f32, f32), (f32, f32))> = match points {
            [] => return None,
            [single] => vec![(*single, *single)],
            many => many.windows(2).map(|pair| (pair[0], pair[1])).collect(),
        };
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in points {
            left = left.min(*x);
            top = top.min(*y);
            right = right.max(*x);
            bottom = bottom.max(*y);
        }
        let clamp_x = |value: f32| (value.max(0.0) as u32).min(width);
        let clamp_y = |value: f32| (value.max(0.0) as u32).min(height);
        let (x0, y0) = (clamp_x(left - radius - 1.0), clamp_y(top - radius - 1.0));
        let (x1, y1) = (
            clamp_x(right + radius + 2.0),
            clamp_y(bottom + radius + 2.0),
        );
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let span = (x1 - x0) as usize;
        let mut values = vec![0.0f32; span * (y1 - y0) as usize];
        let boxes: Vec<_> = segments
            .iter()
            .map(|(a, b)| {
                let sx0 = clamp_x(a.0.min(b.0) - radius - 1.0).max(x0);
                let sy0 = clamp_y(a.1.min(b.1) - radius - 1.0).max(y0);
                let sx1 = clamp_x(a.0.max(b.0) + radius + 2.0).min(x1);
                let sy1 = clamp_y(a.1.max(b.1) + radius + 2.0).min(y1);
                (*a, *b, sx0, sy0, sx1, sy1)
            })
            .collect();
        let cost = values.len() * COVERAGE_COST;
        rows_parallel_with(&mut values, span, cost, |row, line| {
            let y = y0 + row as u32;
            for &(a, b, sx0, sy0, sx1, sy1) in &boxes {
                if y < sy0 || y >= sy1 {
                    continue;
                }
                for x in sx0..sx1 {
                    let near = distance_to_segment((x as f32 + 0.5, y as f32 + 0.5), a, b);
                    let value = (radius - near + 0.5).clamp(0.0, 1.0);
                    let slot = &mut line[(x - x0) as usize];
                    *slot = slot.max(value);
                }
            }
        });
        Some(Self {
            x0,
            y0,
            x1,
            y1,
            values,
        })
    }

    fn at(&self, x: u32, y: u32) -> f32 {
        self.values[(y - self.y0) as usize * (self.x1 - self.x0) as usize + (x - self.x0) as usize]
    }
}

pub fn paint_polyline(
    image: &mut Image,
    points: &[(f32, f32)],
    thickness: f32,
    color: [u8; 3],
    opacity: f32,
) {
    if let Some(mask) = Coverage::of(points, thickness, image.width, image.height) {
        paint_mask(image, &mask, color, opacity);
    }
}

pub(crate) fn paint_mask(image: &mut Image, mask: &Coverage, color: [u8; 3], opacity: f32) {
    let stride = image.width as usize * BYTES;
    let span = (mask.x1 - mask.x0) as usize;
    let left = mask.x0 as usize * BYTES;
    let [red, green, blue] = color;
    let rows = &mut image.pixels[mask.y0 as usize * stride..mask.y1 as usize * stride];
    let cost = mask.values.len() * BYTES;
    rows_parallel_with(rows, stride, cost, |row, line| {
        let values = &mask.values[row * span..(row + 1) * span];
        for (pixel, value) in line[left..left + span * BYTES]
            .as_chunks_mut::<BYTES>()
            .0
            .iter_mut()
            .zip(values)
        {
            let amount = value * opacity;
            if amount <= 0.0 {
                continue;
            }
            for (channel, target) in pixel[..3].iter_mut().zip([blue, green, red]) {
                *channel =
                    (f32::from(*channel) * (1.0 - amount) + f32::from(target) * amount) as u8;
            }
        }
    });
}

pub fn crop(pixels: &[u8], width: u32, rect: PixelRect) -> Vec<u8> {
    let stride = width as usize * BYTES;
    let row = rect.width as usize * BYTES;
    let mut out = Vec::with_capacity(row * rect.height as usize);
    for line in 0..rect.height as usize {
        let start = (rect.y as usize + line) * stride + rect.x as usize * BYTES;
        out.extend_from_slice(&pixels[start..start + row]);
    }
    out
}

pub struct Plane<'a> {
    pub bytes: &'a [u8],
    pub pitch: usize,
    pub height: u32,
    pub flipped: bool,
}

impl Plane<'_> {
    fn line(&self, row: usize) -> usize {
        let source = if self.flipped {
            self.height as usize - 1 - row
        } else {
            row
        };
        source * self.pitch
    }

    pub fn covers(&self, width: u32) -> bool {
        let row = width as usize * BYTES;
        self.height == 0
            || self.pitch >= row
                && (self.height as usize - 1) * self.pitch + row <= self.bytes.len()
    }
}

const PARALLEL_BYTES: usize = 1 << 18;
const MAX_WORKERS: usize = 8;
const COVERAGE_COST: usize = 64;

pub(crate) fn workers(bytes: usize) -> usize {
    if bytes < PARALLEL_BYTES {
        return 1;
    }
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .clamp(1, MAX_WORKERS)
}

pub(crate) fn rows_parallel(
    out: &mut [u8],
    row_bytes: usize,
    work: impl Fn(usize, &mut [u8]) + Sync,
) {
    let cost = out.len();
    rows_parallel_with(out, row_bytes, cost, work);
}

fn rows_parallel_with<T: Send>(
    out: &mut [T],
    row: usize,
    cost: usize,
    work: impl Fn(usize, &mut [T]) + Sync,
) {
    let row = row.max(1);
    let rows = out.len() / row;
    let threads = workers(cost).min(rows.max(1));
    if threads <= 1 {
        for (index, line) in out.chunks_exact_mut(row).enumerate() {
            work(index, line);
        }
        return;
    }
    let per = rows.div_ceil(threads);
    let work = &work;
    std::thread::scope(|scope| {
        for (part, block) in out.chunks_mut(per * row).enumerate() {
            scope.spawn(move || {
                for (index, line) in block.chunks_exact_mut(row).enumerate() {
                    work(part * per + index, line);
                }
            });
        }
    });
}

fn spans(count: u32, from: u32, origin: u32) -> Vec<(usize, usize)> {
    (0..count)
        .map(|index| {
            let start = (u64::from(index) * u64::from(from) / u64::from(count)) as usize;
            let end = (u64::from(index + 1) * u64::from(from) / u64::from(count)) as usize;
            let end = end.max(start + 1).min(from as usize);
            (origin as usize + start, origin as usize + end)
        })
        .collect()
}

pub fn scale_into(plane: &Plane, rect: PixelRect, out_width: u32, out_height: u32, out: &mut [u8]) {
    let out_row = out_width as usize * BYTES;
    let out = &mut out[..out_row * out_height as usize];
    if (rect.width, rect.height) == (out_width, out_height) {
        let left = rect.x as usize * BYTES;
        rows_parallel(out, out_row, |row, line| {
            let start = plane.line(rect.y as usize + row) + left;
            line.copy_from_slice(&plane.bytes[start..start + out_row]);
        });
        return;
    }
    let columns = spans(out_width, rect.width, rect.x);
    let rows = spans(out_height, rect.height, rect.y);
    rows_parallel(out, out_row, |y, line| {
        let (top, bottom) = rows[y];
        for (pixel, &(left, right)) in line.as_chunks_mut::<BYTES>().0.iter_mut().zip(&columns) {
            let mut sum = [0u32; BYTES];
            for row in top..bottom {
                let start = plane.line(row);
                let source = &plane.bytes[start + left * BYTES..start + right * BYTES];
                for value in source.as_chunks::<BYTES>().0 {
                    for (total, channel) in sum.iter_mut().zip(value) {
                        *total += u32::from(*channel);
                    }
                }
            }
            let count = ((bottom - top) * (right - left)) as u32;
            *pixel = sum.map(|total| (total / count) as u8);
        }
    });
}

#[cfg(test)]
pub fn scale(pixels: &[u8], width: u32, height: u32, out_width: u32, out_height: u32) -> Vec<u8> {
    let mut out = vec![0u8; out_width as usize * out_height as usize * BYTES];
    scale_into(
        &Plane {
            bytes: pixels,
            pitch: width as usize * BYTES,
            height,
            flipped: false,
        },
        PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        },
        out_width,
        out_height,
        &mut out,
    );
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::edit::Window;

    #[test]
    fn scaling_averages_into_the_smaller_frame() {
        let pixels: Vec<u8> = [[0u8, 0, 0, 255], [200, 200, 200, 255]]
            .iter()
            .cycle()
            .take(16)
            .flatten()
            .copied()
            .collect();
        let out = scale(&pixels, 4, 4, 2, 2);
        assert_eq!(out.len(), 2 * 2 * 4);
        assert!(out.as_chunks::<4>().0.iter().all(|pixel| pixel[0] == 100));
        assert_eq!(scale(&pixels, 4, 4, 4, 4), pixels);
    }

    fn checker(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let value = if (x / 2 + y / 2) % 2 == 0 { 255 } else { 0 };
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        pixels
    }

    fn spread(pixels: &[u8], width: u32, rect: PixelRect) -> u8 {
        let values: Vec<u8> = (rect.y..rect.y + rect.height)
            .flat_map(|y| (rect.x..rect.x + rect.width).map(move |x| (x, y)))
            .map(|(x, y)| pixels[((y * width + x) * 4) as usize])
            .collect();
        values.iter().max().unwrap() - values.iter().min().unwrap()
    }

    #[test]
    fn blur_smooths_inside_and_leaves_the_rest() {
        let (width, height) = (64, 64);
        let mut pixels = checker(width, height);
        let original = pixels.clone();
        let area = Area {
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        };
        blur(
            &mut Image {
                pixels: &mut pixels,
                width,
                height,
            },
            area,
        );
        let inner = PixelRect {
            x: 24,
            y: 24,
            width: 16,
            height: 16,
        };
        assert!(spread(&pixels, width, inner) < 40);
        assert_eq!(pixels[..4 * 64 * 10], original[..4 * 64 * 10]);
    }

    #[test]
    fn strokes_paint_along_the_line_only() {
        let (width, height) = (100, 100);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let stroke = Stroke {
            points: vec![(0.1, 0.5), (0.9, 0.5)],
            width: 0.04,
            color: [255, 0, 0],
            window: Window {
                start: Duration::ZERO,
                end: Duration::from_secs(1),
            },
        };
        draw(
            &mut Image {
                pixels: &mut pixels,
                width,
                height,
            },
            &stroke,
        );
        let at = |x: u32, y: u32| ((y * width + x) * 4) as usize;
        assert_eq!(pixels[at(50, 50) + 2], 255);
        assert_eq!(pixels[at(50, 50)], 0);
        assert_eq!(pixels[at(50, 20) + 2], 0);
    }

    fn offset(width: u32, x: u32, y: u32) -> usize {
        ((y * width + x) * 4) as usize
    }

    #[test]
    fn blur_brush_smears_the_path_and_leaves_far_pixels() {
        let (width, height) = (80, 80);
        let mut pixels = checker(width, height);
        let original = pixels.clone();
        blur_polyline(
            &mut Image {
                pixels: &mut pixels,
                width,
                height,
            },
            &[(10.0, 40.0), (70.0, 40.0)],
            12.0,
        );
        let path = PixelRect {
            x: 20,
            y: 37,
            width: 40,
            height: 6,
        };
        assert_eq!(spread(&original, width, path), 255);
        assert!(spread(&pixels, width, path) < 60);
        let row = (width * 4) as usize;
        assert_eq!(pixels[..row * 30], original[..row * 30]);
        assert_eq!(pixels[row * 50..], original[row * 50..]);
        assert_eq!(pixels[offset(width, 2, 40)], original[offset(width, 2, 40)]);
    }

    #[test]
    fn blur_brush_ignores_empty_and_offscreen_paths_and_clips_at_edges() {
        let (width, height) = (16, 16);
        let mut pixels = checker(width, height);
        let original = pixels.clone();
        let mut image = Image {
            pixels: &mut pixels,
            width,
            height,
        };
        blur_polyline(&mut image, &[], 8.0);
        blur_polyline(&mut image, &[(-50.0, -50.0)], 8.0);
        blur_polyline(&mut image, &[(0.0, 0.0), (16.0, 16.0)], 6.0);
        assert_ne!(pixels, original);
        assert_eq!(pixels[offset(width, 15, 0)], original[offset(width, 15, 0)]);
        assert_eq!(pixels[offset(width, 0, 15)], original[offset(width, 0, 15)]);
    }

    fn noise(width: u32, height: u32) -> Vec<u8> {
        let mut seed = 0x2545_f491_u32;
        (0..width * height * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect()
    }

    fn plain_box_blur(pixels: &[u8], width: u32, rect: PixelRect, radius: usize) -> Vec<[u32; 3]> {
        let (w, h) = (rect.width as usize, rect.height as usize);
        let mut block: Vec<[u32; 3]> = crop(pixels, width, rect)
            .as_chunks::<BYTES>()
            .0
            .iter()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]].map(u32::from))
            .collect();
        let span = (2 * radius + 1) as u32;
        let near = |at: usize, offset: isize, limit: usize| {
            (at as isize + offset).clamp(0, limit as isize - 1) as usize
        };
        for _ in 0..BLUR_PASSES {
            for (dx, dy) in [(1, 0), (0, 1)] {
                let source = block.clone();
                for (index, value) in block.iter_mut().enumerate() {
                    let mut sum = [0u32; 3];
                    for offset in -(radius as isize)..=radius as isize {
                        let x = near(index % w, offset * dx, w);
                        let y = near(index / w, offset * dy, h);
                        for (total, channel) in sum.iter_mut().zip(source[y * w + x]) {
                            *total += channel;
                        }
                    }
                    *value = sum.map(|total| total / span);
                }
            }
        }
        block
    }

    #[test]
    fn blur_matches_a_plain_box_blur_exactly() {
        for (width, height) in [(37u32, 23u32), (256, 160)] {
            let original = noise(width, height);
            let area = Area {
                x: 0.1,
                y: 0.2,
                width: 0.85,
                height: 0.75,
            };
            let rect = area.pixels(width, height);
            let block = plain_box_blur(&original, width, rect, blur_radius(rect) as usize);
            let mut expected = original.clone();
            for (index, value) in block.iter().enumerate() {
                let x = rect.x + index as u32 % rect.width;
                let y = rect.y + index as u32 / rect.width;
                for (channel, target) in value.iter().enumerate() {
                    expected[offset(width, x, y) + channel] = *target as u8;
                }
            }
            let mut pixels = original;
            blur(
                &mut Image {
                    pixels: &mut pixels,
                    width,
                    height,
                },
                area,
            );
            assert_eq!(pixels, expected, "{width}x{height}");
        }
    }

    #[test]
    fn blur_brush_matches_a_plain_box_blur_exactly() {
        let (width, height) = (240u32, 180u32);
        let original = noise(width, height);
        let points = [(20.0, 30.0), (200.0, 150.0), (220.0, 40.0)];
        let thickness = 22.0;
        let mask = Coverage::of(&points, thickness, width, height).unwrap();
        let radius = brush_blur_radius(thickness);
        let reach = radius * BLUR_PASSES as u32;
        let (x, y) = (mask.x0.saturating_sub(reach), mask.y0.saturating_sub(reach));
        let rect = PixelRect {
            x,
            y,
            width: (mask.x1 + reach).min(width) - x,
            height: (mask.y1 + reach).min(height) - y,
        };
        let block = plain_box_blur(&original, width, rect, radius as usize);
        let mut expected = original.clone();
        for row in mask.y0..mask.y1 {
            for column in mask.x0..mask.x1 {
                let amount = mask.at(column, row);
                let value = block[((row - rect.y) * rect.width + column - rect.x) as usize];
                let at = offset(width, column, row);
                for (channel, target) in expected[at..at + 3].iter_mut().zip(value) {
                    if amount > 0.0 {
                        *channel =
                            (f32::from(*channel) * (1.0 - amount) + target as f32 * amount) as u8;
                    }
                }
            }
        }
        let mut pixels = original;
        blur_polyline(
            &mut Image {
                pixels: &mut pixels,
                width,
                height,
            },
            &points,
            thickness,
        );
        assert_eq!(pixels, expected);
    }

    fn reference(pixels: &[u8], width: u32, rect: PixelRect, size: (u32, u32)) -> Vec<u8> {
        let cropped = crop(pixels, width, rect);
        let stride = rect.width as usize * BYTES;
        let span = |index: u32, from: u32, to: u32| {
            let start = (u64::from(index) * u64::from(from) / u64::from(to)) as usize;
            let end = (u64::from(index + 1) * u64::from(from) / u64::from(to)) as usize;
            (start, end.max(start + 1).min(from as usize))
        };
        let mut out = Vec::new();
        for y in 0..size.1 {
            let (top, bottom) = span(y, rect.height, size.1);
            for x in 0..size.0 {
                let (left, right) = span(x, rect.width, size.0);
                let mut sum = [0u32; BYTES];
                for row in top..bottom {
                    for pixel in cropped[row * stride + left * BYTES..row * stride + right * BYTES]
                        .as_chunks::<BYTES>()
                        .0
                    {
                        for (total, value) in sum.iter_mut().zip(pixel) {
                            *total += u32::from(*value);
                        }
                    }
                }
                let count = ((bottom - top) * (right - left)) as u32;
                out.extend(sum.map(|total| (total / count) as u8));
            }
        }
        out
    }

    #[test]
    fn fused_scaling_matches_crop_then_scale() {
        let (width, height) = (333u32, 211u32);
        let pixels = noise(width, height);
        let flipped: Vec<u8> = pixels
            .chunks_exact(width as usize * 4)
            .rev()
            .flatten()
            .copied()
            .collect();
        let rects = [
            PixelRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            PixelRect {
                x: 17,
                y: 9,
                width: 200,
                height: 120,
            },
        ];
        for rect in rects {
            for size in [
                (rect.width, rect.height),
                (64, 40),
                (rect.width / 3, rect.height / 2),
                (300, 200),
            ] {
                let expected = reference(&pixels, width, rect, size);
                for (bytes, flip) in [(&pixels, false), (&flipped, true)] {
                    let mut out = vec![0u8; expected.len()];
                    let plane = Plane {
                        bytes,
                        pitch: width as usize * 4,
                        height,
                        flipped: flip,
                    };
                    scale_into(&plane, rect, size.0, size.1, &mut out);
                    assert_eq!(out, expected, "{rect:?} {size:?} {flip}");
                }
            }
        }
    }

    #[test]
    fn crop_copies_the_requested_block() {
        let width = 8;
        let pixels: Vec<u8> = (0..width * 4).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        let rect = PixelRect {
            x: 2,
            y: 1,
            width: 4,
            height: 2,
        };
        let out = crop(&pixels, width, rect);
        assert_eq!(out.len(), 4 * 2 * 4);
        assert_eq!(out[0], 10);
        assert_eq!(out[16], 18);
    }
}
