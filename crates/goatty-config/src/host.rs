//! Host settings (`host.json` next to each goattyd): the shell environment
//! sessions on that host get. Edited through the daemon, so a remote host
//! keeps its own.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::{Document, Issue, Migration};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HostSettings {
    pub shell: ShellSettings,
    pub prompt: PromptSettings,
    pub history: HistorySettings,
    /// The user's own aliases, by name (#88). Rendered after the plugins'
    /// aliases, so one of these with the same name wins.
    pub aliases: BTreeMap<String, UserAlias>,
    /// Plugins' configuration, by plugin id (Stage 5): the built-in ones
    /// (`autosuggestions`, `syntax-highlighting`, `starship`, `atuin`,
    /// `fzf`) and installed ones. Which plugins are installed is host state,
    /// not a setting (goatty-shell's lock file); an entry for a plugin that
    /// isn't there is ignored.
    pub plugins: BTreeMap<String, PluginConfig>,
}

/// One plugin's configuration in `host.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    /// Load it in new shells. Off keeps it installed but out of the shell.
    #[serde(skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Setting id → value, checked against the plugin's own schema when
    /// the shell configuration is built (bad values fall back to the
    /// plugin's default and are reported there).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, Value>,
    /// Names of the plugin's aliases the user switched off (the rest of
    /// the plugin stays on).
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub disabled_aliases: BTreeSet<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self { enabled: true, settings: BTreeMap::new(), disabled_aliases: BTreeSet::new() }
    }
}

/// One of the user's aliases (`aliases.<name>` in `host.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserAlias {
    /// What typing the name runs: shell code as you'd type it
    /// (`git status --short`, `ls -l | less`).
    pub command: String,
    /// Heading it's listed under ("" = no group).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// A few words for the list.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Off keeps it in the list but out of the shell.
    #[serde(skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn is_true(b: &bool) -> bool {
    *b
}

impl Default for UserAlias {
    fn default() -> Self {
        Self { command: String::new(), group: String::new(), description: String::new(), enabled: true }
    }
}

impl UserAlias {
    pub fn new(command: impl Into<String>) -> Self {
        Self { command: command.into(), ..Self::default() }
    }
}

/// Names an alias can have: no quoting or globbing surprises in zsh
/// (letters, digits, `_ . + - : , @ %`; not starting with `-`; `..` and
/// `...` are fine, `.` alone is the `source` builtin).
pub fn is_alias_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.+-:,@%".contains(&b))
        && !s.starts_with('-')
        && s != "."
}

/// Why `name` can't be an alias name, in words; `None` when it can.
pub fn alias_name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        Some("give the alias a name".into())
    } else if name.chars().any(char::is_whitespace) {
        Some("an alias name can't contain spaces".into())
    } else if name.len() > 64 {
        Some("at most 64 characters".into())
    } else if name.starts_with('-') {
        Some("an alias name can't start with -".into())
    } else if name == "." {
        Some("`.` is the source builtin".into())
    } else if !is_alias_name(name) {
        Some("use letters, digits and _ . + - : , @ % only".into())
    } else {
        None
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

/// How goatty's managed zsh starts. The tools it loads (autosuggestions,
/// syntax highlighting, Starship, Atuin, fzf) are built-in plugins,
/// turned on and off in [`HostSettings::plugins`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZshSettings {
    /// Source the user's `.zshenv` and `.zprofile` (environment: PATH,
    /// EDITOR, ...). Zsh reads these for every terminal, so this is on.
    pub user_env: bool,
    /// Also source the user's `.zshrc`. Off: most `.zshrc` files load their
    /// own prompt and plugins, which would fight goatty's.
    pub user_rc: bool,
    /// OSC 133 prompt marks and OSC 7 working directory reports.
    pub integration: bool,
}

impl Default for ZshSettings {
    fn default() -> Self {
        Self { user_env: true, user_rc: false, integration: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptStyle {
    /// Starship's modules with Nerd Font icons (goatty draws them with its
    /// bundled font whatever the terminal font is).
    #[default]
    Rich,
    /// Directory, git and status only, plain characters: works with any font.
    Minimal,
}

/// How the prompt looks. goatty turns these into the Starship plugin's
/// config (and its small fallback prompt); whether Starship is used at all
/// is `plugins.starship.enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptSettings {
    pub style: PromptStyle,
    /// Input on its own line below the prompt.
    pub two_line: bool,
    /// Empty line before each prompt.
    pub newline_before: bool,
    /// Show how long a command took when it ran at least this long; 0: never.
    pub duration_threshold_ms: u32,
}

impl Default for PromptSettings {
    fn default() -> Self {
        Self { style: PromptStyle::Rich, two_line: true, newline_before: false, duration_threshold_ms: 2000 }
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
}

impl HistorySettings {
    pub const MIN_SIZE: u32 = 100;
    pub const MAX_SIZE: u32 = 10_000_000;
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self { size: 100_000, ignore_duplicates: true, ignore_space: true, share_between_sessions: false }
    }
}

impl Document for HostSettings {
    const NAME: &'static str = "host";
    const VERSION: u32 = 2;
    const OPEN_MAPS: &'static [&'static str] = &["aliases", "plugins"];

    fn migrations() -> &'static [Migration] {
        &[(1, builtin_tools_became_plugins)]
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
        let bad: Vec<String> = self.aliases.keys().filter(|n| !is_alias_name(n)).cloned().collect();
        for name in bad {
            let why = alias_name_problem(&name).unwrap_or_default();
            issues.push(Issue::new(format!("aliases.{name}"), format!("not a usable alias name ({why}); ignored")));
            self.aliases.remove(&name);
        }
        let empty: Vec<String> = self.aliases.iter().filter(|(_, a)| a.command.trim().is_empty()).map(|(n, _)| n.clone()).collect();
        for name in empty {
            issues.push(Issue::new(format!("aliases.{name}.command"), "empty; alias ignored"));
            self.aliases.remove(&name);
        }
        for a in self.aliases.values_mut() {
            if a.group.trim() != a.group {
                a.group = a.group.trim().to_owned();
            }
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

/// Version 1 → 2 (#98): the built-in tools became plugins. Their switches
/// move to `plugins.<id>.enabled`, Starship's and Atuin's options to their
/// plugin settings. Only keys present in the file move, so it stays sparse.
fn builtin_tools_became_plugins(doc: &mut Value) {
    // (where it was, plugin, its setting there: `None` for `enabled`)
    const MOVES: &[(&[&str], &str, Option<&str>)] = &[
        (&["shell", "zsh", "autosuggestions"], "autosuggestions", None),
        (&["shell", "zsh", "syntax_highlighting"], "syntax-highlighting", None),
        (&["shell", "zsh", "fzf"], "fzf", None),
        (&["prompt", "starship"], "starship", None),
        (&["prompt", "prefer_user_config"], "starship", Some("prefer_user_config")),
        (&["history", "atuin"], "atuin", None),
        (&["history", "atuin_up_arrow"], "atuin", Some("up_arrow")),
    ];
    let object = || Value::Object(Default::default());
    for &(path, plugin, setting) in MOVES {
        let (key, parents) = path.split_last().expect("non-empty path");
        let Some(v) = parents.iter().try_fold(&mut *doc, |v, p| v.get_mut(*p)).and_then(Value::as_object_mut).and_then(|o| o.remove(*key))
        else {
            continue;
        };
        let Some(root) = doc.as_object_mut() else { return };
        let Some(entry) = root.entry("plugins").or_insert_with(object).as_object_mut().map(|p| p.entry(plugin).or_insert_with(object))
        else {
            continue;
        };
        let target = match setting {
            None => entry.as_object_mut().map(|e| (e, "enabled")),
            Some(s) => entry.as_object_mut().and_then(|e| e.entry("settings").or_insert_with(object).as_object_mut()).map(|o| (o, s)),
        };
        if let Some((obj, name)) = target {
            obj.entry(name).or_insert(v);
        }
    }
}

impl HostSettings {
    /// Whether plugin `id` loads (on unless its entry says otherwise).
    pub fn plugin_enabled(&self, id: &str) -> bool {
        self.plugins.get(id).is_none_or(|p| p.enabled)
    }

    /// A plugin's stored setting as a bool, `default` when it isn't set
    /// (or isn't a bool: the shell build reports that).
    pub fn plugin_bool(&self, id: &str, key: &str, default: bool) -> bool {
        self.plugins.get(id).and_then(|p| p.settings.get(key)).and_then(Value::as_bool).unwrap_or(default)
    }

    /// Change plugin `id`'s entry; an entry left with nothing but defaults
    /// is dropped, so `host.json` stays sparse.
    pub fn edit_plugin(&mut self, id: &str, f: impl FnOnce(&mut PluginConfig)) {
        let entry = self.plugins.entry(id.to_owned()).or_default();
        f(entry);
        if *entry == PluginConfig::default() {
            self.plugins.remove(id);
        }
    }

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

    /// Names of the `GOATTY_*` variables [`apply_env`](Self::apply_env)
    /// accepts: one it ignores (an empty `GOATTY_SHELL`, a
    /// `GOATTY_SHELL_MANAGED` that isn't a boolean) overrides nothing.
    pub fn env_overrides(var: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
        let accepted = |name: &str| {
            let v = var(name);
            match name {
                "GOATTY_SHELL" => v.is_some_and(|s| !s.is_empty()),
                _ => v.as_deref().and_then(parse_bool).is_some(),
            }
        };
        Self::ENV_KEYS.iter().map(|(name, _)| *name).filter(|k| accepted(k)).collect()
    }

    /// Each `GOATTY_*` variable [`apply_env`](Self::apply_env) reads, with
    /// the settings it replaces.
    pub const ENV_KEYS: &'static [(&'static str, &'static [&'static str])] = &[
        ("GOATTY_SHELL", &["shell.program"]),
        ("GOATTY_SHELL_MANAGED", &["shell.managed"]),
        ("GOATTY_ZSH_USER_RC", &["shell.zsh.user_rc"]),
    ];

    /// The settings a `GOATTY_*` variable called `name` replaces.
    pub fn env_keys(name: &str) -> &'static [&'static str] {
        Self::ENV_KEYS.iter().find(|(n, _)| *n == name).map_or(&[], |(_, k)| k)
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
    fn env_keys_name_real_settings() {
        for (var, keys) in HostSettings::ENV_KEYS {
            for k in *keys {
                assert!(crate::store::is_known_key::<HostSettings>(k), "{var}: {k}");
            }
        }
        assert_eq!(HostSettings::env_keys("GOATTY_SHELL"), ["shell.program"]);
        assert!(HostSettings::env_keys("GOATTY_NOPE").is_empty());
    }

    #[test]
    fn env_overrides_are_the_variables_apply_env_accepts() {
        let vars = [("GOATTY_SHELL", ""), ("GOATTY_SHELL_MANAGED", "maybe"), ("GOATTY_ZSH_USER_RC", "off")];
        let var = |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(HostSettings::env_overrides(var), ["GOATTY_ZSH_USER_RC"]);
        assert_eq!(HostSettings::env_overrides(|k| (k == "GOATTY_SHELL").then(|| "/bin/sh".to_string())), ["GOATTY_SHELL"]);
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

    #[test]
    fn user_aliases() {
        let loaded = crate::store::parse::<HostSettings>(
            r#"{"aliases": {
                "gs": {"command": "git status", "group": " Git "},
                "..": {"command": "cd ..", "enabled": false},
                "bad name": {"command": "x"},
                "empty": {"command": "  "},
                "half": {"command": "ls", "enabled": "yes"}
            },
            "plugins": {"docker": {"disabled_aliases": ["di"]}}}"#,
        );
        let a = &loaded.value.aliases;
        assert_eq!(a["gs"].command, "git status");
        assert_eq!(a["gs"].group, "Git", "trimmed");
        assert!(a["gs"].enabled, "enabled by default");
        assert!(!a[".."].enabled);
        assert!(a["half"].enabled, "a bad value falls back, the alias stays");
        assert!(!a.contains_key("bad name") && !a.contains_key("empty"));
        assert_eq!(loaded.issues.len(), 3, "{:?}", loaded.issues);
        assert!(loaded.unknown.is_empty(), "alias names aren't unknown settings");
        assert!(loaded.value.plugins["docker"].disabled_aliases.contains("di"));
        let json = crate::store::to_json(&loaded.value, &loaded.unknown);
        assert!(json.contains("\"gs\": {\n      \"command\": \"git status\",\n      \"group\": \"Git\"\n    }"), "only what's set: {json}");
        let back = crate::store::parse::<HostSettings>(&json);
        assert_eq!(back.value, loaded.value, "{json}");
        assert!(crate::store::is_known_key::<HostSettings>("aliases.gs.command"));
        let set = crate::store::set_key::<HostSettings>("", "aliases.ll.command", "ls -l").unwrap();
        assert_eq!(set.value.aliases["ll"], UserAlias::new("ls -l"));
    }

    #[test]
    fn version_1_tool_switches_move_to_plugins() {
        let loaded = crate::store::parse::<HostSettings>(
            r#"{"version": 1,
                "shell": {"zsh": {"autosuggestions": false, "syntax_highlighting": false, "fzf": true, "user_rc": true}},
                "prompt": {"starship": false, "prefer_user_config": false, "two_line": false},
                "history": {"atuin_up_arrow": true, "size": 500},
                "plugins": {"atuin": {"settings": {"other": 1}}, "eza": {"enabled": false}}}"#,
        );
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert!(loaded.unknown.is_empty(), "{:?}", loaded.unknown);
        let s = &loaded.value;
        assert!(!s.plugin_enabled("autosuggestions"));
        assert!(!s.plugin_enabled("syntax-highlighting"));
        assert!(s.plugin_enabled("fzf"));
        assert!(!s.plugin_enabled("starship"));
        assert!(!s.plugin_bool("starship", "prefer_user_config", true));
        assert!(s.plugin_enabled("atuin"));
        assert!(s.plugin_bool("atuin", "up_arrow", false));
        assert_eq!(s.plugins["atuin"].settings["other"], 1, "existing plugin settings stay");
        assert!(!s.plugin_enabled("eza"));
        assert!(s.shell.zsh.user_rc && !s.prompt.two_line && s.history.size == 500, "the rest is untouched");
        // Written back as version 2, without the old keys.
        let json = crate::store::to_json(&loaded.value, &loaded.unknown);
        assert!(json.contains("\"version\": 2"), "{json}");
        assert!(!json.contains("syntax_highlighting") && !json.contains("atuin_up_arrow"), "{json}");
        // A version 2 file is left alone; old keys in it are unknown.
        let v2 = crate::store::parse::<HostSettings>(r#"{"version": 2, "prompt": {"starship": false}}"#);
        assert!(v2.value.plugin_enabled("starship"));
        assert_eq!(v2.unknown.len(), 1);
    }

    #[test]
    fn plugin_entries_stay_sparse() {
        let mut s = HostSettings::default();
        s.edit_plugin("atuin", |p| p.enabled = false);
        assert!(!s.plugin_enabled("atuin"));
        s.edit_plugin("atuin", |p| p.enabled = true);
        assert!(s.plugins.is_empty(), "back to the default: no entry");
        s.edit_plugin("starship", |p| {
            p.settings.insert("prefer_user_config".into(), false.into());
        });
        assert!(!s.plugin_bool("starship", "prefer_user_config", true));
    }

    #[test]
    fn alias_names() {
        for ok in ["gs", "..", "...", "g+", "docker:ps", "a.b", "x_1", "c,d", "@x", "%"] {
            assert!(is_alias_name(ok), "{ok}");
            assert_eq!(alias_name_problem(ok), None);
        }
        for bad in ["", ".", "-x", "a b", "a=b", "a'b", "$x", "a;b", "a*", "~", "ö"] {
            assert!(!is_alias_name(bad), "{bad}");
            assert!(alias_name_problem(bad).is_some(), "{bad}");
        }
    }
}
