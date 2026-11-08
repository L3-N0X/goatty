//! What's installed: `installed/lock.json` plus a verified copy of each
//! plugin's files in `installed/<id>/`.
//!
//! The lock records where a plugin came from, the checksum of every file
//! that was approved, and the packages it installed. Whatever goes into a
//! shell configuration is checked against these checksums again, so a file
//! changed after the install is noticed instead of sourced.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use goatty_plugin::{Manifest, Permission, manifest};
use serde::{Deserialize, Serialize};

use crate::dirs::Dirs;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Lock {
    pub version: u32,
    pub plugins: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub version: String,
    /// Registry name, or `local`.
    pub source: String,
    /// For `local`: the directory it was installed from.
    #[serde(default)]
    pub path: Option<String>,
    pub verified: bool,
    /// Every file of the plugin, as approved: relative path → sha256.
    pub files: BTreeMap<String, String>,
    pub packages: Vec<Package>,
    pub permissions: Vec<Permission>,
    /// The plan digest the user approved.
    pub digest: String,
    pub installed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub id: String,
    pub version: String,
    pub bins: Vec<String>,
    /// A copy already on PATH was used; nothing was downloaded.
    #[serde(default)]
    pub system: Option<String>,
}

fn file(dirs: &Dirs) -> PathBuf {
    dirs.installed_dir().join("lock.json")
}

/// `installed/<id>/`.
pub fn bundle_dir(dirs: &Dirs, id: &str) -> PathBuf {
    dirs.installed_dir().join(id)
}

impl Lock {
    /// Missing file: nothing installed.
    pub fn load(dirs: &Dirs) -> Result<Lock> {
        match std::fs::read(file(dirs)) {
            Ok(bytes) => {
                let lock: Lock = serde_json::from_slice(&bytes).context("installed/lock.json is unreadable")?;
                if lock.version > VERSION {
                    bail!("installed/lock.json was written by a newer goatty");
                }
                Ok(lock)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Lock { version: VERSION, plugins: BTreeMap::new() }),
            Err(e) => Err(e).context("read installed/lock.json"),
        }
    }

    pub fn save(&self, dirs: &Dirs) -> Result<()> {
        let path = file(dirs);
        std::fs::create_dir_all(dirs.installed_dir())?;
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        let mut text = serde_json::to_string_pretty(&Lock { version: VERSION, plugins: self.plugins.clone() })?;
        text.push('\n');
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path).context("write installed/lock.json")?;
        Ok(())
    }

    /// Executables on PATH from installed plugins: name → plugin id.
    pub fn executables(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (id, e) in &self.plugins {
            for p in &e.packages {
                for b in &p.bins {
                    out.insert(manifest::bin_name(b).to_owned(), id.clone());
                }
            }
        }
        out
    }
}

/// Read a file of an installed plugin, checked against the lock.
pub fn read_checked(dirs: &Dirs, id: &str, entry: &Entry, rel: &str) -> Result<Vec<u8>> {
    let want = entry.files.get(rel).with_context(|| format!("{id}: {rel} isn't part of the approved files"))?;
    let bytes = std::fs::read(bundle_dir(dirs, id).join(rel)).with_context(|| format!("{id}: {rel} is missing"))?;
    if goatty_plugin::bundle::sha256_hex(&bytes) != *want {
        bail!("{id}: {rel} was changed after it was installed; reinstall the plugin");
    }
    Ok(bytes)
}

/// The installed manifest, checked against the lock.
pub fn manifest_of(dirs: &Dirs, id: &str, entry: &Entry) -> Result<Manifest> {
    let bytes = read_checked(dirs, id, entry, manifest::FILE)?;
    let text = String::from_utf8(bytes).context("plugin.toml isn't UTF-8")?;
    let files = entry.files.keys().cloned().collect();
    let m = manifest::parse(&text, Some(&files)).map_err(|e| anyhow::anyhow!("{id}: {e}"))?;
    if m.id != id {
        bail!("installed/{id} holds plugin {:?}", m.id);
    }
    Ok(m)
}
