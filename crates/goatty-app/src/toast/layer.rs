//! [`ToastLayer`]: draws one window's toasts in its bottom-right corner and
//! runs their timers.
//!
//! A window's root view creates one and renders it as its last child (it
//! positions itself absolutely). Nothing in it is focusable, so the terminal
//! keeps the keyboard. Each card occludes the terminal below it (no
//! selections through a toast); the space between cards does not.
//!
//! Timers are one GPUI task for the earliest deadline, so a window with
//! toasts but no change in sight draws no frames. Cards animate in and out
//! with a spring (opacity, a short slide, and height, so the others move
//! smoothly); `reduce_motion` turns that off.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Context, FontWeight, Hsla, IntoElement, Render, SharedString, SpringAnimation, SpringConfig, Task,
    Transformation, Window, div, percentage, prelude::*, px,
};

use super::stack::Stack;
use super::{Level, Toast, ToastId};
use crate::settings::AppSettings;
use crate::ui::icons::Icon;
use crate::ui::style::Ui;

const WIDTH: f32 = 360.;
const MARGIN: f32 = 16.;
const GAP: f32 = 8.;
/// Taller than any card, for the height animation (cards grow from 0 to
/// their natural height while the spring is below 1).
const MAX_ANIMATED_HEIGHT: f32 = 240.;
/// Slide distance when entering and leaving.
const SLIDE: f32 = 28.;
/// Lightly underdamped: a hint of overshoot, settled in ~0.3 s.
const SPRING: SpringConfig = SpringConfig::new(420., 34., 1.);

pub struct ToastLayer {
    stack: Stack,
    /// The card under the pointer (pauses the timers).
    hovered: Option<ToastId>,
    window_active: bool,
    timer: Option<Task<()>>,
    _subscription: gpui::Subscription,
}

impl ToastLayer {
    /// A layer for `window`, registered so [`super::show`] finds it.
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let me = cx.entity().downgrade();
        let pending = super::register(cx, window.window_handle(), me);
        let sub = cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            this.sync_pause(cx);
        });
        let mut this =
            Self { stack: Stack::default(), hovered: None, window_active: window.is_window_active(), timer: None, _subscription: sub };
        let now = Instant::now();
        this.stack.set_paused(!this.window_active, now);
        for (id, toast) in pending {
            this.stack.push(id, toast, now);
        }
        this.changed(cx);
        this
    }

    pub(super) fn window_active(&self) -> bool {
        self.window_active
    }

    pub(super) fn contains(&self, id: ToastId) -> bool {
        self.stack.contains(id)
    }

    pub(super) fn show(&mut self, id: ToastId, toast: Toast, cx: &mut Context<'_, Self>) {
        self.stack.push(id, toast, Instant::now());
        self.changed(cx);
    }

    pub(super) fn update(&mut self, id: ToastId, toast: Toast, cx: &mut Context<'_, Self>) {
        if !self.stack.update(id, toast.clone(), Instant::now()) {
            self.stack.push(id, toast, Instant::now());
        }
        self.changed(cx);
    }

    pub(super) fn dismiss(&mut self, id: ToastId, cx: &mut Context<'_, Self>) {
        if self.stack.contains(id) {
            self.stack.dismiss(id, Instant::now());
            self.changed(cx);
        }
    }

    pub(super) fn dismiss_all(&mut self, cx: &mut Context<'_, Self>) {
        if !self.stack.is_empty() {
            self.stack.dismiss_all(Instant::now());
            self.changed(cx);
        }
    }

    fn set_hovered(&mut self, id: ToastId, hovered: bool, cx: &mut Context<'_, Self>) {
        if hovered {
            self.hovered = Some(id);
        } else if self.hovered == Some(id) {
            self.hovered = None;
        }
        self.sync_pause(cx);
    }

    fn sync_pause(&mut self, cx: &mut Context<'_, Self>) {
        self.stack.set_paused(self.hovered.is_some() || !self.window_active, Instant::now());
        self.changed(cx);
    }

    /// Expire what's due, redraw, and wait for the next deadline.
    fn changed(&mut self, cx: &mut Context<'_, Self>) {
        let now = Instant::now();
        self.stack.advance(now);
        if self.hovered.is_some_and(|id| !self.stack.contains(id)) {
            // The hovered card went away under the pointer (dismissed).
            self.hovered = None;
            self.stack.set_paused(!self.window_active, now);
        }
        self.timer = self.stack.next_wake().map(|at| {
            let wait = at.saturating_duration_since(now) + Duration::from_millis(1);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| this.changed(cx));
            })
        });
        cx.notify();
    }

    fn render_card(&self, ix: usize, ui: Ui, cx: &mut Context<'_, Self>) -> AnyElement {
        let e = &self.stack.entries()[ix];
        let (id, toast, count, leaving) = (e.id, &e.toast, e.count, e.leaving());
        let color = level_color(toast.level, ui);
        let group = SharedString::from(format!("toast-{}", id.0));
        let sticky = toast.visible_for().is_none();
        let border = match toast.level {
            Level::Warning | Level::Error => color.opacity(0.45),
            _ => ui.border,
        };

        let icon = div().size(px(26.)).flex_none().rounded_full().bg(color.opacity(0.14)).flex().items_center().justify_center().child(
            match toast.level {
                Level::Loading => Icon::Loader
                    .svg()
                    .size(px(15.))
                    .text_color(color)
                    .with_animation(("toast-spin", id.0), Animation::new(Duration::from_millis(900)).repeat(), |s, t| {
                        s.with_transformation(Transformation::rotate(percentage(t)))
                    })
                    .into_any_element(),
                level => level_icon(level).svg().size(px(15.)).text_color(color).into_any_element(),
            },
        );

        let title = div()
            .flex()
            .items_center()
            .gap_2()
            .min_h(px(26.))
            .child(div().font_weight(FontWeight::MEDIUM).child(toast.title.clone()))
            .when(count > 1, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .rounded_full()
                        .bg(ui.surface_hover)
                        .text_xs()
                        .text_color(ui.muted)
                        .child(format!("×{count}")),
                )
            });

        let actions = (!toast.actions.is_empty()).then(|| {
            div().flex().flex_wrap().gap_2().mt_2().children(toast.actions.iter().enumerate().map(|(i, a)| {
                let run = a.run.clone();
                let layer = cx.entity().downgrade();
                div()
                    .id(("toast-action", id.0 * 8 + i as u64))
                    .px_2p5()
                    .py_0p5()
                    .rounded_md()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(i == 0, |d| d.bg(ui.accent).text_color(ui.on_accent).hover(|s| s.opacity(0.9)))
                    .when(i > 0, |d| d.border_1().border_color(ui.border).hover(|s| s.bg(ui.surface_hover)))
                    .child(a.label.clone())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        // Not inside `layer.update`: the handler may show toasts.
                        run(window, cx);
                        if let Some(layer) = layer.upgrade() {
                            layer.update(cx, |l, cx| l.dismiss(id, cx));
                        }
                    })
            }))
        });

        let close = div()
            .id(("toast-close", id.0))
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .size(px(22.))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(ui.muted)
            .when(!sticky, |d| d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)))
            .hover(|s| s.bg(ui.surface_hover).text_color(ui.text))
            .child(Icon::Close.svg().size(px(12.)).text_color(ui.muted))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.dismiss(id, cx);
            }));

        let card = div()
            .id(("toast-card", id.0))
            .group(group)
            .relative()
            .occlude()
            .w_full()
            .flex()
            .items_start()
            .gap_3()
            .pl_3()
            .pr_10()
            .py_2p5()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(ui.surface)
            .shadow_lg()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.set_hovered(id, *hovered, cx)))
            .child(icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(title)
                    .when_some(toast.body.clone(), |d, body| d.child(div().text_xs().text_color(ui.muted).child(body)))
                    .children(actions),
            )
            .child(close);

        let target = if leaving { 0f32 } else { 1. };
        div()
            .id(("toast", id.0))
            .w_full()
            .flex_none()
            .child(card)
            .with_spring(("toast-motion", id.0), SpringAnimation::new(SPRING).to(target).from(0.).with_epsilon(0.002), |d, v| {
                let shown = v.clamp(0., 1.);
                let d = d.opacity(shown).left(px((1. - v) * SLIDE)).pt(px(GAP * shown));
                // Settled: natural height, and the card's shadow isn't clipped.
                if v < 0.995 { d.overflow_hidden().max_h(px(MAX_ANIMATED_HEIGHT * v.max(0.))) } else { d }
            })
            .into_any_element()
    }
}

fn level_icon(level: Level) -> Icon {
    match level {
        Level::Info => Icon::Info,
        Level::Success => Icon::CircleCheck,
        Level::Warning => Icon::TriangleAlert,
        Level::Error => Icon::CircleX,
        Level::Loading => Icon::Loader,
    }
}

fn level_color(level: Level, ui: Ui) -> Hsla {
    match level {
        Level::Info | Level::Loading => ui.accent,
        Level::Success => ui.success,
        Level::Warning => ui.warning,
        Level::Error => ui.danger,
    }
}

impl Render for ToastLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if self.stack.is_empty() {
            return div().into_any_element();
        }
        let ui = Ui::get(cx);
        let s = cx.global::<AppSettings>();
        let ui_font = s.ui_font.clone();
        let cards: Vec<AnyElement> = (0..self.stack.entries().len()).map(|ix| self.render_card(ix, ui, cx)).collect();
        div()
            .absolute()
            .bottom(px(MARGIN))
            .right(px(MARGIN))
            .w(px(WIDTH))
            .max_w(window.viewport_size().width - px(2. * MARGIN))
            .flex()
            .flex_col()
            .font_family(ui_font)
            .text_sm()
            .text_color(ui.text)
            .children(cards)
            .into_any_element()
    }
}
