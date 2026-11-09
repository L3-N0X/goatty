//! Shortcuts: the actions users can bind, their default keys, and how the
//! `keybindings` overrides combine with them.
//!
//! Keystrokes use GPUI's spelling (`ctrl-shift-c`, `ctrl-,`). The app maps
//! action names to GPUI actions and checks that keystrokes parse.
//!
//! # Symbols and Shift
//!
//! A symbol key is named by the character it types, never by Shift plus the
//! unshifted character: GPUI on Linux reports Ctrl+Shift+= on a US layout as
//! `ctrl-+`, and Ctrl+Shift+(the + key) on a German one as `ctrl-*`, without
//! `shift`. So `ctrl-shift-+` could never fire. [`canonical_keystroke`] drops
//! Shift in front of symbols and digits, and the defaults list the characters
//! the common layouts type.

use std::collections::BTreeMap;

/// A bindable action.
#[derive(Debug)]
pub struct ActionInfo {
    /// Stable name used in settings.
    pub name: &'static str,
    pub label: &'static str,
}

pub static ACTIONS: &[ActionInfo] = &[
    ActionInfo { name: "copy", label: "Copy the selection" },
    ActionInfo { name: "copy_or_interrupt", label: "Copy if there's a selection, otherwise interrupt (Ctrl+C)" },
    ActionInfo { name: "paste", label: "Paste" },
    ActionInfo { name: "scroll_page_up", label: "Scroll back one screen" },
    ActionInfo { name: "scroll_page_down", label: "Scroll forward one screen" },
    ActionInfo { name: "increase_font_size", label: "Bigger text" },
    ActionInfo { name: "decrease_font_size", label: "Smaller text" },
    ActionInfo { name: "reset_font_size", label: "Reset text size" },
    ActionInfo { name: "open_settings", label: "Open settings" },
    ActionInfo { name: "command_palette", label: "Show the command palette" },
    ActionInfo { name: "reload_shells", label: "Reload running shells (pick up shell settings)" },
    ActionInfo { name: "new_tab", label: "New tab" },
    ActionInfo { name: "close_pane", label: "Close the pane (the tab with its last pane)" },
    ActionInfo { name: "close_tab", label: "Close the tab" },
    ActionInfo { name: "next_tab", label: "Next tab" },
    ActionInfo { name: "previous_tab", label: "Previous tab" },
    ActionInfo { name: "move_tab_left", label: "Move the tab left" },
    ActionInfo { name: "move_tab_right", label: "Move the tab right" },
    ActionInfo { name: "select_tab_1", label: "Go to tab 1" },
    ActionInfo { name: "select_tab_2", label: "Go to tab 2" },
    ActionInfo { name: "select_tab_3", label: "Go to tab 3" },
    ActionInfo { name: "select_tab_4", label: "Go to tab 4" },
    ActionInfo { name: "select_tab_5", label: "Go to tab 5" },
    ActionInfo { name: "select_tab_6", label: "Go to tab 6" },
    ActionInfo { name: "select_tab_7", label: "Go to tab 7" },
    ActionInfo { name: "select_tab_8", label: "Go to tab 8" },
    ActionInfo { name: "select_last_tab", label: "Go to the last tab" },
    ActionInfo { name: "rename_tab", label: "Rename the tab" },
    ActionInfo { name: "split_right", label: "Split: new pane on the right" },
    ActionInfo { name: "split_down", label: "Split: new pane below" },
    ActionInfo { name: "focus_pane_left", label: "Focus the pane on the left" },
    ActionInfo { name: "focus_pane_right", label: "Focus the pane on the right" },
    ActionInfo { name: "focus_pane_up", label: "Focus the pane above" },
    ActionInfo { name: "focus_pane_down", label: "Focus the pane below" },
    ActionInfo { name: "resize_pane_left", label: "Move the pane's divider left" },
    ActionInfo { name: "resize_pane_right", label: "Move the pane's divider right" },
    ActionInfo { name: "resize_pane_up", label: "Move the pane's divider up" },
    ActionInfo { name: "resize_pane_down", label: "Move the pane's divider down" },
    ActionInfo { name: "toggle_pane_zoom", label: "Zoom the pane (fill the tab) or back" },
    ActionInfo { name: "equalize_panes", label: "Make the tab's panes the same size" },
];

pub fn find_action(name: &str) -> Option<&'static ActionInfo> {
    ACTIONS.iter().find(|a| a.name == name)
}

/// Default keystroke → action table (canonical spellings).
pub static DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("ctrl-shift-c", "copy"),
    ("ctrl-c", "copy_or_interrupt"),
    ("ctrl-shift-v", "paste"),
    ("ctrl-v", "paste"),
    ("shift-pageup", "scroll_page_up"),
    ("shift-pagedown", "scroll_page_down"),
    // Ctrl with the plus key, with or without Shift, on any layout:
    // US Ctrl+= / Ctrl+Shift+= type "=" / "+", German Ctrl++ / Ctrl+Shift++
    // type "+" / "*". The keypad's + matches "+" too.
    ("ctrl-=", "increase_font_size"),
    ("ctrl-+", "increase_font_size"),
    ("ctrl-*", "increase_font_size"),
    // Ctrl with the minus key, with or without Shift ("-" / "_" on US and German).
    ("ctrl--", "decrease_font_size"),
    ("ctrl-_", "decrease_font_size"),
    ("ctrl-0", "reset_font_size"),
    ("ctrl-,", "open_settings"),
    ("ctrl-shift-p", "command_palette"),
    // Tabs. Alt+digit and Alt+arrows fall through to the shell when there's
    // no such tab or pane, so a single pane loses nothing.
    ("ctrl-shift-t", "new_tab"),
    ("ctrl-shift-w", "close_pane"),
    ("ctrl-tab", "next_tab"),
    ("ctrl-shift-tab", "previous_tab"),
    ("ctrl-pagedown", "next_tab"),
    ("ctrl-pageup", "previous_tab"),
    ("ctrl-shift-pageup", "move_tab_left"),
    ("ctrl-shift-pagedown", "move_tab_right"),
    ("alt-1", "select_tab_1"),
    ("alt-2", "select_tab_2"),
    ("alt-3", "select_tab_3"),
    ("alt-4", "select_tab_4"),
    ("alt-5", "select_tab_5"),
    ("alt-6", "select_tab_6"),
    ("alt-7", "select_tab_7"),
    ("alt-8", "select_tab_8"),
    ("alt-9", "select_last_tab"),
    // Panes (Terminator's split keys, Windows Terminal's focus/resize keys).
    ("ctrl-shift-e", "split_right"),
    ("ctrl-shift-o", "split_down"),
    ("alt-left", "focus_pane_left"),
    ("alt-right", "focus_pane_right"),
    ("alt-up", "focus_pane_up"),
    ("alt-down", "focus_pane_down"),
    ("alt-shift-left", "resize_pane_left"),
    ("alt-shift-right", "resize_pane_right"),
    ("alt-shift-up", "resize_pane_up"),
    ("alt-shift-down", "resize_pane_down"),
    ("ctrl-shift-z", "toggle_pane_zoom"),
];

/// Modifiers in canonical order, with the spellings accepted for each.
const MODIFIERS: &[(&str, &[&str])] = &[
    ("ctrl", &["ctrl", "control"]),
    ("alt", &["alt", "option"]),
    ("shift", &["shift"]),
    ("super", &["super", "cmd", "win", "platform"]),
    ("fn", &["fn", "function"]),
];

/// Key names accepted as aliases (`ctrl-plus` → `ctrl-+`).
const KEY_ALIASES: &[(&str, &str)] =
    &[("plus", "+"), ("minus", "-"), ("equal", "="), ("underscore", "_"), ("esc", "escape"), ("return", "enter"), ("del", "delete")];

/// A keystroke in canonical form: lowercase, modifiers in a fixed order
/// (`ctrl-alt-shift-super-fn`), key aliases resolved, and no `shift` in front
/// of a symbol or digit (see the module docs). `Err` explains what's wrong.
pub fn canonical_keystroke(k: &str) -> Result<String, String> {
    let k = k.trim().to_lowercase();
    if k.is_empty() {
        return Err("empty keystroke".into());
    }
    if k.contains(char::is_whitespace) {
        return Err(format!("{k:?}: one keystroke only (no spaces or key sequences)"));
    }
    // The key itself may be "-": `ctrl--`, `-`.
    let (mods, key) = match k.strip_suffix("--") {
        Some(rest) => (rest, "-"),
        None if k == "-" => ("", "-"),
        None => match k.rsplit_once('-') {
            Some((mods, key)) => (mods, key),
            None => ("", k.as_str()),
        },
    };
    if key.is_empty() {
        return Err(format!("{k:?}: no key after the modifiers"));
    }
    let mut on = [false; MODIFIERS.len()];
    if !mods.is_empty() {
        for m in mods.split('-') {
            let Some(i) = MODIFIERS.iter().position(|(_, names)| names.contains(&m)) else {
                return Err(format!("{k:?}: unknown modifier {m:?} (use ctrl, alt, shift, super)"));
            };
            on[i] = true;
        }
    }
    let key = KEY_ALIASES.iter().find(|(alias, _)| *alias == key).map_or(key, |(_, k)| k);
    if MODIFIERS.iter().any(|(_, names)| names.contains(&key)) {
        return Err(format!("{k:?}: a modifier alone can't be a shortcut"));
    }
    if is_symbol(key) {
        on[2] = false; // shift
    }
    let mut out: Vec<&str> = MODIFIERS.iter().zip(on).filter(|(_, on)| *on).map(|((name, _), _)| *name).collect();
    out.push(key);
    Ok(out.join("-"))
}

/// A single character without case: a symbol or digit, which already says
/// whether Shift was held.
fn is_symbol(key: &str) -> bool {
    let mut c = key.chars();
    matches!((c.next(), c.next()), (Some(ch), None) if !ch.is_alphabetic())
}

/// Canonical spelling for map keys; falls back to trimmed lowercase when the
/// keystroke doesn't parse (validation reports it).
pub fn normalize_keystroke(k: &str) -> String {
    canonical_keystroke(k).unwrap_or_else(|_| k.trim().to_lowercase())
}

/// Rewrite `overrides` to canonical keystrokes. Returns, per problem, the
/// original spelling and what's wrong: a keystroke that doesn't parse (kept
/// as is), or one that names the same keys as another entry (dropped; the
/// first in map order wins).
pub fn canonicalize_overrides(overrides: &mut BTreeMap<String, Option<String>>) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let mut out: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut origin: BTreeMap<String, String> = BTreeMap::new();
    for (key, action) in std::mem::take(overrides) {
        let canon = match canonical_keystroke(&key) {
            Ok(c) => c,
            Err(e) => {
                problems.push((key.clone(), e));
                key.clone()
            }
        };
        if let Some(prev) = out.get(&canon) {
            if *prev != action {
                let first = &origin[&canon];
                problems.push((key, format!("same keys as {first:?}; only {first:?} is used")));
            }
            continue;
        }
        origin.insert(canon.clone(), key);
        if action.is_some() && default_action(&canon) == action.as_deref() {
            continue; // repeats a default
        }
        out.insert(canon, action);
    }
    *overrides = out;
    problems
}

/// The defaults with `overrides` applied: a keystroke mapped to an action
/// (re)binds it, mapped to `null` unbinds it. Sorted by keystroke.
pub fn effective_bindings(overrides: &BTreeMap<String, Option<String>>) -> Vec<(String, String)> {
    let mut map: BTreeMap<String, String> = DEFAULT_BINDINGS.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect();
    for (key, action) in overrides {
        let key = normalize_keystroke(key);
        match action {
            Some(a) if find_action(a).is_some() => {
                map.insert(key, a.clone());
            }
            Some(_) => {} // reported by validation
            None => {
                map.remove(&key);
            }
        }
    }
    map.into_iter().collect()
}

/// Keystrokes bound to `action`, given the overrides.
pub fn keys_for(action: &str, overrides: &BTreeMap<String, Option<String>>) -> Vec<String> {
    effective_bindings(overrides).into_iter().filter(|(_, a)| a == action).map(|(k, _)| k).collect()
}

/// The action `key` runs, given the overrides.
pub fn action_for(key: &str, overrides: &BTreeMap<String, Option<String>>) -> Option<String> {
    let key = normalize_keystroke(key);
    effective_bindings(overrides).into_iter().find(|(k, _)| *k == key).map(|(_, a)| a)
}

fn default_action(key: &str) -> Option<&'static str> {
    DEFAULT_BINDINGS.iter().find(|(k, _)| *k == key).map(|(_, a)| *a)
}

/// Whether `key` → `action` is one of the defaults.
pub fn is_default(key: &str, action: &str) -> bool {
    default_action(&normalize_keystroke(key)) == Some(action)
}

/// Edit helper: bind `key` to `action`, keeping the override map minimal
/// (an override equal to the default is removed).
pub fn set_binding(overrides: &mut BTreeMap<String, Option<String>>, key: &str, action: Option<&str>) {
    let key = normalize_keystroke(key);
    overrides.retain(|k, _| normalize_keystroke(k) != key);
    if default_action(&key) != action {
        overrides.insert(key, action.map(str::to_owned));
    }
}

/// Edit helper: remove every override for `action` so it's back to its defaults.
pub fn reset_action(overrides: &mut BTreeMap<String, Option<String>>, action: &str) {
    overrides.retain(|key, a| {
        let default = default_action(&normalize_keystroke(key));
        a.as_deref() != Some(action) && !(a.is_none() && default == Some(action))
    });
}

/// What a keystroke takes away from the shell or the programs in it, if
/// anything: once bound, the key no longer reaches them.
pub fn shell_conflict(key: &str) -> Option<String> {
    let key = canonical_keystroke(key).ok()?;
    let parts: Vec<&str> = if key.ends_with("--") || key == "-" {
        let mut p: Vec<&str> = key.trim_end_matches('-').split('-').filter(|s| !s.is_empty()).collect();
        p.push("-");
        p
    } else {
        key.split('-').collect()
    };
    let (k, mods) = parts.split_last()?;
    let mods: Vec<&str> = mods.to_vec();
    let ctrl_only = mods == ["ctrl"];
    if ctrl_only && let Some(what) = ctrl_key_use(k) {
        return Some(format!("Ctrl+{} is {what} in the shell", k.to_uppercase()));
    }
    if mods == ["alt"] && k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric() || c.is_ascii_punctuation()) {
        return Some(format!("Alt+{} is used by shell line editing and many TUIs", k.to_uppercase()));
    }
    if mods.is_empty() || mods == ["shift"] {
        let typing = k.chars().count() == 1
            || matches!(
                *k,
                "enter" | "tab" | "backspace" | "delete" | "escape" | "space" | "up" | "down" | "left" | "right" | "home" | "end"
            );
        if typing {
            return Some("without Ctrl or Alt this key is part of normal typing".into());
        }
    }
    None
}

/// What zsh (emacs mode), the terminal, or common TUIs do with Ctrl+`key`.
fn ctrl_key_use(key: &str) -> Option<&'static str> {
    Some(match key {
        "a" => "start of line",
        "b" => "back one character",
        "c" => "interrupt",
        "d" => "end of input (closes the shell on an empty line)",
        "e" => "end of line",
        "f" => "forward one character",
        "g" => "abort",
        "h" => "backspace",
        "i" => "Tab",
        "j" | "m" => "Enter",
        "k" => "cut to the end of the line",
        "l" => "clear screen",
        "n" => "next history entry",
        "o" => "accept and run the next line",
        "p" => "previous history entry",
        "q" => "resume output (XON)",
        "r" => "history search",
        "s" => "pause output (XOFF) / forward search",
        "t" => "swap characters",
        "u" => "cut the line",
        "v" => "insert the next key literally (also Vim block selection)",
        "w" => "delete the previous word",
        "x" => "the prefix of many editing commands",
        "y" => "paste what was cut",
        "z" => "suspend",
        "[" => "Escape",
        "\\" => "quit (SIGQUIT)",
        "_" | "-" | "/" => "undo",
        "space" | "@" => "set mark",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_name_real_actions() {
        for (k, a) in DEFAULT_BINDINGS {
            assert!(find_action(a).is_some(), "{k} -> {a}");
            assert_eq!(canonical_keystroke(k).as_deref(), Ok(*k));
        }
    }

    #[test]
    fn canonical_spelling() {
        let c = |k| canonical_keystroke(k).unwrap();
        assert_eq!(c("Ctrl-Shift-C"), "ctrl-shift-c");
        assert_eq!(c("shift-ctrl-c"), "ctrl-shift-c");
        assert_eq!(c("control-option-cmd-k"), "ctrl-alt-super-k");
        // Shift is implied by the symbol: GPUI never reports it.
        assert_eq!(c("ctrl-shift-+"), "ctrl-+");
        assert_eq!(c("ctrl-shift-_"), "ctrl-_");
        assert_eq!(c("ctrl-shift--"), "ctrl--");
        assert_eq!(c("ctrl-shift-1"), "ctrl-1");
        assert_eq!(c("ctrl-plus"), "ctrl-+");
        assert_eq!(c("ctrl--"), "ctrl--");
        assert_eq!(c("-"), "-");
        assert_eq!(c("shift-pageup"), "shift-pageup");
        assert_eq!(c("alt-shift-ä"), "alt-shift-ä");
        for bad in ["", "ctrl-", "hyper-k", "ctrl-shift", "ctrl-a ctrl-b", "ctrl-+-+"] {
            assert!(canonical_keystroke(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn overrides_get_canonical_keys() {
        let mut o = BTreeMap::new();
        o.insert("ctrl-shift-+".to_string(), Some("paste".to_string()));
        o.insert("Ctrl-Plus".to_string(), Some("copy".to_string()));
        o.insert("ctrl-+-+".to_string(), Some("copy".to_string()));
        let problems = canonicalize_overrides(&mut o);
        assert_eq!(o.len(), 2, "{o:?}");
        assert_eq!(o.get("ctrl-+"), Some(&Some("copy".to_string())), "first in map order wins");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|(k, _)| k == "ctrl-+-+"));
        assert!(problems.iter().any(|(k, e)| k == "ctrl-shift-+" && e.contains("Ctrl-Plus")));

        // Overrides that repeat a default (as recorded before the fix) go away.
        let mut o = BTreeMap::from([("ctrl-shift-*".to_string(), Some("increase_font_size".to_string()))]);
        assert!(canonicalize_overrides(&mut o).is_empty());
        assert!(o.is_empty(), "{o:?}");
    }

    #[test]
    fn zoom_defaults_cover_us_and_german_layouts() {
        let none = BTreeMap::new();
        // What GPUI on Linux reports for Ctrl(+Shift) with the plus/minus keys.
        for typed in ["ctrl-=", "ctrl-+", "ctrl-*"] {
            assert_eq!(action_for(typed, &none).as_deref(), Some("increase_font_size"), "{typed}");
        }
        for typed in ["ctrl--", "ctrl-_"] {
            assert_eq!(action_for(typed, &none).as_deref(), Some("decrease_font_size"), "{typed}");
        }
    }

    #[test]
    fn overrides_rebind_and_unbind() {
        let mut o = BTreeMap::new();
        set_binding(&mut o, "ctrl-v", None);
        set_binding(&mut o, "Ctrl-Alt-S", Some("open_settings"));
        assert_eq!(o.len(), 2);
        assert_eq!(keys_for("paste", &o), ["ctrl-shift-v"]);
        assert_eq!(keys_for("open_settings", &o), ["ctrl-,", "ctrl-alt-s"]);

        // Setting a key back to its default leaves no override.
        set_binding(&mut o, "ctrl-v", Some("paste"));
        assert_eq!(o.len(), 1);

        set_binding(&mut o, "ctrl-,", None);
        reset_action(&mut o, "open_settings");
        assert!(o.is_empty(), "{o:?}");
        assert_eq!(keys_for("open_settings", &o), ["ctrl-,"]);

        // An old non-canonical spelling is replaced, not shadowed.
        o.insert("ctrl-shift-*".into(), Some("paste".into()));
        set_binding(&mut o, "ctrl-*", None);
        assert_eq!(o.len(), 1);
        assert_eq!(o.get("ctrl-*"), Some(&None));
        assert_eq!(action_for("ctrl-*", &o), None);
    }

    #[test]
    fn shell_conflicts() {
        assert!(shell_conflict("ctrl-r").unwrap().contains("history search"));
        assert!(shell_conflict("ctrl-w").is_some());
        assert!(shell_conflict("ctrl-_").unwrap().contains("undo"));
        assert!(shell_conflict("alt-f").is_some());
        assert!(shell_conflict("a").is_some());
        assert!(shell_conflict("shift-enter").is_some());
        assert_eq!(shell_conflict("ctrl-shift-r"), None);
        assert_eq!(shell_conflict("ctrl-,"), None);
        assert_eq!(shell_conflict("shift-pageup"), None);
        assert_eq!(shell_conflict("f5"), None);
    }
}
