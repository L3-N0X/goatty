//! GPUI keystrokes → terminal keys. Pure mapping, no I/O; the byte encoding
//! itself lives in `goatty_term::input`.

use goatty_term::input::{Key, Mods};
use gpui::{Keystroke, Modifiers};

pub fn mods(m: &Modifiers) -> Mods {
    Mods { ctrl: m.control, alt: m.alt, shift: m.shift, logo: m.platform }
}

/// The terminal key for `ks`, or `None` for modifier-only and unknown keys
/// and for Super combinations (those belong to the compositor).
pub fn terminal_key(ks: &Keystroke) -> Option<(Key<'_>, Mods)> {
    let m = ks.modifiers;
    if m.platform {
        return None;
    }
    let key = match ks.key.as_str() {
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        "space" => Key::Space,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        k if k.len() >= 2 && k.starts_with('f') && k[1..].parse::<u8>().is_ok() => Key::F(k[1..].parse().ok()?),
        k => match &ks.key_char {
            // Typed text from the layout (already shifted). With ctrl the
            // layout often gives nothing; fall back to the key name.
            Some(t) if !m.control => Key::Text(t.as_str()),
            _ if k.chars().count() == 1 => Key::Text(k),
            _ => return None,
        },
    };
    let mut mods = mods(&m);
    // Text is already shifted by the layout; don't let the encoder see shift.
    if matches!(key, Key::Text(_)) {
        mods.shift = false;
    }
    Some((key, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(Debug of the key, mods)` for a keystroke spec plus layout text.
    fn map(src: &'static str, key_char: Option<&str>) -> Option<(String, Mods)> {
        let mut ks = Keystroke::parse(src).unwrap();
        ks.key_char = key_char.map(String::from);
        terminal_key(&ks).map(|(k, m)| (format!("{k:?}"), m))
    }

    #[test]
    fn named_and_function_keys() {
        assert_eq!(map("enter", None).unwrap().0, "Enter");
        assert_eq!(map("f5", None).unwrap().0, "F(5)");
        let (key, m) = map("shift-up", None).unwrap();
        assert_eq!((key.as_str(), m.shift), ("Up", true));
    }

    #[test]
    fn text_uses_layout_char_without_shift() {
        let (key, m) = map("shift-a", Some("A")).unwrap();
        assert_eq!(key, r#"Text("A")"#);
        assert!(!m.shift);
        // Ctrl combos often have no key_char: fall back to the key name.
        let (key, m) = map("ctrl-r", None).unwrap();
        assert_eq!((key.as_str(), m.ctrl), (r#"Text("r")"#, true));
    }

    #[test]
    fn super_and_modifier_only_are_ignored() {
        assert!(map("super-t", None).is_none());
        assert!(map("shift", None).is_none());
    }
}
