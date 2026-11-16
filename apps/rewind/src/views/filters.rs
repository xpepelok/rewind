use gpui::{AnyElement, Context, Div, ElementId, Entity, SharedString, div, prelude::*, px};

use rewind_config::{Command, STYLE_SLOTS, game_key};
use rewind_nvbus::ansel::{
    Capture, Component, Control, ControlChange, ControlKind, GameControl, Scalar, Shot,
};
use rewind_nvbus::session::{Command as Lens, GameEntry, Layer, Mode, Shooting, Span, State};

use crate::assets::{Icon, icon};
use crate::state::Rewind;
use crate::theme::{GUTTER, Palette, THEMED_CONTENT_INSET, ThemeExt};
use crate::ui::scrollbar::with_scrollbar;
use crate::ui::{
    ButtonKind, Selector, Slider, Toggle, Tooltip, button, checkbox, divider, hint, label, section,
    setting_row, thumbnail,
};
use crate::views::overlay::{Overlay, Page};
use crate::views::pages::{hotkey_text, scroll, slider_row, title_block};

const MAX_STEPS: u32 = 1000;
const DEFAULT_STEPS: f32 = 100.0;
const TOOL_ICON: f32 = 20.0;
const PANORAMA_STEP: f32 = 256.0;
const PROFILE_TILE: f32 = 60.0;
const PROFILE_NAME: f32 = 18.0;
const PROFILE_GAP: f32 = 2.0;
const FILTER_ROW: f32 = 6.0;
const BANNER_INSET: f32 = 8.0;
const BANNER_PADDING: f32 = 12.0;
const PHOTO_CARD_GAP: f32 = 4.0;
const PHOTO_CONTROL_PADDING: f32 = 12.0;
const PHOTO_BOTTOM_PADDING: f32 = 88.0;
const STYLE_NONE: &str = "Нет";
const STYLE_OFF: &str = "без фильтров";
const STYLE_EMPTY: &str = "пусто";
const FILTERS_HELPER: &str =
    "Позволяет настроить внешний вид игр, применяя фильтры постобработки в реальном времени.";
const TURN_ON_FILTERS: &str = "Включите, чтобы изменить внешний вид игры";
const TURN_ON_PHOTO: &str = "Включите, чтобы делать снимки в игре";
const NO_GAME: &str = "Запустите поддерживаемую игру";
const CONNECTING: &str = "Применение фильтров";
const NOT_AVAILABLE: &str = "В данный момент отсутствует";
const NO_STYLE: &str = "Нет активных фильтров. Выберите профиль, чтобы начать.";
const ADD_FROM_AVAILABLE: &str = "Добавить из доступных фильтров";
const CLEAR_ALL: &str = "Очистить все фильтры";
const MOVE_HELPER: &str = "Перемещайтесь по сцене с помощью мыши и клавиш со стрелками";
const HIDE_HELPER: &str = "Нажмите H, чтобы скрыть/показать эту панель";
const SHOOT: &str = "СНИМОК";
const PROFILES: &str = "Профили";
const CAMERA: &str = "Камера";
const AVAILABLE: &str = "Доступные фильтры";
const RESOLUTION: &str = "Разрешение";
const GRID: &str = "Показать сетку";
const CAMERA_SWITCH: &str = "Игровые фильтры и фоторежим";
const CAPTURE_SETUP: &str = "Настроить захват видео";
const CAPTURE_SETUP_HINT: &str = "Длительность мгновенного повтора, формат вывода";
const RTX_FILTERS: [&str; 2] = [
    "RTX HDR",
    "Увеличение резкости (масштабирование изображения)",
];
const SHOOTING: &str = "Создание снимка...";
const SHOT_FAILED: &str = "Не удалось сделать снимок сцены из-за внутренней ошибки";
const HIGHLIGHTS_ON: &str = "Автоматическая запись важных моментов игры";
const HIGHLIGHTS_OFF: &str = "Включите, чтобы автоматически записывать важные моменты игры";
const HIGHLIGHTS_HELPER: &str =
    "Включите, чтобы автоматически записывать лучшие моменты в поддерживаемых играх.";
const HIGHLIGHTS_GAME: &str = "Записывать лучшие моменты в этой игре";
const SUMMARY_NOTE: &str = "Примечание. Несохраненные клипы удаляются автоматически";
const SUMMARY_HELPER: &str = "Захватывающие моменты из вашего последнего игрового сеанса были записаны автоматически. Просмотрите и сохраните понравившиеся клипы в Галерее";
const HIGHLIGHTS_NEED_REPLAY: &str =
    "Включите мгновенный повтор: лучшие моменты записываются с его помощью.";
const HIGHLIGHTS_NO_GAMES: &str =
    "Игры, поддерживающие создание лучших моментов, появятся здесь после первого запуска.";
const FILTER_NAMES: [(&str, &str); 19] = [
    ("autodepthoffield", "Автоматическая глубина резкости"),
    ("black&white", "Черно-белый"),
    ("brightness/contrast", "Экспозиция / Контраст"),
    ("color", "Цвет"),
    ("colorblind", "Режим для дальтоников"),
    ("depthoffield", "Глубина резкости"),
    ("details", "Подробнее"),
    ("letterbox", "Полный кадр"),
    ("nightmode", "Ночной режим"),
    ("oldfilm", "Старая пленка"),
    ("painterly", "Живопись"),
    ("sharpen", "Увеличение резкости"),
    ("sharpen+", "Увеличение резкости+"),
    ("specialfx", "Спецэффекты"),
    ("splitscreen", "Разделение экрана"),
    ("tilt-shift", "Шифт-объектив"),
    ("vignette", "Виньетка"),
    ("watercolor", "Акварель"),
    ("none", "Нет"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhotoChoice {
    pub shot: Shot,
    pub multiplier: usize,
    pub panorama: u32,
    pub hdr: bool,
    pub enhance: bool,
    pub grid: bool,
}

impl Default for PhotoChoice {
    fn default() -> Self {
        Self {
            shot: Shot::Regular,
            multiplier: 0,
            panorama: 0,
            hdr: false,
            enhance: false,
            grid: false,
        }
    }
}

pub(super) fn filter_name(name: &str) -> String {
    let key: String = name
        .chars()
        .filter(|letter| !letter.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    FILTER_NAMES
        .iter()
        .find(|(english, _)| *english == key)
        .map_or_else(|| name.to_owned(), |(_, russian)| (*russian).to_owned())
}

fn usable(rewind: &Rewind) -> bool {
    rewind.lens.present
        && rewind
            .lens
            .features
            .is_some_and(|features| features.filters)
}

pub(super) fn filters_subtitle(rewind: &Rewind) -> String {
    if !rewind.settings.filters.enabled {
        return TURN_ON_FILTERS.to_owned();
    }
    let hotkey = hotkey_text(rewind, Command::Filters);
    if !usable(rewind) {
        return format!("{hotkey} — изменить фильтры");
    }
    match rewind.active_style() {
        Some(slot) => format!("{hotkey} — изменить фильтры (стиль {})", slot + 1),
        None => format!("{hotkey} — изменить фильтры (нет)"),
    }
}

pub(super) fn photo_subtitle(rewind: &Rewind) -> String {
    if rewind.settings.filters.enabled {
        format!("{} — фоторежим", hotkey_text(rewind, Command::PhotoMode))
    } else {
        TURN_ON_PHOTO.to_owned()
    }
}

pub(super) fn highlights_subtitle(rewind: &Rewind) -> &'static str {
    if rewind.settings.highlights.enabled {
        HIGHLIGHTS_ON
    } else {
        HIGHLIGHTS_OFF
    }
}

fn steps(minimum: f32, maximum: f32, step: f32) -> u32 {
    if maximum <= minimum {
        return 1;
    }
    let step = if step > 0.0 {
        step
    } else {
        (maximum - minimum) / DEFAULT_STEPS
    };
    (((maximum - minimum) / step).round() as u32).clamp(1, MAX_STEPS)
}

fn position(value: f32, minimum: f32, maximum: f32, count: u32) -> u32 {
    if maximum <= minimum {
        return 0;
    }
    let share = ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0);
    (share * count as f32).round() as u32
}

fn value_at(position: u32, minimum: f32, maximum: f32, count: u32) -> f32 {
    minimum + (maximum - minimum) * position.min(count) as f32 / count.max(1) as f32
}

fn shown(control: &Control, component: &Component) -> String {
    let (low, high) = (component.minimum, component.maximum);
    let scaled = control.maximum_shown > control.minimum_shown && high > low;
    let value = if scaled {
        let share = (component.current - low) / (high - low);
        control.minimum_shown + share * (control.maximum_shown - control.minimum_shown)
    } else {
        component.current
    };
    let digits = control.precision.clamp(0, 4) as usize;
    let digits = if scaled || control.scalar != Scalar::Float || digits > 0 {
        digits
    } else {
        2
    };
    format!("{value:.digits$}{}", control.unit)
}

fn shot_label(shot: Shot, hdr: bool) -> &'static str {
    match shot {
        Shot::Regular if hdr => "Снимок экрана (EXR)",
        Shot::Regular => "Снимок экрана",
        Shot::Stereo => "Снимок экрана (стерео)",
        Shot::Highres => "Сверхвысокое разрешение",
        Shot::Panorama => "Панорамное фото",
        Shot::PanoramaStereo => "Панорамное фото (стерео)",
    }
}

fn allowed(state: &State) -> Vec<Shot> {
    Shot::ALL
        .into_iter()
        .enumerate()
        .filter(|(index, _)| state.shots.get(*index).copied().unwrap_or(*index == 0))
        .map(|(_, shot)| shot)
        .collect()
}

fn banner(text: &'static str, p: &Palette) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(GUTTER))
        .py(px(10.0))
        .bg(p.card.unwrap_or(p.header))
        .when(p.soft, |banner| {
            banner
                .mx(px(BANNER_INSET))
                .mt(px(BANNER_INSET))
                .px(px(BANNER_PADDING))
                .py(px(BANNER_PADDING))
                .rounded(px(p.radius()))
        })
        .when_some(p.row_line, |banner, line| {
            banner.border_1().border_color(line)
        })
        .child(icon(Icon::Info, TOOL_ICON, p.text))
        .child(label(text, p.text))
}

fn framed(
    top: Option<Div>,
    body: gpui::Stateful<Div>,
    bar: &'static str,
    handle: &gpui::ScrollHandle,
    bottom: Option<Div>,
) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(top)
        .child(with_scrollbar(body.into_any_element(), bar, handle))
        .children(bottom)
        .into_any_element()
}

fn offline_filters(rtx: bool) -> Vec<&'static str> {
    RTX_FILTERS
        .into_iter()
        .filter(|_| rtx)
        .chain(
            FILTER_NAMES
                .iter()
                .map(|(_, russian)| *russian)
                .filter(|name| *name != STYLE_NONE),
        )
        .collect()
}

fn offline_list(rtx: bool, p: &Palette) -> Div {
    let names = offline_filters(rtx);
    let mut body = div()
        .flex()
        .flex_col()
        .child(section(format!("{AVAILABLE} ({})", names.len()), p));
    for name in names {
        body = body.child(
            div()
                .px(px(if p.soft { THEMED_CONTENT_INSET } else { GUTTER }))
                .py(px(FILTER_ROW))
                .child(label(name, p.text)),
        );
    }
    body
}

fn idle_capture_rows(p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_col()
                .px(px(if p.soft {
                    PHOTO_CONTROL_PADDING
                } else {
                    GUTTER
                }))
                .py(px(8.0))
                .when(p.soft, |row| {
                    row.mx(px(BANNER_INSET))
                        .my(px(PHOTO_CARD_GAP))
                        .rounded(px(p.radius()))
                })
                .when_some(p.card, |row, card| row.bg(card))
                .when_some(p.row_line, |row, line| row.border_1().border_color(line))
                .child(label(RESOLUTION, p.text))
                .child(
                    div()
                        .pt(px(10.0))
                        .pl(px(8.0))
                        .child(Slider::new("photo-idle-size", 0, 0, 1, 1).disabled(true)),
                ),
        )
        .child(
            setting_row(p)
                .child(label(GRID, p.text).flex_1())
                .child(Toggle::new("photo-idle-grid", false).disabled(true)),
        )
}

fn angle_row(
    title: &str,
    id: &'static str,
    range: Option<(f32, f32, f32)>,
    state: &Entity<Rewind>,
    command: fn(f32) -> Lens,
    p: &Palette,
) -> Div {
    let Some((low, high, value)) = range.filter(|(low, high, _)| high > low) else {
        return slider_row(
            title,
            Slider::new(id, 0, 0, 1, 1).disabled(true),
            "0".to_owned(),
            p,
        );
    };
    let count = steps(low, high, 1.0);
    let state = state.clone();
    slider_row(
        title,
        Slider::new(id, position(value, low, high, count), 0, count, 1).on_input(
            move |at, _, cx| {
                let value = value_at(at, low, high, count);
                state.update(cx, |rewind, _| rewind.lens_send(command(value)));
            },
        ),
        format!("{value:.0}"),
        p,
    )
}

fn notice(text: &'static str, p: &Palette) -> Div {
    div()
        .px(px(if p.soft { THEMED_CONTENT_INSET } else { GUTTER }))
        .py(px(12.0))
        .child(hint(text, p))
}

fn notice_text(text: String, p: &Palette) -> AnyElement {
    div()
        .px(px(GUTTER))
        .py(px(6.0))
        .child(hint(text, p))
        .into_any_element()
}

fn tool(
    id: impl Into<ElementId>,
    glyph: Icon,
    tip: &'static str,
    p: &Palette,
) -> gpui::Stateful<Div> {
    let hover = p.hover;
    div()
        .id(id)
        .flex_none()
        .p(px(6.0))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .tooltip(Tooltip::text(tip))
        .child(icon(glyph, TOOL_ICON, p.text))
}

fn send(
    state: &Entity<Rewind>,
    command: Lens,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) + 'static {
    let state = state.clone();
    move |_, cx| state.update(cx, |rewind, _| rewind.lens_send(command.clone()))
}

fn control_rows(
    state: &Entity<Rewind>,
    index: usize,
    order: usize,
    control: &Control,
    p: &Palette,
) -> Vec<AnyElement> {
    let many = control.components.len() > 1;
    let key = index * 10_000 + order * 10;
    match control.kind {
        ControlKind::Label => return vec![notice_text(control.name.clone(), p)],
        ControlKind::Button | ControlKind::Edit | ControlKind::Pulldown => return Vec::new(),
        _ => {}
    }
    control
        .components
        .iter()
        .enumerate()
        .map(|(part, component)| {
            let title = if many && !component.name.is_empty() {
                format!("{} — {}", control.name, component.name)
            } else {
                control.name.clone()
            };
            let id = control.id;
            let set = move |value: f32| Lens::Set {
                layer: index,
                control: id,
                component: part,
                value,
            };
            if control.scalar == Scalar::Bool || control.kind == ControlKind::Boolean {
                let on = component.current != 0.0;
                return setting_row(p)
                    .child(label(title, p.text).flex_1())
                    .child(
                        Toggle::new(("filter-toggle", key + part), on)
                            .on_click(send(state, set(f32::from(u8::from(!on))))),
                    )
                    .into_any_element();
            }
            if !control.labels.is_empty() {
                let last = control.labels.len() as i32 - 1;
                let current = (component.current.round() as i32).clamp(0, last);
                let name = control.labels[current as usize].clone();
                let state = state.clone();
                return setting_row(p)
                    .child(label(title, p.text).flex_1())
                    .child(Selector::new(("filter-choice", key + part), name).on_step(
                        move |forward, _, cx| {
                            let next = if forward { current + 1 } else { current - 1 };
                            let next = next.rem_euclid(last + 1);
                            state.update(cx, |rewind, _| rewind.lens_send(set(next as f32)));
                        },
                    ))
                    .into_any_element();
            }
            let (low, high) = (component.minimum, component.maximum);
            let count = steps(low, high, component.step);
            let state = state.clone();
            slider_row(
                &title,
                Slider::new(
                    ("filter-slider", key + part),
                    position(component.current, low, high, count),
                    0,
                    count,
                    1,
                )
                .on_input(move |value, _, cx| {
                    let value = value_at(value, low, high, count);
                    state.update(cx, |rewind, _| rewind.lens_send(set(value)));
                }),
                shown(control, component),
                p,
            )
            .into_any_element()
        })
        .collect()
}

fn game_rows(state: &Entity<Rewind>, entry: &GameEntry, p: &Palette) -> Option<AnyElement> {
    if !entry.visible {
        return None;
    }
    let id = entry.id;
    let key = id.unsigned_abs() as usize;
    let change = move |change: ControlChange| Lens::Game {
        control: id,
        change,
    };
    Some(match &entry.control {
        GameControl::Label => notice_text(entry.name.clone(), p),
        GameControl::Edit(_) => return None,
        GameControl::Button => setting_row(p)
            .child(
                button(
                    ("game-button", key),
                    entry.name.clone(),
                    ButtonKind::Secondary,
                    true,
                    p,
                )
                .on_click({
                    let press = send(state, change(ControlChange::Pressed));
                    move |_, window, cx| press(window, cx)
                }),
            )
            .into_any_element(),
        GameControl::Toggle(on) => {
            let on = *on;
            setting_row(p)
                .child(label(entry.name.clone(), p.text).flex_1())
                .child(
                    Toggle::new(("game-toggle", key), on)
                        .on_click(send(state, change(ControlChange::Toggle(!on)))),
                )
                .into_any_element()
        }
        GameControl::List { entries, selected } => {
            let found = entries.iter().position(|(key, _)| key == selected);
            let name = found.map_or_else(String::new, |index| entries[index].1.clone());
            let keys: Vec<i32> = entries.iter().map(|(key, _)| *key).collect();
            let at = found.unwrap_or(0) as i32;
            let state = state.clone();
            setting_row(p)
                .child(label(entry.name.clone(), p.text).flex_1())
                .child(
                    Selector::new(("game-choice", key), name).on_step(move |forward, _, cx| {
                        if keys.is_empty() {
                            return;
                        }
                        let step = if forward { 1 } else { -1 };
                        let next = (at + step).rem_euclid(keys.len() as i32);
                        let picked = change(ControlChange::Selected(keys[next as usize]));
                        state.update(cx, |rewind, _| rewind.lens_send(picked));
                    }),
                )
                .into_any_element()
        }
        GameControl::Slider {
            minimum,
            maximum,
            value,
            step,
        } => {
            let (low, high) = (*minimum, *maximum);
            let count = steps(low, high, *step);
            let state = state.clone();
            slider_row(
                &entry.name,
                Slider::new(
                    ("game-slider", key),
                    position(*value, low, high, count),
                    0,
                    count,
                    1,
                )
                .on_input(move |at, _, cx| {
                    let moved = change(ControlChange::Slider(value_at(at, low, high, count)));
                    state.update(cx, |rewind, _| rewind.lens_send(moved));
                }),
                format!("{value:.1}"),
                p,
            )
            .into_any_element()
        }
    })
}

impl Overlay {
    fn camera_switch(&self, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let enabled = self.state.read(cx).settings.filters.enabled;
        let state = self.state.clone();
        setting_row(&p)
            .child(title_block(
                CAMERA_SWITCH,
                Some(FILTERS_HELPER.to_owned()),
                &p,
            ))
            .child(
                Toggle::new("camera-toggle", enabled).on_click(move |_, cx| {
                    state.update(cx, |rewind, cx| rewind.set_filters_enabled(!enabled, cx));
                }),
            )
    }

    fn camera_notice(&self, photo: bool, cx: &mut Context<Self>) -> Option<&'static str> {
        let rewind = self.state.read(cx);
        let lens = &rewind.lens;
        if !rewind.settings.filters.enabled {
            return Some(if photo {
                TURN_ON_PHOTO
            } else {
                TURN_ON_FILTERS
            });
        }
        if !lens.present {
            return Some(NO_GAME);
        }
        let Some(features) = lens.features else {
            return Some(CONNECTING);
        };
        if (photo && !features.photo) || (!photo && !features.filters) {
            return Some(NOT_AVAILABLE);
        }
        None
    }

    fn layer_block(
        &self,
        index: usize,
        count: usize,
        layer: &Layer,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = cx.palette();
        let state = self.state.clone();
        let folded = self.folded.contains(&index);
        let hover = p.hover;
        let name = filter_name(&layer.name);
        let removed = name.clone();
        let header = setting_row(&p)
            .child(
                div()
                    .id(("layer-fold", index))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(label(name, p.title))
                    .on_click(cx.listener(move |overlay, _, _, cx| {
                        if !overlay.folded.remove(&index) {
                            overlay.folded.insert(index);
                        }
                        cx.notify();
                    })),
            )
            .when(index > 0, |row| {
                row.child(
                    tool(("layer-up", index), Icon::ArrowUp, "Выше", &p).on_click({
                        let lift = send(
                            &state,
                            Lens::Move {
                                from: index,
                                to: index - 1,
                            },
                        );
                        move |_, window, cx| lift(window, cx)
                    }),
                )
            })
            .when(index + 1 < count, |row| {
                row.child(
                    tool(("layer-down", index), Icon::ArrowDown, "Ниже", &p).on_click({
                        let lower = send(
                            &state,
                            Lens::Move {
                                from: index,
                                to: index + 1,
                            },
                        );
                        move |_, window, cx| lower(window, cx)
                    }),
                )
            })
            .child(
                tool(("layer-reset", index), Icon::Restore, "Сбросить", &p).on_click({
                    let reset = send(&state, Lens::ResetLayer(index));
                    move |_, window, cx| reset(window, cx)
                }),
            )
            .child(
                tool(
                    ("layer-remove", index),
                    Icon::Delete,
                    "Удалить этот фильтр",
                    &p,
                )
                .on_click(cx.listener(move |overlay, _, _, cx| {
                    overlay.folded.clear();
                    let name = removed.clone();
                    overlay.state.update(cx, |rewind, cx| {
                        rewind.remove_filter(index, &name, cx);
                    });
                })),
            );
        let rows = layer
            .controls
            .iter()
            .enumerate()
            .filter(|_| !folded)
            .flat_map(|(order, control)| control_rows(&state, index, order, control, &p));
        div().flex().flex_col().child(header).children(rows)
    }

    fn profile_tiles(
        &self,
        active: Option<usize>,
        live: bool,
        sizes: [usize; STYLE_SLOTS],
        cx: &mut Context<Self>,
    ) -> Div {
        let p = cx.palette();
        let mut row = div().flex().gap(px(8.0)).px(px(GUTTER)).py(px(8.0));
        for slot in std::iter::once(None).chain((0..STYLE_SLOTS).map(Some)) {
            let chosen = live && slot == active;
            let hover = p.hover;
            let name: SharedString = match slot {
                Some(slot) => (slot + 1).to_string().into(),
                None => STYLE_NONE.into(),
            };
            let (ink, line) = match (live, chosen) {
                (true, true) => (p.accent, p.accent),
                (true, false) => (p.title, p.divider),
                (false, _) => (p.disabled, p.divider),
            };
            let tile = div()
                .id(("filter-style", slot.map_or(0, |slot| slot + 1)))
                .flex_1()
                .h(px(PROFILE_TILE))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(PROFILE_GAP))
                .bg(p.card.unwrap_or(p.header))
                .rounded(px(p.radius()))
                .border_1()
                .border_color(line)
                .child(label(name, ink).text_size(px(PROFILE_NAME)));
            row = row.child(if live {
                tile.cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(hint(style_caption(slot, sizes), &p))
                    .on_click(cx.listener(move |overlay, _, _, cx| {
                        overlay.folded.clear();
                        overlay
                            .state
                            .update(cx, |rewind, cx| rewind.pick_style(slot, cx));
                    }))
            } else {
                tile
            });
        }
        row
    }

    fn active_filters(&mut self, lens: &State, hint: bool, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let count = lens.layers.len();
        let mut body = div().flex().flex_col();
        if hint {
            body = body.child(notice(NO_STYLE, &p));
        }
        body = body.child(section(format!("Активные фильтры ({count})"), &p));
        if lens.layers.is_empty() {
            body = body.child(notice(ADD_FROM_AVAILABLE, &p));
        }
        for (index, layer) in lens.layers.iter().enumerate() {
            body = body.child(self.layer_block(index, count, layer, cx));
        }
        if count > 0 {
            body = body.child(setting_row(&p).child(
                button("filter-clear", CLEAR_ALL, ButtonKind::Secondary, true, &p).on_click(
                    cx.listener(|overlay, _, _, cx| {
                        overlay.folded.clear();
                        overlay
                            .state
                            .update(cx, |rewind, cx| rewind.clear_filters(cx));
                    }),
                ),
            ));
        }
        body
    }

    fn available(&mut self, lens: &State, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let mut body = div()
            .flex()
            .flex_col()
            .child(section(format!("{AVAILABLE} ({})", lens.filters.len()), &p));
        for (index, filter) in lens.filters.iter().enumerate() {
            let id = filter.id.clone();
            let hover = p.hover;
            body = body.child(
                div()
                    .id(("filter-pick", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(if p.soft { THEMED_CONTENT_INSET } else { GUTTER }))
                    .py(px(FILTER_ROW))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .tooltip(Tooltip::text("Добавить фильтр"))
                    .child(label(filter_name(&filter.name), p.text))
                    .child(icon(Icon::Plus, TOOL_ICON, p.icon))
                    .on_click(cx.listener(move |overlay, _, _, cx| {
                        let id = id.clone();
                        overlay
                            .state
                            .update(cx, |rewind, cx| rewind.add_filter(id, cx));
                    })),
            );
        }
        body
    }

    pub(super) fn filters_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let lens = rewind.lens.clone();
        let enabled = rewind.settings.filters.enabled;
        let active = rewind.active_style();
        let sizes = rewind
            .settings
            .filters
            .game(&lens.path)
            .map(|game| game.styles.each_ref().map(Vec::len))
            .unwrap_or_default();
        let trouble = self.camera_notice(false, cx);
        let live = trouble.is_none();
        let mut page = scroll("filters-scroll")
            .track_scroll(self.page_scroll(Page::Filters))
            .pt(px(8.0));
        if !enabled {
            page = page.child(self.camera_switch(cx)).child(divider(&p));
        }
        page = page
            .child(section(PROFILES, &p))
            .child(self.profile_tiles(active, live, sizes, cx))
            .child(divider(&p));
        page = if live {
            let hint = active.is_none() && lens.layers.is_empty();
            page.child(self.active_filters(&lens, hint, cx))
                .child(divider(&p))
                .child(self.available(&lens, cx))
        } else {
            page.child(offline_list(true, &p))
        };
        if enabled {
            page = page.child(divider(&p)).child(self.camera_switch(cx));
        }
        framed(
            trouble.map(|text| banner(text, &p)),
            page,
            "filters-bar",
            self.page_scroll(Page::Filters),
            None,
        )
    }

    fn capture_rows(&mut self, lens: &State, cx: &mut Context<Self>) -> (Div, Capture) {
        let p = cx.palette();
        let state = self.state.clone();
        let choice = self.photo_choice;
        let overlay = cx.entity();
        let mut body = div().flex().flex_col();
        let kinds = allowed(lens);
        let shot = if kinds.contains(&choice.shot) {
            choice.shot
        } else {
            Shot::Regular
        };
        let hdr = choice.hdr && lens.hdr;
        body = body.child(
            setting_row(&p)
                .child(label("Снимок", p.text).flex_1())
                .child(Selector::new("photo-shot", shot_label(shot, hdr)).on_step({
                    let overlay = overlay.clone();
                    move |forward, _, cx| {
                        overlay.update(cx, |overlay, cx| {
                            let kinds = allowed(&overlay.state.read(cx).lens);
                            let at = kinds.iter().position(|kind| *kind == shot).unwrap_or(0);
                            let next = if forward {
                                at + 1
                            } else {
                                at + kinds.len() - 1
                            };
                            overlay.photo_choice.shot = kinds[next % kinds.len().max(1)];
                            cx.notify();
                        });
                    }
                })),
        );
        if shot == Shot::Highres && !lens.highres.is_empty() {
            let at = choice.multiplier.min(lens.highres.len() - 1);
            let entry = lens.highres[at];
            let total = lens.highres.len();
            body = body.child(
                setting_row(&p)
                    .child(label(RESOLUTION, p.text).flex_1())
                    .child(
                        Selector::new(
                            "photo-highres",
                            format!("{}×{} ({}x)", entry.width, entry.height, entry.multiplier),
                        )
                        .on_step({
                            let overlay = overlay.clone();
                            move |forward, _, cx| {
                                overlay.update(cx, |overlay, cx| {
                                    let next = if forward { at + 1 } else { at + total - 1 };
                                    overlay.photo_choice.multiplier = next % total;
                                    cx.notify();
                                });
                            }
                        }),
                    ),
            );
            let method = if choice.enhance {
                "Улучшенный"
            } else {
                "Стандартный"
            };
            body = body.child(
                setting_row(&p)
                    .child(label("Метод", p.text).flex_1())
                    .child(Selector::new("photo-method", method).on_step({
                        let overlay = overlay.clone();
                        move |_, _, cx| {
                            overlay.update(cx, |overlay, cx| {
                                overlay.photo_choice.enhance ^= true;
                                cx.notify();
                            });
                        }
                    })),
            );
        }
        if matches!(shot, Shot::Panorama | Shot::PanoramaStereo)
            && let Some((low, high)) = lens.panorama
            && high > low
        {
            let (low, high) = (low as f32, high as f32);
            let count = steps(low, high, PANORAMA_STEP);
            let at = choice.panorama.min(count);
            body = body.child(slider_row(
                RESOLUTION,
                Slider::new("photo-panorama", at, 0, count, 1).on_input({
                    let overlay = overlay.clone();
                    move |at, _, cx| {
                        overlay.update(cx, |overlay, cx| {
                            overlay.photo_choice.panorama = at;
                            cx.notify();
                        });
                    }
                }),
                format!("{:.0}", value_at(at, low, high, count)),
                &p,
            ));
        }
        if lens.hdr {
            body = body.child(
                setting_row(&p)
                    .child(label("Сохранить как HDR", p.text).flex_1())
                    .child(Toggle::new("photo-hdr", choice.hdr).on_click({
                        let overlay = overlay.clone();
                        move |_, cx| {
                            overlay.update(cx, |overlay, cx| {
                                overlay.photo_choice.hdr ^= true;
                                cx.notify();
                            });
                        }
                    })),
            );
        }
        body = body.child(setting_row(&p).child(label(GRID, p.text).flex_1()).child(
            Toggle::new("photo-grid", choice.grid).on_click({
                let overlay = overlay.clone();
                move |_, cx| {
                    overlay.update(cx, |overlay, cx| {
                        overlay.photo_choice.grid ^= true;
                        let on = overlay.photo_choice.grid;
                        overlay.state.read(cx).lens_send(Lens::Grid(on));
                        cx.notify();
                    });
                }
            }),
        ));
        for entry in &lens.game_controls {
            body = body.children(game_rows(&state, entry, &p));
        }
        let capture = Capture {
            shot,
            multiplier: (shot == Shot::Highres)
                .then(|| {
                    lens.highres
                        .get(choice.multiplier.min(lens.highres.len().max(1) - 1))
                })
                .flatten()
                .map(|entry| entry.multiplier),
            panorama_width: lens
                .panorama
                .filter(|_| matches!(shot, Shot::Panorama | Shot::PanoramaStereo))
                .map(|(low, high)| {
                    let count = steps(low as f32, high as f32, PANORAMA_STEP);
                    value_at(choice.panorama.min(count), low as f32, high as f32, count) as i32
                }),
            exr: hdr,
            enhance: choice.enhance && shot == Shot::Highres,
            jxr: false,
        };
        (body, capture)
    }

    fn camera_block(&mut self, lens: &State, live: bool, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let folded = self.camera_folded;
        let hover = p.hover;
        let glyph = if folded {
            Icon::ArrowDown
        } else {
            Icon::ArrowUp
        };
        let block = div().flex().flex_col().child(
            div()
                .id("photo-camera")
                .flex()
                .items_center()
                .justify_between()
                .px(px(if p.soft { THEMED_CONTENT_INSET } else { GUTTER }))
                .py(px(10.0))
                .when(p.soft, |header| {
                    header
                        .mx(px(BANNER_INSET))
                        .my(px(PHOTO_CARD_GAP))
                        .px(px(PHOTO_CONTROL_PADDING))
                        .py(px(PHOTO_CONTROL_PADDING))
                        .rounded(px(p.radius()))
                })
                .when_some(p.card, |header, card| header.bg(card))
                .when_some(p.row_line, |header, line| {
                    header.border_1().border_color(line)
                })
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(label(CAMERA, p.title))
                .child(icon(glyph, TOOL_ICON, p.text))
                .on_click(cx.listener(|overlay, _, _, cx| {
                    overlay.camera_folded ^= true;
                    cx.notify();
                })),
        );
        if folded {
            return block;
        }
        let range = |span: &Option<Span>| {
            span.as_ref()
                .filter(|_| live)
                .map(|span| (span.minimum, span.maximum, span.value))
        };
        block
            .child(angle_row(
                "Угол камеры",
                "photo-roll",
                range(&lens.roll),
                &self.state,
                Lens::Roll,
                &p,
            ))
            .child(angle_row(
                "Поле зрения",
                "photo-fov",
                range(&lens.fov),
                &self.state,
                Lens::Fov,
                &p,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(if p.soft { THEMED_CONTENT_INSET } else { GUTTER }))
                    .py(px(8.0))
                    .child(icon(Icon::Info, TOOL_ICON, p.muted))
                    .child(div().flex_1().min_w_0().child(hint(MOVE_HELPER, &p))),
            )
    }

    fn photo_footer(&self, capture: Option<Capture>, lens: &State, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let state = self.state.clone();
        let (busy, progress) = match &lens.shooting {
            Shooting::Running { done, total } if *total > 0 => {
                (true, Some(format!("{SHOOTING} {done}/{total}")))
            }
            Shooting::Running { .. } => (true, Some(SHOOTING.to_owned())),
            Shooting::Saved(path) => (false, Some(path.clone())),
            Shooting::Failed(_) => (false, Some(SHOT_FAILED.to_owned())),
            Shooting::Idle => (false, None),
        };
        let shoot = match capture {
            Some(_) if busy => button("photo-abort", "ОТМЕНА", ButtonKind::Danger, true, &p)
                .on_click({
                    let abort = send(&state, Lens::Abort);
                    move |_, window, cx| abort(window, cx)
                }),
            Some(capture) => {
                button("photo-shoot", SHOOT, ButtonKind::Primary, true, &p).on_click({
                    let shoot = send(&state, Lens::Capture(capture));
                    move |_, window, cx| shoot(window, cx)
                })
            }
            None => button("photo-shoot", SHOOT, ButtonKind::Primary, false, &p),
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .children(
                progress
                    .filter(|_| capture.is_some())
                    .map(|text| notice_text(text, &p)),
            )
            .child(div().h(px(1.0)).mx(px(GUTTER)).bg(p.divider))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(GUTTER))
                    .py(px(10.0))
                    .child(icon(Icon::HidePanel, TOOL_ICON, p.muted))
                    .child(div().flex_1().min_w_0().child(hint(HIDE_HELPER, &p)))
                    .child(shoot),
            )
    }

    pub(super) fn photo_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let lens = rewind.lens.clone();
        let enabled = rewind.settings.filters.enabled;
        let trouble = self
            .camera_notice(true, cx)
            .or((lens.mode != Mode::Photo).then_some(CONNECTING));
        let live = trouble.is_none();
        let mut page = scroll("photo-scroll")
            .track_scroll(self.page_scroll(Page::PhotoMode))
            .pt(px(8.0))
            .when(p.soft, |page| page.pb(px(PHOTO_BOTTOM_PADDING)));
        if !enabled {
            page = page.child(self.camera_switch(cx)).child(divider(&p));
        }
        let capture = if live {
            let (rows, capture) = self.capture_rows(&lens, cx);
            page = page.child(rows);
            Some(capture)
        } else {
            page = page.child(idle_capture_rows(&p));
            None
        };
        page = page
            .child(divider(&p))
            .child(self.camera_block(&lens, live, cx))
            .child(divider(&p));
        page = if live {
            page.child(self.active_filters(&lens, false, cx))
                .child(divider(&p))
                .child(self.available(&lens, cx))
        } else {
            page.child(offline_list(false, &p))
        };
        if enabled {
            page = page.child(divider(&p)).child(self.camera_switch(cx));
        }
        let footer = self.photo_footer(capture, &lens, cx);
        framed(
            trouble.map(|text| banner(text, &p)),
            page,
            "photo-bar",
            self.page_scroll(Page::PhotoMode),
            Some(footer),
        )
    }

    pub(super) fn highlights_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let rewind = self.state.read(cx);
        let highlights = rewind.settings.highlights.clone();
        let replay_on = rewind.settings.replay_enabled;
        let running = rewind.moment_game.as_deref().map(game_key);
        let clips = rewind.moment_clips.len();
        let enabled = highlights.enabled;
        let hover = p.hover;
        let mut page = scroll("highlights-scroll")
            .track_scroll(self.page_scroll(Page::Highlights))
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Запись лучших моментов",
                        Some(HIGHLIGHTS_HELPER.to_owned()),
                        &p,
                    ))
                    .child(Toggle::new("highlights-toggle", enabled).on_click({
                        let state = state.clone();
                        move |_, cx| {
                            state.update(cx, |rewind, cx| {
                                rewind.set_highlights_enabled(!enabled, cx);
                            });
                        }
                    })),
            )
            .child(
                div()
                    .id("highlights-capture")
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .px(px(GUTTER))
                    .py(px(10.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(title_block(
                        CAPTURE_SETUP,
                        Some(CAPTURE_SETUP_HINT.to_owned()),
                        &p,
                    ))
                    .child(icon(Icon::Settings, TOOL_ICON, p.text))
                    .on_click(cx.listener(|overlay, _, window, cx| {
                        overlay.push(Page::Replay, window, cx);
                    })),
            )
            .child(divider(&p));
        if !enabled {
            return page.into_any_element();
        }
        if clips > 0 {
            page = page
                .child(
                    self.link(
                        "highlights-review",
                        Icon::Highlights,
                        "Просмотреть лучшие моменты",
                        Some(format!("Лучшие моменты ({clips})")),
                        Icon::ChevronRight,
                        cx,
                    )
                    .on_click(cx.listener(|overlay, _, window, cx| {
                        overlay.push(Page::Summary, window, cx);
                    })),
                )
                .child(divider(&p));
        }
        if !replay_on {
            page = page.child(notice(HIGHLIGHTS_NEED_REPLAY, &p));
        }
        if highlights.games.is_empty() {
            page = page.child(notice(HIGHLIGHTS_NO_GAMES, &p));
        }
        let mut row = 0usize;
        for (key, game) in &highlights.games {
            let name = if game.name.is_empty() {
                key
            } else {
                &game.name
            };
            let title = if running.as_deref() == Some(key.as_str()) {
                format!("{name} — запущена игра")
            } else {
                name.clone()
            };
            row += 1;
            page = page.child(section(title, &p)).child(
                setting_row(&p)
                    .child(label(HIGHLIGHTS_GAME, p.text).flex_1())
                    .child(Toggle::new(("moment-game", row), game.enabled).on_click({
                        let (state, key) = (state.clone(), key.clone());
                        move |_, cx| {
                            state.update(cx, |rewind, cx| {
                                rewind.toggle_moment_game(&key, cx);
                            });
                        }
                    })),
            );
            for kind in game.kinds.iter().filter(|_| game.enabled) {
                row += 1;
                let (game_key, kind_id) = (key.clone(), kind.id.clone());
                let state = state.clone();
                page = page.child(
                    setting_row(&p)
                        .child(label(kind.name.clone(), p.text).flex_1())
                        .child(Toggle::new(("moment-kind", row), kind.enabled).on_click(
                            move |_, cx| {
                                state.update(cx, |rewind, cx| {
                                    rewind.toggle_moment_kind(&game_key, &kind_id, cx);
                                });
                            },
                        )),
                );
            }
        }
        page.into_any_element()
    }
}

impl Overlay {
    pub(super) fn summary_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let clips = self.state.read(cx).moment_clips.clone();
        let picked = clips.iter().filter(|clip| clip.picked).count();
        let mut page = scroll("summary-scroll")
            .track_scroll(self.page_scroll(Page::Summary))
            .pt(px(8.0))
            .child(notice(SUMMARY_HELPER, &p))
            .child(section(
                format!("Выбрано лучших моментов: {picked}/{}", clips.len()),
                &p,
            ));
        for (index, clip) in clips.iter().enumerate() {
            let hover = p.hover;
            let file = clip
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = clip.path.clone();
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(GUTTER))
                    .py(px(8.0))
                    .child(
                        div()
                            .id(("summary-pick", index))
                            .cursor_pointer()
                            .child(checkbox(clip.picked, &p))
                            .on_click(cx.listener(move |overlay, _, _, cx| {
                                overlay
                                    .state
                                    .update(cx, |rewind, cx| rewind.pick_moment(index, cx));
                            })),
                    )
                    .child(thumbnail(clip.preview.clone(), Icon::Highlights, &p))
                    .child(
                        div()
                            .id(("summary-open", index))
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .child(title_block(clip.name.clone(), Some(file), &p))
                            .on_click(cx.listener(move |overlay, _, window, cx| {
                                overlay.open_viewer(path.clone(), window, cx);
                            })),
                    ),
            );
        }
        page.child(notice(SUMMARY_NOTE, &p))
            .child(divider(&p))
            .child(
                setting_row(&p)
                    .child(
                        button(
                            "summary-keep",
                            format!("Сохранить в Галерее ({picked})"),
                            ButtonKind::Primary,
                            !clips.is_empty(),
                            &p,
                        )
                        .on_click(cx.listener(|overlay, _, window, cx| {
                            overlay
                                .state
                                .update(cx, |rewind, cx| rewind.keep_moments(cx));
                            overlay.pop(window, cx);
                        })),
                    )
                    .child(
                        button(
                            "summary-discard",
                            "Удалить все",
                            ButtonKind::Danger,
                            !clips.is_empty(),
                            &p,
                        )
                        .on_click(cx.listener(|overlay, _, window, cx| {
                            overlay
                                .state
                                .update(cx, |rewind, cx| rewind.discard_moments(cx));
                            overlay.pop(window, cx);
                        })),
                    ),
            )
            .into_any_element()
    }
}

pub(super) fn steer_key(key: &str) -> Option<i32> {
    Some(match key {
        "w" | "ц" => 0x57,
        "a" | "ф" => 0x41,
        "s" | "ы" => 0x53,
        "d" | "в" => 0x44,
        "z" | "я" => 0x5A,
        "x" | "ч" => 0x58,
        "q" | "й" => 0x51,
        "e" | "у" => 0x45,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "space" => 0x20,
        _ => return None,
    })
}

pub(super) fn packed(horizontal: f32, vertical: f32) -> i32 {
    let low = (horizontal.round() as i32).clamp(-32_768, 32_767) as i16 as u16;
    let high = (vertical.round() as i32).clamp(-32_768, 32_767) as i16 as u16;
    (u32::from(high) << 16 | u32::from(low)) as i32
}

fn style_caption(slot: Option<usize>, sizes: [usize; STYLE_SLOTS]) -> String {
    match slot.map(|slot| sizes[slot]) {
        None => STYLE_OFF.to_owned(),
        Some(0) => STYLE_EMPTY.to_owned(),
        Some(count) => format!("фильтров: {count}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_list_without_a_game_matches_the_counts_nvidia_shows() {
        let photo = offline_filters(false);
        let filters = offline_filters(true);
        assert_eq!(photo.len(), 18);
        assert_eq!(filters.len(), 20);
        assert_eq!(filters[0], "RTX HDR");
        assert_eq!(photo[0], "Автоматическая глубина резкости");
        assert!(!filters.contains(&STYLE_NONE));
    }

    fn control(low_shown: f32, high_shown: f32, precision: i32, unit: &str) -> Control {
        Control {
            id: 1,
            name: "Intensity".to_owned(),
            kind: ControlKind::Slider,
            scalar: Scalar::Float,
            minimum_shown: low_shown,
            maximum_shown: high_shown,
            precision,
            unit: unit.to_owned(),
            labels: Vec::new(),
            tooltip: String::new(),
            components: Vec::new(),
        }
    }

    fn component(current: f32, minimum: f32, maximum: f32) -> Component {
        Component {
            current,
            default: 0.0,
            minimum,
            maximum,
            step: 0.01,
            name: String::new(),
        }
    }

    #[test]
    fn a_slider_walks_its_whole_range_in_whole_steps() {
        let count = steps(-1.0, 1.0, 0.01);
        assert_eq!(count, 200);
        assert_eq!(position(-1.0, -1.0, 1.0, count), 0);
        assert_eq!(position(0.0, -1.0, 1.0, count), 100);
        assert_eq!(position(9.0, -1.0, 1.0, count), 200);
        assert_eq!(value_at(150, -1.0, 1.0, count), 0.5);
        assert_eq!(value_at(999, -1.0, 1.0, count), 1.0);
    }

    #[test]
    fn a_range_without_a_step_or_without_width_still_makes_a_slider() {
        assert_eq!(steps(0.0, 1.0, 0.0), 100);
        assert_eq!(steps(0.0, 1.0, 0.000_001), MAX_STEPS);
        assert_eq!(steps(2.0, 2.0, 0.1), 1);
        assert_eq!(position(2.0, 2.0, 2.0, 1), 0);
    }

    #[test]
    fn values_are_shown_on_the_scale_the_filter_asks_for() {
        let percent = control(0.0, 100.0, 0, "%");
        assert_eq!(shown(&percent, &component(0.25, 0.0, 1.0)), "25%");
        let signed = control(-100.0, 100.0, 0, "");
        assert_eq!(shown(&signed, &component(0.0, -1.0, 1.0)), "0");
        let raw = control(0.0, 0.0, 0, "");
        assert_eq!(shown(&raw, &component(0.5, 0.0, 1.0)), "0.50");
        let precise = control(0.0, 10.0, 1, "");
        assert_eq!(shown(&precise, &component(0.33, 0.0, 1.0)), "3.3");
    }

    #[test]
    fn filters_carry_the_names_the_nvidia_overlay_gives_them() {
        assert_eq!(
            filter_name("Brightness / Contrast"),
            "Экспозиция / Контраст"
        );
        assert_eq!(filter_name("Depth of field"), "Глубина резкости");
        assert_eq!(filter_name("Sharpen+"), "Увеличение резкости+");
        assert_eq!(filter_name("Sharpen"), "Увеличение резкости");
        assert_eq!(filter_name("Remove Hud [Beta]"), "Remove Hud [Beta]");
    }

    #[test]
    fn camera_keys_work_in_either_keyboard_layout() {
        assert_eq!(steer_key("w"), Some(0x57));
        assert_eq!(steer_key("ц"), Some(0x57));
        assert_eq!(steer_key("left"), Some(0x25));
        assert_eq!(steer_key("f"), None);
    }

    #[test]
    fn a_mouse_step_is_packed_as_two_signed_halves() {
        assert_eq!(packed(3.0, 2.0), 0x0002_0003);
        assert_eq!(packed(-1.0, 0.0), 0x0000_FFFF);
        assert_eq!(packed(0.0, -1.0), 0xFFFF_0000_u32 as i32);
        assert_eq!(packed(1e9, 0.0), 0x0000_7FFF);
    }

    #[test]
    fn only_shots_the_game_allows_are_offered_and_a_plain_one_always_is() {
        let mut state = State::default();
        assert_eq!(allowed(&state), [Shot::Regular]);
        state.shots = vec![true, false, true, true, false];
        assert_eq!(
            allowed(&state),
            [Shot::Regular, Shot::Highres, Shot::Panorama]
        );
        assert_eq!(shot_label(Shot::Regular, true), "Снимок экрана (EXR)");
    }
}
