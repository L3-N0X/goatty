//! The `shell` commands (`goattyd shell status|install|uninstall|dismiss`),
//! printing to stdout. They run on the host whose shells they manage, which
//! for a remote goattyd is the remote host: `goatty-setup` in a remote
//! session installs there.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use goatty_config::{Effective, HostSettings, store};

use crate::dirs::Dirs;
use crate::doctor::{self, State};
use crate::generation;
use crate::install;
use crate::launch::Launcher;
use crate::tools::{self, Location, TOOLS, Tool};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// `~/x` for paths under `$HOME`.
fn tilde(p: &Path) -> String {
    match std::env::var_os("HOME").and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

fn size(bytes: u64) -> String {
    if bytes < 1_000_000 { format!("{} kB", bytes.div_ceil(1000)) } else { format!("{:.1} MB", bytes as f64 / 1e6) }
}

fn dirs_of(launcher: &Launcher) -> Result<&Dirs> {
    launcher.dirs().context("$HOME is not set, so goatty has nowhere to keep its shell environment")
}

/// `goattyd shell status`: configuration, what a real shell loads, startup time.
pub fn status(host: &Effective<HostSettings>, launcher: &Launcher) -> Result<()> {
    let dirs = dirs_of(launcher)?;
    let shell = &host.value.shell;
    let launch = launcher.launch(host);
    println!("goatty shell");
    println!("  program     {}", launch.program);
    if !launch.managed {
        if !shell.managed {
            println!("  config      your own dotfiles only (managed zsh is off: GOATTY_SHELL_MANAGED=0)");
        } else if crate::launch::is_zsh(&launch.program) {
            println!("  config      your own dotfiles only (couldn't write the managed config; see goattyd's log)");
        } else {
            println!("  config      started as is: goatty's managed configuration is zsh-only for now");
        }
        return Ok(());
    }
    let z = &shell.zsh;
    match generation::current(dirs) {
        Some(n) => println!("  config      {} (generation {n}; `goattyd shell generations`)", tilde(&dirs.zsh_dir())),
        None => println!("  config      {} (not built)", tilde(&dirs.zsh_dir())),
    }
    for issue in &host.file.issues {
        println!("  settings    \u{26a0} {issue}");
    }
    if !host.env_overrides.is_empty() {
        println!("  overridden  by {}", host.env_overrides.join(", "));
    }
    let home_or = |name: &str| format!("{}/{name}", std::env::var("ZDOTDIR").map(|d| tilde(Path::new(&d))).unwrap_or_else(|_| "~".into()));
    println!("  your files  {}: {}", [".zshenv", ".zprofile"].map(home_or).join(", "), if z.user_env { "sourced" } else { "not sourced" });
    println!(
        "              {}: {}",
        home_or(".zshrc"),
        if z.user_rc { "sourced" } else { "not sourced (GOATTY_ZSH_USER_RC=1 to include it)" }
    );
    let additions = dirs.user_zshrc();
    println!("  additions   {}{}", tilde(&additions), if additions.exists() { "" } else { " (create it for your own aliases etc.)" });

    match doctor::probe(&launch, PROBE_TIMEOUT) {
        Ok(p) => {
            println!("  startup     {} ms (login + interactive, measured just now)", p.elapsed.as_millis());
            println!();
            for f in &p.features {
                let line = match &f.state {
                    State::Ok(detail) => format!("\u{2713} {}", describe_ok(&f.name, detail, dirs)),
                    State::Off => "- off".into(),
                    State::Missing => match tools::find(&f.name) {
                        Some(t) => format!("\u{2717} not installed: goattyd shell install {} ({} {})", t.id, t.name, t.version),
                        None => "\u{2717} not available".into(),
                    },
                };
                println!("  {:<20} {line}", f.name);
            }
            let stderr = p.stderr.trim();
            if !stderr.is_empty() {
                println!("\nThe shell printed errors while starting:\n{stderr}");
            }
            if p.missing().any(|m| tools::find(m).is_some_and(|t| t.default)) {
                println!("\nInstall what's missing with `goattyd shell install` (or `goatty-setup` in a goatty shell).");
            }
        }
        Err(e) => {
            println!("  startup     failed: {e:#}");
            println!("\n  as seen from goattyd's PATH:");
            let path = std::env::var("PATH").ok();
            for t in TOOLS {
                let line = match t.locate(dirs, path.as_deref()) {
                    Location::System(p) => format!("\u{2713} {}", tilde(&p)),
                    Location::Managed { version, .. } => format!("\u{2713} managed {}", version.unwrap_or_default()),
                    Location::Missing => "\u{2717} not found".into(),
                };
                println!("  {:<20} {line}", t.id);
            }
        }
    }
    Ok(())
}

fn describe_ok(feature: &str, detail: &str, dirs: &Dirs) -> String {
    let path = Path::new(detail);
    if feature == "integration" {
        return "OSC 133 prompt marks, OSC 7 working directory".into();
    }
    let managed = path.starts_with(dirs.bin_dir()) || path.starts_with(dirs.plugins_dir());
    match tools::find(feature) {
        Some(t) if managed => format!("managed {} {}", t.name, t.managed_version(dirs).unwrap_or_default()),
        _ => tilde(path),
    }
}

/// Default tools the managed shell reports missing. Falls back to
/// goattyd's own `PATH` when the shell can't be probed.
fn missing_defaults(host: &Effective<HostSettings>, launcher: &Launcher, dirs: &Dirs) -> Vec<&'static Tool> {
    let launch = launcher.launch(host);
    match doctor::probe(&launch, PROBE_TIMEOUT) {
        Ok(p) => p.missing().filter_map(tools::find).filter(|t| t.default).collect(),
        Err(e) => {
            log::warn!("couldn't probe the shell ({e:#}); checking goattyd's PATH instead");
            let path = std::env::var("PATH").ok();
            TOOLS.iter().filter(|t| t.default && t.locate(dirs, path.as_deref()) == Location::Missing).collect()
        }
    }
}

fn resolve_names(names: &[String]) -> Result<Vec<&'static Tool>> {
    let mut out: Vec<&'static Tool> = Vec::new();
    for n in names {
        let Some(t) = tools::find(n) else {
            let ids: Vec<_> = TOOLS.iter().map(|t| t.id).collect();
            bail!("unknown tool {n:?}; known: {}", ids.join(", "));
        };
        if !out.iter().any(|o| o.id == t.id) {
            out.push(t);
        }
    }
    Ok(out)
}

/// `goattyd shell install [TOOL...]`: no names installs the default tools
/// the shell is missing. Asks before downloading unless `yes`.
pub fn install(host: &Effective<HostSettings>, launcher: &Launcher, names: &[String], yes: bool) -> Result<()> {
    let dirs = dirs_of(launcher)?;
    let requested = if names.is_empty() { missing_defaults(host, launcher, dirs) } else { resolve_names(names)? };
    let target = tools::current_target();
    let mut plan = Vec::new();
    for t in requested {
        if t.managed_version(dirs).as_deref() == Some(t.version) {
            println!("{} {} is already installed.", t.name, t.version);
            continue;
        }
        let Some(asset) = t.asset_for(&target) else {
            println!("{} has no goatty-managed build for {target}; install it with your package manager ({}).", t.name, t.homepage);
            continue;
        };
        plan.push((t, asset));
    }
    if plan.is_empty() {
        println!("Nothing to install.");
        return Ok(());
    }

    println!("goatty will download and install into {} (no sudo):", tilde(&dirs.data));
    let width = plan.iter().map(|(t, _)| t.name.len() + t.version.len() + 1).max().unwrap_or(0);
    for (t, a) in &plan {
        println!("  {:<width$}  {:>8}  {}", format!("{} {}", t.name, t.version), size(a.size), t.summary);
    }
    println!("  {:<width$}  {:>8}", "", size(plan.iter().map(|(_, a)| a.size).sum()));
    println!("Each download is checked against a pinned sha256 checksum.");
    if !yes && !confirm("Continue? [Y/n] ")? {
        println!("Cancelled.");
        return Ok(());
    }

    let mut failed = 0;
    for (t, a) in &plan {
        print!("  {} {} ... ", t.name, t.version);
        let _ = std::io::stdout().flush();
        match install::download(a.url, &dirs.cache).and_then(|bytes| install::install_archive(t, a, &bytes, dirs)) {
            Ok(_) => println!("\u{2713}"),
            Err(e) => {
                println!("\u{2717} {e:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} installs failed", plan.len());
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

/// `goattyd shell uninstall TOOL...|--all`: remove managed copies.
pub fn uninstall(launcher: &Launcher, names: &[String], all: bool) -> Result<()> {
    let dirs = dirs_of(launcher)?;
    let list = if all { TOOLS.iter().collect() } else { resolve_names(names)? };
    if list.is_empty() {
        bail!("name the tools to remove, or pass --all");
    }
    for t in list {
        if install::uninstall(t, dirs)? {
            println!("Removed goatty's copy of {}.", t.name);
        } else if !all {
            println!("{} has no goatty-managed copy.", t.name);
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
