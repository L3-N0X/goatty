//! `goattyd plugin …`: search, inspect, install, configure and remove
//! plugins on this host from the command line. Same operations as the
//! settings window's Plugins page (through [`HostService`]).

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use goatty_plugin::api::{Changed, PackageState, Plan};
use goatty_plugin::manifest::SettingKind;
use goatty_plugin::{Permission, schema};

use super::Candidate;
use super::registry::{self, Source};
use crate::service::HostService;

fn size(bytes: u64) -> String {
    if bytes < 1_000_000 { format!("{} kB", bytes.div_ceil(1000)) } else { format!("{:.1} MB", bytes as f64 / 1e6) }
}

fn badges(perms: &[Permission]) -> String {
    let mut b = Vec::new();
    if perms.contains(&Permission::Binaries) {
        b.push("installs programs");
    }
    if perms.contains(&Permission::ShellCode) {
        b.push("runs shell code");
    }
    if perms.contains(&Permission::Network) {
        b.push("uses the network");
    }
    if b.is_empty() { String::new() } else { format!("  [{}]", b.join(", ")) }
}

/// Ask; anything but y/yes is no. Without a terminal, `--yes` is required.
fn approve(prompt: &str) -> Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("not asking without a terminal; review the plan above and pass --yes to approve it");
    }
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

pub fn print_changed(c: &Changed) {
    for l in &c.lines {
        println!("{l}");
    }
    if c.reload {
        println!("Running shells: `goattyd shell reload` (or `goatty-reload` in one shell).");
    }
}

/// `goattyd plugin search [QUERY]`.
pub fn search(svc: &HostService, query: Option<&str>, refresh: bool) -> Result<()> {
    let cat = svc.plugins(|p| Ok(p.catalog(refresh, &svc.settings().value)))?;
    for r in cat.registries.iter().filter(|r| r.error.is_some()) {
        println!("registry {}: {}", r.name, r.error.as_deref().unwrap_or_default());
    }
    let q = query.map(str::to_lowercase);
    let hits: Vec<_> = cat
        .available
        .iter()
        .filter(|a| {
            q.as_deref().is_none_or(|q| {
                a.id.contains(q)
                    || a.name.to_lowercase().contains(q)
                    || a.summary.to_lowercase().contains(q)
                    || a.keywords.iter().any(|k| k.contains(q))
            })
        })
        .collect();
    if hits.is_empty() {
        println!("No plugins found{}.", query.map(|q| format!(" for {q:?}")).unwrap_or_default());
        return Ok(());
    }
    let width = hits.iter().map(|a| a.id.len()).max().unwrap_or(0);
    for a in hits {
        let state = match (&a.installed, &a.incompatible) {
            (Some(v), _) if *v == a.version => "  (installed)".to_string(),
            (Some(v), _) => format!("  (installed {v}; update available)"),
            (None, Some(why)) => format!("  (not for this host: {why})"),
            _ => String::new(),
        };
        println!("{:<width$}  {:<8} {}{}{state}", a.id, a.version, a.summary, badges(&a.permissions));
    }
    println!("\nDetails: goattyd plugin info ID · install: goattyd plugin install ID");
    Ok(())
}

/// `goattyd plugin info ID`.
pub fn info(svc: &HostService, id: &str) -> Result<()> {
    let c = svc.plugins(|p| p.find(id, None)).or_else(|e| match local_installed(svc, id) {
        Some(c) => Ok(c),
        None => Err(e),
    })?;
    let m = &c.manifest;
    println!("{} {}  ({})", m.name, m.version, m.id);
    println!("  {}", m.summary);
    if !m.description.trim().is_empty() {
        for line in m.description.trim().lines() {
            println!("  {line}");
        }
    }
    println!("  publisher  {}", m.publisher);
    if let Some(h) = &m.homepage {
        println!("  homepage   {h}");
    }
    let how = match (c.builtin, c.verified) {
        (Some(_), _) => " (comes with goatty)",
        (None, true) => " (signed registry)",
        (None, false) => " (local, unverified)",
    };
    println!("  source     {}{how}", c.source);
    let stored = svc.settings().value.plugins.get(id).map(|p| p.settings.clone()).unwrap_or_default();
    let (values, _) = schema::resolve(m, &stored);
    if !m.settings.is_empty() {
        println!("\nSettings (goattyd plugin set {id} KEY VALUE):");
        for s in &m.settings {
            let v = values.get(&s.id).map(schema::plain_string).unwrap_or_default();
            let kind = match s.kind {
                SettingKind::Choice => s.choices.iter().map(|c| c.value.as_str()).collect::<Vec<_>>().join("|"),
                k => k.label().to_owned(),
            };
            println!("  {:<18} {v:<12} {} ({kind})", s.id, s.label);
            if !s.help.is_empty() {
                println!("  {:<18} {:<12} {}", "", "", s.help);
            }
        }
    }
    println!("\nPermissions:");
    for g in goatty_plugin::permissions::describe(m) {
        println!("  {} {}{}", if g.permission.runs_code() { "⚠" } else { "•" }, g.title, details(&g.details));
        if !g.reason.is_empty() {
            println!("      why: {}", g.reason);
        }
    }
    Ok(())
}

fn details(d: &[String]) -> String {
    if d.is_empty() { String::new() } else { format!(": {}", d.join(", ")) }
}

/// An installed local plugin, found again from its directory.
fn local_installed(svc: &HostService, id: &str) -> Option<Candidate> {
    let dirs = svc.launcher.dirs()?;
    let lock = super::lock::Lock::load(dirs).ok()?;
    let e = lock.plugins.get(id)?;
    let m = super::lock::manifest_of(dirs, id, e).ok()?;
    Some(Candidate {
        manifest: m,
        files: e.files.clone(),
        dir: super::lock::bundle_dir(dirs, id),
        source: e.source.clone(),
        verified: e.verified,
        builtin: None,
    })
}

/// Print an install plan the way the user approves it.
pub fn print_plan(plan: &Plan) {
    let verb = match &plan.replaces {
        Some(old) if *old != plan.version => format!("Update {} {old} → {}", plan.name, plan.version),
        Some(_) => format!("Reinstall {} {}", plan.name, plan.version),
        None => format!("Install {} {}", plan.name, plan.version),
    };
    let from = if plan.builtin {
        "goatty (built in)".to_string()
    } else if plan.verified {
        format!("registry {:?} (signature checked)", plan.source)
    } else {
        plan.source.clone()
    };
    println!("{verb} from {from}");
    println!("  {}", plan.summary);
    let mut who = vec![format!("by {}", plan.publisher)];
    who.extend(plan.homepage.clone());
    who.extend(plan.license.clone());
    println!("  {}", who.join(" · "));

    if !plan.downloads.is_empty() {
        println!("\nDownloads into goatty's own directory (no sudo; checked against the sha256 before anything is unpacked):");
        for d in &plan.downloads {
            match &d.system {
                Some(p) => println!("  {} {}: already installed at {p}; goatty uses that copy", d.package, d.version),
                None if d.current => println!("  {} {}: goatty's copy is already installed", d.package, d.version),
                None => {
                    println!("  {} {}  {}  {}", d.package, d.version, size(d.size), d.url);
                    println!("      sha256 {}", d.sha256);
                }
            }
        }
        let bins: Vec<&str> = plan.downloads.iter().flat_map(|d| d.bins.iter().map(String::as_str)).collect();
        if !bins.is_empty() {
            println!("  On PATH in goatty shells: {}", bins.join(", "));
        }
        let scripts: Vec<&str> = plan.downloads.iter().filter_map(|d| d.script.as_deref()).collect();
        if !scripts.is_empty() {
            println!("  Sourced by goatty shells: {}", scripts.join(", "));
        }
    }
    println!("\nPermissions:");
    for g in &plan.permissions {
        println!("  {} {}{}", if g.permission.runs_code() { "⚠" } else { "•" }, g.title, details(&g.details));
        if !g.reason.is_empty() {
            println!("      why: {}", g.reason);
        }
        println!("      {}", g.meaning);
    }
    if !plan.aliases.is_empty() {
        println!("\nAliases:");
        for a in &plan.aliases {
            println!("  {a}");
        }
    }
    if !plan.env.is_empty() {
        println!("\nEnvironment:");
        for e in &plan.env {
            println!("  {e}");
        }
    }
    if let Some((file, code)) = &plan.shell_code {
        println!("\nShell code ({file}), run at the start of every goatty shell:");
        for line in code.lines() {
            println!("  │ {line}");
        }
    }
    for r in &plan.requires {
        println!("\nNeeds {r}");
    }
    for w in &plan.warnings {
        println!("\n⚠ {w}");
    }
    println!();
}

/// `goattyd plugin install ID[@VERSION]… | --path DIR`.
pub fn install(svc: &HostService, targets: &[String], path: Option<&Path>, yes: bool) -> Result<()> {
    let mut candidates = Vec::new();
    if let Some(dir) = path {
        candidates.push(Candidate::local(dir)?);
    }
    for t in targets {
        let (id, version) = match t.split_once('@') {
            Some((id, v)) => (id, Some(v)),
            None => (t.as_str(), None),
        };
        candidates.push(svc.plugins(|p| p.find(id, version))?);
    }
    if candidates.is_empty() {
        bail!("name the plugins to install (goattyd plugin search lists them), or pass --path DIR");
    }
    let settings = svc.settings().value;
    for c in candidates {
        let plan = svc.plugins(|p| p.plan(&c, &settings))?;
        print_plan(&plan);
        if !yes && !approve(&format!("{} {}?", if plan.replaces.is_some() { "Update" } else { "Install" }, plan.name))? {
            println!("Not installed.");
            continue;
        }
        print_changed(&svc.install_plugin(&c, &plan.digest)?);
    }
    Ok(())
}

/// `goattyd plugin update [ID…]`: newer versions from the registries.
pub fn update(svc: &HostService, ids: &[String], yes: bool) -> Result<()> {
    let cat = svc.plugins(|p| Ok(p.catalog(true, &svc.settings().value)))?;
    let todo: Vec<_> = cat.installed.iter().filter(|i| i.update.is_some() && (ids.is_empty() || ids.contains(&i.id))).collect();
    if todo.is_empty() {
        println!("Everything is up to date.");
        return Ok(());
    }
    let settings = svc.settings().value;
    for i in todo {
        let c = svc.plugins(|p| p.find(&i.id, i.update.as_deref()))?;
        let plan = svc.plugins(|p| p.plan(&c, &settings))?;
        print_plan(&plan);
        if !yes && !approve(&format!("Update {}?", plan.name))? {
            println!("Not updated.");
            continue;
        }
        print_changed(&svc.install_plugin(&c, &plan.digest)?);
    }
    Ok(())
}

/// `goattyd plugin uninstall ID…`.
pub fn uninstall(svc: &HostService, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        bail!("name the plugins to remove");
    }
    for id in ids {
        print_changed(&svc.uninstall_plugin(id)?);
    }
    Ok(())
}

/// `goattyd plugin list`.
pub fn list(svc: &HostService) -> Result<()> {
    let cat = svc.plugins(|p| Ok(p.catalog(false, &svc.settings().value)))?;
    if cat.installed.iter().all(|i| i.builtin) {
        println!("Only goatty's built-in plugins so far. Browse more with `goattyd plugin search`.");
    }
    for i in &cat.installed {
        let mut notes = Vec::new();
        if !i.enabled {
            notes.push(if i.builtin { "off" } else { "disabled" }.to_string());
        }
        if i.builtin {
            notes.push("built in".into());
            for p in i.packages.iter().filter(|p| p.needs_install()) {
                notes.push(match &p.state {
                    PackageState::Managed { version } => format!("{} {version} → {}: goattyd shell install {}", p.id, p.version, i.id),
                    _ => format!("{} not installed: goattyd shell install {}", p.id, i.id),
                });
            }
        } else if !i.verified {
            notes.push("local, unverified".into());
        }
        if let Some(why) = &i.incompatible {
            notes.push(format!("off: {why}"));
        }
        if let Some(u) = &i.update {
            notes.push(format!("{u} available"));
        }
        let notes = if notes.is_empty() { String::new() } else { format!("  ({})", notes.join("; ")) };
        println!("{:<20} {:<8} {:<10} {}{notes}", i.id, i.version, i.source, i.manifest.summary);
    }
    for issue in &cat.issues {
        println!("⚠ {issue}");
    }
    Ok(())
}

fn installed_manifest(svc: &HostService, id: &str) -> Result<goatty_plugin::Manifest> {
    if let Some(b) = crate::builtins::get(id) {
        return Ok(b.manifest().clone());
    }
    let dirs = svc.launcher.dirs().context("$HOME is not set")?;
    let lock = super::lock::Lock::load(dirs)?;
    let e = lock.plugins.get(id).with_context(|| format!("{id} isn't installed"))?;
    super::lock::manifest_of(dirs, id, e)
}

fn print_outcome(o: &crate::service::ApplyOutcome) {
    match o.generation {
        Some(g) if !o.diff.is_empty() => println!("Applied (generation {g}). Running shells: `goattyd shell reload`."),
        _ => println!("Saved; the shell configuration didn't change."),
    }
    for i in &o.issues {
        println!("⚠ {i}");
    }
}

/// `goattyd plugin enable|disable ID`.
pub fn set_enabled(svc: &HostService, id: &str, enabled: bool) -> Result<()> {
    installed_manifest(svc, id)?;
    let id = id.to_owned();
    print_outcome(&svc.edit(|h| h.edit_plugin(&id, |p| p.enabled = enabled))?);
    Ok(())
}

/// `goattyd plugin set ID KEY VALUE` (checked against the plugin's schema),
/// or with `value: None` back to the default.
pub fn set(svc: &HostService, id: &str, key: &str, value: Option<&str>) -> Result<()> {
    let m = installed_manifest(svc, id)?;
    let Some(s) = m.setting(key) else {
        let keys: Vec<&str> = m.settings.iter().map(|s| s.id.as_str()).collect();
        bail!("{} has no setting {key:?}; it has: {}", m.name, keys.join(", "));
    };
    let v = value.map(|t| schema::parse_input(s, t).map_err(anyhow::Error::msg)).transpose()?;
    let (id, key) = (id.to_owned(), key.to_owned());
    print_outcome(&svc.edit(|h| {
        h.edit_plugin(&id, |entry| {
            match v {
                Some(v) => entry.settings.insert(key, v),
                None => entry.settings.remove(&key),
            };
        })
    })?);
    Ok(())
}

/// `goattyd plugin registry list`.
pub fn registries(svc: &HostService, refresh: bool) -> Result<()> {
    let (_, status, issues) = svc.plugins(|p| Ok(p.snapshots(refresh, false)))?;
    for r in status {
        let state = match (&r.error, r.sequence) {
            (Some(e), _) => format!("error: {e}"),
            (None, Some(seq)) => format!("{} plugins, index #{seq}", r.plugins),
            (None, None) => "not fetched yet".into(),
        };
        println!("{:<12} {}{}\n             {state}", r.name, r.url, if r.builtin { " (built in)" } else { "" });
    }
    for i in issues {
        println!("⚠ {i}");
    }
    Ok(())
}

/// `goattyd plugin registry add NAME URL --key HEX…`.
pub fn registry_add(svc: &HostService, name: &str, url: &str, keys: &[String]) -> Result<()> {
    let source = Source { name: name.into(), url: url.into(), keys: keys.to_vec(), builtin: false };
    let dirs = svc.launcher.dirs().context("$HOME is not set")?;
    svc.plugins(|p| registry::add_source(dirs, p.builtin, source.clone()))?;
    println!("Added registry {name}. Its plugins are signed by {}.", keys.join(", "));
    match svc.plugins(|p| registry::refresh(dirs, &source, p.fetch)) {
        Ok(s) => println!("Fetched: {} plugins.", s.index.plugins.len()),
        Err(e) => println!("⚠ couldn't fetch it yet: {e:#}"),
    }
    Ok(())
}

/// `goattyd plugin registry remove NAME`.
pub fn registry_remove(svc: &HostService, name: &str) -> Result<()> {
    let dirs = svc.launcher.dirs().context("$HOME is not set")?;
    if registry::remove_source(dirs, name)? {
        println!("Removed registry {name}. Plugins installed from it stay installed.");
    } else {
        bail!("no added registry called {name:?} (built-in registries can't be removed)");
    }
    Ok(())
}
