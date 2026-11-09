//! "Close this?": asked before closing a pane or tab whose shell is
//! running a program (#119), so a build, an editor with unsaved changes or
//! an SSH session doesn't end by accident.
//!
//! The [`Workspace`](crate::workspace::Workspace) decides when to ask (the
//! `panes.confirm_close` setting, and a foreground program that isn't a
//! shell) and shows a [`CloseConfirm`] over the window. It emits
//! [`CloseConfirmEvent::Done`]; "Don't ask again" turns the setting off.

use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, Window, actions, div, prelude::*, px};

use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, button, toggle};
use crate::ui::icons::Icon;
use crate::ui::style::Ui;

pub const CONTEXT: &str = "CloseConfirm";

const WIDTH: f32 = 460.;

actions!(close_confirm, [Confirm, Cancel]);

pub fn bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    vec![KeyBinding::new("enter", Confirm, c), KeyBinding::new("escape", Cancel, c)]
}

/// What is being closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closing {
    Pane,
    /// A tab with this many panes.
    Tab {
        panes: usize,
    },
}

pub enum CloseConfirmEvent {
    /// `true`: close it.
    Done(bool),
}

pub struct CloseConfirm {
    closing: Closing,
    /// The running programs (not shells), one per busy pane.
    programs: Vec<String>,
    dont_ask: bool,
    focus: FocusHandle,
}

impl EventEmitter<CloseConfirmEvent> for CloseConfirm {}

impl Focusable for CloseConfirm {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The dialog's title and explanation.
pub fn describe(closing: Closing, programs: &[String]) -> (String, String) {
    let list = match programs {
        [] => "a program".to_string(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    let title = match closing {
        Closing::Pane => "Close this pane?".to_string(),
        Closing::Tab { panes: 1 } => "Close this tab?".to_string(),
        Closing::Tab { panes } => format!("Close this tab and its {panes} panes?"),
    };
    let verb = if programs.len() > 1 { "are" } else { "is" };
    let body = format!("{list} {verb} still running. Closing ends it, and anything it hasn't saved is lost.");
    (title, body)
}

impl CloseConfirm {
    pub fn new(closing: Closing, programs: Vec<String>, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { closing, programs, dont_ask: false, focus }
    }

    fn finish(&mut self, close: bool, cx: &mut Context<'_, Self>) {
        if close && self.dont_ask {
            AppSettings::update(cx, |s| s.panes.confirm_close = false);
        }
        cx.emit(CloseConfirmEvent::Done(close));
    }

    fn confirm(&mut self, _: &Confirm, _window: &mut Window, cx: &mut Context<'_, Self>) {
        self.finish(true, cx);
    }

    fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<'_, Self>) {
        self.finish(false, cx);
    }

    fn done(&self, cx: &mut Context<'_, Self>, close: bool) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            let _ = this.update(cx, |this, cx| this.finish(close, cx));
        }
    }
}

impl Render for CloseConfirm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let ui_font = cx.global::<AppSettings>().ui_font.clone();
        let (title, body) = describe(self.closing, &self.programs);
        let label = match self.closing {
            Closing::Pane => "Close pane",
            Closing::Tab { .. } => "Close tab",
        };
        let this = cx.entity().downgrade();
        let flip = move |_: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |this, cx| {
                this.dont_ask = !this.dont_ask;
                cx.notify();
            });
        };

        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.finish(false, cx)))
            .occlude()
            .w(px(WIDTH))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .font_family(ui_font)
            .text_sm()
            .text_color(ui.text)
            .bg(ui.bg)
            .border_1()
            .border_color(ui.warning.opacity(0.6))
            .rounded_lg()
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::TriangleAlert.svg().size(px(18.)).text_color(ui.warning))
                    .child(div().text_base().font_weight(FontWeight::BOLD).child(title)),
            )
            .child(div().text_color(ui.muted).child(body))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(toggle("close-dont-ask", self.dont_ask, ui, flip))
                    .child(div().text_xs().text_color(ui.muted).child("Don't ask again (Settings → Workspace)")),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(button("close-cancel", "Cancel", ButtonKind::Normal, ui, self.done(cx, false)))
                    .child(button("close-ok", label, ButtonKind::Danger, ui, self.done(cx, true))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_name_the_programs() {
        let p = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (title, body) = describe(Closing::Pane, &p(&["nvim"]));
        assert_eq!(title, "Close this pane?");
        assert!(body.starts_with("nvim is still running."), "{body}");
        let (title, body) = describe(Closing::Tab { panes: 3 }, &p(&["cargo", "ssh", "htop"]));
        assert_eq!(title, "Close this tab and its 3 panes?");
        assert!(body.starts_with("cargo, ssh and htop are still running."), "{body}");
        assert_eq!(describe(Closing::Tab { panes: 1 }, &p(&["make"])).0, "Close this tab?");
    }
}
