//! What a paste would do, before it reaches the shell.
//!
//! [`check`] lists the reasons a paste deserves a second look: it would run
//! commands right away, smuggles control or invisible characters, pipes a
//! download into a shell, or is very large. The UI decides what to ask
//! (see `goatty_config::PasteConfirm`); [`clean`] removes what can't be
//! seen. Encoding the paste itself is [`crate::input::encode_paste`].

use crate::TermMode;

/// Pastes larger than this are worth a look before they flood the shell.
pub const LARGE_PASTE: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteRisk {
    /// The application didn't turn on bracketed paste, so every newline
    /// runs what comes before it as soon as it arrives.
    RunsCommands {
        commands: usize,
    },
    /// More than one line (bracketed, so nothing runs by itself).
    Multiline {
        lines: usize,
    },
    /// Escape sequences or other control characters: they can move the
    /// cursor, rewrite what's on screen, or act as keystrokes.
    ControlChars {
        count: usize,
    },
    /// Characters that don't show: bidi overrides that reorder the text
    /// (Trojan Source), zero-width spaces, soft hyphens.
    HiddenChars {
        count: usize,
    },
    /// `curl … | sh` and friends: runs whatever a server sends.
    DownloadToShell,
    Large {
        bytes: usize,
    },
}

impl PasteRisk {
    /// Risks worth stopping for even when multiline pastes are fine.
    pub fn is_suspicious(&self) -> bool {
        !matches!(self, PasteRisk::Multiline { .. })
    }
}

/// Lines in `text`, not counting one trailing newline.
pub fn line_count(text: &str) -> usize {
    let t = text.strip_suffix('\n').unwrap_or(text);
    let t = t.strip_suffix('\r').unwrap_or(t);
    if t.is_empty() { usize::from(!text.is_empty()) } else { t.split('\n').count() }
}

/// Control characters other than tab and newlines (C0, DEL, C1).
pub fn is_control(c: char) -> bool {
    (c.is_control() && !matches!(c, '\t' | '\n' | '\r')) || ('\u{80}'..='\u{9f}').contains(&c)
}

/// Invisible characters that change how text reads or behaves. Joiners
/// (U+200C/D) are left alone: emoji and many scripts need them.
pub fn is_hidden(c: char) -> bool {
    matches!(c,
        '\u{200b}' | '\u{2060}' | '\u{feff}' | '\u{00ad}' | '\u{180e}'
        | '\u{200e}' | '\u{200f}' | '\u{061c}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}')
}

/// Everything about `text` that a paste confirmation should mention, most
/// serious first. Empty: nothing to worry about.
pub fn check(text: &str, mode: TermMode) -> Vec<PasteRisk> {
    let mut risks = Vec::new();
    let lines = line_count(text);
    let has_newline = text.contains(['\n', '\r']);
    if has_newline && !mode.contains(TermMode::BRACKETED_PASTE) {
        // Each line break submits a command; a final line without one waits.
        let commands = text.split(['\n', '\r']).count() - 1 - text.matches("\r\n").count();
        risks.push(PasteRisk::RunsCommands { commands: commands.max(1) });
    }
    if pipes_download_to_shell(text) {
        risks.push(PasteRisk::DownloadToShell);
    }
    let control = text.chars().filter(|&c| is_control(c)).count();
    if control > 0 {
        risks.push(PasteRisk::ControlChars { count: control });
    }
    let hidden = text.chars().filter(|&c| is_hidden(c)).count();
    if hidden > 0 {
        risks.push(PasteRisk::HiddenChars { count: hidden });
    }
    if text.len() > LARGE_PASTE {
        risks.push(PasteRisk::Large { bytes: text.len() });
    }
    if lines > 1 && mode.contains(TermMode::BRACKETED_PASTE) {
        risks.push(PasteRisk::Multiline { lines });
    }
    risks
}

/// `text` without control (except tab and newlines) and hidden characters.
pub fn clean(text: &str) -> String {
    text.chars().filter(|&c| !is_control(c) && !is_hidden(c)).collect()
}

/// `text` as one line: line breaks become spaces, blank lines and trailing
/// whitespace go, and nothing runs until the user presses Enter.
pub fn join_lines(text: &str) -> String {
    text.split(['\n', '\r']).map(str::trim_end).filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join(" ")
}

const FETCHERS: &[&str] = &["curl", "wget", "fetch", "iwr", "irm", "invoke-webrequest", "invoke-restmethod"];
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "python", "python3", "perl", "ruby", "node", "iex", "pwsh"];

/// Some segment runs a fetcher and a later one in the same pipeline is a
/// shell or interpreter (optionally behind sudo/env).
fn pipes_download_to_shell(text: &str) -> bool {
    let word = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_ascii_lowercase();
    for line in text.lines() {
        let mut fetched = false;
        for (i, segment) in line.split('|').enumerate() {
            let mut words = segment.split_whitespace().map(word).filter(|w| !w.is_empty());
            let mut first = words.next();
            while matches!(first.as_deref(), Some("sudo" | "env" | "doas" | "exec" | "command")) {
                first = words.next();
                while first.as_deref().is_some_and(|w| w.starts_with('-') || w.contains('=')) {
                    first = words.next();
                }
            }
            let Some(cmd) = first else { continue };
            let cmd = cmd.rsplit('/').next().unwrap_or(&cmd).to_string();
            if i > 0 && fetched && SHELLS.contains(&cmd.as_str()) {
                return true;
            }
            fetched |= FETCHERS.contains(&cmd.as_str());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRACKETED: TermMode = TermMode::BRACKETED_PASTE;

    #[test]
    fn single_line_bracketed_paste_is_fine() {
        assert!(check("ls -la", BRACKETED).is_empty());
        assert!(check("ls -la", TermMode::empty()).is_empty());
        assert!(check("echo 日本 👩‍👩‍👧", BRACKETED).is_empty(), "joiners in emoji are fine");
    }

    #[test]
    fn newlines_without_bracketed_paste_run_commands() {
        assert_eq!(check("ls\n", TermMode::empty()), [PasteRisk::RunsCommands { commands: 1 }]);
        assert_eq!(check("a\nb\nc", TermMode::empty()), [PasteRisk::RunsCommands { commands: 2 }]);
        assert_eq!(check("a\r\nb\r\n", TermMode::empty()), [PasteRisk::RunsCommands { commands: 2 }]);
        // Bracketed: only multiline, which isn't suspicious.
        let r = check("a\nb\n", BRACKETED);
        assert_eq!(r, [PasteRisk::Multiline { lines: 2 }]);
        assert!(!r[0].is_suspicious());
        assert!(check("one line\n", BRACKETED).is_empty(), "a trailing newline is still one line");
    }

    #[test]
    fn control_and_hidden_characters() {
        assert_eq!(check("echo hi\x1b[201~; rm -rf ~", BRACKETED), [PasteRisk::ControlChars { count: 1 }]);
        assert_eq!(check("ls \u{202e}txt.exe", BRACKETED), [PasteRisk::HiddenChars { count: 1 }]);
        assert_eq!(check("a\u{200b}b\u{9b}", BRACKETED), [PasteRisk::ControlChars { count: 1 }, PasteRisk::HiddenChars { count: 1 }]);
        assert_eq!(clean("a\u{200b}b\x1b[31mc\td\n\u{200d}"), "ab[31mc\td\n\u{200d}");
    }

    #[test]
    fn downloads_piped_into_shells() {
        for s in [
            "curl -fsSL https://x.sh | sh",
            "curl https://x | sudo bash",
            "wget -qO- https://x | sudo -E env FOO=1 bash -s",
            "curl -s https://x | tee log | /bin/zsh",
            "iwr https://x | iex",
        ] {
            assert!(check(s, BRACKETED).contains(&PasteRisk::DownloadToShell), "{s}");
        }
        for s in ["curl https://x | jq .", "echo curl | sh", "ls | bash", "curl -o x.sh https://x"] {
            assert!(!check(s, BRACKETED).contains(&PasteRisk::DownloadToShell), "{s}");
        }
    }

    #[test]
    fn large_pastes() {
        let big = "x".repeat(LARGE_PASTE + 1);
        assert_eq!(check(&big, BRACKETED), [PasteRisk::Large { bytes: LARGE_PASTE + 1 }]);
    }

    #[test]
    fn helpers() {
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\n"), 1);
        assert_eq!(line_count("a\nb"), 2);
        assert_eq!(line_count("\n"), 1);
        assert_eq!(join_lines("cd x &&  \n\n  make\r\n"), "cd x &&   make");
    }
}
