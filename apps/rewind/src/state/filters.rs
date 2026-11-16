use std::rc::Rc;
use std::thread;
use std::time::Duration;

use gpui::Context;
use rewind_config::{Amount, FilterLayer, FilterValue, STYLE_SLOTS, game_key};
use rewind_nvbus::Camera;
use rewind_nvbus::session::{Command as Lens, Mode, Shooting, State, StyleLayer};

use super::{Rewind, Signal};
use crate::assets::Icon;

const STYLE_SAVE_DELAY: Duration = Duration::from_millis(400);

mod msg {
    pub const SHOT_SAVED: &str = "Снимок фоторежима сохранён";
    pub const SHOT_FAILED: &str = "Не удалось сделать снимок фоторежима";
    pub const CAMERA_FAILED: &str = "Игра отклонила команду фильтров";

    pub const NO_GAME: &str = "Запустите поддерживаемую игру";
    pub const CLEARED: &str = "Все фильтры сброшены";
    pub const ENABLED: &str =
        "Игровые фильтры включены. Перезапустите игру, чтобы увидеть эффекты фильтра.";

    pub fn removed(name: &str) -> String {
        format!("Удален фильтр «{name}»")
    }

    pub fn code(code: u32) -> String {
        format!("Код {code}")
    }

    pub fn style(slot: Option<usize>) -> String {
        match slot {
            Some(slot) => format!("Применение стиля {}", slot + 1),
            None => "Фильтры выключены".to_owned(),
        }
    }
}

pub fn saved(style: &[StyleLayer]) -> Vec<FilterLayer> {
    style
        .iter()
        .map(|layer| FilterLayer {
            filter: layer.filter.clone(),
            values: layer
                .values
                .iter()
                .map(|(control, amounts)| FilterValue {
                    control: *control,
                    amounts: amounts.iter().copied().map(Amount).collect(),
                })
                .collect(),
        })
        .collect()
}

pub fn stored(style: &[FilterLayer]) -> Vec<StyleLayer> {
    style
        .iter()
        .map(|layer| StyleLayer {
            filter: layer.filter.clone(),
            values: layer
                .values
                .iter()
                .map(|value| {
                    let amounts = value.amounts.iter().map(|amount| amount.0);
                    (value.control, amounts.collect())
                })
                .collect(),
        })
        .collect()
}

impl Rewind {
    pub(super) fn start_camera(&mut self) {
        if !self.settings.filters.enabled || self.camera.is_some() {
            return;
        }
        thread::spawn(|| {
            rewind_nvbus::set_filters_enabled(true).ok();
        });
        let signals = self.signals.clone();
        self.camera = Some(Camera::start(self.paths.camera(), move || {
            signals.unbounded_send(Signal::Camera).ok();
        }));
    }

    pub fn set_filters_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.change(|settings| settings.filters.enabled = enabled, cx);
        if enabled {
            self.start_camera();
            self.toast(Icon::Filters, msg::ENABLED, None, false, cx);
        } else {
            self.camera = None;
            self.lens = Rc::default();
            self.styled = None;
            thread::spawn(|| {
                rewind_nvbus::set_filters_enabled(false).ok();
            });
        }
        if self.capturing.is_none() {
            self.rebind();
        }
    }

    pub fn lens_send(&self, command: Lens) {
        if let Some(camera) = &self.camera {
            camera.send(command);
        }
    }

    pub fn open_camera(&self, mode: Mode) {
        if self.lens.present && self.lens.mode != mode {
            self.lens_send(Lens::Open(mode));
        }
    }

    pub fn close_camera(&self) {
        if self.lens.mode != Mode::Off {
            self.lens_send(Lens::Close);
        }
    }

    pub fn active_style(&self) -> Option<usize> {
        self.settings
            .filters
            .game(&self.lens.path)
            .and_then(|game| game.active)
    }

    pub fn pick_style(&mut self, slot: Option<usize>, cx: &mut Context<Self>) {
        if self.lens.path.is_empty() || slot.is_some_and(|slot| slot >= STYLE_SLOTS) {
            return;
        }
        let path = self.lens.path.clone();
        self.settings
            .filters
            .update(&path, |game| game.active = slot);
        self.save();
        let style = self
            .settings
            .filters
            .game(&path)
            .and_then(|game| game.current())
            .map(stored)
            .unwrap_or_default();
        self.lens_send(Lens::Apply(style));
        cx.notify();
    }

    pub fn hotkey_style(&mut self, slot: Option<usize>, cx: &mut Context<Self>) {
        let usable =
            self.lens.present && self.lens.features.is_some_and(|features| features.filters);
        if !usable {
            self.toast(Icon::Filters, msg::NO_GAME, None, false, cx);
            return;
        }
        let slot = slot.filter(|slot| self.active_style() != Some(*slot));
        self.pick_style(slot, cx);
        self.toast(Icon::Filters, msg::style(slot), None, false, cx);
    }

    pub fn clear_filters(&mut self, cx: &mut Context<Self>) {
        self.lens_send(Lens::Clear);
        self.toast(Icon::Filters, msg::CLEARED, None, false, cx);
    }

    pub fn remove_filter(&mut self, index: usize, name: &str, cx: &mut Context<Self>) {
        self.lens_send(Lens::Remove(index));
        self.toast(Icon::Filters, msg::removed(name), None, false, cx);
    }

    pub fn add_filter(&mut self, filter: String, cx: &mut Context<Self>) {
        if self.active_style().is_none() && !self.lens.path.is_empty() {
            let path = self.lens.path.clone();
            self.settings
                .filters
                .update(&path, |game| game.active = Some(0));
        }
        self.lens_send(Lens::Add(filter));
        cx.notify();
    }

    fn greet_game(&mut self) {
        if !self.lens.present {
            self.styled = None;
            return;
        }
        let ready = self.lens.features.is_some_and(|features| features.filters);
        if !ready || self.lens.path.is_empty() {
            return;
        }
        let key = game_key(&self.lens.path);
        if self.styled.as_deref() == Some(key.as_str()) {
            return;
        }
        self.styled = Some(key);
        let style = self
            .settings
            .filters
            .game(&self.lens.path)
            .and_then(|game| game.current())
            .filter(|style| !style.is_empty())
            .map(stored);
        if let Some(style) = style {
            self.lens_send(Lens::Apply(style));
        }
    }

    fn remember_style(&mut self, before: &State, cx: &mut Context<Self>) {
        let lens = &self.lens;
        let editing = lens.mode != Mode::Off && before.mode != Mode::Off;
        let settled = !lens.restoring && lens.layers.iter().all(|layer| layer.loaded);
        if !editing || !settled || lens.layers == before.layers || lens.path.is_empty() {
            return;
        }
        let style = saved(&lens.style());
        let path = lens.path.clone();
        let known = self.settings.filters.game(&path);
        let slot = known.and_then(|game| game.active);
        if slot.is_none() && style.is_empty() {
            return;
        }
        let slot = slot.unwrap_or(0);
        if known.is_some_and(|game| game.active == Some(slot) && game.styles[slot] == style) {
            return;
        }
        self.settings.filters.update(&path, |game| {
            game.active = Some(slot);
            game.styles[slot] = style;
        });
        self.style_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STYLE_SAVE_DELAY).await;
            this.update(cx, |rewind, _| rewind.save()).ok();
        }));
    }

    fn report_shot(&mut self, before: &State, cx: &mut Context<Self>) {
        if self.lens.shooting == before.shooting {
            return;
        }
        match self.lens.shooting.clone() {
            Shooting::Saved(path) => {
                self.toast(Icon::PhotoMode, msg::SHOT_SAVED, Some(path), false, cx);
                self.request_gallery_scan(None, Duration::ZERO, cx);
            }
            Shooting::Failed(code) => {
                self.toast(
                    Icon::PhotoMode,
                    msg::SHOT_FAILED,
                    Some(msg::code(code)),
                    true,
                    cx,
                );
            }
            Shooting::Idle | Shooting::Running { .. } => {}
        }
    }

    pub(super) fn camera_changed(&mut self, cx: &mut Context<Self>) {
        let Some(camera) = &self.camera else {
            return;
        };
        let before = std::mem::replace(&mut self.lens, Rc::new(camera.state()));
        self.greet_game();
        self.remember_style(&before, cx);
        self.report_shot(&before, cx);
        if self.lens.error != before.error
            && let Some(reason) = self.lens.error.clone()
        {
            self.toast(Icon::Filters, msg::CAMERA_FAILED, Some(reason), true, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_keeps_its_filters_and_values_through_the_settings_file() {
        let style = vec![
            StyleLayer {
                filter: "Sharpen.fx".to_owned(),
                values: vec![(1, vec![0.25]), (4, vec![1.0, 0.5, 0.0])],
            },
            StyleLayer {
                filter: "NvVignette.fx".to_owned(),
                values: Vec::new(),
            },
        ];
        assert_eq!(stored(&saved(&style)), style);
    }
}
