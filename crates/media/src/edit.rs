use std::time::Duration;

use crate::span::Span;

pub const SPEEDS: [f32; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
pub const GAIN_MAX: f32 = 2.0;
pub const MIN_AREA: f32 = 0.04;
pub const MIN_WINDOW: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Area {
    pub const FULL: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub fn centered(width: f32, height: f32) -> Self {
        Self {
            x: (1.0 - width) / 2.0,
            y: (1.0 - height) / 2.0,
            width,
            height,
        }
        .clamped()
    }

    pub fn from_corners(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            width: (a.0 - b.0).abs(),
            height: (a.1 - b.1).abs(),
        }
        .clamped()
    }

    pub fn spanning(a: (f32, f32), b: (f32, f32)) -> Self {
        let (left, top) = (a.0.min(b.0).clamp(0.0, 1.0), a.1.min(b.1).clamp(0.0, 1.0));
        let (right, bottom) = (a.0.max(b.0).clamp(0.0, 1.0), a.1.max(b.1).clamp(0.0, 1.0));
        Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }

    pub fn clamped(self) -> Self {
        let width = self.width.clamp(MIN_AREA, 1.0);
        let height = self.height.clamp(MIN_AREA, 1.0);
        Self {
            x: self.x.clamp(0.0, 1.0 - width),
            y: self.y.clamp(0.0, 1.0 - height),
            width,
            height,
        }
    }

    pub fn moved(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
        .clamped()
    }

    pub fn is_full(&self) -> bool {
        self.width >= 0.999 && self.height >= 0.999
    }

    pub fn contains(&self, point: (f32, f32)) -> bool {
        point.0 >= self.x
            && point.0 <= self.x + self.width
            && point.1 >= self.y
            && point.1 <= self.y + self.height
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn pixels(&self, width: u32, height: u32) -> PixelRect {
        let left = ((self.x * width as f32).round() as u32).min(width.saturating_sub(2));
        let top = ((self.y * height as f32).round() as u32).min(height.saturating_sub(2));
        let right = ((self.right() * width as f32).round() as u32).clamp(left + 2, width);
        let bottom = ((self.bottom() * height as f32).round() as u32).clamp(top + 2, height);
        PixelRect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    pub fn even(self) -> Self {
        Self {
            width: (self.width & !1).max(2),
            height: (self.height & !1).max(2),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: Duration,
    pub end: Duration,
}

impl Window {
    pub fn around(at: Duration, length: Duration, limit: Duration) -> Self {
        let start = at.min(limit.saturating_sub(MIN_WINDOW));
        Self {
            start,
            end: (start + length).min(limit).max(start + MIN_WINDOW),
        }
    }

    pub fn contains(&self, at: Duration) -> bool {
        at >= self.start && at < self.end
    }

    pub fn length(&self) -> Duration {
        self.end.saturating_sub(self.start)
    }

    pub fn with_start(self, start: Duration) -> Self {
        Self {
            start: start.min(self.end.saturating_sub(MIN_WINDOW)),
            ..self
        }
    }

    pub fn with_end(self, end: Duration, limit: Duration) -> Self {
        Self {
            end: end.min(limit).max(self.start + MIN_WINDOW),
            ..self
        }
    }

    pub fn shifted(self, start: Duration, limit: Duration) -> Self {
        let length = self.length();
        let start = start.min(limit.saturating_sub(length));
        Self {
            start,
            end: start + length,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Blur {
    pub area: Area,
    pub window: Window,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub points: Vec<(f32, f32)>,
    pub width: f32,
    pub color: [u8; 3],
    pub window: Window,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mix {
    pub master: f32,
    pub game: f32,
    pub mic: f32,
    pub discord: f32,
    pub master_muted: bool,
    pub game_muted: bool,
    pub mic_muted: bool,
    pub discord_muted: bool,
}

impl Default for Mix {
    fn default() -> Self {
        Self {
            master: 1.0,
            game: 1.0,
            mic: 1.0,
            discord: 1.0,
            master_muted: false,
            game_muted: false,
            mic_muted: false,
            discord_muted: false,
        }
    }
}

impl Mix {
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    pub fn gains(&self) -> (f32, f32) {
        let master = if self.master_muted { 0.0 } else { self.master };
        let game = if self.game_muted { 0.0 } else { self.game };
        let mic = if self.mic_muted { 0.0 } else { self.mic };
        (master * game, master * mic)
    }

    pub fn track_gain(&self, track: usize) -> f32 {
        let (game, mic) = self.gains();
        match track {
            0 => game,
            1 => mic,
            _ => {
                let master = if self.master_muted { 0.0 } else { self.master };
                let discord = if self.discord_muted {
                    0.0
                } else {
                    self.discord
                };
                master * discord
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub span: Span,
    pub speed: f32,
    pub crop: Option<Area>,
    pub blurs: Vec<Blur>,
    pub strokes: Vec<Stroke>,
    pub mutes: Vec<Window>,
    pub mix: Mix,
    pub height: Option<u32>,
    pub bitrate: Option<u32>,
}

impl Edit {
    pub fn new(span: Span) -> Self {
        Self {
            span,
            speed: 1.0,
            crop: None,
            blurs: Vec::new(),
            strokes: Vec::new(),
            mutes: Vec::new(),
            mix: Mix::default(),
            height: None,
            bitrate: None,
        }
    }

    pub fn crop_area(&self) -> Area {
        self.crop
            .filter(|area| !area.is_full())
            .unwrap_or(Area::FULL)
    }

    pub fn needs_render(&self) -> bool {
        (self.speed - 1.0).abs() > f32::EPSILON
            || !self.crop_area().is_full()
            || !self.blurs.is_empty()
            || !self.strokes.is_empty()
            || !self.mutes.is_empty()
            || !self.mix.is_neutral()
            || self.height.is_some()
            || self.bitrate.is_some()
    }

    pub fn output_size(&self, width: u32, height: u32) -> (PixelRect, (u32, u32)) {
        let rect = self.crop_area().pixels(width, height).even();
        let size = match self.height.filter(|target| *target < rect.height) {
            Some(target) => {
                let scaled = u64::from(rect.width) * u64::from(target) / u64::from(rect.height);
                (((scaled as u32) & !1).max(2), (target & !1).max(2))
            }
            None => (rect.width, rect.height),
        };
        (rect, size)
    }

    pub fn output_length(&self) -> Duration {
        self.span.length().div_f32(self.speed.max(0.01))
    }

    pub fn muted_at(&self, at: Duration) -> bool {
        self.mutes.iter().any(|window| window.contains(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lower_quality_keeps_the_aspect_and_even_sides() {
        let mut edit = Edit::new(Span::whole(Duration::from_secs(1)));
        assert_eq!(edit.output_size(2560, 1440).1, (2560, 1440));
        edit.height = Some(720);
        assert!(edit.needs_render());
        assert_eq!(edit.output_size(2560, 1440).1, (1280, 720));
        edit.height = Some(2160);
        assert_eq!(edit.output_size(2560, 1440).1, (2560, 1440));
        edit.height = Some(241);
        let (width, height) = edit.output_size(1920, 1080).1;
        assert_eq!((width % 2, height % 2), (0, 0));
    }

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn a_plain_trim_does_not_need_a_render() {
        let mut edit = Edit::new(Span::whole(MINUTE));
        assert!(!edit.needs_render());
        edit.crop = Some(Area::FULL);
        assert!(!edit.needs_render());
        edit.speed = 2.0;
        assert!(edit.needs_render());
        assert_eq!(edit.output_length(), Duration::from_secs(30));
    }

    #[test]
    fn areas_stay_inside_the_frame() {
        let area = Area {
            x: 0.9,
            y: -0.5,
            width: 0.5,
            height: 0.001,
        }
        .clamped();
        assert!(area.right() <= 1.0 && area.y >= 0.0);
        assert!(area.height >= MIN_AREA);
        let rect = Area::FULL.pixels(1919, 1081).even();
        assert_eq!((rect.width, rect.height), (1918, 1080));
    }

    #[test]
    fn windows_keep_a_minimum_length_and_limit() {
        let window = Window::around(MINUTE, Duration::from_secs(3), MINUTE);
        assert_eq!(window.end, MINUTE);
        assert_eq!(window.length(), MIN_WINDOW);
        let moved = Window {
            start: Duration::ZERO,
            end: Duration::from_secs(5),
        }
        .shifted(MINUTE, MINUTE);
        assert_eq!(moved.end, MINUTE);
        assert_eq!(moved.length(), Duration::from_secs(5));
    }

    #[test]
    fn muting_the_master_silences_both_tracks() {
        let mix = Mix {
            master_muted: true,
            ..Mix::default()
        };
        assert_eq!(mix.gains(), (0.0, 0.0));
        let mix = Mix {
            master: 0.5,
            mic: 2.0,
            ..Mix::default()
        };
        assert_eq!(mix.gains(), (0.5, 1.0));
        let mix = Mix {
            master: 0.5,
            discord: 0.5,
            ..Mix::default()
        };
        assert_eq!(mix.track_gain(0), 0.5);
        assert_eq!(mix.track_gain(2), 0.25);
        let mix = Mix {
            discord_muted: true,
            ..Mix::default()
        };
        assert_eq!(mix.track_gain(1), 1.0);
        assert_eq!(mix.track_gain(2), 0.0);
    }
}
