//! Where a plugin's package is on this host: a system copy, goatty's own
//! managed copy (`tools/<id>/<version>/`, activated by the `bin/<exe>` or
//! `plugins/<id>` link), or nowhere.
//!
//! A system copy wins when the package prefers it (the default): goatty
//! only installs what's missing.

use std::path::{Path, PathBuf};

use goatty_plugin::manifest::{Package, bin_name};

use crate::dirs::Dirs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    System(PathBuf),
    Managed { path: PathBuf, version: String },
    Missing,
}

/// goatty's active copy of `p`: (version, path of the first executable or
/// of the sourced script). `None` when it isn't installed or a link is
/// broken.
pub fn managed(dirs: &Dirs, p: &Package) -> Option<(String, PathBuf)> {
    let tool_dir = dirs.tools_dir().join(&p.id);
    let version_of = |link: &Path| -> Option<String> {
        let target = std::fs::read_link(link).ok()?;
        let rel = target.strip_prefix(&tool_dir).ok()?;
        rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned())
    };
    let mut found: Option<(String, PathBuf)> = None;
    for b in &p.bin {
        let link = dirs.bin_dir().join(bin_name(b));
        let v = version_of(&link).filter(|_| link.is_file())?;
        found.get_or_insert((v, link));
    }
    if let Some(src) = &p.source {
        let link = dirs.plugins_dir().join(&p.id);
        let v = version_of(&link).filter(|_| link.join(src).is_file())?;
        found = Some(found.unwrap_or((v, link.join(src))));
    }
    found
}

/// Where `p` comes from on this host, as seen with `path_var` (a `PATH`
/// value). The managed shell may see a longer `PATH` (the user's
/// `.zprofile` runs first); `goattyd shell status` asks a real shell.
pub fn locate(dirs: &Dirs, p: &Package, path_var: Option<&str>) -> Location {
    if p.prefer_system {
        let system = match &p.source {
            Some(_) => p.system_paths.iter().map(PathBuf::from).find(|f| f.is_file()),
            None => {
                let found: Vec<PathBuf> = p.bin.iter().filter_map(|b| which(bin_name(b), path_var, &dirs.bin_dir())).collect();
                (!found.is_empty() && found.len() == p.bin.len()).then(|| found[0].clone())
            }
        };
        if let Some(path) = system {
            return Location::System(path);
        }
    }
    match managed(dirs, p) {
        Some((version, path)) => Location::Managed { path, version },
        None => Location::Missing,
    }
}

/// First executable `exe` in `path_var`, skipping goatty's own `bin/`.
pub fn which(exe: &str, path_var: Option<&str>, managed_bin: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path_var?)
        .filter(|d| d != managed_bin)
        .map(|d| d.join(exe))
        .find(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins;

    fn write_exe(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn locate_prefers_system() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let sys = tmp.path().join("sysbin");
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(dirs.bin_dir()).unwrap();
        let starship = &builtins::get("starship").unwrap().manifest().packages[0];
        let path = std::env::join_paths([&sys, &dirs.bin_dir()]).unwrap();
        let path = path.to_str().unwrap();
        assert_eq!(locate(&dirs, starship, Some(path)), Location::Missing);

        // A managed copy, laid out the way the old tool installer (and
        // install_package) did it: a dangling link counts as missing.
        let payload = dirs.tools_dir().join("starship/1.27.0/starship");
        std::os::unix::fs::symlink(&payload, dirs.bin_dir().join("starship")).unwrap();
        assert_eq!(locate(&dirs, starship, Some(path)), Location::Missing);
        write_exe(&payload);
        assert_eq!(
            locate(&dirs, starship, Some(path)),
            Location::Managed { path: dirs.bin_dir().join("starship"), version: "1.27.0".into() }
        );

        // A system copy wins.
        write_exe(&sys.join("starship"));
        assert_eq!(locate(&dirs, starship, Some(path)), Location::System(sys.join("starship")));
    }

    #[test]
    fn finds_zsh_plugins_installed_before_plugins_existed() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let mut p = builtins::get("autosuggestions").unwrap().manifest().packages[0].clone();
        p.system_paths.clear(); // the host's distro copy mustn't count here
        assert_eq!(managed(&dirs, &p), None);
        // The old installer: tools/autosuggestions/0.7.1/<root>/, linked as plugins/autosuggestions.
        let root = dirs.tools_dir().join("autosuggestions/0.7.1/zsh-autosuggestions-0.7.1");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("zsh-autosuggestions.zsh"), "").unwrap();
        std::fs::create_dir_all(dirs.plugins_dir()).unwrap();
        std::os::unix::fs::symlink(&root, dirs.plugins_dir().join("autosuggestions")).unwrap();
        let link = dirs.plugins_dir().join("autosuggestions/zsh-autosuggestions.zsh");
        assert_eq!(managed(&dirs, &p), Some(("0.7.1".into(), link.clone())));
        assert_eq!(locate(&dirs, &p, None), Location::Managed { path: link, version: "0.7.1".into() });
    }
}
