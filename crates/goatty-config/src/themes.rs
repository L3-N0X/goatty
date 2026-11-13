//! The theme library: built-in themes plus user themes stored as
//! `$XDG_CONFIG_HOME/goatty/themes/<id>.json`:
//!
//! ```json
//! { "name": "My Night", "colors": { "foreground": "#c0caf5", "background": "#1a1b26",
//!   "cursor": "#c0caf5", "selection_foreground": "#c0caf5", "selection_background": "#283457",
//!   "accent": "#7aa2f7", "ansi": ["#15161e", "...16 colors..."] } }
//! ```
//!
//! A user theme with a built-in's id replaces that built-in. The settings
//! window's theme editor and `goatty theme import` write these files.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{self, Issue};
use crate::theme::{DEFAULT_THEME, THEMES, ThemeColors, ThemeSettings, find_theme};

/// The on-disk form of a user theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    pub colors: ThemeColors,
}

/// A theme available to pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeEntry {
    pub id: String,
    pub name: String,
    pub colors: ThemeColors,
    /// From `themes/` (editable, deletable).
    pub user: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ThemeLibrary {
    /// Where user themes live (`None`: no `$HOME`).
    pub dir: Option<PathBuf>,
    pub user: Vec<ThemeEntry>,
    /// Theme files that couldn't be read.
    pub issues: Vec<Issue>,
}

/// `$XDG_CONFIG_HOME/goatty/themes`.
pub fn themes_dir() -> Option<PathBuf> {
    crate::config_dir().map(|d| d.join("themes"))
}

/// A file-name-safe id from a theme name: lowercase, `-` between words.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "theme".into() } else { out }
}

impl ThemeLibrary {
    /// Built-ins plus the themes in [`themes_dir`].
    pub fn load_default() -> Self {
        match themes_dir() {
            Some(d) => Self::load(&d),
            None => Self::default(),
        }
    }

    /// Built-ins plus the `*.json` themes in `dir` (a missing dir is fine).
    pub fn load(dir: &Path) -> Self {
        let mut lib = Self { dir: Some(dir.to_path_buf()), ..Self::default() };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        for path in files {
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str::<ThemeFile>(&t).map_err(|e| e.to_string()));
            match parsed {
                Ok(f) => lib.user.push(ThemeEntry { id, name: f.name, colors: f.colors, user: true }),
                Err(e) => lib.issues.push(Issue::new(format!("themes/{id}.json"), format!("not a goatty theme ({e}); skipped"))),
            }
        }
        lib
    }

    /// Every theme: built-ins (replaced by a user theme of the same id),
    /// then the other user themes.
    pub fn all(&self) -> Vec<ThemeEntry> {
        let mut v: Vec<ThemeEntry> = THEMES
            .iter()
            .map(|t| {
                self.user.iter().find(|u| u.id == t.id).cloned().unwrap_or(ThemeEntry {
                    id: t.id.into(),
                    name: t.name.into(),
                    colors: t.colors,
                    user: false,
                })
            })
            .collect();
        v.extend(self.user.iter().filter(|u| find_theme(&u.id).is_none()).cloned());
        v
    }

    pub fn get(&self, id: &str) -> Option<ThemeEntry> {
        self.user
            .iter()
            .find(|u| u.id == id)
            .cloned()
            .or_else(|| find_theme(id).map(|t| ThemeEntry { id: t.id.into(), name: t.name.into(), colors: t.colors, user: false }))
    }

    /// The colors to use: the active theme (by appearance when following
    /// the system) with the overrides applied. An unknown id falls back to
    /// the default theme and comes back as an issue.
    pub fn resolve(&self, s: &ThemeSettings, system_dark: bool) -> (ThemeColors, Option<Issue>) {
        let id = s.active_id(system_dark);
        let field = match (s.follow_system, system_dark) {
            (false, _) => "theme.name",
            (true, true) => "theme.dark",
            (true, false) => "theme.light",
        };
        let (base, issue) = match self.get(id) {
            Some(t) => (t.colors, None),
            None => (
                find_theme(DEFAULT_THEME).expect("default theme").colors,
                Some(Issue::new(field, format!("unknown theme {id:?}; using the default"))),
            ),
        };
        (s.overrides.apply(base), issue)
    }

    /// Every theme `s` names that doesn't exist: `theme.name`, or both
    /// `theme.light` and `theme.dark` when following the system (the one
    /// not in use now too, before the desktop switches to it).
    pub fn check(&self, s: &ThemeSettings) -> Vec<Issue> {
        let fields: &[(&str, &str)] =
            if s.follow_system { &[("theme.light", &s.light), ("theme.dark", &s.dark)] } else { &[("theme.name", &s.name)] };
        fields
            .iter()
            .filter(|(_, id)| self.get(id).is_none())
            .map(|(field, id)| Issue::new(*field, format!("unknown theme {id:?}; using the default")))
            .collect()
    }

    /// An id for a new theme called `name` that doesn't clash with existing
    /// themes (built-in or user).
    pub fn unique_id(&self, name: &str) -> String {
        let base = slugify(name);
        let taken = |id: &str| self.get(id).is_some();
        if !taken(&base) {
            return base;
        }
        (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).expect("unbounded")
    }

    /// Write `themes/<id>.json` (replacing it if present). Returns its path.
    pub fn save(&self, id: &str, name: &str, colors: &ThemeColors) -> std::io::Result<PathBuf> {
        let dir = self.dir.as_ref().ok_or_else(|| std::io::Error::other("no $HOME: nowhere to store themes"))?;
        let path = dir.join(format!("{}.json", slugify(id)));
        let mut text = serde_json::to_string_pretty(&ThemeFile { name: name.into(), colors: *colors }).expect("json");
        text.push('\n');
        store::write_text(&path, &text)?;
        Ok(path)
    }

    /// Remove a user theme. Returns whether it existed.
    pub fn delete(&self, id: &str) -> std::io::Result<bool> {
        let Some(dir) = &self.dir else { return Ok(false) };
        match std::fs::remove_file(dir.join(format!("{id}.json"))) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Cheap change detection for polling the themes directory.
    pub fn stamp(dir: &Path) -> Vec<(PathBuf, store::FileStamp)> {
        let mut v: Vec<(PathBuf, store::FileStamp)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .map(|p| (p.clone(), store::FileStamp::of(&p)))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Color;

    #[test]
    fn slugs() {
        assert_eq!(slugify("My Night Theme!"), "my-night-theme");
        assert_eq!(slugify("  Tokyo  Night (Storm) "), "tokyo-night-storm");
        assert_eq!(slugify("???"), "theme");
    }

    #[test]
    fn check_reports_every_unknown_theme() {
        let lib = ThemeLibrary::default();
        let mut s = ThemeSettings::default();
        assert!(lib.check(&s).is_empty());
        s.follow_system = false;
        s.name = "nope".into();
        s.dark = "also-nope".into();
        assert_eq!(lib.check(&s).iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["theme.name"]);
        s.follow_system = true;
        assert_eq!(lib.check(&s).iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["theme.dark"], "the unused one too");
    }

    #[test]
    fn user_themes_load_save_shadow_and_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("themes");
        let lib = ThemeLibrary::load(&dir);
        assert!(lib.user.is_empty() && lib.issues.is_empty());
        assert_eq!(lib.all().len(), THEMES.len());

        let mut colors = find_theme("nord").unwrap().colors;
        colors.background = Color(0x000000);
        colors.accent = Some(Color(0xa3be8c));
        lib.save("my-nord", "My Nord", &colors).unwrap();
        lib.save("dracula", "Dracula (mine)", &colors).unwrap();
        std::fs::write(dir.join("broken.json"), "{\"name\": 1}").unwrap();

        let lib = ThemeLibrary::load(&dir);
        assert_eq!(lib.user.len(), 2);
        assert_eq!(lib.issues.len(), 1);
        let all = lib.all();
        assert_eq!(all.len(), THEMES.len() + 1, "dracula replaced, my-nord added");
        assert!(all.iter().find(|t| t.id == "dracula").unwrap().user);
        assert_eq!(lib.unique_id("My Nord"), "my-nord-2");
        assert_eq!(lib.unique_id("Nord"), "nord-2");
        assert_eq!(lib.unique_id("Fresh"), "fresh");

        let s = ThemeSettings { name: "my-nord".into(), ..Default::default() };
        let (c, issue) = lib.resolve(&s, true);
        assert_eq!(c.background, Color(0));
        assert_eq!(c.accent, Some(Color(0xa3be8c)));
        assert_eq!(c.accent(), Color(0xa3be8c));
        assert!(issue.is_none());

        let s = ThemeSettings { follow_system: true, light: "nope".into(), ..Default::default() };
        let (c, issue) = lib.resolve(&s, false);
        assert_eq!(issue.unwrap().path, "theme.light");
        assert_eq!(c, find_theme(DEFAULT_THEME).unwrap().colors);

        assert!(lib.delete("my-nord").unwrap());
        assert!(!lib.delete("my-nord").unwrap());
    }
}
