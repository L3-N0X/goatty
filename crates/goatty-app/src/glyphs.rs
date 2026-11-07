//! Per-cell glyph cache: shape each distinct (text, bold, italic) once,
//! remember the resolved (font, glyph id) — including font fallback — and
//! paint cells directly at grid positions afterwards.
//!
//! This is how dedicated terminals avoid per-frame text shaping. The cost is
//! no programming ligatures (they need run-level shaping); the "shaped"
//! render mode keeps that path available.

use std::collections::HashMap;

use goatty_term::CellFlags;
use gpui::{Font, FontId, FontStyle, FontWeight, GlyphId, Hsla, Pixels, TextRun, WindowTextSystem, px};

#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub font_id: FontId,
    pub id: GlyphId,
    pub x: Pixels,
    pub emoji: bool,
}

/// Style bits that select a font face.
fn face_bits(flags: CellFlags) -> u8 {
    (flags.contains(CellFlags::BOLD) as u8) | (flags.contains(CellFlags::ITALIC) as u8) << 1
}

pub struct GlyphCache {
    font: Font,
    font_size: Pixels,
    simple: HashMap<(char, u8), Option<Glyph>>,
    complex: HashMap<(String, u8), Vec<Glyph>>,
    pub ascent: Pixels,
    pub descent: Pixels,
}

impl GlyphCache {
    pub fn new() -> Self {
        Self {
            font: gpui::font("monospace"),
            font_size: px(0.),
            simple: HashMap::new(),
            complex: HashMap::new(),
            ascent: px(0.),
            descent: px(0.),
        }
    }

    /// Reset when the font or size changes; returns true if it was reset.
    pub fn ensure(&mut self, ts: &WindowTextSystem, font: &Font, font_size: Pixels) -> bool {
        if self.font_size == font_size && &self.font == font {
            return false;
        }
        self.font = font.clone();
        self.font_size = font_size;
        self.simple.clear();
        self.complex.clear();
        // Take vertical metrics from a shaped layout, exactly like gpui's own
        // `paint_line` does. `TextSystem::descent()` is *negative* on the
        // wgpu/cosmic-text backend while `LineLayout::descent` is positive;
        // mixing them shifted glyphs down and underlines up.
        let run =
            TextRun { len: 1, font: font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
        let layout = ts.layout_line("M", font_size, &[run], None);
        self.ascent = layout.ascent;
        self.descent = layout.descent.abs();
        true
    }

    fn face(&self, bits: u8) -> Font {
        let mut f = self.font.clone();
        if bits & 1 != 0 {
            f.weight = FontWeight::BOLD;
        }
        if bits & 2 != 0 {
            f.style = FontStyle::Italic;
        }
        f
    }

    fn shape(&self, ts: &WindowTextSystem, text: &str, bits: u8) -> Vec<Glyph> {
        let run = TextRun {
            len: text.len(),
            font: self.face(bits),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let layout = ts.layout_line(text, self.font_size, &[run], None);
        layout
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter().map(move |g| Glyph { font_id: r.font_id, id: g.id, x: g.position.x, emoji: g.is_emoji }))
            .collect()
    }

    pub fn simple(&mut self, ts: &WindowTextSystem, ch: char, flags: CellFlags) -> Option<Glyph> {
        let bits = face_bits(flags);
        if let Some(g) = self.simple.get(&(ch, bits)) {
            return *g;
        }
        let mut buf = [0u8; 4];
        let g = self.shape(ts, ch.encode_utf8(&mut buf), bits).into_iter().next();
        self.simple.insert((ch, bits), g);
        g
    }

    pub fn complex(&mut self, ts: &WindowTextSystem, text: &str, flags: CellFlags) -> &[Glyph] {
        let key = (text.to_string(), face_bits(flags));
        if !self.complex.contains_key(&key) {
            let g = self.shape(ts, text, key.1);
            self.complex.insert(key.clone(), g);
        }
        &self.complex[&key]
    }
}
