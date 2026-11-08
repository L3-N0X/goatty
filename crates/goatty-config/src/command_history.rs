//! The commands the user ran recently from the command palette, so the
//! palette lists them first.
//!
//! State, not a setting: it lives in `$XDG_STATE_HOME/goatty/command-history.json`
//! (like [`crate::color_history`]). Loading never fails: a missing or damaged
//! file is an empty history.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::color_history::state_dir_from;

/// How many command ids are remembered.
pub const MAX_HISTORY: usize = 32;

/// Command ids, most recent first, no duplicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandHistory {
    ids: Vec<String>,
}

impl CommandHistory {
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// 0 for the most recent command, `None` if it's not in the history.
    pub fn rank(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|i| i == id)
    }

    /// Remember `id` as the most recent (moving it up if it's already there).
    /// Returns whether the history changed.
    pub fn push(&mut self, id: &str) -> bool {
        if self.ids.first().is_some_and(|i| i == id) {
            return false;
        }
        self.ids.retain(|i| i != id);
        self.ids.insert(0, id.to_owned());
        self.ids.truncate(MAX_HISTORY);
        true
    }

    /// Read the history at `path`, skipping entries that aren't strings.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else { return Self::default() };
        let Ok(value) = serde_json::from_str::<Value>(&text) else { return Self::default() };
        let mut h = Self::default();
        let list = value.get("commands").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for id in list.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()) {
            if !h.ids.iter().any(|i| i == id) && h.ids.len() < MAX_HISTORY {
                h.ids.push(id.to_owned());
            }
        }
        h
    }

    /// Write the history to `path` (atomically, creating the directory).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = serde_json::to_string_pretty(&json!({ "version": 1, "commands": self.ids })).map_err(std::io::Error::other)?;
        text.push('\n');
        let tmp = path.with_extension("json.tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

/// The history file for this user. `None` without `$HOME`.
pub fn history_file() -> Option<PathBuf> {
    state_dir_from(|k| std::env::var(k).ok()).map(|d| d.join("command-history.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_keeps_newest_first_without_duplicates() {
        let mut h = CommandHistory::default();
        assert!(h.push("a"));
        assert!(h.push("b"));
        assert!(!h.push("b"));
        assert!(h.push("a"));
        assert_eq!(h.ids(), ["a", "b"]);
        assert_eq!(h.rank("b"), Some(1));
        assert_eq!(h.rank("c"), None);
        for i in 0..MAX_HISTORY + 5 {
            h.push(&i.to_string());
        }
        assert_eq!(h.ids().len(), MAX_HISTORY);
    }

    #[test]
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/command-history.json");
        let mut h = CommandHistory::default();
        h.push("settings.open");
        h.push("shell.reload");
        h.save(&path).unwrap();
        assert_eq!(CommandHistory::load(&path), h);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn damaged_files_are_empty_or_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.json");
        assert!(CommandHistory::load(&path).is_empty(), "missing");
        std::fs::write(&path, "{").unwrap();
        assert!(CommandHistory::load(&path).is_empty(), "garbage");
        std::fs::write(&path, r#"{"commands": ["a", 1, "", "a", "b"]}"#).unwrap();
        assert_eq!(CommandHistory::load(&path).ids(), ["a", "b"]);
    }
}
