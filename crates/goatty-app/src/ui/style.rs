//! UI colors derived from the active terminal theme, so goatty's windows
//! match whatever theme the user picked, plus the type scale, radii and
//! spacing every goatty window shares.
//!
//! The colors follow Material 3's tonal surfaces: one background, then
//! containers a little closer to the foreground (cards, controls), an
//! accent for what's selected or primary, and a soft accent container for
//! selected-but-quiet things (the sidebar's current page, chips).

use goatty_config::{Color, ThemeColors};
use gpui::{App, Hsla, rgb};

use crate::settings::AppSettings;

/// Type scale (px). Hierarchy comes from size *and* weight: a section
/// heading is never smaller than the labels under it.
pub mod text {
    /// Page title.
    pub const TITLE: f32 = 24.0;
    /// A section heading above a group, a dialog's title.
    pub const HEADING: f32 = 15.0;
    /// Row labels, body text, inputs.
    pub const BODY: f32 = 14.0;
    /// Buttons, segmented controls, menu items.
    pub const LABEL: f32 = 13.0;
    /// Descriptions under labels, notes.
    pub const SMALL: f32 = 12.5;
    /// Badges, keycaps, captions.
    pub const CAPTION: f32 = 11.5;
}

/// Corner radii (px).
pub mod radius {
    /// Cards / groups, dialogs.
    pub const CARD: f32 = 14.0;
    /// Inputs, menus, list items.
    pub const CONTROL: f32 = 10.0;
    /// Keycaps, swatches, small chips.
    pub const SMALL: f32 = 6.0;
}

#[derive(Debug, Clone, Copy)]
pub struct Ui {
    pub dark: bool,
    /// Window background (the page under the cards).
    pub bg: Hsla,
    /// The settings sidebar, dialogs' footers.
    pub sidebar: Hsla,
    /// Cards / preference groups.
    pub card: Hsla,
    /// Generic raised surface (popovers use `menu`).
    pub surface: Hsla,
    pub surface_hover: Hsla,
    /// Filled controls on a card: segmented track, steppers, tonal buttons.
    pub field: Hsla,
    pub field_hover: Hsla,
    pub input_bg: Hsla,
    /// Popover menus and tooltips.
    pub menu: Hsla,
    /// Lines inside a card (between rows).
    pub divider: Hsla,
    /// Outlines (inputs, unselected controls).
    pub border: Hsla,
    pub text: Hsla,
    /// Secondary text: descriptions.
    pub muted: Hsla,
    /// Tertiary text: placeholders, captions, disabled.
    pub faint: Hsla,
    pub accent: Hsla,
    /// Text on an accent background.
    pub on_accent: Hsla,
    /// A quiet accent container (selected nav item, chips).
    pub accent_soft: Hsla,
    /// Accent-colored text that reads on `bg` and `accent_soft`.
    pub accent_text: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    /// Behind dialogs.
    pub scrim: Hsla,
}

/// `a` moved towards `b` by `t` (0..1), in RGB.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = a.rgb();
    let (br, bg, bb) = b.rgb();
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u32;
    Color((m(ar, br) << 16) | (m(ag, bg) << 8) | m(ab, bb))
}

pub fn h(c: Color) -> Hsla {
    rgb(c.0).into()
}

impl Ui {
    pub fn from_theme(c: &ThemeColors) -> Self {
        let (bg, fg) = (c.background, c.foreground);
        let dark = c.is_dark();
        let accent = c.accent();
        let tone = |t: f32| h(mix(bg, fg, t));
        // Light themes: the page sits a step darker and cards are the
        // theme's background, like Material's light surfaces.
        let (page, card) = if dark { (bg, mix(bg, fg, 0.055)) } else { (mix(bg, fg, 0.045), bg) };
        let card_c = card;
        let accent_text = if dark { mix(accent, fg, 0.15) } else { mix(accent, Color(0), 0.12) };
        Self {
            dark,
            bg: h(page),
            sidebar: if dark { tone(0.03) } else { tone(0.07) },
            card: h(card),
            surface: h(mix(card_c, fg, 0.03)),
            surface_hover: h(mix(card_c, fg, if dark { 0.07 } else { 0.06 })),
            field: h(mix(card_c, fg, if dark { 0.07 } else { 0.06 })),
            field_hover: h(mix(card_c, fg, if dark { 0.12 } else { 0.1 })),
            input_bg: h(mix(card_c, fg, if dark { 0.03 } else { 0.0 })),
            menu: h(mix(card_c, fg, if dark { 0.06 } else { 0.0 })),
            divider: h(mix(card_c, fg, if dark { 0.075 } else { 0.08 })),
            border: h(mix(card_c, fg, if dark { 0.16 } else { 0.18 })),
            text: h(fg),
            muted: h(mix(bg, fg, 0.66)),
            faint: h(mix(bg, fg, 0.45)),
            accent: h(accent),
            on_accent: h(if accent.luma() > 0.55 { Color(0x101010) } else { Color(0xffffff) }),
            accent_soft: h(mix(page, accent, if dark { 0.2 } else { 0.16 })),
            accent_text: h(accent_text),
            success: h(c.ansi[2]),
            warning: h(c.ansi[3]),
            danger: h(c.ansi[1]),
            scrim: Hsla { h: 0.0, s: 0.0, l: 0.0, a: if dark { 0.5 } else { 0.32 } },
        }
    }

    /// Colors for the current theme.
    pub fn get(cx: &App) -> Self {
        Self::from_theme(&cx.global::<AppSettings>().colors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_accent_defaults_to_ansi_blue_and_honors_override() {
        let mut colors = goatty_config::find_theme("goatty-night").unwrap().colors;
        let default_ui = Ui::from_theme(&colors);
        assert_eq!(default_ui.accent, h(colors.ansi[4]));

        let green = colors.ansi[2];
        colors.accent = Some(green);
        let green_ui = Ui::from_theme(&colors);
        assert_eq!(green_ui.accent, h(green));
        assert_ne!(green_ui.accent, default_ui.accent);
        assert_eq!(green_ui.accent_soft, h(mix(colors.background, green, 0.2)));
        assert_eq!(green_ui.accent_text, h(mix(green, colors.foreground, 0.15)));
        assert_eq!(green_ui.on_accent, h(Color(0x101010)), "bright green uses dark text on accent");
    }
}
