use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    AnyElement, ClipboardItem, Context, Div, ObjectFit, SharedString, Window, div, img, prelude::*,
    px,
};

use crate::assets::{Icon, icon};
use crate::channels::Managing;
use crate::channels::text as yt;
use crate::platform;
use crate::theme::{GUTTER, PANEL_WIDTH, ThemeExt, alpha};
use crate::ui::input::TextInput;
use crate::ui::{ButtonKind, Selector, button, divider, field, hint, label, section, setting_row};
use crate::views::overlay::{Overlay, PROBE_REFRESH, Page};

const PREVIEW_WIDTH: f32 = PANEL_WIDTH - GUTTER * 2.0;
const PLAY_SIZE: f32 = 56.0;
const DESCRIPTION_LIMIT: usize = youtube::publish::MAX_DESCRIPTION_CHARS;

pub struct PublishedEdit {
    pub title: gpui::Entity<TextInput>,
    pub description: gpui::Entity<TextInput>,
    pub privacy: Option<youtube::Privacy>,
    pub thumbnail: Option<PathBuf>,
    pub filled_for: Option<String>,
    pub remembered: Option<(String, String)>,
    pub confirm_delete: bool,
    pub new_thumbnail: Option<String>,
}

impl PublishedEdit {
    pub fn new(window: &mut Window, cx: &mut Context<Overlay>) -> Self {
        Self {
            title: cx.new(|cx| {
                TextInput::new("Название", window, cx).max_chars(youtube::publish::MAX_TITLE_CHARS)
            }),
            description: cx.new(|cx| {
                TextInput::new("Описание", window, cx)
                    .multiline()
                    .max_chars(DESCRIPTION_LIMIT)
            }),
            privacy: None,
            thumbnail: None,
            filled_for: None,
            remembered: None,
            confirm_delete: false,
            new_thumbnail: None,
        }
    }
}

const fn step_privacy(privacy: youtube::Privacy, forward: bool) -> youtube::Privacy {
    use youtube::Privacy::{Private, Public, Unlisted};
    match (privacy, forward) {
        (Private, true) | (Public, false) => Unlisted,
        (Unlisted, true) | (Private, false) => Public,
        (Public, true) | (Unlisted, false) => Private,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Published {
    Video(String),
    Shot(String),
}

impl Overlay {
    pub fn open_published(&mut self, item: Published, window: &mut Window, cx: &mut Context<Self>) {
        self.close_viewer(window, cx);
        let viewed = self.published_file(&item, cx);
        self.published_edit.filled_for = None;
        self.published_edit.thumbnail = None;
        self.published_edit.confirm_delete = false;
        if let Published::Video(video_id) = &item {
            let id = video_id.clone();
            let known = self.state.read(cx).remembered(&id).unwrap_or_default();
            self.published_edit.privacy = known.privacy;
            self.published_edit
                .title
                .update(cx, |input, cx| input.set_text(known.title.clone(), cx));
            self.published_edit.description.update(cx, |input, cx| {
                input.set_text(known.description.clone(), cx)
            });
            self.published_edit.remembered = Some((
                self.published_edit.title.read(cx).text().to_owned(),
                self.published_edit.description.read(cx).text().to_owned(),
            ));
            self.state
                .update(cx, |rewind, cx| rewind.load_published(&id, cx));
        }
        let video = matches!(item, Published::Video(_));
        self.published = Some(item);
        self.push(Page::Published, window, cx);
        match viewed {
            Some(path) if video => self.open_viewer(path, window, cx),
            Some(path) => self.open_photo(path, false, window, cx),
            None => {}
        }
    }

    fn published_file(&self, item: &Published, cx: &gpui::App) -> Option<PathBuf> {
        let rewind = self.state.read(cx);
        match item {
            Published::Video(video_id) => rewind
                .published(video_id)
                .and_then(|entry| rewind.youtube.local_video(&entry)),
            Published::Shot(link) => {
                let upload = rewind
                    .uploads
                    .entries
                    .iter()
                    .find(|upload| upload.link == *link)?;
                Some(upload.file.clone())
                    .filter(|file| file.exists())
                    .or_else(|| rewind.upload_image(upload))
            }
        }
    }

    fn copy_link(&self, link: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
        self.state.update(cx, |rewind, cx| {
            rewind.toast(Icon::Copy, "Ссылка скопирована", Some(link), false, cx)
        });
    }

    pub fn published_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let body = match self.published.clone() {
            Some(Published::Video(video_id)) => self.published_video(&video_id, cx),
            Some(Published::Shot(link)) => self.published_shot(&link, cx),
            None => None,
        };
        let p = cx.palette();
        div()
            .id("published-scroll")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .pb(px(GUTTER))
            .child(body.unwrap_or_else(|| {
                div()
                    .px(px(GUTTER))
                    .pt(px(GUTTER))
                    .child(hint("Эта публикация уже убрана из истории.", &p))
            }))
            .into_any_element()
    }

    fn preview(
        &self,
        id: &'static str,
        image: Option<PathBuf>,
        playable: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = cx.palette();
        div()
            .id(id)
            .relative()
            .mx(px(GUTTER))
            .mt(px(12.0))
            .w(px(PREVIEW_WIDTH))
            .h(px(PREVIEW_WIDTH * 9.0 / 16.0))
            .overflow_hidden()
            .bg(p.field)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .map(|frame| match image {
                Some(image) => frame.child(
                    img(image)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                ),
                None => frame.child(icon(
                    if playable { Icon::Play } else { Icon::Photo },
                    32.0,
                    p.muted,
                )),
            })
            .when(playable, |frame| {
                frame.child(
                    div()
                        .absolute()
                        .size(px(PLAY_SIZE))
                        .rounded_full()
                        .bg(alpha(p.stage, 0.6))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(
                            Icon::Play,
                            28.0,
                            if p.soft { p.white } else { p.title },
                        )),
                )
            })
    }

    fn link_row(&self, id: &'static str, link: &str, cx: &mut Context<Self>) -> Div {
        let p = cx.palette();
        let copy = link.to_owned();
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .mx(px(GUTTER))
            .px(px(10.0))
            .py(px(8.0))
            .bg(p.field)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(p.text)
                    .child(SharedString::from(link.to_owned())),
            )
            .child(
                div()
                    .id(id)
                    .flex_none()
                    .cursor_pointer()
                    .child(label("Копировать", p.accent))
                    .on_click(
                        cx.listener(move |overlay, _, _, cx| overlay.copy_link(copy.clone(), cx)),
                    ),
            )
    }

    fn published_video(&mut self, video_id: &str, cx: &mut Context<Self>) -> Option<Div> {
        let rewind = self.state.read(cx);
        let entry = rewind.published(video_id)?;
        let preview = rewind.youtube.thumbnail(video_id);
        let account = rewind.youtube.accounts.get(&entry.account_id).map_or_else(
            || entry.account_id.clone(),
            |account| account.display_name(),
        );
        let local = rewind.youtube.local_video(&entry);
        let details = rewind.youtube.details.get(video_id).cloned();
        let busy = rewind.youtube.managing.get(video_id).copied();
        if let Some(details) = &details
            && self.published_edit.filled_for.as_deref() != Some(video_id)
        {
            self.published_edit.filled_for = Some(video_id.to_owned());
            self.published_edit.privacy = details.privacy.or(Some(entry.privacy));
            self.published_edit.thumbnail = None;
            let (title, description) = (details.title.clone(), details.description.clone());
            let remembered = self.published_edit.remembered.take();
            let untouched = |shown: &str, pick: fn(&(String, String)) -> &String| {
                remembered.as_ref().is_none_or(|known| pick(known) == shown)
            };
            if untouched(self.published_edit.title.read(cx).text(), |known| &known.0) {
                self.published_edit
                    .title
                    .update(cx, |input, cx| input.set_text(title, cx));
            }
            if untouched(self.published_edit.description.read(cx).text(), |known| {
                &known.1
            }) {
                self.published_edit
                    .description
                    .update(cx, |input, cx| input.set_text(description, cx));
            }
        }
        let p = cx.palette();
        let link = entry.url();
        let open_link = link.clone();
        let (play, forget) = (video_id.to_owned(), video_id.to_owned());
        let status = match busy {
            Some(Managing::Reading) => Some("Загружаю данные из YouTube…"),
            Some(Managing::Saving) => Some("Сохраняю на YouTube…"),
            Some(Managing::Deleting) => Some("Удаляю с YouTube…"),
            None if details.is_none() => Some("Данные из YouTube не загружены"),
            None => None,
        };
        let ready = details.is_some() && busy.is_none();
        let privacy = self.published_edit.privacy.unwrap_or(entry.privacy);
        let thumbnail_label = self
            .published_edit
            .thumbnail
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(
                || "Выбрать картинку…".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    self.preview("published-play", preview, true, cx)
                        .on_click(cx.listener(move |overlay, _, _, cx| {
                            overlay
                                .state
                                .update(cx, |rewind, cx| rewind.play_published(&play, cx))
                        })),
                )
                .child(
                    div()
                        .px(px(GUTTER))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(label(entry.title.clone(), p.text))
                        .child(hint(
                            format!(
                                "{account} · {} · {} · {} · {}",
                                youtube::iso_date(entry.uploaded_at),
                                crate::gallery::format_size(entry.bytes),
                                yt::privacy(entry.privacy),
                                yt::status(&entry.status)
                            ),
                            &p,
                        )),
                )
                .child(self.link_row("published-copy", &link, cx))
                .child(
                    div().px(px(GUTTER)).child(
                        button(
                            "published-open",
                            "Открыть на YouTube",
                            ButtonKind::Secondary,
                            true,
                            &p,
                        )
                        .w_full()
                        .on_click(move |_, _, _| platform::shell::open_url(&open_link)),
                    ),
                )
                .child(divider(&p))
                .child(section("Изменить", &p))
                .when_some(status, |column, status| {
                    column.child(div().px(px(GUTTER)).child(hint(status, &p)))
                })
                .child(
                    div()
                        .px(px(GUTTER))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(hint("Название", &p))
                        .child(field(self.published_edit.title.clone(), false, &p))
                        .child(hint("Описание", &p))
                        .child(field(self.published_edit.description.clone(), true, &p)),
                )
                .child(
                    setting_row(&p)
                        .child(label("Доступ", p.text).flex_1())
                        .child(
                            Selector::new("published-privacy", yt::privacy(privacy)).on_step({
                                let entity = cx.entity();
                                move |forward, _, cx| {
                                    entity.update(cx, |overlay, cx| {
                                        overlay.published_edit.privacy =
                                            Some(step_privacy(privacy, forward));
                                        cx.notify();
                                    })
                                }
                            }),
                        ),
                )
                .child(
                    setting_row(&p)
                        .child(label("Значок", p.text).flex_1())
                        .child(
                            div()
                                .id("published-thumbnail")
                                .cursor_pointer()
                                .child(label(thumbnail_label, p.accent))
                                .on_click(cx.listener(|overlay, _, window, cx| {
                                    let picked = rfd::FileDialog::new()
                                        .add_filter("Картинка", &["png", "jpg", "jpeg"])
                                        .set_parent(window)
                                        .pick_file();
                                    if picked.is_some() {
                                        overlay.published_edit.thumbnail = picked;
                                        cx.notify();
                                    }
                                })),
                        ),
                )
                .child(
                    div().px(px(GUTTER)).pt(px(4.0)).child(
                        button(
                            "published-save",
                            "Сохранить на YouTube",
                            ButtonKind::Primary,
                            ready,
                            &p,
                        )
                        .w_full()
                        .when(ready, |button| {
                            let id = video_id.to_owned();
                            button.on_click(cx.listener(move |overlay, _, _, cx| {
                                overlay.save_published_edit(&id, cx)
                            }))
                        }),
                    ),
                )
                .child(
                    div().px(px(GUTTER)).child(
                        button(
                            "published-delete",
                            "Удалить с YouTube",
                            ButtonKind::Danger,
                            busy.is_none(),
                            &p,
                        )
                        .w_full()
                        .when(busy.is_none(), |button| {
                            button.on_click(cx.listener(|overlay, _, _, cx| {
                                overlay.published_edit.confirm_delete = true;
                                cx.notify();
                            }))
                        }),
                    ),
                )
                .child(divider(&p))
                .child(section("На этом компьютере", &p))
                .child(match local {
                    Some(local) => {
                        let (open, reveal) = (local.clone(), local.clone());
                        div()
                            .flex()
                            .gap(px(10.0))
                            .px(px(GUTTER))
                            .child(
                                button(
                                    "published-file",
                                    "Открыть файл",
                                    ButtonKind::Secondary,
                                    true,
                                    &p,
                                )
                                .flex_1()
                                .on_click(move |_, _, _| platform::shell::open(&open)),
                            )
                            .child(
                                button(
                                    "published-folder",
                                    "Показать в папке",
                                    ButtonKind::Secondary,
                                    true,
                                    &p,
                                )
                                .flex_1()
                                .on_click(move |_, _, _| platform::shell::select(&reveal)),
                            )
                    }
                    None => div()
                        .px(px(GUTTER))
                        .child(hint("Исходного файла здесь уже нет.", &p)),
                })
                .child(
                    div().px(px(GUTTER)).child(
                        button(
                            "published-forget",
                            "Убрать из истории",
                            ButtonKind::Secondary,
                            true,
                            &p,
                        )
                        .w_full()
                        .on_click(cx.listener(
                            move |overlay, _, window, cx| {
                                overlay
                                    .state
                                    .update(cx, |rewind, cx| rewind.forget_published(&forget, cx));
                                overlay.go_back(window, cx);
                            },
                        )),
                    ),
                ),
        )
    }

    fn save_published_edit(&mut self, video_id: &str, cx: &mut Context<Self>) {
        let Some(details) = self.state.read(cx).youtube.details.get(video_id).cloned() else {
            return;
        };
        let title = self.published_edit.title.read(cx).text().trim().to_owned();
        let description = self
            .published_edit
            .description
            .read(cx)
            .text()
            .trim_end()
            .to_owned();
        if title.is_empty() {
            self.state.update(cx, |rewind, cx| {
                rewind.toast(Icon::Close, "Название не может быть пустым", None, true, cx)
            });
            return;
        }
        let current = details.privacy.or_else(|| {
            self.state
                .read(cx)
                .published(video_id)
                .map(|entry| entry.privacy)
        });
        let changes = youtube::manage::Changes {
            title: (title != details.title).then_some(title),
            description: (description != details.description).then_some(description),
            privacy: self
                .published_edit
                .privacy
                .filter(|privacy| Some(*privacy) != current),
            thumbnail: self.published_edit.thumbnail.take(),
        };
        if changes.is_empty() {
            self.state.update(cx, |rewind, cx| {
                rewind.toast(Icon::Check, "Ничего не изменилось", None, false, cx)
            });
            return;
        }
        let id = video_id.to_owned();
        if changes.thumbnail.is_some() {
            self.published_edit.new_thumbnail = Some(id.clone());
        }
        self.state
            .update(cx, |rewind, cx| rewind.save_published(&id, changes, cx));
        cx.notify();
    }

    pub fn youtube_delete_layer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.published_edit.confirm_delete {
            return None;
        }
        let Some(Published::Video(video_id)) = self.published.clone() else {
            return None;
        };
        let p = cx.palette();
        let title = self
            .state
            .read(cx)
            .published(&video_id)
            .map(|entry| entry.title)
            .unwrap_or_default();
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.stage, 0.55))
                .occlude()
                .child(
                    div()
                        .w(px(PANEL_WIDTH - GUTTER * 4.0))
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .p(px(20.0))
                        .bg(p.panel)
                        .shadow(p.toast_shadow())
                        .child(label("Удалить видео с YouTube навсегда?", p.text))
                        .child(hint(
                            format!("«{title}» — восстановить его будет нельзя."),
                            &p,
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(10.0))
                                .child(
                                    button(
                                        "youtube-delete-cancel",
                                        "Отмена",
                                        ButtonKind::Secondary,
                                        true,
                                        &p,
                                    )
                                    .on_click(cx.listener(
                                        |overlay, _, _, cx| {
                                            overlay.published_edit.confirm_delete = false;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    button(
                                        "youtube-delete-confirm",
                                        "Удалить навсегда",
                                        ButtonKind::Danger,
                                        true,
                                        &p,
                                    )
                                    .on_click(cx.listener(
                                        move |overlay, _, _, cx| {
                                            overlay.published_edit.confirm_delete = false;
                                            let id = video_id.clone();
                                            overlay.state.update(cx, |rewind, cx| {
                                                rewind.delete_published(&id, cx)
                                            });
                                            cx.notify();
                                        },
                                    )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn published_shot(&mut self, link: &str, cx: &mut Context<Self>) -> Option<Div> {
        let rewind = self.state.read(cx);
        let upload = rewind
            .uploads
            .entries
            .iter()
            .find(|upload| upload.link == link)?
            .clone();
        let key = (link.to_owned(), rewind.previewing_uploads());
        let (preview, local, size) = self
            .shot_files
            .get(key, Some(PROBE_REFRESH), Instant::now(), |_| {
                let size = std::fs::metadata(&upload.file).ok().map(|meta| meta.len());
                (
                    rewind.upload_image(&upload),
                    size.map(|_| upload.file.clone()),
                    size,
                )
            })
            .clone();
        let p = cx.palette();
        let open_link = link.to_owned();
        let forget = link.to_owned();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child({
                    let shown = preview.clone().or_else(|| local.clone());
                    let viewed = open_link.clone();
                    self.preview("published-shot", shown, false, cx)
                        .on_click(move |_, _, _| platform::shell::open_url(&viewed))
                })
                .child(
                    div()
                        .px(px(GUTTER))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(label(upload.name(), p.text))
                        .child(hint(
                            match size {
                                Some(size) => format!(
                                    "{} · {} · {}",
                                    upload.host.label(),
                                    youtube::iso_date(upload.uploaded_at),
                                    crate::gallery::format_size(size)
                                ),
                                None => format!(
                                    "{} · {}",
                                    upload.host.label(),
                                    youtube::iso_date(upload.uploaded_at)
                                ),
                            },
                            &p,
                        )),
                )
                .child(self.link_row("published-shot-copy", link, cx))
                .child(
                    div()
                        .flex()
                        .gap(px(10.0))
                        .px(px(GUTTER))
                        .child(
                            button(
                                "published-shot-open",
                                "Открыть в браузере",
                                ButtonKind::Secondary,
                                true,
                                &p,
                            )
                            .flex_1()
                            .on_click(move |_, _, _| platform::shell::open_url(&open_link)),
                        )
                        .when_some(upload.delete.clone(), |row, delete| {
                            row.child(
                                button(
                                    "published-shot-delete",
                                    "Удалить с хостинга",
                                    ButtonKind::Danger,
                                    true,
                                    &p,
                                )
                                .flex_1()
                                .on_click(move |_, _, _| platform::shell::open_url(&delete)),
                            )
                        }),
                )
                .when(upload.delete.is_none(), |page| {
                    page.child(div().px(px(GUTTER)).child(hint(
                        format!(
                            "{} не даёт удалить анонимно загруженный снимок.",
                            upload.host.label()
                        ),
                        &p,
                    )))
                })
                .child(divider(&p))
                .child(section("На этом компьютере", &p))
                .child(match local {
                    Some(local) => {
                        let (open, reveal) = (local.clone(), local.clone());
                        div()
                            .flex()
                            .gap(px(10.0))
                            .px(px(GUTTER))
                            .child(
                                button(
                                    "published-shot-file",
                                    "Открыть файл",
                                    ButtonKind::Secondary,
                                    true,
                                    &p,
                                )
                                .flex_1()
                                .on_click(move |_, _, _| platform::shell::open(&open)),
                            )
                            .child(
                                button(
                                    "published-shot-folder",
                                    "Показать в папке",
                                    ButtonKind::Secondary,
                                    true,
                                    &p,
                                )
                                .flex_1()
                                .on_click(move |_, _, _| platform::shell::select(&reveal)),
                            )
                    }
                    None => div()
                        .px(px(GUTTER))
                        .child(hint("Исходного файла здесь уже нет.", &p)),
                })
                .child(
                    div().px(px(GUTTER)).child(
                        button(
                            "published-shot-forget",
                            "Убрать из истории",
                            ButtonKind::Secondary,
                            true,
                            &p,
                        )
                        .w_full()
                        .on_click(cx.listener(
                            move |overlay, _, window, cx| {
                                overlay
                                    .state
                                    .update(cx, |rewind, cx| rewind.forget_upload(&forget, cx));
                                overlay.go_back(window, cx);
                            },
                        )),
                    ),
                ),
        )
    }
}
