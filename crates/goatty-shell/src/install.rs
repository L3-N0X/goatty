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

use anyhow::{Context, Result, bail};
use goatty_plugin::manifest::AssetFormat;

use crate::dirs::Dirs;
use crate::tools::{Asset, Kind, Tool};

/// Download `url` with curl (or wget) through a temp file under `cache`.
pub fn download(url: &str, cache: &Path) -> Result<Vec<u8>> {
    goatty_plugin::fetch::download(url, cache)
}

/// Lowercase hex sha256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    goatty_plugin::bundle::sha256_hex(bytes)
}

/// A plugin package to install (`[[package]]` with the asset for this host).
pub struct PackageInstall<'a> {
    pub id: &'a str,
    pub version: &'a str,
    pub sha256: &'a str,
    pub format: AssetFormat,
    pub root: &'a str,
    /// Paths of the executables relative to `root`; linked as `bin/<file name>`.
    pub bins: &'a [String],
}

/// Verify and unpack a plugin package into `tools/<id>/<version>/` and
/// link its executables into `bin/`. A `bin/` link that belongs to
/// something else (a built-in tool, another package, a user's file) is
/// never replaced. Older versions of the package are removed. Returns the
/// activated links.
pub fn install_package(p: &PackageInstall<'_>, archive: &[u8], dirs: &Dirs) -> Result<Vec<PathBuf>> {
    let got = sha256_hex(archive);
    if !got.eq_ignore_ascii_case(p.sha256) {
        bail!("checksum mismatch for {} {}: expected {}, got {got}; not installing", p.id, p.version, p.sha256);
    }
    let names: Vec<&str> = p.bins.iter().map(|b| goatty_plugin::manifest::bin_name(b)).collect();
    let tool_dir = dirs.tools_dir().join(p.id);
    for name in &names {
        if let Some(owner) = link_owner(dirs, name)
            && owner != p.id
        {
            bail!("{name} is already provided by {owner}; not replacing it");
        }
    }

    std::fs::create_dir_all(&tool_dir).with_context(|| format!("create {}", tool_dir.display()))?;
    let staging = tool_dir.join(format!(".{}.partial-{}", p.version, std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = (|| -> Result<()> {
        match p.format {
            AssetFormat::TarGz => {
                let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
                ar.set_preserve_permissions(false);
                ar.unpack(&staging).context("unpack the download")?;
            }
            AssetFormat::Binary => {
                let dest = staging.join(p.root).join(&p.bins[0]);
                std::fs::create_dir_all(dest.parent().context("no parent")?)?;
                std::fs::write(&dest, archive)?;
            }
        }
        for b in p.bins {
            let exe = staging.join(p.root).join(b);
            let meta = std::fs::symlink_metadata(&exe).with_context(|| format!("the download has no {}/{b}", p.root))?;
            if !meta.is_file() {
                bail!("{}/{b} in the download is not a regular file", p.root);
            }
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let version_dir = tool_dir.join(p.version);
    if version_dir.exists() {
        std::fs::remove_dir_all(&version_dir).with_context(|| format!("remove old {}", version_dir.display()))?;
    }
    std::fs::rename(&staging, &version_dir)?;
    let mut links = Vec::new();
    for (b, name) in p.bins.iter().zip(&names) {
        let link = dirs.bin_dir().join(name);
        replace_symlink(&version_dir.join(p.root).join(b), &link)?;
        links.push(link);
    }
    for entry in std::fs::read_dir(&tool_dir)?.flatten() {
        if entry.file_name() != p.version {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(links)
}

/// Which managed tool or package `bin/<name>` belongs to: the `<id>` of
/// `tools/<id>/...` it points into, `"?"` for anything else that exists.
pub fn link_owner(dirs: &Dirs, name: &str) -> Option<String> {
    let link = dirs.bin_dir().join(name);
    link.symlink_metadata().ok()?;
    let target = std::fs::read_link(&link).ok();
    let owner = target
        .as_deref()
        .and_then(|t| t.strip_prefix(dirs.tools_dir()).ok())
        .and_then(|rel| rel.components().next())
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    Some(owner.unwrap_or_else(|| "?".into()))
}

/// Remove a plugin package: its `bin/` links (only those pointing into its
/// own directory) and `tools/<id>/`.
pub fn uninstall_package(id: &str, bins: &[String], dirs: &Dirs) -> Result<()> {
    for b in bins {
        let name = goatty_plugin::manifest::bin_name(b);
        if link_owner(dirs, name).as_deref() == Some(id) {
            std::fs::remove_file(dirs.bin_dir().join(name)).with_context(|| format!("remove bin/{name}"))?;
        }
    }
    let dir = dirs.tools_dir().join(id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    Ok(())
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
    fn installs_plugin_packages_without_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let archive = targz(&[("eza", b"#!/bin/sh\necho eza\n", 0o644)]);
        let sha = sha256_hex(&archive);
        let bins = vec!["eza".to_string()];
        let p = PackageInstall { id: "eza", version: "0.23.4", sha256: &sha, format: AssetFormat::TarGz, root: "", bins: &bins };
        let links = install_package(&p, &archive, &dirs).unwrap();
        assert_eq!(links, [dirs.bin_dir().join("eza")]);
        assert_eq!(std::fs::metadata(&links[0]).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(link_owner(&dirs, "eza").as_deref(), Some("eza"));

        // Another package can't take over bin/eza; a wrong checksum installs nothing.
        let other = PackageInstall { id: "other", ..p };
        assert!(install_package(&other, &archive, &dirs).unwrap_err().to_string().contains("already provided by eza"));
        let bad = PackageInstall { sha256: "00", ..p };
        assert!(install_package(&bad, &archive, &dirs).is_err());

        // A raw binary download.
        let raw = b"\x7fELF...";
        let sha_raw = sha256_hex(raw);
        let bins_raw = vec!["tool".to_string()];
        let r = PackageInstall { id: "tool", version: "1", sha256: &sha_raw, format: AssetFormat::Binary, root: "", bins: &bins_raw };
        install_package(&r, raw, &dirs).unwrap();
        assert_eq!(std::fs::read(dirs.bin_dir().join("tool")).unwrap(), raw);

        uninstall_package("eza", &bins, &dirs).unwrap();
        assert!(dirs.bin_dir().join("eza").symlink_metadata().is_err());
        assert!(!dirs.tools_dir().join("eza").exists());
        assert!(dirs.bin_dir().join("tool").exists(), "other packages untouched");
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
