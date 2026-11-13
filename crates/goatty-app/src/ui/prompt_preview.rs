//! Draws the prompt the way the prompt settings make Starship draw it, with
//! sample values (a repository `goatty`, a Rust crate, a command that took
//! 4 s and failed). It walks the same [`Layout`] goatty-shell turns into
//! `starship.toml`, so the preview and the shell agree on what goes where.

use goatty_config::ThemeColors;
use goatty_config::prompt::{Connection, Layout, Module, Piece, PromptColor, PromptSettings, PromptStyle, Style, xterm_rgb};
use gpui::{AnyElement, Div, FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px, rgb};

/// A color from the settings, in the theme's palette.
pub fn resolve(color: &str, t: &ThemeColors) -> Option<Hsla> {
    let packed = match PromptColor::parse(color)? {
        PromptColor::Ansi(i) => t.ansi[usize::from(i)].0,
        PromptColor::Indexed(i) => {
            let (r, g, b) = xterm_rgb(i);
            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
        }
        PromptColor::Rgb(r, g, b) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
    };
    Some(rgb(packed).into())
}

/// What happened before the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// A new shell: no duration, no status.
    Fresh,
    /// A command that ran 4 s and failed with 101.
    Failed,
}

/// One drawn piece of a line.
enum Item {
    Span(String, Style),
    Fill,
}

/// Draws prompts for one set of settings.
pub struct Preview<'a> {
    pub p: &'a PromptSettings,
    pub theme: &'a ThemeColors,
    /// Starship is on (else goatty's small built-in prompt).
    pub starship: bool,
    pub line_height: f32,
}

impl Preview<'_> {
    fn hsla(&self, c: &Option<String>, fallback: Hsla) -> Hsla {
        c.as_deref().and_then(|c| resolve(c, self.theme)).unwrap_or(fallback)
    }

    fn fg(&self) -> Hsla {
        rgb(self.theme.foreground.0).into()
    }

    fn span(&self, text: String, style: &Style) -> Div {
        div()
            .flex_shrink_0()
            .text_color(self.hsla(&style.fg, self.fg()))
            .when_some(style.bg.as_deref().and_then(|c| resolve(c, self.theme)), |d, bg| d.bg(bg))
            .when(style.bold, |d| d.font_weight(FontWeight::BOLD))
            .child(SharedString::from(text))
    }

    /// The prompt's lines; the last one ends with `input` (what's typed,
    /// the cursor) and the right prompt.
    pub fn prompt(&self, moment: Moment, input: impl IntoElement) -> Vec<Div> {
        let failed = moment == Moment::Failed;
        if !self.starship {
            return self.builtin(failed, input);
        }
        let Layout { left, right } = self.p.layout();
        let mut lines: Vec<Vec<Item>> = vec![Vec::new()];
        self.walk(&left, moment, &mut lines);
        let mut right_items = vec![Vec::new()];
        self.walk(&right, moment, &mut right_items);
        let n = lines.len();
        let mut input = Some(input.into_any_element());
        lines
            .into_iter()
            .enumerate()
            .map(|(i, items)| {
                let mut line = self.line(items);
                if i + 1 == n {
                    line = line.children(input.take());
                    let r: Vec<Item> = right_items.drain(..).flatten().collect();
                    if !r.is_empty() {
                        line = line.child(div().flex_1().min_w(px(8.))).children(r.into_iter().map(|it| self.item(it)));
                    }
                }
                line
            })
            .collect()
    }

    /// An accepted command's prompt when the transient prompt is on: just
    /// the prompt character.
    pub fn transient(&self, failed: bool, input: impl IntoElement) -> Div {
        let mut items = Vec::new();
        self.character(failed, &mut items);
        self.line(items).child(input)
    }

    fn line(&self, items: Vec<Item>) -> Div {
        div()
            .flex()
            .items_center()
            .h(px(self.line_height))
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .children(items.into_iter().map(|it| self.item(it)))
    }

    fn item(&self, it: Item) -> AnyElement {
        match it {
            Item::Span(text, style) => self.span(text, &style).into_any_element(),
            Item::Fill => {
                let c = &self.p.connection;
                let fill = div().flex_1().min_w(px(8.)).overflow_hidden();
                if *c == Connection::None {
                    fill.into_any_element()
                } else {
                    fill.text_color(self.hsla(&Some(self.p.colors.frame.clone()), self.fg()))
                        .child(SharedString::from(c.symbol().repeat(240)))
                        .into_any_element()
                }
            }
        }
    }

    fn walk(&self, pieces: &[Piece], moment: Moment, lines: &mut Vec<Vec<Item>>) {
        for piece in pieces {
            match piece {
                Piece::Text(t, style) => push(lines, Item::Span(t.clone(), style.clone())),
                Piece::Module(m) => {
                    if let Some(items) = self.module(m, moment) {
                        for it in items {
                            push(lines, it);
                        }
                    }
                }
                Piece::Group(g) => {
                    if self.shows(g, moment) {
                        self.walk(g, moment, lines);
                    }
                }
                Piece::Fill => push(lines, Item::Fill),
                Piece::LineBreak => lines.push(Vec::new()),
                Piece::Character => {
                    let mut items = Vec::new();
                    self.character(moment == Moment::Failed, &mut items);
                    for it in items {
                        push(lines, it);
                    }
                }
            }
        }
    }

    /// A group shows when a module in it does.
    fn shows(&self, pieces: &[Piece], moment: Moment) -> bool {
        pieces.iter().any(|p| match p {
            Piece::Module(m) => self.sample(m, moment).is_some(),
            Piece::Group(g) => self.shows(g, moment),
            _ => false,
        })
    }

    fn sample(&self, m: &Module, moment: Moment) -> Option<String> {
        let after = moment == Moment::Failed;
        match m.id {
            "cmd_duration" => (after && self.p.duration_threshold_ms <= 4_000).then(|| "4s".into()),
            "status" => after.then(|| "101".into()),
            "git_status" if self.p.style == PromptStyle::Rich => Some("[!?]".into()),
            _ => m.sample.map(str::to_owned),
        }
    }

    fn module(&self, m: &Module, moment: Moment) -> Option<Vec<Item>> {
        let sample = self.sample(m, moment)?;
        let look = self.p.look(m);
        let mut items = Vec::new();
        if !look.word.is_empty() {
            items.push(Item::Span(format!("{} ", look.word), Style::default()));
        }
        if m.id == "directory" {
            let (before, root, after) = self.p.preview_directory();
            let base = if root.is_empty() { look.style.clone() } else { look.style.clone().bold(false) };
            items.push(Item::Span(format!("{}{before}", look.lead), base.clone()));
            if !root.is_empty() {
                items.push(Item::Span(root, look.style.clone().bold(true)));
                items.push(Item::Span(after, base));
            }
        } else {
            let symbol = if m.content.contains("$symbol") || m.content.contains("${symbol}") { self.p.module_symbol(m) } else { "" };
            items.push(Item::Span(format!("{}{symbol}{sample}", look.lead), look.style.clone()));
        }
        if !look.trail.is_empty() {
            items.push(Item::Span(look.trail.into(), Style::default()));
        }
        Some(items)
    }

    fn character(&self, failed: bool, items: &mut Vec<Item>) {
        let [ok, err, _] = self.p.character_symbols();
        let c = &self.p.character;
        let (sym, color) = if failed { (err, &c.error_color) } else { (ok, &c.success_color) };
        items.push(Item::Span(format!("{sym} "), Style::fg(color).bold(true)));
    }

    /// goatty's small prompt without Starship.
    fn builtin(&self, failed: bool, input: impl IntoElement) -> Vec<Div> {
        let dir = Item::Span("~/projects/goatty".into(), Style::fg("blue"));
        let mark = Item::Span("❯ ".into(), Style::fg(if failed { "red" } else { "green" }));
        if self.p.two_line {
            vec![self.line(vec![dir]), self.line(vec![mark]).child(input)]
        } else {
            vec![self.line(vec![dir, Item::Span(" ".into(), Style::default()), mark]).child(input)]
        }
    }
}

fn push(lines: &mut [Vec<Item>], it: Item) {
    if let Some(l) = lines.last_mut() {
        l.push(it);
    }
}
