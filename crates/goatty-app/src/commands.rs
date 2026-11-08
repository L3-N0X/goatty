//! `goatty settings …` and `goatty theme …`: everything the settings window
//! does with profiles and themes, without a window (scripts, SSH sessions,
//! dotfile setups). Host settings go through the local goattyd when it
//! runs, else straight to this machine's `host.json`; `goattyd settings`
//! on a server edits that host's file.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use goatty_client::Control;
use goatty_config::theme_import;
use goatty_config::themes::ThemeFile;
use goatty_config::{ClientSettings, Document, HostSettings, Loaded, ThemeLibrary, store};
use goatty_shell::HostService;

use crate::cli::{SettingsCmd, ThemeCmd, ThemeFor};

const CLIENT: &str = concat!("goatty-cli ", env!("CARGO_PKG_VERSION"));

fn client_path() -> Result<PathBuf> {
    goatty_config::client_file().context("$HOME is not set: goatty has no settings directory")
}

fn read_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn print_issues(label: &str, issues: &[String]) {
    for i in issues {
        eprintln!("⚠ {label}: {i}");
    }
}

/// The host side: the running goattyd, or this machine's files.
enum Host {
    Daemon(Control),
    Local(HostService),
}

impl Host {
    fn open() -> Self {
        match Control::connect_local(CLIENT) {
            Ok(c) => Host::Daemon(c),
            Err(_) => {
                let daemon = std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists());
                Host::Local(HostService::from_env(daemon))
            }
        }
    }

    fn label(&self) -> String {
        match self {
            Host::Daemon(c) => format!("goattyd on {}", c.host().hostname),
            Host::Local(_) => "this machine (no goattyd running)".into(),
        }
    }

    fn json(&mut self) -> Result<String> {
        match self {
            Host::Daemon(c) => c.host_settings_json(),
            Host::Local(s) => Ok(s.snapshot().json),
        }
    }

    /// Validate, build the generation, save. Returns the issues.
    fn apply(&mut self, json: String) -> Result<Vec<String>> {
        match self {
            Host::Daemon(c) => c.apply_host_settings(json),
            Host::Local(s) => Ok(s.set(&json, true)?.issues),
        }
    }
}

fn confirm(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("not asking without a terminal; pass --yes");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Print a document: one key, every value (`all`), or what's stored.
fn show<T: Document + serde::Serialize>(loaded: &Loaded<T>, effective: &T, key: Option<&str>, all: bool) -> Result<()> {
    match key {
        Some(k) => {
            let v = store::get_key(effective, k).with_context(|| format!("no setting {k:?}"))?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        None if all => println!("{}", serde_json::to_string_pretty(effective)?),
        None => print!("{}", store::to_json(&loaded.value, &loaded.unknown)),
    }
    Ok(())
}

pub fn settings(cmd: SettingsCmd) -> Result<()> {
    match cmd {
        SettingsCmd::Path => {
            let dir = goatty_config::config_dir().context("$HOME is not set")?;
            println!("client  {}", dir.join("settings.json").display());
            println!("host    {}  (this machine; goattyd on other hosts keeps its own)", dir.join("host.json").display());
            println!("themes  {}", dir.join("themes").display());
            Ok(())
        }
        SettingsCmd::Show { key, all } => {
            let c = goatty_config::load_client();
            print_issues("settings.json", &c.file.issues.iter().map(ToString::to_string).collect::<Vec<_>>());
            if !c.env_overrides.is_empty() {
                eprintln!("(overridden by {})", c.env_overrides.join(", "));
            }
            show(&c.file, &c.value, key.as_deref(), all)
        }
        SettingsCmd::Validate { file } => {
            let path = match file {
                Some(f) => f,
                None => client_path()?,
            };
            let text = read_or_empty(&path)?;
            let loaded = store::parse::<ClientSettings>(&text);
            let mut problems: Vec<String> = loaded.issues.iter().map(ToString::to_string).collect();
            let lib = ThemeLibrary::load_default();
            problems.extend(lib.issues.iter().map(ToString::to_string));
            for dark in [false, true] {
                if let (_, Some(i)) = lib.resolve(&loaded.value.theme, dark)
                    && !problems.contains(&i.to_string())
                {
                    problems.push(i.to_string());
                }
            }
            // Keystrokes goatty can't read are already in `issues`; this
            // catches what only GPUI rejects.
            for key in loaded.value.keybindings.keys().filter(|k| goatty_config::keys::canonical_keystroke(k).is_ok()) {
                if gpui::Keystroke::parse(Box::leak(key.clone().into_boxed_str())).is_err() {
                    problems.push(format!("keybindings.{key}: not a valid keystroke"));
                }
            }
            if problems.is_empty() {
                println!("{}: OK", path.display());
                Ok(())
            } else {
                print_issues(&path.display().to_string(), &problems);
                bail!("{} problem(s); goatty uses fallbacks for them", problems.len())
            }
        }
        SettingsCmd::Set { key, value } => {
            let path = client_path()?;
            let loaded = store::set_key::<ClientSettings>(&read_or_empty(&path)?, &key, &value).map_err(anyhow::Error::msg)?;
            store::save(&path, &loaded.value, &loaded.unknown)?;
            println!("{key} = {}", store::get_key(&loaded.value, &key).map(|v| v.to_string()).unwrap_or_default());
            Ok(())
        }
        SettingsCmd::Unset { key } => {
            let path = client_path()?;
            let loaded = store::unset_key::<ClientSettings>(&read_or_empty(&path)?, &key).map_err(anyhow::Error::msg)?;
            store::save(&path, &loaded.value, &loaded.unknown)?;
            println!("{key} = {} (default)", store::get_key(&loaded.value, &key).map(|v| v.to_string()).unwrap_or_default());
            Ok(())
        }
        SettingsCmd::Export { file } => {
            let client = goatty_config::load_client().file.value;
            let mut host = Host::open();
            let host_settings = store::parse::<HostSettings>(&host.json()?).value;
            let text = goatty_config::profile::export(&client, &host_settings);
            if file.as_os_str() == "-" {
                print!("{text}");
            } else {
                std::fs::write(&file, text).with_context(|| format!("write {}", file.display()))?;
                eprintln!("Exported this computer's settings and {}'s shell settings to {}.", host.label(), file.display());
            }
            Ok(())
        }
        SettingsCmd::Import { file, yes } => {
            let text = if file.as_os_str() == "-" {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?
            };
            let (client, host) = goatty_config::profile::import(&text).map_err(anyhow::Error::msg)?;
            let mut target = Host::open();
            println!("This profile replaces:");
            if client.is_some() {
                println!("  appearance, shortcuts and workspace settings on this computer");
            }
            if host.is_some() {
                println!("  shell, prompt and history settings of {}", target.label());
            }
            println!("(the current files are kept in backups/, and the shell configuration can be rolled back)");
            if !confirm("Import?", yes)? {
                println!("Cancelled.");
                return Ok(());
            }
            if let Some(c) = client {
                print_issues("client", &c.issues.iter().map(ToString::to_string).collect::<Vec<_>>());
                store::save(&client_path()?, &c.value, &c.unknown)?;
                println!("Client settings saved.");
            }
            if let Some(h) = host {
                print_issues("host", &h.issues.iter().map(ToString::to_string).collect::<Vec<_>>());
                let issues = target.apply(store::to_json(&h.value, &h.unknown))?;
                print_issues("host", &issues);
                println!("Host settings applied ({}).", target.label());
            }
            Ok(())
        }
    }
}

/// Import every scheme in `path` as a user theme. Returns (id, name, format).
pub fn import_theme_file(path: &Path) -> Result<Vec<(String, String, &'static str)>> {
    import_theme_file_named(path, None)
}

fn import_theme_file_named(path: &Path, name: Option<&str>) -> Result<Vec<(String, String, &'static str)>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "imported".into());
    let schemes = theme_import::import(&text, &stem).map_err(anyhow::Error::msg)?;
    if name.is_some() && schemes.len() > 1 {
        bail!("{} holds {} schemes; --name works with one", path.display(), schemes.len());
    }
    let mut out = Vec::new();
    for s in schemes {
        // Reload so names saved a moment ago count as taken.
        let lib = ThemeLibrary::load_default();
        let name = name.map(str::to_owned).unwrap_or(s.name);
        let id = lib.unique_id(&name);
        lib.save(&id, &name, &s.colors)?;
        out.push((id, name, s.format.label()));
    }
    Ok(out)
}

pub fn theme(cmd: ThemeCmd) -> Result<()> {
    let lib = ThemeLibrary::load_default();
    for i in &lib.issues {
        eprintln!("⚠ {i}");
    }
    match cmd {
        ThemeCmd::List => {
            let c = goatty_config::load_client().value;
            let t = &c.theme;
            for e in lib.all() {
                let mut marks = Vec::new();
                if !t.follow_system && e.id == t.name {
                    marks.push("active");
                }
                if t.follow_system && e.id == t.light {
                    marks.push("light");
                }
                if t.follow_system && e.id == t.dark {
                    marks.push("dark");
                }
                let kind = if e.user { "mine" } else { "built-in" };
                println!("{:<22} {:<24} {kind:<8} {}", e.id, e.name, marks.join(", "));
            }
            Ok(())
        }
        ThemeCmd::Import { files, name } => {
            if name.is_some() && files.len() > 1 {
                bail!("--name works with one file");
            }
            for f in files {
                for (id, n, format) in import_theme_file_named(&f, name.as_deref())? {
                    println!("Imported {n:?} ({format}) as {id}. Use it: goatty theme set {id}");
                }
            }
            Ok(())
        }
        ThemeCmd::Export { id, file } => {
            let e = lib.get(&id).with_context(|| format!("no theme {id:?} (see `goatty theme list`)"))?;
            let mut text = serde_json::to_string_pretty(&ThemeFile { name: e.name, colors: e.colors })?;
            text.push('\n');
            match file {
                Some(f) => std::fs::write(&f, text).with_context(|| format!("write {}", f.display()))?,
                None => print!("{text}"),
            }
            Ok(())
        }
        ThemeCmd::Set { id, target } => {
            if lib.get(&id).is_none() {
                bail!("no theme {id:?} (see `goatty theme list`)");
            }
            let path = client_path()?;
            let mut loaded = store::parse::<ClientSettings>(&read_or_empty(&path)?);
            let t = &mut loaded.value.theme;
            match target {
                None => {
                    t.name = id.clone();
                    t.follow_system = false;
                }
                Some(ThemeFor::Light) => {
                    t.light = id.clone();
                    t.follow_system = true;
                }
                Some(ThemeFor::Dark) => {
                    t.dark = id.clone();
                    t.follow_system = true;
                }
            }
            store::save(&path, &loaded.value, &loaded.unknown)?;
            println!("Theme set to {id}{}.", if target.is_some() { " (following the system)" } else { "" });
            Ok(())
        }
        ThemeCmd::Delete { id } => {
            let e = lib.get(&id).filter(|e| e.user).with_context(|| format!("{id:?} isn't one of your themes"))?;
            lib.delete(&id)?;
            println!("Deleted {:?}.", e.name);
            Ok(())
        }
    }
}
