//! Renderer-facing plain data: a [`Frame`] is a resolved copy of the visible
//! grid, filled once per frame by [`Terminal::snapshot_into`]. No rio-vt
//! types appear here, so renderers depend only on this crate's API.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::style::StyleFlags;

use crate::{Rgb, TermMode, Terminal};

impl Terminal {
    /// Copy everything a renderer needs for one frame into `frame`, reusing
    /// its allocations. Colors are resolved to RGB here so the renderer
    /// never needs rio-vt types.
    pub fn snapshot_into(&self, frame: &mut Frame) {
        let t = self.inner.term.lock();
        let cols = t.columns();
        let rows = t.screen_lines();
        let offset = t.display_offset() as i32;
        let colors = *t.colors();
        let pal = &*self.palette();
        let selection = t.selection.as_ref().and_then(|s| s.to_range(&t));
        let mode = t.mode();

        frame.cols = cols;
        frame.rows = rows;
        frame.display_offset = offset as usize;
        frame.history = t.history_size();
        frame.background = pal.resolve(rio_vt::config::colors::AnsiColor::Named(rio_vt::config::colors::NamedColor::Background), &colors);
        frame.cells.clear();
        frame.cells.reserve(cols * rows);
        frame.extra_text.clear();

        for r in 0..rows {
            let line = Line(r as i32 - offset);
            let row = &t.grid[line];
            for c in 0..cols {
                let sq = row[Column(c)];
                let style = t.grid.style_of(&sq);
                let mut fg = pal.resolve_fg(style.fg, style.flags, &colors);
                let mut bg = pal.resolve(style.bg, &colors);
                if style.flags.contains(StyleFlags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let pos = Pos::new(line, Column(c));
                let selected = selection.as_ref().is_some_and(|s| s.contains(pos));
                if selected {
                    fg = pal.selection_fg;
                    bg = pal.selection_bg;
                }
                let mut ch = sq.c();
                if ch == '\0' || style.flags.contains(StyleFlags::HIDDEN) {
                    ch = ' ';
                }
                let mut extra = 0u32;
                if sq.has_grapheme() {
                    let start = frame.extra_text.len();
                    frame.extra_text.extend(t.grid.cell_text(pos).skip(1));
                    let len = frame.extra_text.len() - start;
                    extra = ((start as u32) << 8) | (len.min(255) as u32);
                }
                let mut flags = CellFlags::empty();
                if sq.is_wide() {
                    flags |= CellFlags::WIDE;
                }
                if sq.is_spacer() || sq.is_leading_spacer() {
                    flags |= CellFlags::SPACER;
                }
                let sf = style.flags;
                if sf.contains(StyleFlags::BOLD) {
                    flags |= CellFlags::BOLD;
                }
                if sf.contains(StyleFlags::ITALIC) {
                    flags |= CellFlags::ITALIC;
                }
                if sf.intersects(StyleFlags::ALL_UNDERLINES) {
                    flags |= CellFlags::UNDERLINE;
                }
                if sf.contains(StyleFlags::UNDERCURL) {
                    flags |= CellFlags::UNDERCURL;
                }
                if sf.contains(StyleFlags::STRIKEOUT) {
                    flags |= CellFlags::STRIKE;
                }
                if sf.contains(StyleFlags::DIM) {
                    flags |= CellFlags::DIM;
                }
                frame.cells.push(Cell { ch, fg, bg, flags, extra });
            }
        }

        let cur = t.cursor();
        frame.cursor = if matches!(cur.content, CursorShape::Hidden) {
            None
        } else {
            let row = cur.pos.row.0 + offset;
            (row >= 0 && (row as usize) < rows).then(|| Cursor {
                col: cur.pos.col.0,
                row: row as usize,
                shape: match cur.content {
                    CursorShape::Underline => CursorKind::Underline,
                    CursorShape::Beam => CursorKind::Beam,
                    _ => CursorKind::Block,
                },
                wide: t.grid[cur.pos].is_wide(),
            })
        };
        frame.cursor_color = pal.cursor;
        frame.mode = mode;
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellFlags(u16);

impl CellFlags {
    pub const WIDE: Self = Self(1);
    pub const SPACER: Self = Self(1 << 1);
    pub const BOLD: Self = Self(1 << 2);
    pub const ITALIC: Self = Self(1 << 3);
    pub const UNDERLINE: Self = Self(1 << 4);
    pub const UNDERCURL: Self = Self(1 << 5);
    pub const STRIKE: Self = Self(1 << 6);
    pub const DIM: Self = Self(1 << 7);
    pub const fn empty() -> Self {
        Self(0)
    }
    pub fn contains(self, o: Self) -> bool {
        self.0 & o.0 == o.0
    }
}

impl std::ops::BitOrAssign for CellFlags {
    fn bitor_assign(&mut self, o: Self) {
        self.0 |= o.0
    }
}

/// One resolved cell. 16 bytes; a 300x100 frame is ~480 KB.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: CellFlags,
    /// Packed (offset << 8 | len) into `Frame::extra_text` for combining marks.
    pub extra: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Block,
    Underline,
    Beam,
}

#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub shape: CursorKind,
    pub wide: bool,
}

pub struct Frame {
    pub cols: usize,
    pub rows: usize,
    pub display_offset: usize,
    pub history: usize,
    pub cells: Vec<Cell>,
    pub extra_text: Vec<char>,
    pub cursor: Option<Cursor>,
    pub cursor_color: Rgb,
    pub background: Rgb,
    pub mode: TermMode,
}

impl Frame {
    pub fn row(&self, r: usize) -> &[Cell] {
        &self.cells[r * self.cols..(r + 1) * self.cols]
    }
    pub fn extra(&self, cell: &Cell) -> &[char] {
        if cell.extra == 0 {
            return &[];
        }
        let start = (cell.extra >> 8) as usize;
        let len = (cell.extra & 0xff) as usize;
        &self.extra_text[start..start + len]
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            cols: 0,
            rows: 0,
            display_offset: 0,
            history: 0,
            cells: Vec::new(),
            extra_text: Vec::new(),
            cursor: None,
            cursor_color: Rgb::default(),
            background: Rgb::default(),
            mode: TermMode::empty(),
        }
    }
}
