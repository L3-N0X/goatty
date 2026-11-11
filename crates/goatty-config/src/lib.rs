//! goatty settings: the profile model (Stage 4).
//!
//! Two versioned JSON documents, split by where they apply (#14):
//!
//! * [`ClientSettings`] in `settings.json` on the machine running the
//!   window: font, theme, cursor, keybindings, workspace behavior.
//! * [`HostSettings`] in `host.json` next to each goattyd: shell, prompt,
//!   history. The daemon owns it; the GUI edits a remote host's copy
//!   through the protocol.
//!
//! Both live in `$XDG_CONFIG_HOME/goatty/` ([`config_dir`]). [`store`]
//! loads them leniently (bad values fall back with an [`Issue`]), migrates
//! old versions, and saves only non-default values with backups. `GOATTY_*`
//! environment variables override the files for one run without being
//! saved:
//!
//! | Variable               | Setting                                   |
//! |------------------------|-------------------------------------------|
//! | `GOATTY_FONT`          | [`FontSettings::family`]                  |
//! | `GOATTY_FONT_SIZE`     | [`FontSettings::size`]                    |
//! | `GOATTY_RENDER`        | [`TerminalSettings::renderer`] (`shaped`/`glyphs`) |
//! | `GOATTY_THEME`         | [`ThemeSettings::name`]                   |
//! | `GOATTY_STATS`         | [`DebugSettings::stats`] (any value)      |
//! | `GOATTY_SHELL`         | [`ShellSettings::program`]                |
//! | `GOATTY_SHELL_MANAGED` | [`ShellSettings::managed`] (`0`/`1`)      |
//! | `GOATTY_ZSH_USER_RC`   | [`ZshSettings::user_rc`] (`0`/`1`)        |
//!
//! `GOATTY_SOCKET` is not a setting: it says where a daemon lives and is
//! handled by `goatty_proto::socket_path`.

mod client;
pub mod color_history;
pub mod colorspace;
pub mod command_history;
mod host;
pub mod keys;
pub mod profile;
pub mod store;
mod theme;
pub mod theme_import;
pub mod themes;

use std::path::PathBuf;

pub use client::{
    ClientSettings, CopyOnSelect, CursorShape, DebugSettings, FontSettings, LinkClick, LinkSettings, MiddleClick, OnLaunch, OpenIn,
    Padding, PaneSettings, PasteConfirm, PasteSettings, Renderer, SelectionSettings, SshHost, SshSettings, TabActivity, TabBarShow,
    TabSettings, TabStyle, TerminalSettings, WorkspaceSettings, ssh_host_problem,
};
pub use color_history::ColorHistory;
pub use colorspace::ColorFormat;
pub use command_history::CommandHistory;
pub use host::{
    HistorySettings, HostSettings, PluginConfig, PromptSettings, PromptStyle, ShellSettings, UserAlias, ZshSettings, alias_name_problem,
    is_alias_name,
};
pub use store::{Document, FileStamp, Issue, Loaded};
pub use theme::{
    Color, ColorSlot, DEFAULT_LIGHT_THEME, DEFAULT_THEME, THEMES, Theme, ThemeColors, ThemeOverrides, ThemeSettings, find_theme,
};
pub use themes::{ThemeEntry, ThemeLibrary};

/// `$XDG_CONFIG_HOME/goatty` (default `~/.config/goatty`). `None` without `$HOME`.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(|k| std::env::var(k).ok())
}

pub fn config_dir_from(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| var("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("goatty"))
}

/// The client settings file.
pub fn client_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

/// Exists once the welcome window (first-run setup) was shown. State,
/// not a setting: `$XDG_STATE_HOME/goatty/welcome-shown`.
pub fn welcome_marker() -> Option<PathBuf> {
    color_history::state_dir_from(|k| std::env::var(k).ok()).map(|d| d.join("welcome-shown"))
}

/// This host's settings file (what a goattyd here uses).
pub fn host_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("host.json"))
}

/// A document loaded from its file, plus the environment overrides on top.
#[derive(Debug, Clone)]
pub struct Effective<T> {
    /// As stored in the file: what the settings window edits and saves.
    pub file: Loaded<T>,
    /// With `GOATTY_*` overrides applied: what goatty uses.
    pub value: T,
    /// The variables that override something right now.
    pub env_overrides: Vec<&'static str>,
}

impl<T: Clone> Effective<T> {
    /// Settings that didn't come from a file or the environment (tests,
    /// imports): file and effective value are the same.
    pub fn plain(value: T) -> Self {
        Self { file: Loaded { value: value.clone(), issues: Vec::new(), unknown: Vec::new() }, value, env_overrides: Vec::new() }
    }
}

/// Client settings from [`client_file`] plus the environment.
pub fn load_client() -> Effective<ClientSettings> {
    let file: Loaded<ClientSettings> = client_file().map(|p| store::load(&p)).unwrap_or_default();
    let mut value = file.value.clone();
    value.apply_env(|k| std::env::var(k).ok());
    Effective { file, value, env_overrides: ClientSettings::env_overrides(|k| std::env::var(k).ok()) }
}

/// Host settings from [`host_file`] plus the environment.
pub fn load_host() -> Effective<HostSettings> {
    let file: Loaded<HostSettings> = host_file().map(|p| store::load(&p)).unwrap_or_default();
    with_host_env(file)
}

/// Apply the process environment to loaded host settings.
pub fn with_host_env(file: Loaded<HostSettings>) -> Effective<HostSettings> {
    let mut value = file.value.clone();
    value.apply_env(|k| std::env::var(k).ok());
    Effective { file, value, env_overrides: HostSettings::env_overrides(|k| std::env::var(k).ok()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_follows_xdg() {
        let d = config_dir_from(|k| (k == "HOME").then(|| "/home/u".into())).unwrap();
        assert_eq!(d, PathBuf::from("/home/u/.config/goatty"));
        let d = config_dir_from(|k| match k {
            "HOME" => Some("/home/u".into()),
            "XDG_CONFIG_HOME" => Some("/cfg".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(d, PathBuf::from("/cfg/goatty"));
        assert!(config_dir_from(|_| None).is_none());
    }
}
