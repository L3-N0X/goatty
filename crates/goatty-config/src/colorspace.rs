//! Color spaces and text formats for the color picker (and anything else
//! that takes a color from a user): HSV, HSL, Oklab/Oklch conversions and a
//! lenient parser that understands what people paste.
//!
//! Everything works on opaque sRGB [`Color`]s (themes have no alpha). An
//! alpha component in pasted text (`#rrggbbaa`, `rgb(… / 50%)`) is accepted
//! and ignored. Out-of-gamut Oklab/Oklch values are mapped into sRGB by
//! lowering chroma (keeping lightness and hue), like CSS Color 4 suggests.

use crate::Color;

/// sRGB channels, each 0.0..=1.0.
pub type Rgbf = [f32; 3];

/// Hue in degrees (0..360), saturation and value in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

/// Hue in degrees (0..360), saturation and lightness in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

/// Oklab: perceptual lightness `l` (0..=1), green-red `a`, blue-yellow `b`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

/// Oklab in polar form: lightness `l` (0..=1), chroma `c` (0..~0.4 inside
/// sRGB), hue `h` in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

/// Chroma that spans a full percentage (`oklch(… 100% …)`) in CSS Color 4.
const CHROMA_100: f32 = 0.4;

// ---- Color <-> channels ----------------------------------------------------------

/// The channels of `c` as floats.
pub fn to_rgbf(c: Color) -> Rgbf {
    let (r, g, b) = c.rgb();
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

/// The nearest 8-bit color (channels are clamped).
pub fn from_rgbf(rgb: Rgbf) -> Color {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    Color((q(rgb[0]) << 16) | (q(rgb[1]) << 8) | q(rgb[2]))
}

pub fn rgb8(r: u8, g: u8, b: u8) -> Color {
    Color(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
}

fn wrap_hue(h: f32) -> f32 {
    let h = h % 360.0;
    if h < 0.0 { h + 360.0 } else { h }
}

// ---- HSV / HSL -------------------------------------------------------------------

impl Hsv {
    pub fn new(h: f32, s: f32, v: f32) -> Self {
        Self { h: wrap_hue(h), s: s.clamp(0.0, 1.0), v: v.clamp(0.0, 1.0) }
    }

    pub fn to_rgbf(self) -> Rgbf {
        hue_to_rgb(self.h, self.s * self.v, self.v - self.s * self.v)
    }

    pub fn to_color(self) -> Color {
        from_rgbf(self.to_rgbf())
    }

    /// Achromatic colors have no hue: it comes out as 0 (callers that edit
    /// interactively keep the previous hue instead).
    pub fn from_rgbf([r, g, b]: Rgbf) -> Self {
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        Self { h: hue_of(r, g, b, max, d), s: if max <= 0.0 { 0.0 } else { d / max }, v: max }
    }

    pub fn from_color(c: Color) -> Self {
        Self::from_rgbf(to_rgbf(c))
    }
}

impl Hsl {
    pub fn new(h: f32, s: f32, l: f32) -> Self {
        Self { h: wrap_hue(h), s: s.clamp(0.0, 1.0), l: l.clamp(0.0, 1.0) }
    }

    pub fn to_rgbf(self) -> Rgbf {
        let c = (1.0 - (2.0 * self.l - 1.0).abs()) * self.s;
        hue_to_rgb(self.h, c, self.l - c / 2.0)
    }

    pub fn to_color(self) -> Color {
        from_rgbf(self.to_rgbf())
    }

    pub fn from_rgbf([r, g, b]: Rgbf) -> Self {
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let l = (max + min) / 2.0;
        let s = if d <= 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()).max(f32::EPSILON) };
        Self { h: hue_of(r, g, b, max, d), s: s.clamp(0.0, 1.0), l }
    }

    pub fn from_color(c: Color) -> Self {
        Self::from_rgbf(to_rgbf(c))
    }
}

/// Chroma `c` at hue `h`, plus `m` added to every channel.
fn hue_to_rgb(h: f32, c: f32, m: f32) -> Rgbf {
    let hp = wrap_hue(h) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

fn hue_of(r: f32, g: f32, b: f32, max: f32, d: f32) -> f32 {
    if d <= 0.0 {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    wrap_hue(h * 60.0)
}

// ---- Oklab / Oklch ---------------------------------------------------------------

fn srgb_to_linear(x: f32) -> f32 {
    if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.003_130_8 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

impl Oklab {
    pub fn from_rgbf([r, g, b]: Rgbf) -> Self {
        let (r, g, b) = (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    pub fn from_color(c: Color) -> Self {
        Self::from_rgbf(to_rgbf(c))
    }

    /// Linear-light sRGB, possibly outside 0..=1 when out of gamut.
    fn to_linear(self) -> [f32; 3] {
        let l = self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b;
        let m = self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b;
        let s = self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b;
        let (l, m, s) = (l * l * l, m * m * m, s * s * s);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
            -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    }

    /// The color, or the same lightness and hue with less chroma if it
    /// doesn't fit in sRGB.
    pub fn to_rgbf(self) -> Rgbf {
        Oklch::from(self).to_rgbf()
    }

    pub fn to_color(self) -> Color {
        from_rgbf(self.to_rgbf())
    }
}

impl From<Oklab> for Oklch {
    fn from(o: Oklab) -> Self {
        let c = o.a.hypot(o.b);
        // Below this the hue is numerical noise.
        let h = if c < 1e-4 { 0.0 } else { wrap_hue(o.b.atan2(o.a).to_degrees()) };
        Self { l: o.l, c, h }
    }
}

impl From<Oklch> for Oklab {
    fn from(o: Oklch) -> Self {
        let h = o.h.to_radians();
        Self { l: o.l, a: o.c * h.cos(), b: o.c * h.sin() }
    }
}

const EPS: f32 = 1e-4;

fn in_gamut(rgb: [f32; 3]) -> bool {
    rgb.iter().all(|&x| (-EPS..=1.0 + EPS).contains(&x))
}

impl Oklch {
    pub fn new(l: f32, c: f32, h: f32) -> Self {
        Self { l: l.clamp(0.0, 1.0), c: c.max(0.0), h: wrap_hue(h) }
    }

    pub fn from_rgbf(rgb: Rgbf) -> Self {
        Oklab::from_rgbf(rgb).into()
    }

    pub fn from_color(c: Color) -> Self {
        Self::from_rgbf(to_rgbf(c))
    }

    fn linear_at(self, c: f32) -> [f32; 3] {
        Oklab::from(Self { c, ..self }).to_linear()
    }

    /// The largest chroma at this lightness and hue that still fits in sRGB.
    pub fn max_chroma(l: f32, h: f32) -> f32 {
        let l = l.clamp(0.0, 1.0);
        if l <= 0.0 || l >= 1.0 {
            return 0.0;
        }
        let probe = Self { l, c: 0.0, h };
        let (mut lo, mut hi) = (0.0_f32, 0.5_f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if in_gamut(probe.linear_at(mid)) { lo = mid } else { hi = mid }
        }
        lo
    }

    /// In sRGB (gamut mapped by reducing chroma).
    pub fn to_rgbf(self) -> Rgbf {
        let l = self.l.clamp(0.0, 1.0);
        if l >= 1.0 {
            return [1.0; 3];
        }
        if l <= 0.0 {
            return [0.0; 3];
        }
        let this = Self { l, c: self.c.max(0.0), h: self.h };
        let lin = if in_gamut(this.linear_at(this.c)) {
            this.linear_at(this.c)
        } else {
            let (mut lo, mut hi) = (0.0_f32, this.c);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                if in_gamut(this.linear_at(mid)) { lo = mid } else { hi = mid }
            }
            this.linear_at(lo)
        };
        [linear_to_srgb(lin[0].clamp(0.0, 1.0)), linear_to_srgb(lin[1].clamp(0.0, 1.0)), linear_to_srgb(lin[2].clamp(0.0, 1.0))]
    }

    pub fn to_color(self) -> Color {
        from_rgbf(self.to_rgbf())
    }
}

// ---- Text formats ----------------------------------------------------------------

/// A way to write a color as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorFormat {
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Oklch,
    Oklab,
}

impl ColorFormat {
    pub const ALL: [ColorFormat; 6] = [Self::Hex, Self::Rgb, Self::Hsl, Self::Hsv, Self::Oklch, Self::Oklab];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hex => "Hex",
            Self::Rgb => "RGB",
            Self::Hsl => "HSL",
            Self::Hsv => "HSV",
            Self::Oklch => "OKLCH",
            Self::Oklab => "OKLab",
        }
    }

    /// A short hint of what the format looks like.
    pub fn example(self) -> &'static str {
        match self {
            Self::Hex => "#1a2b3c",
            Self::Rgb => "rgb(26, 43, 60)",
            Self::Hsl => "hsl(210, 40%, 17%)",
            Self::Hsv => "hsv(210, 57%, 24%)",
            Self::Oklch => "oklch(0.29 0.04 250)",
            Self::Oklab => "oklab(0.29 -0.01 -0.04)",
        }
    }

    /// `c` written in this format (parses back to `c`, give or take rounding).
    pub fn format(self, c: Color) -> String {
        match self {
            Self::Hex => c.to_string(),
            Self::Rgb => {
                let (r, g, b) = c.rgb();
                format!("rgb({r}, {g}, {b})")
            }
            Self::Hsl => {
                let h = Hsl::from_color(c);
                format!("hsl({}, {}%, {}%)", number(h.h, 1), number(h.s * 100.0, 1), number(h.l * 100.0, 1))
            }
            Self::Hsv => {
                let h = Hsv::from_color(c);
                format!("hsv({}, {}%, {}%)", number(h.h, 1), number(h.s * 100.0, 1), number(h.v * 100.0, 1))
            }
            Self::Oklch => {
                let o = Oklch::from_color(c);
                format!("oklch({} {} {})", number(o.l, 4), number(o.c, 4), number(o.h, 2))
            }
            Self::Oklab => {
                let o = Oklab::from_color(c);
                format!("oklab({} {} {})", number(o.l, 4), number(o.a, 4), number(o.b, 4))
            }
        }
    }
}

/// `x` with at most `decimals` decimals, no trailing zeros, no `-0`.
pub fn number(x: f32, decimals: usize) -> String {
    let s = format!("{x:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

// ---- Parsing ---------------------------------------------------------------------

/// A unit suffix on a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    None,
    Percent,
    Deg,
    Turn,
    Rad,
    Grad,
}

#[derive(Debug, Clone, Copy)]
struct Num {
    v: f32,
    unit: Unit,
}

impl Num {
    fn parse(t: &str) -> Option<Self> {
        if t == "none" {
            return Some(Self { v: 0.0, unit: Unit::None });
        }
        let (body, unit) = [("%", Unit::Percent), ("deg", Unit::Deg), ("turn", Unit::Turn), ("grad", Unit::Grad), ("rad", Unit::Rad)]
            .into_iter()
            .find_map(|(suffix, unit)| t.strip_suffix(suffix).map(|b| (b, unit)))
            .unwrap_or((t, Unit::None));
        let v: f32 = body.parse().ok()?;
        v.is_finite().then_some(Self { v, unit })
    }

    /// A 0..=1 fraction: `50%` is 0.5, a plain number is already a fraction
    /// when `plain_is_percent` is false, else 0..100.
    fn fraction(self, plain_is_percent: bool) -> f32 {
        match self.unit {
            Unit::Percent => self.v / 100.0,
            _ if plain_is_percent => self.v / 100.0,
            _ => self.v,
        }
    }

    fn degrees(self) -> f32 {
        match self.unit {
            Unit::Turn => self.v * 360.0,
            Unit::Rad => self.v.to_degrees(),
            Unit::Grad => self.v * 0.9,
            _ => self.v,
        }
    }

    /// An Oklab lightness: `70%`, `0.7`, or (leniently) `70`.
    fn lightness(self) -> f32 {
        match self.unit {
            Unit::Percent => self.v / 100.0,
            _ if self.v > 1.0 => self.v / 100.0,
            _ => self.v,
        }
    }

    /// An Oklab a/b/chroma value: `100%` is 0.4.
    fn chroma(self) -> f32 {
        match self.unit {
            Unit::Percent => self.v / 100.0 * CHROMA_100,
            _ => self.v,
        }
    }

    fn channel(self) -> f32 {
        match self.unit {
            Unit::Percent => self.v / 100.0,
            _ => self.v / 255.0,
        }
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let (digits, prefixed) = match s.strip_prefix('#').or_else(|| s.strip_prefix("0x")) {
        Some(d) => (d, true),
        None => (s, false),
    };
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&digits[i..i + 1], 16).ok().map(|n| n * 17);
    let w = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    match (digits.len(), prefixed) {
        (3 | 4, _) if digits.len() == 3 || prefixed => Some(rgb8(v(0)?, v(1)?, v(2)?)),
        (6 | 8, _) if digits.len() == 6 || prefixed => Some(rgb8(w(0)?, w(2)?, w(4)?)),
        _ => None,
    }
}

/// Read a color from text: `#rgb`, `#rrggbb` (also `0x…`, `…aa` alpha
/// ignored), `rgb()`, `hsl()`, `hsv()`/`hsb()`, `oklch()`, `oklab()` in CSS
/// syntax (commas or spaces, `%`/`deg`/`turn` units, `/ alpha`), or three bare
/// numbers like `12, 200, 99` that mean `hint` (RGB when `hint` is
/// [`ColorFormat::Hex`]).
pub fn parse_color(input: &str, hint: ColorFormat) -> Option<Color> {
    let s = input.trim().trim_end_matches(';').trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some(c) = parse_hex(&s) {
        return Some(c);
    }
    let (name, args) = match s.find('(') {
        Some(i) => (s[..i].trim(), s[i + 1..].strip_suffix(')')?),
        None => ("", s.as_str()),
    };
    let format = match name {
        "" => hint,
        "rgb" | "rgba" => ColorFormat::Rgb,
        "hsl" | "hsla" => ColorFormat::Hsl,
        "hsv" | "hsb" | "hsva" => ColorFormat::Hsv,
        "oklch" => ColorFormat::Oklch,
        "oklab" => ColorFormat::Oklab,
        _ => return None,
    };
    let toks: Vec<Num> =
        args.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|t| !t.is_empty()).map(Num::parse).collect::<Option<_>>()?;
    if !(3..=4).contains(&toks.len()) {
        return None;
    }
    let [a, b, c] = [toks[0], toks[1], toks[2]];
    Some(match format {
        ColorFormat::Hex | ColorFormat::Rgb => from_rgbf([a.channel(), b.channel(), c.channel()]),
        ColorFormat::Hsl => Hsl::new(a.degrees(), b.fraction(true), c.fraction(true)).to_color(),
        ColorFormat::Hsv => Hsv::new(a.degrees(), b.fraction(true), c.fraction(true)).to_color(),
        ColorFormat::Oklch => Oklch::new(a.lightness(), b.chroma(), c.degrees()).to_color(),
        ColorFormat::Oklab => Oklab { l: a.lightness().clamp(0.0, 1.0), a: b.chroma(), b: c.chroma() }.to_color(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color, b: Color, tol: i32) -> bool {
        let (a, b) = (a.rgb(), b.rgb());
        (a.0 as i32 - b.0 as i32).abs() <= tol && (a.1 as i32 - b.1 as i32).abs() <= tol && (a.2 as i32 - b.2 as i32).abs() <= tol
    }

    /// A spread of colors: the 8-bit cube corners, grays, and a coarse sweep.
    fn samples() -> Vec<Color> {
        let mut v = vec![Color(0x000000), Color(0xffffff), Color(0x808080), Color(0xff0000), Color(0x00ff00), Color(0x0000ff)];
        for r in (0..=255).step_by(51) {
            for g in (0..=255).step_by(51) {
                for b in (0..=255).step_by(51) {
                    v.push(rgb8(r as u8, g as u8, b as u8));
                }
            }
        }
        v.extend([Color(0x1e1e2e), Color(0xcdd6f4), Color(0x89b4fa), Color(0xf38ba8), Color(0xfabd2f)]);
        v
    }

    #[test]
    fn hsv_and_hsl_round_trip() {
        for c in samples() {
            assert_eq!(Hsv::from_color(c).to_color(), c, "hsv {c}");
            assert_eq!(Hsl::from_color(c).to_color(), c, "hsl {c}");
        }
    }

    #[test]
    fn hsv_known_values() {
        let h = Hsv::from_color(Color(0xff8000));
        assert!((h.h - 30.0).abs() < 0.2 && h.s == 1.0 && h.v == 1.0, "{h:?}");
        assert_eq!(Hsv::new(120.0, 1.0, 1.0).to_color(), Color(0x00ff00));
        assert_eq!(Hsv::new(240.0, 0.5, 1.0).to_color(), Color(0x8080ff));
        assert_eq!(Hsl::new(0.0, 1.0, 0.5).to_color(), Color(0xff0000));
        // Achromatic: no hue to speak of, saturation 0.
        let g = Hsv::from_color(Color(0x777777));
        assert_eq!((g.h, g.s), (0.0, 0.0));
    }

    #[test]
    fn oklab_round_trip_and_references() {
        for c in samples() {
            let back = Oklab::from_color(c).to_color();
            assert!(close(back, c, 1), "oklab {c} -> {back}");
            let back = Oklch::from_color(c).to_color();
            assert!(close(back, c, 1), "oklch {c} -> {back}");
        }
        let white = Oklab::from_color(Color(0xffffff));
        assert!((white.l - 1.0).abs() < 1e-3 && white.a.abs() < 1e-3 && white.b.abs() < 1e-3, "{white:?}");
        // Reference values (CSS Color 4): red is oklch(0.628 0.2577 29.23).
        let red = Oklch::from_color(Color(0xff0000));
        assert!((red.l - 0.628).abs() < 2e-3 && (red.c - 0.2577).abs() < 2e-3 && (red.h - 29.23).abs() < 0.2, "{red:?}");
    }

    #[test]
    fn out_of_gamut_keeps_lightness_and_hue() {
        let wild = Oklch::new(0.7, 0.45, 150.0);
        let c = wild.to_color();
        let back = Oklch::from_color(c);
        assert!(back.c < wild.c);
        assert!((back.l - 0.7).abs() < 0.02, "{back:?}");
        assert!((back.h - 150.0).abs() < 3.0, "{back:?}");
        assert_eq!(Oklch::new(1.2, 0.1, 10.0).to_color(), Color(0xffffff));
        assert_eq!(Oklch::new(-1.0, 0.1, 10.0).to_color(), Color(0x000000));
        // max_chroma sits right at the edge of the gamut.
        let m = Oklch::max_chroma(0.7, 150.0);
        assert!(in_gamut(Oklch { l: 0.7, c: m, h: 150.0 }.linear_at(m)));
        assert!(!in_gamut(Oklch { l: 0.7, c: m, h: 150.0 }.linear_at(m + 0.01)));
        assert_eq!(Oklch::max_chroma(0.0, 10.0), 0.0);
        assert_eq!(Oklch::max_chroma(1.0, 10.0), 0.0);
    }

    #[test]
    fn every_format_reads_back() {
        for c in samples() {
            for f in ColorFormat::ALL {
                let text = f.format(c);
                let back = parse_color(&text, f).unwrap_or_else(|| panic!("{f:?}: {text:?} doesn't parse"));
                // Hex and rgb() are exact; the rest are rounded to a few decimals.
                assert!(
                    close(back, c, if matches!(f, ColorFormat::Hex | ColorFormat::Rgb) { 0 } else { 1 }),
                    "{f:?} {text}: {c} -> {back}"
                );
            }
        }
    }

    #[test]
    fn parses_hex_variants() {
        for (text, want) in [
            ("#1a2b3c", 0x1a2b3c),
            ("1A2B3C", 0x1a2b3c),
            ("  #1a2b3c  ", 0x1a2b3c),
            ("#abc", 0xaabbcc),
            ("abc", 0xaabbcc),
            ("#1a2b3c80", 0x1a2b3c),
            ("#abcd", 0xaabbcc),
            ("0x1a2b3c", 0x1a2b3c),
            ("#1a2b3c;", 0x1a2b3c),
        ] {
            assert_eq!(parse_color(text, ColorFormat::Hex), Some(Color(want)), "{text}");
        }
        for text in ["", "#", "#12", "#12345", "#1234567", "12345", "#ggg", "red", "#1a2b3c4d5e"] {
            assert_eq!(parse_color(text, ColorFormat::Hex), None, "{text:?}");
        }
    }

    #[test]
    fn parses_css_functions() {
        let p = |s: &str| parse_color(s, ColorFormat::Hex);
        assert_eq!(p("rgb(255, 128, 0)"), Some(Color(0xff8000)));
        assert_eq!(p("rgb(255 128 0)"), Some(Color(0xff8000)));
        assert_eq!(p("RGB(255,128,0)"), Some(Color(0xff8000)));
        assert_eq!(p("rgba(255, 128, 0, 0.5)"), Some(Color(0xff8000)));
        assert_eq!(p("rgb(255 128 0 / 50%)"), Some(Color(0xff8000)));
        assert_eq!(p("rgb(100%, 50%, 0%)"), Some(Color(0xff8000)));
        assert_eq!(p("rgb(300, -5, 0)"), Some(Color(0xff0000)), "clamped");
        assert_eq!(p("hsl(0, 100%, 50%)"), Some(Color(0xff0000)));
        assert_eq!(p("hsl(120deg 100% 25%)"), Some(Color(0x008000)));
        assert_eq!(p("hsl(0.5turn 100% 50%)"), Some(Color(0x00ffff)));
        assert_eq!(p("hsl(-120, 100%, 50%)"), Some(Color(0x0000ff)), "negative hues wrap");
        assert_eq!(p("hsv(240, 100%, 100%)"), Some(Color(0x0000ff)));
        assert_eq!(p("hsb(240 50 100)"), Some(Color(0x8080ff)), "plain s/v are percentages");
        assert_eq!(p("oklch(100% 0 0)"), Some(Color(0xffffff)));
        assert_eq!(p("oklch(0 0 0)"), Some(Color(0x000000)));
        assert!(close(p("oklch(0.628 0.2577 29.23)").unwrap(), Color(0xff0000), 1));
        assert!(close(p("oklch(62.8% 64.4% 29.23deg)").unwrap(), Color(0xff0000), 1), "100% chroma is 0.4");
        assert!(close(p("oklch(62.8 0.2577 29.23)").unwrap(), Color(0xff0000), 1), "lightness 0..100 is tolerated");
        assert!(close(p("oklab(0.628 0.2249 0.1258)").unwrap(), Color(0xff0000), 1));
        assert_eq!(p("oklch(none none none)"), Some(Color(0x000000)));
    }

    #[test]
    fn parses_bare_numbers_by_hint() {
        assert_eq!(parse_color("255, 128, 0", ColorFormat::Rgb), Some(Color(0xff8000)));
        assert_eq!(parse_color("255 128 0", ColorFormat::Hex), Some(Color(0xff8000)));
        assert_eq!(parse_color("(255, 128, 0)", ColorFormat::Rgb), Some(Color(0xff8000)));
        assert_eq!(parse_color("240 100% 100%", ColorFormat::Hsv), Some(Color(0x0000ff)));
        assert_eq!(parse_color("240 100 50", ColorFormat::Hsl), Some(Color(0x0000ff)));
        assert_eq!(parse_color("1 0 0", ColorFormat::Oklch), Some(Color(0xffffff)));
    }

    #[test]
    fn rejects_junk() {
        for text in [
            "rgb(1, 2)",
            "rgb(1, 2, 3, 4, 5)",
            "rgb(1, 2, x)",
            "rgb(1, 2, 3",
            "foo(1, 2, 3)",
            "1 2",
            "nan nan nan",
            "inf 0 0",
            "hsl(a b c)",
            "hello world",
        ] {
            assert_eq!(parse_color(text, ColorFormat::Rgb), None, "{text:?}");
        }
    }

    #[test]
    fn formats_are_tidy() {
        let c = Color(0xff8000);
        assert_eq!(ColorFormat::Hex.format(c), "#ff8000");
        assert_eq!(ColorFormat::Rgb.format(c), "rgb(255, 128, 0)");
        assert_eq!(ColorFormat::Hsl.format(Color(0xff0000)), "hsl(0, 100%, 50%)");
        assert_eq!(ColorFormat::Hsv.format(Color(0x000000)), "hsv(0, 0%, 0%)");
        assert_eq!(ColorFormat::Oklch.format(Color(0xffffff)), "oklch(1 0 0)");
        assert!(!ColorFormat::Oklab.format(Color(0x000000)).contains("-0"));
        for f in ColorFormat::ALL {
            assert!(parse_color(f.example(), f).is_some(), "{f:?} example {:?}", f.example());
        }
    }
}
