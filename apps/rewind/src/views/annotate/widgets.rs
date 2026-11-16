use gpui::{AnyElement, Context, MouseButton, Stateful, anchored, deferred, div, prelude::*, px};

use rewind_media::Align;

use super::{Annotate, Board, FONTS, PALETTE, TOOL_BUTTON, Tool, color_of};
use crate::assets::{Icon, icon};
use crate::theme::{CONTROL_TEXT, Palette, SMALL_TEXT, ThemeExt, alpha};
use crate::ui::Tooltip;

const SWATCH: f32 = 18.0;
const PALETTE_COLUMNS: usize = 6;

pub fn square_button(
    id: impl Into<gpui::ElementId>,
    glyph: Icon,
    active: bool,
    p: &Palette,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(TOOL_BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .when(active, |item| item.bg(alpha(p.accent, 0.22)))
        .hover(|style| style.bg(p.hover))
        .child(icon(glyph, 20.0, if active { p.accent } else { p.icon }))
}

pub fn separator(p: &Palette) -> gpui::Div {
    div().flex_none().w(px(1.0)).h(px(22.0)).bg(p.divider)
}

pub fn swatches<T: Annotate>(
    board: &Board,
    prefix: &'static str,
    cx: &mut Context<T>,
) -> Vec<AnyElement> {
    let p = cx.palette();
    PALETTE
        .iter()
        .enumerate()
        .map(|(index, color)| {
            let color = *color;
            let chosen = board.color == color;
            div()
                .id((prefix, index))
                .flex_none()
                .size(px(SWATCH))
                .rounded_full()
                .bg(color_of(color))
                .border_2()
                .border_color(if chosen { p.accent } else { p.divider })
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |host: &mut T, _, _, cx| {
                        cx.stop_propagation();
                        let board = host.board();
                        board.palette = None;
                        board.set_color(color, cx);
                    }),
                )
                .into_any_element()
        })
        .collect()
}

pub fn text_controls<T: Annotate>(board: &Board, cx: &mut Context<T>) -> AnyElement {
    let p = cx.palette();
    let style = &board.style;
    let toggle = |id: &'static str, glyph: Icon, active: bool, tip: &'static str| {
        square_button(id, glyph, active, &p).tooltip(Tooltip::text(tip))
    };
    let font = style.font.clone();
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(
            div()
                .id("text-font")
                .flex_none()
                .px(px(10.0))
                .h(px(TOOL_BUTTON))
                .flex()
                .items_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|s| s.bg(p.hover))
                .text_size(px(SMALL_TEXT))
                .text_color(p.text)
                .font_family(font.clone())
                .child(font)
                .tooltip(Tooltip::text("Шрифт: нажмите, чтобы сменить"))
                .on_click(cx.listener(|host: &mut T, _, _, cx| {
                    let board = host.board();
                    let next = FONTS
                        .iter()
                        .position(|font| *font == board.style.font)
                        .map_or(0, |index| (index + 1) % FONTS.len());
                    board.style.font = FONTS[next].to_owned();
                    cx.notify();
                })),
        )
        .child(
            toggle("text-bold", Icon::Bold, style.bold, "Жирный").on_click(cx.listener(
                |host: &mut T, _, _, cx| {
                    let style = &mut host.board().style;
                    style.bold = !style.bold;
                    cx.notify();
                },
            )),
        )
        .child(
            toggle("text-italic", Icon::Italic, style.italic, "Курсив").on_click(cx.listener(
                |host: &mut T, _, _, cx| {
                    let style = &mut host.board().style;
                    style.italic = !style.italic;
                    cx.notify();
                },
            )),
        )
        .child(
            toggle(
                "text-underline",
                Icon::Underline,
                style.underline,
                "Подчёркнутый",
            )
            .on_click(cx.listener(|host: &mut T, _, _, cx| {
                let style = &mut host.board().style;
                style.underline = !style.underline;
                cx.notify();
            })),
        )
        .child(
            toggle("text-strike", Icon::Strike, style.strike, "Зачёркнутый").on_click(cx.listener(
                |host: &mut T, _, _, cx| {
                    let style = &mut host.board().style;
                    style.strike = !style.strike;
                    cx.notify();
                },
            )),
        )
        .child(separator(&p))
        .children(
            [
                (Align::Left, Icon::AlignLeft, "По левому краю"),
                (Align::Center, Icon::AlignCenter, "По центру"),
                (Align::Right, Icon::AlignRight, "По правому краю"),
            ]
            .into_iter()
            .map(|(align, glyph, tip)| {
                toggle(
                    match align {
                        Align::Left => "text-left",
                        Align::Center => "text-center",
                        Align::Right => "text-right",
                    },
                    glyph,
                    style.align == align,
                    tip,
                )
                .on_click(cx.listener(move |host: &mut T, _, _, cx| {
                    host.board().style.align = align;
                    cx.notify();
                }))
            }),
        )
        .into_any_element()
}

pub fn tool_buttons<T: Annotate>(board: &Board, cx: &mut Context<T>) -> Vec<AnyElement> {
    let p = cx.palette();
    board
        .tools()
        .map(|(tool, glyph, _, tip)| {
            let tool = *tool;
            square_button(("tool", tool as usize), *glyph, board.tool == tool, &p)
                .tooltip(Tooltip::text(tip))
                .on_click(cx.listener(move |host: &mut T, _, window, cx| {
                    host.board().choose_tool(tool, window, cx)
                }))
                .into_any_element()
        })
        .collect()
}

pub fn size_controls<T: Annotate>(board: &Board, cx: &mut Context<T>) -> Vec<AnyElement> {
    let p = cx.palette();
    vec![
        square_button("size-down", Icon::Minus, false, &p)
            .tooltip(Tooltip::text("Тоньше (−, колесо мыши)"))
            .on_click(cx.listener(|host: &mut T, _, _, cx| {
                let board = host.board();
                board.set_size(board.size.saturating_sub(1), cx)
            }))
            .into_any_element(),
        div()
            .flex_none()
            .w(px(28.0))
            .text_center()
            .text_size(px(CONTROL_TEXT))
            .text_color(p.text)
            .child(board.size.to_string())
            .into_any_element(),
        square_button("size-up", Icon::Plus, false, &p)
            .tooltip(Tooltip::text("Толще (+, колесо мыши)"))
            .on_click(cx.listener(|host: &mut T, _, _, cx| {
                let board = host.board();
                board.set_size(board.size + 1, cx)
            }))
            .into_any_element(),
    ]
}

pub fn counter_hint(board: &Board, p: &Palette) -> Option<gpui::Div> {
    (board.tool == Tool::Counter).then(|| {
        div()
            .flex_none()
            .px(px(6.0))
            .text_size(px(SMALL_TEXT))
            .text_color(p.muted)
            .child(format!("следующий № {} · Ctrl+колесо", board.counter))
    })
}

pub fn eyedropper<T: Annotate>(board: &Board, cx: &mut Context<T>) -> Stateful<gpui::Div> {
    let p = cx.palette();
    square_button("eyedropper", Icon::Eyedropper, board.picking, &p)
        .tooltip(Tooltip::text("Пипетка (G)"))
        .on_click(cx.listener(|host: &mut T, _, _, cx| {
            let board = host.board();
            board.picking = !board.picking;
            cx.notify();
        }))
}

fn history(id: &'static str, glyph: Icon, enabled: bool, p: &Palette) -> Stateful<gpui::Div> {
    square_button(id, glyph, false, p).opacity(if enabled { 1.0 } else { 0.35 })
}

pub fn undo_button<T: Annotate>(board: &Board, cx: &mut Context<T>) -> Stateful<gpui::Div> {
    let p = cx.palette();
    let enabled = board.can_undo();
    history("picture-undo", Icon::Undo, enabled, &p)
        .when(enabled, |item| {
            item.on_click(cx.listener(|host: &mut T, _, _, cx| host.board().undo(cx)))
        })
        .tooltip(Tooltip::text("Отменить (Ctrl+Z)"))
}

pub fn redo_button<T: Annotate>(board: &Board, cx: &mut Context<T>) -> Stateful<gpui::Div> {
    let p = cx.palette();
    let enabled = board.can_redo();
    history("picture-redo", Icon::Redo, enabled, &p)
        .when(enabled, |item| {
            item.on_click(cx.listener(|host: &mut T, _, _, cx| host.board().redo(cx)))
        })
        .tooltip(Tooltip::text("Повторить (Ctrl+Y)"))
}

pub fn delete_button<T: Annotate>(board: &Board, cx: &mut Context<T>) -> Stateful<gpui::Div> {
    let p = cx.palette();
    let enabled = board.has_selected();
    history("picture-delete", Icon::Delete, enabled, &p)
        .when(enabled, |item| {
            item.on_click(cx.listener(|host: &mut T, _, _, cx| host.board().delete_selected(cx)))
        })
        .tooltip(Tooltip::text("Удалить выбранное (Delete)"))
}

pub fn palette_popover<T: Annotate>(
    board: &Board,
    id: &'static str,
    extra: Vec<AnyElement>,
    cx: &mut Context<T>,
) -> Option<AnyElement> {
    let p = cx.palette();
    let position = board.palette?;
    let mut swatches = swatches(board, "palette", cx).into_iter();
    let rows: Vec<AnyElement> = (0..PALETTE.len().div_ceil(PALETTE_COLUMNS))
        .map(|_| {
            div()
                .flex()
                .gap(px(8.0))
                .children(swatches.by_ref().take(PALETTE_COLUMNS))
                .into_any_element()
        })
        .collect();
    Some(
        deferred(
            anchored().position(position).snap_to_window().child(
                div()
                    .id(id)
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .bg(p.header)
                    .shadow(p.toast_shadow())
                    .occlude()
                    .children(rows)
                    .child(
                        div()
                            .id("palette-pick")
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .text_size(px(SMALL_TEXT))
                            .text_color(p.text)
                            .child(icon(Icon::Eyedropper, 18.0, p.icon))
                            .child("Взять цвет с картинки (G)")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|host: &mut T, _, _, cx| {
                                    cx.stop_propagation();
                                    let board = host.board();
                                    board.palette = None;
                                    board.picking = true;
                                    cx.notify();
                                }),
                            ),
                    )
                    .children(extra)
                    .on_mouse_down_out(cx.listener(|host: &mut T, _, _, cx| {
                        host.board().palette = None;
                        cx.notify();
                    })),
            ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}
