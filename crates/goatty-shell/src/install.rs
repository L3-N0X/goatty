//! User-space installs of the pinned tools in [`crate::tools`].
//!
//! An install downloads the release archive, checks it against the pinned
//! sha256, unpacks it into `tools/<id>/<version>/` (staged, then renamed),
//! and switches the `bin/<exe>` or `plugins/<id>` symlink atomically. No
//! sudo, nothing outside goatty's data directory.
//!
//! Downloads go through `curl` (or `wget`): the checksum, not the transport,
//! is what makes an archive trusted, so goattyd needs no TLS stack of its own.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::dirs::Dirs;
use crate::tools::{Asset, Kind, Tool};

/// Download `url` with curl (or wget) through a temp file under `cache`.
pub fn download(url: &str, cache: &Path) -> Result<Vec<u8>> {
    let dir = cache.join("downloads");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let part = dir.join(format!("{}.part", std::process::id()));
    let result = fetch_to(url, &part).and_then(|()| std::fs::read(&part).context("read download"));
    let _ = std::fs::remove_file(&part);
    result
}

fn fetch_to(url: &str, out: &Path) -> Result<()> {
    let attempts: [(&str, Vec<&str>); 2] =
        [("curl", vec!["--proto", "=https", "--tlsv1.2", "-fsSL", "--retry", "2", "-o"]), ("wget", vec!["-q", "--https-only", "-O"])];
    for (prog, args) in attempts {
        let status = Command::new(prog).args(args).arg(out).arg(url).stdin(Stdio::null()).status();
        match status {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => bail!("{prog} failed ({s}) for {url}"),
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e).context(format!("run {prog}")),
        }
    }
    bail!("neither curl nor wget is installed; install one of them to download tools")
}

/// Lowercase hex sha256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Verify `archive` against `asset`, unpack it and make it the active
/// managed copy of `tool`. Returns the activated path (binary or plugin
/// entry file). Older managed versions of the tool are removed.
pub fn install_archive(tool: &Tool, asset: &Asset, archive: &[u8], dirs: &Dirs) -> Result<PathBuf> {
    let got = sha256_hex(archive);
    if !got.eq_ignore_ascii_case(asset.sha256) {
        bail!("checksum mismatch for {} {}: expected {}, got {got}; not installing", tool.name, tool.version, asset.sha256);
    }

    let tool_dir = dirs.tools_dir().join(tool.id);
    std::fs::create_dir_all(&tool_dir).with_context(|| format!("create {}", tool_dir.display()))?;
    let staging = tool_dir.join(format!(".{}.partial-{}", tool.version, std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = unpack_and_check(tool, asset, archive, &staging);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let version_dir = tool_dir.join(tool.version);
    if version_dir.exists() {
        std::fs::remove_dir_all(&version_dir).with_context(|| format!("remove old {}", version_dir.display()))?;
    }
    std::fs::rename(&staging, &version_dir)?;

    let payload = version_dir.join(asset.root);
    let (link_target, active) = match tool.kind {
        Kind::Binary { exe } => (payload.join(exe), payload.join(exe)),
        Kind::ZshPlugin { entry } => (payload.clone(), payload.join(entry)),
    };
    replace_symlink(&link_target, &tool.link_path(dirs))?;

    // One managed version per tool. Running processes keep their open
    // files, so removing an older version under them is safe on Unix.
    for entry in std::fs::read_dir(&tool_dir)?.flatten() {
        if entry.file_name() != tool.version {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(active)
}

fn unpack_and_check(tool: &Tool, asset: &Asset, archive: &[u8], staging: &Path) -> Result<()> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    ar.set_preserve_permissions(true);
    // `unpack` refuses entries that would land outside `staging`.
    ar.unpack(staging).with_context(|| format!("unpack {}", asset.url))?;
    let payload = staging.join(asset.root);
    match tool.kind {
        Kind::Binary { exe } => {
            let bin = payload.join(exe);
            if !bin.is_file() {
                bail!("{} archive has no {}/{exe}", tool.name, asset.root);
            }
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
        }
        Kind::ZshPlugin { entry } => {
            if !payload.join(entry).is_file() {
                bail!("{} archive has no {}/{entry}", tool.name, asset.root);
            }
        }
    }
    Ok(())
}

/// Point `link` at `target`, atomically (symlink beside it, then rename).
fn replace_symlink(target: &Path, link: &Path) -> Result<()> {
    let dir = link.parent().context("link has no parent")?;
    std::fs::create_dir_all(dir)?;
    let name = link.file_name().context("link has no name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).with_context(|| format!("symlink {}", tmp.display()))?;
    std::fs::rename(&tmp, link).with_context(|| format!("activate {}", link.display()))?;
    Ok(())
}

/// Remove the managed copy of `tool`. Returns whether there was one.
pub fn uninstall(tool: &Tool, dirs: &Dirs) -> Result<bool> {
    let link = tool.link_path(dirs);
    let tool_dir = dirs.tools_dir().join(tool.id);
    let had = link.symlink_metadata().is_ok() || tool_dir.exists();
    match std::fs::remove_file(&link) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e).context(format!("remove {}", link.display())),
        _ => {}
    }
    if tool_dir.exists() {
        std::fs::remove_dir_all(&tool_dir).with_context(|| format!("remove {}", tool_dir.display()))?;
    }
    Ok(had)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Location;

    fn targz(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data, mode) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(*mode);
            h.set_cksum();
            b.append_data(&mut h, path, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    fn demo_tool(kind: Kind) -> Tool {
        Tool { id: "demo", name: "demo", summary: "", homepage: "", version: "1.0.0", kind, default: false, system_paths: &[], assets: &[] }
    }

    #[test]
    fn installs_binary_and_switches_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let archive = targz(&[("demo-1.0.0/demo", b"#!/bin/sh\necho hi\n", 0o644), ("demo-1.0.0/README", b"x", 0o644)]);
        let asset =
            Asset { target: "any", url: "https://example/demo.tar.gz", sha256: leak(sha256_hex(&archive)), size: 0, root: "demo-1.0.0" };
        let tool = demo_tool(Kind::Binary { exe: "demo" });

        let active = install_archive(&tool, &asset, &archive, &dirs).unwrap();
        assert_eq!(active, dirs.tools_dir().join("demo/1.0.0/demo-1.0.0/demo"));
        let link = dirs.bin_dir().join("demo");
        assert_eq!(std::fs::read_link(&link).unwrap(), active);
        assert_eq!(std::fs::metadata(&link).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(tool.managed_version(&dirs).as_deref(), Some("1.0.0"));
        assert!(matches!(tool.locate(&dirs, Some("")), Location::Managed { .. }));

        // A new version replaces the old one entirely.
        let v2 = Tool { version: "2.0.0", ..demo_tool(Kind::Binary { exe: "demo" }) };
        install_archive(&v2, &asset, &archive, &dirs).unwrap();
        assert_eq!(v2.managed_version(&dirs).as_deref(), Some("2.0.0"));
        assert!(!dirs.tools_dir().join("demo/1.0.0").exists());

        assert!(uninstall(&v2, &dirs).unwrap());
        assert!(link.symlink_metadata().is_err());
        assert!(!dirs.tools_dir().join("demo").exists());
        assert!(!uninstall(&v2, &dirs).unwrap());
    }

    #[test]
    fn installs_plugin_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let archive = targz(&[("p-1/demo.zsh", b"demo_loaded=1\n", 0o644), ("p-1/src/x.zsh", b"", 0o644)]);
        let asset = Asset { target: "any", url: "https://example/p.tar.gz", sha256: leak(sha256_hex(&archive)), size: 0, root: "p-1" };
        let tool = demo_tool(Kind::ZshPlugin { entry: "demo.zsh" });
        let active = install_archive(&tool, &asset, &archive, &dirs).unwrap();
        assert_eq!(active, dirs.tools_dir().join("demo/1.0.0/p-1/demo.zsh"));
        assert!(tool.managed_path(&dirs).is_file(), "plugins/demo/demo.zsh resolves through the link");
    }

    #[test]
    fn rejects_bad_checksum_and_bad_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let archive = targz(&[("demo", b"x", 0o755)]);
        let tool = demo_tool(Kind::Binary { exe: "demo" });

        let wrong = Asset { target: "any", url: "u", sha256: leak("0".repeat(64)), size: 0, root: "" };
        let err = install_archive(&tool, &wrong, &archive, &dirs).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
        assert!(!dirs.tools_dir().join("demo").exists(), "nothing unpacked before the check");

        let missing = Asset { target: "any", url: "u", sha256: leak(sha256_hex(&archive)), size: 0, root: "sub" };
        assert!(install_archive(&tool, &missing, &archive, &dirs).is_err());
        assert!(dirs.bin_dir().join("demo").symlink_metadata().is_err(), "nothing activated");
        let leftovers: Vec<_> = std::fs::read_dir(dirs.tools_dir().join("demo")).unwrap().collect();
        assert!(leftovers.is_empty(), "staging dir cleaned up");
    }
}
