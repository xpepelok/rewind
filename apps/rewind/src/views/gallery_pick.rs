use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, Bounds, Context, DispatchPhase, Hsla, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle, Styled, Task,
    UniformListScrollHandle, Window, canvas, div, prelude::*, px,
};

use crate::gallery::GalleryItem;
use crate::theme::ThemeExt;
use crate::views::overlay::Overlay;

const DRAG_THRESHOLD: f32 = 5.0;
const EDGE_INSET: f32 = 14.0;
const SCROLL_GAIN: f32 = 0.32;
const SCROLL_MIN: f32 = 2.0;
const SCROLL_MAX: f32 = 44.0;
const SCROLL_TICK: Duration = Duration::from_millis(16);
const PAINT_STEP: f32 = 6.0;

pub type CardBounds = Rc<RefCell<HashMap<PathBuf, Bounds<Pixels>>>>;

pub struct Picking {
    pub cards: CardBounds,
    pub list: UniformListScrollHandle,
    source: Option<Rc<Vec<GalleryItem>>>,
    order: Vec<PathBuf>,
    anchor: Option<PathBuf>,
    gesture: Option<Gesture>,
    swallow_click: bool,
    autoscroll: Option<Task<()>>,
}

impl Default for Picking {
    fn default() -> Self {
        Self {
            cards: CardBounds::default(),
            list: UniformListScrollHandle::new(),
            source: None,
            order: Vec::new(),
            anchor: None,
            gesture: None,
            swallow_click: false,
            autoscroll: None,
        }
    }
}

struct Gesture {
    origin: Point<Pixels>,
    pointer: Point<Pixels>,
    last: Point<Pixels>,
    start: Option<PathBuf>,
    additive: bool,
    mode: Mode,
}

enum Mode {
    Pending,
    Marquee(Vec<PathBuf>),
    Paint(bool),
}

fn content(position: Point<Pixels>, offset: Point<Pixels>) -> Point<Pixels> {
    Point::new(position.x - offset.x, position.y - offset.y)
}

fn span(a: Point<Pixels>, b: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::from_corners(
        Point::new(a.x.min(b.x), a.y.min(b.y)),
        Point::new(a.x.max(b.x), a.y.max(b.y)),
    )
}

impl Picking {
    pub fn scroll(&self) -> ScrollHandle {
        self.list.0.borrow().base_handle.clone()
    }

    pub fn set_order(&mut self, shown: &Rc<Vec<GalleryItem>>) -> bool {
        if self
            .source
            .as_ref()
            .is_some_and(|source| Rc::ptr_eq(source, shown))
        {
            return false;
        }
        self.cards.borrow_mut().clear();
        self.order = shown.iter().map(|item| item.path.clone()).collect();
        self.source = Some(shown.clone());
        true
    }

    pub fn order(&self) -> &[PathBuf] {
        &self.order
    }

    pub fn forget(&mut self) {
        self.source = None;
        self.order = Vec::new();
        self.cards.take();
    }

    pub fn cancel(&mut self) {
        self.gesture = None;
        self.autoscroll = None;
        self.swallow_click = false;
    }

    pub fn dragging(&self) -> bool {
        self.gesture
            .as_ref()
            .is_some_and(|gesture| !matches!(gesture.mode, Mode::Pending))
    }

    fn card_at(&self, point: Point<Pixels>) -> Option<PathBuf> {
        self.cards
            .borrow()
            .iter()
            .find(|(_, bounds)| bounds.contains(&point))
            .map(|(path, _)| path.clone())
    }

    fn track(&mut self, selected: &mut Vec<PathBuf>, offset: Point<Pixels>) -> bool {
        let Self { cards, gesture, .. } = self;
        let Some(gesture) = gesture.as_mut() else {
            return false;
        };
        let current = content(gesture.pointer, offset);
        let cards = cards.borrow();
        match &gesture.mode {
            Mode::Pending => false,
            Mode::Marquee(base) => {
                let rect = span(gesture.origin, current);
                let mut next = base.clone();
                let mut taken: HashSet<&PathBuf> = base.iter().collect();
                for (path, bounds) in cards.iter() {
                    if bounds.intersects(&rect) && taken.insert(path) {
                        next.push(path.clone());
                    }
                }
                let changed = next != *selected;
                *selected = next;
                changed
            }
            Mode::Paint(target) => {
                let target = *target;
                let from = gesture.last;
                gesture.last = current;
                let dx = f32::from(current.x - from.x);
                let dy = f32::from(current.y - from.y);
                let steps = ((dx.hypot(dy) / PAINT_STEP).ceil() as usize).max(1);
                let mut changed = false;
                for step in 0..=steps {
                    let t = step as f32 / steps as f32;
                    let probe = Point::new(from.x + px(dx * t), from.y + px(dy * t));
                    let Some((path, _)) = cards.iter().find(|(_, bounds)| bounds.contains(&probe))
                    else {
                        continue;
                    };
                    let present = selected.contains(path);
                    if target && !present {
                        selected.push(path.clone());
                        changed = true;
                    } else if !target && present {
                        selected.retain(|other| other != path);
                        changed = true;
                    }
                }
                changed
            }
        }
    }

    fn marquee(&self, offset: Point<Pixels>) -> Option<Bounds<Pixels>> {
        let gesture = self.gesture.as_ref()?;
        matches!(gesture.mode, Mode::Marquee(_)).then(|| {
            let origin = Point::new(gesture.origin.x + offset.x, gesture.origin.y + offset.y);
            span(origin, gesture.pointer)
        })
    }
}

pub fn toggle(selected: &mut Vec<PathBuf>, path: &PathBuf) {
    if let Some(index) = selected.iter().position(|other| other == path) {
        selected.remove(index);
    } else {
        selected.push(path.clone());
    }
}

pub fn extend(selected: &mut Vec<PathBuf>, order: &[PathBuf], anchor: &PathBuf, to: &PathBuf) {
    let from = order.iter().position(|item| item == anchor);
    let until = order.iter().position(|item| item == to);
    let range = match (from, until) {
        (Some(from), Some(until)) => &order[from.min(until)..=from.max(until)],
        _ => std::slice::from_ref(to),
    };
    let taken: HashSet<PathBuf> = selected.iter().cloned().collect();
    selected.extend(range.iter().filter(|path| !taken.contains(*path)).cloned());
}

pub fn card_probe(cards: CardBounds, scroll: ScrollHandle, path: PathBuf) -> impl IntoElement {
    canvas(
        move |bounds, _, _| {
            let mut stored = bounds;
            stored.origin = content(bounds.origin, scroll.offset());
            cards.borrow_mut().insert(path, stored);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

impl Overlay {
    pub fn pick_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let offset = self.picking.scroll().offset();
        let origin = content(event.position, offset);
        self.picking.swallow_click = false;
        let start = self.picking.card_at(origin);
        self.picking.gesture = Some(Gesture {
            origin,
            pointer: event.position,
            last: origin,
            start,
            additive: event.modifiers.control || event.modifiers.shift,
            mode: Mode::Pending,
        });
        cx.notify();
    }

    fn pick_move(
        &mut self,
        position: Point<Pixels>,
        pressed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !pressed {
            self.pick_up(cx);
            return;
        }
        let offset = self.picking.scroll().offset();
        let selected = &self.selected;
        let Some(gesture) = self.picking.gesture.as_mut() else {
            return;
        };
        gesture.pointer = position;
        if matches!(gesture.mode, Mode::Pending) {
            let current = content(position, offset);
            let dx = f32::from(current.x - gesture.origin.x);
            let dy = f32::from(current.y - gesture.origin.y);
            if dx.hypot(dy) < DRAG_THRESHOLD {
                return;
            }
            gesture.mode = match &gesture.start {
                Some(start) => Mode::Paint(!selected.contains(start)),
                None if gesture.additive => Mode::Marquee(selected.clone()),
                None => Mode::Marquee(Vec::new()),
            };
            if let Some(start) = gesture.start.clone() {
                self.picking.anchor = Some(start);
            }
            self.start_autoscroll(window, cx);
        }
        self.picking.track(&mut self.selected, offset);
        cx.notify();
    }

    fn pick_up(&mut self, cx: &mut Context<Self>) {
        let Some(gesture) = self.picking.gesture.take() else {
            return;
        };
        self.picking.autoscroll = None;
        match gesture.mode {
            Mode::Pending => {
                if gesture.start.is_none() && !gesture.additive {
                    self.clear_selection(cx);
                }
            }
            _ => self.picking.swallow_click = true,
        }
        cx.notify();
    }

    fn start_autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking.autoscroll.is_some() {
            return;
        }
        self.picking.autoscroll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(SCROLL_TICK).await;
                let alive = this
                    .update(cx, |overlay, cx| overlay.autoscroll(cx))
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
    }

    fn autoscroll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(gesture) = self.picking.gesture.as_ref() else {
            return false;
        };
        let scroll = self.picking.scroll();
        let viewport = scroll.bounds();
        let y = f32::from(gesture.pointer.y);
        let top = f32::from(viewport.top()) + EDGE_INSET;
        let bottom = f32::from(viewport.bottom()) - EDGE_INSET;
        let overshoot = if y < top {
            y - top
        } else if y > bottom {
            y - bottom
        } else {
            return true;
        };
        let speed =
            (overshoot.abs() * SCROLL_GAIN).clamp(SCROLL_MIN, SCROLL_MAX) * overshoot.signum();
        let offset = scroll.offset();
        let max = f32::from(scroll.max_offset().height);
        let next = (f32::from(offset.y) - speed).clamp(-max, 0.0);
        if (next - f32::from(offset.y)).abs() < 0.01 {
            return true;
        }
        scroll.set_offset(Point::new(offset.x, px(next)));
        self.picking.track(&mut self.selected, scroll.offset());
        cx.notify();
        true
    }

    pub fn pick_click(
        &mut self,
        path: &PathBuf,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        if std::mem::take(&mut self.picking.swallow_click) || self.picking.dragging() {
            return true;
        }
        if modifiers.shift && !self.selected.is_empty() {
            let anchor = self.picking.anchor.clone().unwrap_or_else(|| path.clone());
            extend(&mut self.selected, self.picking.order(), &anchor, path);
        } else if modifiers.control || !self.selected.is_empty() {
            toggle(&mut self.selected, path);
        } else {
            return false;
        }
        self.picking.anchor = Some(path.clone());
        cx.notify();
        true
    }

    pub fn toggle_picked(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        toggle(&mut self.selected, path);
        self.picking.anchor = Some(path.clone());
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected = self.picking.order().to_vec();
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) -> bool {
        self.picking.anchor = None;
        if self.selected.is_empty() {
            return false;
        }
        self.selected.clear();
        cx.notify();
        true
    }

    pub fn everything_selected(&self) -> bool {
        let order = self.picking.order();
        if order.is_empty() || self.selected.len() < order.len() {
            return false;
        }
        let selected: HashSet<&PathBuf> = self.selected.iter().collect();
        order.iter().all(|path| selected.contains(path))
    }

    pub fn pick_listeners(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.picking.gesture.as_ref()?;
        let entity = cx.entity().downgrade();
        Some(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let moving = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let pressed = event.pressed_button == Some(MouseButton::Left);
                        moving
                            .update(cx, |overlay, cx| {
                                overlay.pick_move(event.position, pressed, window, cx)
                            })
                            .ok();
                    });
                    let releasing = entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                            return;
                        }
                        releasing.update(cx, |overlay, cx| overlay.pick_up(cx)).ok();
                    });
                },
            )
            .absolute()
            .size_0()
            .into_any_element(),
        )
    }

    pub fn marquee_layer(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let p = cx.palette();
        let scroll = self.picking.scroll();
        let viewport = scroll.bounds();
        let rect = self.picking.marquee(scroll.offset())?;
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .left(rect.origin.x - viewport.origin.x)
                        .top(rect.origin.y - viewport.origin.y)
                        .w(rect.size.width)
                        .h(rect.size.height)
                        .border_1()
                        .border_color(Hsla {
                            a: 0.85,
                            ..p.accent
                        })
                        .bg(Hsla {
                            a: 0.14,
                            ..p.accent
                        }),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn shift_picks_the_whole_run_between_anchor_and_click() {
        let order = paths(&["a", "b", "c", "d", "e"]);
        let mut selected = paths(&["b"]);
        extend(&mut selected, &order, &order[1], &order[3]);
        assert_eq!(selected, paths(&["b", "c", "d"]));
        extend(&mut selected, &order, &order[3], &order[0]);
        assert_eq!(selected, paths(&["b", "c", "d", "a"]));
    }

    #[test]
    fn toggling_twice_leaves_nothing() {
        let mut selected = Vec::new();
        let path = PathBuf::from("clip.mp4");
        toggle(&mut selected, &path);
        assert_eq!(selected, vec![path.clone()]);
        toggle(&mut selected, &path);
        assert!(selected.is_empty());
    }

    #[test]
    fn a_forgotten_gallery_is_ordered_afresh_on_the_next_visit() {
        let shown = Rc::new(vec![GalleryItem {
            game: String::new(),
            path: PathBuf::from("clip.mp4"),
            preview: None,
            duration: None,
            modified: std::time::SystemTime::UNIX_EPOCH,
            size: 0,
        }]);
        let mut picking = picking_with_grid();
        assert!(picking.set_order(&shown));
        assert!(!picking.set_order(&shown));
        picking.forget();
        assert!(picking.order().is_empty() && picking.cards.borrow().is_empty());
        assert_eq!(Rc::strong_count(&shown), 1);
        assert!(picking.set_order(&shown));
        assert_eq!(picking.order(), paths(&["clip.mp4"]));
    }

    fn picking_with_grid() -> Picking {
        let picking = Picking::default();
        for (index, name) in ["a", "b", "c", "d"].iter().enumerate() {
            let x = (index % 2) as f32 * 110.0;
            let y = (index / 2) as f32 * 70.0;
            picking.cards.borrow_mut().insert(
                PathBuf::from(name),
                Bounds::new(Point::new(px(x), px(y)), gpui::size(px(100.0), px(60.0))),
            );
        }
        picking
    }

    #[test]
    fn a_marquee_picks_every_card_it_touches() {
        let mut picking = picking_with_grid();
        picking.gesture = Some(Gesture {
            origin: Point::new(px(105.0), px(5.0)),
            pointer: Point::new(px(150.0), px(100.0)),
            last: Point::default(),
            start: None,
            additive: false,
            mode: Mode::Marquee(Vec::new()),
        });
        let mut selected = Vec::new();
        assert!(picking.track(&mut selected, Point::default()));
        selected.sort();
        assert_eq!(selected, paths(&["b", "d"]));
    }

    #[test]
    fn painting_from_a_picked_card_unpicks_what_it_crosses() {
        let mut picking = picking_with_grid();
        picking.gesture = Some(Gesture {
            origin: Point::new(px(10.0), px(10.0)),
            pointer: Point::new(px(120.0), px(10.0)),
            last: Point::new(px(10.0), px(10.0)),
            start: Some(PathBuf::from("a")),
            additive: false,
            mode: Mode::Paint(false),
        });
        let mut selected = paths(&["a", "b", "c"]);
        assert!(picking.track(&mut selected, Point::default()));
        assert_eq!(selected, paths(&["c"]));
    }
}
