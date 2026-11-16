use gpui::{App, BoxShadow, Global, Hsla, Rgba, point, px};
use rewind_config::Theme;

pub const FONT_FAMILY: &str = "Segoe UI";
pub const PANEL_WIDTH: f32 = 512.0;
pub const HEADER_HEIGHT: f32 = 55.0;
pub const GUTTER: f32 = 16.0;
pub const THEMED_CONTENT_INSET: f32 = 20.0;
pub const TEXT_INSET: f32 = 55.0;
pub const DIVIDER_INSET: f32 = 15.0;
pub const ICON: f32 = 24.0;
pub const TITLE_TEXT: f32 = 20.0;
pub const PAGE_TITLE_TEXT: f32 = 18.0;
pub const BODY_TEXT: f32 = 16.0;
pub const SMALL_TEXT: f32 = 13.0;
pub const CONTROL_TEXT: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub panel: Hsla,
    pub header: Hsla,
    pub divider: Hsla,
    pub text: Hsla,
    pub title: Hsla,
    pub muted: Hsla,
    pub icon: Hsla,
    pub disabled: Hsla,
    pub accent: Hsla,
    pub accent_dim: Hsla,
    pub accent_hover: Hsla,
    pub stage: Hsla,
    pub track: Hsla,
    pub field: Hsla,
    pub field_line: Hsla,
    pub dots: Hsla,
    pub hover: Hsla,
    pub badge: Hsla,
    pub check_mark: Hsla,
    pub record: Hsla,
    pub toast: Hsla,
    pub scrim: Hsla,
    pub warning: Hsla,
    pub white: Hsla,
    pub yellow: Hsla,
    pub stats_green: Hsla,
    pub stats_label: Hsla,
    pub indicator: Hsla,
    pub stats_backdrop: Hsla,
    pub soft: bool,
    pub round: f32,
    pub card: Option<Hsla>,
    pub row_line: Option<Hsla>,
    pub square: bool,
}

pub const fn hex(value: u32) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

pub fn rgb(value: u32) -> Hsla {
    hex(value).into()
}

pub fn rgba(value: u32, alpha: f32) -> Hsla {
    Rgba {
        a: alpha,
        ..hex(value)
    }
    .into()
}

pub fn alpha(color: Hsla, value: f32) -> Hsla {
    Hsla {
        a: color.a * value,
        ..color
    }
}

pub fn blend(from: Hsla, to: Hsla, share: f32) -> Hsla {
    let (from, to) = (Rgba::from(from), Rgba::from(to));
    let mix = |from: f32, to: f32| from + (to - from) * share;
    Rgba {
        r: mix(from.r, to.r),
        g: mix(from.g, to.g),
        b: mix(from.b, to.b),
        a: mix(from.a, to.a),
    }
    .into()
}

const SQUARE_KNOB: f32 = 2.0;

impl Palette {
    pub fn of(theme: Theme) -> Self {
        match theme {
            Theme::Classic => Self::dark(),
            Theme::Carbon => Self::carbon(),
            Theme::Ruby => Self::ruby(),
            Theme::Light => Self::light(),
        }
    }

    pub const fn radius(&self) -> f32 {
        self.round
    }

    pub const fn knob_radius(&self, size: f32) -> f32 {
        if self.square { SQUARE_KNOB } else { size / 2.0 }
    }

    pub fn carbon() -> Self {
        Self {
            panel: rgb(0x0b0b0c),
            header: rgb(0x0b0b0c),
            divider: rgb(0x303033),
            text: rgb(0xf5f5f5),
            title: rgb(0xffffff),
            muted: rgb(0xa2a2a5),
            icon: rgb(0xf5f5f5),
            disabled: rgb(0x626266),
            accent: rgb(0xf5f5f5),
            accent_dim: rgb(0x323235),
            accent_hover: rgb(0xffffff),
            track: rgb(0x181819),
            field: rgb(0x121213),
            field_line: rgb(0x46464a),
            dots: rgb(0x303033),
            hover: rgb(0x1b1b1d),
            check_mark: rgb(0x0a0a0a),
            record: rgb(0xf5f5f5),
            toast: rgba(0x181819, 0.96),
            soft: true,
            round: 4.0,
            row_line: Some(rgb(0x303033)),
            square: true,
            ..Self::dark()
        }
    }

    pub fn ruby() -> Self {
        Self {
            panel: rgb(0x151618),
            header: rgb(0x151618),
            divider: rgb(0x34363a),
            text: rgb(0xededee),
            title: rgb(0xffffff),
            muted: rgb(0xa6a8ad),
            icon: rgb(0xd0d1d4),
            disabled: rgb(0x696b70),
            accent: rgb(0xdc505b),
            accent_dim: rgb(0x65343b),
            accent_hover: rgb(0xec6470),
            track: rgb(0x303136),
            field: rgb(0x2b2c30),
            field_line: rgb(0x45464b),
            dots: rgb(0x46474c),
            hover: rgb(0x33353a),
            check_mark: rgb(0xffffff),
            toast: rgba(0x28292d, 0.96),
            soft: true,
            round: 12.0,
            card: Some(rgb(0x242529)),
            ..Self::dark()
        }
    }

    pub fn light() -> Self {
        Self {
            panel: rgb(0xf3f5f8),
            header: rgb(0xffffff),
            divider: rgb(0xd8dde6),
            text: rgb(0x1c1f24),
            title: rgb(0x111418),
            muted: rgb(0x5c6675),
            icon: rgb(0x3f4b5c),
            disabled: rgb(0xa0a7b2),
            accent: rgb(0x2563eb),
            accent_dim: rgb(0xcbd9fa),
            accent_hover: rgb(0x3b76f0),
            track: rgb(0xe4e9f1),
            field: rgb(0xffffff),
            field_line: rgb(0xc6ceda),
            dots: rgb(0xd3d9e3),
            hover: rgb(0xe8edf5),
            check_mark: rgb(0xffffff),
            toast: rgba(0xffffff, 0.96),
            soft: true,
            round: 8.0,
            card: Some(rgb(0xffffff)),
            row_line: Some(rgb(0xdfe4eb)),
            ..Self::dark()
        }
    }

    pub fn dark() -> Self {
        Self {
            panel: rgb(0x191919),
            header: rgb(0x393939),
            divider: rgb(0x858585),
            text: rgb(0xeeeeee),
            title: rgb(0xf2f2f2),
            muted: rgb(0xa8a8a8),
            icon: rgb(0xbdbdbd),
            disabled: rgb(0x6b6b6b),
            accent: rgb(0x76b900),
            accent_dim: rgb(0x47690c),
            accent_hover: rgb(0x8acb14),
            stage: rgb(0x000000),
            track: rgb(0x262626),
            field: rgb(0x393939),
            field_line: rgb(0x636363),
            dots: rgb(0x5e5e5e),
            hover: rgb(0x262626),
            badge: rgba(0x000000, 0.72),
            check_mark: rgb(0x191919),
            record: rgb(0xe02828),
            toast: rgba(0x191919, 0.94),
            scrim: rgba(0x000000, 0.35),
            warning: rgb(0xffb43e),
            white: rgb(0xffffff),
            yellow: rgb(0xffd400),
            stats_green: rgb(0x76b900),
            stats_label: rgba(0xffffff, 0.7),
            indicator: rgba(0x000000, 0.35),
            stats_backdrop: rgb(0x000000),
            soft: false,
            round: 0.0,
            card: None,
            row_line: None,
            square: false,
        }
    }

    pub fn toast_shadow(&self) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: rgba(0x000000, 0.55),
            offset: point(px(0.0), px(6.0)),
            blur_radius: px(18.0),
            spread_radius: px(0.0),
        }]
    }

}

pub struct ActivePalette(pub Palette);

impl Global for ActivePalette {}

pub trait ThemeExt {
    fn palette(&self) -> Palette;
}

impl ThemeExt for App {
    fn palette(&self) -> Palette {
        self.try_global::<ActivePalette>()
            .map(|active| active.0)
            .unwrap_or_else(Palette::dark)
    }
}

pub fn install(cx: &mut App) {
    cx.set_global(ActivePalette(Palette::dark()));
}

pub fn apply(theme: Theme, cx: &mut App) {
    cx.set_global(ActivePalette(Palette::of(theme)));
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_classic_theme_keeps_the_original_colors() {
        assert_eq!(Palette::of(Theme::Classic), Palette::dark());
    }

    #[test]
    fn every_new_theme_changes_shapes_and_stays_readable() {
        for theme in Theme::ALL
            .iter()
            .copied()
            .filter(|theme| *theme != Theme::Classic)
        {
            let look = Palette::of(theme);
            assert!(look.radius() > 0.0, "{theme:?}");
            assert_ne!(look.panel, Palette::dark().panel, "{theme:?}");
            assert_ne!(look.text, look.panel, "{theme:?}");
            assert_ne!(look.check_mark, look.accent, "{theme:?}");
        }
    }

    #[test]
    fn themed_surfaces_and_controls_have_distinct_shapes() {
        let carbon = Palette::carbon();
        let ruby = Palette::ruby();
        let light = Palette::light();
        assert!(carbon.square);
        assert!(carbon.card.is_none());
        assert!(carbon.row_line.is_some());
        assert!(ruby.card.is_some());
        assert!(ruby.row_line.is_none());
        assert!(!ruby.square);
        assert!(light.card.is_some());
        assert!(light.row_line.is_some());
        assert!(!light.square);
        assert_eq!(Palette::dark().card, None);
        assert_eq!(Palette::dark().row_line, None);
    }

    #[test]
    fn blending_runs_from_one_color_to_the_other() {
        let (black, white) = (rgb(0x000000), rgb(0xffffff));
        assert_eq!(blend(black, white, 0.0), black);
        assert_eq!(blend(black, white, 1.0), white);
    }
}
