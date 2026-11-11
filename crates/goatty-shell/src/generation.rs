//! Generations: built, validated, atomically activated shell configurations.
//!
//! Applying host settings is a build-and-activate step (plan §6):
//!
//! 1. render the startup files from the settings ([`crate::zsh::render`]),
//! 2. compare with the active generation; identical → nothing to do,
//! 3. write them into a staging directory with the settings snapshot,
//! 4. validate (`zsh -n` on every startup file),
//! 5. rename the staging directory to `generations/<n>` and switch the
//!    `current` symlink to it (rename over the old link: atomic),
//! 6. keep the newest [`KEEP`] generations for rollback.
//!
//! Sessions get `ZDOTDIR=<data>/current/zsh`, so a new session always sees
//! a complete generation. Running shells already read theirs.
//!
//! A rollback switches `current` back and returns the generation, whose
//! `host.json` the caller restores as the settings file, so the next build
//! reproduces it instead of undoing the rollback.
//!
//! Each generation's `info.json` ([`Info`]) records when it was built, the
//! generation it was built on, what changed since then and its name
//! ([`crate::history`]).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::dirs::Dirs;
use crate::history::{self, Changes};
use crate::zsh::GeneratedFile;

/// Generations kept (the active one is never removed).
pub const KEEP: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub number: u32,
    pub path: PathBuf,
    pub created_unix: u64,
    pub active: bool,
}

impl Generation {
    pub fn zsh_dir(&self) -> PathBuf {
        self.path.join("zsh")
    }

    /// The host settings this generation was built from.
    pub fn settings_json(&self) -> Option<String> {
        std::fs::read_to_string(self.path.join("host.json")).ok()
    }
}

/// A generation's `info.json`. `created` stays the first field: goatty
/// before #55 read it with a string search.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    pub created: u64,
    /// The goatty version that built it.
    pub goatty: String,
    /// The generation that was active when it was built.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Changes>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The `info.json` in generation directory `dir` (defaults if unreadable).
pub fn read_info(dir: &Path) -> Info {
    std::fs::read_to_string(dir.join("info.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Replace `info.json` in `dir` (atomically: written next to it, renamed).
pub(crate) fn write_info(dir: &Path, info: &Info) -> Result<()> {
    let tmp = dir.join(format!(".info.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string(info)? + "\n")?;
    std::fs::rename(&tmp, dir.join("info.json")).with_context(|| format!("write {}", dir.join("info.json").display()))
}

/// Hold the generations lock: builders in other processes (goattyd,
/// `goatty --local`, the CLI) wait.
pub(crate) fn lock(dirs: &Dirs) -> Result<File> {
    let lock = File::create(dirs.generations_dir().join(".lock"))?;
    lock.lock()?;
    Ok(lock)
}

/// The active generation's number.
pub fn current(dirs: &Dirs) -> Option<u32> {
    let target = std::fs::read_link(dirs.current_link()).ok()?;
    target.file_name()?.to_str()?.parse().ok()
}

/// All generations, oldest first.
pub fn list(dirs: &Dirs) -> Vec<Generation> {
    let active = current(dirs);
    let mut v: Vec<Generation> = std::fs::read_dir(dirs.generations_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let number: u32 = e.file_name().to_str()?.parse().ok()?;
            let path = e.path();
            let created_unix = read_info(&path).created;
            Some(Generation { number, path, created_unix, active: active == Some(number) })
        })
        .collect();
    v.sort_by_key(|g| g.number);
    v
}

fn read_files(dir: &Path, like: &[GeneratedFile]) -> Option<Vec<String>> {
    like.iter().map(|f| std::fs::read_to_string(dir.join(&f.name)).ok()).collect()
}

/// What [`activate`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activated {
    pub generation: u32,
    /// `false`: the active generation already matched.
    pub created: bool,
}

/// Build and activate a generation from `files` and the settings snapshot
/// `settings_json`, unless the active one already matches both.
pub fn activate(dirs: &Dirs, files: &[GeneratedFile], settings_json: &str) -> Result<Activated> {
    let gens = dirs.generations_dir();
    std::fs::create_dir_all(&gens).with_context(|| format!("create {}", gens.display()))?;
    let _lock = lock(dirs)?;

    if let Some(n) = current(dirs) {
        let path = gens.join(n.to_string());
        let same_files = read_files(&path.join("zsh"), files).is_some_and(|old| old.iter().zip(files).all(|(o, f)| *o == f.contents));
        let same_settings = std::fs::read_to_string(path.join("host.json")).is_ok_and(|s| s == settings_json);
        if same_files && same_settings {
            return Ok(Activated { generation: n, created: false });
        }
    }

    let number = list(dirs).last().map(|g| g.number).unwrap_or(0) + 1;
    let staging = gens.join(format!(".build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| -> Result<()> {
        let zdir = staging.join("zsh");
        std::fs::create_dir_all(&zdir)?;
        for f in files {
            let path = zdir.join(&f.name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, &f.contents)?;
        }
        std::fs::write(staging.join("host.json"), settings_json)?;
        let version = env!("CARGO_PKG_VERSION");
        let base = current(dirs).and_then(|n| list(dirs).into_iter().find(|g| g.number == n));
        let tree = files.iter().map(|f| (f.name.clone(), f.contents.clone())).collect();
        let info = Info {
            created: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            goatty: version.to_owned(),
            parent: base.as_ref().map(|b| b.number),
            changes: Some(base.map(|b| history::compare(&b, settings_json, &tree, version)).unwrap_or_default()),
            name: None,
        };
        write_info(&staging, &info)?;
        validate(&zdir, files)?;
        std::fs::rename(&staging, gens.join(number.to_string()))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    switch(dirs, number)?;
    prune(dirs);
    Ok(Activated { generation: number, created: true })
}

/// Syntax-check every zsh file with `zsh -n`. Skipped without zsh (then no
/// session can run it anyway).
fn validate(zdir: &Path, files: &[GeneratedFile]) -> Result<()> {
    let Some(zsh) = crate::launch::which("zsh") else { return Ok(()) };
    for f in files.iter().filter(|f| f.name.ends_with(".zsh") || f.name.starts_with(".z")) {
        let out = Command::new(&zsh).arg("-n").arg(zdir.join(&f.name)).stdin(Stdio::null()).output().context("run zsh -n")?;
        if !out.status.success() {
            bail!("{} doesn't parse: {}", f.name, String::from_utf8_lossy(&out.stderr).trim());
        }
    }
    Ok(())
}

/// Point `current` at generation `number` (relative link, renamed into place).
fn switch(dirs: &Dirs, number: u32) -> Result<()> {
    let link = dirs.current_link();
    let tmp = dirs.data.join(format!(".current.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(Path::new("generations").join(number.to_string()), &tmp)?;
    std::fs::rename(&tmp, &link).with_context(|| format!("activate generation {number}"))?;
    Ok(())
}

fn prune(dirs: &Dirs) {
    let gens = list(dirs);
    let excess = gens.len().saturating_sub(KEEP);
    for g in gens.iter().filter(|g| !g.active).take(excess) {
        let _ = std::fs::remove_dir_all(&g.path);
    }
}

/// Make generation `to` (default: the one before the active one) active.
/// The caller restores its settings snapshot ([`Generation::settings_json`]).
pub fn rollback(dirs: &Dirs, to: Option<u32>) -> Result<Generation> {
    let gens = list(dirs);
    let active = current(dirs);
    let target = match to {
        Some(n) => gens.iter().find(|g| g.number == n).with_context(|| format!("no generation {n}"))?,
        None => {
            let a = active.context("no active generation")?;
            gens.iter().rev().find(|g| g.number < a).context("no older generation to roll back to")?
        }
    };
    if !target.zsh_dir().join(".zshrc").is_file() {
        bail!("generation {} is incomplete", target.number);
    }
    // Locked like a build, so a concurrent build doesn't switch underneath.
    let _lock = lock(dirs)?;
    switch(dirs, target.number)?;
    Ok(Generation { active: true, ..target.clone() })
}

/// A changed file, as a unified diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub name: String,
    pub unified: String,
}

/// How `files` differ from the active generation (all of them are new when
/// there's none).
pub fn diff(dirs: &Dirs, files: &[GeneratedFile]) -> Vec<FileDiff> {
    let dir = dirs.zsh_dir();
    files
        .iter()
        .filter_map(|f| {
            let old = std::fs::read_to_string(dir.join(&f.name)).unwrap_or_default();
            (old != f.contents).then(|| FileDiff {
                name: f.name.clone(),
                unified: similar::TextDiff::from_lines(&old, &f.contents)
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("active/{}", f.name), &format!("new/{}", f.name))
                    .to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(rc: &str) -> Vec<GeneratedFile> {
        vec![GeneratedFile::new(".zshrc", rc), GeneratedFile::new("starship.toml", "x = 1\n")]
    }

    #[test]
    fn activate_is_idempotent_and_versioned() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        assert_eq!(current(&dirs), None);
        assert_eq!(diff(&dirs, &files("a=1\n")).len(), 2, "everything is new");

        assert_eq!(activate(&dirs, &files("a=1\n"), "{}").unwrap(), Activated { generation: 1, created: true });
        assert_eq!(activate(&dirs, &files("a=1\n"), "{}").unwrap(), Activated { generation: 1, created: false });
        assert_eq!(std::fs::read_to_string(dirs.zsh_dir().join(".zshrc")).unwrap(), "a=1\n");
        assert!(diff(&dirs, &files("a=1\n")).is_empty());

        let d = diff(&dirs, &files("a=2\n"));
        assert_eq!(d.len(), 1);
        assert!(d[0].unified.contains("-a=1") && d[0].unified.contains("+a=2"), "{}", d[0].unified);

        // A settings-only change is a new generation too (rollback target).
        assert_eq!(activate(&dirs, &files("a=1\n"), "{\"x\":1}").unwrap().generation, 2);
        assert_eq!(activate(&dirs, &files("a=2\n"), "{\"x\":1}").unwrap().generation, 3);
        assert_eq!(std::fs::read_to_string(dirs.zsh_dir().join(".zshrc")).unwrap(), "a=2\n");
        assert_eq!(list(&dirs).iter().filter(|g| g.active).map(|g| g.number).collect::<Vec<_>>(), [3]);
        assert!(list(&dirs)[0].created_unix > 0);
    }

    #[test]
    fn rollback_switches_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        assert!(rollback(&dirs, None).is_err());
        activate(&dirs, &files("a=1\n"), "one").unwrap();
        activate(&dirs, &files("a=2\n"), "two").unwrap();
        let g = rollback(&dirs, None).unwrap();
        assert_eq!(g.number, 1);
        assert_eq!(g.settings_json().as_deref(), Some("one"));
        assert_eq!(current(&dirs), Some(1));
        assert_eq!(std::fs::read_to_string(dirs.zsh_dir().join(".zshrc")).unwrap(), "a=1\n");
        assert!(rollback(&dirs, None).is_err(), "nothing older than 1");
        assert_eq!(rollback(&dirs, Some(2)).unwrap().number, 2);
        assert!(rollback(&dirs, Some(9)).is_err());
        // Building after a rollback continues the numbering.
        assert_eq!(activate(&dirs, &files("a=3\n"), "three").unwrap().generation, 3);
    }

    #[test]
    fn prunes_old_generations() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        for i in 0..(KEEP + 4) {
            activate(&dirs, &files(&format!("a={i}\n")), "{}").unwrap();
        }
        let gens = list(&dirs);
        assert_eq!(gens.len(), KEEP);
        assert_eq!(gens.last().unwrap().number as usize, KEEP + 4);
        rollback(&dirs, Some(gens[0].number)).unwrap();
        activate(&dirs, &files("new\n"), "{}").unwrap();
        let after = list(&dirs);
        assert_eq!(after.len(), KEEP);
        assert!(after.last().unwrap().active);
    }

    #[test]
    fn invalid_config_is_not_activated() {
        if crate::launch::which("zsh").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        activate(&dirs, &files("a=1\n"), "{}").unwrap();
        let err = activate(&dirs, &files("if then fi (\n"), "{}").unwrap_err();
        assert!(err.to_string().contains(".zshrc doesn't parse"), "{err}");
        assert_eq!(current(&dirs), Some(1));
        assert_eq!(list(&dirs).len(), 1, "no half-built generation left");
    }
}
