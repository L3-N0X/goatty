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
    pub ssh: SshSettings,
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
    /// Space around the grid in logical pixels, per side.
    pub padding: Padding,
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
        Self {
            renderer: Renderer::Glyphs,
            padding: Padding::all(6.0),
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            opacity: 1.0,
            blur: false,
        }
    }
}

/// Space around the terminal grid, per side. Written as one number when
/// every side is the same (`"padding": 6`), else as an object
/// (`{"top": 4, "right": 10, "bottom": 4, "left": 10}`; missing sides are 6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Padding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Padding {
    pub const DEFAULT: f32 = 6.0;

    pub const fn all(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }

    pub fn is_uniform(&self) -> bool {
        self.top == self.right && self.right == self.bottom && self.bottom == self.left
    }

    /// The sides in CSS order (top, right, bottom, left), for editing one by index.
    pub fn sides_mut(&mut self) -> [&mut f32; 4] {
        [&mut self.top, &mut self.right, &mut self.bottom, &mut self.left]
    }

    /// One side by its index in [`Padding::sides`].
    pub fn side_mut(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.top,
            1 => &mut self.right,
            2 => &mut self.bottom,
            _ => &mut self.left,
        }
    }

    pub fn sides(&self) -> [f32; 4] {
        [self.top, self.right, self.bottom, self.left]
    }
}

impl Default for Padding {
    fn default() -> Self {
        Self::all(Self::DEFAULT)
    }
}

impl Serialize for Padding {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.is_uniform() {
            return s.serialize_f32(self.top);
        }
        use serde::ser::SerializeStruct;
        let mut m = s.serialize_struct("Padding", 4)?;
        m.serialize_field("top", &self.top)?;
        m.serialize_field("right", &self.right)?;
        m.serialize_field("bottom", &self.bottom)?;
        m.serialize_field("left", &self.left)?;
        m.end()
    }
}

impl<'de> Deserialize<'de> for Padding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        fn side() -> f32 {
            Padding::DEFAULT
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Sides {
            #[serde(default = "side")]
            top: f32,
            #[serde(default = "side")]
            right: f32,
            #[serde(default = "side")]
            bottom: f32,
            #[serde(default = "side")]
            left: f32,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            All(f32),
            Sides(Sides),
        }
        match Either::deserialize(d).map_err(|_| serde::de::Error::custom("expected a number or {top, right, bottom, left}"))? {
            Either::All(v) => Ok(Padding::all(v)),
            Either::Sides(s) => Ok(Padding { top: s.top, right: s.right, bottom: s.bottom, left: s.left }),
        }
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

/// The hosts goatty offers to connect to (command palette, settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshSettings {
    /// Also list the hosts in `~/.ssh/config` (and the files it includes).
    pub discover: bool,
    /// Hosts added by hand, and the choices made for hosts from
    /// `~/.ssh/config` (matched by `host`).
    pub hosts: Vec<SshHost>,
    /// Where a host picked in the palette or the settings opens.
    pub open_in: OpenIn,
}

impl Default for SshSettings {
    fn default() -> Self {
        Self { discover: true, hosts: Vec::new(), open_in: OpenIn::Window }
    }
}

/// Where a connection to another host opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenIn {
    /// A terminal window of its own.
    #[default]
    Window,
    /// A new tab of the window it was picked from.
    Tab,
}

impl OpenIn {
    /// The other one (Shift+Enter in the session picker).
    pub fn flip(self) -> Self {
        match self {
            OpenIn::Window => OpenIn::Tab,
            OpenIn::Tab => OpenIn::Window,
        }
    }
}

impl SshSettings {
    pub fn get(&self, host: &str) -> Option<&SshHost> {
        self.hosts.iter().find(|h| h.host == host)
    }

    /// Whether `host` connects with plain SSH.
    pub fn plain(&self, host: &str) -> bool {
        self.get(host).is_some_and(|h| h.plain)
    }

    /// Change the entry for `host` (created if missing). An entry left
    /// without a name or a choice is removed when `keep` is false: hosts
    /// from `~/.ssh/config` need no entry of their own.
    pub fn edit(&mut self, host: &str, keep: bool, f: impl FnOnce(&mut SshHost)) {
        let i = match self.hosts.iter().position(|h| h.host == host) {
            Some(i) => i,
            None => {
                self.hosts.push(SshHost { host: host.into(), ..Default::default() });
                self.hosts.len() - 1
            }
        };
        f(&mut self.hosts[i]);
        if !keep && self.hosts[i].name.is_none() && !self.hosts[i].plain {
            self.hosts.remove(i);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SshHost {
    /// What ssh is given: an alias from `~/.ssh/config`, `host` or `user@host`.
    pub host: String,
    /// Shown instead of `host`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Connect with plain SSH (`ssh host` in a local session) instead of
    /// goatty's daemon on the host. Nothing is installed there.
    pub plain: bool,
}

impl SshHost {
    /// The name to show.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.host)
    }
}

/// Why `host` can't be given to ssh as a destination, if it can't.
pub fn ssh_host_problem(host: &str) -> Option<&'static str> {
    if host.is_empty() {
        Some("it's empty")
    } else if host.starts_with('-') {
        Some("it can't start with -")
    } else if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Some("it can't contain spaces")
    } else {
        None
    }
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
        let pad = &mut self.terminal.padding;
        if pad.is_uniform() {
            let mut v = pad.top;
            clamp(&mut v, 0.0, TerminalSettings::MAX_PADDING, "terminal.padding", &mut issues);
            *pad = Padding::all(v);
        } else {
            for (side, name) in pad.sides_mut().into_iter().zip(["top", "right", "bottom", "left"]) {
                clamp(side, 0.0, TerminalSettings::MAX_PADDING, &format!("terminal.padding.{name}"), &mut issues);
            }
        }
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
        let mut seen = std::collections::HashSet::new();
        let mut i = 0;
        self.ssh.hosts.retain_mut(|h| {
            let path = format!("ssh.hosts.{i}");
            i += 1;
            h.host = h.host.trim().to_string();
            h.name = h.name.take().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            if let Some(why) = ssh_host_problem(&h.host) {
                issues.push(Issue::new(path, format!("host {:?}: {why}; ignored", h.host)));
                return false;
            }
            if !seen.insert(h.host.clone()) {
                issues.push(Issue::new(path, format!("{} is listed twice; the first entry counts", h.host)));
                return false;
            }
            true
        });
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

    /// Names of the `GOATTY_*` variables [`apply_env`](Self::apply_env)
    /// accepts. One it ignores (an empty `GOATTY_THEME`, a size that isn't
    /// a number) overrides nothing, so its rows stay editable.
    pub fn env_overrides(var: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
        let accepted = |name: &str| {
            let v = var(name);
            match name {
                "GOATTY_FONT_SIZE" => v.is_some_and(|s| s.parse::<f32>().is_ok()),
                "GOATTY_RENDER" => matches!(v.as_deref(), Some("shaped" | "glyphs")),
                "GOATTY_STATS" => v.is_some(),
                _ => v.is_some_and(|s| !s.is_empty()),
            }
        };
        Self::ENV_KEYS.iter().map(|(name, _)| *name).filter(|k| accepted(k)).collect()
    }

    /// Each `GOATTY_*` variable [`apply_env`](Self::apply_env) reads, with
    /// the settings it replaces (the settings window marks their rows).
    pub const ENV_KEYS: &'static [(&'static str, &'static [&'static str])] = &[
        ("GOATTY_FONT", &["font.family"]),
        ("GOATTY_FONT_SIZE", &["font.size"]),
        ("GOATTY_RENDER", &["terminal.renderer"]),
        ("GOATTY_THEME", &["theme.name", "theme.follow_system"]),
        ("GOATTY_STATS", &["debug.stats"]),
    ];

    /// The settings a `GOATTY_*` variable called `name` replaces.
    pub fn env_keys(name: &str) -> &'static [&'static str] {
        Self::ENV_KEYS.iter().find(|(n, _)| *n == name).map_or(&[], |(_, k)| k)
    }

    /// Checks that need this machine (installed fonts, programs on PATH),
    /// made after the `GOATTY_*` overrides: things that parse but won't
    /// work as written. Nothing is changed; goatty already falls back.
    pub fn check_environment(&self, fonts: &[String], has_program: impl Fn(&str) -> bool) -> Vec<Issue> {
        let mut issues = Vec::new();
        let installed = |name: &str| fonts.iter().any(|f| f.eq_ignore_ascii_case(name));
        // Fallbacks are matched exactly, as `FontSettings::resolve_family` does.
        let fallback_installed = |name: &str| fonts.iter().any(|f| f == name);
        match self.font.family.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) if !GENERIC_FONTS.contains(&f) && !fonts.is_empty() && !installed(f) => {
                issues.push(Issue::new("font.family", format!("{f:?} isn't installed; the system picks another font")));
            }
            None if !fonts.is_empty() && !self.font.fallbacks.is_empty() && !self.font.fallbacks.iter().any(|f| fallback_installed(f)) => {
                issues.push(Issue::new("font.fallbacks", "none of these fonts is installed; using the system's monospace font"));
            }
            _ => {}
        }
        if let Some(program) = self.links.file_opener.as_deref().and_then(split_words).and_then(|w| w.into_iter().next())
            && !has_program(&program)
        {
            issues.push(Issue::new("links.file_opener", format!("{program:?} isn't installed or not on PATH; opening files will fail")));
        }
        issues
    }
}

/// Font names that pick a font by kind rather than by family.
const GENERIC_FONTS: &[&str] = &["monospace", "sans-serif", "serif", "system-ui"];

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
    fn hosts_open_in_a_window_unless_set_to_tabs() {
        assert_eq!(ClientSettings::default().ssh.open_in, OpenIn::Window);
        let s: ClientSettings = serde_json::from_str(r#"{"ssh":{"open_in":"tab"}}"#).unwrap();
        assert_eq!((s.ssh.open_in, s.ssh.discover), (OpenIn::Tab, true));
        assert_eq!(OpenIn::Tab.flip(), OpenIn::Window);
    }

    #[test]
    fn ssh_hosts_are_checked_and_edited() {
        let mut s = ClientSettings::default();
        let h = |host: &str, name: Option<&str>| SshHost { host: host.into(), name: name.map(Into::into), plain: false };
        s.ssh.hosts =
            vec![h(" box ", Some("  ")), h("-oProxyCommand=x", None), h("a b", None), h("box", Some("Box")), h("pi", Some(" Pi "))];
        let issues = s.validate();
        let paths: Vec<_> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["ssh.hosts.1", "ssh.hosts.2", "ssh.hosts.3"]);
        assert_eq!(s.ssh.hosts, [h("box", None), h("pi", Some("Pi"))]);
        assert_eq!(s.ssh.hosts[1].label(), "Pi");

        // A discovered host's choice comes and goes without leaving an empty entry.
        s.ssh.edit("nas", false, |h| h.plain = true);
        assert!(s.ssh.plain("nas"));
        s.ssh.edit("nas", false, |h| h.plain = false);
        assert!(s.ssh.get("nas").is_none());
        // A host added by hand stays.
        s.ssh.edit("box", true, |h| h.plain = false);
        assert!(s.ssh.get("box").is_some());
    }

    #[test]
    fn padding_is_a_number_or_per_side() {
        use crate::store;
        let parse = |json: &str| store::parse::<ClientSettings>(json);
        assert_eq!(parse(r#"{"terminal": {"padding": 10}}"#).value.terminal.padding, Padding::all(10.0));
        let l = parse(r#"{"terminal": {"padding": {"left": 12, "right": 12}}}"#);
        assert!(l.issues.is_empty(), "{:?}", l.issues);
        assert_eq!(l.value.terminal.padding, Padding { top: 6.0, right: 12.0, bottom: 6.0, left: 12.0 });
        // Per side: each one is clamped on its own.
        let l = parse(r#"{"terminal": {"padding": {"top": 99}}}"#);
        assert_eq!(l.value.terminal.padding.top, TerminalSettings::MAX_PADDING);
        assert_eq!(l.issues.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["terminal.padding.top"]);
        // Garbage falls back to the default.
        let l = parse(r#"{"terminal": {"padding": "wide"}}"#);
        assert_eq!(l.value.terminal.padding, Padding::default());
        assert_eq!(l.issues.len(), 1);
        // Uniform padding is saved as one number; mixed as an object.
        let mut s = ClientSettings::default();
        s.terminal.padding = Padding::all(8.0);
        assert!(store::to_json(&s, &[]).contains(r#""padding": 8.0"#));
        s.terminal.padding.left = 20.0;
        let json = store::to_json(&s, &[]);
        assert_eq!(parse(&json).value.terminal.padding, s.terminal.padding, "{json}");
    }

    #[test]
    fn environment_checks() {
        let fonts: Vec<String> = ["DejaVu Sans Mono", "Fira Code"].map(String::from).to_vec();
        let ok = |_: &str| true;
        let paths = |s: &ClientSettings, has: &dyn Fn(&str) -> bool| {
            s.check_environment(&fonts, has).into_iter().map(|i| i.path).collect::<Vec<_>>()
        };
        let mut s = ClientSettings::default();
        assert!(paths(&s, &ok).is_empty());
        s.font.family = Some("fira code".into());
        assert!(paths(&s, &ok).is_empty(), "case doesn't matter");
        s.font.family = Some("monospace".into());
        assert!(paths(&s, &ok).is_empty(), "generic names are fine");
        s.font.family = Some("Iosevka".into());
        let issues = s.check_environment(&fonts, ok);
        assert_eq!(issues[0].path, "font.family");
        assert!(issues[0].message.contains("the system picks another font"), "{}", issues[0]);
        assert!(s.check_environment(&[], ok).is_empty(), "no font list: nothing to check against");

        s.font.family = None;
        s.font.fallbacks = vec!["Nope Mono".into()];
        assert_eq!(paths(&s, &ok), ["font.fallbacks"]);
        s.font.fallbacks = vec!["fira code".into()];
        assert_eq!(paths(&s, &ok), ["font.fallbacks"], "fallbacks are matched exactly, like the resolver does");
        s.font.fallbacks = vec!["Fira Code".into()];
        assert!(paths(&s, &ok).is_empty());

        let mut s = ClientSettings::default();
        s.links.file_opener = Some("'my editor' --goto {path}".into());
        assert_eq!(paths(&s, &|p| p != "my editor"), ["links.file_opener"]);
        assert!(paths(&s, &ok).is_empty());
    }

    #[test]
    fn env_keys_name_real_settings() {
        for (var, keys) in ClientSettings::ENV_KEYS {
            for k in *keys {
                assert!(crate::store::is_known_key::<ClientSettings>(k), "{var}: {k}");
            }
        }
        assert!(ClientSettings::env_overrides(|k| (k == "GOATTY_THEME").then(String::new)).is_empty(), "an empty value is ignored");
        let vars = [("GOATTY_FONT_SIZE", "huge"), ("GOATTY_RENDER", "glyphs"), ("GOATTY_THEME", "nord"), ("GOATTY_STATS", "")];
        let var = |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(ClientSettings::env_overrides(var), ["GOATTY_RENDER", "GOATTY_THEME", "GOATTY_STATS"]);
    }

    #[test]
    fn validation_clamps_and_reports() {
        let mut s = ClientSettings::default();
        s.font.size = 400.0;
        s.terminal.padding = Padding::all(-3.0);
        s.terminal.opacity = f32::NAN;
        s.theme.name = "nope".into();
        s.keybindings.insert("ctrl-x".into(), Some("explode".into()));
        let issues = s.validate();
        assert_eq!(s.font.size, FontSettings::MAX_SIZE);
        assert_eq!(s.terminal.padding, Padding::all(0.0));
        assert_eq!(s.terminal.opacity, TerminalSettings::MIN_OPACITY);
        let paths: Vec<_> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["font.size", "terminal.padding", "terminal.opacity", "keybindings.ctrl-x"]);
        assert!(ClientSettings::default().validate().is_empty());
    }
}
