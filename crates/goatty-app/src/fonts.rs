//! The bundled JetBrainsMono Nerd Font (`assets/fonts`): registered at start
//! so Nerd Font symbols (prompt, tab icons) work without installing a font.

use std::borrow::Cow;

use gpui::{App, Font, FontFallbacks};

pub use goatty_config::FontSettings;

/// The faces the terminal uses (regular, bold, italic, bold italic).
const FACES: &[&[u8]] = &[
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-BoldItalic.ttf"),
];

/// Register the bundled faces. Returns the font families installed on the
/// system, read before registering (font resolution prefers an installed
/// Nerd Font over the bundled one).
pub fn register(cx: &App) -> Vec<String> {
    let system = cx.text_system().all_font_names();
    if let Err(e) = cx.text_system().add_fonts(FACES.iter().map(|f| Cow::Borrowed(*f)).collect()) {
        log::error!("couldn't load the bundled {}: {e:#}", FontSettings::BUNDLED);
    }
    system
}

/// The terminal font for `family`: glyphs it lacks (Nerd Font symbols in a
/// plain font) come from the bundled font.
pub fn terminal(family: &str) -> Font {
    let mut font = gpui::font(family.to_string());
    if family != FontSettings::BUNDLED {
        font.fallbacks = Some(FontFallbacks::from_fonts(vec![FontSettings::BUNDLED.to_string()]));
    }
    font
}

/// The font for Nerd Font icons in goatty's own UI: always the bundled one,
/// whose code points are checked by the tests in `workspace::icons`.
pub fn icons() -> Font {
    gpui::font(FontSettings::BUNDLED)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// The bundled faces parse and are the family the settings resolve to.
    #[test]
    fn bundled_faces() {
        for data in FACES {
            let face = ttf_parser::Face::parse(data, 0).expect("bundled font parses");
            let family = face
                .names()
                .into_iter()
                .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
                .and_then(|n| n.to_string())
                .expect("family name");
            // Bold/Italic faces name their family "JetBrainsMono Nerd Font"
            // plus the style in the legacy name; the typographic family is
            // the plain one.
            let typographic = face
                .names()
                .into_iter()
                .find(|n| n.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY && n.is_unicode())
                .and_then(|n| n.to_string());
            assert_eq!(typographic.as_deref().unwrap_or(&family), FontSettings::BUNDLED);
        }
    }

    /// Whether the bundled regular face has a glyph for `c`.
    pub fn has_glyph(c: char) -> bool {
        ttf_parser::Face::parse(FACES[0], 0).expect("bundled font parses").glyph_index(c).is_some()
    }
}
