//! Keyboard and mouse → byte encoding (xterm conventions).
//!
//! UI-agnostic: the GPUI layer maps its keystrokes into [`Key`] + [`Mods`].
//! Kitty keyboard protocol is not implemented yet (TODO: honor
//! `Term::keyboard_mode()`).

use crate::TermMode;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl Mods {
    /// xterm modifier parameter (1 + bitmask), or None when no modifiers.
    fn param(self) -> Option<u8> {
        let m = (self.shift as u8) | (self.alt as u8) << 1 | (self.ctrl as u8) << 2 | (self.logo as u8) << 3;
        (m != 0).then_some(m + 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key<'a> {
    /// Printable text as produced by the keyboard layout (already shifted).
    Text(&'a str),
    Enter,
    Backspace,
    Tab,
    Escape,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

/// Encode a key press. `None` means "not something the terminal handles".
pub fn encode_key(key: &Key, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let esc = |mut v: Vec<u8>| {
        if mods.alt {
            v.insert(0, 0x1b);
        }
        v
    };
    let csi_letter = |letter: u8| -> Vec<u8> {
        match mods.param() {
            Some(m) => format!("\x1b[1;{m}{}", letter as char).into_bytes(),
            None if app_cursor => vec![0x1b, b'O', letter],
            None => vec![0x1b, b'[', letter],
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        match mods.param() {
            Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
            None => format!("\x1b[{n}~").into_bytes(),
        }
    };
    let ss3_f = |letter: u8| -> Vec<u8> {
        match mods.param() {
            Some(m) => format!("\x1b[1;{m}{}", letter as char).into_bytes(),
            None => vec![0x1b, b'O', letter],
        }
    };

    Some(match key {
        Key::Enter => esc(vec![b'\r']),
        Key::Backspace => {
            if mods.ctrl {
                esc(vec![0x08])
            } else {
                esc(vec![0x7f])
            }
        }
        Key::Tab => {
            if mods.shift {
                b"\x1b[Z".to_vec()
            } else {
                esc(vec![b'\t'])
            }
        }
        Key::Escape => esc(vec![0x1b]),
        Key::Space => {
            if mods.ctrl {
                esc(vec![0])
            } else {
                esc(vec![b' '])
            }
        }
        Key::Up => csi_letter(b'A'),
        Key::Down => csi_letter(b'B'),
        Key::Right => csi_letter(b'C'),
        Key::Left => csi_letter(b'D'),
        Key::Home => csi_letter(b'H'),
        Key::End => csi_letter(b'F'),
        Key::PageUp => csi_tilde(5),
        Key::PageDown => csi_tilde(6),
        Key::Insert => csi_tilde(2),
        Key::Delete => csi_tilde(3),
        Key::F(n) => match n {
            1 => ss3_f(b'P'),
            2 => ss3_f(b'Q'),
            3 => ss3_f(b'R'),
            4 => ss3_f(b'S'),
            5 => csi_tilde(15),
            6 => csi_tilde(17),
            7 => csi_tilde(18),
            8 => csi_tilde(19),
            9 => csi_tilde(20),
            10 => csi_tilde(21),
            11 => csi_tilde(23),
            12 => csi_tilde(24),
            _ => return None,
        },
        Key::Text(s) => {
            if s.is_empty() {
                return None;
            }
            if mods.ctrl {
                let mut chars = s.chars();
                let c = chars.next()?;
                if chars.next().is_none()
                    && let Some(b) = ctrl_byte(c)
                {
                    return Some(esc(vec![b]));
                }
            }
            esc(s.as_bytes().to_vec())
        }
    })
}

/// Map ctrl+<char> to its C0 control byte.
fn ctrl_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' | ' ' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '-' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// Wrap pasted text, honoring bracketed paste and neutralizing an embedded
/// end-marker (a classic paste-injection trick).
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let clean = text.replace("\x1b[201~", "");
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(clean.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        // Without bracketed paste, newlines would execute immediately; xterm
        // converts LF to CR.
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// Encode a mouse event for applications that enabled mouse reporting.
/// `col`/`row` are 0-based viewport cells.
pub fn encode_mouse(button: MouseButton, action: MouseAction, col: usize, row: usize, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    if action == MouseAction::Motion {
        let dragging = button != MouseButton::None;
        let ok = mode.contains(TermMode::MOUSE_MOTION) || (dragging && mode.contains(TermMode::MOUSE_DRAG));
        if !ok {
            return None;
        }
    }
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if action == MouseAction::Motion {
        code += 32;
    }
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }
    let (x, y) = (col + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if action == MouseAction::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes());
    }
    // Legacy X10 encoding: release is button 3, coordinates capped at 223.
    if action == MouseAction::Release {
        code = 3 + (code & !3);
    }
    if x > 223 || y > 223 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', 32 + code as u8, 32 + x as u8, 32 + y as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_is_etx() {
        let m = Mods { ctrl: true, ..Default::default() };
        assert_eq!(encode_key(&Key::Text("c"), m, TermMode::empty()).unwrap(), vec![3]);
    }

    #[test]
    fn arrows_respect_decckm() {
        let none = Mods::default();
        assert_eq!(encode_key(&Key::Up, none, TermMode::empty()).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(&Key::Up, none, TermMode::APP_CURSOR).unwrap(), b"\x1bOA");
        let ctrl = Mods { ctrl: true, ..Default::default() };
        assert_eq!(encode_key(&Key::Right, ctrl, TermMode::APP_CURSOR).unwrap(), b"\x1b[1;5C");
    }

    #[test]
    fn alt_prefixes_escape() {
        let m = Mods { alt: true, ..Default::default() };
        assert_eq!(encode_key(&Key::Text("b"), m, TermMode::empty()).unwrap(), b"\x1bb");
    }

    #[test]
    fn paste_strips_injected_end_marker() {
        let out = encode_paste("rm -rf x\x1b[201~; echo pwned\n", TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~rm -rf x; echo pwned\n\x1b[201~");
    }
}
