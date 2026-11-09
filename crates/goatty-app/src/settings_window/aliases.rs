//! The settings window's Aliases page (#88): every alias goatty's zsh
//! defines on the connected host, the user's own grouped by their groups
//! and each plugin's under the plugin, with switches, a form to add and
//! edit, and a filter.
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
    AnyElement, App, Context, Entity, Focusable, FontWeight, IntoElement, ParentElement, SharedString, Styled, Subscription, Window, div,
    prelude::*, px,
};

use super::{Page, SettingsWindow};
use crate::ui::controls::{ButtonKind, button, note, notice, toggle};
use crate::ui::style::Ui;
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

/// The page's own state (the aliases themselves are host settings).
pub struct AliasesPage {
    filter: Entity<TextInput>,
    inputs: Inputs,
    /// The form is open: `Some(None)` adds, `Some(Some(name))` edits `name`.
    form: Option<Option<String>>,
    /// Why the last Save didn't go through.
    form_error: Option<String>,
    /// Folded groups ([`group_key`]).
    collapsed: HashSet<String>,
    _subs: Vec<Subscription>,
}

impl AliasesPage {
    pub fn new(cx: &mut Context<'_, SettingsWindow>) -> Self {
        let filter = cx.new(|cx| TextInput::new("Filter aliases…", "", cx));
        let inputs = Inputs {
            name: cx.new(|cx| TextInput::new("gst", "", cx)),
            command: cx.new(|cx| TextInput::new("git status", "", cx)),
            group: cx.new(|cx| TextInput::new("Git (optional)", "", cx)),
            description: cx.new(|cx| TextInput::new("What it does (optional)", "", cx)),
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
        Self { filter, inputs, form: None, form_error: None, collapsed: HashSet::new(), _subs: subs }
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

    fn close_alias_form(&mut self, cx: &mut Context<'_, Self>) {
        self.aliases.form = None;
        self.aliases.form_error = None;
        cx.notify();
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

    // ---- Rendering ----------------------------------------------------------

    pub(super) fn aliases_page(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let (header, ok) = self.host_header(ui);
        let mut page = div().flex().flex_col().gap_1().child(header);
        let Some(h) = self.host_value().filter(|_| ok) else { return page.into_any_element() };
        page = page.child(note(
            ui,
            "An alias is a short name for a longer command: type gst, run git status. Yours are listed by group, then the ones your plugins bring. One of yours with a plugin alias's name replaces it.",
        ));
        if !h.shell.managed {
            page = page.child(notice(
                ui,
                ui.warning,
                vec!["goatty's managed zsh is off on this host (Shell page), so none of these are loaded.".into()],
            ));
        }
        page =
            page.child(div().flex().items_center().gap_2().pt_2().child(div().flex_1().child(self.aliases.filter.clone())).child(button(
                "alias-new",
                "New alias",
                ButtonKind::Primary,
                ui,
                self.act_window(cx, |v, window, cx| v.open_alias_form(None, ["", "", "", ""], window, cx)),
            )));
        if self.aliases.form.is_some() {
            page = page.child(self.alias_form(ui, h, cx));
        }
        if self.plugins.loading && self.plugins.catalog.is_none() {
            page = page.child(note(ui, "Loading the plugins' aliases…"));
        }

        let groups = self.alias_table();
        let query = self.aliases.filter.read(cx).text().trim().to_lowercase();
        if h.aliases.is_empty() && query.is_empty() {
            page = page.child(self.suggestions(ui, cx));
        }
        let mut shown = 0;
        for g in &groups {
            let rows: Vec<&Row> = g.rows.iter().filter(|r| matches(&query, g, r)).collect();
            if rows.is_empty() {
                continue;
            }
            shown += rows.len();
            page = page.child(self.alias_group(ui, g, &rows, !query.is_empty(), cx));
        }
        if shown == 0 && !query.is_empty() {
            page = page.child(note(ui, "No alias matches."));
        }
        if !groups.iter().any(|g| g.plugin_id().is_some()) && self.plugins.catalog.is_some() {
            page = page.child(note(ui, "Plugins can bring aliases too (Docker: dps, dcu, …). Browse them on the Plugins page."));
        }
        page.child(note(ui, "From a shell: goattyd alias list | add NAME COMMAND | edit | disable | remove (on the host)."))
            .into_any_element()
    }

    fn mono(cx: &Context<'_, Self>) -> SharedString {
        cx.global::<crate::settings::AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into()).into()
    }

    /// Quick starts while the user has no aliases.
    fn suggestions(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let mono = Self::mono(cx);
        let mut chips = div().flex().flex_wrap().gap_2();
        for (i, &(name, command, group)) in SUGGESTIONS.iter().enumerate() {
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("alias-suggest-{i}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(ui.border)
                    .bg(ui.surface)
                    .cursor_pointer()
                    .hover(|s| s.bg(ui.surface_hover))
                    .text_sm()
                    .font_family(mono.clone())
                    .child(SharedString::from(format!("{name} → {command}")))
                    .on_click({
                        let f = self.act_window(cx, move |v, window, cx| v.open_alias_form(None, [name, command, group, ""], window, cx));
                        move |_, window, cx| f(window, cx)
                    }),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .my_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(ui.border)
            .child(div().text_sm().child("No aliases of your own yet. Start with one of these, or press New alias:"))
            .child(chips)
            .into_any_element()
    }

    fn alias_form(&self, ui: Ui, h: &HostSettings, cx: &Context<'_, Self>) -> AnyElement {
        let editing = self.aliases.form.clone().flatten();
        let i = &self.aliases.inputs;
        let name = i.name.read(cx).text().trim().to_owned();
        let command = i.command.read(cx).text().trim().to_owned();
        let title = match &editing {
            Some(n) => format!("Edit {n}"),
            None => "New alias".to_owned(),
        };
        let field = |label: &str, input: &Entity<TextInput>, width: Option<f32>| {
            let d = div().flex().flex_col().gap_1().child(div().text_xs().text_color(ui.muted).child(SharedString::from(label.to_owned())));
            match width {
                Some(w) => d.w(px(w)).child(input.clone()),
                None => d.flex_1().min_w_0().child(input.clone()),
            }
        };
        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .my_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(ui.accent.opacity(0.6))
            .bg(ui.surface)
            .child(div().font_weight(FontWeight::BOLD).child(SharedString::from(title)))
            .child(div().flex().gap_3().child(field("Name", &i.name, Some(160.))).child(field("Command", &i.command, None)))
            .child(div().flex().gap_3().child(field("Group", &i.group, Some(160.))).child(field("Description", &i.description, None)));

        let groups = aliases::user_groups(h);
        if !groups.is_empty() {
            let mut chips = div().flex().flex_wrap().items_center().gap_1().child(div().text_xs().text_color(ui.muted).child("Groups:"));
            for (n, g) in groups.into_iter().enumerate() {
                let input = i.group.clone();
                chips = chips.child(
                    div()
                        .id(SharedString::from(format!("alias-group-{n}")))
                        .px_2()
                        .rounded_md()
                        .border_1()
                        .border_color(ui.border)
                        .text_xs()
                        .cursor_pointer()
                        .hover(|s| s.bg(ui.surface_hover))
                        .child(SharedString::from(g.clone()))
                        .on_click(move |_, _, cx| input.update(cx, |input, cx| input.set_text(g.clone(), cx))),
                );
            }
            card = card.child(chips);
        }

        // What it will do, and what it replaces, while typing.
        let mut hints = Vec::new();
        if !name.is_empty() && !command.is_empty() && alias_name_problem(&name).is_none() {
            hints.push(note(ui, format!("Typing {name} will run: {command}")).font_family(Self::mono(cx)));
        }
        for g in self.alias_table().iter().filter(|g| g.plugin_id().is_some()) {
            if g.rows.iter().any(|r| r.name == name) {
                hints.push(note(ui, format!("Replaces {}'s alias {name}.", g.title())));
            }
        }
        card = card.children(hints);
        if let Some(e) = &self.aliases.form_error {
            card = card.child(notice(ui, ui.danger, vec![e.clone()]));
        }
        card.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(button("alias-cancel", "Cancel", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.close_alias_form(cx))))
                .child(button(
                    "alias-save",
                    if editing.is_some() { "Save" } else { "Add" },
                    ButtonKind::Primary,
                    ui,
                    self.act(cx, |v, cx| v.save_alias(cx)),
                )),
        )
        .child(note(ui, "Enter saves. Changes are staged like every shell setting: apply them below."))
        .into_any_element()
    }

    fn alias_group(&self, ui: Ui, g: &Group, rows: &[&Row], filtering: bool, cx: &Context<'_, Self>) -> AnyElement {
        let key = group_key(g);
        let folded = !filtering && self.aliases.collapsed.contains(&key);
        let any_on = g.rows.iter().any(|r| r.enabled);
        let active = g.rows.iter().filter(|r| r.active).count();
        let plugin_off = matches!(g.source, Source::Plugin { enabled: false, .. });
        let count = if active == g.rows.len() { format!("{active}") } else { format!("{active} of {}", g.rows.len()) };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .pt_4()
            .pb_1()
            .border_b_1()
            .border_color(ui.border)
            .child(
                div()
                    .id(SharedString::from(format!("alias-fold-{key}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .cursor_pointer()
                    .child(div().w(px(12.)).text_color(ui.muted).child(if folded { "▸" } else { "▾" }))
                    .child(div().font_weight(FontWeight::BOLD).child(SharedString::from(g.title().to_owned())))
                    .when(g.plugin_id().is_some(), |d| d.child(badge("plugin", ui.muted)))
                    .child(div().text_xs().text_color(ui.muted).child(SharedString::from(count)))
                    .on_click({
                        let key = key.clone();
                        let f = self.act(cx, move |v, cx| {
                            if !v.aliases.collapsed.remove(&key) {
                                v.aliases.collapsed.insert(key.clone());
                            }
                            cx.notify();
                        });
                        move |_, window, cx| f(window, cx)
                    }),
            )
            .when(plugin_off, |d| {
                d.child(button(
                    SharedString::from(format!("alias-plugin-{key}")),
                    "Plugin is disabled…",
                    ButtonKind::Normal,
                    ui,
                    self.act(cx, |v, cx| {
                        v.page = Page::Plugins;
                        cx.notify();
                    }),
                ))
            })
            .child({
                let group = g.clone();
                toggle(
                    SharedString::from(format!("alias-all-{key}")),
                    any_on,
                    ui,
                    self.act(cx, move |v, cx| v.set_group_enabled(&group, !any_on, cx)),
                )
            });
        let mut d = div().flex().flex_col().child(header);
        if !folded {
            for r in rows {
                d = d.child(self.alias_row(ui, g, r, cx));
            }
        }
        d.into_any_element()
    }

    fn alias_row(&self, ui: Ui, g: &Group, r: &Row, cx: &Context<'_, Self>) -> AnyElement {
        let mono = Self::mono(cx);
        let target = match g.plugin_id() {
            Some(id) => Target::Plugin { id: id.to_owned(), alias: r.name.clone() },
            None => Target::User(r.name.clone()),
        };
        let id = |what: &str| SharedString::from(format!("alias-{what}-{}-{}", group_key(g), r.name));
        let on = r.enabled;
        let mut text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(div().text_sm().font_family(mono.clone()).truncate().child(SharedString::from(r.expansion.clone())))
            .when(!r.description.is_empty(), |d| {
                d.child(div().text_xs().text_color(ui.muted).child(SharedString::from(r.description.clone())))
            });
        if let Some(n) = &r.note {
            let color = if r.active {
                ui.accent
            } else if r.enabled {
                ui.warning
            } else {
                ui.muted
            };
            text = text.child(div().text_xs().text_color(color).child(SharedString::from(n.clone())));
        }
        let mut row = div()
            .flex()
            .items_center()
            .gap_3()
            .py_2()
            .border_b_1()
            .border_color(ui.border.opacity(0.5))
            .child({
                let target = target.clone();
                toggle(id("on"), on, ui, self.act(cx, move |v, cx| v.set_alias_enabled(target.clone(), !on, cx)))
            })
            .child(
                div()
                    .w(px(110.))
                    .flex_shrink_0()
                    .font_family(mono)
                    .font_weight(FontWeight::BOLD)
                    .truncate()
                    .when(!r.active, |d| d.text_color(ui.muted))
                    .child(SharedString::from(r.name.clone())),
            )
            .child(text.when(!r.active, |d| d.opacity(0.75)));
        row = match &target {
            Target::User(name) => {
                let (edit_name, delete_name) = (name.clone(), name.clone());
                row.child(button(
                    id("edit"),
                    "Edit",
                    ButtonKind::Normal,
                    ui,
                    self.act_window(cx, move |v, window, cx| {
                        let Some(a) = v.host_value().and_then(|h| h.aliases.get(&edit_name)).cloned() else { return };
                        v.open_alias_form(Some(edit_name.clone()), [&edit_name, &a.command, &a.group, &a.description], window, cx);
                    }),
                ))
                .child(button(
                    id("delete"),
                    "Delete",
                    ButtonKind::Danger,
                    ui,
                    self.act(cx, move |v, cx| v.delete_alias(delete_name.clone(), cx)),
                ))
            }
            Target::Plugin { .. } => {
                let (name, expansion, plugin) = (r.name.clone(), r.expansion.clone(), g.title().to_owned());
                let mine = self.host_value().is_some_and(|h| h.aliases.contains_key(&name));
                row.when(!mine, |row| {
                    row.child(button(
                        id("customize"),
                        "Customize…",
                        ButtonKind::Normal,
                        ui,
                        self.act_window(cx, move |v, window, cx| {
                            v.open_alias_form(None, [&name, &expansion, &plugin, ""], window, cx);
                        }),
                    ))
                })
                .when(
                    r.condition.is_some() && !r.active && r.enabled && r.note.as_deref().is_some_and(|n| n.starts_with("only while")),
                    |row| {
                        row.child(button(
                            id("settings"),
                            "Plugin settings…",
                            ButtonKind::Normal,
                            ui,
                            self.act(cx, |v, cx| {
                                v.page = Page::Plugins;
                                cx.notify();
                            }),
                        ))
                    },
                )
            }
        };
        row.into_any_element()
    }
}

fn badge(text: &str, color: gpui::Hsla) -> AnyElement {
    div()
        .px_2()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(0.6))
        .text_xs()
        .text_color(color)
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
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
}
