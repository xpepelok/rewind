use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Context, FontWeight, IntoElement, MouseButton, ObjectFit, Render,
    Stateful, Window, WindowControlArea, div, ease_in_out, img, linear_color_stop, linear_gradient,
    prelude::*, px, relative,
};

use crate::assets::{Icon, icon};
use crate::platform::system;
use crate::theme::{FONT_FAMILY, Palette, SMALL_TEXT, ThemeExt, alpha, rgb, rgba};
use gpui::Hsla;
use crate::ui::strong;

const TITLEBAR: f32 = 40.0;
const LOGO_MARK: f32 = 22.0;
const CONTROL: f32 = 28.0;
const HERO_W: f32 = 464.0;
const HERO_H: f32 = 781.0;
const TEXT_W: f32 = 468.0;
const SLIDE_INTERVAL: Duration = Duration::from_millis(4000);

struct Slide {
    shot: &'static str,
    head: &'static str,
    body: &'static str,
}

const SLIDES: &[Slide] = &[
    Slide {
        shot: "shots/home.png",
        head: "Мгновенный повтор",
        body: "Rewind постоянно держит в памяти последние минуты игры. Увидел крутой момент — жми горячую клавишу, и клип сохранится задним числом, даже если ты не включал запись заранее. А если видеодрайвер моргнёт, буфер сам перезапустится и продолжит писать — запись не оборвётся незаметно.",
    },
    Slide {
        shot: "shots/filters.png",
        head: "Фильтры и профили",
        body: "Накладывай цветовые фильтры прямо поверх игры — сделай картинку теплее, контрастнее или кинематографичнее. Для каждой игры сохраняется до трёх профилей, между которыми можно переключаться на лету, не открывая настройки заново.",
    },
    Slide {
        shot: "shots/photo.png",
        head: "Фоторежим",
        body: "Фоторежим ставит экшен на паузу и даёт свободную камеру: облети сцену, выбери ракурс и поймай чистый кадр без интерфейса и подсказок. Отличный способ собрать скриншоты, обои и обложки прямо из игры.",
    },
    Slide {
        shot: "shots/highlights.png",
        head: "Лучшие моменты",
        body: "Rewind сам ловит важные события по протоколу игры — убийства, серии, победные раунды — и нарезает их в отдельные клипы. Все хайлайты складываются в галерею, где ты одним кликом оставляешь лучшее и удаляешь лишнее.",
    },
    Slide {
        shot: "shots/settings.png",
        head: "Всё под себя",
        body: "Качество, битрейт, кодек, длина буфера и горячие клавиши — всё настраивается под твою систему и привычки. Нужен максимум качества или минимум нагрузки на игру — Rewind подстроится, а не навяжет своё.",
    },
    Slide {
        shot: "shots/appearance.png",
        head: "Темы оформления",
        body: "Четыре продуманные темы — Classic, Carbon, Ruby и Light — меняют не только цвета, но и формы кнопок, скругления и иконки. Выбери вид, который приятнее именно тебе, и весь интерфейс мгновенно перестроится под него.",
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Install,
    Reinstall,
    Update,
    Uninstall,
}

impl Job {
    fn flag(self) -> &'static str {
        match self {
            Self::Uninstall => system::UNINSTALL_FLAG,
            _ => system::INSTALL_FLAG,
        }
    }

    fn progress(self) -> &'static str {
        match self {
            Self::Install => "Устанавливаю Rewind…",
            Self::Reinstall => "Переустанавливаю…",
            Self::Update => "Обновляю…",
            Self::Uninstall => "Удаляю…",
        }
    }

    fn done_ok(self) -> &'static str {
        match self {
            Self::Install => "Готово — Rewind установлен",
            Self::Reinstall => "Готово — Rewind переустановлен",
            Self::Update => "Готово — Rewind обновлён",
            Self::Uninstall => "Готово — Rewind удалён",
        }
    }

    fn done_failed(self) -> &'static str {
        match self {
            Self::Uninstall => "Не удалось удалить",
            _ => "Не удалось установить",
        }
    }
}

enum Phase {
    Choose,
    Working(Job),
    Done { job: Job, ok: bool },
}

pub struct Installer {
    installed: bool,
    version: Option<String>,
    phase: Phase,
    slide: usize,
    prev: usize,
    ticking: bool,
    manual: bool,
    progress: f32,
}

impl Installer {
    const CURRENT: &'static str = env!("CARGO_PKG_VERSION");

    pub fn new() -> Self {
        Self {
            installed: system::is_installed(),
            version: system::installed_version(),
            phase: Phase::Choose,
            slide: 0,
            prev: 0,
            ticking: false,
            manual: false,
            progress: 0.0,
        }
    }

    fn refresh(&mut self) {
        self.installed = system::is_installed();
        self.version = system::installed_version();
    }

    fn start_carousel(&mut self, cx: &mut Context<Self>) {
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SLIDE_INTERVAL).await;
                let alive = this
                    .update(cx, |installer, cx| {
                        if matches!(installer.phase, Phase::Choose) && !installer.manual {
                            installer.prev = installer.slide;
                            installer.slide = (installer.slide + 1) % SLIDES.len();
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    fn run(&mut self, job: Job, cx: &mut Context<Self>) {
        self.phase = Phase::Working(job);
        self.progress = 0.0;
        cx.notify();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(110))
                    .await;
                let going = this
                    .update(cx, |installer, cx| {
                        if matches!(installer.phase, Phase::Working(_)) {
                            installer.progress = (installer.progress + 0.06).min(0.92);
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();

        let flag = job.flag();
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let ok = cx
                .background_executor()
                .spawn(async move { system::perform(flag) })
                .await;
            let min = Duration::from_millis(1600);
            if let Some(rest) = min.checked_sub(started.elapsed()) {
                cx.background_executor().timer(rest).await;
            }
            this.update(cx, |installer, cx| {
                installer.refresh();
                installer.progress = 1.0;
                installer.phase = Phase::Done { job, ok };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.refresh();
        self.phase = Phase::Choose;
        cx.notify();
    }

    fn go(&mut self, delta: i32, cx: &mut Context<Self>) {
        let n = SLIDES.len() as i32;
        self.manual = true;
        self.prev = self.slide;
        self.slide = (((self.slide as i32 + delta) % n + n) % n) as usize;
        cx.notify();
    }

    fn jump(&mut self, index: usize, cx: &mut Context<Self>) {
        self.manual = true;
        self.prev = self.slide;
        self.slide = index;
        cx.notify();
    }

}

fn version_key(value: &str) -> Vec<u64> {
    value
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn mark(size: f32, radius: f32) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(radius))
        .bg(linear_gradient(
            160.0,
            linear_color_stop(rgb(0x2b2b2b), 0.0),
            linear_color_stop(rgb(0x151515), 1.0),
        ))
        .border_1()
        .border_color(rgb(0x070707))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(Icon::Replay, size * 0.6, rgb(0x8acb14)))
}

fn nav_btn(id: &'static str, glyph: Icon, delta: i32, cx: &mut Context<Installer>) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(38.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x000000, 0.42))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0x000000, 0.72)))
        .child(icon(glyph, 18.0, rgb(0xffffff)))
        .on_click(cx.listener(move |installer, _, _, cx| installer.go(delta, cx)))
}

fn control(id: &'static str, glyph: Icon, danger: bool, p: &Palette) -> Stateful<gpui::Div> {
    let hover = if danger { p.record } else { p.hover };
    div()
        .id(id)
        .w(px(CONTROL + 8.0))
        .h(px(CONTROL))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(icon(glyph, 15.0, p.icon))
}

fn cta(
    id: &'static str,
    text: impl Into<gpui::SharedString>,
    bg: Hsla,
    hover: Hsla,
    fg: Hsla,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .w_full()
        .h(px(50.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(bg)
        .text_size(px(15.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(fg)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(text.into())
}

impl Installer {
    fn titlebar(&self, p: &Palette) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(TITLEBAR))
            .flex()
            .items_center()
            .pl(px(12.0))
            .pr(px(7.0))
            .bg(p.header)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .window_control_area(WindowControlArea::Drag)
                    .child(mark(LOGO_MARK, 6.0))
                    .child(strong("Rewind", 14.0, p.title)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        control("min", Icon::Minus, false, p)
                            .on_click(|_, window, _| window.minimize_window()),
                    )
                    .child(
                        control("close", Icon::Close, true, p)
                            .on_click(|_, _window, _| std::process::exit(0)),
                    ),
            )
    }

    fn actions(&self, p: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        match &self.phase {
            Phase::Choose if !self.installed => div().w_full().child(
                cta("install", "Установить", p.accent, p.accent_hover, p.check_mark)
                    .on_click(cx.listener(|installer, _, _, cx| installer.run(Job::Install, cx))),
            ),
            Phase::Choose => {
                let current = version_key(Self::CURRENT);
                let (label, job) = match self.version.as_deref() {
                    Some(v) if version_key(v) < current => {
                        (format!("Обновить до {}", Self::CURRENT), Job::Update)
                    }
                    Some(v) if version_key(v) > current => {
                        (format!("Поставить {}", Self::CURRENT), Job::Reinstall)
                    }
                    _ => ("Переустановить".to_owned(), Job::Reinstall),
                };
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        cta("uninstall", "Деинсталлировать", p.record, rgb(0xf04545), p.white)
                            .flex_1()
                            .on_click(
                                cx.listener(|installer, _, _, cx| installer.run(Job::Uninstall, cx)),
                            ),
                    )
                    .child(
                        cta("main", label, p.accent, p.accent_hover, p.check_mark)
                            .flex_1()
                            .on_click(cx.listener(move |installer, _, _, cx| installer.run(job, cx))),
                    )
            }
            Phase::Working(_) => div(),
            Phase::Done { ok, job } => {
                let (ok, job) = (*ok, *job);
                let mut row = div().w_full().flex().items_center().gap(px(8.0));
                if !ok {
                    row = row.child(
                        cta("retry", "Повторить", p.accent, p.accent_hover, p.check_mark)
                            .flex_1()
                            .on_click(cx.listener(|installer, _, _, cx| installer.back(cx))),
                    );
                }
                row.child(
                    cta("done", "Готово", p.field, rgb(0x4a4a4a), p.text)
                        .flex_1()
                        .on_click(move |_, _window, _cx| {
                            if ok && job != Job::Uninstall {
                                let _ = system::start_elevated();
                            }
                            std::process::exit(0)
                        }),
                )
            }
        }
    }
}

impl Render for Installer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.ticking {
            self.start_carousel(cx);
        }
        let p = cx.palette();
        let choosing = matches!(self.phase, Phase::Choose);
        let working = matches!(self.phase, Phase::Working(_));
        let slide = &SLIDES[self.slide];

        let (head, body): (String, String) = match &self.phase {
            Phase::Choose => (slide.head.to_owned(), slide.body.to_owned()),
            Phase::Working(job) => (
                job.progress().to_owned(),
                "Подтвердите запрос контроля учётных записей (UAC) и подождите пару секунд."
                    .to_owned(),
            ),
            Phase::Done { job, ok } => {
                let job = *job;
                if *ok {
                    (
                        job.done_ok().to_owned(),
                        "Оверлей открывается по Alt+Z поверх игры.".to_owned(),
                    )
                } else {
                    (
                        job.done_failed().to_owned(),
                        "Добавьте папку установки в исключения Defender и попробуйте снова."
                            .to_owned(),
                    )
                }
            }
        };
        let head_color = match &self.phase {
            Phase::Done { ok: false, .. } => p.record,
            _ => p.title,
        };

        let pct = (self.progress * 100.0).round() as u32;
        let (bar_label, bar_fill, bar_color) = match &self.phase {
            Phase::Choose => ("Готов к установке".to_owned(), 0.0_f32, p.accent),
            Phase::Working(_) => (format!("Установка… {pct}%"), self.progress, p.accent),
            Phase::Done { ok: true, .. } => ("Установлено".to_owned(), 1.0, p.accent),
            Phase::Done { ok: false, .. } => ("Не удалось".to_owned(), 1.0, p.record),
        };

        let dots: Vec<_> = (0..SLIDES.len()).map(|index| {
            let active = index == self.slide;
            div()
                .id(("dot", index))
                .flex_none()
                .size(px(7.0))
                .rounded_full()
                .bg(if active { p.accent } else { p.dots })
                .cursor_pointer()
                .on_click(cx.listener(move |installer, _, _, cx| installer.jump(index, cx)))
        }).collect();

        let nav = choosing.then(|| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_between()
                .px(px(10.0))
                .child(nav_btn("nav-prev", Icon::ChevronLeft, -1, cx))
                .child(nav_btn("nav-next", Icon::ChevronRight, 1, cx))
        });

        let fade = Animation::new(Duration::from_millis(360)).with_easing(ease_in_out);
        let hero = div()
            .id("hero-shot")
            .flex_none()
            .w(px(HERO_W))
            .h(px(HERO_H))
            .relative()
            .overflow_hidden()
            .rounded(px(10.0))
            .bg(p.panel)
            .border_1()
            .border_color(p.divider)
            .child(
                img(SLIDES[self.prev].shot)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_animation(("shot-out", self.slide), fade.clone(), |el, t| {
                        el.opacity(1.0 - t)
                    }),
            )
            .child(
                img(slide.shot)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_animation(("shot-in", self.slide), fade.clone(), |el, t| el.opacity(t)),
            )
            .children(nav);

        let caption = div()
            .w(px(TEXT_W))
            .flex()
            .flex_col()
            .justify_start()
            .gap(px(16.0))
            .child(
                div()
                    .text_size(px(30.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .line_height(px(36.0))
                    .text_color(head_color)
                    .child(head),
            )
            .child(
                div()
                    .max_w(px(500.0))
                    .text_size(px(15.0))
                    .text_color(p.muted)
                    .line_height(px(25.0))
                    .child(body),
            )
            .with_animation(("cap", self.slide), fade.clone(), |el, t| el.opacity(t));

        let shot_col = div()
            .flex_none()
            .w(px(HERO_W))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .child(hero)
            .children(choosing.then(|| div().flex().gap(px(6.0)).children(dots)));

        let main_row = div()
            .flex_1()
            .flex()
            .flex_row()
            .gap(px(48.0))
            .px(px(60.0))
            .pt(px(40.0))
            .pb(px(20.0))
            .child(caption)
            .child(shot_col);

        let progress = div()
            .w(px(TEXT_W))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(SMALL_TEXT))
                            .text_color(p.muted)
                            .child(bar_label),
                    )
                    .child(
                        div()
                            .text_size(px(SMALL_TEXT))
                            .text_color(p.muted)
                            .child(if working {
                                format!("{pct}%")
                            } else {
                                String::new()
                            }),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(6.0))
                    .rounded_full()
                    .bg(p.track)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(bar_fill))
                            .rounded_full()
                            .bg(bar_color),
                    ),
            );

        let bottom = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(48.0))
            .px(px(60.0))
            .pb(px(34.0))
            .child(progress)
            .child(div().flex_none().w(px(HERO_W)).child(self.actions(&p, cx)));

        let content = div()
            .flex_1()
            .flex()
            .flex_col()
            .child(main_row)
            .child(bottom);

        div()
            .font_family(FONT_FAMILY)
            .font_weight(FontWeight::NORMAL)
            .size_full()
            .bg(p.panel)
            .text_color(p.text)
            .flex()
            .flex_col()
            .child(self.titlebar(&p))
            .child(div().flex_none().h(px(1.0)).bg(alpha(p.white, 0.06)))
            .child(content)
    }
}
