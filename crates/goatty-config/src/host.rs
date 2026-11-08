//! Host settings (`host.json` next to each goattyd): the shell environment
//! sessions on that host get. Edited through the daemon, so a remote host
//! keeps its own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::{Document, Issue, Migration};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HostSettings {
    pub shell: ShellSettings,
    pub prompt: PromptSettings,
    pub history: HistorySettings,
    /// Installed plugins' configuration, by plugin id (Stage 5). Which
    /// plugins are installed is host state, not a setting (goatty-shell's
    /// lock file); an entry for a plugin that isn't installed is ignored.
    pub plugins: BTreeMap<String, PluginConfig>,
}

/// One plugin's configuration in `host.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    /// Load it in new shells. Off keeps it installed but out of the shell.
    pub enabled: bool,
    /// Setting id → value, checked against the plugin's own schema when
    /// the shell configuration is built (bad values fall back to the
    /// plugin's default and are reported there).
    pub settings: BTreeMap<String, Value>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self { enabled: true, settings: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellSettings {
    /// Shell program. `None`: zsh if installed, else `$SHELL`, else /bin/sh.
    pub program: Option<String>,
    /// Run zsh with goatty's managed configuration (see `goatty-shell`).
    /// `false`: plain zsh with only the user's own dotfiles. Other shells
    /// are always launched as they are.
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
    /// fzf key bindings (Ctrl+T, Alt+C) when fzf is installed.
    pub fzf: bool,
    /// OSC 133 prompt marks and OSC 7 working directory reports.
    pub integration: bool,
}

impl Default for ZshSettings {
    fn default() -> Self {
        Self { user_env: true, user_rc: false, autosuggestions: true, syntax_highlighting: true, fzf: true, integration: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptStyle {
    /// Starship's modules with icons (best with a Nerd Font).
    #[default]
    Rich,
    /// Directory, git and status only, plain characters: works with any font.
    Minimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptSettings {
    /// Use Starship when installed (else a small built-in prompt).
    pub starship: bool,
    pub style: PromptStyle,
    /// Input on its own line below the prompt.
    pub two_line: bool,
    /// Empty line before each prompt.
    pub newline_before: bool,
    /// Show how long a command took when it ran at least this long; 0: never.
    pub duration_threshold_ms: u32,
    /// Use `~/.config/starship.toml` (or `STARSHIP_CONFIG`) when it exists,
    /// instead of goatty's generated config.
    pub prefer_user_config: bool,
}

impl Default for PromptSettings {
    fn default() -> Self {
        Self {
            starship: true,
            style: PromptStyle::Rich,
            two_line: true,
            newline_before: false,
            duration_threshold_ms: 2000,
            prefer_user_config: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistorySettings {
    /// Entries kept in memory and in the history file.
    pub size: u32,
    pub ignore_duplicates: bool,
    /// Commands starting with a space aren't saved.
    pub ignore_space: bool,
    /// New commands show up in other running shells right away.
    pub share_between_sessions: bool,
    /// Atuin's searchable history on Ctrl+R, when installed.
    pub atuin: bool,
    /// Let Atuin take over the Up arrow too.
    pub atuin_up_arrow: bool,
}

impl HistorySettings {
    pub const MIN_SIZE: u32 = 100;
    pub const MAX_SIZE: u32 = 10_000_000;
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            size: 100_000,
            ignore_duplicates: true,
            ignore_space: true,
            share_between_sessions: false,
            atuin: true,
            atuin_up_arrow: false,
        }
    }
}

impl Document for HostSettings {
    const NAME: &'static str = "host";
    const VERSION: u32 = 1;
    const OPEN_MAPS: &'static [&'static str] = &["plugins"];

    fn migrations() -> &'static [Migration] {
        &[]
    }

    fn validate(&mut self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let h = &mut self.history;
        let size = h.size.clamp(HistorySettings::MIN_SIZE, HistorySettings::MAX_SIZE);
        if size != h.size {
            issues.push(Issue::new(
                "history.size",
                format!("{} is outside {}–{}; using {size}", h.size, HistorySettings::MIN_SIZE, HistorySettings::MAX_SIZE),
            ));
            h.size = size;
        }
        if self.prompt.duration_threshold_ms > 3_600_000 {
            issues.push(Issue::new("prompt.duration_threshold_ms", "more than an hour; using one hour"));
            self.prompt.duration_threshold_ms = 3_600_000;
        }
        let bad: Vec<String> = self.plugins.keys().filter(|id| !is_plugin_id(id)).cloned().collect();
        for id in bad {
            issues.push(Issue::new(format!("plugins.{id}"), "not a plugin id; ignored"));
            self.plugins.remove(&id);
        }
        if let Some(p) = &self.shell.program
            && p.trim().is_empty()
        {
            issues.push(Issue::new("shell.program", "empty; using the default shell"));
            self.shell.program = None;
        }
        issues
    }
}

impl HostSettings {
    /// Apply `GOATTY_*` overrides read through `var`.
    pub fn apply_env(&mut self, var: impl Fn(&str) -> Option<String>) {
        if let Some(sh) = var("GOATTY_SHELL").filter(|s| !s.is_empty()) {
            self.shell.program = Some(sh);
        }
        if let Some(b) = var("GOATTY_SHELL_MANAGED").as_deref().and_then(parse_bool) {
            self.shell.managed = b;
        }
        if let Some(b) = var("GOATTY_ZSH_USER_RC").as_deref().and_then(parse_bool) {
            self.shell.zsh.user_rc = b;
        }
    }

    pub fn env_overrides(var: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
        ["GOATTY_SHELL", "GOATTY_SHELL_MANAGED", "GOATTY_ZSH_USER_RC"].into_iter().filter(|k| var(k).is_some()).collect()
    }
}

/// Same rule as `goatty_plugin::manifest::is_id` (goatty-config doesn't
/// depend on the plugin crate).
fn is_plugin_id(s: &str) -> bool {
    (2..=40).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && s.as_bytes()[0].is_ascii_lowercase()
        && !s.ends_with('-')
}

pub(crate) fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> HostSettings {
        let mut s = HostSettings::default();
        s.apply_env(|k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()));
        s
    }

    #[test]
    fn shell_env_toggles() {
        let s = with(&[("GOATTY_SHELL_MANAGED", "0"), ("GOATTY_ZSH_USER_RC", "yes"), ("GOATTY_SHELL", "/bin/sh")]);
        assert!(!s.shell.managed);
        assert!(s.shell.zsh.user_rc);
        assert_eq!(s.shell.program.as_deref(), Some("/bin/sh"));
        assert!(with(&[("GOATTY_SHELL_MANAGED", "maybe")]).shell.managed);
    }

    #[test]
    fn validation() {
        let mut s = HostSettings::default();
        s.history.size = 5;
        s.shell.program = Some("  ".into());
        let issues = s.validate();
        assert_eq!(s.history.size, HistorySettings::MIN_SIZE);
        assert_eq!(s.shell.program, None);
        assert_eq!(issues.len(), 2);
        assert!(HostSettings::default().validate().is_empty());
    }

    #[test]
    fn plugin_entries_are_an_open_map() {
        let loaded = crate::store::parse::<HostSettings>(
            r#"{"plugins": {"eza": {"settings": {"icons": "always"}}, "zoxide": {"enabled": false}, "Bad Id": {}}}"#,
        );
        let p = &loaded.value.plugins;
        assert!(p["eza"].enabled, "enabled by default");
        assert_eq!(p["eza"].settings["icons"], "always");
        assert!(!p["zoxide"].enabled);
        assert!(!p.contains_key("Bad Id"));
        assert_eq!(loaded.issues.len(), 1, "{:?}", loaded.issues);
        assert!(loaded.unknown.is_empty(), "plugin ids aren't unknown settings");
        // Sparse save keeps the entries; `set` accepts plugin keys.
        let json = crate::store::to_json(&loaded.value, &loaded.unknown);
        assert!(json.contains("\"icons\": \"always\"") && json.contains("\"zoxide\""), "{json}");
        assert!(crate::store::is_known_key::<HostSettings>("plugins.eza.settings.icons"));
    }
}
