//! Shortcuts: the actions users can bind, their default keys, and how the
//! `keybindings` overrides combine with them.
//!
//! Keystrokes use GPUI's spelling (`ctrl-shift-c`, `ctrl-,`). The app maps
//! action names to GPUI actions and checks that keystrokes parse.

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
];

pub fn find_action(name: &str) -> Option<&'static ActionInfo> {
    ACTIONS.iter().find(|a| a.name == name)
}

/// Default keystroke → action table.
pub static DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("ctrl-shift-c", "copy"),
    ("ctrl-c", "copy_or_interrupt"),
    ("ctrl-shift-v", "paste"),
    ("ctrl-v", "paste"),
    ("shift-pageup", "scroll_page_up"),
    ("shift-pagedown", "scroll_page_down"),
    // Layouts report Shift+= as "=" or "+"; bind both spellings.
    ("ctrl-shift-=", "increase_font_size"),
    ("ctrl-shift-+", "increase_font_size"),
    ("ctrl-shift--", "decrease_font_size"),
    ("ctrl-shift-_", "decrease_font_size"),
    ("ctrl-0", "reset_font_size"),
    ("ctrl-,", "open_settings"),
];

/// Normalized spelling of a keystroke for map keys: lowercase, trimmed.
pub fn normalize_keystroke(k: &str) -> String {
    k.trim().to_ascii_lowercase()
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

/// Edit helper: bind `key` to `action`, keeping the override map minimal
/// (an override equal to the default is removed).
pub fn set_binding(overrides: &mut BTreeMap<String, Option<String>>, key: &str, action: Option<&str>) {
    let key = normalize_keystroke(key);
    let default = DEFAULT_BINDINGS.iter().find(|(k, _)| *k == key).map(|(_, a)| *a);
    if default == action {
        overrides.remove(&key);
    } else {
        overrides.insert(key, action.map(str::to_owned));
    }
}

/// Edit helper: remove every override for `action` so it's back to its defaults.
pub fn reset_action(overrides: &mut BTreeMap<String, Option<String>>, action: &str) {
    overrides.retain(|key, a| {
        let default = DEFAULT_BINDINGS.iter().find(|(k, _)| k == key).map(|(_, a)| *a);
        a.as_deref() != Some(action) && !(a.is_none() && default == Some(action))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_name_real_actions() {
        for (k, a) in DEFAULT_BINDINGS {
            assert!(find_action(a).is_some(), "{k} -> {a}");
            assert_eq!(normalize_keystroke(k), *k);
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
    }
}
