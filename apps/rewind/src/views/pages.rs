use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, App, ClipboardItem, Context, Div, Entity, SharedString, Window, div, prelude::*, px,
};

use rewind_config::{
    BITRATE_MAX_MBPS, BITRATE_MIN_MBPS, COPY_SECONDS_MAX, COPY_SECONDS_MIN, Codec, Command, Corner,
    DISK_LIMIT_MAX_GB, DISK_LIMIT_MIN_GB, MARK_SPAN_MAX_SECONDS, MARK_SPAN_MIN_SECONDS, Quality,
    REPLAY_MAX_SECONDS, REPLAY_MIN_SECONDS, REPLAY_STEPS, Settings, Theme, VOLUME_MAX, replay_step,
};

use crate::assets::{Icon, icon};
use crate::gallery::{GalleryItem, format_duration};
use crate::motion::Key;
use crate::platform;
use crate::share;
use crate::state::Rewind;
use crate::theme::{GUTTER, ICON, Palette, SMALL_TEXT, TEXT_INSET, ThemeExt};
use crate::ui::scrollbar::with_scrollbar;
use crate::ui::{
    Selector, Slider, Toggle, Tooltip, checkbox, divider, field, hint, keybox, label, nav_row,
    row_icon, section, setting_row, thumbnail, value_box,
};
use crate::views::library::PagePicker;
use crate::views::overlay::{Overlay, PROBE_REFRESH, Page, ScreenUse};
use crate::views::published::Published;

const SAVING_REPLAY: &str = "Сохраняю повтор…";
const THUMB_SIZE: f32 = 86.0;
const THUMB_GAP: f32 = 8.0;
pub const THUMBS_VISIBLE: usize = 4;
const VALUE_WIDTH: f32 = 48.0;
const REPLAY_VALUE_WIDTH: f32 = 96.0;
const NO_HOTKEY: &str = "Нет";
const PRESS_KEYS: &str = "Нажмите клавиши";
const DEFAULT_MICROPHONE: &str = "По умолчанию";
const UPLOADS_SHOWN: usize = 100;

pub(super) fn hotkey_text(rewind: &Rewind, command: Command) -> String {
    rewind
        .settings
        .bindings
        .get(command)
        .map_or_else(|| NO_HOTKEY.to_owned(), |hotkey| hotkey.label())
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub fn parse_span(text: &str) -> Option<u32> {
    let text = text.trim().to_lowercase().replace(',', ".");
    if text.is_empty() {
        return None;
    }
    let seconds = if text.contains(':') {
        let parts = text
            .split(':')
            .map(|part| part.trim().parse::<u32>().ok())
            .collect::<Option<Vec<_>>>()?;
        match parts.as_slice() {
            [minutes, seconds] => minutes.checked_mul(60)?.checked_add(*seconds)?,
            [hours, minutes, seconds] => hours
                .checked_mul(3600)?
                .checked_add(minutes.checked_mul(60)?)?
                .checked_add(*seconds)?,
            _ => return None,
        }
    } else {
        let mut total = 0.0;
        let mut rest = text.as_str();
        while !rest.trim_start().is_empty() {
            rest = rest.trim_start();
            let number_end = rest
                .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
                .unwrap_or(rest.len());
            let number: f64 = rest[..number_end].parse().ok()?;
            rest = rest[number_end..].trim_start();
            let unit_end = rest
                .find(|ch: char| !ch.is_alphabetic())
                .unwrap_or(rest.len());
            let scale = match &rest[..unit_end] {
                "" | "м" | "мин" | "m" | "min" => 60.0,
                "ч" | "час" | "часа" | "часов" | "h" | "hr" => 3600.0,
                "с" | "сек" | "s" | "sec" => 1.0,
                _ => return None,
            };
            rest = &rest[unit_end..];
            total += number * scale;
        }
        if !total.is_finite() || total > f64::from(u32::MAX) {
            return None;
        }
        total.round() as u32
    };
    Some(seconds.clamp(REPLAY_MIN_SECONDS, REPLAY_MAX_SECONDS))
}

pub fn clock(seconds: u32) -> String {
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

pub fn span_text(seconds: u32) -> String {
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds} с"),
        (0, minutes) if seconds.is_multiple_of(60) => format!("{minutes} мин"),
        (0, _) => format!("{} мин {} с", seconds / 60, seconds % 60),
        (hours, 0) => format!("{hours} ч"),
        (hours, minutes) => format!("{hours} ч {minutes} мин"),
    }
}

fn replay_disk_hint(
    video: &rewind_config::Video,
    free: Option<u64>,
    memory: Option<u64>,
) -> String {
    let bitrate = video.bitrate_mbps() * 1_000_000;
    let replay = std::time::Duration::from_secs(u64::from(video.replay_seconds));
    let needed_bytes = rewind_capture::replay_bytes(bitrate, replay);
    let needed = needed_bytes as f64 / GIB;
    let base = format!(
        "Можно записать до 24 ч. Повтор хранится на диске в папке временных файлов: для {} при текущем качестве нужно до {needed:.1} ГБ.",
        span_text(video.replay_seconds)
    );
    let Some(free_bytes) = free else {
        return base;
    };
    let free = free_bytes as f64 / GIB;
    let plan = rewind_capture::ring_plan(needed_bytes, Some(free_bytes), memory);
    if plan.on_disk && plan.bytes >= needed_bytes {
        return format!("{base} Свободно {free:.0} ГБ.");
    }
    let per_hour = rewind_capture::replay_bytes(bitrate, std::time::Duration::from_secs(3600));
    let fits = ((plan.bytes as f64 / per_hour as f64 * 3600.0) as u32).min(video.replay_seconds);
    if plan.on_disk {
        return format!(
            "{base} Свободно только {free:.0} ГБ, поэтому повтор будет примерно {}: Rewind оставляет 10 ГБ свободными.",
            span_text(fits)
        );
    }
    format!(
        "{base} Свободно только {free:.0} ГБ, а Rewind оставляет 10 ГБ свободными, поэтому повтор хранится в памяти: примерно {}.",
        span_text(fits)
    )
}

pub(super) fn change(
    state: &Entity<Rewind>,
    apply: impl Fn(&mut Settings) + 'static,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, cx| state.update(cx, |rewind, cx| rewind.change(&apply, cx))
}

impl Overlay {
    fn step_share_host(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |rewind, cx| {
            rewind.change(
                |settings| settings.sharing.host = settings.sharing.host.step(forward),
                cx,
            )
        });
        cx.notify();
    }

    pub(super) fn sharing_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let host = self.state.read(cx).settings.sharing.host;
        let overlay = cx.entity();
        let selector = Selector::new("share-host", host.label()).on_step(move |forward, _, cx| {
            overlay.update(cx, |overlay, cx| overlay.step_share_host(forward, cx))
        });
        let shots = self.state.read(cx).settings.screenshots.clone();
        let shot_hotkey = hotkey_text(self.state.read(cx), Command::Screenshot);
        let state = self.state.clone();
        scroll("sharing-scroll")
            .track_scroll(&self.sharing_scroll)
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Произвольный захват",
                        Some(format!(
                            "По {shot_hotkey} экран замирает, и можно выбрать область, окно или весь текущий экран. Выделение двигается и тянется за края. Потом снимок можно скопировать, сохранить, загрузить в {} или открыть в редакторе: карандаш, стрелки, текст, кисть размытия. Enter или Ctrl+C — скопировать, Esc — отмена.",
                            host.label()
                        )),
                        &p,
                    ))
                    .child(Toggle::new("custom-capture", shots.custom).on_click(change(
                        &state,
                        |settings| settings.screenshots.custom = !settings.screenshots.custom,
                    ))),
            )
            .when(shots.custom, |page| {
                page.child(
                    setting_row(&p)
                        .child(title_block(
                            "Копировать сразу после выделения",
                            Some(
                                "Как «Ножницы» в Windows: отпустили кнопку мыши — снимок уже в буфере обмена. В галерею он попадает, только если включить настройку ниже."
                                    .to_owned(),
                            ),
                            &p,
                        ))
                        .child(Toggle::new("copy-on-select", shots.copy_on_select).on_click(
                            change(&state, |settings| {
                                settings.screenshots.copy_on_select =
                                    !settings.screenshots.copy_on_select
                            }),
                        )),
                )
                .child(
                    setting_row(&p)
                        .child(title_block(
                            "Сохранять в галерею после выделения",
                            Some(
                                "Снимок остаётся в буфере обмена и вдобавок сохраняется в галерею. Работает вместе с копированием после выделения."
                                    .to_owned(),
                            ),
                            &p,
                        ))
                        .child(
                            Toggle::new("save-on-select", shots.saves_selection())
                                .disabled(!shots.copy_on_select)
                                .on_click(change(&state, |settings| {
                                    settings.screenshots.save_on_select =
                                        !settings.screenshots.save_on_select
                                })),
                        ),
                )
            })
            .child(divider(&p))
            .child(section("Куда заливать скриншоты", &p))
            .child(
                setting_row(&p)
                    .child(label("Сервис", p.text).flex_1())
                    .child(selector),
            )
            .children(
                share::retention(host).map(|note| div().px(px(GUTTER)).child(hint(note, &p))),
            )
            .child(divider(&p))
            .child(section("Как это работает", &p))
            .child(div().px(px(GUTTER)).child(hint(
                format!(
                    "Ключи и аккаунты не нужны: картинка заливается анонимно. Кнопка «Опубликовать» в просмотре снимка или в меню галереи заливает её, копирует ссылку в буфер обмена и показывает уведомление. {} делает снимок и сразу публикует его. Ошибки пишутся в журнал logs\\uploads.log.",
                    hotkey_text(self.state.read(cx), Command::PublishScreenshot)
                ),
                &p,
            )))
            .child(divider(&p))
            .child(self.uploads_section(cx))
            .into_any_element()
    }

    fn uploads_section(&mut self, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let uploads = &rewind.uploads.entries;
        let key = (
            uploads.len(),
            uploads.first().map(|upload| upload.link.clone()),
            rewind.previewing_uploads(),
        );
        let entries = self
            .upload_rows
            .get(key, Some(PROBE_REFRESH), Instant::now(), |_| {
                Rc::new(
                    uploads
                        .iter()
                        .take(UPLOADS_SHOWN)
                        .map(|upload| {
                            let size = std::fs::metadata(&upload.file).ok().map(|meta| meta.len());
                            (upload.clone(), rewind.upload_image(upload), size)
                        })
                        .collect(),
                )
            })
            .clone();
        let mut column = div()
            .flex()
            .flex_col()
            .child(section("История публикаций", &p));
        if entries.is_empty() {
            return column.child(
                div()
                    .px(px(GUTTER))
                    .child(hint("Пока ничего не опубликовано.", &p)),
            );
        }
        let action = |id: (&'static str, usize), glyph: Icon, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .p(px(4.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .tooltip(Tooltip::text(tip))
                .child(icon(glyph, 20.0, p.icon))
        };
        for (index, (entry, preview, size)) in entries.iter().cloned().enumerate() {
            let (open, copy, forget) = (entry.link.clone(), entry.link.clone(), entry.link.clone());
            let view = entry.link.clone();
            let delete = entry.delete.clone();
            column = column.child(
                div()
                    .id(("upload", index))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(GUTTER))
                    .py(px(8.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(thumbnail(preview, Icon::Photo, &p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(label(entry.name(), p.text))
                            .child(hint(
                                match size {
                                    Some(size) => format!(
                                        "{} · {} · {} · {}",
                                        entry.host.label(),
                                        youtube::iso_date(entry.uploaded_at),
                                        crate::gallery::format_size(size),
                                        entry.link
                                    ),
                                    None => format!(
                                        "{} · {} · {}",
                                        entry.host.label(),
                                        youtube::iso_date(entry.uploaded_at),
                                        entry.link
                                    ),
                                },
                                &p,
                            )),
                    )
                    .child(
                        action(("upload-copy", index), Icon::Copy, "Копировать ссылку").on_click(
                            cx.listener(move |overlay, _, _, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                let detail = copy.clone();
                                overlay.state.update(cx, |rewind, cx| {
                                    rewind.toast(
                                        Icon::Copy,
                                        "Ссылка скопирована",
                                        Some(detail),
                                        false,
                                        cx,
                                    )
                                });
                            }),
                        ),
                    )
                    .when_some(delete, |row, delete| {
                        row.child(
                            action(("upload-delete", index), Icon::Delete, "Удалить с хостинга")
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    platform::shell::open_url(&delete);
                                }),
                        )
                    })
                    .child(
                        action(("upload-forget", index), Icon::Close, "Убрать из истории")
                            .on_click(cx.listener(move |overlay, _, _, cx| {
                                cx.stop_propagation();
                                overlay
                                    .state
                                    .update(cx, |rewind, cx| rewind.forget_upload(&forget, cx));
                            })),
                    )
                    .child(
                        action(("upload-open", index), Icon::Open, "Открыть в браузере").on_click(
                            move |_, _, cx| {
                                cx.stop_propagation();
                                platform::shell::open_url(&open);
                            },
                        ),
                    )
                    .on_click(cx.listener(move |overlay, _, window, cx| {
                        overlay.open_published(Published::Shot(view.clone()), window, cx)
                    })),
            );
        }
        column
    }

    pub(super) fn specs_section(&self, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let state = self.state.clone();
        let specs = self.state.read(cx).settings.youtube.specs.clone();
        let enabled = specs.enabled;
        let check = |id: &'static str, title: &str, checked: bool, apply: fn(&mut Settings)| {
            let row = div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(18.0))
                .px(px(GUTTER + 14.0))
                .py(px(9.0))
                .opacity(if enabled { 1.0 } else { 0.4 })
                .child(checkbox(checked, &p))
                .child(label(title.to_owned(), p.text));
            if enabled {
                let handler = change(&state, apply);
                row.cursor_pointer()
                    .on_click(move |_, window, cx| handler(window, cx))
            } else {
                row
            }
        };
        div()
            .flex()
            .flex_col()
            .child(section("Описание видео", &p))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Добавлять характеристики ПК",
                        Some("Видеокарта, процессор и другое в конце описания".to_owned()),
                        &p,
                    ))
                    .child(
                        Toggle::new("specs-toggle", enabled).on_click(change(&state, |settings| {
                            settings.youtube.specs.enabled ^= true
                        })),
                    ),
            )
            .child(check(
                "specs-gpu",
                "Видеокарта",
                specs.gpu,
                |settings| settings.youtube.specs.gpu ^= true,
            ))
            .child(check(
                "specs-cpu",
                "Процессор",
                specs.cpu,
                |settings| settings.youtube.specs.cpu ^= true,
            ))
            .child(check(
                "specs-memory",
                "Оперативная память",
                specs.memory,
                |settings| settings.youtube.specs.memory ^= true,
            ))
            .child(check(
                "specs-display",
                "Монитор",
                specs.display,
                |settings| settings.youtube.specs.display ^= true,
            ))
            .child(check("specs-system", "Windows", specs.system, |settings| {
                settings.youtube.specs.system ^= true
            }))
    }
}

pub(super) fn step(
    state: &Entity<Rewind>,
    apply: impl Fn(&mut Settings, bool) + 'static,
) -> impl Fn(bool, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |forward, _, cx| {
        state.update(cx, |rewind, cx| {
            rewind.change(|settings| apply(settings, forward), cx)
        })
    }
}

pub(super) fn set(
    state: &Entity<Rewind>,
    apply: impl Fn(&mut Settings, u32) + 'static,
) -> impl Fn(u32, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |value, _, cx| {
        state.update(cx, |rewind, cx| {
            rewind.change(|settings| apply(settings, value), cx)
        })
    }
}

fn percent(value: u8) -> u32 {
    u32::from(value)
}

fn as_u8(value: u32) -> u8 {
    value.min(u32::from(u8::MAX)) as u8
}

fn mark_span_row(
    title: &'static str,
    id: &'static str,
    value: u32,
    min: u32,
    max: u32,
    on_input: impl Fn(u32, &mut Window, &mut App) + 'static,
    p: &Palette,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(16.0))
        .px(px(GUTTER))
        .py(px(6.0))
        .when(p.soft, |row| {
            row.mx(px(THEME_ROW_MARGIN))
                .my(px(THEME_ROW_GAP))
                .px(px(THEME_ROW_INSET))
                .py(px(THEME_ROW_INSET))
                .rounded(px(p.radius()))
        })
        .when_some(p.card, |row, card| row.bg(card))
        .when_some(p.row_line, |row, line| row.border_1().border_color(line))
        .child(label(title, p.text).w(px(110.0)))
        .child(
            div()
                .flex_1()
                .pl(px(8.0))
                .child(Slider::new(id, value, min, max, 5).on_input(on_input)),
        )
        .child(value_box(span_text(value), p, REPLAY_VALUE_WIDTH))
}

pub(super) fn title_block(
    title: impl Into<SharedString>,
    subtitle: Option<String>,
    p: &Palette,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(label(title, p.text))
        .when_some(subtitle, |block, subtitle| block.child(hint(subtitle, p)))
}

pub(super) fn scroll(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .pb(px(12.0))
}

pub(super) fn slider_row(title: &str, slider: Slider, value: String, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .px(px(GUTTER))
        .py(px(8.0))
        .when(p.soft, |row| {
            row.mx(px(THEME_ROW_MARGIN))
                .my(px(THEME_ROW_GAP))
                .px(px(THEME_ROW_INSET))
                .py(px(THEME_ROW_INSET))
                .rounded(px(p.radius()))
        })
        .when_some(p.card, |row, card| row.bg(card))
        .when_some(p.row_line, |row, line| row.border_1().border_color(line))
        .child(label(title.to_owned(), p.text))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.0))
                .pt(px(10.0))
                .pl(px(8.0))
                .child(slider)
                .child(value_box(value, p, VALUE_WIDTH)),
        )
}

const DISABLED_ROW: f32 = 0.4;
const PAGE_BARS: [&str; Page::COUNT] = [
    "home-bar",
    "replay-bar",
    "settings-bar",
    "hotkeys-bar",
    "interface-bar",
    "notifications-bar",
    "captured-bar",
    "audio-bar",
    "video-bar",
    "profiles-bar",
    "files-bar",
    "gallery-bar",
    "youtube-bar",
    "sharing-bar",
    "replace-bar",
    "published-bar",
    "screens-bar",
    "screens-replay-bar",
    "photo-bar",
    "highlights-bar",
    "filters-bar",
    "summary-bar",
    "update-bar",
    "appearance-bar",
    "misc-bar",
];
const VERBOSE_NOTE: &str = "Записывает в журнал каждый шаг, на котором запись, повтор, сохранение или загрузка могут сорваться. Включайте, когда что-то работает неправильно, и приложите файл к сообщению об ошибке.";
const APPEARANCE_NOTE: &str = "Тема меняет цвета и форму кнопок, переключателей и ползунков. Расположение элементов остаётся прежним.";
const SWATCH_WIDTH: f32 = 72.0;
const SWATCH_HEIGHT: f32 = 28.0;
const THEME_ROW_INSET: f32 = 12.0;
const THEME_ROW_MARGIN: f32 = 8.0;
const THEME_ROW_GAP: f32 = 4.0;
const SCREEN_IDS: [&str; 8] = [
    "screen-1", "screen-2", "screen-3", "screen-4", "screen-5", "screen-6", "screen-7", "screen-8",
];

impl Overlay {
    pub fn page_body(
        &mut self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = match page {
            Page::Home => self.home(window, cx),
            Page::Replay => self.replay(cx),
            Page::Settings => self.settings(cx),
            Page::Hotkeys => self.hotkeys(cx),
            Page::Interface => self.interface(cx),
            Page::Notifications => self.notifications(cx),
            Page::Captured => self.captured(cx),
            Page::Audio => self.audio(cx),
            Page::Video => self.video(cx),
            Page::Profiles => self.profiles_page(cx),
            Page::Files => self.files(window, cx),
            Page::Gallery => self.gallery_page(cx),
            Page::YouTube => {
                let page = self.youtube_page(cx);
                with_scrollbar(page, "youtube-scrollbar", &self.youtube_scroll).into_any_element()
            }
            Page::Sharing => {
                let page = self.sharing_page(cx);
                with_scrollbar(page, "sharing-scrollbar", &self.sharing_scroll).into_any_element()
            }
            Page::Replace => self.replace_page(cx),
            Page::Update => self.update_page(cx),
            Page::Published => self.published_page(cx),
            Page::ScreensRecord => self.screens_page(ScreenUse::Record, cx),
            Page::ScreensReplay => self.screens_page(ScreenUse::Replay, cx),
            Page::PhotoMode => self.photo_page(cx),
            Page::Highlights => self.highlights_page(cx),
            Page::Filters => self.filters_page(cx),
            Page::Summary => self.summary_page(cx),
            Page::Appearance => self.appearance(cx),
            Page::Misc => self.misc(cx),
        };
        match page.bar() {
            Some(owner) => self.barred(owner, body),
            None => body,
        }
    }

    pub(super) fn page_scroll(&self, page: Page) -> &gpui::ScrollHandle {
        &self.page_scrolls[page as usize]
    }

    fn barred(&self, page: Page, body: AnyElement) -> AnyElement {
        with_scrollbar(body, PAGE_BARS[page as usize], self.page_scroll(page)).into_any_element()
    }

    fn save_replay_click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).screen_choice() {
            self.push(Page::ScreensReplay, window, cx);
        } else {
            self.state.update(cx, |rewind, cx| rewind.save_replay(cx));
        }
    }

    fn screens_page(&mut self, purpose: ScreenUse, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let screens = self.state.read(cx).screens.clone();
        let note = match purpose {
            ScreenUse::Record => "Запись начнётся на выбранном экране.",
            ScreenUse::Replay => "Сохранится повтор с выбранного экрана.",
        };
        let mut list = scroll("screens-scroll")
            .track_scroll(self.page_scroll(Page::ScreensRecord))
            .pt(px(12.0))
            .child(
                div()
                    .px(px(GUTTER))
                    .pb(px(8.0))
                    .child(hint(note.to_owned(), &p)),
            );
        for (index, (screen, id)) in screens.into_iter().zip(SCREEN_IDS).enumerate() {
            let size = format!("{}×{}", screen.width, screen.height);
            let subtitle = if screen.is_primary() {
                format!("{size} · основной")
            } else {
                size
            };
            let row = self
                .link(
                    id,
                    Icon::Screenshot,
                    &format!("Экран {}", index + 1),
                    Some(subtitle),
                    Icon::ChevronRight,
                    cx,
                )
                .on_click(cx.listener(move |overlay, _, window, cx| {
                    overlay.pop(window, cx);
                    overlay.state.update(cx, |rewind, cx| match purpose {
                        ScreenUse::Record => rewind.toggle_recording_on(Some(screen), cx),
                        ScreenUse::Replay => rewind.save_replay_on(Some(screen), cx),
                    });
                }));
            list = list.child(row);
        }
        list.into_any_element()
    }

    pub(super) fn link(
        &self,
        id: &'static str,
        glyph: Icon,
        title: &str,
        subtitle: Option<String>,
        trailing: Icon,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = cx.palette();
        nav_row(id, Key::new("row", id), &p, cx)
            .child(row_icon(glyph, &p))
            .child(title_block(title.to_owned(), subtitle, &p))
            .child(icon(trailing, ICON, p.text))
    }

    fn gallery_strip(&self, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let items = &self.state.read(cx).gallery;
        let total = items.len();
        if total == 0 {
            return div()
                .h(px(THUMB_SIZE))
                .mx(px(GUTTER))
                .mb(px(4.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .bg(p.header)
                .child(icon(Icon::Gallery, 24.0, p.muted))
                .child(hint(
                    "Здесь появятся последние клипы и снимки".to_owned(),
                    &p,
                ));
        }
        let offset = self
            .gallery_offset
            .min(total.saturating_sub(THUMBS_VISIBLE));
        let visible: Vec<GalleryItem> = items
            .iter()
            .skip(offset)
            .take(THUMBS_VISIBLE)
            .cloned()
            .collect();
        let can_back = offset > 0;
        let can_forward = offset + THUMBS_VISIBLE < total;
        let thumbs = visible
            .iter()
            .enumerate()
            .map(|(index, item)| {
                self.clip_card(
                    (crate::views::library::HOME_STRIP, offset + index),
                    item,
                    (THUMB_SIZE, THUMB_SIZE),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let arrow = |id: &'static str, glyph: Icon, enabled: bool, delta: isize| {
            div()
                .id(id)
                .flex_none()
                .w(px(32.0))
                .flex()
                .justify_center()
                .child(icon(glyph, ICON, if enabled { p.text } else { p.disabled }))
                .when(enabled, |arrow| {
                    arrow
                        .cursor_pointer()
                        .on_click(cx.listener(move |overlay, _, _, cx| {
                            overlay.gallery_offset =
                                overlay.gallery_offset.saturating_add_signed(delta);
                            cx.notify();
                        }))
                })
        };
        div()
            .flex()
            .items_center()
            .px(px(GUTTER))
            .pb(px(4.0))
            .child(arrow("gallery-back", Icon::ChevronLeft, can_back, -1))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .gap(px(THUMB_GAP))
                    .h(px(THUMB_SIZE))
                    .children(thumbs),
            )
            .child(arrow("gallery-forward", Icon::ChevronRight, can_forward, 1))
    }

    fn home(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let recording = rewind.recording;
        let mic_mode = rewind.settings.audio.mic_mode;
        let record_hotkey = hotkey_text(rewind, Command::ToggleRecording);
        let replay_hotkey = hotkey_text(rewind, Command::ToggleReplay);
        let save_hotkey = hotkey_text(rewind, Command::SaveReplay);
        let replay_enabled = rewind.replay_enabled();
        let replay_saving = rewind.replay_saving();
        let screenshot_hotkey = hotkey_text(rewind, Command::Screenshot);
        let photo_subtitle = crate::views::filters::photo_subtitle(rewind);
        let highlights_subtitle = crate::views::filters::highlights_subtitle(rewind).to_owned();
        let filters_subtitle = crate::views::filters::filters_subtitle(rewind);
        let mic_hotkey = hotkey_text(rewind, Command::ToggleMic);
        let stats_hotkey = hotkey_text(rewind, Command::ToggleStats);
        let state = self.state.clone();

        let record_subtitle = match recording {
            Some(elapsed) => format!(
                "{record_hotkey} - Остановить · {}",
                format_duration(elapsed)
            ),
            None => format!("{record_hotkey} - Запуск"),
        };
        let gallery = self
            .link(
                "gallery",
                Icon::Gallery,
                "Галерея",
                None,
                Icon::ChevronRight,
                cx,
            )
            .on_click(
                cx.listener(|overlay, _, window, cx| overlay.push(Page::Gallery, window, cx)),
            );
        let record = self
            .link(
                "record",
                Icon::Record,
                "Запись",
                Some(record_subtitle),
                if recording.is_some() {
                    Icon::Stop
                } else {
                    Icon::Play
                },
                cx,
            )
            .on_click(cx.listener(|overlay, _, window, cx| {
                let rewind = overlay.state.read(cx);
                if rewind.recording.is_none() && rewind.screen_choice() {
                    overlay.push(Page::ScreensRecord, window, cx);
                } else {
                    overlay
                        .state
                        .update(cx, |rewind, cx| rewind.toggle_recording(cx));
                }
            }));
        let replay_subtitle = if replay_saving {
            SAVING_REPLAY.to_owned()
        } else if replay_enabled {
            format!("{save_hotkey} — сохранить повтор")
        } else {
            format!("{replay_hotkey} — включение/выключение")
        };
        let save_button = replay_enabled.then(|| {
            div()
                .id("replay-save-now")
                .flex_none()
                .p(px(6.0))
                .mr(px(4.0))
                .when(!replay_saving, |button| {
                    button.cursor_pointer().tooltip(Tooltip::text("Сохранить"))
                })
                .child(icon(
                    Icon::Save,
                    ICON,
                    if replay_saving { p.disabled } else { p.text },
                ))
                .on_click(cx.listener(move |overlay, _, window, cx| {
                    cx.stop_propagation();
                    if !replay_saving {
                        overlay.save_replay_click(window, cx);
                    }
                }))
        });
        let replay = nav_row("replay", Key::new("row", "replay"), &p, cx)
            .child(row_icon(Icon::Replay, &p))
            .child(title_block("Мгновенный повтор", Some(replay_subtitle), &p))
            .children(save_button)
            .child(icon(Icon::ChevronRight, ICON, p.text))
            .on_click(cx.listener(|overlay, _, window, cx| overlay.push(Page::Replay, window, cx)));
        let screenshot = self
            .link(
                "screenshot",
                Icon::Screenshot,
                "Снимок экрана",
                Some(format!("{screenshot_hotkey} — сохранить снимок экрана")),
                Icon::Photo,
                cx,
            )
            .on_click(cx.listener(|overlay, _, _, cx| {
                overlay.state.update(cx, |rewind, _| rewind.screenshot())
            }));
        let photo_mode = self
            .link(
                "photo-mode",
                Icon::PhotoMode,
                "Фоторежим",
                Some(photo_subtitle),
                Icon::ChevronRight,
                cx,
            )
            .on_click(
                cx.listener(|overlay, _, window, cx| overlay.push(Page::PhotoMode, window, cx)),
            );
        let highlights = self
            .link(
                "highlights",
                Icon::Highlights,
                "Лучшие моменты",
                Some(highlights_subtitle),
                Icon::ChevronRight,
                cx,
            )
            .on_click(
                cx.listener(|overlay, _, window, cx| overlay.push(Page::Highlights, window, cx)),
            );
        let filters = self
            .link(
                "filters",
                Icon::Filters,
                "Игровые фильтры",
                Some(filters_subtitle),
                Icon::ChevronRight,
                cx,
            )
            .on_click(
                cx.listener(|overlay, _, window, cx| overlay.push(Page::Filters, window, cx)),
            );
        let microphone =
            div()
                .flex()
                .items_center()
                .gap(px(if p.soft {
                    THEME_ROW_INSET + THEME_ROW_GAP
                } else {
                    TEXT_INSET - GUTTER - ICON
                }))
                .px(px(GUTTER))
                .py(px(12.0))
                .when(p.soft, |row| {
                    row.mx(px(THEME_ROW_MARGIN))
                        .my(px(THEME_ROW_GAP))
                        .px(px(THEME_ROW_INSET))
                        .rounded(px(p.radius()))
                })
                .when_some(p.card, |row, card| row.bg(card))
                .when_some(p.row_line, |row, line| row.border_1().border_color(line))
                .child(row_icon(Icon::Mic, &p))
                .child(title_block(
                    "Микрофон",
                    Some(format!("{mic_hotkey} — включение/выключение")),
                    &p,
                ))
                .child(Selector::new("mic-mode", mic_mode.label()).on_step(step(
                    &state,
                    |settings, forward| {
                        settings.audio.mic_mode = settings.audio.mic_mode.step(forward)
                    },
                )));
        let stats = self
            .link(
                "stats",
                Icon::Stats,
                "Статистика",
                Some(format!(
                    "{stats_hotkey} — открыть/закрыть оверлей статистики"
                )),
                Icon::ChevronRight,
                cx,
            )
            .on_click(
                cx.listener(|overlay, _, window, cx| overlay.push(Page::Interface, window, cx)),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(px(12.0))
            .child(gallery)
            .child(self.gallery_strip(cx))
            .child(divider(&p))
            .child(record)
            .child(replay)
            .child(screenshot)
            .child(photo_mode)
            .child(highlights)
            .child(filters)
            .child(divider(&p))
            .child(microphone)
            .child(div().flex_1())
            .child(divider(&p))
            .child(stats)
            .child(div().h(px(12.0)))
            .into_any_element()
    }

    fn replay(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let enabled = rewind.replay_enabled();
        let toggle_hotkey = hotkey_text(rewind, Command::ToggleReplay);
        let save_hotkey = hotkey_text(rewind, Command::SaveReplay);
        let saving = rewind.replay_saving();
        let state = self.state.clone();
        let save = self
            .link(
                "replay-save",
                Icon::Replay,
                "Сохранить повтор",
                Some(if saving {
                    SAVING_REPLAY.to_owned()
                } else {
                    format!("{save_hotkey} — сохранить")
                }),
                Icon::Check,
                cx,
            )
            .on_click(cx.listener(move |overlay, _, window, cx| {
                if !saving {
                    overlay.save_replay_click(window, cx);
                }
            }));
        let settings = self
            .link(
                "replay-settings",
                Icon::Settings,
                "Настройки",
                None,
                Icon::ChevronRight,
                cx,
            )
            .on_click(cx.listener(|overlay, _, window, cx| overlay.push(Page::Video, window, cx)));
        scroll("replay-scroll")
            .track_scroll(self.page_scroll(Page::Replay))
            .pt(px(12.0))
            .child(
                setting_row(&p)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(TEXT_INSET - GUTTER - ICON))
                            .child(row_icon(Icon::Replay, &p))
                            .child(title_block(
                                if enabled {
                                    "Включен"
                                } else {
                                    "Выключен"
                                },
                                Some(format!("{toggle_hotkey} — включение/выключение")),
                                &p,
                            )),
                    )
                    .child(
                        Toggle::new("replay-toggle", enabled).on_click(move |_, cx| {
                            state.update(cx, |rewind, cx| rewind.toggle_replay(cx))
                        }),
                    ),
            )
            .child(divider(&p))
            .when(enabled, |page| page.child(save))
            .child(settings)
            .into_any_element()
    }

    fn settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let encoder = self.state.read(cx).encoder.clone();
        let entry = |overlay: &Self,
                     id: &'static str,
                     glyph: Icon,
                     title: &str,
                     target: Page,
                     cx: &mut Context<Self>| {
            overlay
                .link(id, glyph, title, None, Icon::ChevronRight, cx)
                .on_click(
                    cx.listener(move |overlay, _, window, cx| overlay.push(target, window, cx)),
                )
        };
        scroll("settings-scroll")
            .track_scroll(self.page_scroll(Page::Settings))
            .pt(px(12.0))
            .child(entry(
                self,
                "hotkeys",
                Icon::Keyboard,
                "Сочетания клавиш",
                Page::Hotkeys,
                cx,
            ))
            .child(entry(
                self,
                "interface",
                Icon::Monitor,
                "Игровой интерфейс",
                Page::Interface,
                cx,
            ))
            .child(entry(
                self,
                "notifications",
                Icon::Bell,
                "Уведомления",
                Page::Notifications,
                cx,
            ))
            .child(entry(
                self,
                "captured",
                Icon::Screenshot,
                "Оверлей в записи",
                Page::Captured,
                cx,
            ))
            .child(divider(&p))
            .child(entry(self, "audio", Icon::Audio, "Аудио", Page::Audio, cx))
            .child(entry(
                self,
                "video",
                Icon::Videocam,
                "Захват видео",
                Page::Video,
                cx,
            ))
            .child(entry(
                self,
                "profiles",
                Icon::Settings,
                "Профили игр",
                Page::Profiles,
                cx,
            ))
            .child(entry(
                self,
                "files",
                Icon::Storage,
                "Файлы и дисковое пространство",
                Page::Files,
                cx,
            ))
            .child(divider(&p))
            .child(self.youtube_entry(cx))
            .child(entry(
                self,
                "sharing",
                Icon::Upload,
                "Скриншоты",
                Page::Sharing,
                cx,
            ))
            .child(divider(&p))
            .child(entry(
                self,
                "appearance",
                Icon::Palette,
                "Внешний вид",
                Page::Appearance,
                cx,
            ))
            .child(entry(
                self,
                "replace",
                Icon::Monitor,
                "Замена NVIDIA ShadowPlay",
                Page::Replace,
                cx,
            ))
            .child(entry(
                self,
                "misc",
                Icon::Settings,
                "Прочее",
                Page::Misc,
                cx,
            ))
            .when_some(encoder, |page, encoder| {
                page.child(divider(&p)).child(
                    div()
                        .px(px(GUTTER))
                        .text_size(px(SMALL_TEXT))
                        .text_color(p.muted)
                        .child(format!("Кодировщик: {encoder}")),
                )
            })
            .into_any_element()
    }

    fn hotkeys(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let capturing = rewind.capturing;
        let taken = rewind.taken.clone();
        let markers = rewind.settings.replay.markers;
        let copy_span = span_text(rewind.settings.sharing.copy_seconds);
        let offer_upload = rewind.settings.youtube.offer_upload;
        let labels: Vec<(Command, String)> = Command::ALL
            .iter()
            .map(|command| (*command, hotkey_text(rewind, *command)))
            .collect();
        let text_of = |command: Command| {
            labels
                .iter()
                .find(|(other, _)| *other == command)
                .map(|(_, text)| text.clone())
                .unwrap_or_default()
        };
        let row = |command: Command, title: String, cx: &mut Context<Self>| {
            let active = capturing == Some(command);
            let conflict = taken.contains(&command);
            let shown = if active {
                PRESS_KEYS.to_owned()
            } else {
                text_of(command)
            };
            setting_row(&p)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(label(title, p.text))
                        .when(conflict, |block| {
                            block.child(
                                div()
                                    .text_size(px(SMALL_TEXT))
                                    .text_color(p.warning)
                                    .child("Занято другой программой"),
                            )
                        }),
                )
                .child(
                    div()
                        .id(("hotkey", command as usize))
                        .cursor_pointer()
                        .child(keybox(shown, active, &p))
                        .on_click(cx.listener(move |overlay, _, _, cx| {
                            overlay
                                .state
                                .update(cx, |rewind, cx| rewind.begin_capture(command, cx))
                        })),
                )
        };
        let footer = div()
            .id("reset-hotkeys")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(TEXT_INSET - GUTTER - ICON))
            .px(px(GUTTER))
            .py(px(16.0))
            .border_t_1()
            .border_color(p.divider)
            .mx(px(0.0))
            .cursor_pointer()
            .child(icon(Icon::Restore, ICON, p.text))
            .child(label("Восстановить настройки по умолчанию?", p.text))
            .on_click(cx.listener(|overlay, _, _, cx| {
                overlay
                    .state
                    .update(cx, |rewind, cx| rewind.reset_bindings(cx))
            }));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                scroll("hotkeys-scroll")
                    .track_scroll(self.page_scroll(Page::Hotkeys))
                    .child(section("Общие", &p))
                    .child(row(
                        Command::ToggleOverlay,
                        "Открыть/закрыть внутриигровой оверлей".into(),
                        cx,
                    ))
                    .child(row(
                        Command::ToggleMic,
                        "Включить/выключить микрофон".into(),
                        cx,
                    ))
                    .child(row(Command::PushToTalk, "Включить Push-to-talk".into(), cx))
                    .child(section("Монитор статистики", &p))
                    .child(row(
                        Command::ToggleStats,
                        "Вкл./выкл. оверлей статистики".into(),
                        cx,
                    ))
                    .child(section("Запись", &p))
                    .child(row(
                        Command::ToggleRecording,
                        "Включить/выключить запись".into(),
                        cx,
                    ))
                    .child(row(
                        Command::ToggleReplay,
                        "Вкл./выкл. мгновенный повтор".into(),
                        cx,
                    ))
                    .child(row(Command::SaveReplay, "Сохранить повтор".to_owned(), cx))
                    .when(markers, |list| {
                        list.child(row(Command::Mark, "Поставить метку".into(), cx))
                    })
                    .when(offer_upload, |list| {
                        list.child(row(
                            Command::UploadReplay,
                            "Залить последний повтор на YouTube".into(),
                            cx,
                        ))
                    })
                    .child(row(
                        Command::CopyLastClip,
                        format!("Скопировать последние {copy_span} для мессенджеров"),
                        cx,
                    ))
                    .child(section("Захват изображения", &p))
                    .child(row(Command::PhotoMode, "Открыть фоторежим".into(), cx))
                    .child(row(Command::Filters, "Изменить фильтры".into(), cx))
                    .child(row(
                        Command::Style1,
                        "Включение/выключение стиля 1".into(),
                        cx,
                    ))
                    .child(row(
                        Command::Style2,
                        "Включение/выключение стиля 2".into(),
                        cx,
                    ))
                    .child(row(
                        Command::Style3,
                        "Включение/выключение стиля 3".into(),
                        cx,
                    ))
                    .child(row(Command::StyleOff, "Выключить фильтры".into(), cx))
                    .child(row(
                        Command::Screenshot,
                        "Сохранить снимок экрана".into(),
                        cx,
                    ))
                    .child(row(
                        Command::PublishScreenshot,
                        "Сделать и опубликовать снимок".into(),
                        cx,
                    )),
            )
            .child(footer)
            .into_any_element()
    }

    fn misc(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let verbose = self.state.read(cx).settings.verbose_logging;
        let logs = self.state.read(cx).logs_dir();
        let shown = logs.join(rewind_trace::FILE).display().to_string();
        scroll("misc-scroll")
            .track_scroll(self.page_scroll(Page::Misc))
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Подробное логирование",
                        Some(VERBOSE_NOTE.to_owned()),
                        &p,
                    ))
                    .child(
                        Toggle::new("verbose-logging", verbose)
                            .on_click(change(&state, |settings| settings.verbose_logging ^= true)),
                    ),
            )
            .child(
                self.link(
                    "open-logs",
                    Icon::Folder,
                    "Открыть папку журналов",
                    Some(shown),
                    Icon::Open,
                    cx,
                )
                .on_click(move |_, _, _| {
                    std::fs::create_dir_all(&logs).ok();
                    crate::platform::shell::open(&logs);
                }),
            )
            .into_any_element()
    }

    fn appearance(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let current = self.state.read(cx).settings.interface.theme;
        let mut page = scroll("appearance-scroll")
            .track_scroll(self.page_scroll(Page::Appearance))
            .pt(px(12.0))
            .child(
                div()
                    .px(px(GUTTER))
                    .pb(px(8.0))
                    .child(hint(APPEARANCE_NOTE, &p)),
            );
        for (index, theme) in Theme::ALL.iter().copied().enumerate() {
            let look = Palette::of(theme);
            let hover = p.hover;
            page = page.child(
                div()
                    .id(("theme", index))
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .px(px(GUTTER))
                    .py(px(10.0))
                    .when(p.soft, |row| {
                        row.mx(px(THEME_ROW_MARGIN))
                            .my(px(THEME_ROW_GAP))
                            .px(px(THEME_ROW_INSET))
                            .py(px(THEME_ROW_INSET))
                            .rounded(px(p.radius()))
                    })
                    .when_some(p.card, |row, card| row.bg(card))
                    .when_some(p.row_line, |row, line| row.border_1().border_color(line))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .w(px(SWATCH_WIDTH))
                            .h(px(SWATCH_HEIGHT))
                            .border_1()
                            .border_color(p.divider)
                            .child(div().flex_1().bg(look.panel))
                            .child(div().flex_1().bg(look.header))
                            .child(div().flex_1().bg(look.accent)),
                    )
                    .child(label(theme.label(), p.text).flex_1())
                    .child(checkbox(theme == current, &p))
                    .on_click(cx.listener(move |overlay, _, _, cx| {
                        overlay.state.update(cx, |rewind, cx| {
                            rewind.change(|settings| settings.interface.theme = theme, cx);
                        });
                        crate::theme::apply(theme, cx);
                    })),
            );
        }
        page.into_any_element()
    }

    fn interface(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let interface = self.state.read(cx).settings.interface.clone();
        let selector_row = |title: &str, selector: Selector| {
            setting_row(&p)
                .child(label(title.to_owned(), p.text).flex_1())
                .child(selector)
        };
        scroll("interface-scroll")
            .track_scroll(self.page_scroll(Page::Interface))
            .child(section("Статистика", &p))
            .child(selector_row(
                "Оверлей статистики",
                Selector::new("stats-corner", interface.stats_corner.label()).on_step(step(
                    &state,
                    |settings, forward| {
                        let corner = settings.interface.stats_corner.step(forward);
                        settings.interface.stats_corner = corner;
                        let (x, y) = match corner {
                            Corner::TopLeft => (0, 0),
                            Corner::TopRight | Corner::Off => (100, 0),
                            Corner::BottomLeft => (0, 100),
                            Corner::BottomRight => (100, 100),
                        };
                        settings.interface.stats_x = x;
                        settings.interface.stats_y = y;
                    },
                )),
            ))
            .child(slider_row(
                "Горизонтальное положение",
                Slider::new("stats-x", percent(interface.stats_x), 0, 100, 1)
                    .on_input(set(&state, |settings, value| {
                        settings.interface.stats_x = as_u8(value)
                    })),
                interface.stats_x.to_string(),
                &p,
            ))
            .child(slider_row(
                "Вертикальное положение",
                Slider::new("stats-y", percent(interface.stats_y), 0, 100, 1)
                    .on_input(set(&state, |settings, value| {
                        settings.interface.stats_y = as_u8(value)
                    })),
                interface.stats_y.to_string(),
                &p,
            ))
            .child(selector_row(
                "Расположение",
                Selector::new("stats-layout", interface.stats_layout.label()).on_step(step(
                    &state,
                    |settings, forward| {
                        settings.interface.stats_layout =
                            settings.interface.stats_layout.step(forward)
                    },
                )),
            ))
            .child(slider_row(
                "Непрозрачность фона",
                Slider::new("stats-opacity", percent(interface.stats_opacity), 0, 100, 1).on_input(
                    set(&state, |settings, value| {
                        settings.interface.stats_opacity = as_u8(value)
                    }),
                ),
                interface.stats_opacity.to_string(),
                &p,
            ))
            .child(selector_row(
                "Размер шрифта",
                Selector::new("stats-font", interface.stats_font_size.label()).on_step(step(
                    &state,
                    |settings, forward| {
                        settings.interface.stats_font_size =
                            settings.interface.stats_font_size.step(forward)
                    },
                )),
            ))
            .child(selector_row(
                "Цвет шрифта",
                Selector::new("stats-color", interface.stats_font_color.label()).on_step(step(
                    &state,
                    |settings, forward| {
                        settings.interface.stats_font_color =
                            settings.interface.stats_font_color.step(forward)
                    },
                )),
            ))
            .child(divider(&p))
            .child(section("Индикаторы состояния", &p))
            .child(selector_row(
                "Запись",
                Selector::new("indicator-record", interface.recording_indicator.label()).on_step(
                    step(&state, |settings, forward| {
                        settings.interface.recording_indicator =
                            settings.interface.recording_indicator.step(forward)
                    }),
                ),
            ))
            .child(selector_row(
                "Мгновенный повтор",
                Selector::new("indicator-replay", interface.replay_indicator.label()).on_step(
                    step(&state, |settings, forward| {
                        settings.interface.replay_indicator =
                            settings.interface.replay_indicator.step(forward)
                    }),
                ),
            ))
            .child(selector_row(
                "Микрофон",
                Selector::new("indicator-mic", interface.mic_indicator.label()).on_step(step(
                    &state,
                    |settings, forward| {
                        settings.interface.mic_indicator =
                            settings.interface.mic_indicator.step(forward)
                    },
                )),
            ))
            .into_any_element()
    }

    fn captured(&mut self, cx: &mut Context<Self>) -> AnyElement {
        const SUBTITLE: &str =
            "Выключено — на записях, повторах и снимках экрана ничего из оверлея не видно";
        let p = cx.palette();
        let state = self.state.clone();
        let captured = self.state.read(cx).settings.captured.clone();
        let captured_part =
            |id: &'static str, title: &str, checked: bool, apply: fn(&mut Settings)| {
                let handler = change(&state, apply);
                let enabled = captured.enabled;
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .px(px(GUTTER + 14.0))
                    .py(px(9.0))
                    .opacity(if enabled { 1.0 } else { 0.4 })
                    .when(enabled, |row| row.cursor_pointer())
                    .child(checkbox(checked && enabled, &p))
                    .child(label(title.to_owned(), p.text))
                    .on_click(move |_, window, cx| {
                        if enabled {
                            handler(window, cx)
                        }
                    })
            };
        scroll("captured-scroll")
            .track_scroll(self.page_scroll(Page::Captured))
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Оверлей в видео и на снимках",
                        Some(SUBTITLE.to_owned()),
                        &p,
                    ))
                    .child(
                        Toggle::new("captured-toggle", captured.enabled)
                            .on_click(change(&state, |settings| settings.captured.enabled ^= true)),
                    ),
            )
            .child(captured_part(
                "captured-panel",
                "Панель оверлея",
                captured.panel,
                |settings| settings.captured.panel ^= true,
            ))
            .child(captured_part(
                "captured-notifications",
                "Уведомления",
                captured.notifications,
                |settings| settings.captured.notifications ^= true,
            ))
            .child(captured_part(
                "captured-indicators",
                "Индикаторы записи, повтора и микрофона",
                captured.indicators,
                |settings| settings.captured.indicators ^= true,
            ))
            .child(captured_part(
                "captured-stats",
                "Статистика",
                captured.stats,
                |settings| settings.captured.stats ^= true,
            ))
            .into_any_element()
    }

    fn notifications(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let notifications = self.state.read(cx).settings.notifications.clone();
        let enabled = notifications.enabled;
        let check = |id: &'static str, title: &str, checked: bool, apply: fn(&mut Settings)| {
            let handler = change(&state, apply);
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(18.0))
                .px(px(GUTTER + 14.0))
                .py(px(9.0))
                .cursor_pointer()
                .opacity(if enabled { 1.0 } else { 0.4 })
                .child(checkbox(checked, &p))
                .child(label(title.to_owned(), p.text))
                .on_click(move |_, window, cx| handler(window, cx))
        };
        scroll("notifications-scroll")
            .track_scroll(self.page_scroll(Page::Notifications))
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Показывать уведомления",
                        Some("Выключите, чтобы отключить сразу все уведомления".to_owned()),
                        &p,
                    ))
                    .child(
                        Toggle::new("notifications-toggle", enabled).on_click(change(
                            &state,
                            |settings| {
                                settings.notifications.enabled = !settings.notifications.enabled
                            },
                        )),
                    ),
            )
            .child(divider(&p))
            .child(section("Общие", &p))
            .child(check(
                "notify-overlay",
                "Открыть/закрыть внутриигровой оверлей",
                notifications.overlay_toggled,
                |settings| settings.notifications.overlay_toggled ^= true,
            ))
            .child(section("Галерея", &p))
            .child(check(
                "notify-recording-saved",
                "Запись сохранена",
                notifications.recording_saved,
                |settings| settings.notifications.recording_saved ^= true,
            ))
            .child(check(
                "notify-replay-saved",
                "Мгновенный повтор сохранен",
                notifications.replay_saved,
                |settings| settings.notifications.replay_saved ^= true,
            ))
            .child(check(
                "notify-screenshot",
                "Снимок экрана сохранен",
                notifications.screenshot_saved,
                |settings| settings.notifications.screenshot_saved ^= true,
            ))
            .child(section("Запись и мгновенный повтор", &p))
            .child(check(
                "notify-replay-toggled",
                "Запись мгновенного повтора включена/выключена",
                notifications.replay_toggled,
                |settings| settings.notifications.replay_toggled ^= true,
            ))
            .child(check(
                "notify-recording-started",
                "Запись запущена",
                notifications.recording_started,
                |settings| settings.notifications.recording_started ^= true,
            ))
            .into_any_element()
    }

    fn microphone_picker(&self, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let mut options: Vec<Option<String>> = vec![None];
        options.extend(rewind.microphones.iter().cloned().map(Some));
        let selected = options
            .iter()
            .position(|option| *option == rewind.settings.audio.mic_device)
            .unwrap_or(0);
        let names = options
            .iter()
            .map(|option| {
                SharedString::from(
                    option
                        .clone()
                        .unwrap_or_else(|| DEFAULT_MICROPHONE.to_owned()),
                )
            })
            .collect();
        let dropdown = self.page_dropdown(
            PagePicker::Microphone,
            names,
            selected,
            move |overlay, index, cx| {
                let choice = options.get(index).cloned().flatten();
                overlay.state.update(cx, |rewind, cx| {
                    rewind.change(|settings| settings.audio.mic_device = choice, cx)
                });
            },
            cx,
        );
        setting_row(&p)
            .child(label("Источник", p.text).flex_1())
            .child(dropdown)
    }

    fn audio(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let audio = self.state.read(cx).settings.audio.clone();
        let max = u32::from(VOLUME_MAX);
        scroll("audio-scroll")
            .track_scroll(self.page_scroll(Page::Audio))
            .child(section("Системные звуки", &p))
            .child(slider_row(
                "Громкость",
                Slider::new("system-volume", percent(audio.system_volume), 0, max, 1).on_input(
                    set(&state, |settings, value| {
                        settings.audio.system_volume = as_u8(value)
                    }),
                ),
                format!("{} %", audio.system_volume),
                &p,
            ))
            .child(divider(&p))
            .child(section("Микрофон", &p))
            .child(self.microphone_picker(cx))
            .child(slider_row(
                "Громкость",
                Slider::new("mic-volume", percent(audio.mic_volume), 0, max, 1)
                    .on_input(set(&state, |settings, value| {
                        settings.audio.mic_volume = as_u8(value)
                    })),
                format!("{} %", audio.mic_volume),
                &p,
            ))
            .child(slider_row(
                "Усиление",
                Slider::new("mic-boost", percent(audio.mic_boost), 0, max, 1)
                    .on_input(set(&state, |settings, value| {
                        settings.audio.mic_boost = as_u8(value)
                    })),
                format!("{} %", audio.mic_boost),
                &p,
            ))
            .child(divider(&p))
            .child(section("Аудиодорожки", &p))
            .child(
                setting_row(&p)
                    .items_start()
                    .child(title_block(
                        "Формат",
                        Some(audio.tracks.hint().to_owned()),
                        &p,
                    ))
                    .child(Selector::new("tracks", audio.tracks.label()).on_step(step(
                        &state,
                        |settings, forward| {
                            settings.audio.tracks = settings.audio.tracks.step(forward)
                        },
                    ))),
            )
            .into_any_element()
    }

    fn video(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let rewind = self.state.read(cx);
        let video = rewind.settings.video.clone();
        let free = *self.free_space.get(
            rewind.settings.files.temp.clone(),
            Some(PROBE_REFRESH),
            Instant::now(),
            |temp| rewind_capture::free_space(temp),
        );
        let saving = rewind.settings.replay.clone();
        let sharing = rewind.settings.sharing.clone();
        let upload = rewind.settings.youtube.clone();
        let upload_hotkey = hotkey_text(rewind, Command::UploadReplay);
        let copy_hotkey = hotkey_text(rewind, Command::CopyLastClip);
        let av1_encoder = rewind.av1_encoder;
        let mark_hotkey = hotkey_text(rewind, Command::Mark);
        let encoder = rewind.encoder.clone();
        let several_screens = rewind.screens.len() > 1;
        let choose_screen = rewind.settings.choose_screen;
        let selector_row = |title: &str, selector: Selector| {
            setting_row(&p)
                .child(label(title.to_owned(), p.text).flex_1())
                .child(selector)
        };
        scroll("video-scroll").track_scroll(self.page_scroll(Page::Video))
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Запись рабочего стола",
                        Some(
                            "Выключите, чтобы не захватывать весь рабочий стол при видеозаписи и на снимках экрана."
                                .to_owned(),
                        ),
                        &p,
                    ))
                    .child(Toggle::new("capture-desktop", video.capture_desktop).on_click(change(
                        &state,
                        |settings| settings.video.capture_desktop = !settings.video.capture_desktop,
                    ))),
            )
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Записывать только окно игры",
                        Some(
                            "Пока идёт игра, записывается только её окно: в клип не попадут уведомления, Discord и сообщения поверх игры. Картинку отдаёт сама Windows, как в OBS, в игру ничего не встраивается. Без игры записывается рабочий стол. Разрешение и битрейт берутся из настроек ниже."
                                .to_owned(),
                        ),
                        &p,
                    ))
                    .child(Toggle::new("window-only", video.window_only).on_click(change(
                        &state,
                        |settings| settings.video.window_only = !settings.video.window_only,
                    ))),
            )
            .child(
                setting_row(&p)
                    .when(!several_screens, |row| row.opacity(DISABLED_ROW))
                    .child(title_block(
                        "Выбор экрана для захвата",
                        Some(
                            if several_screens {
                                "При запуске записи и сохранении повтора из оверлея можно выбрать экран. Повтор ведётся на каждом экране отдельно, поэтому нагрузка на видеокарту и место под повтор растут по числу экранов. Сочетания клавиш всегда работают с основным экраном."
                            } else {
                                "Доступно, когда подключено больше одного монитора."
                            }
                            .to_owned(),
                        ),
                        &p,
                    ))
                    .child(
                        Toggle::new("choose-screen", several_screens && choose_screen)
                            .disabled(!several_screens)
                            .on_click(change(&state, |settings| {
                                settings.choose_screen = !settings.choose_screen;
                            })),
                    ),
            )
            .child(divider(&p))
            .child(
                div()
                    .px(px(GUTTER))
                    .child(label("Длительность мгновенного повтора", p.text))
                    .child(hint(
                        replay_disk_hint(&video, free, rewind_capture::free_memory()),
                        &p,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .px(px(GUTTER))
                    .pt(px(14.0))
                    .pb(px(6.0))
                    .pl(px(GUTTER + 8.0))
                    .child(
                        Slider::new(
                            "replay-length",
                            replay_step(video.replay_seconds) as u32,
                            0,
                            (REPLAY_STEPS.len() - 1) as u32,
                            1,
                        )
                        .on_input(set(&state, |settings, value| {
                            settings.video.replay_seconds =
                                REPLAY_STEPS[(value as usize).min(REPLAY_STEPS.len() - 1)]
                        })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(REPLAY_VALUE_WIDTH))
                            .child(field(self.replay_input.clone(), false, &p)),
                    ),
            )
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Выбирать отрезок перед сохранением",
                        Some(
                            "При сохранении откроется полоса времени, на которой можно выбрать нужный кусок. Если выключено, сохраняется весь повтор."
                                .to_owned(),
                        ),
                        &p,
                    ))
                    .child(Toggle::new("choose-range", saving.choose_range).on_click(change(
                        &state,
                        |settings| settings.replay.choose_range = !settings.replay.choose_range,
                    ))),
            )
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Метки",
                        Some(format!(
                            "{mark_hotkey} ставит метку во время игры. При сохранении повтора метки видны на полосе времени, а клип вокруг метки сохраняется одной кнопкой. Сочетание клавиш меняется в разделе «Сочетания клавиш»."
                        )),
                        &p,
                    ))
                    .child(Toggle::new("markers", saving.markers).on_click(change(
                        &state,
                        |settings| settings.replay.markers = !settings.replay.markers,
                    ))),
            )
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Предлагать залить повтор на YouTube",
                        Some(format!(
                            "После сохранения повтора уведомление предложит залить его. Чтобы согласиться, нажмите {upload_hotkey}. Название берётся из имени файла, описание — по умолчанию: с характеристиками ПК, если они включены, и с подписью Rewind."
                        )),
                        &p,
                    ))
                    .child(Toggle::new("offer-upload", upload.offer_upload).on_click(change(
                        &state,
                        |settings| settings.youtube.offer_upload = !settings.youtube.offer_upload,
                    ))),
            )
            .when(upload.offer_upload, |page| {
                page.child(
                    setting_row(&p)
                        .child(label("Что делать по сочетанию", p.text).flex_1())
                        .child(Selector::new("upload-mode", upload.upload_mode.label()).on_step(
                            step(&state, |settings, forward| {
                                settings.youtube.upload_mode = settings.youtube.upload_mode.step(forward)
                            }),
                        )),
                )
                .when(upload.upload_mode == rewind_config::UploadMode::Direct, |page| {
                    page.child(
                        setting_row(&p)
                            .child(label("Доступ", p.text).flex_1())
                            .child(
                                Selector::new("upload-privacy", upload.upload_privacy.label())
                                    .on_step(step(&state, |settings, forward| {
                                        settings.youtube.upload_privacy =
                                            settings.youtube.upload_privacy.step(forward)
                                    })),
                            ),
                    )
                })
            })
            .child(
                div()
                    .px(px(GUTTER))
                    .pt(px(8.0))
                    .child(label("Копирование последнего клипа", p.text))
                    .child(hint(
                        format!(
                            "{copy_hotkey} сохраняет последние секунды повтора, сжимает их под лимит и кладёт файл в буфер обмена — останется вставить в чат."
                        ),
                        &p,
                    )),
            )
            .child(
                setting_row(&p)
                    .child(label("Куда", p.text).flex_1())
                    .child(Selector::new("copy-target", sharing.quick.label()).on_step(step(
                        &state,
                        |settings, forward| settings.sharing.quick = settings.sharing.quick.step(forward),
                    ))),
            )
            .child(mark_span_row(
                "Длина",
                "copy-seconds",
                sharing.copy_seconds,
                COPY_SECONDS_MIN,
                COPY_SECONDS_MAX,
                set(&state, |settings, value| settings.sharing.copy_seconds = value),
                &p,
            ))
            .when(saving.markers, |page| {
                page.child(mark_span_row(
                    "До метки",
                    "before-mark",
                    saving.before_mark,
                    MARK_SPAN_MIN_SECONDS,
                    MARK_SPAN_MAX_SECONDS,
                    set(&state, |settings, value| settings.replay.before_mark = value),
                    &p,
                ))
                .child(mark_span_row(
                    "После метки",
                    "after-mark",
                    saving.after_mark,
                    MARK_SPAN_MIN_SECONDS,
                    MARK_SPAN_MAX_SECONDS,
                    set(&state, |settings, value| settings.replay.after_mark = value),
                    &p,
                ))
            })
            .child(divider(&p))
            .child(
                div()
                    .px(px(GUTTER))
                    .pb(px(4.0))
                    .child(label("Формат вывода", p.text))
                    .child(hint(
                        "Настройка параметров видеозаписи в функциях «Мгновенный повтор» и «Запись».",
                        &p,
                    )),
            )
            .child(selector_row(
                "Качество",
                Selector::new("quality", video.quality.label()).on_step(step(&state, |settings, forward| {
                    settings.video.quality = settings.video.quality.step(forward)
                })),
            ))
            .child(selector_row(
                "Разрешение",
                Selector::new("resolution", video.resolution.label()).on_step(step(
                    &state,
                    |settings, forward| settings.video.resolution = settings.video.resolution.step(forward),
                )),
            ))
            .child(selector_row(
                "Частота кадров",
                Selector::new("frame-rate", video.frame_rate.label()).on_step(step(
                    &state,
                    |settings, forward| settings.video.frame_rate = settings.video.frame_rate.step(forward),
                )),
            ))
            .child(selector_row(
                "Кодек",
                Selector::new("codec", video.codec.label()).on_step(step(&state, |settings, forward| {
                    settings.video.codec = settings.video.codec.step(forward)
                })),
            ))
            .when(video.frame_rate.is_variable(), |page| {
                page.child(div().px(px(GUTTER)).pb(px(6.0)).child(hint(
                    "Записывается ровно столько кадров, сколько показывает игра, до 120 в секунду. Пока картинка стоит, кадры не тратят место. Буфер повтора при этом не сбрасывается.",
                    &p,
                )))
            })
            .when(video.codec == Codec::Av1, |page| {
                page.child(div().px(px(GUTTER)).pb(px(6.0)).child(hint(
                    if av1_encoder {
                        "AV1 даёт ту же картинку при меньшем размере файла. Для просмотра в Windows нужно расширение «AV1 Video Extension», в Windows 11 оно обычно уже есть."
                    } else {
                        "Ваша видеокарта не умеет записывать AV1, для этого нужна RTX 40 или новее. Запись пойдёт в HEVC."
                    },
                    &p,
                )))
            })
            .child(slider_row(
                "Скорость передачи (Мбит/с)",
                Slider::new(
                    "bitrate",
                    video.bitrate_mbps(),
                    BITRATE_MIN_MBPS,
                    BITRATE_MAX_MBPS,
                    1,
                )
                .on_input(set(&state, |settings, value| {
                    settings.video.quality = Quality::Custom;
                    settings.video.custom_bitrate_mbps = value;
                })),
                video.bitrate_mbps().to_string(),
                &p,
            ))
            .when_some(encoder, |page, encoder| {
                page.child(
                    div()
                        .px(px(GUTTER))
                        .pt(px(8.0))
                        .text_size(px(SMALL_TEXT))
                        .text_color(p.muted)
                        .child(format!("Кодировщик: {encoder}")),
                )
            })
            .into_any_element()
    }

    fn path_row(
        &self,
        id: &'static str,
        title: &str,
        path: &Path,
        apply: fn(&mut Settings, std::path::PathBuf),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = cx.palette();
        let current = path.to_path_buf();
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(GUTTER))
            .py(px(10.0))
            .cursor_pointer()
            .child(title_block(
                title.to_owned(),
                Some(path.display().to_string()),
                &p,
            ))
            .child(icon(Icon::Folder, ICON, p.text))
            .on_click(cx.listener(move |_, _, window, cx| {
                let picking = rfd::AsyncFileDialog::new()
                    .set_directory(&current)
                    .set_parent(window)
                    .pick_folder();
                cx.spawn(async move |this, cx| {
                    let Some(folder) = picking.await else {
                        return;
                    };
                    let folder = folder.path().to_path_buf();
                    this.update(cx, |overlay, cx| {
                        overlay.state.update(cx, |rewind, cx| {
                            rewind.change(|settings| apply(settings, folder), cx)
                        });
                    })
                    .ok();
                })
                .detach();
            }))
    }

    fn files(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let state = self.state.clone();
        let files = self.state.read(cx).settings.files.clone();
        scroll("files-scroll").track_scroll(self.page_scroll(Page::Files))
            .pt(px(8.0))
            .child(
                setting_row(&p)
                    .child(title_block(
                        "Ограничение места на диске",
                        Some(
                            "Ограничить объем памяти, занимаемой мгновенными повторами, снимками экрана и записями"
                                .to_owned(),
                        ),
                        &p,
                    ))
                    .child(Toggle::new("disk-limit", files.disk_limit).on_click(change(
                        &state,
                        |settings| settings.files.disk_limit = !settings.files.disk_limit,
                    ))),
            )
            .when(files.disk_limit, |page| {
                page.child(slider_row(
                    "Лимит (ГБ)",
                    Slider::new(
                        "disk-limit-gb",
                        files.disk_limit_gb,
                        DISK_LIMIT_MIN_GB,
                        DISK_LIMIT_MAX_GB,
                        5,
                    )
                    .on_input(set(&state, |settings, value| settings.files.disk_limit_gb = value)),
                    files.disk_limit_gb.to_string(),
                    &p,
                ))
            })
            .child(self.path_row(
                "temp-path",
                "Расположение временных файлов",
                &files.temp,
                |settings, folder| settings.files.temp = folder,
                cx,
            ))
            .child(self.path_row(
                "gallery-path",
                "Расположение Галереи",
                &files.gallery,
                |settings, folder| settings.files.gallery = folder,
                cx,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{clock, parse_span, span_text};

    #[test]
    fn typed_replay_lengths_are_understood() {
        assert_eq!(parse_span("45"), Some(45 * 60));
        assert_eq!(parse_span("45 мин"), Some(45 * 60));
        assert_eq!(parse_span("1:30"), Some(90));
        assert_eq!(parse_span("1:30:00"), Some(5400));
        assert_eq!(parse_span("2ч 15м"), Some(8100));
        assert_eq!(parse_span("1,5 ч"), Some(5400));
        assert_eq!(parse_span("90 с"), Some(90));
        assert_eq!(parse_span("5 s"), Some(15));
        assert_eq!(parse_span("100ч"), Some(86_400));
        assert_eq!(parse_span("abc"), None);
        assert_eq!(parse_span(""), None);
    }

    #[test]
    fn long_replays_read_in_hours() {
        assert_eq!(span_text(45), "45 с");
        assert_eq!(span_text(90), "1 мин 30 с");
        assert_eq!(span_text(300), "5 мин");
        assert_eq!(span_text(5400), "1 ч 30 мин");
        assert_eq!(span_text(86_400), "24 ч");
        assert_eq!(clock(300), "05:00");
        assert_eq!(clock(86_400), "24:00:00");
    }
}
