//! A plugin directory as files with checksums: what a registry index lists
//! and what an install copies. Hidden files are not part of a plugin;
//! symlinks and oversized files are refused.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub const MAX_FILE: u64 = 1 << 20;
pub const MAX_TOTAL: u64 = 4 << 20;
pub const MAX_FILES: usize = 200;

/// Lowercase hex sha256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The plugin's files under `dir`: relative path (with `/`) → sha256.
pub fn list_files(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut total = 0u64;
    walk(dir, "", &mut out, &mut total)?;
    if !out.contains_key(crate::manifest::FILE) {
        bail!("{} has no {}", dir.display(), crate::manifest::FILE);
    }
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>, total: &mut u64) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if !crate::manifest::is_bundle_path(&rel) {
            bail!("{rel:?}: plugin file names may only use letters, digits, '.', '_', '+' and '-'");
        }
        let meta = std::fs::symlink_metadata(e.path())?;
        if meta.file_type().is_symlink() {
            bail!("{rel} is a symlink; plugins contain plain files only");
        } else if meta.is_dir() {
            walk(&e.path(), &rel, out, total)?;
        } else if meta.is_file() {
            if meta.len() > MAX_FILE {
                bail!("{rel} is larger than {} KiB", MAX_FILE >> 10);
            }
            *total += meta.len();
            if *total > MAX_TOTAL {
                bail!("the plugin's files are larger than {} MiB together", MAX_TOTAL >> 20);
            }
            let bytes = std::fs::read(e.path())?;
            out.insert(rel, sha256_hex(&bytes));
            if out.len() > MAX_FILES {
                bail!("more than {MAX_FILES} files");
            }
        }
    }
    Ok(())
}

/// Copy the files listed in `files` from `src` to `dst`, checking each
/// against its sha256 first. Only listed files are copied.
pub fn copy_verified(src: &Path, dst: &Path, files: &BTreeMap<String, String>) -> Result<()> {
    for (rel, sha) in files {
        if !crate::manifest::is_bundle_path(rel) {
            bail!("refusing file {rel:?}");
        }
        let from = src.join(rel);
        let meta = std::fs::symlink_metadata(&from).with_context(|| format!("{rel} is missing"))?;
        if !meta.is_file() || meta.len() > MAX_FILE {
            bail!("{rel} is not a plain file of at most {} KiB", MAX_FILE >> 10);
        }
        let bytes = std::fs::read(&from)?;
        let got = sha256_hex(&bytes);
        if got != *sha {
            bail!("{rel} doesn't match its checksum (expected {sha}, got {got})");
        }
        let to = dst.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&to, bytes).with_context(|| format!("write {}", to.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("zsh")).unwrap();
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join("plugin.toml"), "x").unwrap();
        std::fs::write(src.join("zsh/init.zsh"), "y").unwrap();
        std::fs::write(src.join(".git/config"), "hidden").unwrap();
        let files = list_files(&src).unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), ["plugin.toml", "zsh/init.zsh"]);
        assert_eq!(files["plugin.toml"], sha256_hex(b"x"));

        let dst = tmp.path().join("dst");
        copy_verified(&src, &dst, &files).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("zsh/init.zsh")).unwrap(), "y");

        std::fs::write(src.join("zsh/init.zsh"), "changed").unwrap();
        let err = copy_verified(&src, &tmp.path().join("dst2"), &files).unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");

        std::os::unix::fs::symlink("/etc/passwd", src.join("link")).unwrap();
        assert!(list_files(&src).unwrap_err().to_string().contains("symlink"));
    }
}
