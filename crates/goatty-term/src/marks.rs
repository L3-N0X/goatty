//! OSC 133 shell-integration marks, which rio-vt doesn't handle (it logs
//! them as unknown OSCs). [`MarkScanner`] watches the same bytes the parser
//! gets and reports the marks goatty uses. For now that is only "command
//! finished" (`OSC 133 ; D [; status]`), which tabs show as activity (#122).
//!
//! The scanner keeps its state between chunks, so a mark split across two
//! reads is still found. Outside escape sequences it only runs `memchr` for
//! ESC. Inside an OSC it jumps to the terminator and keeps just the first
//! few bytes, so a large OSC (an inline image, a clipboard write) costs
//! about as much as plain text.

use memchr::{memchr, memchr2};

/// A shell-integration mark found in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// `OSC 133 ; D`: the command ended, with its exit status if the shell
    /// reported one.
    CommandFinished(Option<i32>),
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// The longest OSC prefix we look at: `133;D;-2147483648` plus a bit.
const KEEP: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// After ESC.
    Esc,
    /// Inside `ESC ]`, collecting the payload.
    Osc,
    /// ESC inside an OSC: `\` ends it (ST), anything else aborts it.
    OscEsc,
}

#[derive(Debug)]
pub(crate) struct MarkScanner {
    state: State,
    /// The OSC payload's first [`KEEP`] bytes.
    buf: [u8; KEEP],
    len: usize,
}

impl Default for MarkScanner {
    fn default() -> Self {
        Self { state: State::Ground, buf: [0; KEEP], len: 0 }
    }
}

impl MarkScanner {
    /// Scan the next chunk of output and call `on` for each mark.
    pub(crate) fn scan(&mut self, bytes: &[u8], mut on: impl FnMut(Mark)) {
        let mut i = 0;
        while i < bytes.len() {
            match self.state {
                State::Ground => match memchr(ESC, &bytes[i..]) {
                    Some(p) => {
                        i += p + 1;
                        self.state = State::Esc;
                    }
                    None => return,
                },
                State::Esc => {
                    self.state = match bytes[i] {
                        b']' => {
                            self.len = 0;
                            State::Osc
                        }
                        ESC => State::Esc,
                        _ => State::Ground,
                    };
                    i += 1;
                }
                State::Osc => {
                    let rest = &bytes[i..];
                    let end = memchr2(BEL, ESC, rest).unwrap_or(rest.len());
                    let take = end.min(KEEP - self.len);
                    self.buf[self.len..self.len + take].copy_from_slice(&rest[..take]);
                    self.len += take;
                    i += end;
                    if let Some(&b) = bytes.get(i) {
                        i += 1;
                        if b == BEL {
                            self.finish(&mut on);
                        } else {
                            self.state = State::OscEsc;
                        }
                    }
                }
                State::OscEsc => {
                    if bytes[i] == b'\\' {
                        self.finish(&mut on);
                        i += 1;
                    } else {
                        // The OSC was cut off; this ESC starts something new.
                        self.state = State::Esc;
                    }
                }
            }
        }
    }

    fn finish(&mut self, on: &mut impl FnMut(Mark)) {
        self.state = State::Ground;
        if let Some(mark) = parse(&self.buf[..self.len]) {
            on(mark);
        }
    }
}

/// `133;D` or `133;D;<status>[;more…]`.
fn parse(payload: &[u8]) -> Option<Mark> {
    let rest = payload.strip_prefix(b"133;D")?;
    let status = match rest {
        [] => None,
        [b';', status @ ..] => {
            let status = status.split(|&b| b == b';').next().unwrap_or_default();
            std::str::from_utf8(status).ok().and_then(|s| s.parse().ok())
        }
        _ => return None,
    };
    Some(Mark::CommandFinished(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(chunks: &[&[u8]]) -> Vec<Mark> {
        let mut s = MarkScanner::default();
        let mut out = Vec::new();
        for c in chunks {
            s.scan(c, |m| out.push(m));
        }
        out
    }

    #[test]
    fn finds_command_finished_with_either_terminator() {
        assert_eq!(marks(&[b"out\x1b]133;D;0\x07\x1b]133;A\x07$ "]), [Mark::CommandFinished(Some(0))]);
        assert_eq!(marks(&[b"\x1b]133;D;127\x1b\\"]), [Mark::CommandFinished(Some(127))]);
        assert_eq!(marks(&[b"\x1b]133;D\x07"]), [Mark::CommandFinished(None)]);
        assert_eq!(marks(&[b"\x1b]133;D;1;aid=42\x07"]), [Mark::CommandFinished(Some(1))]);
        assert_eq!(marks(&[b"\x1b]133;D;oops\x07"]), [Mark::CommandFinished(None)]);
    }

    #[test]
    fn ignores_other_marks_and_oscs() {
        assert!(marks(&[b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07"]).is_empty());
        assert!(marks(&[b"\x1b]0;133;D\x07\x1b]1337;D\x07\x1b[31m133;D\x07"]).is_empty());
        assert!(marks(&[b"\x1b]133;Done\x07"]).is_empty());
    }

    #[test]
    fn finds_marks_split_across_chunks() {
        let stream: &[u8] = b"make\r\n\x1b]133;D;2\x1b\\\x1b]133;A\x07";
        for cut in 0..=stream.len() {
            let (a, b) = stream.split_at(cut);
            assert_eq!(marks(&[a, b]), [Mark::CommandFinished(Some(2))], "cut at {cut}");
        }
    }

    #[test]
    fn long_and_aborted_oscs_dont_swallow_the_next_mark() {
        let image = [b"\x1b]1337;File=inline=1:".as_slice(), &[b'A'; 100_000], b"\x07"].concat();
        assert_eq!(marks(&[&image, b"\x1b]133;D;0\x07"]), [Mark::CommandFinished(Some(0))]);
        // An OSC cut short by another escape sequence.
        assert_eq!(marks(&[b"\x1b]2;title\x1b[0m\x1b]133;D;3\x07"]), [Mark::CommandFinished(Some(3))]);
        assert_eq!(marks(&[b"\x1b\x1b]133;D;4\x07"]), [Mark::CommandFinished(Some(4))]);
    }

    #[test]
    fn terminal_reports_command_finished() {
        use std::sync::Arc;

        use crate::{Palette, TermSize, Terminal, UiEvent};
        let (t, ch) = Terminal::new(TermSize::new(40, 5), false, Arc::new(Palette::default()));
        t.feed(b"\x1b]133;C\x07building\r\n\x1b]133;D;");
        t.feed(b"1\x07\x1b]133;A\x07$ \x07");
        let events: Vec<_> = ch.ui.try_iter().collect();
        assert!(matches!(events.as_slice(), [UiEvent::CommandFinished { exit: Some(1) }, UiEvent::Bell]), "{events:?}");
    }
}
