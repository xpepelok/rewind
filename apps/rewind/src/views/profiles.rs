use std::time::Instant;

use gpui::{AnyElement, Context, div, prelude::*, px};

use rewind_config::{BITRATE_MAX_MBPS, BITRATE_MIN_MBPS, Quality, REPLAY_STEPS, replay_step};

use crate::theme::{GUTTER, ThemeExt};
use crate::ui::{
    ButtonKind, Selector, Slider, Toggle, button, divider, hint, label, section, setting_row,
};
use crate::views::overlay::{Overlay, PROBE_REFRESH, Page};
use crate::views::pages::{change, clock, scroll, set, slider_row, step, title_block};

impl Overlay {
    pub(super) fn profiles_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let rewind = self.state.read(cx);
        let profiles = rewind.settings.profiles.clone();
        let active = rewind.active_profile_name();
        let candidate = self
            .profile_candidate
            .get(
                profiles.games.len(),
                Some(PROBE_REFRESH),
                Instant::now(),
                |_| rewind.profile_candidate(),
            )
            .clone();
        let selector_row = |title: &str, selector: Selector| {
            setting_row(&p)
                .child(label(title.to_owned(), p.text).flex_1())
                .child(selector)
        };
        let mut page = scroll("profiles-scroll").track_scroll(self.page_scroll(Page::Profiles))
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Свои настройки для каждой игры",
                        Some(
                            "Когда запущена игра из списка, включаются её длина повтора, качество и частота кадров, а после выхода из игры возвращаются обычные. Переключение мгновенное: Windows сама сообщает, какое окно стало активным и когда игра закрылась. В игры ничего не встраивается. При переключении профиля буфер повтора начинается заново."
                                .to_owned(),
                        ),
                        &p,
                    ))
                    .child(Toggle::new("profiles-enabled", profiles.enabled).on_click(change(
                        &state,
                        |settings| settings.profiles.enabled = !settings.profiles.enabled,
                    ))),
            )
            .when_some(active, |page, name| {
                page.child(
                    div()
                        .px(px(GUTTER))
                        .pb(px(8.0))
                        .child(hint(format!("Сейчас действует профиль «{name}»."), &p)),
                )
            })
            .child(divider(&p));
        page = match candidate {
            Some((name, executable)) => page.child(
                div().px(px(GUTTER)).py(px(8.0)).child(
                    button(
                        "profile-add",
                        format!("Добавить профиль для «{name}»"),
                        ButtonKind::Primary,
                        true,
                        &p,
                    )
                    .on_click(cx.listener(move |overlay, _, _, cx| {
                        let (name, executable) = (name.clone(), executable.clone());
                        overlay.state.update(cx, |rewind, cx| {
                            rewind.add_profile(name, executable, cx)
                        });
                    })),
                ),
            ),
            None => page.child(div().px(px(GUTTER)).py(px(8.0)).child(hint(
                "Чтобы добавить игру, запустите её и откройте оверлей сочетанием клавиш прямо в игре.",
                &p,
            ))),
        };
        for (index, profile) in profiles.games.iter().enumerate() {
            let video = profile.video.clone();
            page = page
                .child(section(
                    format!("{} · {}", profile.name, profile.executable),
                    &p,
                ))
                .child(slider_row(
                    "Длительность мгновенного повтора",
                    Slider::new(
                        ("profile-replay", index),
                        replay_step(video.replay_seconds) as u32,
                        0,
                        (REPLAY_STEPS.len() - 1) as u32,
                        1,
                    )
                    .on_input(set(&state, move |settings, value| {
                        if let Some(game) = settings.profiles.games.get_mut(index) {
                            game.video.replay_seconds =
                                REPLAY_STEPS[(value as usize).min(REPLAY_STEPS.len() - 1)];
                        }
                    })),
                    clock(video.replay_seconds),
                    &p,
                ))
                .child(selector_row(
                    "Качество",
                    Selector::new(("profile-quality", index), video.quality.label()).on_step(step(
                        &state,
                        move |settings, forward| {
                            if let Some(game) = settings.profiles.games.get_mut(index) {
                                game.video.quality = game.video.quality.step(forward);
                            }
                        },
                    )),
                ))
                .child(selector_row(
                    "Разрешение",
                    Selector::new(("profile-resolution", index), video.resolution.label()).on_step(
                        step(&state, move |settings, forward| {
                            if let Some(game) = settings.profiles.games.get_mut(index) {
                                game.video.resolution = game.video.resolution.step(forward);
                            }
                        }),
                    ),
                ))
                .child(selector_row(
                    "Частота кадров",
                    Selector::new(("profile-fps", index), video.frame_rate.label()).on_step(step(
                        &state,
                        move |settings, forward| {
                            if let Some(game) = settings.profiles.games.get_mut(index) {
                                game.video.frame_rate = game.video.frame_rate.step(forward);
                            }
                        },
                    )),
                ))
                .child(selector_row(
                    "Кодек",
                    Selector::new(("profile-codec", index), video.codec.label()).on_step(step(
                        &state,
                        move |settings, forward| {
                            if let Some(game) = settings.profiles.games.get_mut(index) {
                                game.video.codec = game.video.codec.step(forward);
                            }
                        },
                    )),
                ))
                .child(slider_row(
                    "Скорость передачи (Мбит/с)",
                    Slider::new(
                        ("profile-bitrate", index),
                        video.bitrate_mbps(),
                        BITRATE_MIN_MBPS,
                        BITRATE_MAX_MBPS,
                        1,
                    )
                    .on_input(set(&state, move |settings, value| {
                        if let Some(game) = settings.profiles.games.get_mut(index) {
                            game.video.quality = Quality::Custom;
                            game.video.custom_bitrate_mbps = value;
                        }
                    })),
                    video.bitrate_mbps().to_string(),
                    &p,
                ))
                .child(
                    div().flex().justify_end().px(px(GUTTER)).py(px(8.0)).child(
                        button(
                            ("profile-remove", index),
                            "Удалить профиль",
                            ButtonKind::Danger,
                            true,
                            &p,
                        )
                        .on_click(cx.listener(move |overlay, _, _, cx| {
                            overlay.state.update(cx, |rewind, cx| {
                                rewind.change(
                                    |settings| {
                                        if index < settings.profiles.games.len() {
                                            settings.profiles.games.remove(index);
                                        }
                                    },
                                    cx,
                                )
                            });
                        })),
                    ),
                );
        }
        page.into_any_element()
    }
}
