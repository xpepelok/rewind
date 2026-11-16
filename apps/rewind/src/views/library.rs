use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Div, MouseButton, ObjectFit, Pixels,
    Point, SharedString, Window, div, img, prelude::*, px, uniform_list,
};
use rewind_config::{Gallery, GalleryKind, GallerySort};
use youtube::TaskState;

use crate::assets::{Icon, icon};
use crate::channels::text as yt;
use crate::gallery::{self, GalleryItem, format_duration, is_video};
use crate::motion::{self, DECELERATE, Key};
use crate::platform;
use crate::state::msg;
use crate::theme::{CONTROL_TEXT, GUTTER, ICON, PANEL_WIDTH, SMALL_TEXT, ThemeExt, alpha};
use crate::ui::{
    Avatar, ButtonKind, Dropdown, Tooltip, avatar, button, divider, hint, label, section, thumbnail,
};
use crate::views::editor::bulk_settings;
use crate::views::gallery_pick::card_probe;
use crate::views::overlay::{GalleryView, Overlay, Page};
use crate::views::published::Published;

pub const HOME_STRIP: &str = "thumb";
const SELECT_SIZE: f32 = 18.0;
const SELECT_RADIUS: f32 = 5.0;
const SELECT_TINT: f32 = 0.16;
const SELECT_ID_OFFSET: usize = 1 << 20;
const GRID_COLUMNS: usize = 2;
const CAPTION_LINE: f32 = 18.0;
const ALL_GAMES: &str = "Все игры";
const CARD_GAP: f32 = 12.0;
const MENU_WIDTH: f32 = 248.0;
const MENU_ITEM: f32 = 40.0;
const MENU_IN: Duration = Duration::from_millis(120);
const MENU_KEY: Key = Key::of("gallery.menu");
const PICKER_KEY: Key = Key::of("gallery.picker");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagePicker {
    Sort,
    Kind,
    Game,
    Microphone,
}

impl PagePicker {
    const fn id(self) -> &'static str {
        match self {
            Self::Sort => "gallery-sort",
            Self::Kind => "gallery-kind",
            Self::Game => "gallery-game",
            Self::Microphone => "mic-source",
        }
    }
}
const AVATAR: f32 = 36.0;
const HISTORY_LIMIT: usize = 50;
const SECONDS_PER_DAY: u64 = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Edit,
    Publish,
    PublishDelete,
    Copy,
    Share,
    Open,
    Reveal,
    Delete,
    BulkUpload,
    BulkUploadDelete,
    BulkShare,
    BulkDelete,
}

impl MenuAction {
    fn danger(self) -> bool {
        matches!(self, Self::Delete | Self::BulkDelete)
    }
}

fn menu_entries(menu: &ClipMenu) -> Vec<(MenuAction, Icon, SharedString)> {
    if menu.many.len() > 1 {
        let videos = menu.many.iter().filter(|path| is_video(path)).count();
        let pictures = menu.many.len() - videos;
        let mut entries = Vec::new();
        if videos > 0 {
            entries.push((
                MenuAction::BulkUpload,
                Icon::Upload,
                format!("Загрузить {videos} на YouTube").into(),
            ));
            entries.push((
                MenuAction::BulkUploadDelete,
                Icon::Upload,
                format!("Загрузить {videos} и удалить").into(),
            ));
        }
        if pictures > 0 {
            entries.push((
                MenuAction::BulkShare,
                Icon::Upload,
                format!(
                    "Опубликовать {pictures} {} одной ссылкой",
                    msg::shots_word(pictures)
                )
                .into(),
            ));
        }
        entries.push((
            MenuAction::BulkDelete,
            Icon::Delete,
            format!(
                "Удалить {} {}",
                menu.many.len(),
                msg::files_word(menu.many.len())
            )
            .into(),
        ));
        return entries;
    }
    let entries: &[(MenuAction, Icon, &str)] = if is_video(&menu.path) {
        &[
            (MenuAction::Edit, Icon::Edit, "Редактировать"),
            (MenuAction::Publish, Icon::Upload, "Опубликовать на YouTube"),
            (
                MenuAction::PublishDelete,
                Icon::Upload,
                "Опубликовать и удалить",
            ),
            (MenuAction::Open, Icon::Open, "Открыть"),
            (MenuAction::Reveal, Icon::Folder, "Показать в папке"),
            (MenuAction::Delete, Icon::Delete, "Удалить"),
        ]
    } else {
        &[
            (MenuAction::Edit, Icon::Edit, "Редактировать"),
            (MenuAction::Copy, Icon::Copy, "Копировать"),
            (MenuAction::Share, Icon::Upload, "Опубликовать"),
            (MenuAction::Open, Icon::Open, "Открыть"),
            (MenuAction::Reveal, Icon::Folder, "Показать в папке"),
            (MenuAction::Delete, Icon::Delete, "Удалить"),
        ]
    };
    entries
        .iter()
        .map(|(action, glyph, text)| (*action, *glyph, SharedString::from(*text)))
        .collect()
}

pub struct ClipMenu {
    pub path: PathBuf,
    pub many: Vec<PathBuf>,
    pub at: Point<Pixels>,
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim_end_matches(".DVR").to_owned())
        .unwrap_or_default()
}

fn age(modified: SystemTime) -> String {
    let days = SystemTime::now()
        .duration_since(modified)
        .map_or(0, |elapsed| elapsed.as_secs() / SECONDS_PER_DAY);
    match days {
        0 => "Сегодня".to_owned(),
        1 => "Вчера".to_owned(),
        _ => {
            let seconds = modified
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64);
            youtube::iso_date(seconds)
        }
    }
}

impl Overlay {
    pub fn open_menu(
        &mut self,
        path: PathBuf,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let many = if self.selected.contains(&path) && self.selected.len() > 1 {
            self.selected.clone()
        } else {
            Vec::new()
        };
        self.menu = Some(ClipMenu { path, many, at });
        motion::stop(MENU_KEY, window, cx);
        motion::drive(MENU_KEY, true, MENU_IN, DECELERATE, window, cx);
        cx.notify();
    }

    pub fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            motion::stop(MENU_KEY, window, cx);
            cx.notify();
        }
    }

    pub fn edit_clip(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        self.open_viewer(path, window, cx);
    }

    fn run_menu(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        match action {
            MenuAction::Edit if !is_video(&menu.path) => {
                self.open_photo(menu.path, true, window, cx)
            }
            MenuAction::Edit | MenuAction::Publish => self.edit_clip(menu.path, window, cx),
            MenuAction::PublishDelete => self.publish_and_delete(menu.path, window, cx),
            MenuAction::BulkUpload => self.upload_paths(menu.many, false, window, cx),
            MenuAction::BulkUploadDelete => self.upload_paths(menu.many, true, window, cx),
            MenuAction::BulkShare => {
                let pictures: Vec<PathBuf> = menu
                    .many
                    .into_iter()
                    .filter(|path| !is_video(path))
                    .collect();
                self.selected.clear();
                self.state
                    .update(cx, |rewind, cx| rewind.publish_album(pictures, cx));
            }
            MenuAction::BulkDelete => self.confirm_delete = Some(menu.many),
            MenuAction::Copy => {
                let path = menu.path;
                self.state
                    .update(cx, |rewind, cx| rewind.copy_image(path, None, cx));
            }
            MenuAction::Share => {
                let path = menu.path;
                self.state
                    .update(cx, |rewind, cx| rewind.publish_image(path, None, cx));
            }
            MenuAction::Open => platform::shell::open(&menu.path),
            MenuAction::Reveal => platform::shell::select(&menu.path),
            MenuAction::Delete => self.confirm_delete = Some(vec![menu.path]),
        }
        cx.notify();
    }

    pub fn clip_card(
        &self,
        id: (&'static str, usize),
        item: &GalleryItem,
        size: (f32, f32),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = cx.palette();
        let edit_path = item.path.clone();
        let menu_path = item.path.clone();
        let pick_path = item.path.clone();
        let video = is_video(&item.path);
        let missing_preview = item.preview.is_none();
        let home = id.0 == HOME_STRIP;
        let selected = !home && self.selected.contains(&item.path);
        let choosing = !home && !self.selected.is_empty();
        let hovered = gpui::SharedString::from(format!("{}-{}", id.0, id.1));
        let hovered_mark = hovered.clone();
        div()
            .id(id)
            .group(hovered)
            .relative()
            .flex_none()
            .w(px(size.0))
            .h(px(size.1))
            .bg(if missing_preview && p.soft {
                p.track
            } else {
                p.header
            })
            .overflow_hidden()
            .cursor_pointer()
            .border_2()
            .border_color(if selected {
                p.accent
            } else if missing_preview && p.soft {
                p.field_line
            } else {
                p.panel
            })
            .hover(|style| style.border_color(p.accent))
            .on_click(cx.listener(move |overlay, event: &ClickEvent, window, cx| {
                if !home && overlay.pick_click(&edit_path, event.modifiers(), cx) {
                    return;
                }
                overlay.viewer_from_home = home;
                if video {
                    overlay.edit_clip(edit_path.clone(), window, cx);
                } else {
                    overlay.open_photo(edit_path.clone(), false, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |overlay, event: &gpui::MouseDownEvent, window, cx| {
                    overlay.viewer_from_home = home;
                    overlay.open_menu(menu_path.clone(), event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .when_some(item.preview.clone(), |card, preview| {
                card.child(img(preview).size_full().object_fit(ObjectFit::Cover))
            })
            .when(missing_preview && p.soft, |card| {
                card.flex().items_center().justify_center().child(icon(
                    if video { Icon::Videocam } else { Icon::Photo },
                    ICON,
                    p.icon,
                ))
            })
            .when(!home, |card| {
                card.child(card_probe(
                    self.picking.cards.clone(),
                    self.picking.scroll(),
                    item.path.clone(),
                ))
            })
            .when(selected, |card| {
                card.child(div().absolute().inset_0().bg(alpha(p.accent, SELECT_TINT)))
            })
            .when(!home, |card| {
                card.child(
                    div()
                        .id((id.0, id.1 + SELECT_ID_OFFSET))
                        .absolute()
                        .top(px(8.0))
                        .left(px(8.0))
                        .size(px(SELECT_SIZE))
                        .rounded(px(SELECT_RADIUS))
                        .border_1()
                        .border_color(if selected {
                            p.accent
                        } else if p.soft {
                            p.white
                        } else {
                            p.title
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if selected {
                            p.accent
                        } else {
                            alpha(p.stage, 0.6)
                        })
                        .opacity(if selected || choosing { 1.0 } else { 0.0 })
                        .group_hover(hovered_mark, |mark| mark.opacity(1.0))
                        .when(selected, |mark| {
                            mark.child(icon(
                                Icon::Check,
                                13.0,
                                if p.soft { p.check_mark } else { p.title },
                            ))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |overlay, _, _, cx| {
                            cx.stop_propagation();
                            overlay.toggle_picked(&pick_path, cx);
                        })),
                )
            })
            .when_some(item.duration, |card, duration| {
                card.child(
                    div()
                        .absolute()
                        .bottom(px(4.0))
                        .right(px(4.0))
                        .px(px(4.0))
                        .bg(p.badge)
                        .text_size(px(SMALL_TEXT))
                        .text_color(p.white)
                        .child(format_duration(duration)),
                )
            })
    }

    fn upload_selected(&mut self, delete: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected.clone();
        self.upload_paths(paths, delete, window, cx);
    }

    fn upload_paths(
        &mut self,
        paths: Vec<PathBuf>,
        delete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rewind = self.state.read(cx);
        let Some(account) = rewind
            .youtube
            .accounts
            .active()
            .map(|account| account.id.clone())
        else {
            self.push(Page::YouTube, window, cx);
            return;
        };
        let specs = rewind.settings.youtube.specs.clone();
        let items: Vec<_> = paths
            .iter()
            .filter(|path| is_video(path))
            .map(|path| {
                let mut settings = bulk_settings(path, &specs);
                settings.delete_source = delete.then(|| path.clone());
                (settings, path.clone())
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.selected.clear();
        self.state
            .update(cx, |rewind, cx| rewind.publish_many(account, items, cx));
        cx.notify();
    }

    fn selection_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.selected.is_empty() {
            return None;
        }
        let p = cx.palette();
        let videos = self.selected.iter().filter(|path| is_video(path)).count();
        let pictures = self.selected.len() - videos;
        let host = self.state.read(cx).settings.sharing.host.label();
        let signed_in = self.state.read(cx).youtube.accounts.active().is_some();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .px(px(GUTTER))
                .py(px(12.0))
                .border_t_1()
                .border_color(p.divider)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.0))
                        .child(label(format!("Выбрано: {}", self.selected.len()), p.text).flex_1())
                        .when(!self.everything_selected(), |row| {
                            row.child(
                                div()
                                    .id("selection-all")
                                    .cursor_pointer()
                                    .child(label("Выбрать все (Ctrl+A)", p.accent))
                                    .on_click(
                                        cx.listener(|overlay, _, _, cx| overlay.select_all(cx)),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .id("selection-clear")
                                .cursor_pointer()
                                .child(label("Снять (Esc)", p.accent))
                                .on_click(cx.listener(|overlay, _, _, cx| {
                                    overlay.clear_selection(cx);
                                })),
                        ),
                )
                .when(videos > 0, |bar| {
                    bar.child(
                    div()
                        .flex()
                        .gap(px(10.0))
                        .child(
                            button(
                                "selection-merge",
                                "Склеить",
                                ButtonKind::Secondary,
                                videos >= 2,
                                &p,
                            )
                            .flex_1()
                            .when(videos >= 2, |button| {
                                button.on_click(cx.listener(|overlay, _, _, cx| {
                                    let clips: Vec<PathBuf> = overlay
                                        .selected
                                        .iter()
                                        .filter(|path| is_video(path))
                                        .cloned()
                                        .collect();
                                    overlay.selected.clear();
                                    overlay
                                        .state
                                        .update(cx, |rewind, cx| rewind.merge_clips(clips, cx));
                                    cx.notify();
                                }))
                            }),
                        )
                        .child(
                            button(
                                "selection-upload",
                                if signed_in {
                                    "Загрузить на YouTube"
                                } else {
                                    "Войти в YouTube"
                                },
                                ButtonKind::Primary,
                                videos >= 1,
                                &p,
                            )
                            .flex_1()
                            .when(videos >= 1, |button| {
                                button.on_click(cx.listener(|overlay, _, window, cx| {
                                    overlay.upload_selected(false, window, cx)
                                }))
                            }),
                        ),
                )
                })
                .when(videos >= 1 && signed_in, |bar| {
                    bar.child(
                        button(
                            "selection-upload-delete",
                            format!("Загрузить {videos} и удалить с компьютера"),
                            ButtonKind::Secondary,
                            true,
                            &p,
                        )
                        .on_click(cx.listener(|overlay, _, window, cx| {
                            overlay.upload_selected(true, window, cx)
                        })),
                    )
                })
                .when(pictures > 0, |bar| {
                    bar.child(
                        button(
                            "selection-share",
                            if pictures == 1 {
                                format!("Опубликовать снимок на {host}")
                            } else {
                                format!(
                                    "Опубликовать {pictures} {} одной ссылкой",
                                    msg::shots_word(pictures)
                                )
                            },
                            ButtonKind::Primary,
                            true,
                            &p,
                        )
                        .on_click(cx.listener(|overlay, _, _, cx| {
                            let pictures: Vec<PathBuf> = overlay
                                .selected
                                .iter()
                                .filter(|path| !is_video(path))
                                .cloned()
                                .collect();
                            overlay.selected.clear();
                            overlay
                                .state
                                .update(cx, |rewind, cx| rewind.publish_album(pictures, cx));
                            cx.notify();
                        })),
                    )
                })
                .child(
                    button(
                        "selection-delete",
                        format!(
                            "Удалить {} {}",
                            self.selected.len(),
                            msg::files_word(self.selected.len())
                        ),
                        ButtonKind::Danger,
                        true,
                        &p,
                    )
                    .on_click(cx.listener(|overlay, _, _, cx| {
                        overlay.confirm_delete = Some(overlay.selected.clone());
                        cx.notify();
                    })),
                )
                .child(hint(
                    "Видео: название из имени файла, описание по умолчанию, загружаются одновременно. Снимки публикуются одним альбомом — ссылка сразу копируется.",
                    &p,
                )),
        )
    }

    fn change_gallery(&mut self, apply: impl FnOnce(&mut Gallery), cx: &mut Context<Self>) {
        self.state
            .update(cx, |rewind, cx| rewind.change_gallery_view(apply, cx));
        cx.notify();
    }

    fn gallery_cell(
        &self,
        index: usize,
        item: &GalleryItem,
        (width, height): (f32, f32),
        cx: &mut Context<Self>,
    ) -> Div {
        let p = cx.palette();
        let size = gallery::format_size(item.size);
        let subtitle = match item.game.as_str() {
            "" => format!("{} · {size}", age(item.modified)),
            game => format!("{game} · {} · {size}", age(item.modified)),
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .w(px(width))
            .child(self.clip_card(("gallery-card", index), item, (width, height), cx))
            .child(
                div()
                    .h(px(CAPTION_LINE))
                    .truncate()
                    .text_size(px(SMALL_TEXT))
                    .text_color(p.text)
                    .child(file_title(&item.path)),
            )
            .child(
                div()
                    .h(px(CAPTION_LINE))
                    .truncate()
                    .text_size(px(SMALL_TEXT))
                    .text_color(p.muted)
                    .child(subtitle),
            )
    }

    pub fn arranged_gallery(&mut self, cx: &App) -> GalleryView {
        let rewind = self.state.read(cx);
        let all = &rewind.gallery;
        let key = (
            rewind.gallery_generation,
            all.len(),
            rewind.settings.gallery.clone(),
        );
        self.gallery_view
            .get(key, None, Instant::now(), |(_, _, view)| {
                let options = std::iter::once(None)
                    .chain(gallery::games(all).into_iter().map(Some))
                    .collect();
                (Rc::new(gallery::arrange(all, view)), Rc::new(options))
            })
            .clone()
    }

    pub fn gallery_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let (shown, options) = self.arranged_gallery(cx);
        let rewind = self.state.read(cx);
        let view = rewind.settings.gallery.clone();
        let total = rewind.gallery.len();
        let gallery_dir = rewind.gallery_dir();
        if self.picking.set_order(&shown) && !self.selected.is_empty() {
            let order: std::collections::HashSet<&PathBuf> = self.picking.order().iter().collect();
            self.selected.retain(|path| order.contains(path));
        }
        let width = (PANEL_WIDTH - GUTTER * 2.0 - CARD_GAP) / GRID_COLUMNS as f32;
        let height = width * 9.0 / 16.0;
        let rows = shown.len().div_ceil(GRID_COLUMNS);
        let counter = format!("Показано {} из {total}", shown.len());
        let list = uniform_list(
            "gallery-grid",
            rows,
            cx.processor(move |overlay, range: Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let first = row * GRID_COLUMNS;
                        let last = (first + GRID_COLUMNS).min(shown.len());
                        div()
                            .flex()
                            .gap(px(CARD_GAP))
                            .px(px(GUTTER))
                            .pb(px(CARD_GAP))
                            .children((first..last).map(|index| {
                                overlay.gallery_cell(index, &shown[index], (width, height), cx)
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.picking.list.clone())
        .size_full();
        let list = div()
            .id("gallery-pick")
            .relative()
            .flex_1()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|overlay, event: &gpui::MouseDownEvent, _, cx| {
                    overlay.pick_down(event, cx)
                }),
            )
            .child(list)
            .children(self.marquee_layer(cx))
            .children(self.pick_listeners(cx))
            .child(crate::ui::scrollbar::Scrollbar::new(
                "gallery-scrollbar",
                self.picking.scroll(),
            ));
        let sort_index = GallerySort::ALL
            .iter()
            .position(|sort| *sort == view.sort)
            .unwrap_or(0);
        let sort = self.gallery_dropdown(
            PagePicker::Sort,
            GallerySort::ALL
                .iter()
                .map(|sort| sort.label().into())
                .collect(),
            sort_index,
            |view, index| view.sort = GallerySort::ALL[index],
            cx,
        );
        let kind_index = GalleryKind::ALL
            .iter()
            .position(|kind| *kind == view.kind)
            .unwrap_or(0);
        let kind = self.gallery_dropdown(
            PagePicker::Kind,
            GalleryKind::ALL
                .iter()
                .map(|kind| kind.label().into())
                .collect(),
            kind_index,
            |view, index| view.kind = GalleryKind::ALL[index],
            cx,
        );
        let game_index = options
            .iter()
            .position(|option| *option == view.game)
            .unwrap_or(0);
        let game_names = options
            .iter()
            .map(|option| {
                SharedString::from(option.clone().unwrap_or_else(|| ALL_GAMES.to_owned()))
            })
            .collect();
        let game = self.gallery_dropdown(
            PagePicker::Game,
            game_names,
            game_index,
            move |view, index| view.game = options.get(index).cloned().flatten(),
            cx,
        );
        let filter_row = |title: &'static str, control: Dropdown| {
            div()
                .flex()
                .items_center()
                .px(px(GUTTER))
                .py(px(2.0))
                .child(label(title, p.text).flex_1())
                .child(control)
        };
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("gallery-folder")
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .px(px(GUTTER))
                    .py(px(14.0))
                    .cursor_pointer()
                    .child(icon(Icon::Folder, ICON, p.icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(label("Открыть папку галереи", p.text))
                            .child(hint(gallery_dir.display().to_string(), &p)),
                    )
                    .on_click(move |_, _, _| platform::shell::reveal(&gallery_dir)),
            )
            .child(filter_row("Сортировка", sort))
            .child(filter_row("Показывать", kind))
            .child(filter_row("Игра", game))
            .child(
                div()
                    .px(px(GUTTER))
                    .pt(px(6.0))
                    .pb(px(10.0))
                    .child(hint(counter, &p)),
            )
            .when(total == 0, |page| {
                page.child(
                    div()
                        .px(px(GUTTER))
                        .child(hint("Здесь появятся ваши клипы и снимки экрана.", &p)),
                )
            })
            .when(total > 0 && rows == 0, |page| {
                page.child(
                    div()
                        .px(px(GUTTER))
                        .child(hint("Под эти фильтры ничего не подходит.", &p)),
                )
            })
            .child(list)
            .children(self.selection_bar(cx))
            .into_any_element()
    }

    fn gallery_dropdown(
        &self,
        picker: PagePicker,
        names: Vec<SharedString>,
        selected: usize,
        choose: impl Fn(&mut Gallery, usize) + 'static,
        cx: &mut Context<Self>,
    ) -> Dropdown {
        self.page_dropdown(
            picker,
            names,
            selected,
            move |overlay, index, cx| overlay.change_gallery(|view| choose(view, index), cx),
            cx,
        )
    }

    pub fn page_dropdown(
        &self,
        picker: PagePicker,
        names: Vec<SharedString>,
        selected: usize,
        choose: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Dropdown {
        let entity = cx.entity();
        let shown = (self.page_picker == Some(picker)).then(|| motion::value(PICKER_KEY, cx));
        Dropdown::new(picker.id(), names, selected)
            .shown(shown)
            .on_toggle({
                let entity = entity.clone();
                move |window, cx| {
                    entity.update(cx, |overlay, cx| {
                        overlay.toggle_page_picker(picker, window, cx)
                    })
                }
            })
            .on_select(move |index, window, cx| {
                entity.update(cx, |overlay, cx| {
                    choose(overlay, index, cx);
                    overlay.close_page_picker(window, cx);
                })
            })
    }

    fn toggle_page_picker(
        &mut self,
        picker: PagePicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        motion::stop(PICKER_KEY, window, cx);
        if self.page_picker == Some(picker) {
            self.page_picker = None;
        } else {
            self.page_picker = Some(picker);
            motion::drive(PICKER_KEY, true, MENU_IN, DECELERATE, window, cx);
        }
        cx.notify();
    }

    pub fn close_page_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        motion::stop(PICKER_KEY, window, cx);
        self.page_picker = None;
        cx.notify();
    }

    pub fn menu_layer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = cx.palette();
        let menu = self.menu.as_ref()?;
        let shown = motion::value(MENU_KEY, cx);
        let viewport = window.viewport_size();
        let entries = menu_entries(menu);
        let height = MENU_ITEM * entries.len() as f32 + 8.0;
        let left = f32::from(menu.at.x).min(f32::from(viewport.width) - MENU_WIDTH - 8.0);
        let top = f32::from(menu.at.y).min(f32::from(viewport.height) - height - 8.0);
        let items = entries.into_iter().map(|(action, glyph, text)| {
            let danger = action.danger();
            div()
                .id(("menu-item", action as usize))
                .h(px(MENU_ITEM))
                .flex()
                .items_center()
                .gap(px(14.0))
                .px(px(14.0))
                .cursor_pointer()
                .hover(|style| style.bg(p.dots))
                .child(icon(glyph, 20.0, if danger { p.record } else { p.icon }))
                .child(
                    div()
                        .text_size(px(CONTROL_TEXT))
                        .text_color(if danger { p.record } else { p.text })
                        .child(text),
                )
                .on_click(
                    cx.listener(move |overlay, _, window, cx| overlay.run_menu(action, window, cx)),
                )
        });
        Some(
            div()
                .id("menu-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|overlay, _, window, cx| overlay.close_menu(window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|overlay, _, window, cx| overlay.close_menu(window, cx)),
                )
                .child(
                    div()
                        .id("menu")
                        .absolute()
                        .left(px(left))
                        .top(px(top - 6.0 * (1.0 - shown)))
                        .w(px(MENU_WIDTH))
                        .py(px(4.0))
                        .opacity(shown)
                        .bg(p.header)
                        .shadow(p.toast_shadow())
                        .occlude()
                        .children(items),
                )
                .into_any_element(),
        )
    }

    pub fn delete_layer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = cx.palette();
        let paths = self.confirm_delete.clone()?;
        let (title, name) = match paths.as_slice() {
            [single] => (
                if is_video(single) {
                    "Удалить клип?".to_owned()
                } else {
                    "Удалить снимок?".to_owned()
                },
                format!("{} — попадёт в Корзину", file_title(single)),
            ),
            many => (
                format!("Удалить {} {}?", many.len(), msg::files_word(many.len())),
                "Файлы попадут в Корзину, их можно будет восстановить.".to_owned(),
            ),
        };
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
                        .child(label(title, p.text))
                        .child(hint(name, &p))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(10.0))
                                .child(
                                    button(
                                        "delete-cancel",
                                        "Отмена",
                                        ButtonKind::Secondary,
                                        true,
                                        &p,
                                    )
                                    .on_click(cx.listener(
                                        |overlay, _, _, cx| {
                                            overlay.confirm_delete = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    button(
                                        "delete-confirm",
                                        "Удалить",
                                        ButtonKind::Danger,
                                        true,
                                        &p,
                                    )
                                    .on_click(cx.listener(
                                        move |overlay, _, window, cx| {
                                            overlay.confirm_delete = None;
                                            let paths = paths.clone();
                                            overlay.selected.retain(|path| !paths.contains(path));
                                            let gone: Vec<PathBuf> = paths
                                                .iter()
                                                .filter(|path| {
                                                    !overlay.state.read(cx).uploading(path)
                                                })
                                                .cloned()
                                                .collect();
                                            overlay.replace_viewed(&gone, window, cx);
                                            overlay.state.update(cx, |rewind, cx| {
                                                rewind.delete_clips(paths, cx)
                                            });
                                        },
                                    )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    pub fn youtube_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let channels = &rewind.youtube;
        let supported = channels.supported;
        let signing_in = channels.signing_in;
        let active = channels.accounts.active().map(|account| account.id.clone());
        let accounts = channels.accounts.accounts.clone();
        let tasks: Vec<youtube::Task> = channels.queue.visible().into_iter().cloned().collect();
        let key = (
            channels.generation,
            rewind.gallery_generation,
            rewind.gallery.len(),
            channels.history.entries.len(),
        );
        let history = self
            .history_rows
            .get(key, None, Instant::now(), |_| {
                let mut entries: Vec<&youtube::HistoryEntry> =
                    channels.history.entries.iter().collect();
                entries.sort_by_key(|entry| std::cmp::Reverse(entry.uploaded_at));
                entries.truncate(HISTORY_LIMIT);
                Rc::new(
                    entries
                        .into_iter()
                        .map(|entry| {
                            (
                                entry.clone(),
                                channels.thumbnail(&entry.video_id),
                                rewind.has_local_video(&entry.video_id),
                            )
                        })
                        .collect(),
                )
            })
            .clone();
        let fetches: Vec<Option<u32>> = history
            .iter()
            .map(|(entry, _, _)| channels.fetch_progress(&entry.video_id))
            .collect();
        let names: Vec<(String, String)> = accounts
            .iter()
            .map(|account| (account.id.clone(), account.display_name()))
            .collect();
        let name_of = |id: &str| {
            names
                .iter()
                .find(|(other, _)| other == id)
                .map_or_else(|| id.to_owned(), |(_, name)| name.clone())
        };

        let mut page = div()
            .id("youtube-scroll")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.youtube_scroll)
            .pb(px(GUTTER))
            .child(section("Аккаунты", &p));
        if !supported {
            page = page.child(
                div()
                    .px(px(GUTTER))
                    .child(hint("Для публикации нужен Chrome, Chromium или Edge.", &p)),
            );
        }
        for (index, account) in accounts.iter().enumerate() {
            let id = account.id.clone();
            let remove_id = account.id.clone();
            let is_active = active.as_deref() == Some(account.id.as_str());
            let subtitle = if account.needs_reauth {
                "Нужно войти заново".to_owned()
            } else if account.handle.is_empty() {
                account.id.clone()
            } else {
                account.handle.clone()
            };
            page = page.child(
                div()
                    .id(("account", index))
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .px(px(GUTTER))
                    .py(px(10.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(avatar(&Avatar::of(account), AVATAR, &p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(label(account.display_name(), p.text))
                            .child(hint(subtitle, &p)),
                    )
                    .when(is_active, |row| {
                        row.child(icon(Icon::Check, ICON, p.accent))
                    })
                    .child(
                        div()
                            .id(("account-remove", index))
                            .p(px(4.0))
                            .child(icon(Icon::Close, 20.0, p.muted))
                            .on_click(cx.listener(move |overlay, _, _, cx| {
                                cx.stop_propagation();
                                overlay.state.update(cx, |rewind, cx| {
                                    rewind.remove_youtube_account(&remove_id, cx)
                                });
                            })),
                    )
                    .on_click(cx.listener(move |overlay, _, _, cx| {
                        overlay
                            .state
                            .update(cx, |rewind, cx| rewind.select_youtube_account(&id, cx));
                    })),
            );
        }
        page = page.child(
            div()
                .px(px(GUTTER))
                .pt(px(10.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .when(signing_in, |block| {
                    block.child(hint("Войдите в аккаунт в открывшемся окне браузера…", &p))
                })
                .child(
                    button(
                        "youtube-add",
                        "Добавить аккаунт",
                        ButtonKind::Primary,
                        supported && !signing_in,
                        &p,
                    )
                    .when(supported && !signing_in, |button| {
                        button.on_click(cx.listener(|overlay, _, window, cx| {
                            let placement = crate::views::overlay::sign_in_placement(window);
                            overlay.state.update(cx, |rewind, cx| {
                                rewind.add_youtube_account(placement, cx)
                            })
                        }))
                    }),
                ),
        );

        page = page.child(divider(&p)).child(self.specs_section(cx));

        if !tasks.is_empty() {
            page = page.child(divider(&p)).child(section("Загрузки", &p));
            for (index, task) in tasks.iter().enumerate() {
                let id = task.id.clone();
                let running = matches!(task.state, TaskState::Running);
                let retryable = task.state.is_retryable();
                let status = if running {
                    yt::stage(task.stage, task.percent)
                } else {
                    task.failure_note().map_or_else(
                        || yt::task(&task.state).to_owned(),
                        |note| note.detail.clone(),
                    )
                };
                let (action, action_label) = if retryable {
                    (true, "Повторить")
                } else {
                    (false, "Отменить")
                };
                let show_action = task.state.is_active() || retryable;
                page = page.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(GUTTER))
                        .py(px(8.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(label(task.title().to_owned(), p.text)),
                                )
                                .when(show_action, |row| {
                                    row.child(
                                        div()
                                            .id(("task-action", index))
                                            .cursor_pointer()
                                            .text_size(px(SMALL_TEXT))
                                            .text_color(p.accent)
                                            .child(action_label)
                                            .on_click(cx.listener(move |overlay, _, _, cx| {
                                                let id = id.clone();
                                                overlay.state.update(cx, |rewind, cx| {
                                                    if action {
                                                        rewind.retry_upload(&id, cx);
                                                    } else {
                                                        rewind.cancel_upload(&id, cx);
                                                    }
                                                });
                                            })),
                                    )
                                }),
                        )
                        .child(hint(
                            format!("{} · {status}", name_of(&task.account_id)),
                            &p,
                        ))
                        .when(running, |block| {
                            block.child(
                                div().h(px(4.0)).w_full().bg(p.track).child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(task.fraction()))
                                        .bg(p.accent),
                                ),
                            )
                        }),
                );
            }
        }

        page = page
            .child(divider(&p))
            .child(section("История публикаций", &p));
        if history.is_empty() {
            page = page.child(
                div()
                    .px(px(GUTTER))
                    .child(hint("Пока ничего не опубликовано.", &p)),
            );
        }
        for (index, ((entry, preview, local), fetching)) in history.iter().zip(fetches).enumerate()
        {
            let (preview, local) = (preview.clone(), *local);
            let download_id = entry.video_id.clone();
            let url = entry.url();
            let link = url.clone();
            let video_id = entry.video_id.clone();
            let cancel_id = entry.video_id.clone();
            let downloading = fetching.is_some();
            let fetching = fetching
                .map(|percent| format!(" · скачивается с YouTube {percent}%"))
                .unwrap_or_default();
            page = page.child(
                div()
                    .id(("history", index))
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .px(px(GUTTER))
                    .py(px(10.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(thumbnail(preview, Icon::Play, &p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(label(entry.title.clone(), p.text))
                            .child(hint(
                                format!(
                                    "{} · {} · {} · {} · {}{fetching}",
                                    name_of(&entry.account_id),
                                    youtube::iso_date(entry.uploaded_at),
                                    gallery::format_size(entry.bytes),
                                    yt::privacy(entry.privacy),
                                    yt::status(&entry.status)
                                ),
                                &p,
                            )),
                    )
                    .child(
                        div()
                            .id(("history-copy", index))
                            .p(px(4.0))
                            .rounded(px(4.0))
                            .hover(|style| style.bg(p.hover))
                            .tooltip(Tooltip::text("Копировать ссылку"))
                            .child(icon(Icon::Copy, 20.0, p.icon))
                            .on_click(cx.listener(move |overlay, _, _, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                                let detail = link.clone();
                                overlay.state.update(cx, |rewind, cx| {
                                    rewind.toast(
                                        Icon::Copy,
                                        "Ссылка скопирована",
                                        Some(detail),
                                        false,
                                        cx,
                                    )
                                });
                            })),
                    )
                    .when(downloading, |row| {
                        row.child(
                            div()
                                .id(("history-cancel", index))
                                .p(px(4.0))
                                .rounded(px(4.0))
                                .hover(|style| style.bg(p.hover))
                                .tooltip(Tooltip::text("Отменить скачивание"))
                                .child(icon(Icon::Close, 20.0, p.icon))
                                .on_click(cx.listener(move |overlay, _, _, cx| {
                                    cx.stop_propagation();
                                    overlay.state.update(cx, |rewind, cx| {
                                        rewind.cancel_fetch(&cancel_id, cx)
                                    });
                                })),
                        )
                    })
                    .when(!downloading && !local, |row| {
                        row.child(
                            div()
                                .id(("history-download", index))
                                .p(px(4.0))
                                .rounded(px(4.0))
                                .hover(|style| style.bg(p.hover))
                                .tooltip(Tooltip::text("Скачать в 360p, чтобы смотреть здесь"))
                                .child(icon(Icon::Save, 20.0, p.icon))
                                .on_click(cx.listener(move |overlay, _, window, cx| {
                                    cx.stop_propagation();
                                    let local = overlay.state.update(cx, |rewind, cx| {
                                        rewind.watch_video(&download_id, cx)
                                    });
                                    if let Some(path) = local {
                                        overlay.open_viewer(path, window, cx);
                                    }
                                })),
                        )
                    })
                    .child(
                        div()
                            .id(("history-open", index))
                            .p(px(4.0))
                            .rounded(px(4.0))
                            .hover(|style| style.bg(p.hover))
                            .tooltip(Tooltip::text("Открыть на YouTube"))
                            .child(icon(Icon::Open, 20.0, p.icon))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                platform::shell::open_url(&url);
                            }),
                    )
                    .child(icon(Icon::ChevronRight, ICON, p.text))
                    .on_click(cx.listener(move |overlay, _, window, cx| {
                        overlay.open_published(Published::Video(video_id.clone()), window, cx)
                    })),
            );
        }
        page.into_any_element()
    }

    pub fn update_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::update::{CURRENT, Stage};
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let stage = rewind.update_stage.clone();
        let installed = crate::platform::system::running_installed_copy();
        let status = match &rewind.release {
            Some(release) => format!("Доступна версия {}", release.version),
            None => "Установлена последняя версия".to_owned(),
        };
        let available = rewind.release.is_some();
        let busy = stage == Stage::Downloading;
        let note = if installed {
            "Rewind скачает новую версию со страницы релизов на GitHub, установит её и перезапустится. Идущая запись и загрузки на YouTube будут прерваны."
        } else {
            "Rewind запущен без установки, поэтому обновление устанавливается вручную: откроется страница релизов на GitHub."
        };
        let text = if installed { "Обновить" } else { "Открыть страницу релизов" };
        div()
            .id("update-scroll")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .pb(px(GUTTER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .px(px(GUTTER))
                    .py(px(16.0))
                    .child(icon(
                        if available { Icon::Bell } else { Icon::Check },
                        ICON,
                        p.accent,
                    ))
                    .child(label(status, p.text)),
            )
            .child(divider(&p))
            .child(
                div()
                    .px(px(GUTTER))
                    .pt(px(12.0))
                    .child(hint(format!("Установленная версия: {CURRENT}"), &p)),
            )
            .when(available, |page| {
                page.child(div().px(px(GUTTER)).pt(px(10.0)).child(hint(note, &p)))
                    .child(
                        div()
                            .px(px(GUTTER))
                            .pt(px(18.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .when(busy, |block| {
                                block.child(hint("Скачивание и установка обновления…", &p))
                            })
                            .when_some(
                                match &stage {
                                    Stage::Failed(error) => Some(error.clone()),
                                    _ => None,
                                },
                                |block, error| {
                                    block.child(hint(
                                        format!("Не удалось обновить: {error}"),
                                        &p,
                                    ))
                                },
                            )
                            .child(
                                button("update-install", text, ButtonKind::Primary, !busy, &p)
                                    .w_full()
                                    .when(!busy, |button| {
                                        button.on_click(cx.listener(|overlay, _, _, cx| {
                                            overlay
                                                .state
                                                .update(cx, |rewind, cx| rewind.install_update(cx))
                                        }))
                                    }),
                            ),
                    )
            })
            .into_any_element()
    }

    pub fn replace_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let replaced = rewind.replaced;
        let busy = rewind.replacement_busy;
        let status = if replaced {
            "Rewind запускается вместо NVIDIA ShadowPlay"
        } else {
            "Сейчас работает NVIDIA ShadowPlay"
        };
        let steps = [
            "Отключает службу NVIDIA LocalSystem Container, которая запускает оверлей NVIDIA, ShadowPlay Helper и NVIDIA App.",
            "Выключает самообновление NVIDIA App, чтобы служба не включилась снова.",
            "Перенаправляет запуск NVIDIA Overlay и NVIDIA App на Rewind.",
            "Добавляет Rewind в автозапуск и в список установленных программ.",
        ];
        let kept = "Драйвер, панель управления NVIDIA и NVIDIA Broadcast не затрагиваются. Ничего не удаляется: «Вернуть NVIDIA» восстанавливает все исходные настройки.";
        let (text, kind) = if replaced {
            ("Вернуть NVIDIA ShadowPlay", ButtonKind::Danger)
        } else {
            ("Заменить NVIDIA ShadowPlay", ButtonKind::Primary)
        };
        div()
            .id("replace-scroll")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .pb(px(GUTTER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .px(px(GUTTER))
                    .py(px(16.0))
                    .child(icon(
                        if replaced { Icon::Check } else { Icon::Monitor },
                        ICON,
                        if replaced { p.accent } else { p.icon },
                    ))
                    .child(label(status, p.text)),
            )
            .child(divider(&p))
            .child(section("Что происходит", &p))
            .children(steps.iter().map(|step| {
                div()
                    .flex()
                    .gap(px(12.0))
                    .px(px(GUTTER))
                    .py(px(5.0))
                    .child(
                        div()
                            .flex_none()
                            .mt(px(7.0))
                            .size(px(6.0))
                            .rounded_full()
                            .bg(p.accent),
                    )
                    .child(div().flex_1().min_w_0().child(hint(*step, &p)))
            }))
            .child(div().px(px(GUTTER)).pt(px(10.0)).child(hint(kept, &p)))
            .child(
                div()
                    .px(px(GUTTER))
                    .pt(px(18.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .when(busy, |block| {
                        block.child(hint("Подтвердите запрос администратора Windows…", &p))
                    })
                    .child(
                        button("replace-toggle", text, kind, !busy, &p)
                            .w_full()
                            .when(!busy, |button| {
                                button.on_click(cx.listener(move |overlay, _, _, cx| {
                                    overlay.state.update(cx, |rewind, cx| {
                                        rewind.set_replacement(!replaced, cx)
                                    })
                                }))
                            }),
                    ),
            )
            .into_any_element()
    }

    pub fn youtube_entry(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        self.link(
            "youtube",
            Icon::YouTube,
            "YouTube",
            None,
            Icon::ChevronRight,
            cx,
        )
        .on_click(cx.listener(|overlay, _, window, cx| overlay.push(Page::YouTube, window, cx)))
    }
}
