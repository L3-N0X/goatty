//! The settings window's Aliases page (#88): every alias goatty's zsh
//! defines on the connected host: the user's own first, grouped by their
//! groups, then each plugin's (folded until opened). Switches, a dialog to
//! add and edit, a search and filters.
//!
//! Aliases are host settings (`aliases` and `plugins.<id>.disabled_aliases`
//! in `host.json`): edits go into the same draft as the Shell page and are
//! applied with Preview/Apply. What the list shows (active, replaced, the
//! plugin's condition) comes from `goatty_shell::aliases::table`, the same
//! table `goattyd alias list` prints.

use std::collections::HashSet;

use goatty_config::{HostSettings, UserAlias, alias_name_problem};
use goatty_shell::aliases::{self, Group, PluginAliases, Row, Source};
use gpui::{
    AnyElement, App, Context, Entity, Focusable, FontWeight, IntoElement, MouseButton, ParentElement, SharedString, Styled, Subscription,
    Window, div, prelude::*, px,
};

use super::kit::{self, card, chip, divider, field, modal, search_field, section};
use super::{Page, SettingsWindow};
use crate::ui::controls::{ButtonKind, Tone, badge, banner, button, button_with, icon_button, toggle};
use crate::ui::focus::{self, Ring, Scroller};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};
use crate::ui::text_input::{TextInput, TextInputEvent};

/// Offered while the user has no aliases of their own: (name, command, group).
const SUGGESTIONS: &[(&str, &str, &str)] = &[
    ("..", "cd ..", "Navigation"),
    ("...", "cd ../..", "Navigation"),
    ("gst", "git status", "Git"),
    ("gd", "git diff", "Git"),
    ("glo", "git log --oneline --graph", "Git"),
];

/// The add/edit form's fields.
struct Inputs {
    name: Entity<TextInput>,
    command: Entity<TextInput>,
    group: Entity<TextInput>,
    description: Entity<TextInput>,
}

/// Which aliases the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Mine,
    Plugins,
}

/// The page's own state (the aliases themselves are host settings).
pub struct AliasesPage {
    filter: Entity<TextInput>,
    inputs: Inputs,
    /// The form is open: `Some(None)` adds, `Some(Some(name))` edits `name`.
    form: Option<Option<String>>,
    /// Why the last Save didn't go through.
    form_error: Option<String>,
    scope: Scope,
    /// Groups the user folded or unfolded ([`group_key`]): user groups
    /// start open, plugin groups folded.
    toggled: HashSet<String>,
    /// The form's frame: Tab stays inside it.
    pub(super) modal_focus: gpui::FocusHandle,
    _subs: Vec<Subscription>,
}

impl AliasesPage {
    pub fn new(cx: &mut Context<'_, SettingsWindow>) -> Self {
        let filter = cx.new(|cx| TextInput::new("Search aliases and commands", "", cx).bare());
        let inputs = Inputs {
            name: cx.new(|cx| TextInput::new("gst", "", cx)),
            command: cx.new(|cx| TextInput::new("git status", "", cx)),
            group: cx.new(|cx| TextInput::new("Optional", "", cx)),
            description: cx.new(|cx| TextInput::new("Optional", "", cx)),
        };
        let mut subs = vec![cx.subscribe(&filter, |_, _, _: &TextInputEvent, cx| cx.notify())];
        for input in [&inputs.name, &inputs.command, &inputs.group, &inputs.description] {
            subs.push(cx.subscribe(input, |view, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Changed(_) => {
                    view.aliases.form_error = None;
                    cx.notify();
                }
                TextInputEvent::Submitted(_) => view.save_alias(cx),
            }));
        }
        Self {
            filter,
            inputs,
            form: None,
            form_error: None,
            scope: Scope::All,
            toggled: HashSet::new(),
            modal_focus: cx.focus_handle(),
            _subs: subs,
        }
    }
}

/// What a row's switch and buttons act on.
#[derive(Debug, Clone)]
enum Target {
    User(String),
    Plugin { id: String, alias: String },
}

fn group_key(g: &Group) -> String {
    match &g.source {
        Source::User { group } => format!("user:{}", group.to_lowercase()),
        Source::Plugin { id, .. } => format!("plugin:{id}"),
    }
}

/// `query` (lowercase) matches the row or its group's title.
fn matches(query: &str, group: &Group, r: &Row) -> bool {
    query.is_empty()
        || r.name.to_lowercase().contains(query)
        || r.expansion.to_lowercase().contains(query)
        || r.description.to_lowercase().contains(query)
        || group.title().to_lowercase().contains(query)
}

/// Check the form before it goes into the draft: `Err` blocks saving.
/// `editing` is the alias being edited (its own name isn't "taken").
pub fn check_form(settings: &HostSettings, editing: Option<&str>, name: &str, command: &str) -> Result<(), String> {
    if let Some(why) = alias_name_problem(name) {
        return Err(format!("Name: {why}."));
    }
    if command.trim().is_empty() {
        return Err("Command: what should typing the name run?".into());
    }
    if editing != Some(name) && settings.aliases.contains_key(name) {
        return Err(format!("You already have an alias {name}: edit that one, or pick another name."));
    }
    Ok(())
}

/// Existing groups that complete what's typed (all of them when nothing is).
fn group_suggestions(groups: &[String], typed: &str) -> Vec<String> {
    let t = typed.trim().to_lowercase();
    groups.iter().filter(|g| g.to_lowercase() != t && (t.is_empty() || g.to_lowercase().contains(&t))).cloned().collect()
}

impl SettingsWindow {
    /// The aliases table for the draft (else the host's settings), with
    /// the installed plugins from the catalog.
    fn alias_table(&self) -> Vec<Group> {
        let Some(h) = self.host_value() else { return Vec::new() };
        let installed = self.plugins.catalog.as_ref().map(|c| c.installed.as_slice()).unwrap_or_default();
        let refs: Vec<PluginAliases<'_>> = installed
            .iter()
            .map(|i| PluginAliases { manifest: &i.manifest, enabled: h.plugins.get(&i.id).is_none_or(|p| p.enabled) })
            .collect();
        aliases::table(h, &refs)
    }

    fn set_alias_enabled(&mut self, target: Target, on: bool, cx: &mut Context<'_, Self>) {
        self.edit_host(cx, move |h| match target {
            Target::User(name) => {
                if let Some(a) = h.aliases.get_mut(&name) {
                    a.enabled = on;
                }
            }
            Target::Plugin { id, alias } => {
                let off = &mut h.plugins.entry(id).or_default().disabled_aliases;
                if on {
                    off.remove(&alias);
                } else {
                    off.insert(alias);
                }
            }
        });
    }

    /// Switch a whole group: on when any of it is off, else off.
    fn set_group_enabled(&mut self, group: &Group, on: bool, cx: &mut Context<'_, Self>) {
        let names: Vec<String> = group.rows.iter().map(|r| r.name.clone()).collect();
        let plugin = group.plugin_id().map(str::to_owned);
        self.edit_host(cx, move |h| match plugin {
            Some(id) => {
                let off = &mut h.plugins.entry(id).or_default().disabled_aliases;
                for n in names {
                    if on {
                        off.remove(&n);
                    } else {
                        off.insert(n);
                    }
                }
            }
            None => {
                for n in names {
                    if let Some(a) = h.aliases.get_mut(&n) {
                        a.enabled = on;
                    }
                }
            }
        });
    }

    /// Open the form, filled with `fields` (name, command, group,
    /// description); `editing`: the alias it changes.
    fn open_alias_form(&mut self, editing: Option<String>, fields: [&str; 4], window: &mut Window, cx: &mut Context<'_, Self>) {
        let i = &self.aliases.inputs;
        for (input, text) in [&i.name, &i.command, &i.group, &i.description].into_iter().zip(fields) {
            input.update(cx, |input, cx| input.set_text(text.to_owned(), cx));
        }
        // Editing starts at the command; a new alias at its name.
        let first = if editing.is_some() || !fields[0].is_empty() { &i.command } else { &i.name };
        window.focus(&first.focus_handle(cx), cx);
        self.aliases.form = Some(editing);
        self.aliases.form_error = None;
        cx.notify();
    }

    pub(super) fn close_alias_form(&mut self, cx: &mut Context<'_, Self>) {
        self.aliases.form = None;
        self.aliases.form_error = None;
        cx.notify();
    }

    /// Show only `plugin`'s aliases, unfolded (from its page).
    pub(super) fn show_plugin_aliases(&mut self, plugin: &str, cx: &mut Context<'_, Self>) {
        self.aliases.scope = Scope::Plugins;
        self.aliases.filter.update(cx, |i, cx| i.set_text(plugin.to_owned(), cx));
        cx.notify();
    }

    pub(super) fn aliases_modal_open(&self) -> bool {
        self.aliases.form.is_some()
    }

    /// The form's values into the draft (checked first).
    fn save_alias(&mut self, cx: &mut Context<'_, Self>) {
        let Some(editing) = self.aliases.form.clone() else { return };
        let Some(h) = self.host_value() else { return };
        let i = &self.aliases.inputs;
        let text = |e: &Entity<TextInput>| e.read(cx).text().trim().to_owned();
        let (name, command, group, description) = (text(&i.name), text(&i.command), text(&i.group), text(&i.description));
        if let Err(e) = check_form(h, editing.as_deref(), &name, &command) {
            self.aliases.form_error = Some(e);
            cx.notify();
            return;
        }
        let enabled = editing.as_ref().and_then(|e| h.aliases.get(e)).is_none_or(|a| a.enabled);
        self.edit_host(cx, move |h| {
            if let Some(old) = &editing {
                h.aliases.remove(old);
            }
            h.aliases.insert(name, UserAlias { command, group, description, enabled });
        });
        self.close_alias_form(cx);
    }

    fn delete_alias(&mut self, name: String, cx: &mut Context<'_, Self>) {
        if self.aliases.form.as_ref().is_some_and(|e| e.as_deref() == Some(name.as_str())) {
            self.close_alias_form(cx);
        }
        self.edit_host(cx, move |h| {
            h.aliases.remove(&name);
        });
    }

    /// A callback with the window, for the ones that move focus.
    fn act_window(
        &self,
        cx: &Context<'_, Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<'_, Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |window, cx| {
            let _ = this.update(cx, |view, cx| f(view, window, cx));
        }
    }

    fn group_open(&self, g: &Group) -> bool {
        let toggled = self.aliases.toggled.contains(&group_key(g));
        // User groups start open, plugin groups folded.
        if g.plugin_id().is_some() { toggled } else { !toggled }
    }

    // ---- Rendering ----------------------------------------------------------

    pub(super) fn aliases_page(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        let (notes, h) = self.host_status(ui);
        let mut page = div().flex().flex_col().gap(px(20.)).children(notes);
        let Some(h) = h else { return page.into_any_element() };
        if !h.shell.managed {
            page = page.child(banner(
                ui,
                Tone::Warning,
                "None of these are loaded",
                vec!["goatty's managed zsh is off on this host. Turn it on in Shell.".into()],
            ));
        }
        let groups = self.alias_table();
        let query = self.aliases.filter.read(cx).text().trim().to_lowercase();
        let (mine, theirs): (Vec<&Group>, Vec<&Group>) = groups.iter().partition(|g| g.plugin_id().is_none());
        let count = |gs: &[&Group]| gs.iter().map(|g| g.rows.len()).sum::<usize>();
        let (mine_n, theirs_n) = (count(&mine), count(&theirs));
        let scope = self.aliases.scope;
        let set_scope = |s: Scope| {
            self.act(cx, move |v, cx| {
                v.aliases.scope = s;
                cx.notify();
            })
        };
        page = page.child(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().child(search_field(ui, "alias-search", &self.aliases.filter, window, cx)))
                        .child(
                            button_with(
                                "alias-new",
                                Some(Icon::Plus),
                                "New alias",
                                ButtonKind::Primary,
                                ui,
                                self.act_window(cx, |v, window, cx| v.open_alias_form(None, ["", "", "", ""], window, cx)),
                            )
                            .h(px(38.)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(chip(ui, "alias-all", "All", Some(mine_n + theirs_n), scope == Scope::All, set_scope(Scope::All)))
                        .child(chip(ui, "alias-mine", "Mine", Some(mine_n), scope == Scope::Mine, set_scope(Scope::Mine)))
                        .child(chip(
                            ui,
                            "alias-plugins",
                            "From plugins",
                            Some(theirs_n),
                            scope == Scope::Plugins,
                            set_scope(Scope::Plugins),
                        )),
                ),
        );

        let mut shown = 0;
        if scope != Scope::Plugins {
            let mut s = section(ui, "My aliases").plain();
            if mine.is_empty() && query.is_empty() {
                s = s.child(self.suggestions(ui, cx));
            }
            for g in &mine {
                let rows: Vec<&Row> = g.rows.iter().filter(|r| matches(&query, g, r)).collect();
                if !rows.is_empty() {
                    shown += rows.len();
                    s = s.child(self.alias_group(ui, g, &rows, !query.is_empty(), cx));
                }
            }
            if !mine.is_empty() || query.is_empty() {
                page = page.child(s);
            }
        }
        if scope != Scope::Mine {
            let mut s =
                section(ui, "From plugins").desc("Switch off the ones you don't want; one of yours with the same name replaces it").plain();
            if self.plugins.loading && self.plugins.catalog.is_none() {
                s = s.child(kit::hint(ui, "Loading the plugins' aliases…").px(px(4.)));
            }
            let mut any = false;
            for g in &theirs {
                let rows: Vec<&Row> = g.rows.iter().filter(|r| matches(&query, g, r)).collect();
                if !rows.is_empty() {
                    any = true;
                    shown += rows.len();
                    s = s.child(self.alias_group(ui, g, &rows, !query.is_empty(), cx));
                }
            }
            if theirs.is_empty() && self.plugins.catalog.is_some() {
                s = s.child(
                    card(ui).child(
                        kit::row(ui, "Plugins can bring aliases too")
                            .icon(Icon::Puzzle)
                            .desc("Docker brings dps, dcu and more. Browse the plugins")
                            .on_click("alias-browse", self.listener_go(cx, Page::Plugins)),
                    ),
                );
                any = true;
            }
            if any || query.is_empty() {
                page = page.child(s);
            }
        }
        if shown == 0 && !query.is_empty() {
            page = page.child(card(ui).child(kit::empty(
                ui,
                Icon::Search,
                "No alias matches",
                "Search looks at names, commands, descriptions and groups.",
            )));
        }
        page.child(kit::hint(ui, "From a shell on the host: goattyd alias list, add, edit, disable, remove.").px(px(4.))).into_any_element()
    }

    /// Quick starts while the user has no aliases.
    fn suggestions(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let mono = kit::mono(cx);
        let mut chips = div().flex().flex_wrap().justify_center().gap(px(8.));
        for (i, &(name, command, group)) in SUGGESTIONS.iter().enumerate() {
            let eid: gpui::ElementId = SharedString::from(format!("alias-suggest-{i}")).into();
            let chip = div()
                .id(eid.clone())
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("Add alias {name}: {command}")))
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(30.))
                .px(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(ui.border)
                .cursor_pointer()
                .hover(|s| s.bg(ui.field))
                .text_size(px(text::SMALL))
                .font_family(mono.clone())
                .child(div().font_weight(FontWeight::BOLD).child(name))
                .child(div().text_color(ui.faint).child("→"))
                .child(div().text_color(ui.muted).child(command))
                .on_click({
                    let f = self.act_window(cx, move |v, window, cx| v.open_alias_form(None, [name, command, group, ""], window, cx));
                    move |_, window, cx| f(window, cx)
                });
            chips = chips.child(focus::tab(chip, &eid, ui, Ring::Outer, Scroller::Page));
        }
        card(ui)
            .pb(px(20.))
            .child(kit::empty(
                ui,
                Icon::Link2,
                "No aliases of your own yet",
                "An alias is a short name for a longer command. Start with one of these:",
            ))
            .child(chips.px(px(20.)))
            .into_any_element()
    }

    fn alias_group(&self, ui: Ui, g: &Group, rows: &[&Row], filtering: bool, cx: &Context<'_, Self>) -> AnyElement {
        let key = group_key(g);
        let open = filtering || self.group_open(g);
        let any_on = g.rows.iter().any(|r| r.enabled);
        let active = g.rows.iter().filter(|r| r.active).count();
        let plugin = g.plugin_id().is_some();
        let plugin_off = matches!(g.source, Source::Plugin { enabled: false, .. });
        let count = if active == g.rows.len() { format!("{active}") } else { format!("{active} of {}", g.rows.len()) };
        let title = if g.title().is_empty() { "Ungrouped".to_string() } else { g.title().to_owned() };
        let header_id: gpui::ElementId = SharedString::from(format!("alias-fold-{key}")).into();
        let header = div()
            .id(header_id.clone())
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(title.clone()))
            .aria_expanded(open)
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(52.))
            .px(px(16.))
            .cursor_pointer()
            .hover(|s| s.bg(ui.surface_hover))
            .rounded_t(px(radius::CARD))
            .when(!open, |d| d.rounded_b(px(radius::CARD)))
            .child((if open { Icon::ChevronDown } else { Icon::ChevronRight }).svg().size(px(15.)).text_color(ui.muted))
            .when(plugin, |d| d.child(Icon::Puzzle.svg().size(px(15.)).text_color(ui.muted)))
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(title)))
            .child(div().text_size(px(text::SMALL)).text_color(ui.faint).child(SharedString::from(count)))
            .child(div().flex_1())
            .when(plugin_off, |d| d.child(badge("Plugin off", Tone::Warning, ui)))
            .child({
                let group = g.clone();
                toggle(
                    SharedString::from(format!("alias-all-{key}")),
                    any_on,
                    ui,
                    self.act(cx, move |v, cx| v.set_group_enabled(&group, !any_on, cx)),
                )
            })
            .on_click({
                let key = key.clone();
                cx.listener(move |v, _, _, cx| {
                    if !v.aliases.toggled.remove(&key) {
                        v.aliases.toggled.insert(key.clone());
                    }
                    cx.notify();
                })
            });
        let header = focus::tab(header, &header_id, ui, Ring::Inner, Scroller::Page);
        let mut c = card(ui).child(header);
        if open {
            c = c.child(div().h(px(1.)).bg(ui.divider));
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    c = c.child(divider(ui));
                }
                c = c.child(self.alias_row(ui, g, r, cx));
            }
        }
        c.into_any_element()
    }

    fn alias_row(&self, ui: Ui, g: &Group, r: &Row, cx: &Context<'_, Self>) -> AnyElement {
        let mono = kit::mono(cx);
        let target = match g.plugin_id() {
            Some(id) => Target::Plugin { id: id.to_owned(), alias: r.name.clone() },
            None => Target::User(r.name.clone()),
        };
        let gid = SharedString::from(format!("alias-row-{}-{}", group_key(g), r.name));
        let id = |what: &str| SharedString::from(format!("alias-{what}-{}-{}", group_key(g), r.name));
        let on = r.enabled;
        let mut text_col = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .text_size(px(text::LABEL))
                    .font_family(mono.clone())
                    .text_color(ui.text)
                    .truncate()
                    .child(SharedString::from(r.expansion.clone())),
            )
            .when(!r.description.is_empty(), |d| {
                d.child(div().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(r.description.clone())))
            });
        if let Some(n) = &r.note {
            let color = if r.active {
                ui.accent_text
            } else if r.enabled {
                ui.warning
            } else {
                ui.faint
            };
            text_col = text_col.child(div().text_size(px(text::SMALL)).text_color(color).child(SharedString::from(n.clone())));
        }
        let actions = match &target {
            Target::User(name) => {
                let (edit_name, delete_name) = (name.clone(), name.clone());
                div()
                    .flex()
                    .gap(px(2.))
                    .invisible()
                    .group_hover(gid.clone(), |s| s.visible())
                    .child(icon_button(
                        id("edit"),
                        Icon::Pencil,
                        "Edit",
                        ui,
                        self.act_window(cx, move |v, window, cx| {
                            let Some(a) = v.host_value().and_then(|h| h.aliases.get(&edit_name)).cloned() else { return };
                            v.open_alias_form(Some(edit_name.clone()), [&edit_name, &a.command, &a.group, &a.description], window, cx);
                        }),
                    ))
                    .child(icon_button(
                        id("delete"),
                        Icon::Trash,
                        "Delete",
                        ui,
                        self.act(cx, move |v, cx| v.delete_alias(delete_name.clone(), cx)),
                    ))
            }
            Target::Plugin { .. } => {
                let (name, expansion, plugin) = (r.name.clone(), r.expansion.clone(), g.title().to_owned());
                let mine = self.host_value().is_some_and(|h| h.aliases.contains_key(&name));
                let settings =
                    r.condition.is_some() && !r.active && r.enabled && r.note.as_deref().is_some_and(|n| n.starts_with("only while"));
                div()
                    .flex()
                    .gap(px(2.))
                    .invisible()
                    .group_hover(gid.clone(), |s| s.visible())
                    .when(settings, |d| {
                        let pid = g.plugin_id().unwrap_or_default().to_owned();
                        d.child(icon_button(
                            id("settings"),
                            Icon::Sliders,
                            "Plugin settings",
                            ui,
                            self.act(cx, move |v, cx| {
                                v.go(Page::Plugins, cx);
                                v.plugins.detail = Some(pid.clone());
                            }),
                        ))
                    })
                    .when(!mine, |d| {
                        d.child(icon_button(
                            id("customize"),
                            Icon::Copy,
                            "Make my own version",
                            ui,
                            self.act_window(cx, move |v, window, cx| v.open_alias_form(None, [&name, &expansion, &plugin, ""], window, cx)),
                        ))
                    })
            }
        };
        div()
            .id(gid.clone())
            .group(gid.clone())
            .flex()
            .items_center()
            .gap(px(14.))
            .min_h(px(54.))
            .px(px(16.))
            .py(px(8.))
            .hover(|s| s.bg(ui.surface_hover))
            .child({
                let target = target.clone();
                toggle(id("on"), on, ui, self.act(cx, move |v, cx| v.set_alias_enabled(target.clone(), !on, cx)))
            })
            .child(
                div()
                    .w(px(112.))
                    .flex_shrink_0()
                    .font_family(mono)
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::BOLD)
                    .truncate()
                    .text_color(if r.active { ui.text } else { ui.faint })
                    .child(SharedString::from(r.name.clone())),
            )
            .child(text_col.when(!r.active, |d| d.opacity(0.7)))
            .child(actions)
            .into_any_element()
    }

    /// The add/edit dialog, while it's open.
    pub(super) fn alias_modal(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let editing = self.aliases.form.clone()?;
        let h = self.host_value()?;
        let i = &self.aliases.inputs;
        let mono = kit::mono(cx);
        let name = i.name.read(cx).text().trim().to_owned();
        let command = i.command.read(cx).text().trim().to_owned();
        let title = match &editing {
            Some(n) => format!("Edit {n}"),
            None => "New alias".to_owned(),
        };

        // The group field completes the user's existing groups.
        let groups = aliases::user_groups(h);
        let typed = i.group.read(cx).text().to_owned();
        let suggestions = if i.group.read(cx).is_focused(window) { group_suggestions(&groups, &typed) } else { Vec::new() };
        let group_input = div().relative().child(i.group.clone()).when(!suggestions.is_empty(), |d| {
            let mut list = kit::menu(ui, "alias-group-menu").min_w(px(200.)).max_h(px(200.));
            for (n, g) in suggestions.into_iter().enumerate() {
                let input = i.group.clone();
                let label = g.clone();
                list = list.child(
                    kit::menu_item_mouse(ui, SharedString::from(format!("alias-group-{n}")), label.into(), None, false).on_mouse_down(
                        MouseButton::Left,
                        move |_, window, cx| {
                            input.update(cx, |input, cx| input.set_text(g.clone(), cx));
                            window.focus(&input.focus_handle(cx), cx);
                        },
                    ),
                );
            }
            d.child(div().absolute().top(px(40.)).left_0().child(gpui::deferred(gpui::anchored().child(list)).with_priority(3)))
        });

        // What it will do, and what it replaces, while typing.
        let preview = (!name.is_empty() || !command.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(12.))
                .rounded(px(radius::CONTROL))
                .bg(ui.bg)
                .border_1()
                .border_color(ui.divider)
                .font_family(mono.clone())
                .text_size(px(text::LABEL))
                .child(div().flex().gap(px(8.)).child(div().text_color(ui.success).child("❯")).child(
                    div().font_weight(FontWeight::BOLD).child(SharedString::from(if name.is_empty() {
                        "…".into()
                    } else {
                        name.clone()
                    })),
                ))
                .child(div().flex().gap(px(8.)).text_color(ui.muted).child(div().child("runs")).child(
                    div().text_color(ui.text).child(SharedString::from(if command.is_empty() { "…".into() } else { command.clone() })),
                ))
        });
        let replaces: Vec<String> = self
            .alias_table()
            .iter()
            .filter(|g| g.plugin_id().is_some() && g.rows.iter().any(|r| r.name == name))
            .map(|g| g.title().to_owned())
            .collect();

        let body = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(field(ui, "Name", div().font_family(mono.clone()).child(i.name.clone())).w(px(150.)))
                    .child(field(ui, "Command", div().font_family(mono.clone()).child(i.command.clone())).flex_1()),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(field(ui, "Group", group_input).w(px(150.)))
                    .child(field(ui, "Description", i.description.clone()).flex_1()),
            )
            .children(preview)
            .children(replaces.into_iter().map(|p| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(text::SMALL))
                    .text_color(ui.muted)
                    .child(Icon::Info.svg().size(px(14.)).text_color(ui.muted))
                    .child(SharedString::from(format!("Replaces the {p} plugin's {name}")))
            }))
            .children(self.aliases.form_error.clone().map(|e| banner(ui, Tone::Danger, e, Vec::new())));
        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(kit::hint(ui, "Applied with the other shell changes").mr(px(8.)))
            .child(button("alias-cancel", "Cancel", ButtonKind::Ghost, ui, self.act(cx, |v, cx| v.close_alias_form(cx))))
            .child(button(
                "alias-save",
                if editing.is_some() { "Save" } else { "Add alias" },
                ButtonKind::Primary,
                ui,
                self.act(cx, |v, cx| v.save_alias(cx)),
            ));
        Some(
            modal(ui, "alias-modal", title, body, footer)
                .track_focus(&self.aliases.modal_focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|v, _, _, cx| {
                        // A click on the scrim (not the card, which occludes) closes.
                        v.close_alias_form(cx)
                    }),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_checks() {
        let mut h = HostSettings::default();
        h.aliases.insert("gs".into(), UserAlias::new("git status"));
        assert_eq!(check_form(&h, None, "gd", "git diff"), Ok(()));
        assert!(check_form(&h, None, "g d", "x").unwrap_err().starts_with("Name:"));
        assert!(check_form(&h, None, "gd", "  ").unwrap_err().starts_with("Command:"));
        assert!(check_form(&h, None, "gs", "x").unwrap_err().contains("already have"));
        assert_eq!(check_form(&h, Some("gs"), "gs", "git status -s"), Ok(()), "editing keeps its own name");
        assert!(check_form(&h, Some("gd"), "gs", "x").is_err(), "renaming onto another alias");
    }

    #[test]
    fn filtering() {
        let g = Group { source: Source::User { group: "Git".into() }, rows: Vec::new() };
        let r = Row {
            name: "gs".into(),
            expansion: "git status".into(),
            description: "Short".into(),
            enabled: true,
            active: true,
            note: None,
            condition: None,
        };
        for q in ["", "gs", "status", "short", "git"] {
            assert!(matches(q, &g, &r), "{q}");
        }
        assert!(!matches("docker", &g, &r));
    }

    #[test]
    fn groups_complete_what_is_typed() {
        let groups = vec!["Git".to_string(), "Docker".to_string(), "Navigation".to_string()];
        assert_eq!(group_suggestions(&groups, ""), groups);
        assert_eq!(group_suggestions(&groups, "g"), ["Git", "Navigation"]);
        assert!(group_suggestions(&groups, "git").is_empty(), "an exact match needs no suggestion");
    }
}
