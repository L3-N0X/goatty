//! Reattach snapshots: serialize the daemon's authoritative terminal state as
//! a VT byte stream that, fed into a *fresh* client terminal, reproduces it.
//!
//! rio-vt's `contents_formatted()` covers the visible screen + cursor. We add
//! what it leaves out and what reconnecting actually needs:
//!   * scrollback (replayed as lines so they scroll into the client history),
//!   * the alternate screen switch (so `vim` exiting restores correctly),
//!   * input-affecting modes (DECCKM, keypad, bracketed paste, mouse, focus),
//!   * the window title.

use std::fmt::Write as _;

use rio_vt::crosswords::formatter::write_sgr;
use rio_vt::crosswords::pos::{Column, Line};
use rio_vt::crosswords::style::Style;

use crate::{Term, TermMode, Terminal};

/// Upper bound on replayed history lines; keeps attach latency bounded.
pub const MAX_REPLAY_HISTORY: usize = 5_000;

impl Terminal {
    /// Full snapshot for a *fresh* client terminal: RIS, up to
    /// `max_history` scrollback lines, screen, modes.
    pub fn reattach_snapshot(&self) -> Vec<u8> {
        self.reattach_snapshot_with_history(MAX_REPLAY_HISTORY)
    }

    pub fn reattach_snapshot_with_history(&self, max_history: usize) -> Vec<u8> {
        self.with_term(|t| serialize(t, max_history.min(MAX_REPLAY_HISTORY)))
    }

    /// Catch-up snapshot for a client terminal that already mirrors this
    /// session but missed some output (flow control skipped it). Unlike
    /// [`Self::reattach_snapshot`] it does not reset the client, so the
    /// client keeps its scrollback; the old screen scrolls into history
    /// (rio's `ED 2` on the primary screen pushes the viewport up) and the
    /// skipped lines are simply absent. Small: one screen plus modes.
    pub fn resync_snapshot(&self) -> Vec<u8> {
        self.with_term(resync)
    }
}

fn resync(t: &mut Term) -> Vec<u8> {
    let mode = t.mode();
    let mut out = String::new();
    // Undo state the skipped bytes may have left on the client: scroll
    // region, origin mode, and the alt-screen switch in whichever direction.
    out.push_str("\x1b[0m\x1b[r\x1b[?6l");
    out.push_str(if mode.contains(TermMode::ALT_SCREEN) { "\x1b[?1049h" } else { "\x1b[?1049l" });
    out.push_str(&String::from_utf8_lossy(&t.contents_formatted()));
    // Mouse protocols are one setting in rio: reset, then set what's active.
    out.push_str("\x1b[?1000l\x1b[?1005l\x1b[?1006l");
    write_modes(t, mode, &mut out);
    out.into_bytes()
}

fn serialize(t: &Term, max_history: usize) -> Vec<u8> {
    let mode = t.mode();
    let mut out = String::new();
    // Full reset of the fresh client, then UTF-8 + autowrap defaults.
    out.push_str("\x1bc");

    let alt = mode.contains(TermMode::ALT_SCREEN);
    // Scrollback only exists for the primary grid. When the alt screen is
    // active, `t.grid` *is* the alt grid (rio swaps them), so history replay
    // is only possible on the primary screen.
    if !alt {
        let history = t.history_size().min(max_history);
        let cols = t.columns();
        let mut current = Style::default();
        for h in (1..=history).rev() {
            write_row(t, Line(-(h as i32)), cols, &mut current, &mut out);
            out.push_str("\x1b[0m\r\n");
            current = Style::default();
        }
        // Push the history off-screen: the last history line must scroll
        // past the top, i.e. (screen_lines - 1) more newlines.
        for _ in 0..t.screen_lines().saturating_sub(1) {
            out.push_str("\r\n");
        }
    } else {
        out.push_str("\x1b[?1049h");
    }

    // Visible screen + cursor position/visibility.
    out.push_str(&String::from_utf8_lossy(&t.contents_formatted()));
    write_modes(t, mode, &mut out);
    out.into_bytes()
}

/// Modes that change how the client must encode input, plus the title.
fn write_modes(t: &Term, mode: TermMode, out: &mut String) {
    let set = |out: &mut String, on: bool, n: u16| {
        let _ = write!(out, "\x1b[?{n}{}", if on { 'h' } else { 'l' });
    };
    set(out, mode.contains(TermMode::APP_CURSOR), 1);
    set(out, mode.contains(TermMode::LINE_WRAP), 7);
    set(out, mode.contains(TermMode::BRACKETED_PASTE), 2004);
    set(out, mode.contains(TermMode::FOCUS_IN_OUT), 1004);
    if mode.contains(TermMode::MOUSE_REPORT_X10) {
        set(out, true, 9);
    }
    if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        set(out, true, 1000);
    }
    if mode.contains(TermMode::MOUSE_DRAG) {
        set(out, true, 1002);
    }
    if mode.contains(TermMode::MOUSE_MOTION) {
        set(out, true, 1003);
    }
    if mode.contains(TermMode::UTF8_MOUSE) {
        set(out, true, 1005);
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        set(out, true, 1006);
    }
    out.push_str(if mode.contains(TermMode::APP_KEYPAD) { "\x1b=" } else { "\x1b>" });
    if !t.title.is_empty() {
        let _ = write!(out, "\x1b]2;{}\x07", t.title.replace(['\x07', '\x1b'], ""));
    }
}

fn write_row(t: &Term, line: Line, cols: usize, current: &mut Style, out: &mut String) {
    let row = &t.grid[line];
    // Trim trailing blanks with default style.
    let last = (0..cols).rev().find(|&c| {
        let sq = row[Column(c)];
        let ch = sq.c();
        (ch != ' ' && ch != '\0') || t.grid.style_of(&sq).renders_on_blank()
    });
    let Some(last) = last else { return };
    let mut c = 0;
    while c <= last {
        let sq = row[Column(c)];
        if sq.is_spacer() || sq.is_leading_spacer() {
            c += 1;
            continue;
        }
        let style = t.grid.style_of(&sq);
        if style != *current {
            write_sgr(out, &style);
            *current = style;
        }
        let pos = rio_vt::crosswords::pos::Pos::new(line, Column(c));
        for (i, ch) in t.grid.cell_text(pos).enumerate() {
            out.push(if i == 0 && ch == '\0' { ' ' } else { ch });
        }
        c += if sq.is_wide() { 2 } else { 1 };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{Palette, TermSize, Terminal};

    fn text_of(t: &Terminal) -> Vec<String> {
        let mut f = crate::Frame::default();
        t.snapshot_into(&mut f);
        (0..f.rows).map(|r| f.row(r).iter().map(|c| c.ch).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn roundtrip_screen_and_history() {
        let size = TermSize::new(20, 5);
        let (a, _ca) = Terminal::new(size, true, Arc::new(Palette::default()));
        for i in 0..12 {
            a.feed(format!("line {i}\r\n").as_bytes());
        }
        a.feed(b"\x1b[31mred\x1b[0m $ ");
        let snap = a.reattach_snapshot();

        let (b, _cb) = Terminal::new(size, false, Arc::new(Palette::default()));
        b.feed(&snap);
        assert_eq!(text_of(&a), text_of(&b));
        let ha = a.with_term(|t| t.history_size());
        let hb = b.with_term(|t| t.history_size());
        assert_eq!(ha, hb, "history should be replayed");
    }

    #[test]
    fn roundtrip_alt_screen_and_modes() {
        let size = TermSize::new(20, 5);
        let (a, _ca) = Terminal::new(size, true, Arc::new(Palette::default()));
        a.feed(b"shell$ vim\r\n\x1b[?1049h\x1b[?1h\x1b[?2004h\x1b[Hvim buffer");
        let (b, _cb) = Terminal::new(size, false, Arc::new(Palette::default()));
        b.feed(&a.reattach_snapshot());
        assert_eq!(text_of(&a), text_of(&b));
        let relevant = crate::TermMode::ALT_SCREEN
            | crate::TermMode::APP_CURSOR
            | crate::TermMode::BRACKETED_PASTE
            | crate::TermMode::MOUSE_MODE
            | crate::TermMode::SGR_MOUSE;
        assert_eq!((a.mode() & relevant).bits(), (b.mode() & relevant).bits());
    }

    #[test]
    fn resync_catches_up_without_losing_history() {
        let size = TermSize::new(20, 5);
        let (a, _ca) = Terminal::new(size, true, Arc::new(Palette::default()));
        let (b, _cb) = Terminal::new(size, false, Arc::new(Palette::default()));
        // Both see the beginning...
        let start: Vec<u8> = (0..10).flat_map(|i| format!("early {i}\r\n").into_bytes()).collect();
        a.feed(&start);
        b.feed(&start);
        let hb_before = b.with_term(|t| t.history_size());
        // ...then the client misses a burst that also enters the alt screen
        // and turns on mouse reporting and app cursor keys.
        a.feed(b"lots of skipped output\r\n\x1b[?1049h\x1b[?1h\x1b[?1000h\x1b[?1006h\x1b[3;3Hin-app");
        b.feed(&a.resync_snapshot());
        assert_eq!(text_of(&a), text_of(&b));
        let relevant = crate::TermMode::ALT_SCREEN | crate::TermMode::APP_CURSOR | crate::TermMode::MOUSE_MODE | crate::TermMode::SGR_MOUSE;
        assert_eq!((a.mode() & relevant).bits(), (b.mode() & relevant).bits());

        // Leaving the app on the daemon side while the client is still
        // skipping must bring the client back to the primary screen too.
        a.feed(b"\x1b[?1049l\x1b[?1l\x1b[?1000lback at prompt");
        b.feed(&a.resync_snapshot());
        assert_eq!(text_of(&a), text_of(&b));
        assert_eq!((a.mode() & relevant).bits(), (b.mode() & relevant).bits());
        assert!(b.with_term(|t| t.history_size()) >= hb_before, "client history kept");
    }
}
