//! goatty settings.
//!
//! One typed [`Settings`] value that the app and the daemon read instead of
//! poking at environment variables all over the code. Today it comes from
//! built-in defaults plus `GOATTY_*` environment overrides. The Stage 4
//! profile engine (versioned JSON profiles, settings GUI) plugs in here:
//! it produces a `Settings`, and the rest of the code doesn't change.
//!
//! | Variable           | Setting                                  |
//! |--------------------|------------------------------------------|
//! | `GOATTY_FONT`      | [`FontSettings::family`]                 |
//! | `GOATTY_FONT_SIZE` | [`FontSettings::size`]                   |
//! | `GOATTY_RENDER`    | [`TerminalSettings::renderer`] (`shaped` or `glyphs`) |
//! | `GOATTY_SHELL`     | [`ShellSettings::program`]               |
//! | `GOATTY_STATS`     | [`DebugSettings::stats`] (any value)     |
//!
//! `GOATTY_SOCKET` is not a setting: it says where a daemon lives and is
//! handled by `goatty_proto::socket_path`.

use serde::{Deserialize, Serialize};

/// Bumped when a stored settings document needs migrating.
pub const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub font: FontSettings,
    pub terminal: TerminalSettings,
    pub shell: ShellSettings,
    pub debug: DebugSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            font: FontSettings::default(),
            terminal: TerminalSettings::default(),
            shell: ShellSettings::default(),
            debug: DebugSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSettings {
    /// Explicit family. `None`: the first installed entry of `fallbacks`.
    pub family: Option<String>,
    /// Preferred monospace families, first installed one wins.
    pub fallbacks: Vec<String>,
    /// Size in logical pixels.
    pub size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        let fallbacks = [
            "JetBrainsMono Nerd Font",
            "JetBrains Mono",
            "FiraCode Nerd Font Mono",
            "FiraCode Nerd Font",
            "CommitMono Nerd Font Mono",
            "Fira Code",
            "DejaVu Sans Mono",
            "Adwaita Mono",
            "Noto Sans Mono",
            "Liberation Mono",
        ];
        Self { family: None, fallbacks: fallbacks.map(String::from).to_vec(), size: 14.0, line_height: 1.25 }
    }
}

impl FontSettings {
    pub const MIN_SIZE: f32 = 6.0;
    pub const MAX_SIZE: f32 = 48.0;

    /// The family to use, given the names of installed fonts.
    pub fn resolve_family(&self, installed: &[String]) -> String {
        self.family
            .clone()
            .or_else(|| self.fallbacks.iter().find(|f| installed.iter().any(|i| i == *f)).cloned())
            .unwrap_or_else(|| "monospace".into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    /// Per-cell glyph cache: fastest, no ligatures.
    #[default]
    Glyphs,
    /// Per-segment line shaping: ligatures, slower when many lines change.
    Shaped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub renderer: Renderer,
    /// Padding around the grid in logical pixels.
    pub padding: f32,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self { renderer: Renderer::Glyphs, padding: 6.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ShellSettings {
    /// Shell program. `None`: zsh if installed, else `$SHELL`, else /bin/sh.
    pub program: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DebugSettings {
    /// Print per-second frame statistics to stderr.
    pub stats: bool,
}

impl Settings {
    /// Defaults plus `GOATTY_*` overrides from the process environment.
    pub fn from_env() -> Self {
        let mut s = Self::default();
        s.apply_env(|k| std::env::var(k).ok());
        s
    }

    /// Apply environment overrides read through `var` (injectable for tests).
    /// Unparsable values are ignored (the current value stays), so a typo
    /// never prevents the terminal from starting.
    pub fn apply_env(&mut self, var: impl Fn(&str) -> Option<String>) {
        if let Some(f) = var("GOATTY_FONT").filter(|f| !f.is_empty()) {
            self.font.family = Some(f);
        }
        if let Some(size) = var("GOATTY_FONT_SIZE").and_then(|s| s.parse::<f32>().ok()) {
            self.font.size = size.clamp(FontSettings::MIN_SIZE, FontSettings::MAX_SIZE);
        }
        match var("GOATTY_RENDER").as_deref() {
            Some("shaped") => self.terminal.renderer = Renderer::Shaped,
            Some("glyphs") => self.terminal.renderer = Renderer::Glyphs,
            _ => {}
        }
        if let Some(sh) = var("GOATTY_SHELL").filter(|s| !s.is_empty()) {
            self.shell.program = Some(sh);
        }
        if var("GOATTY_STATS").is_some() {
            self.debug.stats = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> Settings {
        let mut s = Settings::default();
        s.apply_env(|k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()));
        s
    }

    #[test]
    fn env_overrides() {
        let s = with(&[("GOATTY_FONT", "Iosevka"), ("GOATTY_FONT_SIZE", "16"), ("GOATTY_RENDER", "shaped"), ("GOATTY_STATS", "1")]);
        assert_eq!(s.font.family.as_deref(), Some("Iosevka"));
        assert_eq!(s.font.size, 16.0);
        assert_eq!(s.terminal.renderer, Renderer::Shaped);
        assert!(s.debug.stats);
        assert_eq!(s.shell.program, None);
    }

    #[test]
    fn bad_values_keep_defaults() {
        let s = with(&[("GOATTY_FONT_SIZE", "huge"), ("GOATTY_RENDER", "fancy"), ("GOATTY_FONT", "")]);
        assert_eq!(s, Settings::default());
        assert_eq!(with(&[("GOATTY_FONT_SIZE", "500")]).font.size, FontSettings::MAX_SIZE);
    }

    #[test]
    fn font_resolution() {
        let f = FontSettings::default();
        assert_eq!(f.resolve_family(&["Fira Code".into(), "DejaVu Sans Mono".into()]), "Fira Code");
        assert_eq!(f.resolve_family(&[]), "monospace");
        let explicit = FontSettings { family: Some("Iosevka".into()), ..f };
        assert_eq!(explicit.resolve_family(&[]), "Iosevka");
    }
}
