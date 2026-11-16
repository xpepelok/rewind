use std::path::PathBuf;
use std::time::Duration;

use gpui::Context;
use rewind_nvbus::Moments;
use rewind_nvbus::desk::{Choice, Moment};

use super::{Rewind, Signal, UiEvent};
use crate::assets::Icon;

const TICKS_PER_MILLISECOND: i64 = 10_000;
const BUSY_RETRY: Duration = Duration::from_millis(500);
const BUSY_ATTEMPTS: usize = 20;

mod msg {
    pub const NEEDS_REPLAY: &str = "Лучшим моментам нужен мгновенный повтор";
    pub const SAVED: &str = "Лучший момент сохранен";

    pub fn kept(count: usize) -> String {
        format!("Лучшие моменты ({count}) сохранены в галерее")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MomentClip {
    pub path: PathBuf,
    pub preview: Option<PathBuf>,
    pub name: String,
    pub picked: bool,
}

pub fn sorted(clips: &[MomentClip]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (kept, dropped): (Vec<&MomentClip>, Vec<&MomentClip>) =
        clips.iter().partition(|clip| clip.picked);
    let paths = |clips: Vec<&MomentClip>| clips.into_iter().map(|clip| clip.path.clone()).collect();
    (paths(kept), paths(dropped))
}

pub fn span(end: i64, start_delta: i32, end_delta: i32) -> (i64, i64) {
    let length = i64::from(end_delta.max(0)) - i64::from(start_delta.min(0));
    let tail = i64::from(end_delta.min(0));
    let to = end + tail * TICKS_PER_MILLISECOND;
    (
        to - length.max(0) * TICKS_PER_MILLISECOND - tail * TICKS_PER_MILLISECOND,
        to,
    )
}

impl Rewind {
    fn moment_choice(&self) -> Choice {
        let highlights = &self.settings.highlights;
        let allowed = self
            .moment_game
            .as_deref()
            .is_none_or(|path| highlights.allows(path));
        Choice {
            enabled: highlights.enabled && allowed,
            muted: self
                .moment_game
                .as_deref()
                .map(|path| highlights.muted(path))
                .unwrap_or_default(),
        }
    }

    pub(super) fn start_moments(&mut self) {
        if !self.settings.highlights.enabled || self.moments.is_some() {
            return;
        }
        let signals = self.signals.clone();
        self.moments = Some(Moments::start(
            self.paths.camera(),
            self.moment_choice(),
            move |moment| {
                signals.unbounded_send(Signal::Moment(moment)).ok();
            },
        ));
    }

    fn tell_moments(&self) {
        if let Some(moments) = &self.moments {
            moments.choose(self.moment_choice());
        }
    }

    pub fn set_highlights_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.highlights.enabled = enabled;
        self.save();
        if enabled {
            self.start_moments();
        } else {
            self.moments = None;
            self.moment_game = None;
        }
        cx.notify();
    }

    pub fn toggle_moment_game(&mut self, path: &str, cx: &mut Context<Self>) {
        self.settings.highlights.toggle_game(path);
        self.save();
        self.tell_moments();
        cx.notify();
    }

    pub fn toggle_moment_kind(&mut self, path: &str, id: &str, cx: &mut Context<Self>) {
        self.settings.highlights.toggle(path, id);
        self.save();
        self.tell_moments();
        cx.notify();
    }

    fn moment_name(&self, highlight: &str) -> String {
        self.moment_game
            .as_deref()
            .and_then(|path| self.settings.highlights.game(path))
            .and_then(|game| game.kinds.iter().find(|kind| kind.id == highlight))
            .map_or_else(|| highlight.to_owned(), |kind| kind.name.clone())
    }

    pub(super) fn moment_saved(&mut self, path: PathBuf, preview: PathBuf, cx: &mut Context<Self>) {
        let name = self.moment_pending.pop_front().unwrap_or_default();
        self.moment_clips.push(MomentClip {
            path,
            preview: Some(preview).filter(|preview| preview.is_file()),
            name,
            picked: true,
        });
        self.toast(Icon::Highlights, msg::SAVED, None, false, cx);
        cx.notify();
    }

    pub fn pick_moment(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(clip) = self.moment_clips.get_mut(index) {
            clip.picked ^= true;
            cx.notify();
        }
    }

    pub fn keep_moments(&mut self, cx: &mut Context<Self>) {
        let (kept, dropped) = sorted(&self.moment_clips);
        self.moment_clips.clear();
        if !dropped.is_empty() {
            self.delete_clips(dropped, cx);
        }
        if !kept.is_empty() {
            self.toast(Icon::Highlights, msg::kept(kept.len()), None, false, cx);
        }
        cx.notify();
    }

    pub fn discard_moments(&mut self, cx: &mut Context<Self>) {
        for clip in &mut self.moment_clips {
            clip.picked = false;
        }
        self.keep_moments(cx);
    }

    fn save_moment(&mut self, highlight: &str, start: i32, end: i32, cx: &mut Context<Self>) {
        let name = self.moment_name(highlight);
        if !self.settings.replay_enabled {
            self.toast(Icon::Highlights, msg::NEEDS_REPLAY, None, true, cx);
            return;
        }
        let wait = Duration::from_millis(u64::from(end.max(0).unsigned_abs()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let mut range = None;
            for _ in 0..BUSY_ATTEMPTS {
                let busy = this.update(cx, |rewind, _| {
                    rewind.try_moment(&name, start, end, &mut range)
                });
                if !busy.unwrap_or(false) {
                    return;
                }
                cx.background_executor().timer(BUSY_RETRY).await;
            }
        })
        .detach();
    }

    fn try_moment(
        &mut self,
        name: &str,
        start: i32,
        end: i32,
        range: &mut Option<(i64, i64)>,
    ) -> bool {
        let Some(recorder) = self.recorder_on(self.replay_target) else {
            return false;
        };
        let Some(window) = recorder.replay_window() else {
            return false;
        };
        let (from, to) = *range.get_or_insert_with(|| span(window.end, start, end));
        if to <= window.start {
            return false;
        }
        if !recorder.save_replay_ranges(vec![(from.max(window.start), to)], false) {
            return true;
        }
        self.moment_pending.push_back(name.to_owned());
        false
    }

    pub(super) fn on_moment(&mut self, moment: Moment, cx: &mut Context<Self>) {
        match moment {
            Moment::Joined { game, path, .. } => {
                let key = if path.is_empty() { game.clone() } else { path };
                if self.settings.highlights.meet(&key, &game) {
                    self.save();
                }
                self.moment_game = Some(key);
                self.tell_moments();
            }
            Moment::Left => self.moment_game = None,
            Moment::Configured(definitions) => {
                if let Some(path) = self.moment_game.clone() {
                    let kinds: Vec<_> = definitions
                        .into_iter()
                        .map(|kind| (kind.id, kind.name, kind.interest))
                        .collect();
                    if self.settings.highlights.configure(&path, &kinds) {
                        self.save();
                    }
                    self.tell_moments();
                }
            }
            Moment::Video {
                highlight,
                start,
                end,
                ..
            } => self.save_moment(&highlight, start, end, cx),
            Moment::Screenshot { .. } => self.screenshot(),
            Moment::Summary => {
                rewind_capture::remember_application();
                cx.emit(if self.moment_clips.is_empty() {
                    UiEvent::ShowGallery
                } else {
                    UiEvent::ShowSummary
                });
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1000 * TICKS_PER_MILLISECOND;

    #[test]
    fn a_moment_that_ran_past_now_is_cut_from_the_end_of_the_buffer() {
        let end = 100 * SECOND;
        assert_eq!(span(end, -5000, 2000), (93 * SECOND, end));
    }

    #[test]
    fn a_moment_that_ended_earlier_stops_before_the_end_of_the_buffer() {
        let end = 100 * SECOND;
        assert_eq!(span(end, -8000, -3000), (92 * SECOND, 97 * SECOND));
    }

    #[test]
    fn only_the_picked_moments_stay_in_the_gallery() {
        let clip = |name: &str, picked: bool| MomentClip {
            path: PathBuf::from(name),
            preview: None,
            name: String::new(),
            picked,
        };
        let clips = [
            clip("a.mp4", true),
            clip("b.mp4", false),
            clip("c.mp4", true),
        ];
        let (kept, dropped) = sorted(&clips);
        assert_eq!(kept, [PathBuf::from("a.mp4"), PathBuf::from("c.mp4")]);
        assert_eq!(dropped, [PathBuf::from("b.mp4")]);
    }

    #[test]
    fn a_moment_without_a_past_still_has_its_length() {
        let end = 100 * SECOND;
        assert_eq!(span(end, 0, 4000), (96 * SECOND, end));
        assert_eq!(span(end, 0, 0), (end, end));
    }
}
