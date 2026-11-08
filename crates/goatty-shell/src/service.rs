//! Host settings operations: read, validate + preview, apply (build →
//! activate → save), generations, rollback. goattyd serves these over the
//! protocol; `goatty --local` (no daemon) calls them directly.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use goatty_config::{Effective, HostSettings, store};
use goatty_plugin::api::{self, Changed};
use goatty_plugin::fetch::Fetch;

use crate::generation::{self, FileDiff, Generation};
use crate::launch::Launcher;
use crate::plugins::registry::Source;
use crate::plugins::{Candidate, Plugins};

/// The host settings as stored, for an editor.
#[derive(Debug, Clone)]
pub struct HostSnapshot {
    /// The settings document (non-default values only).
    pub json: String,
    pub issues: Vec<String>,
    pub env_overrides: Vec<String>,
    pub generation: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    pub issues: Vec<String>,
    /// Generated-file changes relative to the active generation, computed
    /// before applying.
    pub diff: Vec<FileDiff>,
    pub applied: bool,
    pub generation: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct HostService {
    pub launcher: Launcher,
    /// `None` without `$HOME`.
    pub host_file: Option<PathBuf>,
    /// Running managed shells (pids), for [`HostService::reload_shells`].
    /// Shared between clones.
    shells: Arc<Mutex<BTreeSet<u32>>>,
    /// Downloads for plugins (curl; fixed responses in tests).
    fetch: Arc<dyn Fetch>,
    /// Built-in plugin registries (the official one).
    registries: Vec<Source>,
}

impl HostService {
    pub fn new(launcher: Launcher, host_file: Option<PathBuf>) -> Self {
        let cache = launcher.dirs().map(|d| d.cache.clone()).unwrap_or_else(std::env::temp_dir);
        Self {
            launcher,
            host_file,
            shells: Arc::default(),
            fetch: Arc::new(goatty_plugin::fetch::Curl { cache }),
            registries: vec![Source::official()],
        }
    }

    /// Use other downloads and built-in registries (tests, sandboxes).
    pub fn with_plugin_sources(mut self, fetch: Arc<dyn Fetch>, registries: Vec<Source>) -> Self {
        self.fetch = fetch;
        self.registries = registries;
        self
    }

    /// Run `f` with this host's plugin operations.
    pub fn plugins<R>(&self, f: impl FnOnce(&Plugins<'_>) -> Result<R>) -> Result<R> {
        let dirs = self.launcher.dirs().context("$HOME is not set, so goatty has nowhere to keep plugins")?;
        f(&Plugins { dirs, fetch: self.fetch.as_ref(), builtin: &self.registries })
    }

    /// This process's host: XDG dirs, `goatty_config::host_file()`.
    pub fn from_env(daemon_exe: Option<PathBuf>) -> Self {
        Self::new(Launcher::from_env(daemon_exe), goatty_config::host_file())
    }

    /// The settings as of now: the file plus the environment.
    pub fn settings(&self) -> Effective<HostSettings> {
        let file = self.host_file.as_deref().map(store::load).unwrap_or_default();
        goatty_config::with_host_env(file)
    }

    pub fn active_generation(&self) -> Option<u32> {
        self.launcher.dirs().and_then(generation::current)
    }

    pub fn snapshot(&self) -> HostSnapshot {
        let host = self.settings();
        let mut issues: Vec<String> = host.file.issues.iter().map(ToString::to_string).collect();
        if let Some(dirs) = self.launcher.dirs() {
            issues.extend(crate::plugins::active(dirs, &host.value).1);
        }
        HostSnapshot {
            json: store::to_json(&host.file.value, &host.file.unknown),
            issues,
            env_overrides: host.env_overrides.iter().map(|s| s.to_string()).collect(),
            generation: self.active_generation(),
        }
    }

    /// Validate `json` and compute the diff; with `apply`, build and
    /// activate the generation, then save the file. A configuration that
    /// fails validation is rejected before anything is written.
    pub fn set(&self, json: &str, apply: bool) -> Result<ApplyOutcome> {
        let path = self.host_file.as_deref().context("no $HOME on this host: nowhere to store settings")?;
        let loaded = store::parse::<HostSettings>(json);
        let mut issues: Vec<String> = loaded.issues.iter().map(ToString::to_string).collect();
        let host = goatty_config::with_host_env(loaded);
        if let Some(dirs) = self.launcher.dirs() {
            issues.extend(crate::plugins::active(dirs, &host.value).1);
        }
        let diff = self.launcher.preview(&host.value);
        if !apply {
            return Ok(ApplyOutcome { issues, diff, applied: false, generation: self.active_generation() });
        }
        let activated = self.launcher.apply(&host).context("not applied")?;
        store::save(path, &host.file.value, &host.file.unknown)
            .with_context(|| format!("built generation {} but couldn't save {}", activated.generation, path.display()))?;
        log::info!("host settings saved; generation {} active", activated.generation);
        Ok(ApplyOutcome { issues, diff, applied: true, generation: Some(activated.generation) })
    }

    /// Change the stored host settings with `f` and apply them like the
    /// settings window does (validated generation, then the save).
    pub fn edit(&self, f: impl FnOnce(&mut HostSettings)) -> Result<ApplyOutcome> {
        let host = self.settings();
        let mut value = host.file.value.clone();
        f(&mut value);
        self.set(&store::to_json(&value, &host.file.unknown), true)
    }

    /// Answer a plugin request (the protocol's `Plugin` message, the GUI).
    /// Installs download: call off the UI thread.
    pub fn plugin_request(&self, req: api::Request) -> Result<api::Reply> {
        let settings = self.settings().value;
        match req {
            api::Request::Catalog { refresh } => self.plugins(|p| Ok(api::Reply::Catalog(p.catalog(refresh, &settings)))),
            api::Request::Plan { id, version } => {
                self.plugins(|p| Ok(api::Reply::Plan(Box::new(p.plan(&p.find(&id, version.as_deref())?, &settings)?))))
            }
            api::Request::Install { id, version, approved } => {
                let c = self.plugins(|p| p.find(&id, version.as_deref()))?;
                Ok(api::Reply::Changed(self.install_plugin(&c, &approved)?))
            }
            api::Request::Uninstall { id } => Ok(api::Reply::Changed(self.uninstall_plugin(&id)?)),
        }
    }

    /// Install `c` as approved (`approved`: the plan's digest), enable it,
    /// and build the shell configuration.
    pub fn install_plugin(&self, c: &Candidate, approved: &str) -> Result<Changed> {
        let settings = self.settings().value;
        let plan = self.plugins(|p| p.install(c, approved, &settings))?;
        let id = plan.id.clone();
        let outcome = self.edit(|h| h.plugins.entry(id).or_default().enabled = true)?;
        let verb = match &plan.replaces {
            Some(old) if *old != plan.version => format!("Updated {} from {old} to {}", plan.name, plan.version),
            Some(_) => format!("Reinstalled {} {}", plan.name, plan.version),
            None => format!("Installed {} {}", plan.name, plan.version),
        };
        Ok(changed(verb, &outcome))
    }

    /// Remove a plugin and its settings, and build the shell configuration.
    pub fn uninstall_plugin(&self, id: &str) -> Result<Changed> {
        let entry = self.plugins(|p| p.uninstall(id))?;
        let id = id.to_owned();
        let outcome = self.edit(|h| {
            h.plugins.remove(&id);
        })?;
        Ok(changed(format!("Uninstalled {id} {}", entry.version), &outcome))
    }

    pub fn generations(&self) -> Vec<Generation> {
        self.launcher.dirs().map(generation::list).unwrap_or_default()
    }

    /// Remember a managed shell this process started, so
    /// [`reload_shells`](Self::reload_shells) can reach it.
    pub fn track_shell(&self, pid: u32) {
        self.shells.lock().unwrap_or_else(|e| e.into_inner()).insert(pid);
    }

    /// The shell exited.
    pub fn untrack_shell(&self, pid: u32) {
        self.shells.lock().unwrap_or_else(|e| e.into_inner()).remove(&pid);
    }

    /// Ask every tracked managed shell to restart with the active
    /// generation. They get `SIGURG`: the managed zsh reloads at once when
    /// it's idle at an empty prompt, otherwise at its next prompt (and not
    /// at all while it has jobs; it says so instead). `SIGURG` is ignored
    /// by default, so a process that isn't our zsh (the user ran
    /// `exec bash`, or the pid was reused) is unaffected. Returns how many
    /// shells were signaled.
    pub fn reload_shells(&self) -> usize {
        let mut shells = self.shells.lock().unwrap_or_else(|e| e.into_inner());
        shells.retain(|&pid| {
            let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
            // SAFETY: kill(2) has no memory-safety preconditions; a stale
            // pid at worst gets an ignored-by-default signal.
            unsafe { libc::kill(pid, libc::SIGURG) == 0 }
        });
        log::info!("asked {} managed shell(s) to reload", shells.len());
        shells.len()
    }

    /// Activate generation `to` (default: the previous one) and restore its
    /// settings file.
    pub fn rollback(&self, to: Option<u32>) -> Result<HostSnapshot> {
        let dirs = self.launcher.dirs().context("no $HOME on this host")?;
        let path = self.host_file.as_deref().context("no $HOME on this host")?;
        let g = crate::cmd::rollback_to(dirs, path, to)?;
        log::info!("rolled back to generation {}", g.number);
        Ok(self.snapshot())
    }
}

fn changed(what: String, o: &ApplyOutcome) -> Changed {
    let mut lines = vec![match o.generation {
        Some(g) if !o.diff.is_empty() => {
            format!("{what}. New goatty shells have it (generation {g}); reload running shells to use it there too.")
        }
        _ => format!("{what}."),
    }];
    lines.extend(o.issues.iter().map(|i| format!("⚠ {i}")));
    Changed { lines, generation: o.generation, reload: !o.diff.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::Dirs;

    #[test]
    fn set_preview_apply_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("config/host.json");
        let svc = HostService::new(Launcher::new(Some(Dirs::under(tmp.path())), None), Some(file.clone()));
        assert_eq!(svc.snapshot().json, "{\n  \"version\": 1\n}\n");

        let preview = svc.set(r#"{"prompt": {"two_line": false}}"#, false).unwrap();
        assert!(!preview.applied && !preview.diff.is_empty() && !file.exists());

        let a = svc.set(r#"{"prompt": {"two_line": false}}"#, true).unwrap();
        assert_eq!(a.generation, Some(1));
        assert!(svc.snapshot().json.contains("two_line"));
        let b = svc.set(r#"{"prompt": {"two_line": false}, "history": {"size": 2000}}"#, true).unwrap();
        assert_eq!(b.generation, Some(2));
        assert_eq!(b.diff.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["settings.zsh"]);

        let back = svc.rollback(None).unwrap();
        assert_eq!(back.generation, Some(1));
        assert!(!back.json.contains("2000"));
        assert_eq!(svc.generations().len(), 2);
    }

    #[test]
    fn plugin_requests_install_enable_and_uninstall() {
        let w = crate::plugins::tests::world();
        let file = w.tmp.path().join("config/host.json");
        let svc = HostService::new(Launcher::new(Some(w.dirs.clone()), None), Some(file.clone()))
            .with_plugin_sources(Arc::new(w.fetch.clone()), w.sources.clone());

        let api::Reply::Plan(plan) = svc.plugin_request(api::Request::Plan { id: "demo".into(), version: None }).unwrap() else {
            panic!("expected a plan")
        };
        let req = api::Request::Install { id: "demo".into(), version: None, approved: plan.digest.clone() };
        let api::Reply::Changed(c) = svc.plugin_request(req).unwrap() else { panic!("expected a change") };
        assert!(c.lines[0].starts_with("Installed Demo 1.0.0"), "{:?}", c.lines);
        assert!(c.reload);
        assert!(std::fs::read_to_string(&file).unwrap().contains("\"demo\""), "enabled in host.json");
        let plugins_zsh = std::fs::read_to_string(w.dirs.zsh_dir().join("plugins.zsh")).unwrap();
        assert!(plugins_zsh.contains("alias -- d='demo'"), "{plugins_zsh}");

        // A setting changed through the host settings reaches the generation.
        svc.edit(|h| {
            h.plugins.get_mut("demo").unwrap().settings.insert("loud".into(), "yes".into());
        })
        .map(|o| assert!(o.issues.iter().any(|i| i.contains("plugins.demo.settings.loud")), "{:?}", o.issues))
        .unwrap();

        let api::Reply::Catalog(cat) = svc.plugin_request(api::Request::Catalog { refresh: false }).unwrap() else { panic!() };
        assert_eq!(cat.installed.len(), 1);
        assert!(cat.installed[0].enabled);

        let api::Reply::Changed(c) = svc.plugin_request(api::Request::Uninstall { id: "demo".into() }).unwrap() else { panic!() };
        assert!(c.lines[0].starts_with("Uninstalled demo"));
        assert!(!std::fs::read_to_string(&file).unwrap().contains("\"demo\""));
        assert!(!std::fs::read_to_string(w.dirs.zsh_dir().join("plugins.zsh")).unwrap().contains("alias --"));
    }
}
