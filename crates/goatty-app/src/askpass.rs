//! goatty as ssh's askpass program: one small window per question.
//!
//! goatty runs ssh with `SSH_ASKPASS` pointing at its own binary (see
//! `goatty_client::askpass`). ssh then starts `goatty "<prompt>"` for each
//! passphrase, password or host key question; `main` notices and calls
//! [`run`] instead of opening a terminal. The answer goes to stdout, the
//! verdict into the exit status, and the process ends.

use goatty_client::askpass::{AskKind, AskRequest};
use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, FocusHandle, Focusable, FontWeight, KeyDownEvent, Render, SharedString, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, button};
use crate::ui::icons::Icon;
use crate::ui::style::Ui;

/// Show the question and exit with the answer.
pub fn run(req: AskRequest) -> ! {
    gpui_platform::application().with_assets(crate::ui::icons::Assets).run(move |cx: &mut App| {
        AppSettings::init(cx);
        let lines = req.prompt.lines().count().max(1) as f32;
        let height = 170. + lines * 20. + if req.kind == AskKind::Secret { 44. } else { 0. };
        let bounds = Bounds::centered(None, size(px(540.), px(height.min(560.))), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("dev.goatty.goatty".into()),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title(title(req.kind));
                cx.new(|cx| {
                    let view = Askpass { req, secret: String::new(), focus: cx.focus_handle() };
                    view.focus.focus(window, cx);
                    view
                })
            },
        );
        if let Err(e) = opened {
            eprintln!("goatty: couldn't show the ssh prompt: {e:#}");
            std::process::exit(1);
        }
        // Closing the window is "no".
        cx.on_window_closed(|_, _| finish(None, 1)).detach();
        cx.activate(true);
    });
    std::process::exit(1)
}

/// Print `answer` (if any) for ssh and exit with `code`.
fn finish(answer: Option<&str>, code: i32) -> ! {
    use std::io::Write;
    if let Some(a) = answer {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{a}");
        let _ = out.flush();
    }
    std::process::exit(code)
}

fn title(kind: AskKind) -> &'static str {
    match kind {
        AskKind::Secret => "ssh needs a passphrase",
        AskKind::HostKey => "Unknown host key",
        AskKind::Confirm => "ssh asks",
        AskKind::Notice => "ssh",
    }
}

struct Askpass {
    req: AskRequest,
    secret: String,
    focus: FocusHandle,
}

impl Focusable for Askpass {
    fn focus_handle(&self, _cx: &App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Askpass {
    fn accept(&self) -> ! {
        match self.req.kind {
            AskKind::Secret => finish(Some(&self.secret), 0),
            AskKind::HostKey => finish(Some("yes"), 0),
            AskKind::Confirm | AskKind::Notice => finish(None, 0),
        }
    }

    fn reject(&self) -> ! {
        finish_reject(self.req.kind)
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<'_, Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        match ks.key.as_str() {
            "enter" => self.accept(),
            "escape" => self.reject(),
            _ if self.req.kind != AskKind::Secret => return,
            "backspace" if m.control => self.secret.clear(),
            "backspace" => {
                self.secret.pop();
            }
            "u" if m.control => self.secret.clear(),
            "v" if m.control => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c: ClipboardItem| c.text()) {
                    self.secret.push_str(text.lines().next().unwrap_or(""));
                }
            }
            _ if m.control || m.alt || m.platform => return,
            _ => match &ks.key_char {
                Some(c) if !c.chars().any(char::is_control) => self.secret.push_str(c),
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for Askpass {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let s = cx.global::<AppSettings>();
        let ui_font = s.ui_font.clone();
        let mono = s.client.font.family.clone().unwrap_or_default();
        let kind = self.req.kind;
        let (icon, color) = match kind {
            AskKind::HostKey => (Icon::TriangleAlert, ui.warning),
            _ => (Icon::Info, ui.accent),
        };
        let prompt = self.req.prompt.trim_end().to_string();
        let prompt_box = div().flex().flex_col().gap_1().children(prompt.lines().map(|l| {
            // Fingerprints and key paths read better in the terminal font.
            let fixed = kind == AskKind::HostKey && (l.contains("SHA256:") || l.contains("fingerprint is"));
            div().when(fixed, |d| d.font_family(mono.clone()).text_xs()).child(SharedString::from(l.to_string()))
        }));
        let field = (kind == AskKind::Secret).then(|| {
            let dots: String = "•".repeat(self.secret.chars().count());
            div()
                .flex()
                .items_center()
                .h(px(32.))
                .px_2()
                .rounded_md()
                .bg(ui.input_bg)
                .border_1()
                .border_color(ui.accent)
                .text_color(ui.text)
                .child(dots)
                .child(div().w(px(1.5)).h(px(16.)).ml(px(1.)).bg(ui.accent))
        });
        let (yes, no) = match kind {
            AskKind::Secret => ("OK", "Cancel"),
            AskKind::HostKey => ("Trust and connect", "Cancel"),
            AskKind::Confirm => ("Yes", "No"),
            AskKind::Notice => ("Close", ""),
        };
        let hint = match kind {
            AskKind::HostKey => "Only continue if you expected a new host or a changed key.",
            AskKind::Secret => "Asked by ssh for goatty's connection. Esc cancels.",
            _ => "",
        };
        let mut buttons = div().flex().items_center().gap_2().child(div().flex_1().text_xs().text_color(ui.muted).child(hint));
        if !no.is_empty() {
            buttons = buttons.child(button("ask-no", no, ButtonKind::Normal, ui, move |_, _| finish_reject(kind)));
        }
        let secret_ok = self.secret.clone();
        buttons = buttons.child(button("ask-yes", yes, ButtonKind::Primary, ui, move |_, _| match kind {
            AskKind::Secret => finish(Some(&secret_ok), 0),
            AskKind::HostKey => finish(Some("yes"), 0),
            _ => finish(None, 0),
        }));

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(ui.bg)
            .font_family(ui_font)
            .text_sm()
            .text_color(ui.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon.svg().size(px(18.)).text_color(color))
                    .child(div().text_base().font_weight(FontWeight::BOLD).child(title(kind))),
            )
            .child(prompt_box)
            .children(field)
            .child(div().flex_1())
            .child(buttons)
    }
}

fn finish_reject(kind: AskKind) -> ! {
    match kind {
        AskKind::HostKey => finish(Some("no"), 1),
        _ => finish(None, 1),
    }
}
