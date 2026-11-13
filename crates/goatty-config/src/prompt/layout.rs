//! What goes where in the prompt: the modules in order, the segment ends
//! and separators, frame, connection line and prompt character, as a tree
//! of [`Piece`]s. goatty-shell writes it as Starship format strings; the
//! settings window draws it with sample values. Keeping one description
//! keeps the preview honest.

use super::{Connection, Frame, MODULES, Module, PromptSettings, PromptStyle, Side};

/// Colors and weight, as Starship styles them (`bold fg:cyan bg:236`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: bool,
}

impl Style {
    pub fn fg(c: &str) -> Self {
        Self { fg: Some(c.to_owned()), ..Self::default() }
    }

    pub fn bg(c: &str) -> Self {
        Self { bg: Some(c.to_owned()), ..Self::default() }
    }

    pub fn on(fg: &str, bg: &str) -> Self {
        Self { fg: Some(fg.to_owned()), bg: Some(bg.to_owned()), bold: false }
    }

    pub fn bold(mut self, b: bool) -> Self {
        self.bold = b;
        self
    }

    /// Starship's style string ("" for none).
    pub fn starship(&self) -> String {
        let mut parts = Vec::new();
        if self.bold {
            parts.push("bold".to_owned());
        }
        if let Some(fg) = &self.fg {
            parts.push(format!("fg:{fg}"));
        }
        if let Some(bg) = &self.bg {
            parts.push(format!("bg:{bg}"));
        }
        parts.join(" ")
    }
}

/// Part of the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Literal text (segment ends, separators, frame).
    Text(String, Style),
    /// A module, drawn as its [`Look`] says.
    Module(&'static Module),
    /// Hidden when none of the modules in it shows.
    Group(Vec<Piece>),
    /// Stretches to push what follows to the right edge.
    Fill,
    LineBreak,
    /// The prompt character.
    Character,
}

impl Piece {
    fn text(s: &str, style: Style) -> Option<Self> {
        (!s.is_empty()).then(|| Self::Text(s.to_owned(), style))
    }
}

/// The whole prompt.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    /// Starship's `format`: every line, ending with the prompt character.
    pub left: Vec<Piece>,
    /// Starship's `right_format`: right-aligned on the input line.
    pub right: Vec<Piece>,
}

/// How a module draws itself: `{word }{lead}{content}{trail}`, `lead` and
/// `content` in `style`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub word: &'static str,
    pub lead: &'static str,
    pub trail: &'static str,
    pub style: Style,
}

impl PromptSettings {
    /// The background of `m`'s segment in Rainbow: its lead module's color.
    pub fn segment_color(&self, m: &Module) -> &str {
        super::module(m.lead()).map_or(m.color, |lead| self.module_color(lead))
    }

    pub fn look(&self, m: &Module) -> Look {
        let color = self.module_color(m);
        let dir = m.id == "directory";
        match self.style {
            PromptStyle::Rich => Look { word: m.word, lead: "", trail: " ", style: Style::fg(color).bold(m.style.contains("bold")) },
            PromptStyle::Lean => Look { word: "", lead: "", trail: " ", style: Style::fg(color).bold(dir) },
            PromptStyle::Classic => Look { word: "", lead: " ", trail: "", style: Style::on(color, &self.colors.segment).bold(dir) },
            PromptStyle::Rainbow => {
                Look { word: "", lead: " ", trail: "", style: Style::on(&self.colors.segment_text, self.segment_color(m)).bold(dir) }
            }
        }
    }

    pub fn layout(&self) -> Layout {
        let on: Vec<&'static Module> = MODULES.iter().filter(|m| self.module_on(m)).collect();
        let side = |s: Side| -> Vec<Vec<&'static Module>> {
            // Neighbours sharing a lead form one segment.
            let mut segs: Vec<Vec<&'static Module>> = Vec::new();
            for m in on.iter().copied().filter(|m| self.module_side(m) == s) {
                match segs.last_mut() {
                    Some(seg) if seg[0].lead() == m.lead() => seg.push(m),
                    _ => segs.push(vec![m]),
                }
            }
            segs
        };
        let left_side = self.side_pieces(&side(Side::Left), Side::Left);
        let right_side = self.side_pieces(&side(Side::Right), Side::Right);
        let segments = self.style.has_segments();
        let frame = Style::fg(&self.colors.frame);
        let (two, framed) = (self.two_line, self.two_line && self.frame != Frame::None);
        let full = two && self.frame == Frame::Full;

        let mut left = Vec::new();
        if framed {
            left.push(Piece::Text("╭─".into(), frame.clone()));
            if !segments {
                left.push(Piece::Text(" ".into(), Style::default()));
            }
        }
        left.extend(left_side);
        let mut right = Vec::new();
        if two {
            if !right_side.is_empty() || full {
                let line = self.connection != Connection::None;
                if line && segments {
                    left.push(Piece::Text(" ".into(), Style::default()));
                }
                left.push(Piece::Fill);
                if line {
                    left.push(Piece::Text(" ".into(), Style::default()));
                }
                left.extend(right_side);
                if full {
                    left.push(Piece::Text("─╮".into(), frame.clone()));
                }
            }
            left.push(Piece::LineBreak);
            if framed {
                left.push(Piece::Text("╰─".into(), frame.clone()));
            }
            left.push(Piece::Character);
            if full {
                right.push(Piece::Text("─╯".into(), frame));
            }
        } else {
            if segments {
                left.push(Piece::Text(" ".into(), Style::default()));
            }
            left.push(Piece::Character);
            right = right_side;
        }
        Layout { left, right }
    }

    /// One side's segments, with ends and separators for the style.
    fn side_pieces(&self, segs: &[Vec<&'static Module>], side: Side) -> Vec<Piece> {
        let modules = |seg: &[&'static Module]| seg.iter().map(|m| Piece::Module(m)).collect::<Vec<_>>();
        // A segment's left and right end: the left side points right, toward
        // the input; the right side points left.
        let (open, close) = match side {
            Side::Left => (self.tails.glyph(Side::Left), self.heads.glyph(Side::Right)),
            Side::Right => (self.heads.glyph(Side::Left), self.tails.glyph(Side::Right)),
        };
        match self.style {
            PromptStyle::Rich | PromptStyle::Lean => segs.iter().flat_map(|s| modules(s)).collect(),
            PromptStyle::Rainbow => segs
                .iter()
                .map(|seg| {
                    let bg = self.segment_color(seg[0]).to_owned();
                    let mut g: Vec<Piece> = Piece::text(open, Style::fg(&bg)).into_iter().collect();
                    g.extend(modules(seg));
                    g.push(Piece::Text(" ".into(), Style::bg(&bg)));
                    g.extend(Piece::text(close, Style::fg(&bg)));
                    Piece::Group(g)
                })
                .collect(),
            PromptStyle::Classic => {
                if segs.is_empty() {
                    return Vec::new();
                }
                let bg = &self.colors.segment;
                let sep = Piece::Text(format!(" {}", self.separator.glyph(side)), Style::on(&self.colors.separator, bg));
                // Separators go between segments that show: before the
                // anchor (a segment that always shows) they follow their
                // segment, after it they precede it.
                let anchor = segs.iter().position(|s| s.iter().any(|m| m.always));
                let mut inner = Vec::new();
                for (i, seg) in segs.iter().enumerate() {
                    match anchor {
                        Some(a) if i == a => inner.extend(modules(seg)),
                        Some(a) if i < a => {
                            let mut g = modules(seg);
                            g.push(sep.clone());
                            inner.push(Piece::Group(g));
                        }
                        None if i == 0 => inner.push(Piece::Group(modules(seg))),
                        _ => {
                            let mut g = vec![sep.clone()];
                            g.extend(modules(seg));
                            inner.push(Piece::Group(g));
                        }
                    }
                }
                let mut bar: Vec<Piece> = Piece::text(open, Style::fg(bg)).into_iter().collect();
                bar.extend(inner);
                bar.push(Piece::Text(" ".into(), Style::bg(bg)));
                bar.extend(Piece::text(close, Style::fg(bg)));
                vec![Piece::Group(bar)]
            }
        }
    }
}
