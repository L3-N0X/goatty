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
    pub tabs: TabSettings,
    pub panes: PaneSettings,
    pub links: LinkSettings,
    pub paste: PasteSettings,
    pub selection: SelectionSettings,
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
    /// Opacity of the terminal background (0.1–1.0). Text and colored
    /// cell backgrounds stay opaque. Below 1 the window is transparent,
    /// which needs a compositor.
    pub opacity: f32,
    /// Blur what shows through a transparent background, where the
    /// desktop supports it (KDE, macOS, Windows).
    pub blur: bool,
}

impl TerminalSettings {
    pub const MAX_PADDING: f32 = 48.0;
    pub const MIN_OPACITY: f32 = 0.1;

    /// Whether the window needs to be transparent.
    pub fn is_transparent(&self) -> bool {
        self.opacity < 1.0
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self { renderer: Renderer::Glyphs, padding: 6.0, cursor_shape: CursorShape::Block, cursor_blink: true, opacity: 1.0, blur: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnLaunch {
    /// Restore the last closed window's tabs and splits (else reattach the
    /// newest detached session, else start one).
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

/// How the tab bar looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TabStyle {
    /// Browser-like tabs with rounded tops, icons and close buttons.
    #[default]
    Modern,
    /// One line of small text, like kitty's tab bar.
    Compact,
}

/// When the tab bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TabBarShow {
    #[default]
    Always,
    /// Only with two tabs or more.
    Multiple,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabSettings {
    pub style: TabStyle,
    pub show: TabBarShow,
    /// Icon of the running program (Nerd Font glyphs, when a Nerd Font is
    /// installed).
    pub icons: bool,
}

impl Default for TabSettings {
    fn default() -> Self {
        Self { style: TabStyle::Modern, show: TabBarShow::Always, icons: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneSettings {
    /// How much panes without focus fade (0 = not at all), when a tab is split.
    pub dim_inactive: f32,
    /// Ask before closing a pane or tab whose shell is running a program
    /// (a build, an editor, an SSH session).
    pub confirm_close: bool,
}

impl PaneSettings {
    pub const MAX_DIM: f32 = 0.6;
}

impl Default for PaneSettings {
    fn default() -> Self {
        Self { dim_inactive: 0.2, confirm_close: true }
    }
}

/// How links in the terminal (URLs, file paths, OSC 8 hyperlinks) open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkClick {
    /// Ctrl+click (Cmd+click on macOS); a plain click still selects.
    #[default]
    CtrlClick,
    /// A click without dragging opens it.
    Click,
    /// Links aren't detected.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LinkSettings {
    pub open_on: LinkClick,
    /// Command that opens files, e.g. `code --goto {path}:{line}:{column}`.
    /// `{path}`, `{line}` and `{column}` are filled in per argument (no
    /// shell involved); without `{path}` the path is appended. `None`:
    /// the desktop's default application.
    pub file_opener: Option<String>,
}

impl LinkSettings {
    /// The opener's argv for a file, or `None` to use the desktop default.
    pub fn file_command(&self, path: &str, line: Option<u32>, column: Option<u32>) -> Option<Vec<String>> {
        let template = self.file_opener.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
        let words = split_words(template)?;
        let (line, column) = (line.unwrap_or(1).to_string(), column.unwrap_or(1).to_string());
        let mut has_path = false;
        let mut argv: Vec<String> = words
            .iter()
            .map(|w| {
                has_path |= w.contains("{path}");
                w.replace("{path}", path).replace("{line}", &line).replace("{column}", &column)
            })
            .collect();
        if !has_path {
            argv.push(path.to_string());
        }
        Some(argv)
    }
}

/// Split a command line into words: whitespace separates, '…' and "…"
/// quote, backslash escapes. `None` for an unterminated quote.
fn split_words(s: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => cur.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => cur.push(chars.next()?),
                        c => cur.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                cur.push(chars.next()?);
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Some(words)
}

/// When pasting asks first (see `goatty_term::paste::check`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PasteConfirm {
    /// Never ask.
    Never,
    /// When the paste would run commands right away, hides control or
    /// invisible characters, pipes a download into a shell, or is huge.
    #[default]
    Suspicious,
    /// Also for every paste of more than one line.
    Multiline,
}

/// What a middle click pastes (in programs without mouse reporting, or
/// with Shift held).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MiddleClick {
    /// The primary selection: the text last selected in any app (Linux).
    /// Where there's no primary selection, the clipboard.
    #[default]
    Primary,
    /// The clipboard.
    Clipboard,
    /// Nothing.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PasteSettings {
    pub confirm: PasteConfirm,
    pub middle_click: MiddleClick,
}

/// Where text selected with the mouse goes by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CopyOnSelect {
    /// The primary selection (Linux), for middle-click pasting anywhere.
    /// Elsewhere nothing.
    #[default]
    Primary,
    /// The clipboard too (and the primary selection on Linux).
    Clipboard,
    /// Nowhere; copy explicitly.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SelectionSettings {
    pub copy_on_select: CopyOnSelect,
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
        clamp(&mut self.panes.dim_inactive, 0.0, PaneSettings::MAX_DIM, "panes.dim_inactive", &mut issues);
        clamp(&mut self.terminal.opacity, TerminalSettings::MIN_OPACITY, 1.0, "terminal.opacity", &mut issues);
        // Theme names are checked against the theme library (user themes
        // live in their own files): `ThemeLibrary::resolve`.
        if self.theme.overrides.ansi.len() > 16 {
            issues.push(Issue::new("theme.overrides.ansi", "only 16 ANSI colors exist; the rest are ignored"));
            self.theme.overrides.ansi.truncate(16);
        }
        // Old spellings (`ctrl-shift-+`) become the ones GPUI reports (`ctrl-+`).
        if let Some(opener) = &self.links.file_opener {
            if opener.trim().is_empty() {
                self.links.file_opener = None;
            } else if split_words(opener).is_none() {
                issues.push(Issue::new("links.file_opener", "unterminated quote; opening files with the desktop default"));
                self.links.file_opener = None;
            }
        }
        for (key, problem) in keys::canonicalize_overrides(&mut self.keybindings) {
            issues.push(Issue::new(format!("keybindings.{key}"), problem));
        }
        for (key, action) in &self.keybindings {
            if let Some(a) = action
                && find_action(a).is_none()
            {
                issues.push(Issue::new(format!("keybindings.{key}"), format!("unknown action {a:?}")));
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
    fn file_opener_commands() {
        let with = |t: &str| LinkSettings { file_opener: Some(t.into()), ..Default::default() };
        assert_eq!(LinkSettings::default().file_command("/a b", None, None), None);
        assert_eq!(with("code --goto {path}:{line}:{column}").file_command("/a b", Some(3), None).unwrap(), ["code", "--goto", "/a b:3:1"]);
        assert_eq!(with("'my editor' -n").file_command("/x;rm", None, None).unwrap(), ["my editor", "-n", "/x;rm"]);
        assert_eq!(with(r#"sh -c "e \"$1\"" _ {path}"#).file_command("/f", None, None).unwrap(), ["sh", "-c", "e \"$1\"", "_", "/f"]);
        assert_eq!(with("  ").file_command("/f", None, None), None);
        assert_eq!(split_words("a 'b"), None);

        let mut s = ClientSettings::default();
        s.links.file_opener = Some("vim 'oops".into());
        let issues = s.validate();
        assert_eq!(issues[0].path, "links.file_opener");
        assert_eq!(s.links.file_opener, None);
    }

    #[test]
    fn validation_clamps_and_reports() {
        let mut s = ClientSettings::default();
        s.font.size = 400.0;
        s.terminal.padding = -3.0;
        s.terminal.opacity = f32::NAN;
        s.theme.name = "nope".into();
        s.keybindings.insert("ctrl-x".into(), Some("explode".into()));
        let issues = s.validate();
        assert_eq!(s.font.size, FontSettings::MAX_SIZE);
        assert_eq!(s.terminal.padding, 0.0);
        assert_eq!(s.terminal.opacity, TerminalSettings::MIN_OPACITY);
        let paths: Vec<_> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["font.size", "terminal.padding", "terminal.opacity", "keybindings.ctrl-x"]);
        assert!(ClientSettings::default().validate().is_empty());
    }
}
