//! What goatty last did about bringing this computer's shell setup to each
//! host (#159): which version of the setup (its fingerprint) was synced
//! there or declined. goatty asks again once the setup here changes.
//!
//! State, not a setting: `$XDG_STATE_HOME/goatty/shell-sync.json`. Loading
//! never fails; a missing or damaged file means "never asked".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What happened with one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSync {
    /// The setup's fingerprint at the time (`goatty_shell::sync::LocalSetup::fingerprint`).
    pub fingerprint: String,
    /// `true`: synced; `false`: the user said "Not now".
    pub synced: bool,
    /// When (unix seconds).
    pub unix: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    #[serde(default)]
    pub hosts: BTreeMap<String, HostSync>,
}

impl SyncState {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        text.push('\n');
        crate::store::write_text(path, &text).map(drop)
    }

    /// The user already answered for this version of the setup.
    pub fn answered(&self, host: &str, fingerprint: &str) -> bool {
        self.hosts.get(host).is_some_and(|h| h.fingerprint == fingerprint)
    }

    /// This version of the setup was synced there already.
    pub fn synced(&self, host: &str, fingerprint: &str) -> bool {
        self.hosts.get(host).is_some_and(|h| h.fingerprint == fingerprint && h.synced)
    }

    pub fn record(&mut self, host: &str, fingerprint: &str, synced: bool) {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.hosts.insert(host.to_owned(), HostSync { fingerprint: fingerprint.to_owned(), synced, unix });
    }
}

/// The state file for this user. `None` without `$HOME`.
pub fn state_file() -> Option<PathBuf> {
    crate::color_history::state_dir_from(|k| std::env::var(k).ok()).map(|d| d.join("shell-sync.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_answers_per_host_and_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/shell-sync.json");
        let mut s = SyncState::load(&path);
        assert!(!s.answered("box", "a"));
        s.record("box", "a", false);
        s.save(&path).unwrap();
        let s = SyncState::load(&path);
        assert!(s.answered("box", "a") && !s.answered("box", "b") && !s.answered("pi", "a"));
        assert!(!s.hosts["box"].synced && !s.synced("box", "a"));
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(SyncState::load(&path), SyncState::default());
    }
}
