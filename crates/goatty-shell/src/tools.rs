//! The tools the managed zsh uses, pinned to tested versions with sha256
//! checksums, and where to find them on this host.
//!
//! Tool ids match the feature names in the generated `.zshrc`
//! (`_goatty_features`), so `goatty-status` output maps straight to a tool.
//! The system copy always wins over a managed one: goatty only installs
//! what's missing. This table becomes the first built-in plugin manifests
//! in Stage 5.

use std::path::{Path, PathBuf};

use crate::dirs::Dirs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A program; `exe` is its file name (in the archive and in `bin/`).
    Binary { exe: &'static str },
    /// A zsh plugin; `entry` is the file the `.zshrc` sources.
    ZshPlugin { entry: &'static str },
}

/// One downloadable release archive (`.tar.gz`).
#[derive(Debug, Clone, Copy)]
pub struct Asset {
    /// `"<arch>-<os>"` as in [`current_target`], or `"any"`.
    pub target: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// Download size in bytes (shown in the install plan).
    pub size: u64,
    /// Directory inside the archive holding the payload (`""`: the top).
    pub root: &'static str,
}

#[derive(Debug)]
pub struct Tool {
    /// Also the feature name in the managed `.zshrc`.
    pub id: &'static str,
    /// Upstream name.
    pub name: &'static str,
    pub summary: &'static str,
    pub homepage: &'static str,
    pub version: &'static str,
    pub kind: Kind,
    /// Installed by a plain `goatty-setup`. Optional tools are integrated
    /// when present but only installed on request.
    pub default: bool,
    /// Where distributions and Homebrew put a zsh plugin, checked in order
    /// before the managed copy. Binaries are looked up in `PATH` instead.
    pub system_paths: &'static [&'static str],
    pub assets: &'static [Asset],
}

pub static TOOLS: &[Tool] = &[
    Tool {
        id: "autosuggestions",
        name: "zsh-autosuggestions",
        summary: "suggests commands as you type, from history and completion",
        homepage: "https://github.com/zsh-users/zsh-autosuggestions",
        version: "0.7.1",
        kind: Kind::ZshPlugin { entry: "zsh-autosuggestions.zsh" },
        default: true,
        system_paths: &[
            "/usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh", // Arch
            "/usr/share/zsh-autosuggestions/zsh-autosuggestions.zsh",             // Debian, Ubuntu, Fedora
            "/usr/local/share/zsh-autosuggestions/zsh-autosuggestions.zsh",       // Homebrew (Intel), FreeBSD
            "/opt/homebrew/share/zsh-autosuggestions/zsh-autosuggestions.zsh",    // Homebrew (Apple silicon)
            "/run/current-system/sw/share/zsh-autosuggestions/zsh-autosuggestions.zsh", // NixOS
        ],
        assets: &[Asset {
            target: "any",
            url: "https://github.com/zsh-users/zsh-autosuggestions/archive/refs/tags/v0.7.1.tar.gz",
            sha256: "0df7affff21cd87ed298e6a3970ed08a1dd66a6efa676454ee5b091ad503badf",
            size: 29_968,
            root: "zsh-autosuggestions-0.7.1",
        }],
    },
    Tool {
        id: "syntax-highlighting",
        name: "zsh-syntax-highlighting",
        summary: "colors the command line while you type",
        homepage: "https://github.com/zsh-users/zsh-syntax-highlighting",
        version: "0.8.0",
        kind: Kind::ZshPlugin { entry: "zsh-syntax-highlighting.zsh" },
        default: true,
        system_paths: &[
            "/usr/share/zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
            "/usr/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
            "/usr/local/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
            "/opt/homebrew/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
            "/run/current-system/sw/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh",
        ],
        assets: &[Asset {
            target: "any",
            url: "https://github.com/zsh-users/zsh-syntax-highlighting/archive/refs/tags/0.8.0.tar.gz",
            sha256: "5981c19ebaab027e356fe1ee5284f7a021b89d4405cc53dc84b476c3aee9cc32",
            size: 155_914,
            root: "zsh-syntax-highlighting-0.8.0",
        }],
    },
    Tool {
        id: "starship",
        name: "Starship",
        summary: "fast, informative prompt",
        homepage: "https://starship.rs",
        version: "1.27.0",
        kind: Kind::Binary { exe: "starship" },
        default: true,
        system_paths: &[],
        assets: &[
            Asset {
                target: "x86_64-linux",
                url: "https://github.com/starship/starship/releases/download/v1.27.0/starship-x86_64-unknown-linux-musl.tar.gz",
                sha256: "10c8eb4cb3b761d9bf59ea828444c2dab36c900b7d69a69e08814b286969df89",
                size: 5_215_016,
                root: "",
            },
            Asset {
                target: "aarch64-linux",
                url: "https://github.com/starship/starship/releases/download/v1.27.0/starship-aarch64-unknown-linux-musl.tar.gz",
                sha256: "341f5cbf24699fa0b27ec88965ad25f862ea09e15d35a543fd6ffdff1d2aa00d",
                size: 4_776_202,
                root: "",
            },
        ],
    },
    Tool {
        id: "atuin",
        name: "Atuin",
        summary: "searchable shell history on Ctrl+R",
        homepage: "https://atuin.sh",
        version: "18.24.2",
        kind: Kind::Binary { exe: "atuin" },
        default: true,
        system_paths: &[],
        assets: &[
            Asset {
                target: "x86_64-linux",
                url: "https://github.com/atuinsh/atuin/releases/download/v18.24.2/atuin-x86_64-unknown-linux-musl.tar.gz",
                sha256: "311cd1ba4b8be27de7071dbcae1c95ff4c2b38aeec82d557707b6b832bfceddf",
                size: 20_351_592,
                root: "atuin-x86_64-unknown-linux-musl",
            },
            Asset {
                target: "aarch64-linux",
                url: "https://github.com/atuinsh/atuin/releases/download/v18.24.2/atuin-aarch64-unknown-linux-musl.tar.gz",
                sha256: "9addf06ece26e68e842489cefa33e81860c288da373538986ea58c23eccb4f14",
                size: 19_129_327,
                root: "atuin-aarch64-unknown-linux-musl",
            },
        ],
    },
    Tool {
        id: "fzf",
        name: "fzf",
        summary: "fuzzy finder: Ctrl+T for files, Alt+C to cd",
        homepage: "https://github.com/junegunn/fzf",
        version: "0.74.5",
        kind: Kind::Binary { exe: "fzf" },
        default: false,
        system_paths: &[],
        assets: &[
            Asset {
                target: "x86_64-linux",
                url: "https://github.com/junegunn/fzf/releases/download/v0.74.5/fzf-0.74.5-linux_amd64.tar.gz",
                sha256: "d47d7f882972c7d674dbbba998ffa4116e68379616ffac7695d0671e9a722c5f",
                size: 2_291_079,
                root: "",
            },
            Asset {
                target: "aarch64-linux",
                url: "https://github.com/junegunn/fzf/releases/download/v0.74.5/fzf-0.74.5-linux_arm64.tar.gz",
                sha256: "cf058001da21724a469b714a29cf4aab9cbe860d218a0d6c1b5b661efad7784c",
                size: 2_078_357,
                root: "",
            },
        ],
    },
];

/// Look a tool up by id or upstream name (`starship`, `zsh-autosuggestions`).
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == name || t.name.eq_ignore_ascii_case(name))
}

/// `"<arch>-<os>"`, e.g. `x86_64-linux`.
pub fn current_target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

impl Tool {
    /// The archive for `target`, if this tool has one.
    pub fn asset_for(&self, target: &str) -> Option<&'static Asset> {
        self.assets.iter().find(|a| a.target == "any" || a.target == target)
    }

    /// Where the active managed copy is linked: `bin/<exe>` or
    /// `plugins/<id>/<entry>`.
    pub fn managed_path(&self, dirs: &Dirs) -> PathBuf {
        match self.kind {
            Kind::Binary { exe } => dirs.bin_dir().join(exe),
            Kind::ZshPlugin { entry } => dirs.plugins_dir().join(self.id).join(entry),
        }
    }

    /// The link that activates a managed install (`bin/<exe>` or `plugins/<id>`).
    pub(crate) fn link_path(&self, dirs: &Dirs) -> PathBuf {
        match self.kind {
            Kind::Binary { exe } => dirs.bin_dir().join(exe),
            Kind::ZshPlugin { .. } => dirs.plugins_dir().join(self.id),
        }
    }

    /// Plugin search order for the generated `.zshrc`: system paths, then
    /// the managed copy.
    pub fn plugin_candidates(&self, dirs: &Dirs) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.system_paths.iter().map(PathBuf::from).collect();
        v.push(self.managed_path(dirs));
        v
    }

    /// Where this tool comes from on this host, as seen with `path_var`
    /// (a `PATH` value). The managed shell may see a longer `PATH` (the
    /// user's `.zprofile` runs first); `goattyd shell status` asks a real
    /// shell for that view.
    pub fn locate(&self, dirs: &Dirs, path_var: Option<&str>) -> Location {
        let managed = self.managed_path(dirs);
        let system = match self.kind {
            Kind::Binary { exe } => which(exe, path_var, &dirs.bin_dir()),
            Kind::ZshPlugin { .. } => self.system_paths.iter().map(PathBuf::from).find(|p| p.is_file()),
        };
        match system {
            Some(p) => Location::System(p),
            None if managed.exists() => Location::Managed { path: managed.clone(), version: self.managed_version(dirs) },
            None => Location::Missing,
        }
    }

    /// Version of the active managed install (from the link target
    /// `tools/<id>/<version>/...`).
    pub fn managed_version(&self, dirs: &Dirs) -> Option<String> {
        let target = std::fs::read_link(self.link_path(dirs)).ok()?;
        let rel = target.strip_prefix(dirs.tools_dir().join(self.id)).ok()?;
        rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    System(PathBuf),
    Managed { path: PathBuf, version: Option<String> },
    Missing,
}

/// First executable `exe` in `path_var`, skipping goatty's own `bin/`.
fn which(exe: &str, path_var: Option<&str>, managed_bin: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path_var?)
        .filter(|d| d != managed_bin)
        .map(|d| d.join(exe))
        .find(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        for t in TOOLS {
            assert!(!t.assets.is_empty(), "{}", t.id);
            for a in t.assets {
                assert_eq!(a.sha256.len(), 64, "{} {}", t.id, a.target);
                assert!(a.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{}", t.id);
                assert!(a.url.starts_with("https://"), "{}", t.id);
                assert!(a.url.contains(t.version), "{}: url doesn't match version", t.id);
            }
            assert_eq!(find(t.id).map(|f| f.id), Some(t.id));
            assert_eq!(find(t.name).map(|f| f.id), Some(t.id));
        }
        assert!(find("zsh-autosuggestions").is_some());
        assert!(find("nope").is_none());
    }

    #[test]
    fn locate_prefers_system() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let sys = tmp.path().join("sysbin");
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(dirs.bin_dir()).unwrap();
        let starship = find("starship").unwrap();
        let path = std::env::join_paths([&sys, &dirs.bin_dir()]).unwrap();
        let path = path.to_str().unwrap();
        assert_eq!(starship.locate(&dirs, Some(path)), Location::Missing);

        // A managed copy (a dangling link counts as missing, so link a real file).
        let payload = dirs.tools_dir().join("starship/1.27.0/starship");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        write_exe(&payload);
        std::os::unix::fs::symlink(&payload, dirs.bin_dir().join("starship")).unwrap();
        assert_eq!(
            starship.locate(&dirs, Some(path)),
            Location::Managed { path: dirs.bin_dir().join("starship"), version: Some("1.27.0".into()) }
        );

        // A system copy wins.
        write_exe(&sys.join("starship"));
        assert_eq!(starship.locate(&dirs, Some(path)), Location::System(sys.join("starship")));
    }

    fn write_exe(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
