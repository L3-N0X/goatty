//! goatty's terminal engine.
//!
//! This crate is the *only* place that talks to `rio-vt` directly. Everything
//! above it (the GPUI renderer, the daemon) goes through [`Terminal`] and the
//! plain data types in this crate, so swapping the VT core (e.g. for
//! `wezterm-term` or `alacritty_terminal`) only touches this crate.
//!
//! Threading model: bytes are fed from a single I/O thread via
//! [`Terminal::feed`]; renderers take the lock briefly once per frame via
//! [`Terminal::with_term`] / [`Terminal::snapshot_into`] and copy out what
//! they need.

pub mod colors;
pub mod input;
pub mod pty;
pub mod snapshot;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Scroll;
use rio_vt::crosswords::pos::{Column, Line, Pos, Side};
use rio_vt::crosswords::style::StyleFlags;
use rio_vt::crosswords::{Crosswords, CrosswordsSize};
use rio_vt::event::{EventListener, RioEvent, WindowId};
use rio_vt::performer::handler::Processor;
use rio_vt::selection::{Selection, SelectionType};

pub use colors::{Palette, Rgb};
pub use rio_vt::crosswords::Mode as TermMode;

/// Events the terminal emits that the *UI* cares about.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Title(String),
    ResetTitle,
    Bell,
    /// OSC 52 clipboard write from the application.
    ClipboardStore(String),
    CwdChanged,
    Exit,
}

/// Grid size in cells plus the pixel size (for TIOCSWINSZ / XTWINOPS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TermSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows, cell_width: 8, cell_height: 16 }
    }
    fn to_crosswords(self) -> CrosswordsSize {
        let mut s = CrosswordsSize::new(self.cols.max(1) as usize, self.rows.max(1) as usize);
        s.square_width = self.cell_width as u32;
        s.square_height = self.cell_height as u32;
        s.width = self.cols as u32 * self.cell_width as u32;
        s.height = self.rows as u32 * self.cell_height as u32;
        s
    }
}

/// Listener plugged into `Crosswords`. It splits events into two channels:
/// *replies* (bytes that must go back to the PTY, e.g. DA/DSR answers) and
/// *UI events*.
///
/// When the same byte stream is parsed by two terminals (daemon + client),
/// only one of them may answer queries, or the shell sees every reply twice.
/// `answer_queries` selects which.
#[derive(Clone)]
pub struct Listener {
    replies: flume::Sender<Vec<u8>>,
    ui: flume::Sender<UiEvent>,
    answer_queries: bool,
    palette: Arc<Palette>,
    size: Arc<AtomicU32>, // packed (cols << 16 | rows)
}

impl EventListener for Listener {
    fn send_event(&self, event: RioEvent, _id: WindowId) {
        match event {
            RioEvent::PtyWrite(_, text) => {
                if self.answer_queries {
                    let _ = self.replies.send(text.into_bytes());
                }
            }
            RioEvent::ColorRequest(_, index, fmt) => {
                if self.answer_queries {
                    let c = self.palette.by_index(index);
                    let reply = fmt(rio_vt::config::colors::ColorRgb { r: c.r, g: c.g, b: c.b });
                    let _ = self.replies.send(reply.into_bytes());
                }
            }
            RioEvent::TextAreaSizeRequest(_, fmt) => {
                if self.answer_queries {
                    let packed = self.size.load(Ordering::Relaxed);
                    let ws = rio_vt::event::WindowSize { cols: (packed >> 16) as u16, rows: (packed & 0xffff) as u16, width: 0, height: 0 };
                    let _ = self.replies.send(fmt(ws).into_bytes());
                }
            }
            // OSC 52 reads are a data-exfiltration vector; deny by default.
            RioEvent::ClipboardLoad(..) => {}
            RioEvent::ClipboardStore(_, text) => {
                let _ = self.ui.send(UiEvent::ClipboardStore(text));
            }
            RioEvent::Title(_, t) => {
                let _ = self.ui.send(UiEvent::Title(t));
            }
            RioEvent::ResetTitle => {
                let _ = self.ui.send(UiEvent::ResetTitle);
            }
            RioEvent::Bell(_) => {
                let _ = self.ui.send(UiEvent::Bell);
            }
            RioEvent::CurrentDirectoryChanged(_) => {
                let _ = self.ui.send(UiEvent::CwdChanged);
            }
            RioEvent::Exit | RioEvent::CloseTerminal(_) => {
                let _ = self.ui.send(UiEvent::Exit);
            }
            _ => {}
        }
    }
}

pub type Term = Crosswords<Listener>;

struct Inner {
    term: Mutex<Term>,
    parser: Mutex<Processor>,
    size: Arc<AtomicU32>,
}

/// Shared handle to one terminal state machine. Cheap to clone.
#[derive(Clone)]
pub struct Terminal {
    inner: Arc<Inner>,
    pub palette: Arc<Palette>,
}

/// Receivers returned by [`Terminal::new`].
pub struct TerminalChannels {
    pub replies: flume::Receiver<Vec<u8>>,
    pub ui: flume::Receiver<UiEvent>,
}

pub const DEFAULT_SCROLLBACK: usize = 10_000;

impl Terminal {
    pub fn new(size: TermSize, answer_queries: bool, palette: Arc<Palette>) -> (Self, TerminalChannels) {
        let (rtx, rrx) = flume::unbounded();
        let (utx, urx) = flume::unbounded();
        let packed = Arc::new(AtomicU32::new(pack(size)));
        let listener = Listener { replies: rtx, ui: utx, answer_queries, palette: palette.clone(), size: packed.clone() };
        let term = Crosswords::new(size.to_crosswords(), CursorShape::Block, listener, WindowId::from(0), 0, DEFAULT_SCROLLBACK);
        let inner = Inner { term: Mutex::new(term), parser: Mutex::new(Processor::default()), size: packed };
        (Self { inner: Arc::new(inner), palette }, TerminalChannels { replies: rrx, ui: urx })
    }

    /// Feed raw PTY output. Returns `true` if a redraw should be scheduled
    /// (false while the application holds a synchronized update, mode 2026).
    pub fn feed(&self, bytes: &[u8]) -> bool {
        let mut parser = self.inner.parser.lock();
        let mut term = self.inner.term.lock();
        parser.advance(&mut *term, bytes);
        if term.synchronized_output() {
            if term.sync_update_expired(rio_vt::time::Instant::now()) {
                term.end_sync_update();
                return true;
            }
            return false;
        }
        true
    }

    pub fn resize(&self, size: TermSize) {
        self.inner.size.store(pack(size), Ordering::Relaxed);
        self.inner.term.lock().resize(size.to_crosswords());
    }

    pub fn size(&self) -> (u16, u16) {
        let p = self.inner.size.load(Ordering::Relaxed);
        ((p >> 16) as u16, (p & 0xffff) as u16)
    }

    /// Run `f` with the terminal locked. Keep it short: the I/O thread is
    /// blocked meanwhile.
    pub fn with_term<R>(&self, f: impl FnOnce(&mut Term) -> R) -> R {
        f(&mut self.inner.term.lock())
    }

    pub fn mode(&self) -> TermMode {
        self.inner.term.lock().mode()
    }

    pub fn title(&self) -> String {
        self.inner.term.lock().title.clone()
    }

    pub fn scroll(&self, delta_lines: i32) {
        self.inner.term.lock().scroll_display(Scroll::Delta(delta_lines));
    }

    pub fn scroll_to_bottom(&self) {
        let mut t = self.inner.term.lock();
        if t.display_offset() != 0 {
            t.scroll_display(Scroll::Bottom);
        }
    }

    // ---- selection -------------------------------------------------------

    /// Start a selection at a *viewport* cell (row 0 = top of what's visible).
    pub fn selection_start(&self, col: usize, row: usize, kind: SelectKind, right_half: bool) {
        let mut t = self.inner.term.lock();
        let pos = viewport_to_grid(&t, col, row);
        let side = if right_half { Side::Right } else { Side::Left };
        let ty = match kind {
            SelectKind::Simple => SelectionType::Simple,
            SelectKind::Word => SelectionType::Semantic,
            SelectKind::Line => SelectionType::Lines,
        };
        t.selection = Some(Selection::new(ty, pos, side));
        t.mark_fully_damaged();
    }

    pub fn selection_update(&self, col: usize, row: usize, right_half: bool) {
        let mut t = self.inner.term.lock();
        let pos = viewport_to_grid(&t, col, row);
        let side = if right_half { Side::Right } else { Side::Left };
        if let Some(sel) = t.selection.as_mut() {
            sel.update(pos, side);
        }
        t.mark_fully_damaged();
    }

    pub fn selection_clear(&self) {
        let mut t = self.inner.term.lock();
        if t.selection.take().is_some() {
            t.mark_fully_damaged();
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        let t = self.inner.term.lock();
        t.selection_to_string().filter(|s| !s.is_empty())
    }

    pub fn has_selection(&self) -> bool {
        let t = self.inner.term.lock();
        t.selection.as_ref().is_some_and(|s| !s.is_empty())
    }

    // ---- frame snapshot -------------------------------------------------

    /// Copy everything a renderer needs for one frame into `frame`, reusing
    /// its allocations. Colors are resolved to RGB here so the renderer
    /// never needs rio-vt types.
    pub fn snapshot_into(&self, frame: &mut Frame) {
        let t = self.inner.term.lock();
        let cols = t.columns();
        let rows = t.screen_lines();
        let offset = t.display_offset() as i32;
        let colors = *t.colors();
        let pal = &*self.palette;
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

fn pack(s: TermSize) -> u32 {
    ((s.cols as u32) << 16) | s.rows as u32
}

fn viewport_to_grid(t: &Term, col: usize, row: usize) -> Pos {
    let offset = t.display_offset() as i32;
    let row = row.min(t.screen_lines().saturating_sub(1));
    let col = col.min(t.columns().saturating_sub(1));
    Pos::new(Line(row as i32 - offset), Column(col))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectKind {
    Simple,
    Word,
    Line,
}

// ---- renderer-facing plain data ------------------------------------------

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
