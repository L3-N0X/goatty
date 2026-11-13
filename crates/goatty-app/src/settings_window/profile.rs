//! The Profile page: where the settings live (copy the path, open the
//! folder), problems reading them, export/import, shell configuration
//! generations (what each changed, names, file diffs), and resetting.

use std::time::{Duration, Instant};

use goatty_config::{ClientSettings, HostSettings, ThemeSettings};
use goatty_plugin::api::Catalog;
use goatty_shell::history::{Entry, NAME_MAX};
use gpui::{
    AnyElement, ClipboardItem, Context, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, div,
    px,
};

use super::host::GenRow;
use super::kit::{self, card, row, section};
use super::{Page, SettingsWindow};
use crate::settings::AppSettings;
use crate::toast::{self, Toast};
use crate::ui::controls::{ButtonKind, Tone, badge, button, button_with, icon_button};
use crate::ui::focus;
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, text};

/// How long "Copied" shows after copying a path.
const COPIED: Duration = Duration::from_millis(1500);
/// Generations shown before "Show all".
const RECENT_GENERATIONS: usize = 5;
/// Changes listed in an open generation before "and N more".
const CHANGE_LINES: usize = 12;

fn missing_profile_plugins(imported: &[String], host: Option<&HostSettings>, catalog: Option<&Catalog>) -> Vec<String> {
    let Some(host) = host else { return Vec::new() };
    imported
        .iter()
        .filter(|id| goatty_shell::builtins::get(id).is_none() && host.plugins.contains_key(id.as_str()))
        .filter(|id| catalog.is_none_or(|c| !c.installed.iter().any(|i| i.id == **id)))
        .cloned()
        .collect()
}

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

/// "2 h ago · history.size 100000 → 5000", with the number first when
/// the generation has a name.
fn gen_desc(e: &Entry) -> String {
    let mut parts = Vec::new();
    if e.name.is_some() {
        parts.push(format!("Generation {}", e.number));
    }
    parts.push(ago(e.created_unix));
    parts.extend(Some(e.summary()).filter(|s| !s.is_empty()));
    parts.join(" · ")
}

impl SettingsWindow {
    pub(super) fn missing_imported_plugins(&self) -> Vec<String> {
        missing_profile_plugins(&self.imported_plugins, self.host_value(), self.plugins.catalog.as_ref())
    }

    fn imported_plugins_section(&self, ui: Ui, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let missing = self.missing_imported_plugins();
        if missing.is_empty() {
            return None;
        }
        let label = self.host_label();
        let rows = missing.into_iter().map(|id| {
            let enabled_in_profile = self.host_value().and_then(|h| h.plugins.get(&id)).is_none_or(|p| p.enabled);
            let disabled_note = if enabled_in_profile { "" } else { " · disabled in profile (installing enables it)" };
            match &self.plugins.catalog {
                None if self.plugins.error.is_some() => row(ui, id)
                    .icon(Icon::Puzzle)
                    .desc("Couldn't check registries on this host")
                    .control(badge("Unavailable", Tone::Warning, ui)),
                None => row(ui, id).icon(Icon::Puzzle).desc("Checking plugin registries…").control(badge("Checking…", Tone::Neutral, ui)),
                Some(cat) => match cat.available.iter().find(|a| a.id == id) {
                    Some(a) if a.incompatible.is_none() => {
                        let pid = id.clone();
                        row(ui, a.name.clone())
                            .icon(Icon::Puzzle)
                            .desc(format!("{} · {} registry {}{disabled_note}", a.summary, a.registry, a.version))
                            .control(button_with(
                                SharedString::from(format!("imported-plugin-install-{id}")),
                                Some(Icon::Download),
                                "Review & install",
                                ButtonKind::Normal,
                                ui,
                                self.act(cx, move |v, cx| v.review_plugin_from(pid.clone(), Page::Profile, cx)),
                            ))
                    }
                    Some(a) => {
                        let why = a.incompatible.as_deref().unwrap_or("Not compatible with this host");
                        row(ui, a.name.clone()).icon(Icon::Puzzle).desc(format!("{why}{disabled_note}")).control(badge(
                            "Not for this host",
                            Tone::Danger,
                            ui,
                        ))
                    }
                    None => {
                        let desc = if cat.registries.iter().any(|r| r.error.is_some()) {
                            format!("Couldn't read every registry; refresh or install locally{disabled_note}")
                        } else {
                            format!("Not in this host's registries; add one with goattyd plugin registry add{disabled_note}")
                        };
                        row(ui, id).icon(Icon::Puzzle).desc(desc).control(badge("Not in registries", Tone::Warning, ui))
                    }
                },
            }
        });
        let actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(icon_button(
                "imported-plugins-refresh",
                if self.plugins.is_loading() { Icon::Loader } else { Icon::Refresh },
                "Check plugin registries",
                ui,
                self.act(cx, |v, cx| v.load_plugins(true, cx)),
            ))
            .child(button(
                "imported-plugins-dismiss",
                "Dismiss",
                ButtonKind::Ghost,
                ui,
                self.act_window(cx, |v, window, cx| {
                    focus::hand_over(&v.focus, focus::Handover::Control(ElementId::from("import")), window, cx);
                    v.imported_plugins.clear();
                    cx.notify();
                }),
            ));
        Some(
            section(ui, "Plugins in this profile")
                .desc(format!("This profile configures plugins that aren't installed on {label} yet"))
                .action(actions)
                .children(rows)
                .into_any_element(),
        )
    }

    /// Open or close a generation's details.
    fn toggle_generation(&mut self, number: u32, cx: &mut Context<'_, Self>) {
        if self.gen_open == Some(number) {
            self.gen_open = None;
        } else {
            self.gen_open = Some(number);
            let name = self.generations.iter().find(|g| g.entry.number == number).and_then(|g| g.entry.name.clone());
            self.gen_name.update(cx, |input, cx| input.set_text(name.unwrap_or_default(), cx));
        }
        self.gen_diff = None;
        cx.notify();
    }

    /// Save the open generation's name (blank removes it), then list again.
    pub(super) fn save_generation_name(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(link), Some(number)) = (self.link.clone(), self.gen_open) else { return };
        let name = self.gen_name.read(cx).text().trim().to_string();
        let name = (!name.is_empty()).then_some(name);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { link.name_generation(number, name.clone()).and_then(|()| Ok((name, link.generations()?))) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok((name, gens)) => {
                        view.generations = gens;
                        let title = match name {
                            Some(n) => format!("Generation {number} is \u{201c}{n}\u{201d}"),
                            None => format!("Generation {number} has no name now"),
                        };
                        toast::show(cx, Toast::success(title).key("generation-name"));
                    }
                    Err(e) => {
                        toast::show(cx, Toast::error("Couldn't name the generation").body(format!("{e:#}")).key("generation-name"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Show or hide the generated-file changes of generation `number`.
    fn toggle_generation_diff(&mut self, number: u32, cx: &mut Context<'_, Self>) {
        if self.gen_diff.as_ref().is_some_and(|(n, _)| *n == number) {
            self.gen_diff = None;
            cx.notify();
            return;
        }
        let Some(link) = self.link.clone() else { return };
        self.gen_diff = Some((number, None));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { link.generation_diff(number) }).await;
            let _ = this.update(cx, |view, cx| {
                if view.gen_diff.as_ref().is_some_and(|(n, _)| *n == number) {
                    view.gen_diff = Some((number, Some(result.map_err(|e| format!("{e:#}")))));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Under an open generation: every change, its name, its file changes.
    fn generation_details(&self, ui: Ui, g: &GenRow, cx: &Context<'_, Self>) -> AnyElement {
        let n = g.entry.number;
        let line = |t: String| div().text_size(px(text::SMALL)).text_color(ui.text).child(SharedString::from(t));
        let mut changes = div().flex().flex_col().gap(px(4.));
        match &g.entry.changes {
            Some(c) if c.base.is_none() => changes = changes.child(line("The first configuration built on this host".into())),
            Some(c) => {
                let lines = c.lines();
                let total = lines.len();
                changes = changes.children(lines.into_iter().take(CHANGE_LINES).map(|l| line(format!("• {l}"))));
                if total > CHANGE_LINES {
                    changes = changes.child(line(format!("and {} more", total - CHANGE_LINES)));
                }
                if total == 0 && c.files == 0 {
                    changes = changes.child(line("No setting and no generated file changed".into()));
                }
            }
            None => changes = changes.child(line("Nothing to compare: the generation before it is gone".into())),
        }
        let files = g.entry.changes.as_ref().map_or(0, |c| c.files);
        let diff_open = self.gen_diff.as_ref().filter(|(d, _)| *d == n);
        let diff_button = (files > 0 || g.entry.changes.as_ref().is_some_and(|c| c.base.is_none())).then(|| {
            let label = match diff_open {
                Some(_) => "Hide file changes".to_string(),
                None if files > 0 => format!("Show file changes ({files})"),
                None => "Show files".to_string(),
            };
            button_with(
                ("gen-diff", n as usize),
                Some(Icon::FileText),
                label,
                ButtonKind::Ghost,
                ui,
                self.act(cx, move |v, cx| v.toggle_generation_diff(n, cx)),
            )
        });
        let diff = diff_open.map(|(_, result)| match result {
            None => kit::hint(ui, "Loading…").into_any_element(),
            Some(Ok(d)) if d.is_empty() => kit::hint(ui, "No generated file changed").into_any_element(),
            Some(Ok(d)) => self.diff_view(ui, cx, d).into_any_element(),
            Some(Err(e)) => kit::hint(ui, e.clone()).into_any_element(),
        });
        let name = kit::field(
            ui,
            &format!("Name (up to {NAME_MAX} characters)"),
            div().flex().items_center().gap(px(8.)).child(div().flex_1().min_w_0().child(self.gen_name.clone())).child(button(
                ("gen-name", n as usize),
                "Save",
                ButtonKind::Normal,
                ui,
                self.act(cx, |v, cx| v.save_generation_name(cx)),
            )),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pl(px(50.))
            .child(changes)
            .children(diff_button.map(|b| div().flex().child(b)))
            .children(diff)
            .child(name)
            .into_any_element()
    }

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
            let n = g.entry.number;
            let control = if g.entry.active {
                badge("Active", Tone::Success, ui).into_any_element()
            } else {
                // The row opens on click; the button mustn't toggle it too.
                div()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(button_with(
                        ("rollback", n as usize),
                        Some(Icon::RotateCcw),
                        "Roll back",
                        ButtonKind::Ghost,
                        ui,
                        self.act(cx, move |v, cx| v.rollback(n, cx)),
                    ))
                    .into_any_element()
            };
            let mut r = row(ui, g.entry.title()).icon(Icon::GitCommit).desc(gen_desc(&g.entry)).control(control);
            if g.rich {
                r = r.on_click(("gen-open", n as usize), cx.listener(move |v, _, _, cx| v.toggle_generation(n, cx)));
            }
            if g.rich && self.gen_open == Some(n) {
                // Clicks in the details (the name field) don't close them.
                r = r.below(
                    div()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.generation_details(ui, g, cx)),
                );
            }
            gens = gens.child(r);
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
        div()
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(status)
            .child(files)
            .child(share)
            .children(self.imported_plugins_section(ui, cx))
            .child(gens)
            .child(reset)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use goatty_config::PluginConfig;
    use goatty_plugin::api::Installed;
    use goatty_plugin::manifest::Manifest;

    use super::*;

    fn installed(id: &str) -> Installed {
        Installed {
            id: id.into(),
            version: "1.0.0".into(),
            source: "test".into(),
            verified: true,
            manifest: Manifest {
                schema: 1,
                id: id.into(),
                name: id.into(),
                version: "1.0.0".into(),
                summary: String::new(),
                description: String::new(),
                publisher: "test".into(),
                license: None,
                homepage: None,
                category: Default::default(),
                keywords: Vec::new(),
                requires: Default::default(),
                compat: Default::default(),
                packages: Vec::new(),
                zsh: Default::default(),
                env: Vec::new(),
                aliases: Vec::new(),
                files: Vec::new(),
                settings: Vec::new(),
                permissions: Default::default(),
            },
            enabled: true,
            incompatible: None,
            update: None,
            installed_unix: 0,
            builtin: false,
            packages: Vec::new(),
        }
    }

    #[test]
    fn missing_profile_plugins_filters_builtins_installed_and_discarded() {
        let imported = vec!["starship".into(), "greet".into(), "demo".into(), "removed".into()];
        let host = HostSettings {
            plugins: BTreeMap::from([
                ("starship".into(), PluginConfig::default()),
                ("greet".into(), PluginConfig::default()),
                ("demo".into(), PluginConfig::default()),
            ]),
            ..Default::default()
        };
        assert_eq!(missing_profile_plugins(&imported, None, None), Vec::<String>::new());
        assert_eq!(missing_profile_plugins(&imported, Some(&host), None), vec!["greet", "demo"]);

        let cat = Catalog { installed: vec![installed("greet")], ..Default::default() };
        assert_eq!(missing_profile_plugins(&imported, Some(&host), Some(&cat)), vec!["demo"]);
    }
}
