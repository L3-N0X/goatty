//! The terminal grid as one custom GPUI element.
//!
//! Per frame: lock the terminal once to copy a resolved [`Frame`], then build
//! a small number of draw calls:
//!   * one quad per run of identical non-default background,
//!   * one shaped text line per row *segment* (a maximal stretch of simple
//!     narrow cells, possibly mixing colors/weights via multiple `TextRun`s),
//!     shaped with `force_width = cell_width` so glyphs land on the grid,
//!   * wide / combining cells shaped individually at their exact cell.
//!
//! GPUI caches shaped lines across frames keyed by (text, runs), so unchanged
//! rows cost a hash lookup, not a reshape.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use goatty_term::{CellFlags, CursorKind, Frame, LinkSpan, Rgb, TermSize};
use gpui::{
    App, Bounds, Element, ElementId, Font, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels,
    Point, ShapedLine, SharedString, Size, StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, fill, point, px, relative, size,
};

use super::glyphs::{Glyph, GlyphCache};
use super::stats::Stats;
use crate::session::Session;

pub fn hsla(c: Rgb) -> Hsla {
    gpui::rgb(c.to_u32()).into()
}

/// Geometry of the last painted grid, for mouse → cell mapping.
#[derive(Clone, Copy, Debug, Default)]
pub struct Geom {
    pub origin: Point<Pixels>,
    pub cell: Size<Pixels>,
    pub cols: usize,
    pub rows: usize,
}

impl Geom {
    /// (col, row, right_half_of_cell)
    pub fn cell_at(&self, p: Point<Pixels>) -> (usize, usize, bool) {
        let x = ((p.x - self.origin.x) / self.cell.width).max(0.0);
        let y = ((p.y - self.origin.y) / self.cell.height).max(0.0);
        let col = (x as usize).min(self.cols.saturating_sub(1));
        let row = (y as usize).min(self.rows.saturating_sub(1));
        (col, row, x.fract() > 0.5)
    }

    /// Selection auto-scroll: lines to scroll per tick for a drag at height
    /// `y`, positive towards history. 0 over the grid; past the top or
    /// bottom edge it grows with the distance, one line per cell height.
    pub fn edge_scroll(&self, y: Pixels) -> i32 {
        const MAX: f32 = 10.0;
        let h = self.cell.height.max(px(1.0));
        let top = self.origin.y;
        let bottom = top + h * self.rows as f32;
        if y < top {
            (1.0 + (top - y) / h).min(MAX) as i32
        } else if y >= bottom {
            -((1.0 + (y - bottom) / h).min(MAX) as i32)
        } else {
            0
        }
    }
}

pub struct TerminalElement {
    pub session: Arc<Session>,
    pub frame: Rc<RefCell<Frame>>,
    pub geom: Rc<StdCell<Geom>>,
    pub stats: Rc<RefCell<Stats>>,
    pub font: Font,
    pub font_size: Pixels,
    pub line_height_factor: f32,
    /// Space around the grid (top, right, bottom, left).
    pub padding: gpui::Edges<Pixels>,
    pub focused: bool,
    /// true: per-cell glyph cache (default). false: per-segment line shaping
    /// (supports ligatures, slower when many lines change).
    pub glyph_mode: bool,
    pub glyphs: Rc<RefCell<GlyphCache>>,
    /// Blink phase; when false the focused cursor is not drawn.
    pub cursor_visible: bool,
    /// The hovered link, underlined.
    pub link_spans: Vec<LinkSpan>,
}

struct GlyphDraw {
    origin: Point<Pixels>,
    glyph: Glyph,
    color: Hsla,
}

enum Deco {
    Underline(Point<Pixels>, Pixels, UnderlineStyle),
    Strike(Point<Pixels>, Pixels, StrikethroughStyle),
}

/// Cursor bounds, shape, and the glyph under a block cursor (shaped or cached).
type CursorLayout = (Bounds<Pixels>, CursorKind, Option<ShapedLine>, Vec<GlyphDraw>);

pub struct Layout {
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    glyphs: Vec<GlyphDraw>,
    decos: Vec<Deco>,
    cursor: Option<CursorLayout>,
    cursor_color: Hsla,
    line_height: Pixels,
    started: Instant,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    fg: Rgb,
    flags: CellFlags,
}

impl TerminalElement {
    fn run_font(&self, flags: CellFlags) -> Font {
        let mut f = self.font.clone();
        if flags.contains(CellFlags::BOLD) {
            f.weight = FontWeight::BOLD;
        }
        if flags.contains(CellFlags::ITALIC) {
            f.style = FontStyle::Italic;
        }
        f
    }

    fn text_run(&self, len: usize, st: RunStyle) -> TextRun {
        let color = hsla(st.fg);
        let underline = st.flags.contains(CellFlags::UNDERLINE).then(|| UnderlineStyle {
            thickness: px(1.0),
            color: Some(color),
            wavy: st.flags.contains(CellFlags::UNDERCURL),
        });
        let strikethrough = st.flags.contains(CellFlags::STRIKE).then(|| StrikethroughStyle { thickness: px(1.0), color: Some(color) });
        TextRun { len, font: self.run_font(st.flags), color, background_color: None, underline, strikethrough }
    }
}

/// Bits of CellFlags that affect glyph rendering (not WIDE/SPACER).
fn style_bits(f: CellFlags) -> CellFlags {
    let mut out = CellFlags::empty();
    for b in [CellFlags::BOLD, CellFlags::ITALIC, CellFlags::UNDERLINE, CellFlags::UNDERCURL, CellFlags::STRIKE] {
        if f.contains(b) {
            out |= b;
        }
    }
    out
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Layout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> Layout {
        let started = Instant::now();
        let ts = window.text_system().clone();
        let font_id = ts.resolve_font(&self.font);
        let cell_w = ts.advance(font_id, self.font_size, 'm').map(|s| s.width).unwrap_or(self.font_size * 0.6);
        let line_height = (self.font_size * self.line_height_factor).round();
        let pad = self.padding;
        let inner_w = bounds.size.width - pad.left - pad.right;
        let inner_h = bounds.size.height - pad.top - pad.bottom;
        let cols = ((inner_w / cell_w).floor() as u16).max(2);
        let rows = ((inner_h / line_height).floor() as u16).max(1);
        let origin = bounds.origin + point(pad.left, pad.top);

        if self.session.terminal.size() != (cols, rows) {
            let scale = window.scale_factor();
            self.session.resize(TermSize {
                cols,
                rows,
                cell_width: (f32::from(cell_w) * scale) as u16,
                cell_height: (f32::from(line_height) * scale) as u16,
            });
        }

        let mut frame = self.frame.borrow_mut();
        let snap_start = Instant::now();
        self.session.terminal.snapshot_into(&mut frame);
        self.stats.borrow_mut().snapshot_us += snap_start.elapsed().as_micros() as u64;
        self.geom.set(Geom { origin, cell: size(cell_w, line_height), cols: frame.cols, rows: frame.rows });

        let default_bg = frame.background;
        let mut cache = self.glyphs.borrow_mut();
        cache.ensure(&ts, &self.font, self.font_size);
        let mut glyph_draws: Vec<GlyphDraw> = Vec::new();
        let mut decos: Vec<Deco> = Vec::new();
        let mut rects = Vec::new();
        let mut lines = Vec::with_capacity(frame.rows * 2);
        let mut seg_text = String::with_capacity(frame.cols);
        let mut runs: Vec<TextRun> = Vec::new();

        for r in 0..frame.rows {
            let row = frame.row(r);
            let y = origin.y + line_height * r as f32;

            // Backgrounds: merge runs of equal non-default color.
            let mut c = 0;
            while c < row.len() {
                let bg = row[c].bg;
                let start = c;
                while c < row.len() && row[c].bg == bg {
                    c += 1;
                }
                if bg != default_bg {
                    rects.push((
                        Bounds::new(point(origin.x + cell_w * start as f32, y), size(cell_w * (c - start) as f32, line_height)),
                        hsla(bg),
                    ));
                }
            }

            if self.glyph_mode {
                let baseline = y + (line_height - cache.ascent - cache.descent) / 2.0 + cache.ascent;
                let mut deco_run: Option<(usize, Rgb, CellFlags)> = None;
                for (col, cell) in row.iter().enumerate() {
                    let deco_flags = deco_bits(cell.flags);
                    // Underline/strike runs (merged across cells).
                    match deco_run {
                        Some((_, fg, f)) if f == deco_flags && fg == cell.fg => {}
                        _ => {
                            if let Some((start, fg, f)) = deco_run.take() {
                                push_deco(&mut decos, origin.x, cell_w, baseline, &cache, start, col, fg, f);
                            }
                            if deco_flags != CellFlags::empty() {
                                deco_run = Some((col, cell.fg, deco_flags));
                            }
                        }
                    }
                    if cell.flags.contains(CellFlags::SPACER) || cell.ch == ' ' {
                        continue;
                    }
                    let x = origin.x + cell_w * col as f32;
                    let color = hsla(cell.fg);
                    let extra = frame.extra(cell);
                    if extra.is_empty() {
                        if let Some(g) = cache.simple(&ts, cell.ch, cell.flags) {
                            glyph_draws.push(GlyphDraw { origin: point(x + g.x, baseline), glyph: g, color });
                        }
                    } else {
                        let mut text = String::new();
                        text.push(cell.ch);
                        text.extend(extra);
                        for g in cache.complex(&ts, &text, cell.flags) {
                            glyph_draws.push(GlyphDraw { origin: point(x + g.x, baseline), glyph: *g, color });
                        }
                    }
                }
                if let Some((start, fg, f)) = deco_run.take() {
                    push_deco(&mut decos, origin.x, cell_w, baseline, &cache, start, row.len(), fg, f);
                }
                continue;
            }

            // Text segments.
            let mut seg_start = 0usize;
            let mut cur: Option<RunStyle> = None;
            let mut run_len = 0usize;
            let flush = |seg_text: &mut String,
                         runs: &mut Vec<TextRun>,
                         cur: &mut Option<RunStyle>,
                         run_len: &mut usize,
                         seg_start: usize,
                         lines: &mut Vec<(Point<Pixels>, ShapedLine)>| {
                if let Some(st) = cur.take() {
                    runs.push(self.text_run(*run_len, st));
                }
                *run_len = 0;
                // Drop trailing spaces that draw nothing.
                let trimmed = seg_text.trim_end_matches(' ').len();
                let mut cut = seg_text.len() - trimmed;
                while cut > 0 {
                    let last = runs.last_mut().unwrap();
                    if last.underline.is_some() || last.strikethrough.is_some() {
                        break;
                    }
                    let take = cut.min(last.len);
                    last.len -= take;
                    cut -= take;
                    if last.len == 0 {
                        runs.pop();
                    }
                    seg_text.truncate(seg_text.len() - take);
                }
                if !seg_text.is_empty() && !runs.is_empty() {
                    let line = ts.shape_line(SharedString::from(seg_text.clone()), self.font_size, runs, Some(cell_w));
                    lines.push((point(origin.x + cell_w * seg_start as f32, y), line));
                }
                seg_text.clear();
                runs.clear();
            };

            for (col, cell) in row.iter().enumerate() {
                if cell.flags.contains(CellFlags::SPACER) {
                    flush(&mut seg_text, &mut runs, &mut cur, &mut run_len, seg_start, &mut lines);
                    continue;
                }
                let extra = frame.extra(cell);
                let wide = cell.flags.contains(CellFlags::WIDE);
                let st = RunStyle { fg: cell.fg, flags: style_bits(cell.flags) };
                if wide || !extra.is_empty() {
                    // Complex cell: shape alone, anchored at its column.
                    flush(&mut seg_text, &mut runs, &mut cur, &mut run_len, seg_start, &mut lines);
                    let mut text = String::new();
                    text.push(cell.ch);
                    text.extend(extra);
                    let run = self.text_run(text.len(), st);
                    let line = ts.shape_line(SharedString::from(text), self.font_size, &[run], None);
                    lines.push((point(origin.x + cell_w * col as f32, y), line));
                    continue;
                }
                if seg_text.is_empty() {
                    if cell.ch == ' ' {
                        continue; // leading blanks: start segment later
                    }
                    seg_start = col;
                } else if col != seg_start + seg_text.chars().count() {
                    // gap (skipped leading blanks never happen mid-seg, but be safe)
                    flush(&mut seg_text, &mut runs, &mut cur, &mut run_len, seg_start, &mut lines);
                    seg_start = col;
                }
                // A plain space can join the current run whatever its color.
                let joins = match cur {
                    Some(c) => c == st || (cell.ch == ' ' && st.flags == CellFlags::empty() && c.flags == CellFlags::empty()),
                    None => false,
                };
                if !joins {
                    if let Some(prev) = cur.take() {
                        runs.push(self.text_run(run_len, prev));
                    }
                    cur = Some(st);
                    run_len = 0;
                }
                seg_text.push(cell.ch);
                run_len += cell.ch.len_utf8();
            }
            flush(&mut seg_text, &mut runs, &mut cur, &mut run_len, seg_start, &mut lines);
        }

        // Hovered link: underline in each stretch's text color.
        for span in &self.link_spans {
            if span.row >= frame.rows || span.cols.start >= frame.cols {
                continue;
            }
            let end = span.cols.end.min(frame.cols);
            let y = origin.y + line_height * span.row as f32;
            let baseline = y + (line_height - cache.ascent - cache.descent) / 2.0 + cache.ascent;
            let color = Some(hsla(frame.row(span.row)[span.cols.start].fg));
            decos.push(Deco::Underline(
                point(origin.x + cell_w * span.cols.start as f32, baseline + cache.descent * 0.618),
                cell_w * (end - span.cols.start) as f32,
                UnderlineStyle { thickness: px(1.0), color, wavy: false },
            ));
        }

        // Cursor.
        let cursor = frame.cursor.map(|cur| {
            let w = if cur.wide { cell_w * 2.0 } else { cell_w };
            let b = Bounds::new(point(origin.x + cell_w * cur.col as f32, origin.y + line_height * cur.row as f32), size(w, line_height));
            let mut cursor_glyphs = Vec::new();
            if self.glyph_mode && self.focused && cur.shape == CursorKind::Block {
                let cell = frame.row(cur.row)[cur.col];
                if cell.ch != ' ' {
                    let baseline = b.origin.y + (line_height - cache.ascent - cache.descent) / 2.0 + cache.ascent;
                    let mut text = String::new();
                    text.push(cell.ch);
                    text.extend(frame.extra(&cell));
                    for g in cache.complex(&ts, &text, cell.flags) {
                        cursor_glyphs.push(GlyphDraw {
                            origin: point(b.origin.x + g.x, baseline),
                            glyph: *g,
                            color: hsla(frame.background),
                        });
                    }
                }
            }
            let glyph = if !self.glyph_mode && self.focused && cur.shape == CursorKind::Block {
                let cell = frame.row(cur.row)[cur.col];
                (cell.ch != ' ').then(|| {
                    let mut text = String::new();
                    text.push(cell.ch);
                    text.extend(frame.extra(&cell));
                    let st = RunStyle { fg: frame.background, flags: style_bits(cell.flags) };
                    let run = self.text_run(text.len(), st);
                    ts.shape_line(SharedString::from(text), self.font_size, &[run], None)
                })
            } else {
                None
            };
            (b, cur.shape, glyph, cursor_glyphs)
        });

        self.stats.borrow_mut().shaped_segments += if self.glyph_mode { glyph_draws.len() } else { lines.len() } as u64;
        Layout { rects, lines, glyphs: glyph_draws, decos, cursor, cursor_color: hsla(frame.cursor_color), line_height, started }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        layout: &mut Layout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint_start = Instant::now();
        for (b, color) in layout.rects.drain(..) {
            window.paint_quad(fill(b, color));
        }
        for (origin, line) in &layout.lines {
            let _ = line.paint(*origin, layout.line_height, gpui::TextAlign::Left, None, window, cx);
        }
        let font_size = self.font_size;
        let paint_glyph = |d: &GlyphDraw, window: &mut Window| {
            let _ = if d.glyph.emoji {
                window.paint_emoji(d.origin, d.glyph.font_id, d.glyph.id, font_size)
            } else {
                window.paint_glyph(d.origin, d.glyph.font_id, d.glyph.id, font_size, d.color)
            };
        };
        for d in &layout.glyphs {
            paint_glyph(d, window);
        }
        for d in layout.decos.drain(..) {
            match d {
                Deco::Underline(o, w, st) => window.paint_underline(o, w, &st),
                Deco::Strike(o, w, st) => window.paint_strikethrough(o, w, &st),
            }
        }
        let cursor = layout.cursor.take().filter(|_| self.cursor_visible || !self.focused);
        if let Some((b, shape, glyph, cursor_glyphs)) = cursor {
            let c = layout.cursor_color;
            let t = px(2.0);
            match (shape, self.focused) {
                (CursorKind::Block, true) => {
                    window.paint_quad(fill(b, c));
                    if let Some(g) = glyph {
                        let _ = g.paint(b.origin, layout.line_height, gpui::TextAlign::Left, None, window, cx);
                    }
                    for d in &cursor_glyphs {
                        paint_glyph(d, window);
                    }
                }
                (CursorKind::Beam, _) => window.paint_quad(fill(Bounds::new(b.origin, size(t, b.size.height)), c)),
                (CursorKind::Underline, _) => {
                    window.paint_quad(fill(Bounds::new(point(b.origin.x, b.origin.y + b.size.height - t), size(b.size.width, t)), c))
                }
                (CursorKind::Block, false) => {
                    // Hollow block when unfocused.
                    let (o, s) = (b.origin, b.size);
                    window.paint_quad(fill(Bounds::new(o, size(s.width, px(1.))), c));
                    window.paint_quad(fill(Bounds::new(point(o.x, o.y + s.height - px(1.)), size(s.width, px(1.))), c));
                    window.paint_quad(fill(Bounds::new(o, size(px(1.), s.height)), c));
                    window.paint_quad(fill(Bounds::new(point(o.x + s.width - px(1.), o.y), size(px(1.), s.height)), c));
                }
            }
        }
        let mut st = self.stats.borrow_mut();
        let pre = paint_start.duration_since(layout.started).as_micros() as u64;
        let paint = paint_start.elapsed().as_micros() as u64;
        st.frames += 1;
        st.prepaint_us += pre;
        st.paint_us += paint;
        st.max_frame_us = st.max_frame_us.max(pre + paint);
    }
}

fn deco_bits(f: CellFlags) -> CellFlags {
    let mut out = CellFlags::empty();
    for b in [CellFlags::UNDERLINE, CellFlags::UNDERCURL, CellFlags::STRIKE] {
        if f.contains(b) {
            out |= b;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn push_deco(
    decos: &mut Vec<Deco>,
    x0: Pixels,
    cell_w: Pixels,
    baseline: Pixels,
    cache: &GlyphCache,
    start: usize,
    end: usize,
    fg: Rgb,
    f: CellFlags,
) {
    let color = Some(hsla(fg));
    let x = x0 + cell_w * start as f32;
    let w = cell_w * (end - start) as f32;
    if f.contains(CellFlags::UNDERLINE) {
        let wavy = f.contains(CellFlags::UNDERCURL);
        decos.push(Deco::Underline(point(x, baseline + cache.descent * 0.618), w, UnderlineStyle { thickness: px(1.0), color, wavy }));
    }
    if f.contains(CellFlags::STRIKE) {
        decos.push(Deco::Strike(point(x, baseline - cache.ascent * 0.32), w, StrikethroughStyle { thickness: px(1.0), color }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_scroll_grows_past_the_edges() {
        let g = Geom { origin: point(px(5.0), px(10.0)), cell: size(px(8.0), px(20.0)), cols: 80, rows: 10 };
        assert_eq!(g.edge_scroll(px(10.0)), 0);
        assert_eq!(g.edge_scroll(px(209.0)), 0);
        assert_eq!(g.edge_scroll(px(9.0)), 1);
        assert_eq!(g.edge_scroll(px(-35.0)), 3);
        assert_eq!(g.edge_scroll(px(210.0)), -1);
        assert_eq!(g.edge_scroll(px(250.0)), -3);
        assert_eq!(g.edge_scroll(px(-10_000.0)), 10);
        assert_eq!(g.edge_scroll(px(10_000.0)), -10);
    }
}
