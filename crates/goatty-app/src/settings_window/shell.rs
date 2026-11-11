//! The host's shell pages: Shell, Prompt (with a live preview), History.

use goatty_config::{HostSettings, PromptStyle};
use gpui::{AnyElement, Context, FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px, rgb};

use super::kit::{self, group, opt, row, section};
use super::{Page, SettingsWindow, builtin_bool_default};
use crate::settings::AppSettings;
use crate::ui::controls::{Tone, banner, segmented, toggle};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};

/// The built-in plugins the Shell page used to switch, now on Plugins.
const BUILTIN_TOOLS: &[&str] = &["autosuggestions", "syntax-highlighting", "fzf", "starship", "atuin"];

impl SettingsWindow {
    /// Problems loading the host's settings, shown on top of its pages.
    /// `None` in the second place: nothing to edit (yet).
    pub(super) fn host_status(&self, ui: Ui) -> (Vec<AnyElement>, Option<HostSettings>) {
        let mut notes = Vec::new();
        if let Some(e) = &self.host_error {
            notes.push(
                banner(ui, Tone::Danger, format!("Couldn't load the settings of {}", self.host_label()), vec![e.clone()])
                    .into_any_element(),
            );
            return (notes, None);
        }
        if self.host.is_none() {
            notes.push(
                kit::empty(ui, Icon::Loader, "Loading…", format!("Asking {} for its settings", self.host_label())).into_any_element(),
            );
            return (notes, None);
        };
        // Problems with a setting on this page show on its row.
        let (issues, vars) = self.unshown_host_problems(self.page);
        if !issues.is_empty() {
            notes.push(banner(ui, Tone::Warning, "Some settings couldn't be read", issues).into_any_element());
        }
        if !vars.is_empty() {
            notes.push(
                banner(ui, Tone::Neutral, format!("Overridden on {} by {}", self.host_label(), vars.join(", ")), Vec::new())
                    .into_any_element(),
            );
        }
        (notes, self.host_value().cloned())
    }

    fn host_page(&self, ui: Ui, sections: impl FnOnce(&HostSettings) -> Vec<AnyElement>) -> AnyElement {
        let (notes, h) = self.host_status(ui);
        let mut page = div().flex().flex_col().gap(px(28.)).children(notes);
        if let Some(h) = h {
            page = page.children(sections(&h));
        }
        page.into_any_element()
    }

    pub(super) fn shell(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        self.host_page(ui, |h| {
            let program = h.shell.program.clone();
            let choice = match program.as_deref() {
                None => "auto",
                Some("zsh") => "zsh",
                Some("bash") => "bash",
                Some("fish") => "fish",
                Some(_) => "custom",
            };
            let zsh = matches!(choice, "auto" | "zsh");
            let z = &h.shell.zsh;
            let managed = h.shell.managed;
            let hero = group(ui).child(
                row(ui, "goatty's managed zsh")
                    .icon(Icon::Zap)
                    .desc(if zsh {
                        "Plugins, prompt, history and shell integration, without touching your dotfiles. Off: plain zsh with your own config"
                    } else {
                        "Only for zsh: pick zsh or Automatic below to use it"
                    })
                    .disabled(!zsh)
                    .control(toggle("managed", managed && zsh, ui, self.host(cx, |h| h.shell.managed = !h.shell.managed))),
            );
            let this = cx.entity().downgrade();
            let program_section = section(ui, "Program")
                .child(row(ui, "Shell").control(self.select(
                    "program", "Shell",
                    vec![
                        opt("auto", "Automatic").desc("zsh if it's installed, else your login shell"),
                        opt("zsh", "zsh"),
                        opt("bash", "bash"),
                        opt("fish", "fish"),
                        opt("custom", "Other…").desc("A program you name"),
                    ],
                    &choice,
                    cx,
                    move |c: &str, _, cx| {
                        let _ = this.update(cx, |v, cx| {
                            v.edit_host(cx, |h| {
                                h.shell.program = match c {
                                    "auto" => None,
                                    "custom" => h
                                        .shell
                                        .program
                                        .clone()
                                        .filter(|p| !["zsh", "bash", "fish"].contains(&p.as_str()))
                                        .or(Some("/bin/sh".into())),
                                    name => Some(name.into()),
                                }
                            })
                        });
                    },
                )))
                .when_some_child((choice == "custom").then_some(()), |_| {
                    row(ui, "Program to run").desc("A path or a name on PATH. Enter saves").below(self.shell_path.clone()).into_any_element()
                });
            let off = !(managed && zsh);
            let dotfiles = section(ui, "Your dotfiles")
                .desc(if off { "Only with goatty's managed zsh" } else { "What goatty's zsh reads from your own configuration" })
                .child(row(ui, "Use ~/.zshenv and ~/.zprofile").desc("Your environment: PATH, EDITOR, …").disabled(off).control(toggle(
                    "user-env",
                    z.user_env,
                    ui,
                    self.host(cx, |h| h.shell.zsh.user_env = !h.shell.zsh.user_env),
                )))
                .child(row(ui, "Also use ~/.zshrc").desc("Most .zshrc files load their own prompt and plugins, which can fight goatty's").disabled(off).control(toggle(
                    "user-rc",
                    z.user_rc,
                    ui,
                    self.host(cx, |h| h.shell.zsh.user_rc = !h.shell.zsh.user_rc),
                )))
                .footer("Your own additions for goatty's zsh go in ~/.config/goatty/zshrc.");
            let on = BUILTIN_TOOLS.iter().filter(|id| h.plugin_enabled(id)).count();
            let integration = section(ui, "Integration")
                .child(row(ui, "Shell integration").desc("Marks prompts and commands (OSC 133) and reports the directory (OSC 7)").disabled(off).control(toggle(
                    "integration",
                    z.integration,
                    ui,
                    self.host(cx, |h| h.shell.zsh.integration = !h.shell.zsh.integration),
                )))
                .child(
                    row(ui, "Built-in plugins")
                        .icon(Icon::Puzzle)
                        .desc(format!("Autosuggestions, syntax highlighting, fzf, Starship and Atuin · {on} of {} on", BUILTIN_TOOLS.len()))
                        .on_click("shell-plugins", self.listener_go(cx, Page::Plugins)),
                );
            vec![hero.into_any_element(), program_section.into_any_element(), dotfiles.into_any_element(), integration.into_any_element()]
        })
    }

    // ---- Prompt ------------------------------------------------------------------

    /// What the prompt looks like with the settings on screen.
    fn prompt_preview(&self, ui: Ui, h: &HostSettings, cx: &Context<'_, Self>) -> impl IntoElement {
        let t = cx.global::<AppSettings>().colors;
        let col = |i: usize| -> Hsla { rgb(t.ansi[i].0).into() };
        let (fg, bg) = (Hsla::from(rgb(t.foreground.0)), Hsla::from(rgb(t.background.0)));
        let p = &h.prompt;
        let starship = h.plugin_enabled("starship");
        let own = starship && h.plugin_bool("starship", "prefer_user_config", builtin_bool_default("starship", "prefer_user_config"));
        let span = |s: &'static str, c: Hsla, bold: bool| div().text_color(c).when(bold, |d| d.font_weight(FontWeight::BOLD)).child(s);
        let mut info: Vec<gpui::Div> = Vec::new();
        if !starship {
            info.push(span("~/projects/goatty", col(4), false));
        } else if p.style == PromptStyle::Rich {
            info.push(span("~/projects/goatty", col(6), true));
            info.push(span(" on ", fg, false));
            info.push(span("\u{e0a0} main", col(5), true));
            info.push(span(" [!]", col(1), true));
            if p.duration_threshold_ms > 0 {
                info.push(span(" took ", fg, false));
                info.push(span("4s", col(3), true));
            }
        } else {
            info.push(span("~/projects/goatty", col(6), true));
            info.push(span(" main", col(5), true));
            info.push(span(" !", col(1), true));
            if p.duration_threshold_ms > 0 {
                info.push(span(" took ", fg, false));
                info.push(span("4s", col(3), true));
            }
        }
        let line_h = px(22.);
        let marker = div().flex().items_center().child(span("❯ ", col(2), true)).child(div().w(px(8.)).h(px(17.)).bg(rgb(t.cursor.0)));
        let prompt = if p.two_line {
            div()
                .flex()
                .flex_col()
                .child(div().flex().h(line_h).items_center().children(info))
                .child(div().h(line_h).flex().items_center().child(marker))
        } else {
            div().flex().h(line_h).items_center().children(info).child(div().w(px(8.))).child(marker)
        };
        let dim = fg.opacity(0.6);
        div()
            .flex()
            .flex_col()
            .rounded(px(radius::CARD))
            .overflow_hidden()
            .border_1()
            .border_color(ui.divider)
            .child(
                div()
                    .bg(bg)
                    .px(px(18.))
                    .py(px(14.))
                    .font_family(kit::mono(cx))
                    .text_size(px(13.5))
                    .text_color(fg)
                    .child(div().h(line_h).flex().items_center().text_color(dim).child("    Finished release profile in 4.2s"))
                    .when(starship && p.newline_before, |d| d.child(div().h(line_h)))
                    .child(prompt),
            )
            .when(own, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(14.))
                        .py(px(9.))
                        .bg(ui.card)
                        .text_size(px(text::SMALL))
                        .text_color(ui.muted)
                        .child(Icon::Info.svg().size(px(14.)).text_color(ui.muted))
                        .child("If you have a ~/.config/starship.toml, it decides how the prompt looks instead"),
                )
            })
    }

    pub(super) fn prompt(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        self.host_page(ui, |h| {
            let p = h.prompt.clone();
            let starship = h.plugin_enabled("starship");
            let own = h.plugin_bool("starship", "prefer_user_config", builtin_bool_default("starship", "prefer_user_config"));
            let needs = |on: bool, desc: &'static str| if on { desc } else { "Needs Starship" };
            let mut durations: Vec<u32> = vec![0, 1000, 2000, 3000, 5000, 10_000, 30_000, 60_000];
            if !durations.contains(&p.duration_threshold_ms) {
                durations.push(p.duration_threshold_ms);
                durations.sort_unstable();
            }
            let duration_opts = durations
                .into_iter()
                .map(|ms| match ms {
                    0 => opt(0, "Never"),
                    ms if ms % 1000 == 0 => opt(ms, format!("After {} s", ms / 1000)),
                    ms => opt(ms, format!("After {ms} ms")),
                })
                .collect();
            let this = cx.entity().downgrade();
            let look = section(ui, "Prompt")
                .child(row(ui, "Starship").desc("Git status, durations and more. Off: goatty's small built-in prompt").control(toggle(
                    "starship",
                    starship,
                    ui,
                    self.toggle_plugin(cx, "starship"),
                )))
                .child(
                    row(ui, "Style")
                        .desc(needs(starship, "Minimal uses plain characters, for fonts without Nerd Font symbols"))
                        .disabled(!starship)
                        .control(segmented("style", &[(PromptStyle::Rich, "Rich"), (PromptStyle::Minimal, "Minimal")], &p.style, ui, {
                            let this = this.clone();
                            move |st, _, cx| {
                                let _ = this.update(cx, |v, cx| v.edit_host(cx, |h| h.prompt.style = st));
                            }
                        })),
                )
                .child(
                    row(ui, "Use my own starship.toml")
                        .desc(needs(starship, "~/.config/starship.toml, when it exists, instead of these settings"))
                        .disabled(!starship)
                        .control(toggle("own-starship", own, ui, self.toggle_plugin_bool(cx, "starship", "prefer_user_config"))),
                );
            let layout = section(ui, "Layout")
                .child(row(ui, "Input on its own line").desc("Type commands on a line below the directory").control(toggle(
                    "two-line",
                    p.two_line,
                    ui,
                    self.host(cx, |h| h.prompt.two_line = !h.prompt.two_line),
                )))
                .child(
                    row(ui, "Empty line before each prompt")
                        .desc(needs(starship, "Space between one command's output and the next prompt"))
                        .disabled(!starship)
                        .control(toggle(
                            "newline",
                            p.newline_before,
                            ui,
                            self.host(cx, |h| h.prompt.newline_before = !h.prompt.newline_before),
                        )),
                )
                .child(
                    row(ui, "Show how long commands took")
                        .desc(needs(starship, "For commands that ran at least this long"))
                        .disabled(!starship)
                        .control(self.select(
                            "duration",
                            "Show how long commands took",
                            duration_opts,
                            &p.duration_threshold_ms,
                            cx,
                            move |ms, _, cx| {
                                let _ = this.update(cx, |v, cx| v.edit_host(cx, |h| h.prompt.duration_threshold_ms = ms));
                            },
                        )),
                );
            vec![self.prompt_preview(ui, h, cx).into_any_element(), look.into_any_element(), layout.into_any_element()]
        })
    }

    // ---- History -----------------------------------------------------------------

    pub(super) fn history(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        self.host_page(ui, |h| {
            let hs = h.history.clone();
            let mut sizes: Vec<u32> = vec![1_000, 10_000, 100_000, 1_000_000];
            if !sizes.contains(&hs.size) {
                sizes.push(hs.size);
                sizes.sort_unstable();
            }
            let size_opts = sizes.into_iter().map(|n| opt(n, group_digits(n))).collect();
            let this = cx.entity().downgrade();
            let atuin = h.plugin_enabled("atuin");
            let kept =
                section(ui, "Saving")
                    .child(row(ui, "Commands to keep").desc("In memory and in the history file").control(self.select(
                        "hist-size",
                        "Commands to keep",
                        size_opts,
                        &hs.size,
                        cx,
                        move |n, _, cx| {
                            let _ = this.update(cx, |v, cx| v.edit_host(cx, |h| h.history.size = n));
                        },
                    )))
                    .child(row(ui, "Skip duplicates").desc("Don't save a command that repeats the one before").control(toggle(
                        "dups",
                        hs.ignore_duplicates,
                        ui,
                        self.host(cx, |h| h.history.ignore_duplicates = !h.history.ignore_duplicates),
                    )))
                    .child(
                        row(ui, "Skip commands starting with a space").desc("Handy for one-off commands with secrets in them").control(
                            toggle("space", hs.ignore_space, ui, self.host(cx, |h| h.history.ignore_space = !h.history.ignore_space)),
                        ),
                    )
                    .child(row(ui, "Share between running shells").desc("Commands show up in other tabs right away").control(toggle(
                        "share",
                        hs.share_between_sessions,
                        ui,
                        self.host(cx, |h| h.history.share_between_sessions = !h.history.share_between_sessions),
                    )));
            let search = section(ui, "Search")
                .child(row(ui, "Atuin").desc("Search your whole history on Ctrl+R, with filters and context").control(toggle(
                    "atuin",
                    atuin,
                    ui,
                    self.toggle_plugin(cx, "atuin"),
                )))
                .child(
                    row(ui, "Atuin on the Up arrow")
                        .desc(if atuin { "Off: Up steps through history matching what you typed" } else { "Needs Atuin" })
                        .disabled(!atuin)
                        .control(toggle(
                            "atuin-up",
                            h.plugin_bool("atuin", "up_arrow", builtin_bool_default("atuin", "up_arrow")),
                            ui,
                            self.toggle_plugin_bool(cx, "atuin", "up_arrow"),
                        )),
                );
            vec![kept.into_any_element(), search.into_any_element()]
        })
    }
}

/// `100000` → `100 000`.
fn group_digits(n: u32) -> SharedString {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(ch);
    }
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_grouped() {
        assert_eq!(group_digits(1_000).as_ref(), "1\u{2009}000");
        assert_eq!(group_digits(100_000).as_ref(), "100\u{2009}000");
        assert_eq!(group_digits(999).as_ref(), "999");
    }
}
