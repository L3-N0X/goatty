//! The Profile page: where the settings live (copy the path, open the
//! folder), problems reading them, export/import, shell configuration
//! generations, and resetting.

use std::time::{Duration, Instant};

use goatty_config::{ClientSettings, ThemeSettings};
use gpui::{AnyElement, ClipboardItem, Context, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, px};

use super::SettingsWindow;
use super::kit::{self, card, row, section};
use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, Tone, badge, button, button_with, icon_button};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, text};

/// How long "Copied" shows after copying a path.
const COPIED: Duration = Duration::from_millis(1500);
/// Generations shown before "Show all".
const RECENT_GENERATIONS: usize = 5;

/// `/home/me/.config/x` → `~/.config/x`, for showing (copying keeps the full path).
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

pub(super) fn ago(created_unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match now.saturating_sub(created_unix) {
        0..60 => "just now".to_string(),
        s @ 60..3600 => format!("{} min ago", s / 60),
        s @ 3600..86400 => format!("{} h ago", s / 3600),
        86400..172800 => "yesterday".to_string(),
        s => format!("{} days ago", s / 86400),
    }
}

impl SettingsWindow {
    fn copy_path(&mut self, path: String, cx: &mut Context<'_, Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
        self.copied = Some((path.into(), Instant::now()));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    /// A settings file: what's in it, its path in the terminal font, copy and open.
    fn file_row(
        &self,
        ui: Ui,
        id: &'static str,
        label: &str,
        path: String,
        folder: Option<std::path::PathBuf>,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let copied = self.copied.as_ref().is_some_and(|(p, at)| p.as_ref() == path && at.elapsed() < COPIED);
        let copy = path.clone();
        row(ui, label.to_owned())
            .icon(Icon::FileText)
            .below(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(50.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .bg(ui.field)
                            .font_family(kit::mono(cx))
                            .text_size(px(text::SMALL))
                            .text_color(ui.text)
                            .truncate()
                            .child(SharedString::from(tilde(&path))),
                    )
                    .child(if copied {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(8.))
                            .text_size(px(text::SMALL))
                            .text_color(ui.success)
                            .child(Icon::Check.svg().size(px(14.)).text_color(ui.success))
                            .child("Copied")
                            .into_any_element()
                    } else {
                        icon_button(
                            SharedString::from(format!("{id}-copy")),
                            Icon::Copy,
                            "Copy the path",
                            ui,
                            self.act(cx, move |v, cx| v.copy_path(copy.clone(), cx)),
                        )
                        .into_any_element()
                    })
                    .children(folder.map(|dir| {
                        icon_button(SharedString::from(format!("{id}-open")), Icon::FolderOpen, "Open the folder", ui, move |_, cx| {
                            let _ = std::fs::create_dir_all(&dir);
                            cx.open_with_system(&dir);
                        })
                    })),
            )
            .into_any_element()
    }

    pub(super) fn profile(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let app = cx.global::<AppSettings>();
        let label = self.host_label();
        let status = match self.problems_section(ui, cx) {
            Some(problems) => problems,
            None => card(ui)
                .flex_row()
                .items_center()
                .gap(px(14.))
                .p(px(16.))
                .child(
                    div()
                        .size(px(36.))
                        .rounded_full()
                        .bg(ui.success.opacity(0.14))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::CircleCheck.svg().size(px(18.)).text_color(ui.success)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child("Every setting was read as written"))
                        .child(div().text_size(px(text::SMALL)).text_color(ui.muted).child("No problems in the settings files")),
                )
                .into_any_element(),
        };

        let client_path = app.path.as_ref().map(|p| p.display().to_string());
        let client_dir = app.path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let local = self.link.as_ref().is_some_and(|l| l.is_local());
        let host_path = if local { goatty_config::host_file().map(|p| p.display().to_string()) } else { None };
        let mut files = section(ui, "Files");
        files = match client_path {
            Some(p) => {
                files.child(self.file_row(ui, "client-file", "Appearance, shortcuts, workspace and hosts", p, client_dir.clone(), cx))
            }
            None => {
                files.child(row(ui, "Appearance, shortcuts, workspace and hosts").icon(Icon::FileText).desc("Not saved: there's no $HOME"))
            }
        };
        files = match host_path {
            Some(p) => files.child(self.file_row(ui, "host-file", "Shell, prompt, history, aliases and plugins", p, None, cx)),
            None => files.child(
                row(ui, format!("Shell settings on {label}"))
                    .icon(Icon::FileText)
                    .desc("~/.config/goatty/host.json on that host; goatty edits it through goattyd"),
            ),
        };
        if !app.env_overrides.is_empty() {
            files = files.footer(format!("Overridden for this run by {}.", app.env_overrides.join(", ")));
        }

        let share = section(ui, "Share")
            .desc("Both kinds of settings in one file, without paths that only fit this machine")
            .child(row(ui, "Export a profile").icon(Icon::Upload).control(button(
                "export",
                "Export…",
                ButtonKind::Normal,
                ui,
                self.act(cx, |v, cx| v.export_profile(cx)),
            )))
            .child(
                row(ui, "Import a profile")
                    .icon(Icon::Download)
                    .desc("Appearance applies right away; shell settings wait for you to apply them")
                    .control(button("import", "Import…", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.import_profile(cx)))),
            );

        let active = self.host.as_ref().and_then(|h| h.generation).map(|g| format!(" Generation {g} is active.")).unwrap_or_default();
        let mut gens = section(ui, "Shell configuration history").desc(format!(
            "Each applied change on {label} builds a checked generation; rolling back brings its settings back too.{active}"
        ));
        let total = self.generations.len();
        let shown: Vec<_> = self.generations.iter().rev().take(if self.gens_expanded { total } else { RECENT_GENERATIONS }).collect();
        if shown.is_empty() {
            gens = gens.child(kit::empty(
                ui,
                Icon::GitCommit,
                "No generations yet",
                "One is built when the first managed shell starts, or when you apply settings.",
            ));
        }
        for g in shown {
            let n = g.number;
            let control = if g.active {
                badge("Active", Tone::Success, ui).into_any_element()
            } else {
                button_with(
                    ("rollback", n as usize),
                    Some(Icon::RotateCcw),
                    "Roll back",
                    ButtonKind::Ghost,
                    ui,
                    self.act(cx, move |v, cx| v.rollback(n, cx)),
                )
                .into_any_element()
            };
            gens = gens.child(row(ui, format!("Generation {n}")).icon(Icon::GitCommit).desc(ago(g.created_unix)).control(control));
        }
        if total > RECENT_GENERATIONS {
            gens = gens.child(row(ui, if self.gens_expanded { "Show fewer".to_string() } else { format!("Show all {total}") }).on_click(
                "gens-more",
                cx.listener(|v, _, _, cx| {
                    v.gens_expanded = !v.gens_expanded;
                    cx.notify();
                }),
            ));
        }

        let confirming = self.confirm_reset;
        let reset = section(ui, "Reset").child(
            row(ui, "Reset appearance, shortcuts and workspace")
                .desc("Back to goatty's defaults on this computer. The current file is kept in backups/")
                .control(button(
                    "reset-client",
                    if confirming { "Reset everything?" } else { "Reset…" },
                    ButtonKind::Danger,
                    ui,
                    self.act(cx, |v, cx| {
                        if v.confirm_reset {
                            AppSettings::set(cx, ClientSettings { theme: ThemeSettings::default(), ..Default::default() });
                        }
                        v.confirm_reset = !v.confirm_reset;
                        cx.notify();
                    }),
                )),
        );
        div().flex().flex_col().gap(px(28.)).child(status).child(files).child(share).child(gens).child(reset).into_any_element()
    }
}
