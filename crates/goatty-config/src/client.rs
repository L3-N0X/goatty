//! Client settings (`settings.json` on the machine running the window):
//! how goatty looks and behaves locally, whichever host it's connected to.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::keys::{self, find_action};
use crate::store::{Document, Issue, Migration};
use crate::theme::ThemeSettings;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ClientSettings {
    pub font: FontSettings,
    pub terminal: TerminalSettings,
    pub theme: ThemeSettings,
    /// Keystroke → action name, or `null` to unbind a default
    /// (see [`keys::effective_bindings`]).
    pub keybindings: BTreeMap<String, Option<String>>,
    pub workspace: WorkspaceSettings,
    pub debug: DebugSettings,
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
    pub const MIN_LINE_HEIGHT: f32 = 1.0;
    pub const MAX_LINE_HEIGHT: f32 = 2.0;

    /// The family to use, given the names of installed fonts.
    pub fn resolve_family(&self, installed: &[String]) -> String {
        self.family
            .clone()
            .filter(|f| !f.is_empty())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub renderer: Renderer,
    /// Padding around the grid in logical pixels.
    pub padding: f32,
    /// Cursor shape when the application doesn't ask for one.
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
}

impl TerminalSettings {
    pub const MAX_PADDING: f32 = 48.0;
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self { renderer: Renderer::Glyphs, padding: 6.0, cursor_shape: CursorShape::Block, cursor_blink: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnLaunch {
    /// Reattach the newest detached session, else start one.
    #[default]
    Reattach,
    /// Always start a new session.
    New,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WorkspaceSettings {
    pub on_launch: OnLaunch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DebugSettings {
    /// Print per-second frame statistics to stderr.
    pub stats: bool,
}

impl Document for ClientSettings {
    const NAME: &'static str = "settings";
    const VERSION: u32 = 1;
    const OPEN_MAPS: &'static [&'static str] = &["keybindings"];

    fn migrations() -> &'static [Migration] {
        &[]
    }

    fn validate(&mut self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let f = &mut self.font;
        clamp(&mut f.size, FontSettings::MIN_SIZE, FontSettings::MAX_SIZE, "font.size", &mut issues);
        clamp(&mut f.line_height, FontSettings::MIN_LINE_HEIGHT, FontSettings::MAX_LINE_HEIGHT, "font.line_height", &mut issues);
        clamp(&mut self.terminal.padding, 0.0, TerminalSettings::MAX_PADDING, "terminal.padding", &mut issues);
        // Theme names are checked against the theme library (user themes
        // live in their own files): `ThemeLibrary::resolve`.
        if self.theme.overrides.ansi.len() > 16 {
            issues.push(Issue::new("theme.overrides.ansi", "only 16 ANSI colors exist; the rest are ignored"));
            self.theme.overrides.ansi.truncate(16);
        }
        for (key, action) in &self.keybindings {
            if let Some(a) = action
                && find_action(a).is_none()
            {
                issues.push(Issue::new(format!("keybindings.{key}"), format!("unknown action {a:?}")));
            }
            if keys::normalize_keystroke(key).is_empty() {
                issues.push(Issue::new("keybindings", "empty keystroke"));
            }
        }
        issues
    }
}

/// Clamp `v` into `[lo, hi]`, reporting a change. NaN becomes `lo`.
pub(crate) fn clamp(v: &mut f32, lo: f32, hi: f32, path: &str, issues: &mut Vec<Issue>) {
    let c = if v.is_nan() { lo } else { v.clamp(lo, hi) };
    if c != *v {
        issues.push(Issue::new(path, format!("{v} is outside {lo}–{hi}; using {c}")));
        *v = c;
    }
}

impl ClientSettings {
    /// Apply `GOATTY_*` overrides read through `var`. Unparsable values are
    /// ignored so a typo never prevents the terminal from starting.
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
        if let Some(t) = var("GOATTY_THEME").filter(|t| !t.is_empty()) {
            self.theme.name = t;
            self.theme.follow_system = false;
        }
        if var("GOATTY_STATS").is_some() {
            self.debug.stats = true;
        }
    }

    /// Names of the `GOATTY_*` variables currently overriding settings.
    pub fn env_overrides(var: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
        ["GOATTY_FONT", "GOATTY_FONT_SIZE", "GOATTY_RENDER", "GOATTY_THEME", "GOATTY_STATS"]
            .into_iter()
            .filter(|k| var(k).is_some())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> ClientSettings {
        let mut s = ClientSettings::default();
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
        assert_eq!(with(&[("GOATTY_THEME", "nord")]).theme.name, "nord");
        assert!(!with(&[("GOATTY_THEME", "nord")]).theme.follow_system, "an explicit theme wins over following the system");
    }

    #[test]
    fn bad_env_values_keep_defaults() {
        let s = with(&[("GOATTY_FONT_SIZE", "huge"), ("GOATTY_RENDER", "fancy"), ("GOATTY_FONT", "")]);
        assert_eq!(s, ClientSettings::default());
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

    #[test]
    fn validation_clamps_and_reports() {
        let mut s = ClientSettings::default();
        s.font.size = 400.0;
        s.terminal.padding = -3.0;
        s.theme.name = "nope".into();
        s.keybindings.insert("ctrl-x".into(), Some("explode".into()));
        let issues = s.validate();
        assert_eq!(s.font.size, FontSettings::MAX_SIZE);
        assert_eq!(s.terminal.padding, 0.0);
        let paths: Vec<_> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["font.size", "terminal.padding", "keybindings.ctrl-x"]);
        assert!(ClientSettings::default().validate().is_empty());
    }
}
