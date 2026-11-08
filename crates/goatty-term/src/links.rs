//! Clickable things in the grid: OSC 8 hyperlinks, URLs and file paths.
//!
//! Detection works on one *logical* line (a row plus the rows it wraps
//! into), only when asked for the cell under the mouse, so it costs nothing
//! while nobody hovers. [`find_links`] is the pure text part; the
//! [`Terminal::link_at`] wrapper maps grid cells to text and back.
//!
//! Paths are only *candidates* here: whether one exists (and on which host)
//! is for the caller to decide.

use std::ops::Range;
use std::path::PathBuf;

use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::square::Hyperlink;

use crate::Terminal;
use crate::terminal::Term;

/// Rows followed in each direction when collecting a wrapped line. Longer
/// logical lines are cut there (a URL wider than this many rows is rare).
const MAX_WRAPPED_ROWS: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// OSC 8: the application chose the target; the text may say anything.
    Hyperlink,
    /// A URL written out in the text.
    Url,
    /// Something that looks like a file path (not checked).
    Path,
}

/// One stretch of a link on one row of the viewport: `cols` is end-exclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    pub row: usize,
    pub cols: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub kind: LinkKind,
    /// The URI (hyperlinks, URLs) or the path as written, without a
    /// `:line:column` suffix.
    pub target: String,
    /// `file.rs:12:5` → 12 and 5.
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// Where the link is on screen (visible parts only).
    pub spans: Vec<LinkSpan>,
}

/// A link found in a piece of text. `range` is in chars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub kind: LinkKind,
    pub range: Range<usize>,
    pub target: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

const SCHEMES: &[&str] = &["https://", "http://", "ftp://", "file://", "ssh://", "sftp://", "git://", "mailto:"];

/// Box drawing and block elements end a link: TUIs draw borders right next
/// to text.
fn is_frame(c: char) -> bool {
    ('\u{2500}'..='\u{259f}').contains(&c)
}

fn url_char(c: char) -> bool {
    !(c.is_whitespace() || c.is_control() || is_frame(c) || matches!(c, '<' | '>' | '"' | '\'' | '`' | '\\' | '^' | '{' | '}' | '|'))
}

fn path_delim(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || is_frame(c)
        || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | ';' | ',' | '=' | '\0')
}

/// Every URL and path candidate in `text`, in order, not overlapping.
pub fn find_links(text: &[char]) -> Vec<Found> {
    let mut out = urls(text);
    let taken: Vec<Range<usize>> = out.iter().map(|f| f.range.clone()).collect();
    out.extend(paths(text).into_iter().filter(|p| !taken.iter().any(|u| u.start < p.range.end && p.range.start < u.end)));
    out.sort_by_key(|f| f.range.start);
    out
}

fn starts_with_ci(text: &[char], at: usize, pat: &str) -> bool {
    let mut i = at;
    for p in pat.chars() {
        match text.get(i) {
            Some(c) if c.eq_ignore_ascii_case(&p) => i += 1,
            _ => return false,
        }
    }
    true
}

fn urls(text: &[char]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let preceded_by_word = i > 0 && (text[i - 1].is_alphanumeric() || text[i - 1] == '+' || text[i - 1] == '-');
        let Some(scheme) = SCHEMES.iter().find(|s| !preceded_by_word && starts_with_ci(text, i, s)) else {
            i += 1;
            continue;
        };
        let body = i + scheme.chars().count();
        let mut end = body;
        while end < text.len() && url_char(text[end]) {
            end += 1;
        }
        end = trim_url_end(text, i, end);
        if end > body {
            out.push(Found { kind: LinkKind::Url, range: i..end, target: text[i..end].iter().collect(), line: None, column: None });
            i = end;
        } else {
            i = body;
        }
    }
    out
}

/// Drop trailing punctuation that belongs to the sentence, and closing
/// brackets without an opening one inside the URL (`(see https://x.org)`).
fn trim_url_end(text: &[char], start: usize, mut end: usize) -> usize {
    loop {
        let Some(&last) = text[start..end].last() else { return end };
        let unbalanced = |open: char, close: char| {
            let s = &text[start..end];
            s.iter().filter(|&&c| c == close).count() > s.iter().filter(|&&c| c == open).count()
        };
        let drop = match last {
            '.' | ',' | ':' | ';' | '!' | '?' | '*' => true,
            ')' => unbalanced('(', ')'),
            ']' => unbalanced('[', ']'),
            _ => false,
        };
        if !drop {
            return end;
        }
        end -= 1;
    }
}

fn paths(text: &[char]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if path_delim(text[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < text.len() && !path_delim(text[i]) {
            i += 1;
        }
        if let Some(f) = path_token(&text[start..i], start) {
            out.push(f);
        }
    }
    out
}

/// Split `src/main.rs:12:5:` into the path and its position.
fn split_position(token: &str) -> (&str, Option<u32>, Option<u32>) {
    let token = token.trim_end_matches(':');
    let mut parts = token.rsplitn(3, ':');
    let last = parts.next().unwrap_or_default();
    let mid = parts.next();
    let rest = parts.next();
    let num = |s: &str| (!s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u32>().ok()).flatten();
    match (rest, mid.and_then(num), num(last)) {
        (Some(p), Some(l), Some(c)) => (p, Some(l), Some(c)),
        (_, _, Some(l)) if mid.is_some() => (&token[..token.len() - last.len() - 1], Some(l), None),
        _ => (token, None, None),
    }
}

fn path_token(token: &[char], offset: usize) -> Option<Found> {
    // Sentence punctuation after a path ("see src/lib.rs.") isn't part of it.
    let mut len = token.len();
    while len > 0 && matches!(token[len - 1], '.' | ',' | '!' | '?') {
        len -= 1;
    }
    let s: String = token[..len].iter().collect();
    if s.contains("://") {
        return None;
    }
    let (path, line, column) = split_position(&s);
    // A trailing ':' that wasn't a position is punctuation ("error in foo.rs:").
    let path = path.trim_end_matches(':');
    if !looks_like_path(path) {
        return None;
    }
    let shown = s.trim_end_matches(':').chars().count();
    Some(Found { kind: LinkKind::Path, range: offset..offset + shown, target: path.to_string(), line, column })
}

fn looks_like_path(p: &str) -> bool {
    if p.is_empty() || p.chars().all(|c| c == '/' || c == '.' || c == '~') {
        return false;
    }
    if p.starts_with('/') || p.starts_with("~/") || p.starts_with("./") || p.starts_with("../") {
        return true;
    }
    if p.starts_with('-') || p.starts_with('$') || p.contains("//") {
        return false;
    }
    if p.contains('/') {
        // `src/main.rs`, `docs/`; not `1/2` or `and/or`-style prose.
        return p.chars().any(|c| c.is_ascii_alphabetic()) && (p.contains('.') || p.ends_with('/') || p.matches('/').count() >= 2);
    }
    // A bare file name needs an extension: `Cargo.toml`, `notes.md`.
    match p.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= 10
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
                && stem.chars().any(|c| c.is_alphabetic())
        }
        None => false,
    }
}

fn hyperlink_of(t: &Term, pos: Pos) -> Option<Hyperlink> {
    let sq = t.grid[pos];
    if !sq.has_hyperlink() {
        return None;
    }
    sq.extras_id_checked().and_then(|id| t.grid.extras_table.get(id)).and_then(|e| e.hyperlink.clone())
}

impl Terminal {
    /// The link under a *viewport* cell, if any. OSC 8 hyperlinks win over
    /// what the text looks like.
    pub fn link_at(&self, col: usize, row: usize) -> Option<Link> {
        let t = self.inner.term.lock();
        let cols = t.columns();
        let rows = t.screen_lines();
        if col >= cols || row >= rows {
            return None;
        }
        let offset = t.display_offset() as i32;
        let line = Line(row as i32 - offset);
        let top = Line(-(t.history_size() as i32));
        let bottom = Line(rows as i32 - 1);
        let wraps = |l: Line| t.grid[l][Column(cols - 1)].wrapline();

        // The logical line: up while the row above wraps into this one,
        // down while this one wraps.
        let mut first = line;
        while first > top && line.0 - first.0 < MAX_WRAPPED_ROWS && wraps(Line(first.0 - 1)) {
            first = Line(first.0 - 1);
        }
        let mut last = line;
        while last < bottom && last.0 - line.0 < MAX_WRAPPED_ROWS && wraps(last) {
            last = Line(last.0 + 1);
        }

        // Text with each char's cell; wide-char spacers have no char.
        let mut text = Vec::with_capacity(cols * (last.0 - first.0 + 1) as usize);
        let mut cells: Vec<Pos> = Vec::with_capacity(text.capacity());
        let mut hovered = None;
        for l in first.0..=last.0 {
            for c in 0..cols {
                let pos = Pos::new(Line(l), Column(c));
                let sq = t.grid[pos];
                if sq.is_spacer() || sq.is_leading_spacer() {
                    if pos == Pos::new(line, Column(col)) {
                        hovered = text.len().checked_sub(1);
                    }
                    continue;
                }
                if pos == Pos::new(line, Column(col)) {
                    hovered = Some(text.len());
                }
                let ch = sq.c();
                text.push(if ch == '\0' { ' ' } else { ch });
                cells.push(pos);
            }
        }
        let hovered = hovered?;

        let to_spans = |range: Range<usize>| -> Vec<LinkSpan> {
            let mut spans: Vec<LinkSpan> = Vec::new();
            for &pos in &cells[range] {
                let vrow = pos.row.0 + offset;
                if vrow < 0 || vrow as usize >= rows {
                    continue;
                }
                let (vrow, c) = (vrow as usize, pos.col.0);
                let w = if t.grid[pos].is_wide() { 2 } else { 1 };
                match spans.last_mut() {
                    Some(s) if s.row == vrow && s.cols.end == c => s.cols.end = c + w,
                    _ => spans.push(LinkSpan { row: vrow, cols: c..c + w }),
                }
            }
            spans
        };

        if let Some(link) = hyperlink_of(&t, cells[hovered]) {
            let same = |i: usize| hyperlink_of(&t, cells[i]).is_some_and(|h| h == link);
            let mut start = hovered;
            while start > 0 && same(start - 1) {
                start -= 1;
            }
            let mut end = hovered + 1;
            while end < cells.len() && same(end) {
                end += 1;
            }
            return Some(Link {
                kind: LinkKind::Hyperlink,
                target: link.uri().to_string(),
                line: None,
                column: None,
                spans: to_spans(start..end),
            });
        }

        let found = find_links(&text).into_iter().find(|f| f.range.contains(&hovered))?;
        Some(Link { kind: found.kind, target: found.target, line: found.line, column: found.column, spans: to_spans(found.range) })
    }

    /// The working directory the shell last reported (OSC 7), if any.
    /// Reports naming another host (a remote session, `ssh` inside the
    /// shell) are ignored, so this is always a path on this machine.
    pub fn current_directory(&self) -> Option<PathBuf> {
        self.inner.term.lock().current_directory.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Palette, TermSize};

    fn found(s: &str) -> Vec<(LinkKind, String, Option<u32>, Option<u32>)> {
        let chars: Vec<char> = s.chars().collect();
        find_links(&chars).into_iter().map(|f| (f.kind, f.target, f.line, f.column)).collect()
    }

    fn targets(s: &str) -> Vec<String> {
        found(s).into_iter().map(|f| f.1).collect()
    }

    #[test]
    fn urls_trim_sentence_punctuation_and_unbalanced_brackets() {
        assert_eq!(targets("see https://example.org/a?b=1&c=2."), ["https://example.org/a?b=1&c=2"]);
        assert_eq!(
            targets("(https://en.wikipedia.org/wiki/Rust_(programming_language))"),
            ["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
        assert_eq!(targets("[link](https://x.org/y)"), ["https://x.org/y"]);
        assert_eq!(targets("'https://x.org' \"http://y.org\""), ["https://x.org", "http://y.org"]);
        assert_eq!(targets("<https://x.org/>"), ["https://x.org/"]);
        assert_eq!(targets("mailto:me@example.org, ftp://h/f"), ["mailto:me@example.org", "ftp://h/f"]);
        assert_eq!(targets("HTTPS://X.ORG"), ["HTTPS://X.ORG"]);
        assert!(targets("https:// nothing").is_empty());
        assert!(targets("xhttps://nope.org").iter().all(|t| !t.starts_with("http")), "scheme glued to a word");
        assert_eq!(targets("│https://x.org│"), ["https://x.org"], "TUI borders end a link");
    }

    #[test]
    fn paths_with_positions() {
        use LinkKind::Path;
        assert_eq!(found("error: --> src/main.rs:12:5"), [(Path, "src/main.rs".into(), Some(12), Some(5))]);
        assert_eq!(found("src/lib.rs:7: warning"), [(Path, "src/lib.rs".into(), Some(7), None)]);
        assert_eq!(found("at /etc/hosts."), [(Path, "/etc/hosts".into(), None, None)]);
        assert_eq!(found("cd ~/coding/rust"), [(Path, "~/coding/rust".into(), None, None)]);
        assert_eq!(found("./run.sh ../x"), [(Path, "./run.sh".into(), None, None), (Path, "../x".into(), None, None)]);
        assert_eq!(found("Cargo.toml  README.md"), [(Path, "Cargo.toml".into(), None, None), (Path, "README.md".into(), None, None)]);
        assert_eq!(found("(crates/goatty-term/src/lib.rs)"), [(Path, "crates/goatty-term/src/lib.rs".into(), None, None)]);
        assert_eq!(found("--file=/tmp/x.log"), [(Path, "/tmp/x.log".into(), None, None)]);
        assert_eq!(found("error in foo.rs:"), [(Path, "foo.rs".into(), None, None)]);
    }

    #[test]
    fn things_that_are_not_paths() {
        for s in ["1/2", "and/or", "v1.2.3", "3.14", "...", "/", "-o", "$HOME/x", "a//b", "10:30"] {
            assert!(found(s).is_empty(), "{s:?} → {:?}", found(s));
        }
        // A URL isn't also a path.
        assert_eq!(found("https://x.org/a/b.html"), [(LinkKind::Url, "https://x.org/a/b.html".into(), None, None)]);
    }

    fn term(cols: u16, rows: u16) -> Terminal {
        Terminal::new(TermSize::new(cols, rows), false, Arc::new(Palette::default())).0
    }

    #[test]
    fn link_at_follows_wrapped_rows() {
        let t = term(10, 4);
        t.feed(b"go https://example.org/abc ok");
        // Row 0 "go https:/", row 1 "/example.o", row 2 "rg/abc ok".
        let link = t.link_at(2, 1).expect("link on the wrapped row");
        assert_eq!(link.kind, LinkKind::Url);
        assert_eq!(link.target, "https://example.org/abc");
        assert_eq!(link.spans, [LinkSpan { row: 0, cols: 3..10 }, LinkSpan { row: 1, cols: 0..10 }, LinkSpan { row: 2, cols: 0..6 }]);
        assert_eq!(t.link_at(0, 0), None, "plain text");
        assert_eq!(t.link_at(8, 2), None);
    }

    #[test]
    fn osc8_hyperlinks_cover_their_cells() {
        let t = term(40, 3);
        t.feed(b"see \x1b]8;;https://goatty.dev/docs\x1b\\the docs\x1b]8;;\x1b\\ now");
        let link = t.link_at(6, 0).expect("hyperlink");
        assert_eq!(link.kind, LinkKind::Hyperlink);
        assert_eq!(link.target, "https://goatty.dev/docs");
        assert_eq!(link.spans, [LinkSpan { row: 0, cols: 4..12 }]);
        assert_eq!(t.link_at(13, 0), None);
    }

    #[test]
    fn wide_chars_keep_columns_right() {
        let t = term(30, 2);
        t.feed("日本 src/main.rs:3".as_bytes());
        let link = t.link_at(8, 0).expect("path after wide chars");
        assert_eq!(link.target, "src/main.rs");
        assert_eq!(link.line, Some(3));
        assert_eq!(link.spans, [LinkSpan { row: 0, cols: 5..18 }]);
    }

    #[test]
    fn cwd_from_osc7() {
        let t = term(10, 2);
        assert_eq!(t.current_directory(), None);
        t.feed(b"\x1b]7;file://localhost/tmp/some%20dir\x1b\\");
        assert_eq!(t.current_directory(), Some(PathBuf::from("/tmp/some dir")));
        t.feed(b"\x1b]7;file://elsewhere.example/srv\x1b\\");
        assert_eq!(t.current_directory(), Some(PathBuf::from("/tmp/some dir")), "another host's directory");
    }
}
