use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, Point, Render,
    RenderImage, ScrollEvent, Subscription, Window, anchored, deferred, div, img, prelude::*, px,
};
use image::{ImageFormat, RgbaImage};

use rewind_media::{Area, Mark};

use crate::assets::{Icon, icon};
use crate::platform;
use crate::state::Rewind;
use crate::theme::{
    CONTROL_TEXT, FONT_FAMILY, GUTTER, HEADER_HEIGHT, ICON, PAGE_TITLE_TEXT, Palette, SMALL_TEXT,
    ThemeExt,
};
use crate::ui::{ButtonKind, button, strong};
use crate::views::annotate::{self, Annotate, Board, Tool, compose, render_image, separator};

const MENU_WIDTH: f32 = 220.0;
const MENU_ITEM: f32 = 36.0;
const BAR_HEIGHT: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Copy,
    Edit,
    Publish,
}

pub struct Closed;

impl EventEmitter<Closed> for Picture {}

pub struct Picture {
    state: Entity<Rewind>,
    path: PathBuf,
    shown: Option<Arc<RenderImage>>,
    board: Board,
    failed: bool,
    editing: bool,
    menu: Option<Point<Pixels>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Annotate for Picture {
    fn board(&mut self) -> &mut Board {
        &mut self.board
    }
}

const fn publish_label(host: rewind_config::ShareHost) -> &'static str {
    match host {
        rewind_config::ShareHost::Catbox => "На Catbox",
        rewind_config::ShareHost::ImgBb => "На ImgBB",
        rewind_config::ShareHost::Pixhost => "На Pixhost",
    }
}

pub fn encode_png(rgba: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let image = RgbaImage::from_raw(width, height, rgba).ok_or("повреждённая картинка")?;
    let mut out = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(out)
}

fn render(
    bgra: &[u8],
    width: u32,
    height: u32,
    marks: &[Mark],
    crop: Option<Area>,
) -> Result<Vec<u8>, String> {
    let (mut pixels, w, h) = compose(bgra, width, height, marks, crop);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    encode_png(pixels, w, h)
}

impl Picture {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn new(
        state: Entity<Rewind>,
        path: PathBuf,
        editing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let board = Board::new(focus.clone(), true, window, cx);
        let subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        let mut picture = Self {
            state,
            path,
            shown: None,
            board,
            failed: false,
            editing,
            menu: None,
            focus,
            _subscriptions: subscriptions,
        };
        picture.load(window, cx);
        window.focus(&picture.focus, cx);
        picture
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { image::open(&path).map(|image| image.into_rgba8()) })
                .await;
            this.update_in(cx, |picture, window, cx| {
                match decoded {
                    Ok(image) => {
                        let (width, height) = image.dimensions();
                        picture.set_source(image.into_raw(), width, height, window, cx);
                    }
                    Err(_) => picture.failed = true,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_source(
        &mut self,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(old) = self.shown.take() {
            cx.drop_image(old, Some(window));
        }
        let mut bgra = rgba;
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        self.shown = render_image(width, height, bgra.clone());
        self.board
            .set_source(Arc::new(bgra), width, height, window, cx);
    }

    pub fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(old) = self.shown.take() {
            cx.drop_image(old, Some(window));
        }
        self.board.drop_base(window, cx);
    }

    fn rendered(&self) -> Option<impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static> {
        if !self.board.changed() {
            return None;
        }
        let (pixels, width, height) = self.board.source()?;
        let (marks, crop) = (self.board.marks().to_vec(), self.board.crop());
        Some(move || render(pixels.bgra(), width, height, &marks, crop))
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.with_rendered(window, cx, |picture, png, cx| {
            let path = picture.path.clone();
            picture
                .state
                .update(cx, |rewind, cx| rewind.copy_image(path, png, cx));
        });
    }

    fn publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).share_progress(&self.path).is_some() {
            return;
        }
        self.with_rendered(window, cx, |picture, png, cx| {
            let path = picture.path.clone();
            picture
                .state
                .update(cx, |rewind, cx| rewind.publish_image(path, png, cx));
        });
    }

    fn with_rendered(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Option<Vec<u8>>, &mut Context<Self>) + 'static,
    ) {
        self.board.commit_text(window, cx);
        self.menu = None;
        let Some(job) = self.rendered() else {
            then(self, None, cx);
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let png = cx.background_executor().spawn(async move { job() }).await;
            this.update(cx, |picture, cx| match png {
                Ok(png) => then(picture, Some(png), cx),
                Err(error) => picture.state.update(cx, |rewind, cx| {
                    rewind.toast(
                        Icon::Photo,
                        "Не удалось обработать снимок",
                        Some(error),
                        true,
                        cx,
                    )
                }),
            })
            .ok();
        })
        .detach();
    }

    fn save(&mut self, overwrite: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.board.commit_text(window, cx);
        let Some(job) = self.rendered() else {
            self.editing = false;
            cx.notify();
            return;
        };
        let source = self.path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let png = cx
                .background_executor()
                .spawn(async move {
                    job().map(|png| {
                        let decoded = image::load_from_memory(&png)
                            .ok()
                            .map(|image| image.into_rgba8());
                        (png, decoded)
                    })
                })
                .await;
            this.update_in(cx, |picture, window, cx| match png {
                Ok((png, decoded)) => {
                    let target = picture.state.update(cx, |rewind, cx| {
                        rewind.save_image(source, png, overwrite, cx)
                    });
                    picture.path = target;
                    picture.board.reset();
                    picture.editing = false;
                    picture.apply_rendered(decoded, window, cx);
                }
                Err(error) => picture.state.update(cx, |rewind, cx| {
                    rewind.toast(
                        Icon::Photo,
                        "Не удалось сохранить снимок",
                        Some(error),
                        true,
                        cx,
                    )
                }),
            })
            .ok();
        })
        .detach();
    }

    fn apply_rendered(
        &mut self,
        decoded: Option<RgbaImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(image) = decoded else {
            return;
        };
        let (width, height) = image.dimensions();
        self.set_source(image.into_raw(), width, height, window, cx);
        cx.notify();
    }

    fn start_editing(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.editing = true;
        cx.notify();
    }

    fn cancel_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.board.discard_typing();
        self.board.reset();
        self.editing = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn on_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.board.palette = None;
        if !self.editing {
            window.focus(&self.focus, cx);
            return;
        }
        self.board
            .press(event.position, event.click_count, window, cx);
    }

    fn on_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.board.drag_to(event.position, event.modifiers, cx);
    }

    fn on_right_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if self.editing {
            self.board.commit_text(window, cx);
            self.board.palette = Some(event.position);
        } else {
            self.menu = Some(event.position);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn on_wheel(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        self.board.wheel(event, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let control = event.keystroke.modifiers.control;
        let shift = event.keystroke.modifiers.shift;
        let key = event.keystroke.key.as_str();
        if self.board.typing() {
            self.board.typing_key(key, control, window, cx);
            return;
        }
        match key {
            "escape" if self.menu.is_some() || self.board.palette.is_some() => {
                self.menu = None;
                self.board.palette = None;
            }
            "escape" if self.board.picking => self.board.picking = false,
            "escape" if self.board.has_selected() => self.board.deselect(),
            "escape" if self.editing => self.cancel_editing(window, cx),
            "escape" => cx.emit(Closed),
            "c" if control => self.copy(window, cx),
            "s" if control && self.editing => self.save(false, window, cx),
            "o" if control => platform::shell::open(&self.path),
            "enter" => self.publish(window, cx),
            _ => {
                if !(self.editing && self.board.edit_key(key, control, shift, window, cx)) {
                    return;
                }
            }
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn run_menu(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            MenuAction::Copy => self.copy(window, cx),
            MenuAction::Edit => self.start_editing(cx),
            MenuAction::Publish => self.publish(window, cx),
        }
        cx.notify();
    }
}

impl Focusable for Picture {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Picture {
    fn header_action(
        &self,
        id: &'static str,
        glyph: Icon,
        text: &'static str,
        p: &Palette,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(6.0))
            .cursor_pointer()
            .text_size(px(CONTROL_TEXT))
            .text_color(p.text)
            .hover(|style| style.bg(p.hover))
            .child(icon(glyph, 18.0, p.icon))
            .child(text)
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let (open, reveal) = (self.path.clone(), self.path.clone());
        let uploading = self.state.read(cx).share_progress(&self.path).is_some();
        let host = publish_label(self.state.read(cx).settings.sharing.host);
        div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .gap(px(4.0))
            .pl(px(GUTTER + 2.0))
            .pr(px(GUTTER - 6.0))
            .bg(p.header)
            .child(strong("Rewind", PAGE_TITLE_TEXT, p.title))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .ml(px(10.0))
                    .text_size(px(CONTROL_TEXT))
                    .text_color(p.muted)
                    .child(name),
            )
            .when(!self.editing, |bar| {
                bar.child(
                    self.header_action("picture-edit", Icon::Edit, "Редактировать", &p)
                        .on_click(cx.listener(|picture, _, _, cx| picture.start_editing(cx))),
                )
            })
            .child(
                self.header_action("picture-copy", Icon::Copy, "Копировать", &p)
                    .on_click(cx.listener(|picture, _, window, cx| picture.copy(window, cx))),
            )
            .child(
                self.header_action("picture-publish", Icon::Upload, host, &p)
                    .when(uploading, |action| action.opacity(0.4))
                    .when(!uploading, |action| {
                        action.on_click(
                            cx.listener(|picture, _, window, cx| picture.publish(window, cx)),
                        )
                    }),
            )
            .child(
                self.header_action("picture-open", Icon::Open, "Открыть в приложении", &p)
                    .on_click(move |_, _, _| platform::shell::open(&open)),
            )
            .child(
                self.header_action("picture-reveal", Icon::Folder, "Показать в папке", &p)
                    .on_click(move |_, _, _| platform::shell::select(&reveal)),
            )
            .child(
                div()
                    .id("picture-close")
                    .flex_none()
                    .ml(px(6.0))
                    .p(px(6.0))
                    .cursor_pointer()
                    .child(icon(Icon::Close, ICON, p.title))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Closed))),
            )
            .into_any_element()
    }

    fn progress(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = cx.palette();
        let rewind = self.state.read(cx);
        let value = rewind.share_progress(&self.path)?;
        let host = rewind.settings.sharing.host.label();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(GUTTER))
                .py(px(8.0))
                .bg(p.panel)
                .child(
                    div()
                        .text_size(px(SMALL_TEXT))
                        .text_color(p.text)
                        .child(format!(
                            "Загрузка на {host}… {}%",
                            (value * 100.0).round() as u32
                        )),
                )
                .child(
                    div().h(px(BAR_HEIGHT)).w_full().bg(p.track).child(
                        div()
                            .h_full()
                            .w(gpui::relative(value.clamp(0.0, 1.0)))
                            .bg(p.accent),
                    ),
                )
                .into_any_element(),
        )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette();
        let board = &self.board;
        let tools = annotate::tool_buttons(board, cx);
        let sizes = annotate::size_controls(board, cx);
        let counter = annotate::counter_hint(board, &p);
        let swatches = annotate::swatches(board, "color", cx);
        let eyedropper = annotate::eyedropper(board, cx);
        let text = (board.tool == Tool::Text || board.typing())
            .then(|| annotate::text_controls(board, cx));
        let undo = annotate::undo_button(board, cx);
        let redo = annotate::redo_button(board, cx);
        let delete = annotate::delete_button(board, cx);
        let changed = board.changed();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(GUTTER))
            .py(px(10.0))
            .bg(p.panel)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.0))
                    .children(tools)
                    .child(separator(&p))
                    .children(sizes)
                    .children(counter),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .children(swatches)
                    .child(eyedropper)
                    .when_some(text, |row, text| row.child(separator(&p)).child(text))
                    .child(separator(&p))
                    .child(undo)
                    .child(redo)
                    .child(delete)
                    .child(div().flex_1())
                    .child(
                        button(
                            "picture-clear",
                            "Сбросить",
                            ButtonKind::Secondary,
                            changed,
                            &p,
                        )
                        .when(changed, |button| {
                            button
                                .on_click(cx.listener(|picture, _, _, cx| picture.board.clear(cx)))
                        }),
                    )
                    .child(
                        button("picture-cancel", "Отмена", ButtonKind::Secondary, true, &p)
                            .on_click(cx.listener(|picture, _, window, cx| {
                                picture.cancel_editing(window, cx)
                            })),
                    )
                    .child(
                        button(
                            "picture-save-copy",
                            "Сохранить копию",
                            ButtonKind::Secondary,
                            changed,
                            &p,
                        )
                        .when(changed, |button| {
                            button.on_click(
                                cx.listener(|picture, _, window, cx| {
                                    picture.save(false, window, cx)
                                }),
                            )
                        }),
                    )
                    .child(
                        button(
                            "picture-save",
                            "Сохранить",
                            ButtonKind::Primary,
                            changed,
                            &p,
                        )
                        .when(changed, |button| {
                            button.on_click(
                                cx.listener(|picture, _, window, cx| {
                                    picture.save(true, window, cx)
                                }),
                            )
                        }),
                    ),
            )
            .into_any_element()
    }

    fn menu_layer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = cx.palette();
        let position = self.menu?;
        let mut actions = vec![(MenuAction::Copy, Icon::Copy, "Копировать")];
        if !self.editing {
            actions.push((MenuAction::Edit, Icon::Edit, "Редактировать"));
        }
        actions.push((MenuAction::Publish, Icon::Upload, "Опубликовать"));
        let items = actions.into_iter().map(|(action, glyph, label)| {
            div()
                .id(("picture-menu", action as usize))
                .h(px(MENU_ITEM))
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(14.0))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .child(icon(glyph, 18.0, p.icon))
                .child(
                    div()
                        .text_size(px(CONTROL_TEXT))
                        .text_color(p.text)
                        .child(label),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |picture, _, window, cx| {
                        cx.stop_propagation();
                        picture.run_menu(action, window, cx);
                    }),
                )
        });
        Some(
            deferred(
                anchored().position(position).snap_to_window().child(
                    div()
                        .id("picture-menu")
                        .w(px(MENU_WIDTH))
                        .py(px(4.0))
                        .bg(p.header)
                        .shadow(p.toast_shadow())
                        .occlude()
                        .children(items)
                        .on_mouse_down_out(cx.listener(|picture, _, _, cx| {
                            picture.menu = None;
                            cx.notify();
                        })),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

impl Render for Picture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.board.refresh_base(window, cx);
        let p = cx.palette();
        let header = self.header(cx);
        let progress = self.progress(cx);
        let toolbar = self.editing.then(|| self.toolbar(cx));
        let cropping = self.editing && self.board.tool == Tool::Crop;
        let layer = annotate::overlay(&self.board, cropping, p, cx);
        let texts = annotate::texts(&self.board);
        let handles = self
            .editing
            .then(|| annotate::crop_handles(&self.board, &p));
        let typing = annotate::typing_field(&self.board, &p);
        let menu = self.menu_layer(cx);
        let palette = annotate::palette_popover(&self.board, "picture-palette", Vec::new(), cx);
        let backdrop = self.board.backdrop().or_else(|| self.shown.clone());
        let cursor = if self.editing {
            self.board.cursor()
        } else {
            CursorStyle::Arrow
        };
        div()
            .id("picture")
            .font_family(FONT_FAMILY)
            .size_full()
            .flex()
            .flex_col()
            .bg(p.panel)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|picture, _, _, cx| picture.board.release(cx)),
            )
            .child(header)
            .children(progress)
            .children(toolbar)
            .child(
                div()
                    .id("picture-stage")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .bg(p.stage)
                    .cursor(cursor)
                    .when_some(backdrop, |stage, image| {
                        stage.child(img(image).size_full().object_fit(ObjectFit::Contain))
                    })
                    .when(self.failed, |stage| {
                        stage.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted)
                                .child("Не удалось открыть картинку"),
                        )
                    })
                    .child(layer)
                    .children(texts)
                    .children(handles.into_iter().flatten())
                    .children(typing)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
                    .on_mouse_down(
                        MouseButton::Navigate(gpui::NavigationDirection::Back),
                        cx.listener(|view, _, _, cx| {
                            if view.editing {
                                cx.stop_propagation();
                                view.board.undo(cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Navigate(gpui::NavigationDirection::Forward),
                        cx.listener(|view, _, _, cx| {
                            if view.editing {
                                cx.stop_propagation();
                                view.board.redo(cx);
                            }
                        }),
                    )
                    .on_scroll(cx.listener(Self::on_wheel)),
            )
            .children(menu)
            .children(palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_crops_and_encodes_a_png() {
        let (width, height) = (8u32, 4u32);
        let bgra: Vec<u8> = (0..width * height)
            .flat_map(|_| [30, 20, 10, 255])
            .collect();
        let crop = Some(Area {
            x: 0.5,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        });
        let png = render(&bgra, width, height, &[], crop).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (4, 4));
        assert_eq!(decoded.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }
}
