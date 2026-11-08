//! The colors the user picked recently, for the color picker's history row.
//!
//! This is state, not a setting: it lives in `$XDG_STATE_HOME/goatty/color-history.json`
//! (not in `settings.json`, so it doesn't end up in exported profiles or get
//! rewritten with backups on every pick). Loading never fails: a missing or
//! damaged file is an empty history.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Color;

/// How many colors are remembered.
pub const MAX_HISTORY: usize = 24;

/// Most recent first, no duplicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorHistory {
    colors: Vec<Color>,
}

impl ColorHistory {
    pub fn colors(&self) -> &[Color] {
        &self.colors
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Remember `c` as the newest entry (moving it up if it's already there).
    /// Returns whether the history changed.
    pub fn push(&mut self, c: Color) -> bool {
        if self.colors.first() == Some(&c) {
            return false;
        }
        self.colors.retain(|&x| x != c);
        self.colors.insert(0, c);
        self.colors.truncate(MAX_HISTORY);
        true
    }

    pub fn clear(&mut self) {
        self.colors.clear();
    }

    /// Read the history at `path`. Unreadable files and entries that aren't
    /// `#rrggbb` are skipped.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else { return Self::default() };
        let Ok(value) = serde_json::from_str::<Value>(&text) else { return Self::default() };
        let mut h = Self::default();
        let list = value.get("colors").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for c in list.iter().filter_map(|v| v.as_str().and_then(Color::parse)) {
            if !h.colors.contains(&c) && h.colors.len() < MAX_HISTORY {
                h.colors.push(c);
            }
        }
        h
    }

    /// Write the history to `path` (atomically, creating the directory).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let colors: Vec<String> = self.colors.iter().map(|c| c.to_string()).collect();
        let mut text = serde_json::to_string_pretty(&json!({ "version": 1, "colors": colors })).map_err(std::io::Error::other)?;
        text.push('\n');
        let tmp = path.with_extension("json.tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

/// `$XDG_STATE_HOME/goatty` (default `~/.local/state/goatty`).
pub fn state_dir_from(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| var("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("goatty"))
}

/// The history file for this user. `None` without `$HOME`.
pub fn history_file() -> Option<PathBuf> {
    state_dir_from(|k| std::env::var(k).ok()).map(|d| d.join("color-history.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_keeps_newest_first_without_duplicates() {
        let mut h = ColorHistory::default();
        assert!(h.push(Color(1)));
        assert!(h.push(Color(2)));
        assert!(h.push(Color(3)));
        assert!(!h.push(Color(3)), "same as the newest: nothing changes");
        assert!(h.push(Color(1)), "an old color moves to the front");
        assert_eq!(h.colors(), &[Color(1), Color(3), Color(2)]);
    }

    #[test]
    fn push_caps_the_length() {
        let mut h = ColorHistory::default();
        for i in 0..(MAX_HISTORY as u32 + 10) {
            h.push(Color(i));
        }
        assert_eq!(h.colors().len(), MAX_HISTORY);
        assert_eq!(h.colors()[0], Color(MAX_HISTORY as u32 + 9));
    }

    #[test]
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/color-history.json");
        let mut h = ColorHistory::default();
        h.push(Color(0x112233));
        h.push(Color(0xaabbcc));
        h.save(&path).unwrap();
        assert_eq!(ColorHistory::load(&path), h);
        assert!(!path.with_extension("json.tmp").exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("#aabbcc") && text.contains("\"version\""), "{text}");
    }

    #[test]
    fn damaged_files_are_empty_or_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.json");
        assert!(ColorHistory::load(&path).is_empty(), "missing");
        std::fs::write(&path, "not json").unwrap();
        assert!(ColorHistory::load(&path).is_empty(), "garbage");
        std::fs::write(&path, r##"{"colors": ["#112233", 5, "nope", "#112233", "#445566"]}"##).unwrap();
        assert_eq!(ColorHistory::load(&path).colors(), &[Color(0x112233), Color(0x445566)]);
    }

    #[test]
    fn state_dir_follows_xdg() {
        let d = state_dir_from(|k| (k == "HOME").then(|| "/home/u".into())).unwrap();
        assert_eq!(d, PathBuf::from("/home/u/.local/state/goatty"));
        let d = state_dir_from(|k| (k == "XDG_STATE_HOME").then(|| "/st".into())).unwrap();
        assert_eq!(d, PathBuf::from("/st/goatty"));
        assert!(state_dir_from(|_| None).is_none());
    }
}
