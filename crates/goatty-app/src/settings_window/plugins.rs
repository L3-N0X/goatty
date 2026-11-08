//! The settings window's Plugins page (Stage 5): what's installed, what
//! the registries offer, the install review, and each plugin's settings
//! rendered from its schema.
//!
//! Installing and removing are host operations (`goatty_plugin::api`
//! through the daemon, or in-process with `--local`). A plugin's settings
//! are host settings (`plugins.<id>` in `host.json`): they're edited in the
//! same draft as the Shell page and applied with Preview/Apply.

use std::collections::HashMap;

use goatty_config::{HostSettings, PluginConfig};
use goatty_plugin::api::{self, Catalog, Installed, PackageState, Plan};
use goatty_plugin::manifest::{Setting, SettingKind};
use goatty_plugin::{Permission, schema};
use gpui::{AnyElement, Context, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Subscription, div, prelude::*, px};
use serde_json::Value;

use super::{Outcome, SettingsWindow};
use crate::ui::controls::{ButtonKind, button, note, notice, row, section, segmented, stepper, toggle};
use crate::ui::style::Ui;
use crate::ui::text_input::{TextInput, TextInputEvent};

/// The page's own state (the catalog is the host's).
#[derive(Default)]
pub struct PluginsPage {
    pub catalog: Option<Catalog>,
    pub loading: bool,
    pub error: Option<String>,
    /// The install/update being reviewed.
    pub plan: Option<Box<Plan>>,
    pub show_code: bool,
    pub show_advanced: bool,
    /// Plugin whose Uninstall was clicked once (the second click removes it).
    pub confirm_uninstall: Option<String>,
    /// Text fields for string/path/list settings: `"<plugin>.<setting>"`.
    inputs: HashMap<String, Entity<TextInput>>,
    filter: Option<Entity<TextInput>>,
    _subs: Vec<Subscription>,
}

/// A host request's result, back on the UI thread.
type Done<T> = Box<dyn FnOnce(&mut SettingsWindow, anyhow::Result<T>, &mut Context<'_, SettingsWindow>)>;

fn badge(text: &str, color: gpui::Hsla) -> AnyElement {
    div()
        .px_2()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(0.6))
        .text_xs()
        .text_color(color)
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn size(bytes: u64) -> String {
    if bytes < 1_000_000 { format!("{} kB", bytes.div_ceil(1000)) } else { format!("{:.1} MB", bytes as f64 / 1e6) }
}

impl SettingsWindow {
    // ---- Host calls ---------------------------------------------------------

    fn plugin_call<T: Send + 'static>(
        &mut self,
        req: api::Request,
        map: fn(api::Reply) -> Option<T>,
        done: Done<T>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(link) = self.link.clone() else {
            self.plugins.error = Some("No host to manage plugins on.".into());
            cx.notify();
            return;
        };
        self.plugins.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let reply = link.plugin(req)?;
                    map(reply).ok_or_else(|| anyhow::anyhow!("unexpected reply from the host"))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.plugins.loading = false;
                done(view, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Load what's installed and available (`refresh`: fetch the registries).
    pub fn load_plugins(&mut self, refresh: bool, cx: &mut Context<'_, Self>) {
        self.plugin_call(
            api::Request::Catalog { refresh },
            |r| match r {
                api::Reply::Catalog(c) => Some(c),
                _ => None,
            },
            Box::new(|view, result, cx| match result {
                Ok(c) => {
                    view.plugins.catalog = Some(c);
                    view.plugins.error = None;
                    view.sync_plugin_inputs(cx);
                }
                Err(e) => view.plugins.error = Some(format!("{e:#}")),
            }),
            cx,
        );
    }

    /// Ask the host what installing (or updating to) `id` would do.
    pub fn review_plugin(&mut self, id: String, version: Option<String>, cx: &mut Context<'_, Self>) {
        self.plugins.show_code = false;
        self.plugin_call(
            api::Request::Plan { id, version },
            |r| match r {
                api::Reply::Plan(p) => Some(p),
                _ => None,
            },
            Box::new(|view, result, _cx| match result {
                Ok(p) => view.plugins.plan = Some(p),
                Err(e) => view.outcome = Some(Outcome { ok: false, lines: vec![format!("{e:#}")], diff: Vec::new(), reload: false }),
            }),
            cx,
        );
    }

    /// Install the reviewed plan (the host refuses if it changed since).
    pub fn install_reviewed(&mut self, cx: &mut Context<'_, Self>) {
        let Some(plan) = self.plugins.plan.take() else { return };
        let id = plan.id.clone();
        let req = api::Request::Install { id: plan.id.clone(), version: Some(plan.version.clone()), approved: plan.digest.clone() };
        self.plugin_call(req, changed, Box::new(move |view, result, cx| view.after_change(result, Some(id), true, cx)), cx);
    }

    pub fn uninstall_plugin(&mut self, id: String, cx: &mut Context<'_, Self>) {
        self.plugins.confirm_uninstall = None;
        let req = api::Request::Uninstall { id: id.clone() };
        self.plugin_call(req, changed, Box::new(move |view, result, cx| view.after_change(result, Some(id), false, cx)), cx);
    }

    /// An install or uninstall finished: report it, keep an unapplied
    /// draft in step with the host (which changed `plugins.<id>`), reload.
    fn after_change(&mut self, result: anyhow::Result<api::Changed>, id: Option<String>, installed: bool, cx: &mut Context<'_, Self>) {
        match result {
            Ok(c) => {
                if let (Some(draft), Some(id)) = (self.draft.as_mut(), id) {
                    if installed {
                        draft.edit_plugin(&id, |p| p.enabled = true);
                    } else {
                        draft.plugins.remove(&id);
                    }
                }
                self.outcome = Some(Outcome { ok: true, lines: c.lines, diff: Vec::new(), reload: c.reload });
            }
            Err(e) => self.outcome = Some(Outcome { ok: false, lines: vec![format!("{e:#}")], diff: Vec::new(), reload: false }),
        }
        self.load_host(cx);
        self.load_plugins(false, cx);
    }

    // ---- Settings -----------------------------------------------------------

    /// Stored values of a plugin (draft first), resolved against its schema.
    fn plugin_values(&self, inst: &Installed) -> schema::Values {
        let stored = self.host_value().and_then(|h| h.plugins.get(&inst.id)).map(|p| p.settings.clone()).unwrap_or_default();
        schema::resolve(&inst.manifest, &stored).0
    }

    fn edit_plugin(&mut self, id: &str, cx: &mut Context<'_, Self>, f: impl FnOnce(&mut PluginConfig)) {
        let id = id.to_owned();
        self.edit_host(cx, move |h: &mut HostSettings| h.edit_plugin(&id, f));
    }

    /// Set (or with `None` reset) one plugin setting in the draft.
    fn set_plugin_value(&mut self, id: &str, key: &str, value: Option<Value>, cx: &mut Context<'_, Self>) {
        let key = key.to_owned();
        self.edit_plugin(id, cx, move |p| match value {
            Some(v) => {
                p.settings.insert(key, v);
            }
            None => {
                p.settings.remove(&key);
            }
        });
    }

    /// Text fields for the installed plugins' string, path and list
    /// settings, showing the current values.
    pub fn sync_plugin_inputs(&mut self, cx: &mut Context<'_, Self>) {
        if self.plugins.filter.is_none() {
            let filter = cx.new(|cx| TextInput::new("Search plugins…", "", cx));
            self.plugins._subs.push(cx.subscribe(&filter, |_, _, _: &TextInputEvent, cx| cx.notify()));
            self.plugins.filter = Some(filter);
        }
        let Some(cat) = self.plugins.catalog.clone() else { return };
        for inst in &cat.installed {
            let values = self.plugin_values(inst);
            for s in inst.manifest.settings.iter().filter(|s| matches!(s.kind, SettingKind::String | SettingKind::Path | SettingKind::List))
            {
                let key = format!("{}.{}", inst.id, s.id);
                let text = match values.get(&s.id) {
                    Some(Value::Array(items)) => items.iter().map(schema::plain_string).collect::<Vec<_>>().join(", "),
                    Some(v) => schema::plain_string(v),
                    None => String::new(),
                };
                match self.plugins.inputs.get(&key) {
                    Some(input) => {
                        if input.read(cx).text() != text {
                            input.update(cx, |i, cx| i.set_text(text, cx));
                        }
                    }
                    None => {
                        let placeholder = s.placeholder.clone().unwrap_or_default();
                        let input = cx.new(|cx| TextInput::new(placeholder, text, cx));
                        let (id, setting) = (inst.id.clone(), s.clone());
                        self.plugins._subs.push(cx.subscribe(&input, move |view, _, ev: &TextInputEvent, cx| {
                            let (TextInputEvent::Changed(text) | TextInputEvent::Submitted(text)) = ev;
                            let value = if text.trim().is_empty() && setting.kind != SettingKind::List {
                                None // back to the default
                            } else {
                                match schema::parse_input(&setting, text) {
                                    Ok(v) => Some(v),
                                    Err(_) => return, // keep typing; invalid values never reach the draft
                                }
                            };
                            view.set_plugin_value(&id, &setting.id, value, cx);
                        }));
                        self.plugins.inputs.insert(key, input);
                    }
                }
            }
        }
    }

    // ---- Rendering ----------------------------------------------------------

    pub(super) fn plugins_page(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let label = self.link.as_ref().map(|l| l.label()).unwrap_or_else(|| "no host".into());
        let mut page = div().flex().flex_col().gap_1().child(note(
            ui,
            format!(
                "Plugins install command-line tools into goatty's own directory on {label} (never system-wide, no sudo) and wire them into goatty's shell. Their settings are host settings: change them here, then preview and apply."
            ),
        ));
        if let Some(plan) = &self.plugins.plan {
            return page.child(self.plan_view(ui, plan, cx)).into_any_element();
        }
        page = page.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .pt_2()
                .children(self.plugins.filter.clone().map(|f| div().flex_1().child(f)))
                .child(button(
                    "plugins-refresh",
                    "Refresh registries",
                    ButtonKind::Normal,
                    ui,
                    self.act(cx, |v, cx| v.load_plugins(true, cx)),
                ))
                .when(self.plugins.loading, |d| d.child(div().text_sm().text_color(ui.muted).child("Working…"))),
        );
        if let Some(e) = &self.plugins.error {
            page = page.child(notice(ui, ui.danger, vec![e.clone()]));
        }
        let Some(cat) = &self.plugins.catalog else {
            return page.child(note(ui, if self.plugins.loading { "Loading…" } else { "" })).into_any_element();
        };
        for r in cat.registries.iter().filter(|r| r.error.is_some()) {
            page = page.child(notice(ui, ui.warning, vec![format!("Registry {}: {}", r.name, r.error.clone().unwrap_or_default())]));
        }
        if !cat.issues.is_empty() {
            page = page.child(notice(ui, ui.warning, cat.issues.iter().map(|i| format!("⚠ {i}")).collect()));
        }

        let query = self.plugins.filter.as_ref().map(|f| f.read(cx).text().to_lowercase()).unwrap_or_default();
        let matches = |id: &str, name: &str, summary: &str, keywords: &[String]| {
            query.is_empty()
                || id.contains(&query)
                || name.to_lowercase().contains(&query)
                || summary.to_lowercase().contains(&query)
                || keywords.iter().any(|k| k.contains(&query))
        };

        let shown = |i: &&Installed| matches(&i.id, &i.manifest.name, &i.manifest.summary, &i.manifest.keywords);
        let builtin: Vec<&Installed> = cat.installed.iter().filter(|i| i.builtin).filter(shown).collect();
        if !builtin.is_empty() {
            page = page.child(section(ui, "Built in"));
            for inst in builtin {
                page = page.child(self.installed_card(ui, inst, cx));
            }
        }
        page = page.child(section(ui, "Installed"));
        let installed: Vec<&Installed> = cat.installed.iter().filter(|i| !i.builtin).filter(shown).collect();
        if installed.is_empty() {
            page = page
                .child(note(ui, if cat.installed.iter().all(|i| i.builtin) { "Nothing yet: pick something below." } else { "No match." }));
        }
        for inst in installed {
            page = page.child(self.installed_card(ui, inst, cx));
        }
        if cat.installed.iter().any(|i| i.manifest.settings.iter().any(|s| s.advanced)) {
            page = page.child(row(
                ui,
                "Show advanced settings",
                None,
                toggle(
                    "plugins-advanced",
                    self.plugins.show_advanced,
                    ui,
                    self.act(cx, |v, cx| {
                        v.plugins.show_advanced = !v.plugins.show_advanced;
                        cx.notify();
                    }),
                ),
            ));
        }

        page = page.child(section(ui, "Available"));
        let available: Vec<&api::Available> =
            cat.available.iter().filter(|a| a.installed.is_none() && matches(&a.id, &a.name, &a.summary, &a.keywords)).collect();
        if available.is_empty() {
            let why = if cat.registries.iter().all(|r| r.sequence.is_none()) {
                "No registry could be read yet. Check the connection and press Refresh."
            } else {
                "Nothing else to install."
            };
            page = page.child(note(ui, why));
        }
        for a in available {
            page = page.child(self.available_card(ui, a, cx));
        }
        let regs: Vec<String> = cat
            .registries
            .iter()
            .map(|r| match r.sequence {
                Some(seq) => format!("{} (#{seq}, {} plugins)", r.name, r.plugins),
                None => format!("{} (not fetched)", r.name),
            })
            .collect();
        page = page.child(note(ui, format!("Registries: {}. Add others with `goattyd plugin registry add`.", regs.join(", "))));
        page.into_any_element()
    }

    fn card(ui: Ui) -> gpui::Div {
        div().flex().flex_col().gap_1().my_1().p_3().rounded_md().border_1().border_color(ui.border).bg(ui.surface)
    }

    fn perm_badges(ui: Ui, perms: &[Permission]) -> Vec<AnyElement> {
        let mut v = Vec::new();
        if perms.contains(&Permission::Binaries) {
            v.push(badge("installs programs", ui.muted));
        }
        if perms.contains(&Permission::ShellCode) {
            v.push(badge("runs shell code", ui.warning));
        }
        if perms.contains(&Permission::Network) {
            v.push(badge("uses the network", ui.muted));
        }
        v
    }

    fn available_card(&self, ui: Ui, a: &api::Available, cx: &Context<'_, Self>) -> AnyElement {
        let (id, version) = (a.id.clone(), a.version.clone());
        Self::card(ui)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().font_weight(FontWeight::BOLD).child(SharedString::from(a.name.clone())))
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui.muted)
                            .child(SharedString::from(format!("{} · by {} · {}", a.version, a.publisher, a.registry))),
                    )
                    .children(Self::perm_badges(ui, &a.permissions))
                    .child(div().flex_1())
                    .when(a.incompatible.is_none(), |d| {
                        d.child(button(
                            SharedString::from(format!("review-{id}")),
                            "Review & install…",
                            ButtonKind::Primary,
                            ui,
                            self.act(cx, move |v, cx| v.review_plugin(id.clone(), Some(version.clone()), cx)),
                        ))
                    }),
            )
            .child(div().text_sm().child(SharedString::from(a.summary.clone())))
            .when_some(a.incompatible.clone(), |d, why| d.child(note(ui, format!("Not for this host: {why}"))))
            .into_any_element()
    }

    fn installed_card(&self, ui: Ui, inst: &Installed, cx: &Context<'_, Self>) -> AnyElement {
        let id = inst.id.clone();
        let enabled = self.host_value().and_then(|h| h.plugins.get(&id)).is_none_or(|p| p.enabled);
        let confirming = self.plugins.confirm_uninstall.as_deref() == Some(id.as_str());
        let source = if inst.builtin {
            "comes with goatty".to_string()
        } else if inst.verified {
            format!("{} · {}", inst.version, inst.source)
        } else {
            format!("{} · local, unverified", inst.version)
        };
        // A built-in plugin whose tool is missing or older than its pin: goatty can download it.
        let fetch = inst.builtin && inst.packages.iter().any(|p| p.available && p.needs_install());
        let outdated = inst.packages.iter().any(|p| matches!(&p.state, PackageState::Managed { version } if *version != p.version));
        let mut card = Self::card(ui).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().font_weight(FontWeight::BOLD).child(SharedString::from(inst.manifest.name.clone())))
                .child(div().text_sm().text_color(ui.muted).child(SharedString::from(source)))
                .children(Self::perm_badges(ui, &goatty_plugin::permissions::all(&inst.manifest)))
                .child(div().flex_1())
                .when_some(inst.update.clone(), |d, newer| {
                    let (id, v) = (id.clone(), newer.clone());
                    d.child(button(
                        SharedString::from(format!("update-{id}")),
                        format!("Update to {newer}…"),
                        ButtonKind::Primary,
                        ui,
                        self.act(cx, move |view, cx| view.review_plugin(id.clone(), Some(v.clone()), cx)),
                    ))
                })
                .when(fetch, |d| {
                    let id = id.clone();
                    d.child(button(
                        SharedString::from(format!("fetch-{id}")),
                        if outdated { "Update…" } else { "Install…" },
                        ButtonKind::Primary,
                        ui,
                        self.act(cx, move |v, cx| v.review_plugin(id.clone(), None, cx)),
                    ))
                })
                .when(!inst.builtin, |d| {
                    let id = id.clone();
                    d.child(button(
                        SharedString::from(format!("uninstall-{id}")),
                        if confirming { "Really uninstall?" } else { "Uninstall" },
                        ButtonKind::Danger,
                        ui,
                        self.act(cx, move |v, cx| {
                            if v.plugins.confirm_uninstall.as_deref() == Some(id.as_str()) {
                                v.uninstall_plugin(id.clone(), cx);
                            } else {
                                v.plugins.confirm_uninstall = Some(id.clone());
                                cx.notify();
                            }
                        }),
                    ))
                })
                .child({
                    let id = id.clone();
                    toggle(
                        SharedString::from(format!("enabled-{id}")),
                        enabled,
                        ui,
                        self.act(cx, move |v, cx| v.edit_plugin(&id, cx, move |p| p.enabled = !enabled)),
                    )
                }),
        );
        card = card.child(div().text_sm().child(SharedString::from(inst.manifest.summary.clone())));
        if inst.builtin {
            for p in &inst.packages {
                let line = match &p.state {
                    PackageState::System { path } => format!("Uses {path}"),
                    PackageState::Managed { version } if *version == p.version => format!("goatty's copy, {version}"),
                    PackageState::Managed { version } => format!("goatty's copy, {version} ({} available)", p.version),
                    PackageState::Missing if p.available => "Not installed yet: Install… downloads it into goatty's directory".into(),
                    PackageState::Missing => {
                        "Not installed, and goatty has no build for this host: install it with your package manager".into()
                    }
                };
                card = card.child(note(ui, line));
            }
        }
        if let Some(why) = &inst.incompatible {
            card = card.child(note(ui, format!("Off on this host: {why}")));
        }
        if !enabled {
            let why = if inst.builtin { "Off: not loaded in new shells." } else { "Disabled: installed, but not loaded in new shells." };
            return card.child(note(ui, why)).into_any_element();
        }
        let values = self.plugin_values(inst);
        let mut group: Option<&str> = None;
        for s in inst.manifest.settings.iter().filter(|s| self.plugins.show_advanced || !s.advanced) {
            if s.group.as_deref() != group {
                group = s.group.as_deref();
                if let Some(g) = group {
                    card = card.child(
                        div()
                            .pt_2()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(ui.muted)
                            .child(SharedString::from(g.to_uppercase())),
                    );
                }
            }
            card = card.child(self.setting_row(ui, inst, s, &values, cx));
        }
        card.into_any_element()
    }

    /// One schema-driven control.
    fn setting_row(&self, ui: Ui, inst: &Installed, s: &Setting, values: &schema::Values, cx: &Context<'_, Self>) -> AnyElement {
        let id = inst.id.clone();
        let key = s.id.clone();
        let value = values.get(&s.id).cloned().unwrap_or(Value::Null);
        let help = (!s.help.is_empty()).then_some(s.help.as_str());
        let elem_prefix = format!("ps-{id}-{key}");
        let elem_id = |suffix: &str| SharedString::from(format!("{elem_prefix}{suffix}"));
        let this = cx.entity().downgrade();
        let set = move |v: Option<Value>| {
            let (this, id, key) = (this.clone(), id.clone(), key.clone());
            move |_: &mut gpui::Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |view, cx| view.set_plugin_value(&id, &key, v.clone(), cx));
            }
        };
        let control: AnyElement = match s.kind {
            SettingKind::Bool => {
                let on = value.as_bool().unwrap_or(false);
                toggle(elem_id(""), on, ui, set(Some(Value::Bool(!on)))).into_any_element()
            }
            SettingKind::Choice if s.choices.len() > 4 => {
                // Too many for one row: a list, one choice per line.
                let selected = schema::plain_string(&value);
                let mut list = div().flex().flex_col().w(px(220.)).rounded_md().border_1().border_color(ui.border).overflow_hidden();
                for (i, c) in s.choices.iter().enumerate() {
                    let on = c.value == selected;
                    list = list.child(
                        div()
                            .id(elem_id(&format!("-{i}")))
                            .px_3()
                            .py_1()
                            .text_sm()
                            .cursor_pointer()
                            .when(on, |d| d.bg(ui.accent).text_color(ui.on_accent))
                            .when(!on, |d| d.hover(|st| st.bg(ui.surface_hover)))
                            .child(SharedString::from(c.label.clone()))
                            .on_click({
                                let f = set(Some(Value::String(c.value.clone())));
                                move |_, window, cx| f(window, cx)
                            }),
                    );
                }
                list.into_any_element()
            }
            SettingKind::Choice => {
                let selected = schema::plain_string(&value);
                let options: Vec<(String, &str)> = s.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
                let this = cx.entity().downgrade();
                let (id, key) = (inst.id.clone(), s.id.clone());
                segmented(&elem_id(""), &options, &selected, ui, move |v: String, _, cx| {
                    let _ = this.update(cx, |view, cx| view.set_plugin_value(&id, &key, Some(Value::String(v)), cx));
                })
                .into_any_element()
            }
            SettingKind::Integer => {
                let n = value.as_i64().unwrap_or(0);
                let step = s.step.unwrap_or(1);
                let clamp = |x: i64| x.clamp(s.min.unwrap_or(i64::MIN), s.max.unwrap_or(i64::MAX));
                let shown = match &s.unit {
                    Some(u) => format!("{n} {u}"),
                    None => n.to_string(),
                };
                stepper(&elem_id(""), shown, ui, set(Some(Value::from(clamp(n - step)))), set(Some(Value::from(clamp(n + step)))))
                    .into_any_element()
            }
            SettingKind::String | SettingKind::Path | SettingKind::List => {
                match self.plugins.inputs.get(&format!("{}.{}", inst.id, s.id)) {
                    Some(input) => div().w(px(260.)).child(input.clone()).into_any_element(),
                    None => div().child(SharedString::from(schema::plain_string(&value))).into_any_element(),
                }
            }
        };
        let mut r = row(ui, &s.label, help, control).into_any_element();
        if s.kind == SettingKind::Choice
            && let Some(c) = s.choices.iter().find(|c| c.value == schema::plain_string(&value)).filter(|c| !c.help.is_empty())
        {
            r = div().flex().flex_col().child(r).child(note(ui, format!("{}: {}", c.label, c.help))).into_any_element();
        }
        r
    }

    /// The install review: everything the plugin will do, before it does it.
    fn plan_view(&self, ui: Ui, plan: &Plan, cx: &Context<'_, Self>) -> AnyElement {
        let mono: SharedString =
            cx.global::<crate::settings::AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into()).into();
        let title = match &plan.replaces {
            Some(old) if *old != plan.version => format!("Update {} {old} → {}", plan.name, plan.version),
            Some(_) => format!("Reinstall {} {}", plan.name, plan.version),
            None => format!("Install {} {}", plan.name, plan.version),
        };
        let source = if plan.builtin {
            "Comes with goatty: this installs the tool it needs into goatty's own directory".to_string()
        } else if plan.verified {
            format!("From the {} registry (signature checked) · by {}", plan.source, plan.publisher)
        } else {
            format!("From {} (not from a registry) · by {}", plan.source, plan.publisher)
        };
        let mut v = Self::card(ui)
            .child(div().text_lg().font_weight(FontWeight::BOLD).child(SharedString::from(title)))
            .child(div().text_sm().text_color(ui.muted).child(SharedString::from(source)))
            .child(div().child(SharedString::from(plan.summary.clone())));
        if !plan.description.trim().is_empty() {
            v = v.child(div().text_sm().child(SharedString::from(plan.description.trim().to_string())));
        }
        if let Some(h) = &plan.homepage {
            v = v.child(note(ui, h.clone()));
        }
        if !plan.warnings.is_empty() {
            v = v.child(notice(ui, ui.warning, plan.warnings.iter().map(|w| format!("⚠ {w}")).collect()));
        }
        if !plan.downloads.is_empty() {
            v = v.child(section(ui, "Downloads"));
            for d in &plan.downloads {
                let line = match &d.system {
                    Some(p) => format!("{} {}: already installed at {p}; goatty uses that copy", d.package, d.version),
                    None if d.current => format!("{} {}: goatty's copy is already installed", d.package, d.version),
                    None => format!("{} {} · {} · {}", d.package, d.version, size(d.size), d.url),
                };
                v = v.child(div().text_sm().child(SharedString::from(line)));
                if d.fetches() {
                    v = v.child(
                        div()
                            .text_xs()
                            .text_color(ui.muted)
                            .font_family(mono.clone())
                            .child(SharedString::from(format!("sha256 {}", d.sha256))),
                    );
                }
            }
            v = v.child(note(
                ui,
                "Into goatty's own directory, no sudo. Each download is checked against the sha256 in the signed manifest before anything is unpacked.",
            ));
        }
        v = v.child(section(ui, "What it may do"));
        for g in &plan.permissions {
            let color = if g.permission.runs_code() { ui.warning } else { ui.text };
            let mut item = div().flex().flex_col().py_1().child(div().text_color(color).child(SharedString::from(format!(
                "{} {}{}",
                if g.permission.runs_code() { "⚠" } else { "•" },
                g.title,
                if g.details.is_empty() { String::new() } else { format!(": {}", g.details.join(", ")) }
            ))));
            if !g.reason.is_empty() {
                item = item.child(div().text_sm().child(SharedString::from(format!("Why: {}", g.reason))));
            }
            item = item.child(note(ui, g.meaning.clone()));
            v = v.child(item);
        }
        let list = |v: gpui::Div, title: &str, items: &[String]| {
            if items.is_empty() {
                return v;
            }
            let mut v = v.child(section(ui, title));
            for i in items {
                v = v.child(div().text_sm().font_family(mono.clone()).child(SharedString::from(i.clone())));
            }
            v
        };
        v = list(v, "Aliases", &plan.aliases);
        v = list(v, "Environment", &plan.env);
        v = list(v, "Needs", &plan.requires);
        if let Some((file, code)) = &plan.shell_code {
            v = v.child(section(ui, "Shell code")).child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(note(ui, format!("{file} runs at the start of every goatty shell ({} lines).", code.lines().count())))
                    .child(button(
                        "plan-code",
                        if self.plugins.show_code { "Hide code" } else { "Read the code" },
                        ButtonKind::Normal,
                        ui,
                        self.act(cx, |v, cx| {
                            v.plugins.show_code = !v.plugins.show_code;
                            cx.notify();
                        }),
                    )),
            );
            if self.plugins.show_code {
                v = v.child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(ui.input_bg)
                        .border_1()
                        .border_color(ui.border)
                        .font_family(mono.clone())
                        .text_xs()
                        .children(
                            code.lines()
                                .map(|l| div().child(SharedString::from(if l.is_empty() { " ".to_string() } else { l.to_string() }))),
                        ),
                );
            }
        }
        let verb = if plan.replaces.is_some() { "Update" } else { "Install" };
        v.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .pt_3()
                .when(self.plugins.loading, |d| d.child(div().text_sm().text_color(ui.muted).child("Installing…")))
                .child(button(
                    "plan-cancel",
                    "Cancel",
                    ButtonKind::Normal,
                    ui,
                    self.act(cx, |v, cx| {
                        v.plugins.plan = None;
                        cx.notify();
                    }),
                ))
                .child(button("plan-install", verb, ButtonKind::Primary, ui, self.act(cx, |v, cx| v.install_reviewed(cx)))),
        )
        .into_any_element()
    }
}

fn changed(r: api::Reply) -> Option<api::Changed> {
    match r {
        api::Reply::Changed(c) => Some(c),
        _ => None,
    }
}
