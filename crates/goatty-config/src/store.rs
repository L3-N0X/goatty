//! Versioned JSON documents on disk.
//!
//! Loading never fails: a bad value falls back to its default and becomes
//! an [`Issue`] (shown in the settings window), unknown keys (say, from a
//! newer goatty) are reported and kept, and older documents are migrated.
//! Saving writes only what differs from the defaults, so improved defaults
//! reach users who never touched a setting. The previous file goes to
//! `backups/` first, and the new one replaces it atomically.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Upgrades a document from version `.0` to `.0 + 1`.
pub type Migration = (u32, fn(&mut Value));

/// A settings document type.
pub trait Document: Serialize + DeserializeOwned + Default + Clone {
    /// File stem and label (`settings`, `host`).
    const NAME: &'static str;
    /// Current schema version, stored as `"version"`.
    const VERSION: u32;
    /// Dotted paths of maps whose keys are user-chosen (not unknown settings).
    const OPEN_MAPS: &'static [&'static str];
    fn migrations() -> &'static [Migration];
    /// Semantic checks after parsing: clamp ranges, report unknown names.
    fn validate(&mut self) -> Vec<Issue>;
}

/// Something wrong with a settings document. goatty used a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Dotted path (`font.size`), empty for the whole document.
    pub path: String,
    pub message: String,
}

impl Issue {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { path: path.into(), message: message.into() }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() { f.write_str(&self.message) } else { write!(f, "{}: {}", self.path, self.message) }
    }
}

/// A key goatty doesn't know, with its value, kept across saves.
pub type Unknown = (Vec<String>, Value);

#[derive(Debug, Clone)]
pub struct Loaded<T> {
    pub value: T,
    pub issues: Vec<Issue>,
    pub unknown: Vec<Unknown>,
}

impl<T: Document> Default for Loaded<T> {
    fn default() -> Self {
        Self { value: T::default(), issues: Vec::new(), unknown: Vec::new() }
    }
}

/// Load `path`. A missing file means defaults, without issues.
pub fn load<T: Document>(path: &Path) -> Loaded<T> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Loaded::default(),
        Err(e) => {
            Loaded { issues: vec![Issue::new("", format!("can't read {} ({e}); using defaults", path.display()))], ..Loaded::default() }
        }
    }
}

/// Parse a document leniently (see the module docs).
pub fn parse<T: Document>(text: &str) -> Loaded<T> {
    let mut issues = Vec::new();
    let fail = |msg: String| Loaded::<T> { issues: vec![Issue::new("", msg)], ..Loaded::default() };
    let user = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => m,
        Ok(_) => return fail(format!("{} must be a JSON object; using defaults", T::NAME)),
        Err(e) => return fail(format!("not valid JSON ({e}); using defaults")),
    };
    let mut user = Value::Object(user);

    let version = match user.as_object_mut().and_then(|m| m.remove("version")) {
        None => T::VERSION,
        Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|&n| n >= 1) {
            Some(n) => n,
            None => {
                issues.push(Issue::new("version", format!("{v} is not a version number; reading it as version {}", T::VERSION)));
                T::VERSION
            }
        },
    };
    if version > T::VERSION {
        issues.push(Issue::new(
            "version",
            format!("written by a newer goatty (version {version}, this one knows {}); unknown settings are kept", T::VERSION),
        ));
    }
    let mut migrations: Vec<&Migration> = T::migrations().iter().filter(|(from, _)| *from >= version && *from < T::VERSION).collect();
    migrations.sort_by_key(|(from, _)| *from);
    for (_, migrate) in migrations {
        migrate(&mut user);
    }

    let default = serde_json::to_value(T::default()).expect("defaults serialize");
    let mut unknown = Vec::new();
    take_unknown(&default, &mut user, &mut Vec::new(), T::OPEN_MAPS, &mut unknown);
    for (path, _) in &unknown {
        issues.push(Issue::new(path.join("."), "unknown setting; ignored (kept in the file)"));
    }

    let mut merged = default.clone();
    merge(&mut merged, user);
    let mut last_error: Option<Vec<String>> = None;
    let mut value = loop {
        match serde_path_to_error::deserialize::<_, T>(merged.clone()) {
            Ok(v) => break v,
            Err(e) => {
                let mut path = path_segments(e.path());
                // The same path failing twice: reset the enclosing value.
                if last_error.as_ref() == Some(&path) {
                    path.pop();
                }
                if path.is_empty() || !revert(&mut merged, &default, &path) {
                    issues.push(Issue::new("", format!("{} couldn't be read ({}); using defaults", T::NAME, e.inner())));
                    break T::default();
                }
                issues.push(Issue::new(path.join("."), format!("{}; using the default", e.inner())));
                last_error = Some(path);
            }
        }
    };
    issues.extend(value.validate());
    Loaded { value, issues, unknown }
}

fn path_segments(p: &serde_path_to_error::Path) -> Vec<String> {
    use serde_path_to_error::Segment;
    let mut out = Vec::new();
    for seg in p.iter() {
        match seg {
            Segment::Map { key } => out.push(key.clone()),
            Segment::Seq { index } => out.push(index.to_string()),
            Segment::Enum { .. } => {}
            Segment::Unknown => break,
        }
    }
    out
}

fn get_mut<'a>(v: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    match v {
        Value::Object(m) => m.get_mut(key),
        Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get_mut(i)),
        _ => None,
    }
}

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m.get(key),
        Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    }
}

/// Put the default back at `path` (or drop the entry when there is no
/// default). Returns whether anything changed.
fn revert(merged: &mut Value, default: &Value, path: &[String]) -> bool {
    let (last, parents) = path.split_last().expect("non-empty path");
    let mut cur = merged;
    let mut def = Some(default);
    for key in parents {
        let Some(next) = get_mut(cur, key) else { return false };
        cur = next;
        def = def.and_then(|d| get(d, key));
    }
    let replacement = def.and_then(|d| get(d, last)).cloned();
    match (cur, replacement) {
        (Value::Object(m), Some(r)) => m.insert(last.clone(), r).is_some(),
        (Value::Object(m), None) => m.remove(last).is_some(),
        (Value::Array(a), r) => match last.parse::<usize>().ok().filter(|&i| i < a.len()) {
            Some(i) => {
                let r = r.unwrap_or(Value::Null);
                let changed = a[i] != r;
                a[i] = r;
                changed
            }
            None => false,
        },
        _ => false,
    }
}

/// Move keys of `user` that `default` doesn't have into `out`.
fn take_unknown(default: &Value, user: &mut Value, path: &mut Vec<String>, open: &[&str], out: &mut Vec<Unknown>) {
    let (Value::Object(d), Value::Object(u)) = (default, user) else { return };
    let keys: Vec<String> = u.keys().cloned().collect();
    for key in keys {
        path.push(key.clone());
        if open.contains(&path.join(".").as_str()) {
            // User-chosen keys; the deserializer checks the values.
        } else if let Some(dv) = d.get(&key) {
            take_unknown(dv, u.get_mut(&key).expect("key exists"), path, open, out);
        } else if let Some(v) = u.remove(&key) {
            out.push((path.clone(), v));
        }
        path.pop();
    }
}

/// Overlay `user` onto `base`, recursing into objects.
fn merge(base: &mut Value, user: Value) {
    match (base, user) {
        (Value::Object(b), Value::Object(u)) => {
            for (k, v) in u {
                match b.get_mut(&k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge(existing, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, u) => *b = u,
    }
}

/// The parts of `full` that differ from `default`.
fn diff(full: &Value, default: Option<&Value>) -> Option<Value> {
    match (full, default) {
        (Value::Object(f), Some(Value::Object(d))) => {
            let m: Map<String, Value> = f.iter().filter_map(|(k, v)| diff(v, d.get(k)).map(|v| (k.clone(), v))).collect();
            (!m.is_empty()).then_some(Value::Object(m))
        }
        (f, Some(d)) if f == d => None,
        (f, _) => Some(f.clone()),
    }
}

fn insert_at(root: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut cur = root;
    for key in parents {
        let entry = cur.entry(key.clone()).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            return; // a known setting sits there now; drop the stale key
        }
        cur = entry.as_object_mut().expect("object");
    }
    cur.entry(last.clone()).or_insert(value);
}

/// The JSON goatty writes: `version`, non-default values, unknown keys.
pub fn to_json<T: Document>(value: &T, unknown: &[Unknown]) -> String {
    let mut root = match diff(
        &serde_json::to_value(value).expect("settings serialize"),
        Some(&serde_json::to_value(T::default()).expect("defaults serialize")),
    ) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    for (path, v) in unknown {
        insert_at(&mut root, path, v.clone());
    }
    root.insert("version".into(), Value::from(T::VERSION));
    let mut text = serde_json::to_string_pretty(&Value::Object(root)).expect("json");
    text.push('\n');
    text
}

/// Save `value` to `path` if its JSON changed. Backs up the previous file
/// to `backups/` (newest [`BACKUPS_KEPT`] kept) and replaces it atomically.
pub fn save<T: Document>(path: &Path, value: &T, unknown: &[Unknown]) -> io::Result<bool> {
    write_text(path, &to_json(value, unknown))
}

pub const BACKUPS_KEPT: usize = 20;

/// Atomically replace `path` with `text` (with a backup), unless identical.
pub fn write_text(path: &Path, text: &str) -> io::Result<bool> {
    let old = std::fs::read_to_string(path).ok();
    if old.as_deref() == Some(text) {
        return Ok(false);
    }
    let dir = path.parent().ok_or_else(|| io::Error::other("settings path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    if old.is_some() {
        backup(path)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

fn backup(path: &Path) -> io::Result<PathBuf> {
    let dir = path.parent().expect("checked").join("backups");
    std::fs::create_dir_all(&dir)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut target = dir.join(format!("{stem}-{secs}.json"));
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stem}-{secs}-{n}.json"));
        n += 1;
    }
    std::fs::copy(path, &target)?;
    let mut olds: Vec<PathBuf> = backups(path);
    while olds.len() > BACKUPS_KEPT {
        let _ = std::fs::remove_file(olds.remove(0));
    }
    Ok(target)
}

/// Backups of `path`, oldest first.
pub fn backups(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent().map(|d| d.join("backups")) else { return Vec::new() };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let prefix = format!("{stem}-");
    let mut v: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), e.path()))
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

/// Whether `key` (dotted) names a setting of `T` (or an entry of an open map).
pub fn is_known_key<T: Document>(key: &str) -> bool {
    let default = serde_json::to_value(T::default()).expect("defaults serialize");
    let mut cur = &default;
    let mut path = String::new();
    for seg in key.split('.') {
        if T::OPEN_MAPS.contains(&path.as_str()) {
            return !seg.is_empty();
        }
        match cur.get(seg) {
            Some(next) => cur = next,
            None => return false,
        }
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(seg);
    }
    !key.is_empty()
}

/// The document's JSON object (empty text: an empty document).
fn raw_object(text: &str) -> Result<Map<String, Value>, String> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err("the settings file isn't a JSON object; fix or remove it first".into()),
        Err(e) => Err(format!("the settings file isn't valid JSON ({e}); fix or remove it first")),
    }
}

/// Parse after an edit, refusing the edit if `key` didn't take.
fn reparse<T: Document>(root: Map<String, Value>, key: &str) -> Result<Loaded<T>, String> {
    let loaded = parse::<T>(&Value::Object(root).to_string());
    let prefix = format!("{key}.");
    match loaded.issues.iter().find(|i| i.path == key || i.path.starts_with(&prefix) || key.starts_with(&format!("{}.", i.path))) {
        Some(i) => Err(i.to_string()),
        None => Ok(loaded),
    }
}

/// Set `key` (dotted, e.g. `history.size`) in the document `text`. `raw`
/// is read as JSON when it parses (`5000`, `true`, `null`, `[...]`), else
/// as a string (`minimal`, `/bin/zsh`). Unknown keys and invalid values are
/// errors; nothing is written.
pub fn set_key<T: Document>(text: &str, key: &str, raw: &str) -> Result<Loaded<T>, String> {
    if key == "version" || !is_known_key::<T>(key) {
        return Err(format!("unknown setting {key:?} (see `show --all` for the known ones)"));
    }
    let mut root = raw_object(text)?;
    let value = serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
    let parts: Vec<String> = key.split('.').map(str::to_owned).collect();
    let (last, parents) = parts.split_last().expect("non-empty key");
    let mut cur = &mut root;
    for p in parents {
        let entry = cur.entry(p.clone()).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        cur = entry.as_object_mut().expect("object");
    }
    cur.insert(last.clone(), value);
    reparse::<T>(root, key)
}

/// Remove `key` from the document, so it's back to its default.
pub fn unset_key<T: Document>(text: &str, key: &str) -> Result<Loaded<T>, String> {
    if !is_known_key::<T>(key) {
        return Err(format!("unknown setting {key:?}"));
    }
    let mut root = raw_object(text)?;
    let parts: Vec<&str> = key.split('.').collect();
    fn remove(m: &mut Map<String, Value>, parts: &[&str]) {
        match parts {
            [] => {}
            [last] => {
                m.remove(*last);
            }
            [first, rest @ ..] => {
                if let Some(Value::Object(child)) = m.get_mut(*first) {
                    remove(child, rest);
                    if child.is_empty() {
                        m.remove(*first);
                    }
                }
            }
        }
    }
    remove(&mut root, &parts);
    reparse::<T>(root, key)
}

/// The value at `key` in `value` (for `show KEY`).
pub fn get_key<T: Document>(value: &T, key: &str) -> Option<Value> {
    let mut cur = serde_json::to_value(value).ok()?;
    for seg in key.split('.').filter(|s| !s.is_empty()) {
        cur = cur.get(seg)?.clone();
    }
    Some(cur)
}

/// Modification stamp, for cheap "did the file change" polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileStamp(Option<(SystemTime, u64)>);

impl FileStamp {
    pub fn of(path: &Path) -> Self {
        Self(std::fs::metadata(path).ok().map(|m| (m.modified().unwrap_or(UNIX_EPOCH), m.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ClientSettings, Renderer};
    use crate::host::HostSettings;

    #[test]
    fn empty_and_missing_are_defaults() {
        let l = parse::<ClientSettings>("{}");
        assert_eq!(l.value, ClientSettings::default());
        assert!(l.issues.is_empty());
        let tmp = tempfile::tempdir().unwrap();
        let l = load::<HostSettings>(&tmp.path().join("nope.json"));
        assert!(l.issues.is_empty());
        assert_eq!(l.value, HostSettings::default());
    }

    #[test]
    fn bad_values_fall_back_one_by_one() {
        let l = parse::<ClientSettings>(
            r##"{
                "version": 1,
                "font": { "size": "big", "line_height": 1.5, "family": "Iosevka" },
                "terminal": { "renderer": "fancy", "padding": 10 },
                "theme": { "name": "nord", "overrides": { "background": "blue", "ansi": [null, "#ff0000", 7] } }
            }"##,
        );
        let v = &l.value;
        assert_eq!(v.font.size, 14.0);
        assert_eq!(v.font.line_height, 1.5);
        assert_eq!(v.font.family.as_deref(), Some("Iosevka"));
        assert_eq!(v.terminal.renderer, Renderer::Glyphs);
        assert_eq!(v.terminal.padding, crate::client::Padding::all(10.0));
        assert_eq!(v.theme.name, "nord");
        assert_eq!(v.theme.overrides.background, None);
        assert_eq!(v.theme.overrides.ansi, vec![None, Some(crate::Color(0xff0000)), None]);
        let paths: Vec<_> = l.issues.iter().map(|i| i.path.as_str()).collect();
        for p in ["font.size", "terminal.renderer", "theme.overrides.background", "theme.overrides.ansi.2"] {
            assert!(paths.contains(&p), "{p} missing from {paths:?}");
        }
    }

    #[test]
    fn garbage_is_defaults_with_an_issue() {
        for text in ["not json", "[1,2]", "{\"font\": 3}"] {
            let l = parse::<ClientSettings>(text);
            assert_eq!(l.value.font, ClientSettings::default().font, "{text}");
            assert!(!l.issues.is_empty(), "{text}");
        }
    }

    #[test]
    fn unknown_keys_are_reported_and_kept() {
        let text = r#"{"version": 1, "future": {"x": 1}, "font": {"size": 15, "kerning": true}, "keybindings": {"ctrl-k": "paste"}}"#;
        let l = parse::<ClientSettings>(text);
        assert_eq!(l.value.font.size, 15.0);
        assert_eq!(l.value.keybindings.get("ctrl-k"), Some(&Some("paste".into())));
        // Key order depends on serde_json's `preserve_order` (feature-unified in the workspace).
        let mut paths: Vec<_> = l.issues.iter().map(|i| i.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["font.kerning", "future"]);

        let out: Value = serde_json::from_str(&to_json(&l.value, &l.unknown)).unwrap();
        assert_eq!(out["future"]["x"], 1);
        assert_eq!(out["font"]["kerning"], true);
        assert_eq!(out["font"]["size"], 15.0);
        assert_eq!(out["version"], 1);
        assert!(out.get("terminal").is_none(), "defaults aren't written: {out}");
    }

    #[test]
    fn newer_version_is_flagged() {
        let l = parse::<HostSettings>(r#"{"version": 9}"#);
        assert_eq!(l.issues[0].path, "version");
    }

    #[test]
    fn dotted_key_editing() {
        assert!(is_known_key::<HostSettings>("history.size"));
        assert!(is_known_key::<HostSettings>("shell.zsh.user_rc"));
        assert!(!is_known_key::<HostSettings>("history.nope"));
        assert!(is_known_key::<ClientSettings>("keybindings.ctrl-k"), "open map entries");
        assert!(!is_known_key::<ClientSettings>(""));

        let l = set_key::<HostSettings>("", "history.size", "5000").unwrap();
        assert_eq!(l.value.history.size, 5000);
        let text = to_json(&l.value, &l.unknown);
        let l = set_key::<HostSettings>(&text, "prompt.style", "minimal").unwrap();
        assert_eq!(l.value.prompt.style, crate::PromptStyle::Minimal);
        assert_eq!(l.value.history.size, 5000, "other values stay");
        let l = set_key::<HostSettings>(&to_json(&l.value, &[]), "shell.program", "/bin/zsh").unwrap();
        assert_eq!(l.value.shell.program.as_deref(), Some("/bin/zsh"));

        assert!(set_key::<HostSettings>("", "history.size", "lots").unwrap_err().starts_with("history.size"));
        assert!(set_key::<HostSettings>("", "prompt.style", "fancy").is_err());
        assert!(set_key::<HostSettings>("", "history.nope", "1").is_err());
        assert!(set_key::<HostSettings>("", "version", "2").is_err());
        assert!(set_key::<HostSettings>("not json", "history.size", "1").is_err());

        let l = unset_key::<HostSettings>(&to_json(&l.value, &[]), "history.size").unwrap();
        assert_eq!(l.value.history.size, HostSettings::default().history.size);
        assert_eq!(get_key(&l.value, "prompt.style"), Some(Value::from("minimal")));
        assert_eq!(get_key(&l.value, "prompt.nope"), None);

        let l = set_key::<ClientSettings>("", "keybindings.ctrl-k", "null").unwrap();
        assert_eq!(l.value.keybindings.get("ctrl-k"), Some(&None));
    }

    #[test]
    fn save_backs_up_and_skips_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let mut s = ClientSettings::default();
        assert!(save(&path, &s, &[]).unwrap());
        assert!(!save(&path, &s, &[]).unwrap(), "unchanged");
        assert!(backups(&path).is_empty(), "nothing to back up the first time");
        s.font.size = 18.0;
        assert!(save(&path, &s, &[]).unwrap());
        assert_eq!(backups(&path).len(), 1);
        assert_eq!(load::<ClientSettings>(&path).value.font.size, 18.0);
        assert_eq!(std::fs::read_to_string(&backups(&path)[0]).unwrap(), "{\n  \"version\": 1\n}\n");
        for i in 0..(BACKUPS_KEPT + 5) {
            s.font.size = 10.0 + i as f32;
            save(&path, &s, &[]).unwrap();
        }
        assert_eq!(backups(&path).len(), BACKUPS_KEPT);
    }
}
