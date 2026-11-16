use std::path::PathBuf;

use gpui::{
    AnyElement, Context, MouseButton, ObjectFit, SharedString, div, img, prelude::*, px, relative,
};

use rewind_capture::ReplayWindow;

use crate::theme::{SMALL_TEXT, ThemeExt, alpha};
use crate::ui::{ButtonKind, Slider, button, hint, label, value_box};
use crate::views::overlay::Overlay;
use crate::views::pages::{clock, span_text};

const HNS_PER_SECOND: i64 = 10_000_000;
const DIALOG_WIDTH: f32 = 640.0;
const TRACK_HEIGHT: f32 = 36.0;
const MARK_WIDTH: f32 = 3.0;
const TIME_WIDTH: f32 = 84.0;
const MARKS_SHOWN: usize = 50;
const PREVIEW_COUNT: usize = 10;
const PREVIEW_WIDTH: u32 = 160;
const STRIP_HEIGHT: f32 = 36.0;
const PREVIEW_DIR: &str = "Rewind previews";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayPick {
    pub window: ReplayWindow,
    pub from: u32,
    pub to: u32,
    pub frames: Vec<Option<PathBuf>>,
}

impl ReplayPick {
    pub fn new(window: ReplayWindow) -> Self {
        let to = seconds_between(window.start, window.end);
        Self {
            window,
            from: 0,
            to,
            frames: Vec::new(),
        }
    }

    fn total(&self) -> u32 {
        seconds_between(self.window.start, self.window.end).max(1)
    }

    fn at(&self, seconds: u32) -> i64 {
        self.window.start + i64::from(seconds) * HNS_PER_SECOND
    }

    fn offset(&self, pts: i64) -> u32 {
        seconds_between(self.window.start, pts).min(self.total())
    }

    fn around(&self, mark: i64, before: u32, after: u32) -> (i64, i64) {
        (
            (mark - i64::from(before) * HNS_PER_SECOND).max(self.window.start),
            mark + i64::from(after) * HNS_PER_SECOND,
        )
    }

    fn whole(&self) -> (i64, i64) {
        (self.window.start, self.window.end)
    }

    fn selected(&self) -> (i64, i64) {
        (self.at(self.from), self.at(self.to))
    }
}

fn frame_times(window: &ReplayWindow) -> Vec<i64> {
    let span = (window.end - window.start).max(1);
    (0..PREVIEW_COUNT)
        .map(|index| window.start + span * (2 * index as i64 + 1) / (2 * PREVIEW_COUNT as i64))
        .collect()
}

fn seconds_between(from: i64, to: i64) -> u32 {
    ((to - from).max(0) / HNS_PER_SECOND).min(i64::from(u32::MAX)) as u32
}

fn ago(total: u32, offset: u32) -> String {
    format!("−{}", clock(total.saturating_sub(offset)))
}

impl Overlay {
    pub fn pick_replay(&mut self, window: ReplayWindow, cx: &mut Context<Self>) {
        let times = frame_times(&window);
        let previews = self.state.read(cx).previews();
        self.replay_pick = Some(ReplayPick::new(window.clone()));
        cx.notify();
        let Some(previews) = previews else {
            return;
        };
        let directory = std::env::temp_dir()
            .join(PREVIEW_DIR)
            .join(format!("{}", window.end));
        cx.spawn(async move |this, cx| {
            let target = directory.clone();
            let frames = cx
                .background_executor()
                .spawn(async move {
                    if let Some(parent) = target.parent() {
                        std::fs::remove_dir_all(parent).ok();
                    }
                    previews.render(&times, &target, PREVIEW_WIDTH)
                })
                .await;
            this.update(cx, |overlay, cx| {
                if let Some(pick) = &mut overlay.replay_pick
                    && pick.window == window
                {
                    pick.frames = frames;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn finish_pick(&mut self, ranges: Vec<(i64, i64)>, cx: &mut Context<Self>) {
        self.replay_pick = None;
        self.state
            .update(cx, |rewind, cx| rewind.save_replay_ranges(ranges, cx));
        cx.notify();
    }

    fn change_pick(&mut self, apply: impl FnOnce(&mut ReplayPick), cx: &mut Context<Self>) {
        if let Some(pick) = &mut self.replay_pick {
            apply(pick);
            pick.from = pick.from.min(pick.total().saturating_sub(1));
            pick.to = pick.to.clamp(pick.from + 1, pick.total());
            cx.notify();
        }
    }

    pub fn replay_pick_layer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = cx.palette();
        let pick = self.replay_pick.clone()?;
        let settings = self.state.read(cx).settings.replay.clone();
        let (before, after) = (settings.before_mark, settings.after_mark);
        let total = pick.total();
        let this = cx.entity().downgrade();
        let strip = (!pick.frames.is_empty()).then(|| {
            div()
                .flex()
                .h(px(STRIP_HEIGHT))
                .w_full()
                .bg(p.field)
                .overflow_hidden()
                .children(pick.frames.iter().map(|frame| {
                    div().flex_1().h_full().overflow_hidden().when_some(
                        frame.clone(),
                        |cell, frame| {
                            cell.child(img(frame).size_full().object_fit(ObjectFit::Cover))
                        },
                    )
                }))
        });
        let track = div()
            .id("replay-track")
            .relative()
            .h(px(TRACK_HEIGHT))
            .w_full()
            .bg(p.field)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(pick.from as f32 / total as f32))
                    .w(relative((pick.to - pick.from) as f32 / total as f32))
                    .bg(alpha(p.accent, 0.35))
                    .border_x_2()
                    .border_color(p.accent),
            )
            .children(pick.window.marks.iter().enumerate().map(|(index, mark)| {
                let offset = pick.offset(*mark);
                let (from, to) = pick.around(*mark, before, after);
                let range = (pick.offset(from), pick.offset(to));
                div()
                    .id(("replay-mark", index))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(offset as f32 / total as f32))
                    .w(px(MARK_WIDTH))
                    .bg(p.warning)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |overlay, _, _, cx| {
                            overlay.change_pick(
                                |pick| {
                                    pick.from = range.0;
                                    pick.to = range.1;
                                },
                                cx,
                            )
                        }),
                    )
            }));
        let scale = div()
            .flex()
            .justify_between()
            .text_size(px(SMALL_TEXT))
            .text_color(p.muted)
            .child(ago(total, 0))
            .child("сейчас");
        let slider_row = |name: &'static str, id: &'static str, value: u32, start: bool| {
            let this = this.clone();
            div()
                .flex()
                .items_center()
                .gap(px(16.0))
                .child(label(name, p.text).w(px(72.0)))
                .child(div().flex_1().pl(px(8.0)).child(
                    Slider::new(id, value, 0, total, 1).on_input(move |value, _, app| {
                        this.update(app, |overlay, cx| {
                            overlay.change_pick(
                                |pick| {
                                    if start {
                                        pick.from = value;
                                        pick.to = pick.to.max(value + 1);
                                    } else {
                                        pick.to = value;
                                        pick.from = pick.from.min(value.saturating_sub(1));
                                    }
                                },
                                cx,
                            )
                        })
                        .ok();
                    }),
                ))
                .child(value_box(ago(total, value), &p, TIME_WIDTH))
        };
        let marks = pick
            .window
            .marks
            .iter()
            .enumerate()
            .take(MARKS_SHOWN)
            .map(|(index, mark)| {
                let range = pick.around(*mark, before, after);
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        label(
                            format!("Метка {} · {}", index + 1, ago(total, pick.offset(*mark))),
                            p.text,
                        )
                        .flex_1(),
                    )
                    .child(
                        button(
                            ("mark-save", index),
                            "Сохранить клип",
                            ButtonKind::Secondary,
                            true,
                            &p,
                        )
                        .on_click(cx.listener(move |overlay, _, _, cx| {
                            overlay.finish_pick(vec![range], cx)
                        })),
                    )
            })
            .collect::<Vec<_>>();
        let all_marks: Vec<(i64, i64)> = pick
            .window
            .marks
            .iter()
            .map(|mark| pick.around(*mark, before, after))
            .collect();
        let marks_block = (!marks.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(hint(
                    SharedString::from(format!(
                        "Клип по метке: {} до и {} после.",
                        span_text(before),
                        span_text(after)
                    )),
                    &p,
                ))
                .children(marks)
                .when(all_marks.len() > 1, |block| {
                    let count = all_marks.len();
                    block.child(
                        div().flex().justify_end().child(
                            button(
                                "marks-save-all",
                                format!("Сохранить клипы по всем меткам ({count})"),
                                ButtonKind::Secondary,
                                true,
                                &p,
                            )
                            .on_click(cx.listener(
                                move |overlay, _, _, cx| overlay.finish_pick(all_marks.clone(), cx),
                            )),
                        ),
                    )
                })
        });
        let whole = pick.whole();
        let selected = pick.selected();
        let actions = div()
            .flex()
            .justify_end()
            .gap(px(10.0))
            .child(
                button("pick-cancel", "Отмена", ButtonKind::Secondary, true, &p).on_click(
                    cx.listener(|overlay, _, _, cx| {
                        overlay.replay_pick = None;
                        cx.notify();
                    }),
                ),
            )
            .child(
                button(
                    "pick-whole",
                    "Сохранить весь повтор",
                    ButtonKind::Secondary,
                    true,
                    &p,
                )
                .on_click(
                    cx.listener(move |overlay, _, _, cx| overlay.finish_pick(vec![whole], cx)),
                ),
            )
            .child(
                button(
                    "pick-range",
                    "Сохранить отрезок",
                    ButtonKind::Primary,
                    true,
                    &p,
                )
                .on_click(
                    cx.listener(move |overlay, _, _, cx| overlay.finish_pick(vec![selected], cx)),
                ),
            );
        let dialog = div()
            .id("replay-pick")
            .w(px(DIALOG_WIDTH))
            .max_h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .p(px(20.0))
            .bg(p.panel)
            .shadow(p.toast_shadow())
            .child(label("Сохранить мгновенный повтор", p.text))
            .child(hint(
                format!(
                    "В буфере {}. Выберите отрезок: двигайте начало и конец или нажмите на метку.",
                    span_text(total)
                ),
                &p,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .children(strip)
                    .child(track)
                    .child(scale),
            )
            .child(slider_row("Начало", "pick-from", pick.from, true))
            .child(slider_row("Конец", "pick-to", pick.to, false))
            .child(hint(
                format!("Отрезок: {}", span_text(pick.to - pick.from)),
                &p,
            ))
            .children(marks_block)
            .child(actions);
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(32.0))
                .bg(alpha(p.stage, 0.55))
                .occlude()
                .child(dialog)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick() -> ReplayPick {
        ReplayPick::new(ReplayWindow {
            start: 100 * HNS_PER_SECOND,
            end: 400 * HNS_PER_SECOND,
            marks: vec![110 * HNS_PER_SECOND],
        })
    }

    #[test]
    fn previews_sit_in_the_middle_of_equal_slices() {
        let times = frame_times(&pick().window);
        assert_eq!(times.len(), PREVIEW_COUNT);
        assert_eq!(times[0], 115 * HNS_PER_SECOND);
        assert!(times.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(*times.last().unwrap() < 400 * HNS_PER_SECOND);
    }

    #[test]
    fn a_fresh_pick_covers_the_whole_buffer() {
        let pick = pick();
        assert_eq!((pick.from, pick.to), (0, 300));
        assert_eq!(pick.selected(), pick.whole());
    }

    #[test]
    fn clips_around_a_mark_stay_inside_the_buffer_start() {
        let pick = pick();
        let (from, to) = pick.around(pick.window.marks[0], 30, 10);
        assert_eq!(from, pick.window.start);
        assert_eq!(to, 120 * HNS_PER_SECOND);
        assert_eq!(ago(300, 10), "−04:50");
    }
}
