//! "Paste this?": the dialog for pastes that deserve a second look (#29).
//!
//! The terminal checks every paste (`goatty_term::paste::check`) and, when
//! the `paste.confirm` setting says so, hands it to the
//! [`Workspace`](crate::workspace::Workspace) as a [`PasteRequest`]. The
//! workspace shows a [`PasteConfirm`] over the pane; it explains what's
//! risky, previews the text with control and invisible characters made
//! visible, and emits [`PasteConfirmEvent::Done`] with what to paste (the
//! text as is, cleaned, or joined into one line) or `None`.

use goatty_term::paste::{self, PasteRisk};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, SharedString, Window, actions, div, prelude::*, px,
};

use crate::settings::AppSettings;
use crate::terminal::PasteRequest;
use crate::ui::controls::{ButtonKind, button};
use crate::ui::icons::Icon;
use crate::ui::style::Ui;

pub const CONTEXT: &str = "PasteConfirm";

const WIDTH: f32 = 640.;
const PREVIEW_LINES: usize = 12;
const PREVIEW_COLS: usize = 160;

actions!(paste_confirm, [Confirm, Cancel]);

pub fn bindings() -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    vec![KeyBinding::new("enter", Confirm, c), KeyBinding::new("escape", Cancel, c)]
}

pub enum PasteConfirmEvent {
    /// Paste this (`None`: don't paste).
    Done(Option<String>),
}

pub struct PasteConfirm {
    request: PasteRequest,
    focus: FocusHandle,
    /// Control or invisible characters that "Paste without them" removes.
    unsafe_chars: usize,
    lines: usize,
}

impl EventEmitter<PasteConfirmEvent> for PasteConfirm {}

impl Focusable for PasteConfirm {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// One risk as a heading and an explanation; `true` for the serious ones.
pub fn describe(risk: &PasteRisk) -> (String, &'static str, bool) {
    let n = |count: usize, one: &str, many: &str| if count == 1 { format!("1 {one}") } else { format!("{count} {many}") };
    match *risk {
        PasteRisk::RunsCommands { commands } => (
            format!("Runs {} right away", n(commands, "command", "commands")),
            "The program in this terminal didn't turn on bracketed paste, so every line break presses Enter.",
            true,
        ),
        PasteRisk::DownloadToShell => (
            "Pipes a download into a shell".into(),
            "Whatever the server sends runs on this computer. Check where the URL points first.",
            true,
        ),
        PasteRisk::ControlChars { count } => (
            format!("Contains {}", n(count, "control character", "control characters")),
            "Escape sequences can rewrite the screen or act as keystrokes. They're marked in the preview.",
            true,
        ),
        PasteRisk::HiddenChars { count } => (
            format!("Contains {}", n(count, "invisible character", "invisible characters")),
            "Bidi overrides and zero-width spaces can make text read differently from what runs.",
            true,
        ),
        PasteRisk::Large { bytes } => (format!("Large paste ({})", size(bytes)), "The program may take a while to take it all in.", false),
        PasteRisk::Multiline { lines } => {
            (format!("{lines} lines"), "Nothing runs until you press Enter; you can still edit the command first.", false)
        }
    }
}

fn size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 { format!("{:.1} MiB", bytes as f64 / (1024. * 1024.)) } else { format!("{} KiB", bytes.div_ceil(1024)) }
}

/// A piece of a preview line; `marked` pieces stand for characters that
/// don't show (or shouldn't run) as themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub marked: bool,
}

/// The first lines of `text` for display: control characters as their
/// symbols (␛), invisible ones as `<U+202E>`, long lines cut. Returns the
/// lines and how many more there are.
pub fn preview(text: &str, max_lines: usize, max_cols: usize) -> (Vec<Vec<Piece>>, usize) {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let body = body.strip_suffix('\r').unwrap_or(body);
    let all: Vec<&str> = body.split('\n').collect();
    let lines = all
        .iter()
        .take(max_lines)
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut pieces: Vec<Piece> = Vec::new();
            let mut cols = 0;
            for c in line.chars() {
                if cols >= max_cols {
                    pieces.push(Piece { text: "…".into(), marked: false });
                    break;
                }
                let (shown, marked) = match c {
                    '\t' => ("⇥".to_string(), false),
                    '\r' => ("␍".to_string(), false),
                    '\x7f' => ("␡".to_string(), true),
                    c if (c as u32) < 0x20 => (char::from_u32(0x2400 + c as u32).unwrap_or('?').to_string(), true),
                    c if paste::is_control(c) || paste::is_hidden(c) => (format!("<U+{:04X}>", c as u32), true),
                    c => (c.to_string(), false),
                };
                cols += shown.chars().count();
                match pieces.last_mut() {
                    Some(p) if p.marked == marked && !marked => p.text.push_str(&shown),
                    _ => pieces.push(Piece { text: shown, marked }),
                }
            }
            pieces
        })
        .collect();
    (lines, all.len().saturating_sub(max_lines))
}

impl PasteConfirm {
    pub fn new(request: PasteRequest, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let unsafe_chars = request.text.chars().filter(|&c| paste::is_control(c) || paste::is_hidden(c)).count();
        let lines = paste::line_count(&request.text);
        Self { request, focus, unsafe_chars, lines }
    }

    /// What Enter does: strip unsafe characters if there are any.
    fn primary(&self) -> String {
        if self.unsafe_chars > 0 { paste::clean(&self.request.text) } else { self.request.text.clone() }
    }

    fn confirm(&mut self, _: &Confirm, _window: &mut Window, cx: &mut Context<'_, Self>) {
        cx.emit(PasteConfirmEvent::Done(Some(self.primary())));
    }

    fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<'_, Self>) {
        cx.emit(PasteConfirmEvent::Done(None));
    }

    fn done(&self, cx: &mut Context<'_, Self>, text: Option<String>) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            let text = text.clone();
            let _ = this.update(cx, |_, cx| cx.emit(PasteConfirmEvent::Done(text)));
        }
    }
}

impl Render for PasteConfirm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let app = cx.global::<AppSettings>();
        let ui_font = app.ui_font.clone();
        let mono: SharedString = app.client.font.family.clone().unwrap_or_else(|| "monospace".into()).into();
        let serious = self.request.risks.iter().any(|r| describe(r).2);
        let title = if self.lines > 1 { format!("Paste {} lines?", self.lines) } else { "Paste this?".to_string() };
        let (lines, more) = preview(&self.request.text, PREVIEW_LINES, PREVIEW_COLS);

        let risks = self.request.risks.iter().map(|risk| {
            let (heading, detail, severe) = describe(risk);
            let color = if severe { ui.warning } else { ui.muted };
            div().flex().gap_2().child(Icon::TriangleAlert.svg().size(px(16.)).mt(px(2.)).flex_shrink_0().text_color(color)).child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_color(ui.text).font_weight(FontWeight::SEMIBOLD).child(heading))
                    .child(div().text_xs().text_color(ui.muted).child(detail)),
            )
        });

        let preview_box = div()
            .flex()
            .flex_col()
            .p_2()
            .rounded_md()
            .bg(ui.input_bg)
            .border_1()
            .border_color(ui.border)
            .font_family(mono)
            .text_xs()
            .overflow_hidden()
            .children(lines.into_iter().map(|pieces| {
                div().flex().min_h(px(16.)).overflow_hidden().whitespace_nowrap().children(pieces.into_iter().map(|p| {
                    if p.marked {
                        div().px(px(1.)).rounded_sm().bg(ui.danger.opacity(0.2)).text_color(ui.danger).child(p.text)
                    } else {
                        div().text_color(ui.text).child(p.text)
                    }
                }))
            }))
            .when(more > 0, |d| {
                d.child(div().pt_1().text_color(ui.muted).child(if more == 1 {
                    "… 1 more line".to_string()
                } else {
                    format!("… {more} more lines")
                }))
            });

        let mut buttons = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().text_xs().text_color(ui.muted).child("Settings → Terminal decides when goatty asks"));
        buttons = buttons.child(button("paste-cancel", "Cancel", ButtonKind::Normal, ui, self.done(cx, None)));
        if self.lines > 1 {
            let one = paste::join_lines(&paste::clean(&self.request.text));
            buttons = buttons.child(button("paste-one-line", "Paste as one line", ButtonKind::Normal, ui, self.done(cx, Some(one))));
        }
        if self.unsafe_chars > 0 {
            let raw = Some(self.request.text.clone());
            buttons = buttons.child(button("paste-raw", "Paste as is", ButtonKind::Danger, ui, self.done(cx, raw)));
            let label = if self.unsafe_chars == 1 {
                "Remove 1 character and paste".to_string()
            } else {
                format!("Remove {} characters and paste", self.unsafe_chars)
            };
            buttons = buttons.child(button("paste-clean", label, ButtonKind::Primary, ui, self.done(cx, Some(self.primary()))));
        } else {
            buttons = buttons.child(button("paste-ok", "Paste", ButtonKind::Primary, ui, self.done(cx, Some(self.primary()))));
        }

        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PasteConfirmEvent::Done(None))))
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
            .border_color(if serious { ui.warning.opacity(0.6) } else { ui.border })
            .rounded_lg()
            .shadow_lg()
            .child(div().text_base().font_weight(FontWeight::BOLD).child(title))
            .child(div().flex().flex_col().gap_2().children(risks))
            .child(preview_box)
            .child(buttons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(line: &[Piece]) -> String {
        line.iter().map(|p| if p.marked { format!("[{}]", p.text) } else { p.text.clone() }).collect()
    }

    #[test]
    fn preview_marks_what_does_not_show() {
        let (lines, more) = preview("echo\thi\x1b[31m\nls \u{202e}gnp.exe\r\n", 10, 100);
        assert_eq!(more, 0);
        assert_eq!(lines.iter().map(|l| plain(l)).collect::<Vec<_>>(), ["echo⇥hi[␛][31m", "ls [<U+202E>]gnp.exe"]);
    }

    #[test]
    fn preview_cuts_long_text() {
        let text = (0..20).map(|i| format!("line {i} {}", "x".repeat(200))).collect::<Vec<_>>().join("\n");
        let (lines, more) = preview(&text, 12, 10);
        assert_eq!(lines.len(), 12);
        assert_eq!(more, 8);
        assert_eq!(plain(&lines[0]), "line 0 xxx…");
    }

    #[test]
    fn every_risk_reads_well() {
        assert_eq!(describe(&PasteRisk::RunsCommands { commands: 1 }).0, "Runs 1 command right away");
        assert_eq!(describe(&PasteRisk::RunsCommands { commands: 3 }).0, "Runs 3 commands right away");
        assert_eq!(describe(&PasteRisk::Large { bytes: 300 * 1024 }).0, "Large paste (300 KiB)");
        assert_eq!(describe(&PasteRisk::Large { bytes: 3 * 1024 * 1024 }).0, "Large paste (3.0 MiB)");
        assert!(!describe(&PasteRisk::Multiline { lines: 2 }).2);
        assert!(describe(&PasteRisk::DownloadToShell).2);
    }
}
