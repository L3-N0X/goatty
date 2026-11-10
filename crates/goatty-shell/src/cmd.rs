//! The `shell` commands (`goattyd shell status|install|uninstall|dismiss`),
//! printing to stdout. They run on the host whose shells they manage, which
//! for a remote goattyd is the remote host: `goatty-setup` in a remote
//! session installs there.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use goatty_config::{Effective, HostSettings, store};

use crate::builtins::{self, BUILTINS, Builtin};
use crate::dirs::Dirs;
use crate::doctor;
use crate::generation;
use crate::launch::Launcher;
use crate::packages::{self, Location};
use crate::plugins::{Candidate, Plugins};
use crate::setup::{self, FeatureState, ToolLocation, size, tilde};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

fn dirs_of(launcher: &Launcher) -> Result<&Dirs> {
    launcher.dirs().context("$HOME is not set, so goatty has nowhere to keep its shell environment")
}

/// `goattyd shell status`: configuration, what a real shell loads, startup
/// time. The same [`setup::Status`] the GUI's Diagnostics page shows.
pub fn status(svc: &crate::HostService) -> Result<()> {
    let s = setup::status(svc)?;
    println!("goatty shell");
    println!("  program     {}", s.program);
    if let Some(why) = &s.unmanaged {
        println!("  config      {why}");
        return Ok(());
    }
    match s.generation {
        Some(n) => println!("  config      {} (generation {n}; `goattyd shell generations`)", s.config_dir),
        None => println!("  config      {} (not built)", s.config_dir),
    }
    for issue in &s.settings_issues {
        println!("  settings    \u{26a0} {issue}");
    }
    if !s.env_overrides.is_empty() {
        println!("  overridden  by {}", s.env_overrides.join(", "));
    }
    for (i, f) in s.user_files.iter().enumerate() {
        let what = match (f.exists, f.sourced, f.path.ends_with(".zshrc")) {
            (false, ..) => "not there",
            (true, true, _) => "sourced",
            (true, false, true) => "not sourced (GOATTY_ZSH_USER_RC=1 to include it)",
            (true, false, false) => "not sourced",
        };
        println!("  {:<10}  {}: {what}", if i == 0 { "your files" } else { "" }, f.path);
    }
    println!("  additions   {}{}", s.additions, if s.additions_exist { "" } else { " (create it for your own aliases etc.)" });

    match &s.startup {
        Some(st) => {
            println!("  startup     {} ms (login + interactive, measured just now)", st.ms);
            println!();
            for f in &st.features {
                let line = match &f.state {
                    FeatureState::Loaded { managed: Some(v), .. } => format!("\u{2713} managed {} {v}", f.name),
                    FeatureState::Loaded { detail, .. } => format!("\u{2713} {detail}"),
                    FeatureState::Off => "- off".into(),
                    FeatureState::Missing => match s.tool(&f.id) {
                        Some(t) => format!("\u{2717} not installed: goattyd shell install {} ({} {})", t.id, t.name, t.version),
                        None => "\u{2717} its command isn't installed (see `goattyd plugin info`)".into(),
                    },
                };
                println!("  {:<20} {line}", f.id);
            }
            if !st.stderr.is_empty() {
                println!("\nThe shell printed errors while starting:\n{}", st.stderr);
            }
            if s.offer().next().is_some() {
                println!("\nInstall what's missing with `goattyd shell install` (or `goatty-setup` in a goatty shell).");
            }
        }
        None => {
            println!("  startup     failed: {}", s.startup_error.as_deref().unwrap_or("unknown error"));
            println!("\n  as seen from goattyd's PATH:");
            for t in &s.tools {
                let line = match &t.location {
                    ToolLocation::System { path } => format!("\u{2713} {path}"),
                    ToolLocation::Managed { version } => format!("\u{2713} managed {version}"),
                    ToolLocation::Missing => "\u{2717} not found".into(),
                };
                println!("  {:<20} {line}", t.id);
            }
        }
    }
    Ok(())
}

/// Built-in plugins that are on, installed by default, and that the
/// managed shell reports missing. Falls back to goattyd's own `PATH` when
/// the shell can't be probed.
fn missing_defaults(host: &Effective<HostSettings>, launcher: &Launcher, dirs: &Dirs) -> Vec<&'static Builtin> {
    let wanted = |b: &&Builtin| b.default_install && host.value.plugin_enabled(b.id);
    let launch = launcher.launch(host);
    match doctor::probe(&launch, PROBE_TIMEOUT) {
        Ok(p) => p.missing().filter_map(builtins::get).filter(wanted).collect(),
        Err(e) => {
            log::warn!("couldn't probe the shell ({e:#}); checking goattyd's PATH instead");
            let path = std::env::var("PATH").ok();
            BUILTINS
                .iter()
                .filter(wanted)
                .filter(|b| b.manifest().packages.iter().any(|p| packages::locate(dirs, p, path.as_deref()) == Location::Missing))
                .collect()
        }
    }
}

fn resolve_names(names: &[String]) -> Result<Vec<&'static Builtin>> {
    let mut out: Vec<&'static Builtin> = Vec::new();
    for n in names {
        let Some(b) = builtins::find(n) else {
            let ids: Vec<_> = BUILTINS.iter().map(|b| b.id).collect();
            bail!("{n:?} isn't one of goatty's built-in plugins ({}); other plugins: goattyd plugin install {n}", ids.join(", "));
        };
        if !out.iter().any(|o| o.id == b.id) {
            out.push(b);
        }
    }
    Ok(out)
}

/// `goattyd shell install [NAME...]`: download the tools of goatty's
/// built-in plugins. No names installs the default ones the shell is
/// missing (`goatty-setup`). Asks before downloading unless `yes`.
pub fn install(host: &Effective<HostSettings>, launcher: &Launcher, names: &[String], yes: bool) -> Result<()> {
    let dirs = dirs_of(launcher)?;
    let requested = if names.is_empty() { missing_defaults(host, launcher, dirs) } else { resolve_names(names)? };
    let fetch = goatty_plugin::fetch::Curl { cache: dirs.cache.clone() };
    let plugins = Plugins { dirs, fetch: &fetch, builtin: &[] };
    let mut todo = Vec::new();
    for b in requested {
        let c = Candidate::builtin(b);
        let plan = match plugins.plan(&c, &host.value) {
            Ok(plan) => plan,
            Err(e) => {
                println!("{e:#}");
                continue;
            }
        };
        if plan.downloads.iter().any(|d| d.fetches()) {
            todo.push((c, plan));
            continue;
        }
        for d in &plan.downloads {
            match &d.system {
                Some(p) => println!("{} is already installed at {}; goatty uses that.", plan.name, tilde(Path::new(p))),
                None => println!("{} {} is already installed.", plan.name, d.version),
            }
        }
    }
    if todo.is_empty() {
        println!("Nothing to install.");
        return Ok(());
    }

    println!("goatty will download and install into {} (no sudo):", tilde(&dirs.data));
    let rows: Vec<(String, u64, &str)> = todo
        .iter()
        .flat_map(|(_, plan)| {
            plan.downloads.iter().filter(|d| d.fetches()).map(|d| (format!("{} {}", plan.name, d.version), d.size, plan.summary.as_str()))
        })
        .collect();
    let width = rows.iter().map(|(n, ..)| n.len()).max().unwrap_or(0);
    for (name, bytes, summary) in &rows {
        println!("  {name:<width$}  {:>8}  {summary}", size(*bytes));
    }
    println!("  {:<width$}  {:>8}", "", size(rows.iter().map(|(_, b, _)| b).sum()));
    println!("Each download is checked against the sha256 checksum goatty pins for it.");
    if !yes && !confirm("Continue? [Y/n] ")? {
        println!("Cancelled.");
        return Ok(());
    }

    let mut failed = 0;
    for (c, plan) in &todo {
        print!("  {} ... ", plan.name);
        let _ = std::io::stdout().flush();
        match plugins.install(c, &plan.digest, &host.value) {
            Ok(_) => println!("\u{2713}"),
            Err(e) => {
                println!("\u{2717} {e:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} installs failed", todo.len());
    }
    println!("Done. New goatty shells use them; `goatty-reload` restarts the current one.");
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("not asking without a terminal; pass --yes to install");
    }
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes"))
}

/// `goattyd shell uninstall NAME...|--all`: remove goatty's downloads of
/// built-in plugins' tools (the plugins stay; system copies still work).
pub fn uninstall(launcher: &Launcher, names: &[String], all: bool) -> Result<()> {
    let dirs = dirs_of(launcher)?;
    let list = if all { BUILTINS.iter().collect() } else { resolve_names(names)? };
    if list.is_empty() {
        bail!("name the tools to remove, or pass --all");
    }
    let fetch = goatty_plugin::fetch::Curl { cache: dirs.cache.clone() };
    let plugins = Plugins { dirs, fetch: &fetch, builtin: &[] };
    for b in list {
        let name = &b.manifest().name;
        if !plugins.remove_packages(b)?.is_empty() {
            println!("Removed goatty's copy of {name}.");
        } else if !all {
            println!("{name} has no goatty-managed copy.");
        }
    }
    Ok(())
}

/// `goattyd shell dismiss`: stop the "not installed" hint in new shells.
pub fn dismiss(launcher: &Launcher) -> Result<()> {
    let marker = dirs_of(launcher)?.hint_marker();
    std::fs::create_dir_all(marker.parent().context("marker has no parent")?)?;
    std::fs::write(&marker, "")?;
    println!("goatty won't mention missing tools again (`goattyd shell status` still lists them; delete {} to undo).", tilde(&marker));
    Ok(())
}

/// `goattyd shell generations`: the built configurations, newest last.
pub fn generations(launcher: &Launcher) -> Result<()> {
    let dirs = dirs_of(launcher)?;
    let list = generation::list(dirs);
    if list.is_empty() {
        println!("No generations yet: one is built when the first managed shell starts.");
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    for g in list {
        let age = now.saturating_sub(g.created_unix);
        let ago = match age {
            0..60 => format!("{age}s ago"),
            60..3600 => format!("{}m ago", age / 60),
            3600..86400 => format!("{}h ago", age / 3600),
            _ => format!("{}d ago", age / 86400),
        };
        println!("{} {:>4}  {ago:<10} {}", if g.active { "*" } else { " " }, g.number, tilde(&g.path));
    }
    Ok(())
}

/// `goattyd shell diff`: what the next session's configuration would change.
pub fn diff(host: &Effective<HostSettings>, launcher: &Launcher) -> Result<()> {
    let diffs = launcher.preview(&host.value);
    if diffs.is_empty() {
        println!("The active generation matches the current settings.");
    }
    for d in diffs {
        print!("{}", d.unified);
    }
    Ok(())
}

/// `goattyd shell rollback [N]`: activate an older generation and restore
/// the host settings it was built from (the current file is backed up).
pub fn rollback(launcher: &Launcher, host_file: &Path, to: Option<u32>) -> Result<()> {
    let dirs = dirs_of(launcher)?;
    let g = rollback_to(dirs, host_file, to)?;
    println!("Generation {} is active again and {} has its settings. New sessions use it.", g.number, tilde(host_file));
    Ok(())
}

/// Roll back and restore the settings file. Shared with the daemon API.
pub fn rollback_to(dirs: &Dirs, host_file: &Path, to: Option<u32>) -> Result<generation::Generation> {
    let g = generation::rollback(dirs, to)?;
    if let Some(json) = g.settings_json() {
        store::write_text(host_file, &json).with_context(|| format!("restore {}", host_file.display()))?;
    }
    Ok(g)
}

// ---- `goattyd settings` --------------------------------------------------------

fn host_path(svc: &crate::HostService) -> Result<&Path> {
    svc.host_file.as_deref().context("$HOME is not set: no settings directory")
}

fn read_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// `goattyd settings path`.
pub fn settings_path(svc: &crate::HostService) -> Result<()> {
    println!("{}", host_path(svc)?.display());
    Ok(())
}

/// `goattyd settings show [KEY] [--all]`: what's stored, every value, or one key.
pub fn settings_show(svc: &crate::HostService, key: Option<&str>, all: bool) -> Result<()> {
    let host = svc.settings();
    for i in &host.file.issues {
        eprintln!("\u{26a0} {i}");
    }
    if !host.env_overrides.is_empty() {
        eprintln!("(overridden by {})", host.env_overrides.join(", "));
    }
    match key {
        Some(k) => {
            let v = store::get_key(&host.value, k).with_context(|| format!("no setting {k:?}"))?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        None if all => println!("{}", serde_json::to_string_pretty(&host.value)?),
        None => print!("{}", store::to_json(&host.file.value, &host.file.unknown)),
    }
    Ok(())
}

/// `goattyd settings validate [FILE]`: exit status 1 on problems.
pub fn settings_validate(svc: &crate::HostService, file: Option<&Path>) -> Result<()> {
    let path = match file {
        Some(f) => f.to_path_buf(),
        None => host_path(svc)?.to_path_buf(),
    };
    let loaded = store::parse::<HostSettings>(&read_or_empty(&path)?);
    if loaded.issues.is_empty() {
        println!("{}: OK", path.display());
        return Ok(());
    }
    for i in &loaded.issues {
        eprintln!("\u{26a0} {}: {i}", path.display());
    }
    bail!("{} problem(s); goatty uses fallbacks for them", loaded.issues.len())
}

fn apply_edit(svc: &crate::HostService, key: &str, loaded: goatty_config::Loaded<HostSettings>) -> Result<()> {
    let outcome = svc.set(&store::to_json(&loaded.value, &loaded.unknown), true)?;
    let value = store::get_key(&loaded.value, key).map(|v| v.to_string()).unwrap_or_default();
    match outcome.generation {
        Some(g) => println!("{key} = {value} (generation {g}; new sessions use it)"),
        None => println!("{key} = {value}"),
    }
    Ok(())
}

/// `goattyd settings set KEY VALUE`: validated, built and activated like
/// an Apply in the settings window.
pub fn settings_set(svc: &crate::HostService, key: &str, value: &str) -> Result<()> {
    let text = read_or_empty(host_path(svc)?)?;
    let loaded = store::set_key::<HostSettings>(&text, key, value).map_err(anyhow::Error::msg)?;
    apply_edit(svc, key, loaded)
}

/// `goattyd settings unset KEY`: back to the default.
pub fn settings_unset(svc: &crate::HostService, key: &str) -> Result<()> {
    let text = read_or_empty(host_path(svc)?)?;
    let loaded = store::unset_key::<HostSettings>(&text, key).map_err(anyhow::Error::msg)?;
    apply_edit(svc, key, loaded)
}
