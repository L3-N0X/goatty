//! Icons for the program running in a tab: Nerd Font glyphs for known
//! tools, shown only when an installed font really has the glyph.
//!
//! Where the glyphs come from: the terminal font if it's a patched Nerd Font
//! (JetBrainsMono Nerd Font, …), else the symbols-only font ("Symbols Nerd
//! Font Mono"). Without either, tabs fall back to goatty's own terminal icon.

use std::collections::HashSet;

use gpui::{App, Font, SharedString};

/// Families that carry only the Nerd Font symbols, tried after the
/// terminal font.
const SYMBOL_FONTS: &[&str] = &["Symbols Nerd Font Mono", "Symbols Nerd Font"];

/// What a tab shows for a program: a glyph and a theme color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIcon {
    pub glyph: char,
    pub color: IconColor,
}

/// Icon colors come from the theme, so icons fit every palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconColor {
    /// The tab's text color.
    Text,
    /// ANSI color 0–15 of the active theme.
    Ansi(u8),
}

// Nerd Fonts v3 code points.
const TERMINAL: char = '\u{e795}'; // nf-dev-terminal
const VIM: char = '\u{e62b}'; // nf-custom-vim
const NEOVIM: char = '\u{f36f}'; // nf-linux-neovim
const EMACS: char = '\u{e632}'; // nf-custom-emacs
const PENCIL: char = '\u{f040}'; // nf-fa-pencil
const GIT: char = '\u{e702}'; // nf-dev-git
const GITHUB: char = '\u{f09b}'; // nf-fa-github
const DOCKER: char = '\u{f308}'; // nf-linux-docker
const KUBERNETES: char = '\u{f10fe}'; // nf-md-kubernetes
const PYTHON: char = '\u{e73c}'; // nf-dev-python
const NODE: char = '\u{e718}'; // nf-dev-nodejs_small
const NPM: char = '\u{e71e}'; // nf-dev-npm
const RUST: char = '\u{e7a8}'; // nf-dev-rust
const GO: char = '\u{e627}'; // nf-seti-go
const RUBY: char = '\u{e739}'; // nf-dev-ruby
const JAVA: char = '\u{e738}'; // nf-dev-java
const LUA: char = '\u{e620}'; // nf-seti-lua
const DATABASE: char = '\u{f1c0}'; // nf-fa-database
const SERVER: char = '\u{f233}'; // nf-fa-server
const CHART: char = '\u{f080}'; // nf-fa-bar_chart
const BOOK: char = '\u{f02d}'; // nf-fa-book
const FOLDER: char = '\u{f07c}'; // nf-fa-folder_open
const SEARCH: char = '\u{f002}'; // nf-fa-search
const GLOBE: char = '\u{f0ac}'; // nf-fa-globe
const COGS: char = '\u{f085}'; // nf-fa-cogs
const COLUMNS: char = '\u{f0db}'; // nf-fa-columns
const SHIELD: char = '\u{f132}'; // nf-fa-shield
const CLOCK: char = '\u{f017}'; // nf-fa-clock_o
const ROBOT: char = '\u{f06a9}'; // nf-md-robot

/// Every glyph the table uses (checked against the font once).
const ALL_GLYPHS: &[char] = &[
    TERMINAL, VIM, NEOVIM, EMACS, PENCIL, GIT, GITHUB, DOCKER, KUBERNETES, PYTHON, NODE, NPM, RUST, GO, RUBY, JAVA, LUA, DATABASE, SERVER,
    CHART, BOOK, FOLDER, SEARCH, GLOBE, COGS, COLUMNS, SHIELD, CLOCK, ROBOT,
];

/// Shells: the tab is "at the prompt".
pub fn is_shell(name: &str) -> bool {
    matches!(name, "zsh" | "bash" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "elvish" | "xonsh" | "pwsh" | "ion")
}

/// The icon for a program, if goatty knows it. Interpreter versions
/// (`python3.12`, `node22`) count as the interpreter.
pub fn icon_for(process: &str) -> Option<ProcessIcon> {
    use IconColor::{Ansi, Text};
    let name = process.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let (glyph, color) = match name {
        _ if is_shell(process) || is_shell(name) => (TERMINAL, Text),
        "vim" | "vi" | "gvim" | "vimdiff" => (VIM, Ansi(2)),
        "nvim" => (NEOVIM, Ansi(2)),
        "emacs" | "emacsclient" => (EMACS, Ansi(5)),
        "nano" | "micro" | "hx" | "helix" | "kak" | "ed" => (PENCIL, Ansi(6)),
        "git" | "lazygit" | "tig" | "gitui" => (GIT, Ansi(1)),
        "gh" => (GITHUB, Text),
        "docker" | "docker-compose" | "podman" | "lazydocker" | "ctop" => (DOCKER, Ansi(4)),
        "kubectl" | "k9s" | "helm" | "minikube" | "kind" => (KUBERNETES, Ansi(4)),
        "python" | "ipython" | "pip" | "uv" | "uvx" | "poetry" | "pytest" => (PYTHON, Ansi(3)),
        "node" | "deno" | "bun" | "yarn" | "tsx" | "ts-node" => (NODE, Ansi(2)),
        "npm" | "npx" | "pnpm" => (NPM, Ansi(1)),
        "cargo" | "rustc" | "rustup" | "rust-analyzer" | "bacon" => (RUST, Ansi(3)),
        "go" | "gopls" => (GO, Ansi(6)),
        "ruby" | "irb" | "bundle" | "rails" | "gem" => (RUBY, Ansi(1)),
        "java" | "javac" | "gradle" | "mvn" | "kotlin" | "sbt" => (JAVA, Ansi(1)),
        "lua" | "luajit" => (LUA, Ansi(4)),
        "psql" | "mysql" | "mariadb" | "sqlite" | "redis-cli" | "mongosh" | "pgcli" | "litecli" => (DATABASE, Ansi(5)),
        "ssh" | "mosh" | "mosh-client" | "et" | "telnet" | "sftp" => (SERVER, Ansi(5)),
        "htop" | "btop" | "top" | "atop" | "glances" | "nvtop" | "bottom" | "btm" => (CHART, Ansi(2)),
        "man" | "less" | "more" | "most" | "bat" | "glow" => (BOOK, Ansi(3)),
        "yazi" | "ranger" | "lf" | "nnn" | "mc" | "broot" | "vifm" => (FOLDER, Ansi(3)),
        "fzf" | "sk" | "rg" | "grep" | "fd" | "find" | "atuin" => (SEARCH, Ansi(6)),
        "curl" | "wget" | "http" | "xh" | "ping" | "mtr" | "aria2c" => (GLOBE, Ansi(4)),
        "make" | "cmake" | "ninja" | "just" | "meson" | "bazel" => (COGS, Text),
        "tmux" | "zellij" | "screen" => (COLUMNS, Ansi(2)),
        "sudo" | "doas" | "su" | "run0" => (SHIELD, Ansi(1)),
        "watch" | "sleep" => (CLOCK, Text),
        "claude" | "codex" | "aider" | "gemini" | "ollama" | "goose" => (ROBOT, Ansi(3)),
        _ => return None,
    };
    Some(ProcessIcon { glyph, color })
}

/// The font that draws Nerd Font glyphs here, and which of ours it has.
#[derive(Debug, Clone, Default)]
pub struct NerdFont {
    pub font: Option<Font>,
    has: HashSet<char>,
    /// What this was computed for: the terminal font and how many fonts
    /// were installed.
    key: Option<(SharedString, usize)>,
}

impl NerdFont {
    /// Find the font for `terminal_family` among `installed` families.
    /// Cheap to call again: only recomputes when the inputs changed.
    pub fn update(&mut self, terminal_family: &str, installed: &[String], cx: &App) {
        let key = Some((SharedString::from(terminal_family.to_string()), installed.len()));
        if self.key == key {
            return;
        }
        *self = Self { key, ..Self::default() };
        let text = cx.text_system();
        let candidates = std::iter::once(terminal_family).chain(SYMBOL_FONTS.iter().copied());
        for family in candidates.filter(|f| installed.iter().any(|i| i == f)) {
            let font = gpui::font(family.to_string());
            let id = text.resolve_font(&font);
            let has: HashSet<char> = ALL_GLYPHS.iter().copied().filter(|&c| text.advance(id, gpui::px(14.), c).is_ok()).collect();
            // A real Nerd Font has (nearly) all of them; one that lacks the
            // terminal glyph isn't one.
            if has.contains(&TERMINAL) {
                log::info!("tab icons: {family} ({}/{} glyphs)", has.len(), ALL_GLYPHS.len());
                self.font = Some(font);
                self.has = has;
                return;
            }
        }
        log::info!("tab icons: no Nerd Font installed; tabs show goatty's own icon");
    }

    /// The icon for `process`, if goatty knows the program and the font
    /// has its glyph.
    pub fn icon(&self, process: &str) -> Option<ProcessIcon> {
        self.font.as_ref()?;
        icon_for(process).filter(|i| self.has.contains(&i.glyph))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_programs() {
        assert_eq!(icon_for("nvim").map(|i| i.glyph), Some(NEOVIM));
        assert_eq!(icon_for("zsh").map(|i| i.glyph), Some(TERMINAL));
        assert_eq!(icon_for("python3.12").map(|i| i.glyph), Some(PYTHON));
        assert_eq!(icon_for("python3").map(|i| i.glyph), Some(PYTHON));
        assert_eq!(icon_for("node22").map(|i| i.glyph), Some(NODE));
        assert_eq!(icon_for("some-random-tool"), None);
        assert!(is_shell("bash") && !is_shell("vim"));
    }

    #[test]
    fn every_glyph_is_listed() {
        let names =
            ["vim", "nvim", "emacs", "nano", "git", "gh", "docker", "kubectl", "python", "node", "npm", "cargo", "go", "ruby", "java"];
        for n in names {
            assert!(ALL_GLYPHS.contains(&icon_for(n).unwrap().glyph), "{n}");
        }
        for n in ["lua", "psql", "ssh", "htop", "man", "yazi", "fzf", "curl", "make", "tmux", "sudo", "watch", "claude", "zsh"] {
            assert!(ALL_GLYPHS.contains(&icon_for(n).unwrap().glyph), "{n}");
        }
    }
}
