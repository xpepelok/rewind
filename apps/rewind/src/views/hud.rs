use std::time::Duration;

use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, Hsla, Render, Subscription, Window, div,
    prelude::*, px, relative,
};

use rewind_config::{Corner, FontColor, Interface, MicMode, OverlayPart, StatsLayout};

use crate::assets::{Icon, icon};
use crate::gallery::format_duration;
use crate::motion::{self, Key};
use crate::platform;
use crate::state::{Rewind, TOAST_EXIT, Toast};
use crate::theme::{CONTROL_TEXT, FONT_FAMILY, Palette, SMALL_TEXT, ThemeExt, alpha};

const MARGIN: f32 = 24.0;
const TOAST_WIDTH: f32 = 380.0;
const TOAST_BADGE: f32 = 60.0;
const TOAST_TITLE: f32 = 15.0;
const TOAST_ENTER: Duration = Duration::from_millis(650);
const TOAST_SLIDE: f32 = 0.4;
const TOAST_FLOW: [f32; 4] = [0.4, 0.0, 0.2, 1.0];
const INDICATOR: f32 = 22.0;

pub const PARTS: [OverlayPart; 3] = [
    OverlayPart::Stats,
    OverlayPart::Indicators,
    OverlayPart::Notifications,
];

pub struct Hud {
    state: Entity<Rewind>,
    part: OverlayPart,
    captured: bool,
    shown: Shown,
    full: Option<platform::window::Extent>,
    _subscription: Subscription,
}

impl Hud {
    fn fit(&mut self, window: &Window, cx: &gpui::App) {
        if self.shown.visible() {
            if let Some(full) = self.full.take() {
                platform::window::unfold(window, full, cx);
            }
        } else if self.full.is_none() {
            self.full = platform::window::fold(window, cx);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Shown {
    toasts: Vec<Toast>,
    recording: Option<u64>,
    replay: bool,
    mic: Option<bool>,
    stats: Option<(u32, Option<u32>)>,
    interface: Interface,
}

impl Shown {
    fn of(rewind: &Rewind, part: OverlayPart) -> Self {
        let interface = rewind.settings.interface.clone();
        let toasts = if part == OverlayPart::Notifications {
            rewind.toasts.clone()
        } else {
            Vec::new()
        };
        let stats =
            (rewind.settings.stats_visible && interface.stats_corner != Corner::Off).then(|| {
                let encoding = rewind.replay_enabled() || rewind.is_recording();
                (
                    rewind.stats.display_fps,
                    encoding.then_some(rewind.stats.encoded_fps),
                )
            });
        Self {
            toasts,
            recording: rewind.recording.map(|elapsed| elapsed.as_secs()),
            replay: rewind.replay_enabled(),
            mic: (rewind.settings.audio.mic_mode != MicMode::Off).then(|| rewind.mic_enabled()),
            stats,
            interface,
        }
        .only(part)
    }

    fn only(mut self, part: OverlayPart) -> Self {
        if part != OverlayPart::Notifications {
            self.toasts.clear();
        }
        if part != OverlayPart::Indicators {
            self.recording = None;
            self.replay = false;
            self.mic = None;
        }
        if part != OverlayPart::Stats {
            self.stats = None;
        }
        self
    }

    fn visible(&self) -> bool {
        let interface = &self.interface;
        !self.toasts.is_empty()
            || self.stats.is_some()
            || (self.recording.is_some() && interface.recording_indicator != Corner::Off)
            || (self.replay && interface.replay_indicator != Corner::Off)
            || (self.mic.is_some() && interface.mic_indicator != Corner::Off)
    }

    fn raises_over(&self, before: &Shown) -> bool {
        let fresh_toast = self
            .toasts
            .iter()
            .any(|toast| before.toasts.iter().all(|old| old.id != toast.id));
        fresh_toast || (self.visible() && !before.visible())
    }
}

impl Hud {
    pub fn new(
        state: Entity<Rewind>,
        part: OverlayPart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let captured = state.read(cx).settings.captured.shows(part);
        platform::window::prepare_hud(window, captured);
        let shown = Shown::of(state.read(cx), part);
        let subscription = cx.observe_in(&state, window, move |hud, state, window, cx| {
            let captured = state.read(cx).settings.captured.shows(part);
            if captured != hud.captured {
                hud.captured = captured;
                platform::window::show_in_capture(window, captured);
            }
            let shown = Shown::of(state.read(cx), part);
            if shown == hud.shown {
                return;
            }
            if shown.raises_over(&hud.shown) {
                platform::window::keep_on_top(window);
            }
            hud.shown = shown;
            hud.fit(window, cx);
            cx.notify();
        });
        let mut hud = Self {
            state,
            part,
            captured,
            shown,
            full: None,
            _subscription: subscription,
        };
        hud.fit(window, cx);
        hud
    }
}

fn phases(shown: f32) -> (f32, f32) {
    let shown = shown.clamp(0.0, 1.0);
    let slide = (shown / TOAST_SLIDE).min(1.0);
    let wipe = ((shown - TOAST_SLIDE) / (1.0 - TOAST_SLIDE)).max(0.0);
    (slide, wipe)
}

fn font_color(color: FontColor, p: &Palette) -> Hsla {
    match color {
        FontColor::White => p.white,
        FontColor::Green => p.stats_green,
        FontColor::Yellow => p.yellow,
    }
}

fn anchored(corner: Corner, content: Div) -> Option<Div> {
    let (top, left) = match corner {
        Corner::Off => return None,
        Corner::TopLeft => (true, true),
        Corner::TopRight => (true, false),
        Corner::BottomLeft => (false, true),
        Corner::BottomRight => (false, false),
    };
    let base = div().absolute().p(px(MARGIN));
    let base = if top { base.top_0() } else { base.bottom_0() };
    let base = if left { base.left_0() } else { base.right_0() };
    Some(base.child(content))
}

fn spaced(fraction: f32, content: Div, vertical: bool) -> Div {
    let fraction = fraction.clamp(0.0, 1.0);
    let before = div().flex_shrink().flex_basis(relative(fraction));
    let after = div().flex_shrink().flex_basis(relative(1.0 - fraction));
    let line = if vertical {
        div().size_full().flex().flex_col()
    } else {
        div().size_full().flex()
    };
    line.child(before).child(content.flex_none()).child(after)
}

impl Hud {
    fn toasts(&self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let toasts = self.state.read(cx).toasts.clone();
        let cards = toasts
            .into_iter()
            .map(|toast| {
                let key = Key::new("toast", toast.id);
                if toast.leaving {
                    motion::drive(key, false, TOAST_EXIT, TOAST_FLOW, window, cx);
                } else if !motion::is_on(key, cx) {
                    motion::drive(key, true, TOAST_ENTER, TOAST_FLOW, window, cx);
                }
                let (slide, wipe) = phases(motion::value(key, cx));
                let tone = if toast.error { p.warning } else { p.accent };
                let cover = TOAST_BADGE + (TOAST_WIDTH - TOAST_BADGE) * (1.0 - wipe);
                div()
                    .relative()
                    .left(px((1.0 - slide) * (TOAST_WIDTH + MARGIN)))
                    .flex()
                    .min_h(px(TOAST_BADGE))
                    .bg(p.toast)
                    .shadow(p.toast_shadow())
                    .child(div().flex_none().w(px(TOAST_BADGE)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(2.0))
                            .px(px(16.0))
                            .py(px(12.0))
                            .child(
                                div()
                                    .text_size(px(TOAST_TITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.title)
                                    .child(toast.title),
                            )
                            .when_some(toast.detail, |text, detail| {
                                text.child(
                                    div()
                                        .text_size(px(SMALL_TEXT))
                                        .text_color(p.muted)
                                        .child(detail),
                                )
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(px(cover))
                            .bg(tone),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(px(TOAST_BADGE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(toast.icon, 26.0, p.stage)),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .absolute()
            .top(px(MARGIN))
            .right(px(MARGIN))
            .w(px(TOAST_WIDTH))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .children(cards)
    }

    fn indicators(&self, p: &Palette, cx: &Context<Self>) -> Vec<AnyElement> {
        let rewind = self.state.read(cx);
        let interface = &rewind.settings.interface;
        let mut groups: Vec<(Corner, Vec<AnyElement>)> = Vec::with_capacity(3);
        let mut place = |corner: Corner, element: AnyElement| {
            if corner == Corner::Off {
                return;
            }
            match groups.iter_mut().find(|(other, _)| *other == corner) {
                Some((_, elements)) => elements.push(element),
                None => groups.push((corner, vec![element])),
            }
        };
        if let Some(elapsed) = rewind.recording {
            place(
                interface.recording_indicator,
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(icon(Icon::Record, INDICATOR, p.record))
                    .child(
                        div()
                            .text_size(px(CONTROL_TEXT))
                            .text_color(if p.soft { p.white } else { p.title })
                            .child(format_duration(elapsed)),
                    )
                    .into_any_element(),
            );
        }
        if rewind.replay_enabled() {
            place(
                interface.replay_indicator,
                icon(Icon::Replay, INDICATOR, p.accent).into_any_element(),
            );
        }
        if rewind.settings.audio.mic_mode != MicMode::Off {
            let glyph = if rewind.mic_enabled() {
                Icon::Mic
            } else {
                Icon::MicOff
            };
            place(
                interface.mic_indicator,
                icon(glyph, INDICATOR, if p.soft { p.white } else { p.title }).into_any_element(),
            );
        }
        groups
            .into_iter()
            .filter_map(|(corner, elements)| {
                anchored(
                    corner,
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .bg(p.indicator)
                        .children(elements),
                )
            })
            .map(IntoElement::into_any_element)
            .collect()
    }

    fn stats(&self, p: &Palette, cx: &Context<Self>) -> Option<AnyElement> {
        let rewind = self.state.read(cx);
        if !rewind.settings.stats_visible {
            return None;
        }
        let interface = &rewind.settings.interface;
        if interface.stats_corner == Corner::Off {
            return None;
        }
        let stats = rewind.stats;
        let color = font_color(interface.stats_font_color, p);
        let entry = |name: &str, value: String| {
            div()
                .flex()
                .gap(px(6.0))
                .child(div().text_color(p.stats_label).child(name.to_owned()))
                .child(div().text_color(color).child(value))
        };
        let mut entries = vec![entry("FPS", stats.display_fps.to_string())];
        if rewind.replay_enabled() || rewind.is_recording() {
            entries.push(entry("REC", format!("{} к/с", stats.encoded_fps)));
        }
        let content = div()
            .flex()
            .when(interface.stats_layout == StatsLayout::Column, |content| {
                content.flex_col()
            })
            .gap(px(14.0))
            .px(px(8.0))
            .py(px(4.0))
            .bg(alpha(
                p.stats_backdrop,
                f32::from(interface.stats_opacity) / 100.0,
            ))
            .text_size(px(interface.stats_font_size.pixels()))
            .children(entries);
        let horizontal = spaced(f32::from(interface.stats_x) / 100.0, content, false);
        Some(
            div()
                .absolute()
                .inset_0()
                .p(px(MARGIN))
                .child(spaced(
                    f32::from(interface.stats_y) / 100.0,
                    horizontal.h_auto(),
                    true,
                ))
                .into_any_element(),
        )
    }
}

impl Render for Hud {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        let layer = div().font_family(FONT_FAMILY).size_full().relative();
        match self.part {
            OverlayPart::Notifications => {
                let toasts = self.toasts(&p, window, cx);
                motion::keep_alive(window, cx);
                layer.child(toasts)
            }
            OverlayPart::Indicators => layer.children(self.indicators(&p, cx)),
            OverlayPart::Stats | OverlayPart::Panel => layer.children(self.stats(&p, cx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(id: u64) -> Toast {
        Toast {
            id,
            icon: Icon::Check,
            title: String::new(),
            detail: None,
            error: false,
            leaving: false,
            stamp: id,
        }
    }

    fn empty() -> Shown {
        Shown {
            toasts: Vec::new(),
            recording: None,
            replay: false,
            mic: None,
            stats: None,
            interface: Interface::default(),
        }
    }

    #[test]
    fn each_window_keeps_only_its_own_part() {
        let all = Shown {
            toasts: vec![toast(1)],
            recording: Some(3),
            replay: true,
            mic: Some(true),
            stats: Some((60, None)),
            interface: Interface::default(),
        };
        let toasts = all.clone().only(OverlayPart::Notifications);
        assert_eq!(toasts.toasts.len(), 1);
        assert!(toasts.recording.is_none() && !toasts.replay && toasts.stats.is_none());
        let indicators = all.clone().only(OverlayPart::Indicators);
        assert!(indicators.toasts.is_empty() && indicators.stats.is_none());
        assert!(indicators.replay && indicators.mic.is_some());
        let stats = all.only(OverlayPart::Stats);
        assert!(stats.toasts.is_empty() && !stats.replay && stats.stats.is_some());
    }

    #[test]
    fn a_toast_slides_in_fully_green_and_then_uncovers_its_text() {
        assert_eq!(phases(0.0), (0.0, 0.0));
        let (slide, wipe) = phases(TOAST_SLIDE / 2.0);
        assert!(slide > 0.0 && slide < 1.0 && wipe == 0.0);
        assert_eq!(phases(TOAST_SLIDE), (1.0, 0.0));
        let (slide, wipe) = phases(0.7);
        assert!(slide == 1.0 && wipe > 0.0 && wipe < 1.0);
        assert_eq!(phases(1.0), (1.0, 1.0));
    }

    #[test]
    fn nothing_shown_is_invisible() {
        assert!(!empty().visible());
        let mut hidden = empty();
        hidden.replay = true;
        hidden.interface.replay_indicator = Corner::Off;
        assert!(!hidden.visible());
    }

    #[test]
    fn new_toast_raises_but_leaving_one_does_not() {
        let before = Shown {
            toasts: vec![toast(1)],
            ..empty()
        };
        let fresh = Shown {
            toasts: vec![toast(1), toast(2)],
            ..empty()
        };
        assert!(fresh.raises_over(&before));
        let mut leaving = before.clone();
        leaving.toasts[0].leaving = true;
        assert_ne!(leaving, before);
        assert!(!leaving.raises_over(&before));
    }

    #[test]
    fn becoming_visible_raises_once() {
        let recording = Shown {
            recording: Some(0),
            ..empty()
        };
        assert!(recording.raises_over(&empty()));
        let later = Shown {
            recording: Some(1),
            ..empty()
        };
        assert_ne!(later, recording);
        assert!(!later.raises_over(&recording));
    }
}
