mod annotate;
#[cfg(windows)]
mod concat;
mod edit;
mod effects;
mod error;
mod mixdown;
mod span;
mod stretch;

#[cfg(windows)]
mod player;
#[cfg(windows)]
mod png;
#[cfg(windows)]
mod probe;
#[cfg(windows)]
mod render;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
mod text;
#[cfg(windows)]
mod thumbnail;
#[cfg(windows)]
mod trim;

pub use annotate::{
    Align, Anchor, LINE_HEIGHT, MARKER_OPACITY, Mark, Shape, TextPainter, TextStyle,
    counter_outline, counter_size, ink_on, polylines, render as render_marks, stroke_thickness,
    text_extent,
};
pub use edit::{Area, Blur, Edit, GAIN_MAX, MIN_WINDOW, Mix, SPEEDS, Stroke, Window};
pub use effects::{Image, blur, blur_polyline, crop, draw, paint_polyline};
pub use error::{MediaError, Result};
pub use span::Span;

#[cfg(windows)]
pub use concat::concat;
#[cfg(windows)]
pub use player::{Frame, Player, PlayerState};
#[cfg(windows)]
pub use png::{Bgra, write_png};
#[cfg(windows)]
pub use probe::{Codec, MediaInfo, probe};
#[cfg(windows)]
pub use render::{AUDIO_BITS_PER_SECOND, audio_tracks, export, export_in_place, render};
#[cfg(windows)]
pub use runtime::Runtime;
#[cfg(windows)]
pub use text::draw_text;
#[cfg(windows)]
pub use thumbnail::{frames_at, thumbnail};
#[cfg(windows)]
pub use trim::{trim, trim_in_place};
