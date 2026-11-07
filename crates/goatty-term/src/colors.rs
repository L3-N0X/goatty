//! goatty-owned color palette and `AnsiColor` → RGB resolution.
//!
//! The VT core stores colors symbolically (named / indexed / rgb); we resolve
//! them here with goatty's theme, honoring runtime overrides applied by the
//! application through OSC 4/10/11/12 (`TermColors`).

use rio_vt::config::colors::term::TermColors;
use rio_vt::config::colors::{AnsiColor, NamedColor};
use rio_vt::crosswords::style::StyleFlags;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Self { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
    }
    pub fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
    fn scale(self, f: f32) -> Self {
        Self { r: (self.r as f32 * f) as u8, g: (self.g as f32 * f) as u8, b: (self.b as f32 * f) as u8 }
    }
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection_fg: Rgb,
    pub selection_bg: Rgb,
    /// Render bold text in the bright variant of colors 0-7 (xterm behavior).
    pub bold_is_bright: bool,
}

impl Default for Palette {
    /// "goatty night": a calm dark theme. Placeholder until the theme system
    /// lands; chosen for legibility of the 16 ANSI colors on the background.
    fn default() -> Self {
        Self {
            ansi: [
                Rgb::hex(0x1d2021), // black
                Rgb::hex(0xe06c75), // red
                Rgb::hex(0x98c379), // green
                Rgb::hex(0xe5c07b), // yellow
                Rgb::hex(0x61afef), // blue
                Rgb::hex(0xc678dd), // magenta
                Rgb::hex(0x56b6c2), // cyan
                Rgb::hex(0xabb2bf), // white
                Rgb::hex(0x5c6370), // bright black
                Rgb::hex(0xff7a85),
                Rgb::hex(0xb5e890),
                Rgb::hex(0xffd68a),
                Rgb::hex(0x82c4ff),
                Rgb::hex(0xdf9cf2),
                Rgb::hex(0x7fd4df),
                Rgb::hex(0xf0f3f6), // bright white
            ],
            foreground: Rgb::hex(0xd7dae0),
            background: Rgb::hex(0x16181d),
            cursor: Rgb::hex(0xf5c26b),
            selection_fg: Rgb::hex(0xf0f3f6),
            selection_bg: Rgb::hex(0x3e4a5e),
            bold_is_bright: true,
        }
    }
}

impl Palette {
    /// From 21 `0xRRGGBB` values: the 16 ANSI colors, then foreground,
    /// background, cursor, selection foreground, selection background (the
    /// layout of the `SetPalette` wire message and `goatty_config::ThemeColors`).
    pub fn from_rgb_list(v: &[u32], bold_is_bright: bool) -> Option<Self> {
        if v.len() != 21 {
            return None;
        }
        let mut ansi = [Rgb::default(); 16];
        for (slot, &c) in ansi.iter_mut().zip(v) {
            *slot = Rgb::hex(c);
        }
        Some(Self {
            ansi,
            foreground: Rgb::hex(v[16]),
            background: Rgb::hex(v[17]),
            cursor: Rgb::hex(v[18]),
            selection_fg: Rgb::hex(v[19]),
            selection_bg: Rgb::hex(v[20]),
            bold_is_bright,
        })
    }
}

fn from_array(a: [f32; 4]) -> Rgb {
    Rgb {
        r: (a[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        g: (a[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        b: (a[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}

impl Palette {
    /// Color for a palette slot (0-255, or a `NamedColor` discriminant).
    pub fn by_index(&self, idx: usize) -> Rgb {
        match idx {
            0..=15 => self.ansi[idx],
            16..=231 => {
                let i = idx - 16;
                let step = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
                Rgb { r: step(i / 36), g: step((i / 6) % 6), b: step(i % 6) }
            }
            232..=255 => {
                let v = (8 + (idx - 232) * 10) as u8;
                Rgb { r: v, g: v, b: v }
            }
            i if i == NamedColor::Foreground as usize => self.foreground,
            i if i == NamedColor::Background as usize => self.background,
            i if i == NamedColor::Cursor as usize => self.cursor,
            i if i == NamedColor::LightForeground as usize => self.foreground,
            i if i == NamedColor::DimForeground as usize => self.foreground.scale(0.66),
            i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
                self.ansi[i - NamedColor::DimBlack as usize].scale(0.66)
            }
            _ => self.foreground,
        }
    }

    pub fn resolve(&self, c: AnsiColor, overrides: &TermColors) -> Rgb {
        match c {
            AnsiColor::Spec(rgb) => Rgb { r: rgb.r, g: rgb.g, b: rgb.b },
            AnsiColor::Indexed(i) => self.slot(i as usize, overrides),
            AnsiColor::Named(n) => self.slot(n as usize, overrides),
        }
    }

    /// Foreground resolution, applying bold-is-bright and dim.
    pub fn resolve_fg(&self, c: AnsiColor, flags: StyleFlags, overrides: &TermColors) -> Rgb {
        let c = match c {
            AnsiColor::Named(n) if self.bold_is_bright && flags.contains(StyleFlags::BOLD) && (n as usize) < 8 => {
                AnsiColor::Indexed(n as u8 + 8)
            }
            AnsiColor::Indexed(i) if self.bold_is_bright && flags.contains(StyleFlags::BOLD) && i < 8 => AnsiColor::Indexed(i + 8),
            other => other,
        };
        let rgb = self.resolve(c, overrides);
        if flags.contains(StyleFlags::DIM) { rgb.scale(0.66) } else { rgb }
    }

    fn slot(&self, idx: usize, overrides: &TermColors) -> Rgb {
        if idx < rio_vt::config::colors::term::COUNT
            && let Some(a) = overrides[idx]
        {
            return from_array(a);
        }
        self.by_index(idx)
    }
}
