//! What each generation changed, and names for them, so a rollback target
//! is recognizable (#55).
//!
//! A generation's [`Changes`] compare it with its *base*: the generation
//! that was active when it was built (after a rollback that isn't the one
//! numbered just before it). They're worked out when the generation is
//! built and kept in its `info.json`, so they survive the base being
//! pruned. Generations from before this existed get theirs computed from
//! the snapshots, against the previous number, while that one is kept.
//!
//! The settings part compares the two `host.json` snapshots with every
//! default filled in, so `history.size 100000 → 5000` shows up whether or
//! not either file spelled the value out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use goatty_config::{HostSettings, store};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::dirs::Dirs;
use crate::generation::{self, FileDiff, Generation};

/// Longest generation name kept.
pub const NAME_MAX: usize = 60;
/// A one-line summary lists changes only while it's at most this long;
/// otherwise it counts them.
const SHORT_MAX: usize = 60;
/// Longest value shown in a change.
const VALUE_MAX: usize = 24;

/// How a generation differs from its base.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Changes {
    /// The generation it was built on. `None`: the first one.
    pub base: Option<u32>,
    pub settings: Vec<SettingChange>,
    /// Plugins whose files appeared in or left the configuration.
    pub plugins_added: Vec<String>,
    pub plugins_removed: Vec<String>,
    /// Generated files that differ.
    pub files: u32,
    /// The goatty version that built it, when the base was built by another.
    pub goatty: Option<String>,
}

/// One setting that differs, values as text. `None` on one side: it was
/// added or removed; an empty string there: a whole entry (an alias).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChange {
    pub key: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// A generation as the history shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub number: u32,
    pub created_unix: u64,
    pub active: bool,
    pub name: Option<String>,
    /// `None`: its base isn't kept any more, so nothing to compare with.
    pub changes: Option<Changes>,
}

impl Entry {
    /// "before trying fish" or "Generation 7".
    pub fn title(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("Generation {}", self.number))
    }

    /// One line: what changed, or "" when that isn't known.
    pub fn summary(&self) -> String {
        self.changes.as_ref().map(|c| c.short(self.number)).unwrap_or_default()
    }
}

impl SettingChange {
    /// `history.size 100000 → 5000`, `starship off`, `alias ll added`.
    pub fn text(&self) -> String {
        let key = label(&self.key);
        if self.key.starts_with("plugins.") && self.key.ends_with(".enabled") {
            return format!("{key} {}", self.new.as_deref().unwrap_or("on"));
        }
        match (&self.old, &self.new) {
            (Some(o), Some(n)) => format!("{key} {o} → {n}"),
            (None, Some(n)) if n.is_empty() => format!("{key} added"),
            (None, Some(n)) => format!("{key} → {n}"),
            (Some(o), None) if o.is_empty() => format!("{key} removed"),
            (Some(_), None) => format!("{key} reset"),
            (None, None) => key,
        }
    }

    /// What part of the settings it's in: `history`, `aliases`, a plugin id.
    pub fn area(&self) -> &str {
        area(&self.key)
    }
}

fn area(key: &str) -> &str {
    let mut parts = key.split('.');
    match parts.next() {
        Some("plugins") => parts.next().unwrap_or("plugins"),
        Some(first) => first,
        None => key,
    }
}

/// `plugins.eza.settings.icons` → `eza.icons`, `aliases.ll` → `alias ll`.
fn label(key: &str) -> String {
    if let Some(rest) = key.strip_prefix("plugins.") {
        let rest = rest.strip_suffix(".enabled").unwrap_or(rest);
        return rest.replacen(".settings.", ".", 1);
    }
    if let Some(rest) = key.strip_prefix("aliases.") {
        return format!("alias {rest}");
    }
    key.to_owned()
}

impl Changes {
    /// Every change as a line, settings first.
    pub fn lines(&self) -> Vec<String> {
        let toggled: BTreeSet<&str> = self.settings.iter().filter(|s| s.key.ends_with(".enabled")).map(|s| s.area()).collect();
        let mut out: Vec<String> = self.settings.iter().map(SettingChange::text).collect();
        // Switching a plugin on or off already says its files came or went.
        out.extend(self.plugins_added.iter().filter(|p| !toggled.contains(p.as_str())).map(|p| format!("{p} plugin added")));
        out.extend(self.plugins_removed.iter().filter(|p| !toggled.contains(p.as_str())).map(|p| format!("{p} plugin removed")));
        if let Some(v) = &self.goatty {
            out.push(format!("built by goatty {v}"));
        }
        out
    }

    /// One short line for generation `number`: the change itself when
    /// there's one or two short ones, else how many and where
    /// (`4 changes: history, aliases`).
    pub fn short(&self, number: u32) -> String {
        let Some(base) = self.base else { return "First configuration".into() };
        let from = if base + 1 == number { String::new() } else { format!("From {base}: ") };
        let lines = self.lines();
        let text = match lines.len() {
            0 if self.files > 0 => format!("{} generated file{} changed", self.files, plural(self.files as usize)),
            0 => "Settings rewritten, nothing changed".into(),
            1 => lines[0].clone(),
            2 if lines[0].chars().count() + lines[1].chars().count() + 2 <= SHORT_MAX => lines.join(", "),
            n => {
                let mut areas: Vec<&str> = Vec::new();
                for s in &self.settings {
                    if !areas.contains(&s.area()) {
                        areas.push(s.area());
                    }
                }
                for p in self.plugins_added.iter().chain(&self.plugins_removed) {
                    if !areas.contains(&p.as_str()) {
                        areas.push(p);
                    }
                }
                if self.goatty.is_some() {
                    areas.push("goatty update");
                }
                let more = areas.len().saturating_sub(3);
                areas.truncate(3);
                let tail = if more > 0 { format!(" +{more}") } else { String::new() };
                format!("{n} changes: {}{tail}", areas.join(", "))
            }
        };
        from + &text
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The settings that differ between two `host.json` snapshots.
pub fn settings_changes(old_json: &str, new_json: &str) -> Vec<SettingChange> {
    let mut old = full(old_json);
    let mut new = full(new_json);
    align_plugins(&mut old, &mut new);
    let mut out = Vec::new();
    walk("", Some(&old), Some(&new), &mut out);
    out
}

/// The settings with every default spelled out (unknown keys dropped).
fn full(json: &str) -> Value {
    let value = store::parse::<HostSettings>(json).value;
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A plugin entry leaves out what's at its default (`enabled: true`, no
/// settings): fill that in, and give a plugin only one side mentions a
/// default entry on the other, so turning one off reads `starship off`.
fn align_plugins(a: &mut Value, b: &mut Value) {
    let ids: BTreeSet<String> =
        [&*a, &*b].iter().filter_map(|v| v.get("plugins").and_then(Value::as_object)).flat_map(|m| m.keys().cloned()).collect();
    for v in [a, b] {
        let Some(plugins) = v.get_mut("plugins").and_then(Value::as_object_mut) else { continue };
        for id in &ids {
            let entry = plugins.entry(id.clone()).or_insert_with(|| Value::Object(Map::new()));
            if let Some(e) = entry.as_object_mut() {
                e.entry("enabled").or_insert(Value::Bool(true));
                e.entry("settings").or_insert_with(|| Value::Object(Map::new()));
                e.entry("disabled_aliases").or_insert_with(|| Value::Array(Vec::new()));
            }
        }
    }
}

fn walk(key: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<SettingChange>) {
    let join = |k: &str| if key.is_empty() { k.to_owned() } else { format!("{key}.{k}") };
    match (a, b) {
        (Some(Value::Object(x)), Some(Value::Object(y))) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                walk(&join(k), x.get(k), y.get(k), out);
            }
        }
        (x, y) if x == y => {}
        // A whole entry (an alias) came or went: one line, no fields.
        (None, Some(Value::Object(_))) => out.push(SettingChange { key: key.into(), old: None, new: Some(String::new()) }),
        (Some(Value::Object(_)), None) => out.push(SettingChange { key: key.into(), old: Some(String::new()), new: None }),
        (x, y) => out.push(SettingChange { key: key.into(), old: x.map(show), new: y.map(show) }),
    }
}

fn show(v: &Value) -> String {
    let s = match v {
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        Value::Null => "default".into(),
        Value::String(s) if s.is_empty() => "\"\"".into(),
        Value::String(s) => s.clone(),
        Value::Array(a) if a.is_empty() => "none".into(),
        Value::Array(a) => a.iter().map(show).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    };
    if s.chars().count() > VALUE_MAX { s.chars().take(VALUE_MAX - 1).collect::<String>() + "…" } else { s }
}

/// Generated files by path relative to the ZDOTDIR.
pub(crate) type Tree = BTreeMap<String, String>;

/// Every file under `dir` (a generation's `zsh/`).
pub(crate) fn tree(dir: &Path) -> Tree {
    fn go(dir: &Path, prefix: &str, out: &mut Tree) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let path = e.path();
            if path.is_dir() {
                go(&path, &rel, out);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                out.insert(rel, text);
            }
        }
    }
    let mut out = Tree::new();
    go(dir, "", &mut out);
    out
}

/// The plugins a generation has: their files, or their `host.json` entry
/// (a plugin that only brings aliases has no files of its own).
fn plugin_ids(t: &Tree, settings: &str) -> BTreeSet<String> {
    let entries = serde_json::from_str::<Value>(settings)
        .ok()
        .and_then(|v| v.get("plugins")?.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()));
    t.keys().filter_map(|k| Some(k.strip_prefix("plugins/")?.split('/').next()?.to_owned())).chain(entries.into_iter().flatten()).collect()
}

/// Compare a generation (`settings`, `files`, built by goatty `version`)
/// with its base.
pub(crate) fn compare(base: &Generation, settings: &str, files: &Tree, version: &str) -> Changes {
    let old_files = tree(&base.zsh_dir());
    let changed =
        old_files.keys().chain(files.keys()).collect::<BTreeSet<_>>().into_iter().filter(|k| old_files.get(*k) != files.get(*k)).count();
    let old_settings = base.settings_json().unwrap_or_default();
    let (old_ids, new_ids) = (plugin_ids(&old_files, &old_settings), plugin_ids(files, settings));
    let base_version = generation::read_info(&base.path).goatty;
    Changes {
        base: Some(base.number),
        settings: settings_changes(&old_settings, settings),
        plugins_added: new_ids.difference(&old_ids).cloned().collect(),
        plugins_removed: old_ids.difference(&new_ids).cloned().collect(),
        files: u32::try_from(changed).unwrap_or(u32::MAX),
        goatty: (!base_version.is_empty() && base_version != version).then(|| version.to_owned()),
    }
}

/// The generation `g` was built on, if it's still kept.
fn base_of<'a>(g: &Generation, all: &'a [Generation]) -> Option<&'a Generation> {
    let base = generation::read_info(&g.path).parent.or(g.number.checked_sub(1))?;
    all.iter().find(|b| b.number == base)
}

/// All generations with names and what they changed, oldest first.
pub fn list(dirs: &Dirs) -> Vec<Entry> {
    let all = generation::list(dirs);
    all.iter()
        .map(|g| {
            let info = generation::read_info(&g.path);
            let changes = info.changes.clone().or_else(|| match base_of(g, &all) {
                Some(base) => {
                    let settings = g.settings_json().unwrap_or_default();
                    Some(compare(base, &settings, &tree(&g.zsh_dir()), &info.goatty))
                }
                None if g.number == 1 => Some(Changes::default()),
                None => None,
            });
            Entry { number: g.number, created_unix: g.created_unix, active: g.active, name: info.name, changes }
        })
        .collect()
}

/// Name generation `number` (`None` or blank: remove the name).
pub fn set_name(dirs: &Dirs, number: u32, name: Option<&str>) -> Result<()> {
    let g = generation::list(dirs).into_iter().find(|g| g.number == number).with_context(|| format!("no generation {number}"))?;
    let name = name.map(|n| n.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|n| !n.is_empty());
    if name.as_ref().is_some_and(|n| n.chars().count() > NAME_MAX) {
        bail!("a generation name can have at most {NAME_MAX} characters");
    }
    let _lock = generation::lock(dirs)?;
    let mut info = generation::read_info(&g.path);
    info.name = name;
    generation::write_info(&g.path, &info)
}

/// The generated files that generation `number` changed, as unified diffs
/// against its base (everything is new for the first one).
pub fn file_diff(dirs: &Dirs, number: u32) -> Result<Vec<FileDiff>> {
    let all = generation::list(dirs);
    let g = all.iter().find(|g| g.number == number).with_context(|| format!("no generation {number}"))?;
    let new = tree(&g.zsh_dir());
    let (base_name, old) = match base_of(g, &all) {
        Some(b) => (b.number.to_string(), tree(&b.zsh_dir())),
        None if number == 1 => ("none".to_string(), Tree::new()),
        None => bail!("the generation {number} was built on isn't kept any more"),
    };
    let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    Ok(keys
        .into_iter()
        .filter_map(|k| {
            let (o, n) = (old.get(k).map_or("", String::as_str), new.get(k).map_or("", String::as_str));
            (o != n).then(|| FileDiff {
                name: k.clone(),
                unified: similar::TextDiff::from_lines(o, n)
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("{base_name}/{k}"), &format!("{number}/{k}"))
                    .to_string(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zsh::GeneratedFile;

    fn change(key: &str, old: Option<&str>, new: Option<&str>) -> SettingChange {
        SettingChange { key: key.into(), old: old.map(Into::into), new: new.map(Into::into) }
    }

    #[test]
    fn settings_changes_fill_in_defaults() {
        let c = settings_changes("{}", r#"{"history": {"size": 5000}}"#);
        assert_eq!(c, [change("history.size", Some("100000"), Some("5000"))]);
        assert_eq!(c[0].text(), "history.size 100000 → 5000");
        assert!(settings_changes(r#"{"version": 3}"#, "{}").is_empty(), "same settings, other spelling");
        assert!(settings_changes("{}", r#"{"nonsense": 1}"#).is_empty(), "unknown keys don't count");
    }

    #[test]
    fn settings_changes_read_well() {
        let texts = |a: &str, b: &str| settings_changes(a, b).iter().map(SettingChange::text).collect::<Vec<_>>();
        assert_eq!(texts("{}", r#"{"plugins": {"starship": {"enabled": false}}}"#), ["starship off"]);
        assert_eq!(texts(r#"{"plugins": {"starship": {"enabled": false}}}"#, "{}"), ["starship on"]);
        assert_eq!(texts("{}", r#"{"plugins": {"fzf-tab": {"settings": {"previews": false}}}}"#), ["fzf-tab.previews → off"]);
        assert_eq!(texts(r#"{"plugins": {"fzf-tab": {"settings": {"previews": false}}}}"#, "{}"), ["fzf-tab.previews reset"]);
        assert!(texts("{}", r#"{"plugins": {"eza": {}}}"#).is_empty(), "an entry at its defaults");
        let alias = r#"{"aliases": {"ll": {"command": "ls -l"}}}"#;
        assert_eq!(texts("{}", alias), ["alias ll added"]);
        assert_eq!(texts(alias, "{}"), ["alias ll removed"]);
        assert_eq!(texts(alias, r#"{"aliases": {"ll": {"command": "ls -la"}}}"#), ["alias ll.command ls -l → ls -la"]);
        let long = format!(r#"{{"shell": {{"program": "/{}"}}}}"#, "x".repeat(40));
        let t = texts("{}", &long);
        assert!(t[0].starts_with("shell.program default → /xxx") && t[0].ends_with('…'), "{t:?}");
    }

    #[test]
    fn short_summary_lists_or_counts() {
        let one = Changes { base: Some(1), settings: vec![change("history.size", Some("100000"), Some("5000"))], ..Default::default() };
        assert_eq!(one.short(2), "history.size 100000 → 5000");
        assert_eq!(one.short(5), "From 1: history.size 100000 → 5000", "built after a rollback");
        let two = Changes {
            settings: vec![change("prompt.two_line", Some("on"), Some("off")), change("x.y", Some("a"), Some("b"))],
            ..one.clone()
        };
        assert_eq!(two.short(2), "prompt.two_line on → off, x.y a → b");
        let many = Changes {
            settings: vec![
                change("history.size", Some("1"), Some("2")),
                change("history.share_between_sessions", Some("off"), Some("on")),
                change("aliases.ll", None, Some("")),
            ],
            plugins_added: vec!["eza".into()],
            ..one.clone()
        };
        assert_eq!(many.short(2), "4 changes: history, aliases, eza");
        assert_eq!(many.lines().last().unwrap(), "eza plugin added");
        assert_eq!(Changes { base: Some(1), files: 3, ..Default::default() }.short(2), "3 generated files changed");
        assert_eq!(
            Changes { base: Some(1), goatty: Some("0.2.0".into()), files: 1, ..Default::default() }.short(2),
            "built by goatty 0.2.0"
        );
        assert_eq!(Changes::default().short(1), "First configuration");
        // A plugin switched off doesn't also say its files went.
        let off = Changes {
            settings: vec![change("plugins.starship.enabled", Some("on"), Some("off"))],
            plugins_removed: vec!["starship".into()],
            ..one
        };
        assert_eq!(off.lines(), ["starship off"]);
    }

    fn files(rc: &str, plugin: Option<&str>) -> Vec<GeneratedFile> {
        let mut v = vec![GeneratedFile::new(".zshrc", rc)];
        v.extend(plugin.map(|p| GeneratedFile::new(format!("plugins/{p}/init.zsh"), "true\n")));
        v
    }

    #[test]
    fn generations_record_their_changes_and_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        generation::activate(&dirs, &files("a\n", None), "{}").unwrap();
        generation::activate(&dirs, &files("a\n", Some("demo")), r#"{"history": {"size": 5000}}"#).unwrap();
        generation::rollback(&dirs, Some(1)).unwrap();
        generation::activate(&dirs, &files("b\n", None), r#"{"prompt": {"two_line": false}}"#).unwrap();

        let l = list(&dirs);
        assert_eq!(
            l.iter().map(Entry::summary).collect::<Vec<_>>(),
            ["First configuration", "history.size 100000 → 5000, demo plugin added", "From 1: prompt.two_line on → off"]
        );
        assert_eq!(l[1].changes.as_ref().unwrap().files, 1);

        set_name(&dirs, 1, Some("  before   trying fish ")).unwrap();
        assert_eq!(list(&dirs)[0].title(), "before trying fish");
        assert_eq!(list(&dirs)[0].summary(), "First configuration", "naming keeps the rest");
        set_name(&dirs, 1, Some(" ")).unwrap();
        assert_eq!(list(&dirs)[0].title(), "Generation 1");
        assert!(set_name(&dirs, 1, Some(&"x".repeat(NAME_MAX + 1))).is_err());
        assert!(set_name(&dirs, 9, Some("x")).is_err());

        let d = file_diff(&dirs, 3).unwrap();
        assert_eq!(d.len(), 1, "against 1, its base, not 2");
        assert!(d[0].unified.contains("-a") && d[0].unified.contains("+b"), "{}", d[0].unified);
        assert_eq!(file_diff(&dirs, 2).unwrap().iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["plugins/demo/init.zsh"]);
        assert_eq!(file_diff(&dirs, 1).unwrap().len(), 1, "everything is new in the first");
    }

    #[test]
    fn older_generations_are_compared_on_the_fly() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        generation::activate(&dirs, &files("a\n", None), "{}").unwrap();
        generation::activate(&dirs, &files("b\n", None), r#"{"history": {"size": 5000}}"#).unwrap();
        // As written before #55: no parent, no changes.
        let g2 = dirs.generations_dir().join("2");
        std::fs::write(g2.join("info.json"), "{\"created\": 5, \"goatty\": \"0.0.1\"}\n").unwrap();
        let l = list(&dirs);
        assert_eq!(l[1].summary(), "history.size 100000 → 5000");
        assert_eq!(l[1].created_unix, 5);
        // A plugin that only adds aliases has no files of its own: its entry counts.
        generation::activate(&dirs, &files("c\n", None), r#"{"history": {"size": 5000}, "plugins": {"eza": {}}}"#).unwrap();
        assert_eq!(list(&dirs)[2].summary(), "eza plugin added");
        // Its base pruned: nothing to say.
        std::fs::remove_dir_all(dirs.generations_dir().join("1")).unwrap();
        assert_eq!(list(&dirs)[0].changes, None);
        assert_eq!(list(&dirs)[1].summary(), "eza plugin added", "recorded when it was built");
        assert!(file_diff(&dirs, 2).is_err());
    }
}
