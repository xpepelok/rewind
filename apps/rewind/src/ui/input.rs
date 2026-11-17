use std::ops::Range;
use std::time::Duration;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, Task, TextRun, UTF16Selection, UnderlineStyle, Window,
    actions, anchored, deferred, div, fill, point, prelude::*, px, relative, size,
};

use crate::theme::{ThemeExt, alpha};

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
        Submit,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        DocumentStart,
        DocumentEnd,
        SelectDocumentStart,
        SelectDocumentEnd,
        Undo,
        Redo
    ]
);

const CONTEXT: &str = "TextInput";
const BLINK: Duration = Duration::from_millis(530);
const HISTORY: usize = 200;
const MENU_WIDTH: f32 = 200.0;
const MENU_ITEM: f32 = 32.0;
const MENU_TEXT: f32 = 13.0;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    content: String,
    selected: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Undo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
}

impl MenuAction {
    const fn label(self) -> &'static str {
        match self {
            Self::Undo => "Отменить",
            Self::Cut => "Вырезать",
            Self::Copy => "Копировать",
            Self::Paste => "Вставить",
            Self::Delete => "Удалить",
            Self::SelectAll => "Выделить всё",
        }
    }

    const fn shortcut(self) -> &'static str {
        match self {
            Self::Undo => "Ctrl+Z",
            Self::Cut => "Ctrl+X",
            Self::Copy => "Ctrl+C",
            Self::Paste => "Ctrl+V",
            Self::Delete => "Del",
            Self::SelectAll => "Ctrl+A",
        }
    }
}

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("ctrl-left", WordLeft, context),
        KeyBinding::new("ctrl-right", WordRight, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("shift-insert", Paste, context),
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("ctrl-insert", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
        KeyBinding::new("shift-delete", Cut, context),
        KeyBinding::new("ctrl-z", Undo, context),
        KeyBinding::new("ctrl-y", Redo, context),
        KeyBinding::new("ctrl-shift-z", Redo, context),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, context),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, context),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, context),
        KeyBinding::new("ctrl-delete", DeleteWordRight, context),
        KeyBinding::new("ctrl-home", DocumentStart, context),
        KeyBinding::new("ctrl-end", DocumentEnd, context),
        KeyBinding::new("ctrl-shift-home", SelectDocumentStart, context),
        KeyBinding::new("ctrl-shift-end", SelectDocumentEnd, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("enter", Submit, context),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Changed,
    Submit,
    Blur,
}

pub struct TextInput {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    layout: Vec<ShapedLine>,
    line_height: Pixels,
    bounds: Option<Bounds<Pixels>>,
    selecting: bool,
    max_chars: Option<usize>,
    masked: bool,
    align_center: bool,
    multiline: bool,
    read_only: bool,
    caret_visible: bool,
    blink: Option<Task<()>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    typing_run: bool,
    menu: Option<(Point<Pixels>, bool)>,
}

impl EventEmitter<InputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |this, _, cx| this.start_blink(cx))
            .detach();
        cx.on_blur(&focus, window, |this, _, cx| {
            this.blink = None;
            this.menu = None;
            this.selected = this.cursor()..this.cursor();
            cx.emit(InputEvent::Blur);
            cx.notify();
        })
        .detach();
        Self {
            focus,
            content: String::new(),
            placeholder: placeholder.into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            layout: Vec::new(),
            line_height: px(0.0),
            bounds: None,
            selecting: false,
            max_chars: None,
            masked: false,
            align_center: false,
            multiline: false,
            read_only: false,
            caret_visible: true,
            blink: None,
            undo: Vec::new(),
            redo: Vec::new(),
            typing_run: false,
            menu: None,
        }
    }

    pub fn max_chars(mut self, max: usize) -> Self {
        self.max_chars = Some(max);
        self
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    pub fn align_center(mut self) -> Self {
        self.align_center = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = self.sanitize(&text.into());
        if text != self.content {
            self.content = text;
            let end = self.content.len();
            self.selected = end..end;
            self.marked = None;
            self.undo.clear();
            self.redo.clear();
            self.typing_run = false;
            cx.notify();
        }
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    fn start_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_visible = true;
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                if this
                    .update(cx, |input, cx| {
                        input.caret_visible = !input.caret_visible;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        if self.blink.is_some() {
            self.start_blink(cx);
        }
    }

    fn sanitize(&self, text: &str) -> String {
        let multiline = self.multiline;
        let filtered: String = text
            .chars()
            .filter(|ch| *ch != '\r')
            .map(|ch| if ch == '\t' { ' ' } else { ch })
            .filter(|ch| !ch.is_control() || (multiline && *ch == '\n'))
            .collect();
        match self.max_chars {
            Some(max) => filtered.chars().take(max).collect(),
            None => filtered,
        }
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            selected: self.selected.clone(),
        }
    }

    fn remember(&mut self, before: Snapshot, typing: bool) {
        if !(typing && self.typing_run) {
            self.undo.push(before);
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.typing_run = typing;
    }

    fn restore(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        self.selected = snapshot.selected;
        self.reversed = false;
        self.marked = None;
        self.typing_run = false;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(snapshot, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(snapshot, cx);
        }
    }

    fn word_at(&self, offset: usize) -> Range<usize> {
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        let start = self.content[..offset]
            .char_indices()
            .rev()
            .take_while(|(_, ch)| is_word(*ch))
            .last()
            .map_or(offset, |(index, _)| index);
        let end = self.content[offset..]
            .char_indices()
            .take_while(|(_, ch)| is_word(*ch))
            .last()
            .map_or(offset, |(index, ch)| offset + index + ch.len_utf8());
        start..end
    }

    fn line_start(&self, offset: usize) -> usize {
        if !self.multiline {
            return 0;
        }
        self.content[..offset]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        if !self.multiline {
            return self.content.len();
        }
        self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |index| offset + index)
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word(self.cursor()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word(self.cursor()), cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if self.selected.is_empty() {
            self.select_to(self.previous_word(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if self.selected.is_empty() {
            self.select_to(self.next_word(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_document_start(
        &mut self,
        _: &SelectDocumentStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    fn select_document_end(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.content.len(), cx);
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        let Some((_, clipboard)) = self.menu else {
            return Vec::new();
        };
        let selected = !self.selected.is_empty();
        let editable = !self.read_only;
        let copyable = selected && !self.masked;
        [
            (MenuAction::Undo, editable && !self.undo.is_empty()),
            (MenuAction::Cut, editable && copyable),
            (MenuAction::Copy, copyable),
            (MenuAction::Paste, editable && clipboard),
            (MenuAction::Delete, editable && selected),
            (
                MenuAction::SelectAll,
                !self.content.is_empty() && self.selected != (0..self.content.len()),
            ),
        ]
        .into_iter()
        .filter_map(|(action, shown)| shown.then_some(action))
        .collect()
    }

    fn run_menu(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            MenuAction::Undo => self.undo(&Undo, window, cx),
            MenuAction::Cut => self.cut(&Cut, window, cx),
            MenuAction::Copy => self.copy(&Copy, window, cx),
            MenuAction::Paste => self.paste(&Paste, window, cx),
            MenuAction::Delete => self.replace_text_in_range(None, "", window, cx),
            MenuAction::SelectAll => self.select_all_text(cx),
        }
        cx.notify();
    }

    fn on_right_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let index = self.index_for_position(event.position);
        if self.selected.is_empty() {
            self.move_to(index, cx);
        }
        let clipboard = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| !text.is_empty());
        self.menu = Some((event.position, clipboard));
        cx.stop_propagation();
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.typing_run = false;
        self.selected = offset..offset;
        self.reversed = false;
        self.reset_blink(cx);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .chars()
            .next()
            .map_or(self.content.len(), |ch| offset + ch.len_utf8())
    }

    fn previous_word(&self, offset: usize) -> usize {
        let head = &self.content[..offset];
        let trimmed = head.trim_end();
        trimmed
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1)
    }

    fn next_word(&self, offset: usize) -> usize {
        let tail = &self.content[offset..];
        let skipped = tail.len() - tail.trim_start().len();
        tail[skipped..]
            .find(char::is_whitespace)
            .map_or(self.content.len(), |index| offset + skipped + index)
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.previous_boundary(self.cursor()), cx);
        } else {
            self.move_to(self.selected.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.next_boundary(self.selected.end), cx);
        } else {
            self.move_to(self.selected.end, cx);
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor()), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_text(cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(self.cursor()), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor()), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(self.cursor()), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.cursor()), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selected.is_empty() {
            self.select_to(self.previous_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selected.is_empty() {
            self.select_to(self.next_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = if self.multiline {
                text.replace('\r', "")
            } else {
                text.replace(['\r', '\n'], " ")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected.clone()].to_owned(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.emit(InputEvent::Submit);
        }
    }

    fn line_starts(&self) -> Vec<usize> {
        std::iter::once(0)
            .chain(self.content.match_indices('\n').map(|(index, _)| index + 1))
            .collect()
    }

    fn line_of(starts: &[usize], offset: usize) -> usize {
        starts
            .iter()
            .rposition(|start| *start <= offset)
            .unwrap_or(0)
    }

    fn vertical(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.multiline || self.layout.is_empty() {
            return;
        }
        let starts = self.line_starts();
        let cursor = self.cursor();
        let line = Self::line_of(&starts, cursor);
        let target = line as isize + delta;
        if target < 0 || target as usize >= starts.len() || target as usize >= self.layout.len() {
            return;
        }
        let x = self.layout[line].x_for_index(cursor - starts[line]);
        let target = target as usize;
        let local = self.layout[target].closest_index_for_x(x);
        self.move_to(starts[target] + local, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, cx);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.menu = None;
        self.selecting = true;
        let index = self.index_for_position(event.position);
        if event.click_count >= 3 {
            self.select_all_text(cx);
            return;
        }
        if event.click_count == 2 {
            self.selected = self.word_at(index);
            self.reversed = false;
            self.typing_run = false;
            cx.notify();
            return;
        }
        if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.index_for_position(event.position), cx);
        }
    }

    fn display_text(&self) -> String {
        if self.masked {
            "•".repeat(self.content.chars().count())
        } else {
            self.content.clone()
        }
    }

    fn to_display(&self, offset: usize) -> usize {
        if self.masked {
            self.content[..offset].chars().count() * '•'.len_utf8()
        } else {
            offset
        }
    }

    fn offset_of_display(&self, offset: usize) -> usize {
        if self.masked {
            let chars = offset / '•'.len_utf8();
            self.content
                .char_indices()
                .nth(chars)
                .map_or(self.content.len(), |(index, _)| index)
        } else {
            offset
        }
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds.as_ref() else {
            return 0;
        };
        if self.content.is_empty() || self.layout.is_empty() {
            return 0;
        }
        if self.multiline {
            let starts = self.line_starts();
            let height = f32::from(self.line_height).max(1.0);
            let row = (f32::from(position.y - bounds.top()) / height)
                .floor()
                .max(0.0) as usize;
            let row = row.min(self.layout.len() - 1).min(starts.len() - 1);
            return starts[row] + self.layout[row].closest_index_for_x(position.x - bounds.left());
        }
        let line = &self.layout[0];
        let shift = self.text_shift(bounds, line);
        self.offset_of_display(line.closest_index_for_x(position.x - bounds.left() - shift))
    }

    fn text_shift(&self, bounds: &Bounds<Pixels>, line: &ShapedLine) -> Pixels {
        if self.align_center && !self.multiline {
            ((bounds.size.width - line.width) / 2.0).max(px(0.0))
        } else {
            px(0.0)
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn splice(&mut self, range: Range<usize>, text: &str) -> usize {
        let inserted = self.sanitize(text);
        let mut next = String::with_capacity(self.content.len() + inserted.len());
        next.push_str(&self.content[..range.start]);
        next.push_str(&inserted);
        next.push_str(&self.content[range.end..]);
        let capped = self.sanitize(&next);
        let cursor = (range.start + inserted.len()).min(capped.len());
        self.content = capped;
        cursor
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        let before = self.snapshot();
        let typing =
            range.is_empty() && text.chars().count() == 1 && !text.contains(char::is_whitespace);
        let cursor = self.splice(range, text);
        self.selected = cursor..cursor;
        self.reversed = false;
        self.marked = None;
        self.reset_blink(cx);
        if before.content != self.content {
            self.remember(before, typing);
            cx.emit(InputEvent::Changed);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        let start = range.start;
        let cursor = self.splice(range, text);
        self.marked = (!text.is_empty()).then_some(start..cursor);
        self.selected = selected
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .map(|inner| (start + inner.start).min(cursor)..(start + inner.end).min(cursor))
            .unwrap_or(cursor..cursor);
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range);
        let starts = self.line_starts();
        let row = if self.multiline {
            Self::line_of(&starts, range.start)
        } else {
            0
        };
        let line = self.layout.get(row)?;
        let base = if self.multiline { starts[row] } else { 0 };
        let shift = self.text_shift(&bounds, line);
        let top = bounds.top() + self.line_height * row as f32;
        let end = self
            .to_display(range.end.max(range.start))
            .saturating_sub(base);
        Some(Bounds::from_corners(
            point(
                bounds.left() + shift + line.x_for_index(self.to_display(range.start) - base),
                top,
            ),
            point(
                bounds.left() + shift + line.x_for_index(end),
                top + self.line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.bounds?.localize(&position)?;
        Some(self.offset_to_utf16(self.index_for_position(position)))
    }
}

struct LineElement {
    input: Entity<TextInput>,
    color: Hsla,
    placeholder: Hsla,
    selection: Hsla,
    caret: Hsla,
}

struct LinePaint {
    lines: Vec<ShapedLine>,
    shift: Pixels,
    caret: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for LineElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LineElement {
    type RequestLayoutState = ();
    type PrepaintState = LinePaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.read(cx);
        let rows = if input.multiline {
            input.content.split('\n').count().max(1)
        } else {
            1
        };
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * rows as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let empty = input.content.is_empty();
        let color = if empty { self.placeholder } else { self.color };
        let run = |len: usize| TextRun {
            len,
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let texts: Vec<SharedString> = if empty {
            vec![input.placeholder.clone()]
        } else if input.multiline {
            input
                .content
                .split('\n')
                .map(|line| SharedString::from(line.to_owned()))
                .collect()
        } else {
            vec![input.display_text().into()]
        };
        let marked = input
            .marked
            .clone()
            .filter(|_| !empty && !input.masked && !input.multiline);
        let lines: Vec<ShapedLine> = texts
            .into_iter()
            .map(|text| {
                let runs = match marked.as_ref() {
                    Some(marked) => [
                        run(marked.start),
                        TextRun {
                            underline: Some(UnderlineStyle {
                                color: Some(color),
                                thickness: px(1.0),
                                wavy: false,
                            }),
                            ..run(marked.end - marked.start)
                        },
                        run(text.len() - marked.end),
                    ]
                    .into_iter()
                    .filter(|run| run.len > 0)
                    .collect(),
                    None => vec![run(text.len())],
                };
                window
                    .text_system()
                    .shape_line(text, font_size, &runs, None)
            })
            .collect();
        let shift = lines
            .first()
            .map_or(px(0.0), |line| input.text_shift(&bounds, line));
        let starts = if input.multiline && !empty {
            input.line_starts()
        } else {
            vec![0]
        };
        let to_display = |offset: usize| if empty { 0 } else { input.to_display(offset) };
        let position = |offset: usize| {
            let row = if input.multiline {
                TextInput::line_of(&starts, offset)
            } else {
                0
            };
            let local = offset - starts.get(row).copied().unwrap_or(0);
            let x = lines
                .get(row)
                .map_or(px(0.0), |line| line.x_for_index(local));
            (row, x)
        };
        let (start, end) = (
            to_display(input.selected.start),
            to_display(input.selected.end),
        );
        let focused = input.focus.is_focused(window);
        let caret = (focused && input.caret_visible && start == end).then(|| {
            let (row, x) = position(to_display(input.cursor()));
            let x = if empty && input.align_center {
                bounds.size.width / 2.0
            } else {
                shift + x
            };
            fill(
                Bounds::new(
                    point(
                        bounds.left() + x,
                        bounds.top() + line_height * row as f32 + px(2.0),
                    ),
                    size(px(1.5), line_height - px(4.0)),
                ),
                self.caret,
            )
        });
        let mut selection = Vec::new();
        if start != end {
            let (first, _) = position(start);
            let (last, _) = position(end);
            for row in first..=last {
                let Some(line) = lines.get(row) else {
                    continue;
                };
                let line_start = starts.get(row).copied().unwrap_or(0);
                let from = start.max(line_start) - line_start;
                let to = end.min(line_start + line.len()).saturating_sub(line_start);
                let left = line.x_for_index(from);
                let right =
                    line.x_for_index(to.max(from)) + if row < last { px(4.0) } else { px(0.0) };
                let top = bounds.top() + line_height * row as f32;
                selection.push(fill(
                    Bounds::from_corners(
                        point(bounds.left() + shift + left, top),
                        point(bounds.left() + shift + right, top + line_height),
                    ),
                    self.selection,
                ));
            }
        }
        LinePaint {
            lines,
            shift,
            caret,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        let line_height = window.line_height();
        for (row, line) in prepaint.lines.iter().enumerate() {
            let origin = point(
                bounds.origin.x + prepaint.shift,
                bounds.origin.y + line_height * row as f32,
            );
            line.paint(origin, line_height, gpui::TextAlign::Left, None, window, cx)
                .ok();
        }
        if let Some(caret) = prepaint.caret.take() {
            window.paint_quad(caret);
        }
        let lines = std::mem::take(&mut prepaint.lines);
        self.input.update(cx, |input, _| {
            input.layout = lines;
            input.line_height = line_height;
            input.bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        let actions = self.menu_actions();
        let menu = self
            .menu
            .filter(|_| !actions.is_empty())
            .map(|(position, _)| {
                let items = actions.into_iter().map(|action| {
                    div()
                        .id(action.label())
                        .h(px(MENU_ITEM))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .cursor_pointer()
                        .hover(|style| style.bg(p.hover))
                        .child(div().text_color(p.text).child(action.label()))
                        .child(div().text_color(p.muted).child(action.shortcut()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |input, _, window, cx| {
                                cx.stop_propagation();
                                input.run_menu(action, window, cx);
                            }),
                        )
                });
                deferred(
                    anchored().position(position).snap_to_window().child(
                        div()
                            .id("text-menu")
                            .w(px(MENU_WIDTH))
                            .py(px(4.0))
                            .bg(p.toast)
                            .shadow(p.toast_shadow())
                            .text_size(px(MENU_TEXT))
                            .occlude()
                            .children(items)
                            .on_mouse_down_out(cx.listener(|input, _, _, cx| {
                                input.menu = None;
                                cx.notify();
                            })),
                    ),
                )
                .with_priority(1)
            });
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::document_start))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_document_start))
            .on_action(cx.listener(Self::select_document_end))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(LineElement {
                input: cx.entity(),
                color: p.text,
                placeholder: p.disabled,
                selection: alpha(p.accent, 0.35),
                caret: p.text,
            })
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn limits_length(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| TextInput::new("", window, cx).max_chars(8));
        window
            .update(cx, |input, _, cx| {
                input.set_text("ПрофильДлинный", cx);
                assert_eq!(input.text(), "ПрофильД");
            })
            .unwrap();
    }

    #[gpui::test]
    fn typing_undoes_as_one_step_and_redoes(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| TextInput::new("", window, cx));
        window
            .update(cx, |input, window, cx| {
                for ch in ["h", "i"] {
                    input.replace_text_in_range(None, ch, window, cx);
                }
                input.replace_text_in_range(None, " there", window, cx);
                assert_eq!(input.text(), "hi there");
                input.undo(&Undo, window, cx);
                assert_eq!(input.text(), "hi");
                input.undo(&Undo, window, cx);
                assert_eq!(input.text(), "");
                input.redo(&Redo, window, cx);
                assert_eq!(input.text(), "hi");
                assert_eq!(input.word_at(1), 0..2);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_focused_field_is_painted_and_takes_typed_text(cx: &mut TestAppContext) {
        cx.update(crate::theme::install);
        let (input, cx) = cx.add_window_view(|window, cx| TextInput::new("", window, cx));
        cx.update(|window, cx| {
            let focus = input.read(cx).focus.clone();
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
        });
        cx.simulate_input("кадр");
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            let input = input.read(cx);
            assert_eq!(input.text(), "кадр");
            assert!(input.focus.is_focused(window));
            assert!(input.bounds.is_some());
        });
    }

    #[gpui::test]
    fn word_navigation(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| TextInput::new("", window, cx));
        window
            .update(cx, |input, _, cx| {
                input.set_text("one two three", cx);
                assert_eq!(input.previous_word(13), 8);
                assert_eq!(input.next_word(0), 3);
                assert_eq!(input.previous_boundary(1), 0);
            })
            .unwrap();
    }
}
