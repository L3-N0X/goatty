//! UI colors derived from the active terminal theme, so goatty's windows
//! match whatever theme the user picked.

use goatty_config::{Color, ThemeColors};
use gpui::{App, Hsla, rgb};

use crate::settings::AppSettings;

#[derive(Debug, Clone, Copy)]
pub struct Ui {
    pub bg: Hsla,
    /// Sidebar, cards.
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub input_bg: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    /// Text on an accent background.
    pub on_accent: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
}

/// `a` moved towards `b` by `t` (0..1), in RGB.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = a.rgb();
    let (br, bg, bb) = b.rgb();
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u32;
    Color((m(ar, br) << 16) | (m(ag, bg) << 8) | m(ab, bb))
}

fn h(c: Color) -> Hsla {
    rgb(c.0).into()
}

impl Ui {
    pub fn from_theme(c: &ThemeColors) -> Self {
        let (bg, fg) = (c.background, c.foreground);
        let dark = c.is_dark();
        let accent = c.ansi[4];
        Self {
            bg: h(bg),
            surface: h(mix(bg, fg, 0.05)),
            surface_hover: h(mix(bg, fg, 0.11)),
            input_bg: h(mix(bg, fg, if dark { 0.03 } else { 0.0 })),
            border: h(mix(bg, fg, 0.16)),
            text: h(fg),
            muted: h(mix(bg, fg, 0.6)),
            accent: h(accent),
            on_accent: h(if accent.luma() > 0.55 { Color(0x101010) } else { Color(0xffffff) }),
            success: h(c.ansi[2]),
            warning: h(c.ansi[3]),
            danger: h(c.ansi[1]),
        }
    }

    /// Colors for the current theme.
    pub fn get(cx: &App) -> Self {
        Self::from_theme(&cx.global::<AppSettings>().client.theme.colors())
    }
}
