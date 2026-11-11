//! The settings window's Plugins page (Stage 5): the installed plugins as
//! a list, each with its own page (settings from its schema, grouped as
//! the manifest groups them; its tools, permissions and aliases), what the
//! registries offer, and the install review.
//!
//! Installing and removing are host operations (`goatty_plugin::api`
//! through the daemon, or in-process with `--local`). A plugin's settings
//! are host settings (`plugins.<id>` in `host.json`): they're edited in the
//! same draft as the Shell page and applied with Preview/Apply.

use std::collections::HashMap;

use goatty_config::{HostSettings, PluginConfig};
use goatty_plugin::api::{self, Catalog, Installed, PackageState, Plan};
use goatty_plugin::manifest::{Category, Setting, SettingKind};
use goatty_plugin::{Permission, permissions, schema};
use gpui::{
    AnyElement, Context, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Subscription, Window, div, prelude::*, px,
};
use serde_json::Value;

use super::kit::{self, card, opt, row, search_field, section};
use super::{Outcome, Page, SettingsWindow};
use crate::ui::controls::{ButtonKind, Tone, badge, banner, button, button_with, icon_button, segmented, stepper, toggle, tooltip};
use crate::ui::focus::{self, Ring, Scroller};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};
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
    /// The installed plugin whose page is open.
    pub detail: Option<String>,
    pub tab: Tab,
    /// Plugin whose Uninstall was clicked once (the second click removes it).
    pub confirm_uninstall: Option<String>,
    /// Text fields for string/path/list settings: `"<plugin>.<setting>"`.
    inputs: HashMap<String, Entity<TextInput>>,
    filter: Option<Entity<TextInput>>,
    _subs: Vec<Subscription>,
}

/// Which list the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Installed,
    Browse,
}

/// A host request's result, back on the UI thread.
type Done<T> = Box<dyn FnOnce(&mut SettingsWindow, anyhow::Result<T>, &mut Context<'_, SettingsWindow>)>;

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
                Ok(p) => {
                    view.plugins.plan = Some(p);
                    view.scroll.set_offset(gpui::point(px(0.), px(0.)));
                }
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
            let filter = cx.new(|cx| TextInput::new("Search plugins", "", cx).bare());
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
                        let input = cx.new(|cx| TextInput::new(placeholder, text, cx).label(s.label.clone()));
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

    pub(super) fn plugins_page(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        if let Some(plan) = &self.plugins.plan {
            return self.plan_view(ui, plan, cx);
        }
        if let Some(inst) =
            self.plugins.detail.as_ref().and_then(|id| self.plugins.catalog.as_ref()?.installed.iter().find(|i| &i.id == id))
        {
            return self.plugin_detail(ui, inst, cx);
        }
        let mut page = div().flex().flex_col().gap(px(20.));
        if let Some(e) = &self.plugins.error {
            page = page.child(banner(ui, Tone::Danger, "Couldn't talk to the host", vec![e.clone()]));
        }
        let Some(cat) = &self.plugins.catalog else {
            return page
                .child(card(ui).child(kit::empty(
                    ui,
                    Icon::Loader,
                    if self.plugins.loading { "Loading plugins…" } else { "No plugins to show" },
                    format!("From {}", self.host_label()),
                )))
                .into_any_element();
        };
        for r in cat.registries.iter().filter(|r| r.error.is_some()) {
            page = page.child(banner(
                ui,
                Tone::Warning,
                format!("Couldn't read the {} registry", r.name),
                vec![r.error.clone().unwrap_or_default()],
            ));
        }
        if !cat.issues.is_empty() {
            page = page.child(banner(ui, Tone::Warning, "Some plugins have problems", cat.issues.clone()));
        }

        let query = self.plugins.filter.as_ref().map(|f| f.read(cx).text().to_lowercase()).unwrap_or_default();
        let matches = |id: &str, name: &str, summary: &str, keywords: &[String]| {
            query.is_empty()
                || id.contains(&query)
                || name.to_lowercase().contains(&query)
                || summary.to_lowercase().contains(&query)
                || keywords.iter().any(|k| k.contains(&query))
        };
        let available: Vec<&api::Available> = cat.available.iter().filter(|a| a.installed.is_none()).collect();
        let tab = self.plugins.tab;
        let installed_label = format!("Installed  {}", cat.installed.len());
        let browse_label = format!("Browse  {}", available.len());
        page = page.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(segmented(
                    "plugins-tab",
                    &[(Tab::Installed, installed_label.as_str()), (Tab::Browse, browse_label.as_str())],
                    &tab,
                    ui,
                    {
                        let this = cx.entity().downgrade();
                        move |t, _, cx| {
                            let _ = this.update(cx, |v, cx| {
                                v.plugins.tab = t;
                                cx.notify();
                            });
                        }
                    },
                ))
                .children(self.plugins.filter.as_ref().map(|f| div().flex_1().child(search_field(ui, "plugin-search", f, window, cx))))
                .child(
                    icon_button(
                        "plugins-refresh",
                        if self.plugins.loading { Icon::Loader } else { Icon::Refresh },
                        "Check the registries for new plugins and updates",
                        ui,
                        self.act(cx, |v, cx| v.load_plugins(true, cx)),
                    )
                    .size(px(38.)),
                ),
        );

        match tab {
            Tab::Installed => {
                let shown = |i: &&Installed| matches(&i.id, &i.manifest.name, &i.manifest.summary, &i.manifest.keywords);
                let builtin: Vec<&Installed> = cat.installed.iter().filter(|i| i.builtin).filter(shown).collect();
                let installed: Vec<&Installed> = cat.installed.iter().filter(|i| !i.builtin).filter(shown).collect();
                if !installed.is_empty() || query.is_empty() {
                    let mut s = section(ui, "Installed");
                    if installed.is_empty() {
                        s = s.child(
                            row(ui, "Nothing installed yet")
                                .icon(Icon::Package)
                                .desc("Add tools like eza, zoxide or bat, in goatty's own directory: never system-wide, no sudo")
                                .on_click(
                                    "plugins-browse",
                                    cx.listener(|v, _, _, cx| {
                                        v.plugins.tab = Tab::Browse;
                                        cx.notify();
                                    }),
                                ),
                        );
                    }
                    page = page.child(s.children(installed.into_iter().map(|i| self.plugin_row(ui, i, cx))));
                }
                if !builtin.is_empty() {
                    page = page.child(
                        section(ui, "Built in")
                            .desc("Come with goatty: switch them off instead of uninstalling")
                            .children(builtin.into_iter().map(|i| self.plugin_row(ui, i, cx))),
                    );
                }
            }
            Tab::Browse => {
                let list: Vec<&&api::Available> = available.iter().filter(|a| matches(&a.id, &a.name, &a.summary, &a.keywords)).collect();
                if list.is_empty() {
                    let (title, why) = if cat.registries.iter().all(|r| r.sequence.is_none()) {
                        ("No registry could be read", "Check the connection and press refresh.")
                    } else if !query.is_empty() {
                        ("No plugin matches", "Search looks at names, summaries and keywords.")
                    } else {
                        ("You have them all", "Every plugin the registries offer is installed.")
                    };
                    page = page.child(card(ui).child(kit::empty(ui, Icon::Puzzle, title, why)));
                } else {
                    page = page
                        .child(div().grid().grid_cols(2).gap(px(12.)).children(list.into_iter().map(|a| self.available_card(ui, a, cx))));
                }
            }
        }
        let regs: Vec<String> = cat
            .registries
            .iter()
            .map(|r| match r.sequence {
                Some(_) => format!("{} ({} plugins)", r.name, r.plugins),
                None => format!("{} (not fetched)", r.name),
            })
            .collect();
        page.child(kit::hint(ui, format!("Registries: {}. Add one with goattyd plugin registry add.", regs.join(", "))).px(px(4.)))
            .into_any_element()
    }

    /// The round icon standing for a plugin.
    fn plugin_avatar(ui: Ui, category: Category, builtin: bool, size: f32) -> impl IntoElement {
        let icon = match category {
            Category::Prompt => Icon::TextCursorInput,
            Category::Application => Icon::AppWindow,
            Category::Environment if builtin => Icon::Zap,
            Category::Environment => Icon::Package,
        };
        div()
            .flex_shrink_0()
            .size(px(size))
            .rounded(px(size * 0.3))
            .bg(ui.accent_soft)
            .flex()
            .items_center()
            .justify_center()
            .child(icon.svg().size(px(size * 0.5)).text_color(ui.accent_text))
    }

    fn enabled(&self, id: &str) -> bool {
        self.host_value().and_then(|h| h.plugins.get(id)).is_none_or(|p| p.enabled)
    }

    /// An installed plugin in the list: opens its page.
    fn plugin_row(&self, ui: Ui, inst: &Installed, cx: &Context<'_, Self>) -> AnyElement {
        let id = inst.id.clone();
        let enabled = self.enabled(&id);
        let missing = inst.packages.iter().any(|p| matches!(p.state, PackageState::Missing));
        let fetch = inst.builtin && inst.packages.iter().any(|p| p.available && p.needs_install());
        let open_id = id.clone();
        let row_id: gpui::ElementId = SharedString::from(format!("plugin-{id}")).into();
        let row = div()
            .id(row_id.clone())
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(inst.manifest.name.clone()))
            .flex()
            .items_center()
            .gap(px(14.))
            .min_h(px(64.))
            .px(px(16.))
            .py(px(10.))
            .cursor_pointer()
            .rounded(px(radius::CARD))
            .hover(|s| s.bg(ui.surface_hover))
            .child(Self::plugin_avatar(ui, inst.manifest.category, inst.builtin, 38.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(text::BODY))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if enabled { ui.text } else { ui.muted })
                                    .child(SharedString::from(inst.manifest.name.clone())),
                            )
                            .when_some(inst.update.clone(), |d, v| d.child(badge(format!("Update {v}"), Tone::Accent, ui)))
                            .when(fetch && enabled, |d| d.child(badge(if missing { "Needs install" } else { "Update" }, Tone::Warning, ui)))
                            .when(inst.incompatible.is_some(), |d| d.child(badge("Not for this host", Tone::Danger, ui)))
                            .when(!inst.verified && !inst.builtin, |d| d.child(badge("Unverified", Tone::Warning, ui))),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(ui.muted)
                            .truncate()
                            .child(SharedString::from(inst.manifest.summary.clone())),
                    ),
            )
            .child({
                let id = id.clone();
                div().on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(
                    toggle(
                        SharedString::from(format!("enabled-{id}")),
                        enabled,
                        ui,
                        self.act(cx, move |v, cx| v.edit_plugin(&id, cx, move |p| p.enabled = !enabled)),
                    )
                    .aria_label(SharedString::from(format!("{} plugin", inst.manifest.name))),
                )
            })
            .child(Icon::ChevronRight.svg().size(px(16.)).text_color(ui.faint))
            .on_click(cx.listener(move |v, _, _, cx| {
                v.plugins.detail = Some(open_id.clone());
                v.plugins.confirm_uninstall = None;
                v.plugins.show_advanced = false;
                v.scroll.set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            }));
        focus::tab(row, &row_id, ui, Ring::Inner, Scroller::Page).into_any_element()
    }

    fn perm_icons(ui: Ui, perms: &[Permission]) -> impl IntoElement {
        let item = |p: Permission, icon: Icon, tone: gpui::Hsla| {
            perms.contains(&p).then(|| {
                div()
                    .id(SharedString::from(format!("perm-{}", p.key())))
                    .size(px(24.))
                    .rounded_full()
                    .bg(tone.opacity(0.12))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon.svg().size(px(13.)).text_color(tone))
                    .tooltip(tooltip(p.title()))
            })
        };
        div()
            .flex()
            .gap(px(4.))
            .children(item(Permission::Binaries, Icon::Package, ui.muted))
            .children(item(Permission::ShellCode, Icon::Code, ui.warning))
            .children(item(Permission::Network, Icon::Globe, ui.muted))
    }

    fn available_card(&self, ui: Ui, a: &api::Available, cx: &Context<'_, Self>) -> AnyElement {
        let (id, version) = (a.id.clone(), a.version.clone());
        card(ui)
            .p(px(16.))
            .gap(px(10.))
            .child(
                div().flex().items_center().gap(px(12.)).child(Self::plugin_avatar(ui, a.category, false, 38.)).child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(SharedString::from(a.name.clone())),
                        )
                        .child(
                            div()
                                .text_size(px(text::CAPTION))
                                .text_color(ui.muted)
                                .truncate()
                                .child(SharedString::from(format!("{} · {}", a.publisher, a.version))),
                        ),
                ),
            )
            .child(
                div().h(px(40.)).text_size(px(text::SMALL)).text_color(ui.muted).line_clamp(2).child(SharedString::from(a.summary.clone())),
            )
            .child(
                div().flex().items_center().justify_between().child(Self::perm_icons(ui, &a.permissions)).child(match &a.incompatible {
                    Some(why) => div()
                        .id(SharedString::from(format!("incompat-{id}")))
                        .child(badge("Not for this host", Tone::Neutral, ui))
                        .tooltip(tooltip(why.clone()))
                        .into_any_element(),
                    None => button_with(
                        SharedString::from(format!("review-{id}")),
                        Some(Icon::Download),
                        "Install",
                        ButtonKind::Normal,
                        ui,
                        self.act(cx, move |v, cx| v.review_plugin(id.clone(), Some(version.clone()), cx)),
                    )
                    .into_any_element(),
                }),
            )
            .into_any_element()
    }

    // ---- A plugin's page ----------------------------------------------------

    fn plugin_detail(&self, ui: Ui, inst: &Installed, cx: &Context<'_, Self>) -> AnyElement {
        let id = inst.id.clone();
        let enabled = self.enabled(&id);
        let confirming = self.plugins.confirm_uninstall.as_deref() == Some(id.as_str());
        let fetch = inst.builtin && inst.packages.iter().any(|p| p.available && p.needs_install());
        let outdated = inst.packages.iter().any(|p| matches!(&p.state, PackageState::Managed { version } if *version != p.version));
        let source = if inst.builtin {
            "Built into goatty".to_string()
        } else if inst.verified {
            format!("From the {} registry", inst.source)
        } else {
            "Installed from a local file, unverified".to_string()
        };
        let mut by = format!("Version {} · {}", inst.version, inst.manifest.publisher);
        if let Some(l) = &inst.manifest.license {
            by.push_str(&format!(" · {l}"));
        }

        let mut actions = div().flex().flex_wrap().items_center().gap(px(8.));
        if let Some(newer) = inst.update.clone() {
            let (id, v) = (id.clone(), newer.clone());
            actions = actions.child(button_with(
                SharedString::from(format!("update-{id}")),
                Some(Icon::Download),
                format!("Update to {newer}"),
                ButtonKind::Primary,
                ui,
                self.act(cx, move |view, cx| view.review_plugin(id.clone(), Some(v.clone()), cx)),
            ));
        }
        if fetch {
            let id = id.clone();
            actions = actions.child(button_with(
                SharedString::from(format!("fetch-{id}")),
                Some(Icon::Download),
                if outdated { "Update the tool" } else { "Install the tool" },
                ButtonKind::Primary,
                ui,
                self.act(cx, move |v, cx| v.review_plugin(id.clone(), None, cx)),
            ));
        }
        if let Some(home) = inst.manifest.homepage.clone() {
            actions =
                actions.child(button_with("plugin-home", Some(Icon::ExternalLink), "Website", ButtonKind::Ghost, ui, move |_, cx| {
                    cx.open_url(&home)
                }));
        }
        actions = actions.child(div().flex_1());
        if !inst.builtin {
            let id = id.clone();
            actions = actions.child(button_with(
                SharedString::from(format!("uninstall-{id}")),
                Some(Icon::Trash),
                if confirming { "Really uninstall?" } else { "Uninstall" },
                ButtonKind::Danger,
                ui,
                self.act(cx, move |v, cx| {
                    if v.plugins.confirm_uninstall.as_deref() == Some(id.as_str()) {
                        v.uninstall_plugin(id.clone(), cx);
                        v.plugins.detail = None;
                    } else {
                        v.plugins.confirm_uninstall = Some(id.clone());
                        cx.notify();
                    }
                }),
            ));
        }
        let hero = card(ui)
            .p(px(20.))
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(Self::plugin_avatar(ui, inst.manifest.category, inst.builtin, 52.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(SharedString::from(source)),
                                    )
                                    .when(inst.verified && !inst.builtin, |d| {
                                        d.child(
                                            div()
                                                .id("verified")
                                                .child(Icon::ShieldCheck.svg().size(px(16.)).text_color(ui.success))
                                                .tooltip(tooltip("Signed by its registry")),
                                        )
                                    }),
                            )
                            .child(div().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(by))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(4.))
                            .child(
                                toggle(SharedString::from(format!("enabled-{id}")), enabled, ui, {
                                    let id = id.clone();
                                    self.act(cx, move |v, cx| v.edit_plugin(&id, cx, move |p| p.enabled = !enabled))
                                })
                                .aria_label(SharedString::from(format!("{} plugin", inst.manifest.name))),
                            )
                            .child(div().text_size(px(text::CAPTION)).text_color(ui.muted).child(if enabled { "On" } else { "Off" })),
                    ),
            )
            .when(!inst.manifest.description.trim().is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(ui.text)
                        .child(SharedString::from(inst.manifest.description.trim().to_string())),
                )
            })
            .when_some(inst.incompatible.clone(), |d, why| d.child(banner(ui, Tone::Warning, "Off on this host", vec![why])))
            .child(actions);

        let mut page = div().flex().flex_col().gap(px(28.)).child(hero);

        // Settings, in the manifest's groups; advanced ones behind an expander.
        let settings: Vec<&Setting> = inst.manifest.settings.iter().collect();
        if !settings.is_empty() {
            let values = self.plugin_values(inst);
            let mut groups: Vec<(Option<String>, Vec<&Setting>)> = Vec::new();
            for s in settings.iter().filter(|s| !s.advanced) {
                match groups.iter_mut().find(|(g, _)| *g == s.group) {
                    Some((_, v)) => v.push(s),
                    None => groups.push((s.group.clone(), vec![s])),
                }
            }
            for (g, items) in groups {
                let title = g.unwrap_or_else(|| "Settings".into());
                page = page
                    .child(section(ui, title).children(items.into_iter().map(|s| self.setting_row(ui, inst, s, &values, !enabled, cx))));
            }
            let advanced: Vec<&&Setting> = settings.iter().filter(|s| s.advanced).collect();
            if !advanced.is_empty() {
                let open = self.plugins.show_advanced;
                let mut s = section(ui, "Advanced").child(
                    row(ui, if open { "Hide advanced settings" } else { "Show advanced settings" })
                        .desc(format!("{} more, for special setups", advanced.len()))
                        .on_click(
                            "plugin-advanced",
                            cx.listener(|v, _, _, cx| {
                                v.plugins.show_advanced = !v.plugins.show_advanced;
                                cx.notify();
                            }),
                        ),
                );
                if open {
                    s = s.children(advanced.into_iter().map(|st| self.setting_row(ui, inst, st, &values, !enabled, cx)));
                }
                page = page.child(s);
            }
            if !enabled {
                page = page.child(kit::hint(ui, "Turn the plugin on to use these settings.").px(px(4.)));
            }
        }

        // The programs it needs.
        if !inst.packages.is_empty() {
            page = page.child(section(ui, "Tools").children(inst.packages.iter().map(|p| {
                let (desc, tone, label) = match &p.state {
                    PackageState::System { path } => (format!("Installed on the system: {path}"), Tone::Success, "Installed"),
                    PackageState::Managed { version } if *version == p.version => {
                        (format!("goatty's copy, version {version}"), Tone::Success, "Installed")
                    }
                    PackageState::Managed { version } => {
                        (format!("goatty's copy, version {version}; {} is available", p.version), Tone::Accent, "Update")
                    }
                    PackageState::Missing if p.available => {
                        ("Not installed: goatty can download it into its own directory".into(), Tone::Warning, "Missing")
                    }
                    PackageState::Missing => {
                        ("Not installed, and goatty has no build for this host: use your package manager".into(), Tone::Danger, "Missing")
                    }
                };
                row(ui, p.id.clone()).icon(Icon::Package).desc(desc).control(badge(label, tone, ui))
            })));
        }

        // What it may do.
        let granted = permissions::describe(&inst.manifest);
        if !granted.is_empty() {
            page = page.child(section(ui, "Permissions").children(granted.into_iter().map(|g| {
                let icon = perm_icon(g.permission);
                row(ui, g.title.clone()).icon(icon).desc_opt((!g.details.is_empty()).then(|| g.details.join(", ")))
            })));
        }

        // Its aliases live on the Aliases page.
        if !inst.manifest.aliases.is_empty() {
            let name = inst.manifest.name.clone();
            page = page.child(
                section(ui, "Aliases").child(
                    row(ui, format!("{} aliases", inst.manifest.aliases.len()))
                        .icon(Icon::Link2)
                        .desc(inst.manifest.aliases.iter().take(6).map(|a| a.name.clone()).collect::<Vec<_>>().join("  ·  "))
                        .on_click(
                            "plugin-aliases",
                            cx.listener(move |v, _, _, cx| {
                                v.go(Page::Aliases, cx);
                                v.show_plugin_aliases(&name, cx);
                            }),
                        ),
                ),
            );
        }
        page.into_any_element()
    }

    /// One schema-driven control.
    fn setting_row(&self, ui: Ui, inst: &Installed, s: &Setting, values: &schema::Values, off: bool, cx: &Context<'_, Self>) -> AnyElement {
        let id = inst.id.clone();
        let key = s.id.clone();
        let value = values.get(&s.id).cloned().unwrap_or(Value::Null);
        let prefix = format!("ps-{id}-{key}");
        let elem_id = |suffix: &str| SharedString::from(format!("{prefix}{suffix}"));
        let this = cx.entity().downgrade();
        let set = move |v: Option<Value>| {
            let (this, id, key) = (this.clone(), id.clone(), key.clone());
            move |_: &mut gpui::Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |view, cx| view.set_plugin_value(&id, &key, v.clone(), cx));
            }
        };
        let r = row(ui, s.label.clone()).desc_opt((!s.help.is_empty()).then(|| s.help.clone())).disabled(off);
        let r = match s.kind {
            SettingKind::Bool => {
                let on = value.as_bool().unwrap_or(false);
                r.control(toggle(elem_id(""), on, ui, set(Some(Value::Bool(!on)))).aria_label(s.label.clone()))
            }
            SettingKind::Choice => {
                let selected = schema::plain_string(&value);
                let short = s.choices.len() <= 3
                    && s.choices.iter().map(|c| c.label.len()).sum::<usize>() <= 24
                    && s.choices.iter().all(|c| c.help.is_empty());
                let this = cx.entity().downgrade();
                let (pid, key) = (inst.id.clone(), s.id.clone());
                let pick = move |v: String, _: &mut gpui::Window, cx: &mut gpui::App| {
                    let _ = this.update(cx, |view, cx| view.set_plugin_value(&pid, &key, Some(Value::String(v)), cx));
                };
                if short {
                    let options: Vec<(String, &str)> = s.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
                    r.control(segmented(&elem_id(""), &options, &selected, ui, pick))
                } else {
                    let options = s
                        .choices
                        .iter()
                        .map(|c| {
                            if c.help.is_empty() {
                                opt(c.value.clone(), c.label.clone())
                            } else {
                                opt(c.value.clone(), c.label.clone()).desc(c.help.clone())
                            }
                        })
                        .collect();
                    r.control(self.select(&elem_id(""), &s.label, options, &selected, cx, pick))
                }
            }
            SettingKind::Integer => {
                let n = value.as_i64().unwrap_or(0);
                let step = s.step.unwrap_or(1);
                let clamp = |x: i64| x.clamp(s.min.unwrap_or(i64::MIN), s.max.unwrap_or(i64::MAX));
                let shown = match &s.unit {
                    Some(u) => format!("{n} {u}"),
                    None => n.to_string(),
                };
                r.control(stepper(
                    &elem_id(""),
                    shown,
                    ui,
                    set(Some(Value::from(clamp(n - step)))),
                    set(Some(Value::from(clamp(n + step)))),
                ))
            }
            SettingKind::String | SettingKind::Path | SettingKind::List => {
                match self.plugins.inputs.get(&format!("{}.{}", inst.id, s.id)) {
                    Some(input) => r.below(input.clone()),
                    None => r.control(div().text_size(px(text::SMALL)).child(SharedString::from(schema::plain_string(&value)))),
                }
            }
        };
        r.into_any_element()
    }

    // ---- The install review -------------------------------------------------

    /// Everything the plugin will do, before it does it.
    fn plan_view(&self, ui: Ui, plan: &Plan, cx: &Context<'_, Self>) -> AnyElement {
        let mono = kit::mono(cx);
        let source = if plan.builtin {
            "Comes with goatty: this installs the tool it needs into goatty's own directory".to_string()
        } else if plan.verified {
            format!("By {} · from the {} registry, signature checked", plan.publisher, plan.source)
        } else {
            format!("By {} · from {}, not from a registry", plan.publisher, plan.source)
        };
        let version = match &plan.replaces {
            Some(old) if *old != plan.version => format!("{old} → {}", plan.version),
            _ => plan.version.clone(),
        };
        let hero = card(ui)
            .p(px(20.))
            .gap(px(14.))
            .child(
                div().flex().items_center().gap(px(16.)).child(Self::plugin_avatar(ui, Category::Environment, plan.builtin, 52.)).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(plan.name.clone())),
                                )
                                .child(badge(version, Tone::Neutral, ui))
                                .when(plan.verified, |d| d.child(Icon::ShieldCheck.svg().size(px(16.)).text_color(ui.success))),
                        )
                        .child(div().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(source))),
                ),
            )
            .child(div().text_size(px(text::BODY)).child(SharedString::from(plan.summary.clone())))
            .when(!plan.description.trim().is_empty(), |d| {
                d.child(
                    div().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(plan.description.trim().to_string())),
                )
            });
        let mut page = div().flex().flex_col().gap(px(28.)).child(hero);
        if !plan.warnings.is_empty() {
            page = page.child(banner(ui, Tone::Warning, "Read this first", plan.warnings.clone()));
        }
        page = page.child(section(ui, "What it may do").children(plan.permissions.iter().map(|g| {
            let mut desc = String::new();
            if !g.details.is_empty() {
                desc.push_str(&g.details.join(", "));
                desc.push_str(". ");
            }
            if !g.reason.is_empty() {
                desc.push_str(&g.reason);
                desc.push(' ');
            }
            desc.push_str(&g.meaning);
            row(ui, g.title.clone()).icon(perm_icon(g.permission)).desc(desc)
        })));
        if !plan.downloads.is_empty() {
            page = page.child(
                section(ui, "Downloads")
                    .children(plan.downloads.iter().map(|d| {
                        let (desc, control) = match &d.system {
                            Some(p) => (format!("Already installed at {p}: goatty uses that copy"), badge("Skipped", Tone::Success, ui)),
                            None if d.current => ("goatty's copy is already installed".to_string(), badge("Skipped", Tone::Success, ui)),
                            None => (url_host(&d.url), badge(size(d.size), Tone::Neutral, ui)),
                        };
                        row(ui, format!("{} {}", d.package, d.version)).icon(Icon::Download).desc(desc).control(control)
                    }))
                    .footer("Into goatty's own directory, no sudo. Each download is checked against the checksum in the signed manifest before it's unpacked."),
            );
        }
        // Short items as chips; long ones (alias definitions) as wrapped lines.
        let chips = |items: &[String]| {
            if items.iter().any(|i| i.len() > 32) {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .rounded(px(radius::CONTROL))
                    .bg(ui.bg)
                    .border_1()
                    .border_color(ui.divider)
                    .font_family(mono.clone())
                    .text_size(px(text::CAPTION))
                    .children(items.iter().map(|i| div().child(SharedString::from(i.clone()))))
            } else {
                div().flex().flex_wrap().gap(px(6.)).children(items.iter().map(|i| {
                    div()
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .bg(ui.field)
                        .font_family(mono.clone())
                        .text_size(px(text::CAPTION))
                        .child(SharedString::from(i.clone()))
                }))
            }
        };
        let adds: Vec<(&str, &Vec<String>)> = [("Aliases", &plan.aliases), ("Environment variables", &plan.env), ("Needs", &plan.requires)]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        if !adds.is_empty() {
            page = page.child(section(ui, "Adds to your shell").children(adds.into_iter().map(|(t, v)| row(ui, t).below(chips(v)))));
        }
        if let Some((file, code)) = &plan.shell_code {
            let open = self.plugins.show_code;
            let mut s = section(ui, "Shell code").child(
                row(ui, if open { "Hide the code" } else { "Read the code" })
                    .icon(Icon::Code)
                    .desc(format!("{file} runs when every goatty shell starts · {} lines", code.lines().count()))
                    .on_click(
                        "plan-code",
                        cx.listener(|v, _, _, cx| {
                            v.plugins.show_code = !v.plugins.show_code;
                            cx.notify();
                        }),
                    ),
            );
            if open {
                s = s.child(
                    div()
                        .id("plan-code-text")
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .mx(px(16.))
                        .mb(px(16.))
                        .p(px(12.))
                        .rounded(px(radius::CONTROL))
                        .bg(ui.bg)
                        .border_1()
                        .border_color(ui.divider)
                        .font_family(mono.clone())
                        .text_size(px(text::CAPTION))
                        .children(code.lines().map(|l| {
                            div().whitespace_nowrap().child(SharedString::from(if l.is_empty() { " ".to_string() } else { l.to_string() }))
                        })),
                );
            }
            page = page.child(s);
        }
        let verb = if plan.replaces.is_some() { "Update" } else { "Install" };
        page.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .when(self.plugins.loading, |d| d.child(kit::hint(ui, "Installing…")))
                .child(button(
                    "plan-cancel",
                    "Cancel",
                    ButtonKind::Ghost,
                    ui,
                    self.act(cx, |v, cx| {
                        v.plugins.plan = None;
                        cx.notify();
                    }),
                ))
                .child(button_with(
                    "plan-install",
                    Some(Icon::Download),
                    verb,
                    ButtonKind::Primary,
                    ui,
                    self.act(cx, |v, cx| v.install_reviewed(cx)),
                )),
        )
        .into_any_element()
    }
}

fn perm_icon(p: Permission) -> Icon {
    match p {
        Permission::Binaries => Icon::Package,
        Permission::ShellCode => Icon::Code,
        Permission::Network => Icon::Globe,
        Permission::Aliases => Icon::Link2,
        Permission::Env => Icon::Sliders,
        Permission::ConfigFiles => Icon::FileText,
    }
}

/// `https://github.com/x/y/releases/…` → `github.com`.
fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    format!("From {}", rest.split('/').next().unwrap_or(rest))
}

fn changed(r: api::Reply) -> Option<api::Changed> {
    match r {
        api::Reply::Changed(c) => Some(c),
        _ => None,
    }
}
