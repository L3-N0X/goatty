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
//! | `GOATTY_SHELL_MANAGED` | [`ShellSettings::managed`] (`0`/`1`) |
//! | `GOATTY_ZSH_USER_RC` | [`ZshSettings::user_rc`] (`0`/`1`)     |
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellSettings {
    /// Shell program. `None`: zsh if installed, else `$SHELL`, else /bin/sh.
    pub program: Option<String>,
    /// Run zsh with goatty's managed configuration (plugins, prompt,
    /// history, shell integration; see `goatty-shell`). `false`: plain zsh
    /// with only the user's own dotfiles. Other shells are always launched
    /// as they are.
    pub managed: bool,
    pub zsh: ZshSettings,
}

impl Default for ShellSettings {
    fn default() -> Self {
        Self { program: None, managed: true, zsh: ZshSettings::default() }
    }
}

/// What goatty's managed zsh loads. Each feature is skipped quietly when
/// its tool isn't installed (`goattyd shell status` shows what's missing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZshSettings {
    /// Source the user's `.zshenv` and `.zprofile` (environment: PATH,
    /// EDITOR, ...). Zsh reads these for every terminal, so this is on.
    pub user_env: bool,
    /// Also source the user's `.zshrc`. Off: most `.zshrc` files load their
    /// own prompt and plugins, which would fight goatty's.
    pub user_rc: bool,
    pub autosuggestions: bool,
    pub syntax_highlighting: bool,
    /// Starship prompt. Off, or when missing: a small built-in prompt.
    pub starship: bool,
    /// Atuin history search on Ctrl+R.
    pub atuin: bool,
    /// fzf key bindings (Ctrl+T, Alt+C) when fzf is installed.
    pub fzf: bool,
    /// OSC 133 prompt marks and OSC 7 working directory reports.
    pub integration: bool,
}

impl Default for ZshSettings {
    fn default() -> Self {
        Self {
            user_env: true,
            user_rc: false,
            autosuggestions: true,
            syntax_highlighting: true,
            starship: true,
            atuin: true,
            fzf: true,
            integration: true,
        }
    }
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
        if let Some(b) = var("GOATTY_SHELL_MANAGED").as_deref().and_then(parse_bool) {
            self.shell.managed = b;
        }
        if let Some(b) = var("GOATTY_ZSH_USER_RC").as_deref().and_then(parse_bool) {
            self.shell.zsh.user_rc = b;
        }
        if var("GOATTY_STATS").is_some() {
            self.debug.stats = true;
        }
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
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
        assert!(s.shell.managed);
        assert!(!s.shell.zsh.user_rc);
    }

    #[test]
    fn shell_toggles() {
        let s = with(&[("GOATTY_SHELL_MANAGED", "0"), ("GOATTY_ZSH_USER_RC", "yes")]);
        assert!(!s.shell.managed);
        assert!(s.shell.zsh.user_rc);
        assert!(with(&[("GOATTY_SHELL_MANAGED", "maybe")]).shell.managed);
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
