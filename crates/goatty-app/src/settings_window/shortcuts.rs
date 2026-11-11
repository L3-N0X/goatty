//! The Shortcuts page: every bindable action in a table, grouped or
//! sorted, searchable by name or by keys (typed or pressed), with filters
//! for changed and unbound actions.

use std::collections::{BTreeMap, HashSet};

use goatty_config::keys::{self, ACTIONS, ActionInfo, GROUPS};
use gpui::{
    AnyElement, Context, Entity, FontWeight, IntoElement, KeyDownEvent, ParentElement, SharedString, Styled, Subscription, Window, div,
    prelude::*, px,
};

use super::SettingsWindow;
use super::kit::{self, card, chip, divider, search_field};
use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, Tone, badge, banner, button_with, icon_button, icon_button_tinted, keycaps, pretty_keys, tooltip};
use crate::ui::focus::{self, Ring, Scroller};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, text};
use crate::ui::text_input::{TextInput, TextInputEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Changed,
    Unbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// By group, in [`GROUPS`] order (collapsible).
    Group,
    Name,
    Keys,
}

pub struct ShortcutsPage {
    query: Entity<TextInput>,
    filter: Filter,
    sort: Sort,
    collapsed: HashSet<&'static str>,
    /// The next keystroke becomes the search (instead of binding anything).
    pub(super) listening: bool,
    /// A pressed keystroke to search for (canonical).
    pressed: Option<String>,
    /// "Reset all" was clicked once; the second click resets.
    confirm_reset: bool,
    _subs: Vec<Subscription>,
}

impl ShortcutsPage {
    pub fn new(cx: &mut Context<'_, SettingsWindow>) -> Self {
        let query = cx.new(|cx| TextInput::new("Search by name or keys, like ctrl+t", "", cx).bare());
        let subs = vec![cx.subscribe(&query, |v, _, _: &TextInputEvent, cx| {
            v.shortcuts.pressed = None;
            cx.notify();
        })];
        Self {
            query,
            filter: Filter::All,
            sort: Sort::Group,
            collapsed: HashSet::new(),
            listening: false,
            pressed: None,
            confirm_reset: false,
            _subs: subs,
        }
    }
}

/// One action as the table shows it.
struct Line {
    action: &'static ActionInfo,
    keys: Vec<String>,
    changed: bool,
}

/// `query` typed as keys: `Ctrl+Shift+T`, `ctrl t` → `ctrl-shift-t`, `ctrl-t`.
fn as_keys(query: &str) -> String {
    query.trim().to_lowercase().replace(['+', ' '], "-")
}

/// Whether `line` matches the search: every word in the name, or the keys.
fn matches(query: &str, line: &Line) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    let label = format!("{} {} {}", line.action.label, line.action.name.replace('_', " "), line.action.group).to_lowercase();
    if q.split_whitespace().all(|w| label.contains(w)) {
        return true;
    }
    let k = as_keys(&q);
    line.keys.iter().any(|key| key.contains(&k) || pretty_keys(key).to_lowercase().contains(&q))
}

fn lines(overrides: &BTreeMap<String, Option<String>>) -> Vec<Line> {
    ACTIONS
        .iter()
        .map(|a| Line {
            action: a,
            keys: keys::keys_for(a.name, overrides),
            changed: overrides.iter().any(|(k, v)| {
                v.as_deref() == Some(a.name) || (v.is_none() && keys::DEFAULT_BINDINGS.iter().any(|(dk, da)| dk == k && *da == a.name))
            }),
        })
        .collect()
}

impl SettingsWindow {
    /// Show the actions matching `text` (from a settings search result).
    pub(super) fn shortcuts_search(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        self.shortcuts.filter = Filter::All;
        self.shortcuts.pressed = None;
        self.shortcuts.query.update(cx, |i, cx| i.set_text(text.to_owned(), cx));
    }

    /// Take a keystroke for the shortcut being recorded, or as the key
    /// search. Returns whether it was used.
    pub(super) fn record_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) -> bool {
        let listening = self.shortcuts.listening;
        if self.recording.is_none() && !listening {
            return false;
        }
        let k = &ev.keystroke;
        if k.key == "escape" && !k.modifiers.modified() {
            self.recording = None;
            self.shortcuts.listening = false;
            cx.stop_propagation();
            cx.notify();
            return true;
        }
        if matches!(k.key.as_str(), "shift" | "control" | "alt" | "platform" | "function") {
            return true; // wait for the actual key
        }
        let mut parts = Vec::new();
        for (on, name) in [
            (k.modifiers.control, "ctrl"),
            (k.modifiers.alt, "alt"),
            (k.modifiers.shift, "shift"),
            (k.modifiers.platform, "super"),
            (k.modifiers.function, "fn"),
        ] {
            if on {
                parts.push(name);
            }
        }
        parts.push(k.key.as_str());
        let keystroke = keys::normalize_keystroke(&parts.join("-"));
        cx.stop_propagation();
        if listening {
            self.shortcuts.listening = false;
            self.shortcuts.query.update(cx, |i, cx| i.set_text(pretty_keys(&keystroke), cx));
            self.shortcuts.pressed = Some(keystroke);
            cx.notify();
            return true;
        }
        let Some(action) = self.recording.take() else { return true };
        let overrides = &cx.global::<AppSettings>().file.value.keybindings;
        // One keystroke runs one action: say which one this took it from.
        self.shortcut_note = keys::action_for(&keystroke, overrides).filter(|a| a != action).map(|prev| {
            let label = keys::find_action(&prev).map_or(prev.as_str(), |a| a.label);
            (action, format!("{} was bound to \"{label}\"; it does this now.", pretty_keys(&keystroke)))
        });
        AppSettings::update(cx, |s| keys::set_binding(&mut s.keybindings, &keystroke, Some(action)));
        cx.notify();
        true
    }

    fn shortcut_row(&self, ui: Ui, line: &Line, cx: &Context<'_, Self>) -> AnyElement {
        let app = cx.global::<AppSettings>();
        let name = line.action.name;
        let recording = self.recording == Some(name);
        // Problems per keystroke: unreadable ones, and keys the user added
        // that the shell would otherwise get (the defaults are curated).
        let mut warnings = Vec::new();
        let problem: Vec<bool> = line
            .keys
            .iter()
            .map(|k| {
                let gpui_error = format!("keybindings.{k}: ");
                let broken = match keys::canonical_keystroke(k) {
                    Err(e) => Some(e),
                    Ok(_) => app.key_errors.iter().find_map(|e| e.strip_prefix(&gpui_error)).map(str::to_owned),
                };
                if let Some(e) = broken {
                    warnings.push(format!("{} doesn't work: {e}", pretty_keys(k)));
                    return true;
                }
                if !keys::is_default(k, name)
                    && let Some(c) = keys::shell_conflict(k)
                {
                    warnings.push(format!("{}: {c}. Bound here, the shell no longer gets it.", pretty_keys(k)));
                    return true;
                }
                false
            })
            .collect();
        if let Some((for_action, msg)) = &self.shortcut_note
            && *for_action == name
        {
            warnings.insert(0, msg.clone());
        }
        let group_id = SharedString::from(format!("sc-{name}"));
        let chips = div().flex().flex_wrap().items_center().gap(px(8.)).children(line.keys.iter().enumerate().map(|(ki, k)| {
            let key = k.clone();
            let gid = SharedString::from(format!("sc-key-{name}-{ki}"));
            div()
                .id(gid.clone())
                .group(gid.clone())
                .relative()
                .flex()
                .items_center()
                .gap(px(4.))
                .when(problem[ki], |d| d.p(px(2.)).rounded(px(8.)).border_1().border_color(ui.warning))
                .child(keycaps(k, ui))
                .child(
                    div()
                        .id(SharedString::from(format!("sc-rm-{name}-{ki}")))
                        .size(px(18.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .invisible()
                        .group_hover(gid, |s| s.visible())
                        .cursor_pointer()
                        .hover(|s| s.bg(ui.danger.opacity(0.15)))
                        .child(Icon::Close.svg().size(px(12.)).text_color(ui.muted))
                        .tooltip(tooltip("Remove this shortcut"))
                        .on_click(move |_, _, cx| AppSettings::update(cx, |s| keys::set_binding(&mut s.keybindings, &key, None))),
                )
        }));
        let cell = if recording {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(12.))
                .rounded_full()
                .border_1()
                .border_color(ui.accent)
                .bg(ui.accent_soft)
                .text_size(px(text::SMALL))
                .text_color(ui.accent_text)
                .child(Icon::Keyboard.svg().size(px(14.)).text_color(ui.accent_text))
                .child("Press the new keys · Esc cancels")
                .into_any_element()
        } else if line.keys.is_empty() {
            div().text_size(px(text::SMALL)).text_color(ui.faint).child("Not set").into_any_element()
        } else {
            chips.into_any_element()
        };
        let this = cx.entity().downgrade();
        let actions = div()
            .w(px(72.))
            .flex_shrink_0()
            .flex()
            .justify_end()
            .gap(px(2.))
            .when(line.changed, |d| {
                d.child(icon_button(
                    SharedString::from(format!("sc-reset-{name}")),
                    Icon::RotateCcw,
                    "Reset to the default",
                    ui,
                    move |_, cx| AppSettings::update(cx, |s| keys::reset_action(&mut s.keybindings, name)),
                ))
            })
            .child(
                icon_button(SharedString::from(format!("sc-add-{name}")), Icon::Plus, "Add a shortcut", ui, move |window, cx| {
                    let _ = this.update(cx, |v, cx| {
                        v.recording = Some(name);
                        v.shortcuts.listening = false;
                        v.shortcut_note = None;
                        window.focus(&v.focus, cx);
                        cx.notify();
                    });
                })
                .when(!line.keys.is_empty() && !line.changed, |d| d.invisible().group_hover(group_id.clone(), |s| s.visible())),
            );
        // The row is a tab stop: Enter records a new shortcut, Delete removes the last one.
        let row_id: gpui::ElementId = group_id.clone().into();
        let last_key = line.keys.last().cloned();
        let row = div()
            .id(row_id.clone())
            .role(gpui::Role::Row)
            .aria_label(line.action.label)
            .aria_description("Enter adds a shortcut, Delete removes the last one")
            .on_key_down({
                let (row_id, this) = (row_id.clone(), cx.entity().downgrade());
                move |ev, window, cx| {
                    // Only the row itself: a button inside it has its own Enter.
                    if ev.keystroke.modifiers.modified() || !focus::is_focused(&row_id, window) {
                        return;
                    }
                    match ev.keystroke.key.as_str() {
                        "enter" => {
                            cx.stop_propagation();
                            let _ = this.update(cx, |v, cx| {
                                v.recording = Some(name);
                                v.shortcuts.listening = false;
                                v.shortcut_note = None;
                                window.focus(&v.focus, cx);
                                cx.notify();
                            });
                        }
                        "delete" | "backspace" => {
                            if let Some(key) = &last_key {
                                cx.stop_propagation();
                                AppSettings::update(cx, |s| keys::set_binding(&mut s.keybindings, key, None));
                            }
                        }
                        _ => {}
                    }
                }
            })
            .group(group_id.clone())
            .flex()
            .flex_col()
            .px(px(16.))
            .py(px(8.))
            .min_h(px(48.))
            .justify_center()
            .when(recording, |d| d.bg(ui.accent_soft.opacity(0.5)))
            .hover(|s| s.bg(ui.surface_hover))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .flex_1()
                            .min_w(px(140.))
                            .child(div().min_w_0().text_size(px(text::BODY)).text_color(ui.text).child(line.action.label))
                            .when(line.changed, |d| d.child(badge("Changed", Tone::Accent, ui))),
                    )
                    .child(div().w(px(240.)).flex_shrink(1.).min_w(px(150.)).child(cell))
                    .child(actions),
            )
            .children((!warnings.is_empty()).then(|| {
                div().flex().flex_col().gap(px(2.)).pt(px(6.)).children(warnings.into_iter().map(|w| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(text::SMALL))
                        .text_color(ui.warning)
                        .child(Icon::TriangleAlert.svg().size(px(13.)).text_color(ui.warning))
                        .child(SharedString::from(w))
                }))
            }));
        focus::tab(row, &row_id, ui, Ring::Inner, Scroller::Page).into_any_element()
    }

    fn column_header(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let sort = self.shortcuts.sort;
        let head = |id: &'static str, label: &'static str, mine: Sort| {
            let on = sort == mine;
            let id: gpui::ElementId = id.into();
            let el = div()
                .id(id.clone())
                .role(gpui::Role::Button)
                .aria_label(label)
                .flex()
                .items_center()
                .gap(px(4.))
                .cursor_pointer()
                .text_color(if on { ui.accent_text } else { ui.muted })
                .hover(|s| s.text_color(ui.text))
                .child(label)
                .child(Icon::ArrowUpDown.svg().size(px(12.)).text_color(if on { ui.accent_text } else { ui.faint }))
                .tooltip(tooltip(if on { "Click again to group by area" } else { "Sort by this column" }))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.shortcuts.sort = if v.shortcuts.sort == mine { Sort::Group } else { mine };
                    cx.notify();
                }));
            focus::tab(el, &id, ui, Ring::Outer, Scroller::Page)
        };
        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .h(px(38.))
            .px(px(16.))
            .text_size(px(text::CAPTION))
            .font_weight(FontWeight::SEMIBOLD)
            .child(div().flex_1().min_w(px(140.)).child(head("sort-name", "ACTION", Sort::Name)))
            .child(div().w(px(240.)).flex_shrink(1.).min_w(px(150.)).child(head("sort-keys", "SHORTCUT", Sort::Keys)))
            .child(div().w(px(72.)))
    }

    pub(super) fn shortcuts_page(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        let app = cx.global::<AppSettings>();
        let all = lines(&app.file.value.keybindings);
        let st = &self.shortcuts;
        let query = st.query.read(cx).text().trim().to_string();
        let changed_n = all.iter().filter(|l| l.changed).count();
        let unbound_n = all.iter().filter(|l| l.keys.is_empty()).count();
        let mut shown: Vec<&Line> = all
            .iter()
            .filter(|l| match st.filter {
                Filter::All => true,
                Filter::Changed => l.changed,
                Filter::Unbound => l.keys.is_empty(),
            })
            .filter(|l| match &st.pressed {
                Some(k) => l.keys.iter().any(|key| key == k),
                None => matches(&query, l),
            })
            .collect();
        match st.sort {
            Sort::Group => {}
            Sort::Name => shown.sort_by_key(|l| l.action.label.to_lowercase()),
            Sort::Keys => shown.sort_by_key(|l| (l.keys.is_empty(), l.keys.first().map(|k| pretty_keys(k).to_lowercase()))),
        }

        let listen = icon_button_tinted(
            "sc-listen",
            Icon::Keyboard,
            if st.listening { "Listening: press the keys to find · Esc stops" } else { "Search by pressing keys" },
            st.listening.then_some(ui.on_accent),
            ui,
            self.act(cx, |v, cx| {
                v.shortcuts.listening = !v.shortcuts.listening;
                v.recording = None;
                cx.notify();
            }),
        )
        .size(px(38.))
        .bg(ui.field)
        .when(st.listening, |d| d.bg(ui.accent).hover(|s| s.bg(ui.accent)));
        let set_filter = |f: Filter| {
            self.act(cx, move |v, cx| {
                v.shortcuts.filter = f;
                cx.notify();
            })
        };
        let toolbar = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().child(if st.listening {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(38.))
                            .px(px(14.))
                            .rounded_full()
                            .bg(ui.accent_soft)
                            .border_1()
                            .border_color(ui.accent)
                            .text_size(px(text::BODY))
                            .text_color(ui.accent_text)
                            .child(Icon::Keyboard.svg().size(px(15.)).text_color(ui.accent_text))
                            .child("Press the shortcut you're looking for…")
                            .into_any_element()
                    } else {
                        search_field(ui, "sc-search", &st.query, window, cx).into_any_element()
                    }))
                    .child(listen),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(chip(ui, "sc-all", "All", Some(all.len()), st.filter == Filter::All, set_filter(Filter::All)))
                    .child(chip(ui, "sc-changed", "Changed", Some(changed_n), st.filter == Filter::Changed, set_filter(Filter::Changed)))
                    .child(chip(ui, "sc-unbound", "Unbound", Some(unbound_n), st.filter == Filter::Unbound, set_filter(Filter::Unbound)))
                    .child(div().flex_1())
                    .when(changed_n > 0, |d| {
                        let confirming = st.confirm_reset;
                        d.child(button_with(
                            "sc-reset-all",
                            Some(Icon::RotateCcw),
                            if confirming { "Reset every shortcut?" } else { "Reset all" },
                            if confirming { ButtonKind::Danger } else { ButtonKind::Ghost },
                            ui,
                            self.act(cx, |v, cx| {
                                if v.shortcuts.confirm_reset {
                                    AppSettings::update(cx, |s| s.keybindings.clear());
                                }
                                v.shortcuts.confirm_reset = !v.shortcuts.confirm_reset;
                                cx.notify();
                            }),
                        ))
                    }),
            );

        let mut table = card(ui).overflow_hidden().child(self.column_header(ui, cx)).child(div().h(px(1.)).bg(ui.divider));
        if shown.is_empty() {
            let (title, desc) = match (&st.pressed, st.filter) {
                (Some(k), _) => {
                    (format!("Nothing uses {}", pretty_keys(k)), "That key goes to the shell. Add it to an action with +.".to_string())
                }
                (None, Filter::Unbound) if query.is_empty() => ("Every action has a shortcut".into(), "Remove one to see it here.".into()),
                (None, Filter::Changed) if query.is_empty() => ("No changes yet".into(), "Every shortcut is goatty's default.".into()),
                _ => ("No matching shortcuts".into(), "Try another name or key, or clear the filter.".into()),
            };
            table = table.child(kit::empty(ui, Icon::Keyboard, title, desc));
        } else if st.sort == Sort::Group {
            let searching = !query.is_empty() || st.pressed.is_some();
            for (gi, g) in GROUPS.iter().enumerate() {
                let rows: Vec<&&Line> = shown.iter().filter(|l| l.action.group == *g).collect();
                if rows.is_empty() {
                    continue;
                }
                let folded = !searching && st.collapsed.contains(g);
                let g = *g;
                if gi > 0 {
                    table = table.child(div().h(px(1.)).bg(ui.divider));
                }
                let head_id: gpui::ElementId = SharedString::from(format!("sc-group-{g}")).into();
                let head = div()
                    .id(head_id.clone())
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(format!("{g} shortcuts")))
                    .aria_expanded(!folded)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(38.))
                    .px(px(14.))
                    .bg(ui.field.opacity(0.5))
                    .cursor_pointer()
                    .hover(|s| s.bg(ui.field))
                    .child((if folded { Icon::ChevronRight } else { Icon::ChevronDown }).svg().size(px(14.)).text_color(ui.muted))
                    .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).child(g))
                    .child(div().text_size(px(text::CAPTION)).text_color(ui.faint).child(rows.len().to_string()))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.shortcuts.collapsed.remove(g) {
                            v.shortcuts.collapsed.insert(g);
                        }
                        cx.notify();
                    }));
                table = table.child(focus::tab(head, &head_id, ui, Ring::Inner, Scroller::Page));
                if !folded {
                    for (i, l) in rows.iter().enumerate() {
                        if i > 0 {
                            table = table.child(divider(ui));
                        }
                        table = table.child(self.shortcut_row(ui, l, cx));
                    }
                }
            }
        } else {
            for (i, l) in shown.iter().enumerate() {
                if i > 0 {
                    table = table.child(divider(ui));
                }
                table = table.child(self.shortcut_row(ui, l, cx));
            }
        }

        let mut page = div().flex().flex_col().gap(px(16.)).child(toolbar);
        if !app.key_errors.is_empty() {
            page = page.child(banner(ui, Tone::Warning, "Some shortcuts don't work", app.key_errors.clone()));
        }
        page.child(table)
            .child(kit::hint(ui, "Shortcuts work in terminal panes; keys no action uses go to the shell. Symbol keys show the character they type: Ctrl+Shift with the + key on a German layout is Ctrl+*.").px(px(4.)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_by_name_or_keys() {
        let a = keys::find_action("new_tab").unwrap();
        let l = Line { action: a, keys: vec!["ctrl-shift-t".into()], changed: false };
        for q in ["", "new tab", "tab new", "TAB", "ctrl+shift+t", "Ctrl+Shift+T", "shift-t", "tabs"] {
            assert!(matches(q, &l), "{q:?}");
        }
        assert!(!matches("paste", &l));
        assert!(!matches("ctrl+alt", &l));
    }
}
