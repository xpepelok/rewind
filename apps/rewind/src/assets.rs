use std::borrow::Cow;

use gpui::{AssetSource, Hsla, Result, SharedString, Svg, prelude::*, px, svg};

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($variant),+
        }

        impl Icon {
            pub const fn path(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!("icons/", $file, ".svg")),+
                }
            }
        }

        const ICON_FILES: &[(&str, &[u8])] = &[
            $((concat!("icons/", $file, ".svg"), include_bytes!(concat!("../assets/icons/", $file, ".svg")))),+
        ];
    };
}

icons! {
    ArrowBack => "arrow-back",
    Audio => "audio",
    Bell => "bell",
    Check => "check",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    Close => "close",
    Copy => "copy",
    Delete => "delete",
    Edit => "edit",
    Expand => "expand",
    Folder => "folder",
    Gallery => "gallery",
    Keyboard => "keyboard",
    Mic => "mic",
    MicOff => "mic-off",
    Monitor => "monitor",
    Open => "open",
    Pause => "pause",
    Pin => "pin",
    Photo => "photo",
    Play => "play",
    Record => "record",
    Redo => "redo",
    Replay => "replay",
    Restore => "restore",
    Save => "save",
    Screenshot => "screenshot",
    Settings => "settings",
    Stats => "stats",
    Stop => "stop",
    Undo => "undo",
    Storage => "storage",
    Upload => "upload",
    Videocam => "videocam",
    YouTube => "youtube",
    ToolSelect => "tool-select",
    ToolCrop => "tool-crop",
    ToolPen => "tool-pen",
    ToolLine => "tool-line",
    ToolArrow => "tool-arrow",
    ToolFrame => "tool-frame",
    ToolRect => "tool-rect",
    ToolCircle => "tool-circle",
    ToolMarker => "tool-marker",
    ToolText => "tool-text",
    ToolPixelate => "tool-pixelate",
    ToolBlurBrush => "tool-blur-brush",
    ToolInvert => "tool-invert",
    ToolCounter => "tool-counter",
    Eyedropper => "eyedropper",
    Bold => "bold",
    Italic => "italic",
    Underline => "underline",
    Strike => "strike",
    AlignLeft => "align-left",
    AlignCenter => "align-center",
    AlignRight => "align-right",
    Minus => "minus",
    Plus => "plus",
    PhotoMode => "photo-mode",
    Highlights => "highlights",
    Filters => "filters",
    ArrowUp => "arrow-up",
    ArrowDown => "arrow-down",
    Info => "info",
    HidePanel => "hide-panel",
    Palette => "palette",
}

const SHOT_FILES: &[(&str, &[u8])] = &[
    ("shots/home.png", include_bytes!("../assets/shots/home.png")),
    ("shots/filters.png", include_bytes!("../assets/shots/filters.png")),
    ("shots/photo.png", include_bytes!("../assets/shots/photo.png")),
    (
        "shots/highlights.png",
        include_bytes!("../assets/shots/highlights.png"),
    ),
    ("shots/settings.png", include_bytes!("../assets/shots/settings.png")),
    (
        "shots/appearance.png",
        include_bytes!("../assets/shots/appearance.png"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICON_FILES
            .iter()
            .chain(SHOT_FILES.iter())
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICON_FILES
            .iter()
            .chain(SHOT_FILES.iter())
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::new_static(name))
            .collect())
    }
}

pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    svg()
        .flex_none()
        .size(px(size))
        .path(icon.path())
        .text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_embedded() {
        for (path, bytes) in ICON_FILES {
            assert!(!bytes.is_empty(), "{path} is empty");
            assert!(Assets.load(path).unwrap().is_some());
        }
    }
}
