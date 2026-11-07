//! Host settings operations: read, validate + preview, apply (build →
//! activate → save), generations, rollback. goattyd serves these over the
//! protocol; `goatty --local` (no daemon) calls them directly.

use std::path::PathBuf;

use anyhow::{Context, Result};
use goatty_config::{Effective, HostSettings, store};

use crate::generation::{self, FileDiff, Generation};
use crate::launch::Launcher;

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
}

impl HostService {
    pub fn new(launcher: Launcher, host_file: Option<PathBuf>) -> Self {
        Self { launcher, host_file }
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
        HostSnapshot {
            json: store::to_json(&host.file.value, &host.file.unknown),
            issues: host.file.issues.iter().map(ToString::to_string).collect(),
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
        let issues: Vec<String> = loaded.issues.iter().map(ToString::to_string).collect();
        let host = goatty_config::with_host_env(loaded);
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

    pub fn generations(&self) -> Vec<Generation> {
        self.launcher.dirs().map(generation::list).unwrap_or_default()
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
}
