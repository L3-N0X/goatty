//! Aliases on a host (#88): the user's own (`aliases` in `host.json`) and
//! the ones installed plugins bring, as one table for the settings
//! window's Aliases page and `goattyd alias list`, and the generated
//! `aliases.zsh`.
//!
//! The managed `.zshrc` reads `plugins.zsh` (plugin aliases) and then
//! `aliases.zsh` (the user's), so a user alias with a plugin alias's name
//! replaces it. Between plugins, the one loaded later (by id) wins. A
//! plugin alias the user switched off is left out of `plugins.zsh`
//! ([`crate::plugins::active`] drops it from the manifest it renders).

pub mod cmd;

use std::collections::BTreeMap;

use goatty_config::{HostSettings, UserAlias};
use goatty_plugin::Manifest;
use goatty_plugin::alias::views;
use goatty_plugin::schema::resolve;

use crate::zsh::quote;

/// An installed plugin, for [`table`].
#[derive(Debug, Clone, Copy)]
pub struct PluginAliases<'a> {
    pub manifest: &'a Manifest,
    /// Installed but switched off (`plugins.<id>.enabled = false`).
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The user's: `group` is the heading ("" = no group).
    User {
        group: String,
    },
    Plugin {
        id: String,
        name: String,
        enabled: bool,
    },
}

/// A heading in the list and its aliases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub source: Source,
    pub rows: Vec<Row>,
}

impl Group {
    /// The heading: the group's name, "Ungrouped", or the plugin's name.
    pub fn title(&self) -> &str {
        match &self.source {
            Source::User { group } if group.is_empty() => "Ungrouped",
            Source::User { group } => group,
            Source::Plugin { name, .. } => name,
        }
    }

    pub fn plugin_id(&self) -> Option<&str> {
        match &self.source {
            Source::Plugin { id, .. } => Some(id),
            Source::User { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// What it runs (a plugin's: with its current settings).
    pub expansion: String,
    pub description: String,
    /// The user's switch for this alias.
    pub enabled: bool,
    /// New shells define it as shown.
    pub active: bool,
    /// Why it isn't active, or what it replaces.
    pub note: Option<String>,
    /// A plugin alias's condition, in words (`"Short aliases" is on`).
    pub condition: Option<String>,
}

/// Every alias for `settings`: the user's groups (ungrouped first, then by
/// name), then each plugin with aliases (in load order, by id).
pub fn table(settings: &HostSettings, plugins: &[PluginAliases<'_>]) -> Vec<Group> {
    let mut plugins: Vec<PluginAliases<'_>> = plugins.iter().filter(|p| !p.manifest.aliases.is_empty()).copied().collect();
    plugins.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));

    // Plugin rows first: which ones are defined, and which plugin defines a
    // name last (that one wins in the shell).
    let mut plugin_groups = Vec::new();
    let mut defined_by: BTreeMap<String, String> = BTreeMap::new(); // name → plugin name
    for p in &plugins {
        let m = p.manifest;
        let config = settings.plugins.get(&m.id);
        let stored = config.map(|c| c.settings.clone()).unwrap_or_default();
        let values = resolve(m, &stored).0;
        let rows: Vec<Row> = views(m, &values)
            .into_iter()
            .map(|v| {
                let enabled = config.is_none_or(|c| !c.disabled_aliases.contains(&v.name));
                let note = if !p.enabled {
                    Some("the plugin is disabled".to_owned())
                } else if !v.on {
                    v.condition.as_ref().map(|c| format!("only while {c} in the plugin's settings"))
                } else if !enabled {
                    Some("switched off".to_owned())
                } else {
                    None
                };
                let active = p.enabled && v.on && enabled;
                if active {
                    defined_by.insert(v.name.clone(), m.name.clone());
                }
                Row { name: v.name, expansion: v.expansion, description: v.description, enabled, active, note, condition: v.condition }
            })
            .collect();
        plugin_groups.push(Group { source: Source::Plugin { id: m.id.clone(), name: m.name.clone(), enabled: p.enabled }, rows });
    }
    // Later plugins and the user's aliases replace earlier definitions.
    for g in &mut plugin_groups {
        let name = g.title().to_owned();
        for r in g.rows.iter_mut().filter(|r| r.active) {
            if let Some(user) = settings.aliases.get(&r.name).filter(|a| a.enabled) {
                r.active = false;
                r.note = Some(format!("replaced by your alias ({})", user.command));
            } else if let Some(other) = defined_by.get(&r.name).filter(|o| **o != name) {
                r.active = false;
                r.note = Some(format!("replaced by {other}'s alias"));
            }
        }
    }

    // Groups compare without case ("git" and "Git" are one, titled by the
    // first alias's spelling).
    let mut by_group: BTreeMap<String, (String, Vec<Row>)> = BTreeMap::new();
    for (name, a) in &settings.aliases {
        let note = (a.enabled)
            .then(|| defined_by.get(name).map(|p| format!("replaces {p}'s alias")))
            .flatten()
            .or_else(|| (!a.enabled).then(|| "switched off".to_owned()));
        let row = Row {
            name: name.clone(),
            expansion: a.command.clone(),
            description: a.description.clone(),
            enabled: a.enabled,
            active: a.enabled,
            note,
            condition: None,
        };
        by_group.entry(a.group.to_lowercase()).or_insert_with(|| (a.group.clone(), Vec::new())).1.push(row);
    }
    let mut out: Vec<Group> = by_group.into_values().map(|(group, rows)| Group { source: Source::User { group }, rows }).collect();
    out.extend(plugin_groups);
    out
}

/// The user's group names, sorted, compared without case (no empty one).
pub fn user_groups(settings: &HostSettings) -> Vec<String> {
    let mut groups: BTreeMap<String, String> = BTreeMap::new();
    for a in settings.aliases.values().filter(|a| !a.group.is_empty()) {
        groups.entry(a.group.to_lowercase()).or_insert_with(|| a.group.clone());
    }
    groups.into_values().collect()
}

/// An alias with its name.
type Named<'a> = (&'a String, &'a UserAlias);

/// `aliases.zsh`: the user's enabled aliases (always written, so the
/// `.zshrc` can source it unconditionally).
pub fn render(settings: &HostSettings) -> String {
    let mut out = String::from(
        "# goatty: your aliases (\"aliases\" in host.json), read after the plugins' aliases so yours win.\n\
         # Generated by goatty: change them in goatty's settings window (Aliases) or with `goattyd alias`.\n",
    );
    // Lowercase group → (its title, its aliases).
    let mut groups: BTreeMap<String, (&str, Vec<Named<'_>>)> = BTreeMap::new();
    for (name, a) in &settings.aliases {
        groups.entry(a.group.to_lowercase()).or_insert_with(|| (&a.group, Vec::new())).1.push((name, a));
    }
    for (group, aliases) in groups.into_values() {
        out.push('\n');
        if !group.is_empty() {
            out.push_str(&format!("# {}\n", group.replace('\n', " ")));
        }
        for (name, a) in aliases {
            if a.enabled {
                // Names are restricted to characters zsh takes literally (`is_alias_name`).
                out.push_str(&format!("alias -- {name}={}\n", quote(&a.command)));
            } else {
                out.push_str(&format!("# off: {name}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use goatty_config::PluginConfig;

    use super::*;

    const DOCKER: &str = r#"
schema = 1
id = "docker"
name = "Docker"
version = "1.0.0"
summary = "s"
publisher = "p"
[requires]
commands = ["docker"]
[[setting]]
id = "aliases"
label = "Short aliases"
type = "bool"
default = true
[[alias]]
name = "dps"
command = "docker"
args = ["ps"]
when = "aliases"
description = "Running containers"
[[alias]]
name = "di"
command = "docker"
args = ["images"]
when = "aliases"
[[alias]]
name = "d"
command = "docker"
"#;

    fn docker() -> Manifest {
        goatty_plugin::manifest::parse(DOCKER, None).unwrap()
    }

    fn settings() -> HostSettings {
        let mut s = HostSettings::default();
        s.aliases.insert("gs".into(), UserAlias { group: "Git".into(), ..UserAlias::new("git status") });
        s.aliases.insert("gl".into(), UserAlias { group: "git".into(), enabled: false, ..UserAlias::new("git log") });
        s.aliases.insert("..".into(), UserAlias::new("cd .."));
        s.aliases.insert("d".into(), UserAlias::new("docker compose"));
        s
    }

    #[test]
    fn groups_and_overrides() {
        let m = docker();
        let mut s = settings();
        s.plugins.insert("docker".into(), PluginConfig { disabled_aliases: ["di".to_owned()].into(), ..Default::default() });
        let t = table(&s, &[PluginAliases { manifest: &m, enabled: true }]);
        let titles: Vec<&str> = t.iter().map(Group::title).collect();
        assert_eq!(titles, ["Ungrouped", "git", "Docker"], "{t:#?}");

        let ungrouped = &t[0].rows;
        assert_eq!(ungrouped.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["..", "d"]);
        assert_eq!(ungrouped[1].note.as_deref(), Some("replaces Docker's alias"));
        assert_eq!(t[1].rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["gl", "gs"], "one group without case");
        assert_eq!(t[1].rows[0].note.as_deref(), Some("switched off"));
        assert!(!t[1].rows[0].active);

        let docker = &t[2];
        assert_eq!(docker.plugin_id(), Some("docker"));
        let row = |n: &str| docker.rows.iter().find(|r| r.name == n).unwrap();
        assert!(row("dps").active);
        assert_eq!(row("dps").expansion, "docker ps");
        assert_eq!(row("dps").description, "Running containers");
        assert!(!row("di").enabled && !row("di").active);
        assert_eq!(row("d").note.as_deref(), Some("replaced by your alias (docker compose)"));
        assert!(!row("d").active);

        // The plugin's condition, and the plugin switched off.
        s.plugins.get_mut("docker").unwrap().settings.insert("aliases".into(), false.into());
        let t = table(&s, &[PluginAliases { manifest: &m, enabled: true }]);
        let dps = t[2].rows.iter().find(|r| r.name == "dps").unwrap();
        assert!(!dps.active);
        assert_eq!(dps.note.as_deref(), Some("only while \"Short aliases\" is on in the plugin's settings"));
        let t = table(&s, &[PluginAliases { manifest: &m, enabled: false }]);
        assert!(t[2].rows.iter().all(|r| !r.active && r.note.as_deref() == Some("the plugin is disabled")));
        assert_eq!(t[0].rows[1].note, None, "nothing to replace from a disabled plugin");
    }

    #[test]
    fn later_plugins_win() {
        let a = docker();
        let mut b = docker();
        b.id = "podman".into();
        b.name = "Podman".into();
        let t = table(
            &HostSettings::default(),
            &[PluginAliases { manifest: &b, enabled: true }, PluginAliases { manifest: &a, enabled: true }],
        );
        assert_eq!(t[0].title(), "Docker", "in load order");
        assert_eq!(t[0].rows[0].note.as_deref(), Some("replaced by Podman's alias"));
        assert!(t[1].rows[0].active);
    }

    #[test]
    fn renders_enabled_aliases_quoted() {
        let mut s = settings();
        s.aliases.insert("q".into(), UserAlias::new("echo 'it''s' | less"));
        let out = render(&s);
        assert!(out.contains("alias -- ..='cd ..'\n"), "{out}");
        assert!(out.contains("# git\n# off: gl\nalias -- gs='git status'\n"), "{out}");
        assert!(out.contains(r"alias -- q='echo '\''it'\'''\''s'\'' | less'"), "{out}");
        assert_eq!(user_groups(&s), ["git"]);
        assert!(render(&HostSettings::default()).starts_with("# goatty"));
    }
}
