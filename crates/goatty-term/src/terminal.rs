//! [`Terminal`]: one VT state machine (rio-vt `Crosswords` + parser) behind
//! a cheap, thread-safe handle.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Scroll;
use rio_vt::crosswords::pos::{Column, Line, Pos, Side};
use rio_vt::crosswords::{Crosswords, CrosswordsSize};
use rio_vt::event::WindowId;
use rio_vt::performer::handler::Processor;
use rio_vt::selection::{Selection, SelectionType};

use crate::listener::Listener;
use crate::{Palette, TermMode, UiEvent};

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

pub(crate) type Term = Crosswords<Listener>;

pub(crate) struct Inner {
    pub(crate) term: Mutex<Term>,
    parser: Mutex<Processor>,
    size: Arc<AtomicU32>,
}

/// Shared handle to one terminal state machine. Cheap to clone.
#[derive(Clone)]
pub struct Terminal {
    pub(crate) inner: Arc<Inner>,
    /// Shared with the listener, which answers color queries from it.
    palette: SharedPalette,
}

/// A palette that can be swapped while the terminal runs (theme changes).
pub(crate) type SharedPalette = Arc<parking_lot::RwLock<Arc<Palette>>>;

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
        let palette: SharedPalette = Arc::new(parking_lot::RwLock::new(palette));
        let listener = Listener { replies: rtx, ui: utx, answer_queries, palette: palette.clone(), size: packed.clone() };
        let term = Crosswords::new(size.to_crosswords(), CursorShape::Block, listener, WindowId::from(0), 0, DEFAULT_SCROLLBACK);
        let inner = Inner { term: Mutex::new(term), parser: Mutex::new(Processor::default()), size: packed };
        (Self { inner: Arc::new(inner), palette }, TerminalChannels { replies: rrx, ui: urx })
    }

    /// The palette colors resolve against.
    pub fn palette(&self) -> Arc<Palette> {
        self.palette.read().clone()
    }

    /// Switch palettes (theme change). Takes effect on the next frame and
    /// for the next color query (OSC 4/10/11).
    pub fn set_palette(&self, palette: Arc<Palette>) {
        *self.palette.write() = palette;
    }

    /// The cursor shape used until the application asks for another
    /// (DECSCUSR), and after it resets. Applies right away if the cursor is
    /// currently in the old default shape.
    pub fn set_default_cursor(&self, kind: crate::CursorKind) {
        let shape = match kind {
            crate::CursorKind::Block => CursorShape::Block,
            crate::CursorKind::Beam => CursorShape::Beam,
            crate::CursorKind::Underline => CursorShape::Underline,
        };
        let mut t = self.inner.term.lock();
        if t.cursor_shape == t.default_cursor_shape {
            t.cursor_shape = shape;
        }
        t.default_cursor_shape = shape;
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
    /// blocked meanwhile. Crate-private so rio-vt types never leak out.
    pub(crate) fn with_term<R>(&self, f: impl FnOnce(&mut Term) -> R) -> R {
        f(&mut self.inner.term.lock())
    }

    pub fn mode(&self) -> TermMode {
        self.inner.term.lock().mode()
    }

    /// Scrollback lines currently held (primary screen).
    pub fn history_size(&self) -> usize {
        self.inner.term.lock().history_size()
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
}

pub(crate) fn pack(s: TermSize) -> u32 {
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
