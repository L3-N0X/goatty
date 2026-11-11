//! The Diagnostics page (#40): what the connected host's shell really
//! loads. goatty starts a managed shell the way a session would and asks
//! it (`goatty_shell::setup::Status`, the same report as `goattyd shell
//! status`): each feature and where it came from, the startup time, what
//! the startup printed to stderr, the user's own startup files. Missing
//! tools can be installed from here.

use std::time::Instant;

use goatty_shell::setup::{FeatureState, Status, ToolLocation, size};
use gpui::{AnyElement, ClipboardItem, Context, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px};

use super::kit::{self, card, row, section};
use super::{Page, SettingsWindow};
use crate::shell_setup::{self, Progress, Step};
use crate::ui::controls::{ButtonKind, Tone, badge, banner, button, button_with};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};

#[derive(Default)]
pub struct DiagPage {
    pub status: Option<Status>,
    pub loading: bool,
    pub error: Option<String>,
    /// An install started here.
    pub progress: Option<Progress>,
}

impl SettingsWindow {
    /// Ask the host's shell again (starts a shell there).
    pub fn load_diagnostics(&mut self, cx: &mut Context<'_, Self>) {
        let Some(link) = self.link.clone() else {
            self.diag.error = Some("This window isn't connected to a goatty host.".into());
            return;
        };
        if self.diag.loading {
            return;
        }
        self.diag.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { link.shell_status() }).await;
            let _ = this.update(cx, |view, cx| {
                view.diag.loading = false;
                match result {
                    Ok(s) => {
                        view.diag.status = Some(s);
                        view.diag.error = None;
                    }
                    Err(e) => view.diag.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Install `ids` (built-in tools), showing progress here, then check again.
    pub fn diag_install(&mut self, ids: Vec<String>, cx: &mut Context<'_, Self>) {
        let (Some(link), Some(status)) = (self.link.clone(), self.diag.status.as_ref()) else { return };
        if self.diag.progress.as_ref().is_some_and(|p| !p.finished) {
            return;
        }
        let progress = Progress::new(status.tools.iter().filter(|t| ids.contains(&t.id)));
        self.diag.progress = Some(progress.clone());
        cx.notify();
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let report = {
                let this = this.clone();
                move |p: &Progress, cx: &mut gpui::App| {
                    let _ = this.update(cx, |v, cx| {
                        v.diag.progress = Some(p.clone());
                        cx.notify();
                    });
                }
            };
            shell_setup::install(link, progress, cx, report).await;
            let _ = this.update(cx, |v, cx| v.load_diagnostics(cx));
        })
        .detach();
    }

    pub(super) fn diagnostics(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let mut page = div().flex().flex_col().gap(px(28.));
        let recheck = button_with(
            "diag-recheck",
            Some(Icon::Refresh),
            if self.diag.loading { "Checking…" } else { "Check again" },
            ButtonKind::Ghost,
            ui,
            self.act(cx, |v, cx| v.load_diagnostics(cx)),
        );
        let Some(s) = &self.diag.status else {
            return match &self.diag.error {
                Some(e) => page
                    .child(banner(ui, Tone::Danger, "Couldn't check the shell", vec![e.clone()]))
                    .child(div().flex().child(recheck))
                    .into_any_element(),
                None => card(ui)
                    .child(kit::empty(
                        ui,
                        Icon::Loader,
                        "Starting a shell to check it…",
                        "goatty starts your shell the way a new tab does and asks what it loaded.",
                    ))
                    .into_any_element(),
            };
        };
        if let Some(e) = &self.diag.error {
            page = page.child(banner(ui, Tone::Danger, "Couldn't check the shell again", vec![e.clone()]));
        }
        page = page.child(self.diag_summary(ui, s, cx));
        if let Some(p) = &self.diag.progress {
            page = page.child(progress_card(ui, p));
        }
        page = page.child(self.diag_shell(ui, s, recheck, cx));
        if let Some(st) = &s.startup {
            page = page.child(self.diag_features(ui, s, st, cx));
            if !st.stderr.is_empty() {
                page = page.child(
                    section(ui, "Startup errors")
                        .desc("What the shell printed while starting: usually a line in one of your startup files")
                        .action(self.copy_button(ui, "diag-copy-errors", "Copy", st.stderr.clone(), cx))
                        .child(mono_block(ui, &st.stderr, cx)),
                );
            }
        }
        let problems = cx.global::<crate::settings::AppSettings>().issues().len() + self.host_issues().len();
        if problems > 0 || !s.env_overrides.is_empty() {
            let mut sec = section(ui, "Settings");
            if problems > 0 {
                sec = sec.child(
                    row(
                        ui,
                        if problems == 1 {
                            "1 setting couldn't be used".to_string()
                        } else {
                            format!("{problems} settings couldn't be used")
                        },
                    )
                    .icon(Icon::TriangleAlert)
                    .desc("Each one, and a button to copy them all, on the Profile page")
                    .on_click("diag-problems", self.listener_go(cx, Page::Profile)),
                );
            }
            if !s.env_overrides.is_empty() {
                sec = sec.child(row(ui, "Overridden by goattyd's environment").icon(Icon::Info).desc(s.env_overrides.join(", ")));
            }
            page = page.child(sec);
        }
        page.into_any_element()
    }

    /// One banner saying how things are, with the fix when there is one.
    fn diag_summary(&self, ui: Ui, s: &Status, cx: &Context<'_, Self>) -> AnyElement {
        let host = if self.link.as_ref().is_some_and(|l| l.is_local()) { "this computer".to_string() } else { self.host_label() };
        if let Some(why) = &s.unmanaged {
            return banner(ui, Tone::Neutral, format!("{} starts as is", s.program), vec![why.clone()]).into_any_element();
        }
        let Some(st) = &s.startup else {
            return banner(
                ui,
                Tone::Danger,
                "The shell didn't finish starting",
                vec![s.startup_error.clone().unwrap_or_default(), "Below: what goattyd finds on its own PATH.".into()],
            )
            .into_any_element();
        };
        let offer: Vec<_> = s.offer().collect();
        let missing = st.features.iter().filter(|f| f.state == FeatureState::Missing).count();
        let installing = self.diag.progress.as_ref().is_some_and(|p| !p.finished);
        let (tone, title) = if !st.stderr.is_empty() {
            (Tone::Warning, "The shell printed errors while starting".to_string())
        } else if missing > 0 {
            (Tone::Warning, if missing == 1 { "1 tool isn't installed".to_string() } else { format!("{missing} tools aren't installed") })
        } else {
            (Tone::Success, "Everything works".to_string())
        };
        let line = format!("Started in {} ms on {host}, measured just now.", st.ms);
        let mut b = div().flex().flex_col().gap(px(10.)).child(banner(ui, tone, title, vec![line]));
        if !offer.is_empty() && !installing {
            let ids: Vec<String> = offer.iter().map(|t| t.id.clone()).collect();
            let bytes: u64 = offer.iter().map(|t| t.download).sum();
            let names: Vec<&str> = offer.iter().map(|t| t.name.as_str()).collect();
            b = b.child(
                div().flex().items_center().gap(px(10.)).px(px(4.)).child(
                    button_with(
                        "diag-install-all",
                        Some(Icon::Download),
                        format!("Install {} ({})", shell_setup::names(&names), size(bytes)),
                        ButtonKind::Primary,
                        ui,
                        self.act(cx, move |v, cx| v.diag_install(ids.clone(), cx)),
                    )
                    .when(installing, |d| d.opacity(0.5)),
                ),
            );
        }
        b.into_any_element()
    }

    fn diag_shell(&self, ui: Ui, s: &Status, recheck: gpui::Stateful<gpui::Div>, cx: &Context<'_, Self>) -> AnyElement {
        let value =
            |t: String| div().max_w(px(360.)).truncate().text_size(px(text::LABEL)).text_color(ui.muted).child(SharedString::from(t));
        let mut sec = section(ui, "Shell")
            .action(div().flex().gap(px(4.)).child(self.copy_button(ui, "diag-copy", "Copy report", report(s), cx)).child(recheck))
            .child(row(ui, "Program").control(value(s.program.clone())));
        if s.managed {
            sec = sec.child(
                row(ui, "Configuration")
                    .desc(s.config_dir.clone())
                    .control(value(s.generation.map_or("Not built yet".into(), |g| format!("Generation {g}")))),
            );
            if let Some(st) = &s.startup {
                sec = sec.child(
                    row(ui, "Startup time").desc("A login shell, from start to the first prompt").control(value(format!("{} ms", st.ms))),
                );
            }
            for f in &s.user_files {
                let (label, tone) = match (f.exists, f.sourced) {
                    (false, _) => ("No file", Tone::Neutral),
                    (true, true) => ("Sourced", Tone::Success),
                    (true, false) => ("Not sourced", Tone::Neutral),
                };
                sec = sec.child(row(ui, f.path.clone()).desc("Your own startup file").control(badge(label, tone, ui)));
            }
            let (label, tone) = if s.additions_exist { ("Sourced", Tone::Success) } else { ("Not created", Tone::Neutral) };
            sec = sec.child(
                row(ui, s.additions.clone()).desc("Your additions to goatty's shell: aliases, functions").control(badge(label, tone, ui)),
            );
            sec = sec.child(row(ui, "Shell settings").icon(Icon::Shell).on_click("diag-to-shell", self.listener_go(cx, Page::Shell)));
        }
        sec.into_any_element()
    }

    fn diag_features(&self, ui: Ui, s: &Status, st: &goatty_shell::setup::Startup, cx: &Context<'_, Self>) -> AnyElement {
        let installing = self.diag.progress.as_ref().is_some_and(|p| !p.finished);
        let rows = st.features.iter().map(|f| {
            let r = row(ui, f.name.clone());
            match &f.state {
                FeatureState::Loaded { detail, managed: Some(v) } => {
                    r.desc(format!("goatty's copy {v} · {detail}")).control(badge("Loaded", Tone::Success, ui))
                }
                FeatureState::Loaded { detail, managed: None } => r.desc(detail.clone()).control(badge("Loaded", Tone::Success, ui)),
                FeatureState::Off => r.desc("Turned off in the settings").control(badge("Off", Tone::Neutral, ui)),
                FeatureState::Missing => match s.tool(&f.id) {
                    Some(t) if t.installable() => {
                        let id = t.id.clone();
                        r.desc(format!("Not installed · {} {} is {}", t.name, t.version, size(t.download))).control(
                            button("diag-install-".to_string() + &t.id, "Install", ButtonKind::Normal, ui, {
                                self.act(cx, move |v, cx| v.diag_install(vec![id.clone()], cx))
                            })
                            .when(installing, |d| d.opacity(0.5)),
                        )
                    }
                    Some(t) => {
                        r.desc(t.unavailable.clone().unwrap_or_else(|| "Not installed".into())).control(badge("Missing", Tone::Warning, ui))
                    }
                    None => r.desc("Its command isn't installed").control(badge("Missing", Tone::Warning, ui)),
                },
            }
        });
        section(ui, "Features")
            .desc("What the shell loaded, and where from")
            .children(rows)
            .footer("goatty prefers a copy installed on the system; its own copies live in ~/.local/share/goatty.")
            .into_any_element()
    }

    pub(super) fn copy_button(
        &self,
        ui: Ui,
        id: &'static str,
        label: &'static str,
        text: String,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let copied = self.copied.as_ref().is_some_and(|(p, at)| p.as_ref() == id && at.elapsed().as_millis() < 1500);
        button_with(
            id,
            Some(if copied { Icon::Check } else { Icon::Copy }),
            if copied { "Copied" } else { label },
            ButtonKind::Ghost,
            ui,
            self.act(cx, move |v, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                v.copied = Some((id.into(), Instant::now()));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
                cx.notify();
            }),
        )
    }
}

/// Each tool being installed, with its state.
pub fn progress_card(ui: Ui, p: &Progress) -> impl IntoElement {
    let title = if p.finished {
        if p.failures().is_empty() {
            "Installed. Your shells reload with the new tools at their next prompt."
        } else {
            "Some installs failed"
        }
    } else {
        "Installing…"
    };
    let rows = p.tools.iter().map(|(id, name, step)| {
        let (icon, color, desc) = match step {
            Step::Waiting => (Icon::Download, ui.faint, "Waiting".to_string()),
            Step::Running => (Icon::Loader, ui.accent_text, "Downloading and checking…".to_string()),
            Step::Done => (Icon::CircleCheck, ui.success, "Installed".to_string()),
            Step::Failed(e) => (Icon::CircleX, ui.danger, e.clone()),
        };
        div()
            .id(SharedString::from(format!("progress-{id}")))
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(16.))
            .py(px(10.))
            .child(icon.svg().size(px(18.)).text_color(color))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().text_size(px(text::BODY)).child(SharedString::from(name.clone())))
                    .child(div().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(desc))),
            )
    });
    section(ui, title).children(rows)
}

fn mono_block(ui: Ui, t: &str, cx: &gpui::App) -> impl IntoElement {
    div()
        .m(px(12.))
        .p(px(12.))
        .rounded(px(radius::CONTROL))
        .bg(ui.field)
        .font_family(kit::mono(cx))
        .text_size(px(text::SMALL))
        .text_color(ui.text)
        .children(t.lines().take(60).map(|l| div().child(SharedString::from(l.to_owned()))))
}

/// The status as text, for a bug report.
fn report(s: &Status) -> String {
    let mut out = vec![format!("program: {}", s.program)];
    match &s.unmanaged {
        Some(why) => out.push(format!("config: {why}")),
        None => out.push(format!("config: {} (generation {})", s.config_dir, s.generation.map_or("none".into(), |g| g.to_string()))),
    }
    out.extend(s.settings_issues.iter().map(|i| format!("settings: {i}")));
    if !s.env_overrides.is_empty() {
        out.push(format!("overridden by: {}", s.env_overrides.join(", ")));
    }
    out.extend(s.user_files.iter().map(|f| {
        format!(
            "{}: {}",
            f.path,
            if !f.exists {
                "not there"
            } else if f.sourced {
                "sourced"
            } else {
                "not sourced"
            }
        )
    }));
    match &s.startup {
        Some(st) => {
            out.push(format!("startup: {} ms", st.ms));
            for f in &st.features {
                out.push(match &f.state {
                    FeatureState::Loaded { detail, managed: Some(v) } => format!("  {} ok (managed {v}) {detail}", f.id),
                    FeatureState::Loaded { detail, managed: None } => format!("  {} ok {detail}", f.id),
                    FeatureState::Off => format!("  {} off", f.id),
                    FeatureState::Missing => format!("  {} missing", f.id),
                });
            }
            if !st.stderr.is_empty() {
                out.push(format!("stderr:\n{}", st.stderr));
            }
        }
        None => {
            out.push(format!("startup failed: {}", s.startup_error.as_deref().unwrap_or("?")));
            for t in &s.tools {
                out.push(match &t.location {
                    ToolLocation::System { path } => format!("  {} {path}", t.id),
                    ToolLocation::Managed { version } => format!("  {} managed {version}", t.id),
                    ToolLocation::Missing => format!("  {} not found", t.id),
                });
            }
        }
    }
    out.join("\n")
}
