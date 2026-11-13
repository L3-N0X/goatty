//! How the prompt looks (`prompt` in `host.json`), and what goatty knows
//! about Starship's modules ([`MODULES`]).
//!
//! The settings follow `p10k configure`: a style (Starship's own look,
//! lean text, a classic bar, rainbow segments), symbols, segment ends and
//! separators, one or two lines with an optional frame and connection
//! line, a transient prompt, the directory, the prompt character, colors,
//! and every module's switch, side, symbol and color. goatty-shell turns
//! them into `starship.toml` and the settings window into a live preview;
//! both start from [`PromptSettings::layout`], so they agree on what goes
//! where.

mod layout;
mod modules;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use layout::{Layout, Look, Piece, Style};
pub use modules::MODULES;

use crate::store::Issue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptStyle {
    /// Starship's own look: colored modules joined by words ("on main",
    /// "via 🦀 v1.85").
    #[default]
    Rich,
    /// Colored text, no backgrounds, no words.
    Lean,
    /// One bar on a shared background, thin separators between segments.
    Classic,
    /// Every segment on its own colored background.
    Rainbow,
}

impl PromptStyle {
    pub const ALL: [Self; 4] = [Self::Rich, Self::Lean, Self::Classic, Self::Rainbow];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rich => "Starship",
            Self::Lean => "Lean",
            Self::Classic => "Classic",
            Self::Rainbow => "Rainbow",
        }
    }

    /// Segments sit on backgrounds (with ends and separators).
    pub fn has_segments(self) -> bool {
        matches!(self, Self::Classic | Self::Rainbow)
    }
}

/// Which symbols modules show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Symbols {
    /// Nerd Font icons (Starship's `nerd-font-symbols` preset). goatty draws
    /// them with its bundled font whatever the terminal font is.
    #[default]
    Nerd,
    /// Starship's defaults, mostly emoji.
    Emoji,
    /// Short words instead of icons (`plain-text-symbols`): any font.
    Text,
    /// No symbols at all.
    None,
}

impl Symbols {
    pub const ALL: [Self; 4] = [Self::Nerd, Self::Emoji, Self::Text, Self::None];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nerd => "Icons",
            Self::Emoji => "Emoji",
            Self::Text => "Text",
            Self::None => "None",
        }
    }
}

/// Which modules are on unless switched one by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModuleSet {
    /// Starship's defaults: whatever is relevant where you are.
    #[default]
    All,
    /// Directory, git and command duration ([`ESSENTIAL`]).
    Essential,
}

/// The modules [`ModuleSet::Essential`] shows.
pub const ESSENTIAL: &[&str] = &["directory", "git_branch", "git_state", "git_status", "cmd_duration"];

/// Between segments of a [`PromptStyle::Classic`] bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Separator {
    #[default]
    Angled,
    Round,
    Slanted,
    Vertical,
    /// Just space.
    Space,
}

impl Separator {
    pub const ALL: [Self; 5] = [Self::Angled, Self::Round, Self::Slanted, Self::Vertical, Self::Space];

    pub fn label(self) -> &'static str {
        match self {
            Self::Angled => "Angled",
            Self::Round => "Round",
            Self::Slanted => "Slanted",
            Self::Vertical => "Vertical",
            Self::Space => "Space",
        }
    }

    /// The thin glyph between two segments on `side` of the prompt.
    pub fn glyph(self, side: Side) -> &'static str {
        match (self, side) {
            (Self::Angled, Side::Left) => "\u{e0b1}",
            (Self::Angled, Side::Right) => "\u{e0b3}",
            (Self::Round, Side::Left) => "\u{e0b5}",
            (Self::Round, Side::Right) => "\u{e0b7}",
            (Self::Slanted, _) => "\u{2571}",
            (Self::Vertical, _) => "\u{2502}",
            (Self::Space, _) => "",
        }
    }
}

/// The shape of a segment's end (p10k's heads and tails).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Flat,
    Sharp,
    Round,
    Slanted,
    Blurred,
}

impl Edge {
    pub const ALL: [Self; 5] = [Self::Flat, Self::Sharp, Self::Round, Self::Slanted, Self::Blurred];

    pub fn label(self) -> &'static str {
        match self {
            Self::Flat => "Flat",
            Self::Sharp => "Sharp",
            Self::Round => "Round",
            Self::Slanted => "Slanted",
            Self::Blurred => "Blurred",
        }
    }

    /// The glyph closing a segment at its `end` (left or right end), drawn
    /// in the segment's background color.
    pub fn glyph(self, end: Side) -> &'static str {
        match (self, end) {
            (Self::Flat, _) => "",
            (Self::Sharp, Side::Left) => "\u{e0b2}",
            (Self::Sharp, Side::Right) => "\u{e0b0}",
            (Self::Round, Side::Left) => "\u{e0b6}",
            (Self::Round, Side::Right) => "\u{e0b4}",
            (Self::Slanted, Side::Left) => "\u{e0ba}",
            (Self::Slanted, Side::Right) => "\u{e0bc}",
            (Self::Blurred, Side::Left) => "░▒▓",
            (Self::Blurred, Side::Right) => "▓▒░",
        }
    }
}

/// Lines around a two-line prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Frame {
    #[default]
    None,
    /// `╭─` and `╰─` on the left.
    Left,
    /// On both sides.
    Full,
}

impl Frame {
    pub const ALL: [Self; 3] = [Self::None, Self::Left, Self::Full];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Left => "Left",
            Self::Full => "Both sides",
        }
    }
}

/// What fills the gap between the left and right side of a two-line
/// prompt's first line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    #[default]
    None,
    Dotted,
    Solid,
}

impl Connection {
    pub const ALL: [Self; 3] = [Self::None, Self::Dotted, Self::Solid];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Dotted => "Dotted",
            Self::Solid => "Solid",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::None => " ",
            Self::Dotted => "·",
            Self::Solid => "─",
        }
    }
}

/// Which side of the prompt a module is on (also: a segment's end).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Left,
    Right,
}

/// How the module list groups modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Status,
    System,
    Vcs,
    Languages,
    Build,
    Environment,
    Cloud,
}

impl Category {
    pub const ALL: [Self; 7] = [Self::System, Self::Vcs, Self::Status, Self::Languages, Self::Build, Self::Environment, Self::Cloud];

    pub fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::System => "Where you are",
            Self::Vcs => "Version control",
            Self::Languages => "Languages",
            Self::Build => "Build tools and packages",
            Self::Environment => "Environments",
            Self::Cloud => "Cloud and containers",
        }
    }
}

/// One Starship module ([`MODULES`]).
#[derive(Debug)]
pub struct Module {
    /// Starship's name (`git_branch`).
    pub id: &'static str,
    pub label: &'static str,
    pub category: Category,
    /// On in Starship's defaults.
    pub on: bool,
    /// The default color: the text in Lean and Classic, the background in
    /// Rainbow. A Starship color (`cyan`, `208`, `#FFAFF3`).
    pub color: &'static str,
    /// Starship's default style, for the Starship look.
    pub style: &'static str,
    /// What it shows, in Starship's format syntax without styling.
    pub content: &'static str,
    /// The word Starship's own look puts before it ("via").
    pub word: &'static str,
    /// Drawn in the same segment as this module (git status with the
    /// branch), whose color the segment takes.
    pub segment: Option<&'static str>,
    /// Starship's default symbol.
    pub symbol: &'static str,
    /// The Nerd Font preset's keys for this module.
    pub nerd: &'static [(&'static str, &'static str)],
    /// The plain text preset's keys for this module.
    pub text: &'static [(&'static str, &'static str)],
    /// What the preview shows for it (without the symbol); `None`: not in
    /// the preview, which is a Rust crate in a git repository on this
    /// computer (no SSH, no rebase, no cloud profile).
    pub sample: Option<&'static str>,
    /// Shows whenever it's on, wherever you are.
    pub always: bool,
}

/// Modules are static and named uniquely.
impl PartialEq for Module {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Module {}

impl Module {
    /// The module whose segment (and Rainbow color) this one is drawn in.
    pub fn lead(&self) -> &'static str {
        self.segment.unwrap_or(self.id)
    }

    /// Whether it has a `symbol` setting. The battery's symbols depend on
    /// its charge and the OS's on the system (`symbols.<name>`).
    pub fn has_symbol(&self) -> bool {
        (self.content.contains("$symbol") || self.content.contains("${symbol}")) && !matches!(self.id, "battery" | "os")
    }

    /// The preset for `symbols`: (key, value) pairs to put in its table.
    pub fn preset(&self, symbols: Symbols) -> Vec<(&'static str, &'static str)> {
        match symbols {
            Symbols::Nerd => self.nerd.to_vec(),
            Symbols::Text => self.text.to_vec(),
            // No symbols: the text preset's words for what isn't a symbol
            // (git ahead/behind, read-only), while symbols are emptied.
            Symbols::None => {
                let clear = matches!(self.id, "os" | "battery");
                self.text.iter().map(|&(k, v)| (k, if clear || k == "symbol" { "" } else { v })).collect()
            }
            Symbols::Emoji => Vec::new(),
        }
    }

    /// Its symbol with `symbols` (styling markup removed).
    pub fn symbol(&self, symbols: Symbols) -> &'static str {
        let key = if self.id == "os" { "symbols.Linux" } else { "symbol" };
        let from = |preset: &'static [(&'static str, &'static str)]| preset.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        let s = match symbols {
            Symbols::Nerd => from(self.nerd).unwrap_or(self.symbol),
            Symbols::Text => from(self.text).unwrap_or(self.symbol),
            Symbols::Emoji => {
                if self.id == "os" {
                    "🐧 "
                } else {
                    self.symbol
                }
            }
            Symbols::None => "",
        };
        strip_markup(s)
    }
}

/// `[x](bold red) ` → `x `: a symbol's text without Starship styling.
fn strip_markup(s: &'static str) -> &'static str {
    if let Some(rest) = s.strip_prefix('[')
        && let Some(end) = rest.find("](")
    {
        return &rest[..end];
    }
    s
}

/// The module called `id`.
pub fn module(id: &str) -> Option<&'static Module> {
    MODULES.iter().find(|m| m.id == id)
}

/// A module's own settings (`prompt.modules.<id>`); unset fields follow
/// the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModuleConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    /// Replaces the symbol from [`PromptSettings::symbols`] ("" = none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// A Starship color ([`PromptColor`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// How the working directory is shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectoryPrompt {
    /// Show at most this many folders (the rest is cut, or abbreviated
    /// with [`abbreviate`](Self::abbreviate)); 0: the whole path.
    pub truncation_length: u8,
    /// Inside a repository, start at its top folder.
    pub truncate_to_repo: bool,
    /// Keep cut folders as their first N letters (`~/p/g/src`, like fish);
    /// 0: drop them.
    pub abbreviate: u8,
    /// Marks where folders were cut.
    pub truncation_symbol: String,
    /// Stands for the home folder.
    pub home_symbol: String,
    /// A lock after folders you can't write to.
    pub read_only: bool,
    /// The repository's top folder in bold.
    pub repo_root: bool,
    /// Paths shown differently (`~/coding/goatty` → `goatty`), in order.
    pub substitutions: Vec<Substitution>,
}

impl Default for DirectoryPrompt {
    fn default() -> Self {
        Self {
            truncation_length: 4,
            truncate_to_repo: true,
            abbreviate: 0,
            truncation_symbol: "…/".into(),
            home_symbol: "~".into(),
            read_only: true,
            repo_root: false,
            substitutions: Vec::new(),
        }
    }
}

/// One of [`DirectoryPrompt::substitutions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Substitution {
    pub from: String,
    pub to: String,
}

impl DirectoryPrompt {
    pub const MAX_LENGTH: u8 = 20;
    pub const MAX_ABBREVIATE: u8 = 9;
}

/// The character the input follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterPrompt {
    /// After a command that succeeded ("" = `❯`, `>` with text symbols).
    pub success: String,
    /// After a command that failed ("" = `❯`, `x` with text symbols).
    pub error: String,
    /// In vi command mode ("" = `❮`, `<` with text symbols).
    pub vicmd: String,
    pub success_color: String,
    pub error_color: String,
}

impl Default for CharacterPrompt {
    fn default() -> Self {
        Self {
            success: String::new(),
            error: String::new(),
            vicmd: String::new(),
            success_color: "green".into(),
            error_color: "red".into(),
        }
    }
}

/// Colors that don't belong to one module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptColors {
    /// The background of the Classic bar.
    pub segment: String,
    /// Text on Rainbow segments.
    pub segment_text: String,
    /// The thin separators in the Classic bar.
    pub separator: String,
    /// The frame and the connection line.
    pub frame: String,
}

impl Default for PromptColors {
    fn default() -> Self {
        Self { segment: "236".into(), segment_text: "black".into(), separator: "bright-black".into(), frame: "bright-black".into() }
    }
}

/// How the prompt looks. goatty turns these into the Starship plugin's
/// config (and its small fallback prompt); whether Starship is used at all
/// is `plugins.starship.enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptSettings {
    pub style: PromptStyle,
    pub symbols: Symbols,
    /// Which modules are on unless switched in [`modules`](Self::modules).
    pub show: ModuleSet,
    /// Input on its own line below the prompt.
    pub two_line: bool,
    /// Empty line before each prompt.
    pub newline_before: bool,
    /// Once a command runs, its prompt shrinks to the prompt character.
    pub transient: bool,
    /// Show how long a command took when it ran at least this long; 0: never.
    pub duration_threshold_ms: u32,
    /// Between segments of the Classic bar.
    pub separator: Separator,
    /// The segment end pointing toward the input (Classic and Rainbow).
    pub heads: Edge,
    /// The other end.
    pub tails: Edge,
    /// Two lines only.
    pub frame: Frame,
    /// Two lines only, with something on the right.
    pub connection: Connection,
    pub directory: DirectoryPrompt,
    pub character: CharacterPrompt,
    pub colors: PromptColors,
    /// Per-module settings by Starship module name.
    pub modules: BTreeMap<String, ModuleConfig>,
}

impl Default for PromptSettings {
    fn default() -> Self {
        Self {
            style: PromptStyle::Rich,
            symbols: Symbols::Nerd,
            show: ModuleSet::All,
            two_line: true,
            newline_before: false,
            transient: false,
            duration_threshold_ms: 2000,
            separator: Separator::Angled,
            heads: Edge::Sharp,
            tails: Edge::Flat,
            frame: Frame::None,
            connection: Connection::None,
            directory: DirectoryPrompt::default(),
            character: CharacterPrompt::default(),
            colors: PromptColors::default(),
            modules: BTreeMap::new(),
        }
    }
}

impl PromptSettings {
    /// Whether module `m` shows (where it's relevant).
    pub fn module_on(&self, m: &Module) -> bool {
        match m.id {
            // The anchor of every style: always there.
            "directory" => true,
            // Its threshold is the switch ("Never" = off).
            "cmd_duration" => self.duration_threshold_ms > 0,
            _ => self.modules.get(m.id).and_then(|c| c.enabled).unwrap_or_else(|| self.default_on(m)),
        }
    }

    /// Whether `m` is on without a switch of its own.
    pub fn default_on(&self, m: &Module) -> bool {
        match self.show {
            ModuleSet::All => m.on,
            ModuleSet::Essential => ESSENTIAL.contains(&m.id),
        }
    }

    pub fn module_side(&self, m: &Module) -> Side {
        self.modules.get(m.id).and_then(|c| c.side).unwrap_or_else(|| default_side(m))
    }

    pub fn module_color<'a>(&'a self, m: &'a Module) -> &'a str {
        self.modules.get(m.id).and_then(|c| c.color.as_deref()).unwrap_or(m.color)
    }

    /// The symbol `m` shows: the user's, else the one from [`symbols`](Self::symbols).
    pub fn module_symbol<'a>(&'a self, m: &'a Module) -> &'a str {
        self.modules.get(m.id).and_then(|c| c.symbol.as_deref()).unwrap_or_else(|| m.symbol(self.symbols))
    }

    /// The prompt character after success, after failure, in vi command mode.
    pub fn character_symbols(&self) -> [&str; 3] {
        let c = &self.character;
        let text = self.symbols == Symbols::Text;
        let pick = |s: &'static str, t: &'static str| if text { t } else { s };
        [
            if c.success.is_empty() { pick("❯", ">") } else { &c.success },
            if c.error.is_empty() { pick("❯", "x") } else { &c.error },
            if c.vicmd.is_empty() { pick("❮", "<") } else { &c.vicmd },
        ]
    }

    /// Change module `id`'s settings; an entry left with only defaults is
    /// dropped, so `host.json` stays sparse.
    pub fn edit_module(&mut self, id: &str, f: impl FnOnce(&mut ModuleConfig)) {
        let entry = self.modules.entry(id.to_owned()).or_default();
        f(entry);
        if *entry == ModuleConfig::default() {
            self.modules.remove(id);
        }
    }

    /// Switch module `m` on or off, keeping no entry when that's its default.
    pub fn set_module_on(&mut self, m: &Module, on: bool) {
        if m.id == "cmd_duration" {
            self.duration_threshold_ms = if on { 2000 } else { 0 };
            return;
        }
        let default = self.default_on(m);
        self.edit_module(m.id, |c| c.enabled = (on != default).then_some(on));
    }

    /// The directory as the preview shows it, for a sample working
    /// directory `~/code/goatty/src` in the repository `goatty`: (before
    /// the repository's top folder, the top folder, after).
    pub fn preview_directory(&self) -> (String, String, String) {
        let d = &self.directory;
        let home = if d.home_symbol.is_empty() { "~" } else { d.home_symbol.as_str() };
        let mut parts: Vec<String> = [home, "code", "goatty", "src"].map(str::to_owned).into();
        let mut root = Some(2);
        let contracted = d.truncate_to_repo && d.abbreviate == 0;
        if contracted {
            parts.drain(..2);
            root = Some(0);
        }
        // Substitutions work on the path as text.
        let joined = parts.join("/");
        let substituted = d.substitutions.iter().fold(joined.clone(), |p, s| if s.from.is_empty() { p } else { p.replace(&s.from, &s.to) });
        if substituted != joined {
            parts = substituted.split('/').map(str::to_owned).collect();
            root = None;
        }
        // Folders cut: those above the repository (when it starts there),
        // then any beyond the length.
        let mut n = match root {
            Some(r) if d.truncate_to_repo => r,
            _ => 0,
        };
        let len = usize::from(d.truncation_length);
        if len > 0 && parts.len() - n > len {
            n = parts.len() - len;
        }
        let cut = match n {
            0 if !contracted => String::new(),
            0 => d.truncation_symbol.clone(),
            _ if d.abbreviate > 0 => parts[..n]
                .iter()
                .map(|p| {
                    if p == home { format!("{p}/") } else { format!("{}/", p.chars().take(usize::from(d.abbreviate)).collect::<String>()) }
                })
                .collect(),
            _ => d.truncation_symbol.clone(),
        };
        parts.drain(..n);
        root = root.and_then(|r| r.checked_sub(n));
        match root.filter(|_| d.repo_root) {
            Some(r) => {
                let before: String = parts[..r].iter().map(|p| format!("{p}/")).collect();
                let after: String = parts[r + 1..].iter().map(|p| format!("/{p}")).collect();
                (format!("{cut}{before}"), parts[r].clone(), after)
            }
            None => (format!("{cut}{}", parts.join("/")), String::new(), String::new()),
        }
    }

    /// Check the values (after parsing): bad ones fall back with an issue.
    pub fn validate(&mut self, issues: &mut Vec<Issue>) {
        if self.duration_threshold_ms > 3_600_000 {
            issues.push(Issue::new("prompt.duration_threshold_ms", "more than an hour; using one hour"));
            self.duration_threshold_ms = 3_600_000;
        }
        let d = &mut self.directory;
        if d.truncation_length > DirectoryPrompt::MAX_LENGTH {
            issues.push(Issue::new("prompt.directory.truncation_length", format!("at most {}", DirectoryPrompt::MAX_LENGTH)));
            d.truncation_length = DirectoryPrompt::MAX_LENGTH;
        }
        if d.abbreviate > DirectoryPrompt::MAX_ABBREVIATE {
            issues.push(Issue::new("prompt.directory.abbreviate", format!("at most {}", DirectoryPrompt::MAX_ABBREVIATE)));
            d.abbreviate = DirectoryPrompt::MAX_ABBREVIATE;
        }
        clean_text(&mut d.truncation_symbol, "prompt.directory.truncation_symbol", issues);
        clean_text(&mut d.home_symbol, "prompt.directory.home_symbol", issues);
        let before = d.substitutions.len();
        d.substitutions.retain(|s| !s.from.trim().is_empty() && !has_control(&s.from) && !has_control(&s.to));
        if d.substitutions.len() != before {
            issues.push(Issue::new("prompt.directory.substitutions", "empty or unprintable paths; ignored"));
        }
        let c = &mut self.character;
        for (s, key) in [(&mut c.success, "success"), (&mut c.error, "error"), (&mut c.vicmd, "vicmd")] {
            clean_text(s, &format!("prompt.character.{key}"), issues);
        }
        let dc = CharacterPrompt::default();
        check_color(&mut c.success_color, &dc.success_color, "prompt.character.success_color", issues);
        check_color(&mut c.error_color, &dc.error_color, "prompt.character.error_color", issues);
        let k = &mut self.colors;
        let dk = PromptColors::default();
        check_color(&mut k.segment, &dk.segment, "prompt.colors.segment", issues);
        check_color(&mut k.segment_text, &dk.segment_text, "prompt.colors.segment_text", issues);
        check_color(&mut k.separator, &dk.separator, "prompt.colors.separator", issues);
        check_color(&mut k.frame, &dk.frame, "prompt.colors.frame", issues);
        let unknown: Vec<String> = self.modules.keys().filter(|id| module(id).is_none()).cloned().collect();
        for id in unknown {
            issues.push(Issue::new(format!("prompt.modules.{id}"), "not a Starship module goatty knows; ignored"));
            self.modules.remove(&id);
        }
        for (id, m) in &mut self.modules {
            if let Some(color) = &m.color {
                match PromptColor::parse(color) {
                    Some(parsed) => m.color = Some(parsed.name()),
                    None => {
                        issues
                            .push(Issue::new(format!("prompt.modules.{id}.color"), format!("{color:?} isn't a color; using the default")));
                        m.color = None;
                    }
                }
            }
            if let Some(s) = &mut m.symbol {
                clean_text(s, &format!("prompt.modules.{id}.symbol"), issues);
            }
        }
        self.modules.retain(|_, m| *m != ModuleConfig::default());
    }
}

/// Where a module goes unless it's moved: the clock on the right, the rest
/// on the left (Starship's own order).
fn default_side(m: &Module) -> Side {
    if m.id == "time" { Side::Right } else { Side::Left }
}

/// The longest symbol or marker the settings take, in characters.
pub const MAX_TEXT: usize = 32;

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Symbols are short, printable and on one line.
fn clean_text(s: &mut String, key: &str, issues: &mut Vec<Issue>) {
    if has_control(s) || s.chars().count() > MAX_TEXT {
        issues.push(Issue::new(key, format!("unprintable characters or longer than {MAX_TEXT}; shortened")));
        *s = s.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect();
    }
}

fn check_color(c: &mut String, default: &str, key: &str, issues: &mut Vec<Issue>) {
    match PromptColor::parse(c) {
        Some(parsed) => *c = parsed.name(),
        None => {
            issues.push(Issue::new(key, format!("{c:?} isn't a color; using {default}")));
            *c = default.to_owned();
        }
    }
}

/// A color as Starship takes it: one of the terminal's 16 (by name or
/// 0–15), another of the 256 (16–255), or `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptColor {
    /// One of the theme's 16 colors (0 black … 7 white, 8–15 bright).
    Ansi(u8),
    /// The fixed 6×6×6 cube and gray ramp (16–255).
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// The names of the 8 base colors, by index.
pub const COLOR_NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"];

impl PromptColor {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(Self::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        if let Ok(n) = s.parse::<u8>() {
            return Some(if n < 16 { Self::Ansi(n) } else { Self::Indexed(n) });
        }
        let (bright, name) = match s.strip_prefix("bright-") {
            Some(n) => (true, n),
            None => (false, s.as_str()),
        };
        let name = if name == "magenta" { "purple" } else { name };
        let i = COLOR_NAMES.iter().position(|n| *n == name)? as u8;
        Some(Self::Ansi(if bright { i + 8 } else { i }))
    }

    /// How Starship and goatty's files write it (`bright-blue`, `236`, `#ff8800`).
    pub fn name(self) -> String {
        match self {
            Self::Ansi(i) if i < 8 => COLOR_NAMES[usize::from(i)].to_owned(),
            Self::Ansi(i) => format!("bright-{}", COLOR_NAMES[usize::from(i - 8)]),
            Self::Indexed(i) => i.to_string(),
            Self::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    /// For zsh's `%F{…}`.
    pub fn zsh(self) -> String {
        match self {
            Self::Ansi(i) | Self::Indexed(i) => i.to_string(),
            Self::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

/// The RGB value of xterm color `i` (16–255: the 6×6×6 cube, then 24
/// grays). The first 16 come from the theme.
pub fn xterm_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
        _ => (0, 0, 0),
    }
}

#[cfg(test)]
mod tests;
