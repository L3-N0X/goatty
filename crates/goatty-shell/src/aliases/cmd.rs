//! `goattyd alias …`: list, add, edit, switch and remove aliases on this
//! host. Same data as the settings window's Aliases page: the user's
//! aliases are `aliases` in `host.json`; plugin aliases can only be
//! switched off and on (`plugins.<id>.disabled_aliases`). Every change is
//! applied like the window's Apply: a validated new generation, then the
//! save.

use anyhow::{Result, bail};
use goatty_config::{HostSettings, UserAlias, alias_name_problem};
use goatty_plugin::Manifest;

use super::{PluginAliases, Source, table};
use crate::plugins::lock::{self, Lock};
use crate::service::{ApplyOutcome, HostService};

/// Installed plugins' manifests with their enabled state (unreadable ones
/// are skipped with a warning: `goattyd plugin list` reports them).
fn installed(svc: &HostService, settings: &HostSettings) -> Vec<(Manifest, bool)> {
    let Some(dirs) = svc.launcher.dirs() else { return Vec::new() };
    let lock = match Lock::load(dirs) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("{e:#}");
            return Vec::new();
        }
    };
    lock.plugins
        .iter()
        .filter_map(|(id, e)| match lock::manifest_of(dirs, id, e) {
            Ok(m) => Some((m, settings.plugins.get(id).is_none_or(|c| c.enabled))),
            Err(e) => {
                log::warn!("{e:#}");
                None
            }
        })
        .collect()
}

fn print_outcome(o: &ApplyOutcome) {
    match o.generation {
        Some(g) if !o.diff.is_empty() => {
            println!("Applied (generation {g}). Running shells: `goattyd shell reload`, or `goatty-reload` in this one.")
        }
        _ => println!("Saved; the shell configuration didn't change."),
    }
    for i in &o.issues {
        println!("\u{26a0} {i}");
    }
}

/// `goattyd alias list`: the user's aliases by group, then each plugin's,
/// with what they run and why one isn't active.
pub fn list(svc: &HostService) -> Result<()> {
    let settings = svc.settings().value;
    let plugins = installed(svc, &settings);
    let refs: Vec<PluginAliases<'_>> = plugins.iter().map(|(m, enabled)| PluginAliases { manifest: m, enabled: *enabled }).collect();
    let groups = table(&settings, &refs);
    if groups.is_empty() {
        println!("No aliases yet. Add one: goattyd alias add gs git status");
        return Ok(());
    }
    let width = groups.iter().flat_map(|g| &g.rows).map(|r| r.name.chars().count()).max().unwrap_or(0).max(4);
    for (i, g) in groups.iter().enumerate() {
        if i > 0 {
            println!();
        }
        match &g.source {
            Source::User { .. } => println!("{}", g.title()),
            Source::Plugin { id, enabled, .. } => {
                println!("{} (plugin {id}{})", g.title(), if *enabled { "" } else { ", disabled" })
            }
        }
        for r in &g.rows {
            let mark = if r.active { '+' } else { '-' };
            let mut line = format!("  {mark} {:<width$}  {}", r.name, r.expansion);
            if !r.description.is_empty() {
                line.push_str(&format!("   # {}", r.description));
            }
            if let Some(n) = &r.note {
                line.push_str(&format!("   ({n})"));
            }
            println!("{line}");
        }
    }
    if !settings.shell.managed {
        println!("\n\u{26a0} goatty's managed zsh is off on this host (shell.managed): these aliases aren't loaded.");
    }
    Ok(())
}

/// The command from the words after the name: one word is taken as it is
/// (quote a command with pipes or quotes as one argument), several are
/// joined with spaces.
pub fn command_from(words: &[String]) -> String {
    words.join(" ").trim().to_owned()
}

/// `goattyd alias add NAME COMMAND… [--group G] [--description D] [--disabled] [--force]`.
pub fn add(
    svc: &HostService,
    name: &str,
    command: &str,
    group: Option<&str>,
    description: Option<&str>,
    enabled: bool,
    force: bool,
) -> Result<()> {
    if let Some(why) = alias_name_problem(name) {
        bail!("{name:?}: {why}");
    }
    if command.trim().is_empty() {
        bail!("give the command the alias runs: goattyd alias add {name} COMMAND…");
    }
    let settings = svc.settings().value;
    if let Some(existing) = settings.aliases.get(name).filter(|_| !force) {
        bail!("{name} already exists ({}); change it with `goattyd alias edit {name} …`, or pass --force to replace it", existing.command);
    }
    for (m, _) in installed(svc, &settings) {
        if m.aliases.iter().any(|a| a.name == name) {
            println!("Note: the {} plugin also has `{name}`; yours replaces it.", m.name);
        }
    }
    let path = std::env::var("PATH").ok();
    if let Some(dirs) = svc.launcher.dirs()
        && let Some(p) = crate::packages::which(name, path.as_deref(), &dirs.bin_dir())
    {
        println!("Note: `{name}` is also a command ({}); in goatty shells the alias runs instead.", p.display());
    }
    let alias = UserAlias {
        command: command.trim().to_owned(),
        group: group.unwrap_or_default().trim().to_owned(),
        description: description.unwrap_or_default().trim().to_owned(),
        enabled,
    };
    let name = name.to_owned();
    print_outcome(&svc.edit(move |h| {
        h.aliases.insert(name, alias);
    })?);
    Ok(())
}

/// What `goattyd alias edit` changes (`None`: keep).
#[derive(Debug, Default, Clone)]
pub struct Edit {
    pub rename: Option<String>,
    pub command: Option<String>,
    pub group: Option<String>,
    pub description: Option<String>,
}

/// `goattyd alias edit NAME [--name NEW] [--command C] [--group G] [--description D]`.
pub fn edit(svc: &HostService, name: &str, e: Edit) -> Result<()> {
    let settings = svc.settings().value;
    let Some(mut alias) = settings.aliases.get(name).cloned() else {
        bail!("you have no alias {name:?} (plugin aliases can only be switched off: `goattyd alias disable {name}`)");
    };
    if e.rename.is_none() && e.command.is_none() && e.group.is_none() && e.description.is_none() {
        bail!("nothing to change: pass --command, --name, --group or --description");
    }
    if let Some(c) = e.command {
        if c.trim().is_empty() {
            bail!("the command can't be empty (remove the alias with `goattyd alias remove {name}`)");
        }
        alias.command = c.trim().to_owned();
    }
    if let Some(g) = e.group {
        alias.group = g.trim().to_owned();
    }
    if let Some(d) = e.description {
        alias.description = d.trim().to_owned();
    }
    let new_name = e.rename.unwrap_or_else(|| name.to_owned());
    if let Some(why) = alias_name_problem(&new_name) {
        bail!("{new_name:?}: {why}");
    }
    if new_name != name && settings.aliases.contains_key(&new_name) {
        bail!("{new_name} already exists; remove it first or pick another name");
    }
    let old = name.to_owned();
    print_outcome(&svc.edit(move |h| {
        h.aliases.remove(&old);
        h.aliases.insert(new_name, alias);
    })?);
    Ok(())
}

/// `goattyd alias remove NAME…` (the user's aliases).
pub fn remove(svc: &HostService, names: &[String]) -> Result<()> {
    let settings = svc.settings().value;
    if let Some(missing) = names.iter().find(|n| !settings.aliases.contains_key(*n)) {
        bail!("you have no alias {missing:?} (plugin aliases can only be switched off: `goattyd alias disable {missing}`)");
    }
    let names = names.to_vec();
    print_outcome(&svc.edit(move |h| {
        for n in &names {
            h.aliases.remove(n);
        }
    })?);
    Ok(())
}

/// Which aliases `enable`/`disable` act on.
#[derive(Debug, Default, Clone)]
pub struct Selection {
    pub names: Vec<String>,
    /// Every alias of the user's group (compared without case).
    pub group: Option<String>,
    /// That plugin's aliases (all of them without names).
    pub plugin: Option<String>,
}

/// `goattyd alias enable|disable [NAME…] [--group G] [--plugin ID]`. A
/// bare name means your alias when you have one, else every plugin alias
/// with that name.
pub fn set_enabled(svc: &HostService, sel: Selection, enabled: bool) -> Result<()> {
    let settings = svc.settings().value;
    let plugins = installed(svc, &settings);
    let mut user: Vec<String> = Vec::new();
    let mut plugin: Vec<(String, String)> = Vec::new(); // (plugin id, alias name)
    if let Some(id) = &sel.plugin {
        let Some((m, _)) = plugins.iter().find(|(m, _)| m.id == *id) else { bail!("{id} isn't installed") };
        if m.aliases.is_empty() {
            bail!("{} has no aliases", m.name);
        }
        let names: Vec<String> = if sel.names.is_empty() { m.aliases.iter().map(|a| a.name.clone()).collect() } else { sel.names.clone() };
        for n in names {
            if !m.aliases.iter().any(|a| a.name == n) {
                let have: Vec<&str> = m.aliases.iter().map(|a| a.name.as_str()).collect();
                bail!("{} has no alias {n:?}; it has: {}", m.name, have.join(", "));
            }
            plugin.push((id.clone(), n));
        }
    } else if let Some(g) = &sel.group {
        let g = g.trim().to_lowercase();
        user.extend(settings.aliases.iter().filter(|(_, a)| a.group.to_lowercase() == g).map(|(n, _)| n.clone()));
        if user.is_empty() {
            bail!("you have no aliases in a group {:?}", sel.group.as_deref().unwrap_or_default());
        }
        if !sel.names.is_empty() {
            bail!("pass names or --group, not both");
        }
    } else {
        if sel.names.is_empty() {
            bail!("which aliases? Pass names, --group G or --plugin ID");
        }
        for n in &sel.names {
            if settings.aliases.contains_key(n) {
                user.push(n.clone());
                continue;
            }
            let from: Vec<&Manifest> = plugins.iter().map(|(m, _)| m).filter(|m| m.aliases.iter().any(|a| a.name == *n)).collect();
            if from.is_empty() {
                bail!("no alias {n:?} (see `goattyd alias list`)");
            }
            plugin.extend(from.into_iter().map(|m| (m.id.clone(), n.clone())));
        }
    }
    print_outcome(&svc.edit(move |h| {
        for n in &user {
            if let Some(a) = h.aliases.get_mut(n) {
                a.enabled = enabled;
            }
        }
        for (id, n) in plugin {
            let off = &mut h.plugins.entry(id).or_default().disabled_aliases;
            if enabled {
                off.remove(&n);
            } else {
                off.insert(n);
            }
        }
    })?);
    Ok(())
}
