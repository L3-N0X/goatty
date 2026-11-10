//! Where goatty keeps its managed shell environment (XDG base directories).
//!
//! ```text
//! $XDG_DATA_HOME/goatty/          (~/.local/share/goatty)
//!   generations/<n>/              one built shell configuration each: zsh/ (the ZDOTDIR),
//!                                 host.json (the settings it came from), info.json
//!   current -> generations/<n>    the active one (switched atomically)
//!   tools/<tool>/<version>/       unpacked managed installs
//!   bin/<exe>                     symlinks to the active managed binaries
//!   plugins/<tool>                symlinks to the active managed zsh plugins
//!   installed/<plugin>/           installed goatty plugins (verified copies of their files)
//!   installed/lock.json           which plugins are installed, from where, what was approved
//!   registries/<name>/            verified snapshots of plugin registries
//! $XDG_CONFIG_HOME/goatty/zshrc   the user's own additions (goatty never writes it)
//! $XDG_CONFIG_HOME/goatty/registries.json   plugin registries added besides the official one
//! $XDG_CACHE_HOME/goatty/         completion dump, downloads
//! $XDG_STATE_HOME/goatty/         small flags (the dismissed setup hint, the GUI offered the setup)
//! ```

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
}

impl Dirs {
    /// From the process environment. `None` without `$HOME`.
    pub fn from_env() -> Option<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// From variables read through `var` (injectable for tests). Relative
    /// XDG paths are invalid per the spec and ignored.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let home = PathBuf::from(var("HOME").filter(|h| !h.is_empty())?);
        let xdg = |name: &str, fallback: &str| {
            var(name).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(fallback)).join("goatty")
        };
        Some(Self {
            data: xdg("XDG_DATA_HOME", ".local/share"),
            config: xdg("XDG_CONFIG_HOME", ".config"),
            cache: xdg("XDG_CACHE_HOME", ".cache"),
            state: xdg("XDG_STATE_HOME", ".local/state"),
        })
    }

    /// All four under one root (tests, sandboxes).
    pub fn under(root: &Path) -> Self {
        Self { data: root.join("data"), config: root.join("config"), cache: root.join("cache"), state: root.join("state") }
    }

    /// The active generation's ZDOTDIR (through the `current` link, so a
    /// path written into the config stays valid across generations).
    pub fn zsh_dir(&self) -> PathBuf {
        self.current_link().join("zsh")
    }
    pub fn generations_dir(&self) -> PathBuf {
        self.data.join("generations")
    }
    /// Symlink to the active generation.
    pub fn current_link(&self) -> PathBuf {
        self.data.join("current")
    }
    pub fn tools_dir(&self) -> PathBuf {
        self.data.join("tools")
    }
    pub fn bin_dir(&self) -> PathBuf {
        self.data.join("bin")
    }
    pub fn plugins_dir(&self) -> PathBuf {
        self.data.join("plugins")
    }
    /// Installed goatty plugins (Stage 5): `installed/<id>/` + `lock.json`.
    pub fn installed_dir(&self) -> PathBuf {
        self.data.join("installed")
    }
    /// Verified registry snapshots: `registries/<name>/`.
    pub fn registries_dir(&self) -> PathBuf {
        self.data.join("registries")
    }
    /// Registries the user added (the official one is built in).
    pub fn registries_file(&self) -> PathBuf {
        self.config.join("registries.json")
    }
    /// The user's own zsh additions, sourced late by the managed `.zshrc`.
    pub fn user_zshrc(&self) -> PathBuf {
        self.config.join("zshrc")
    }
    /// Exists once the user dismissed the "tools missing" hint.
    pub fn hint_marker(&self) -> PathBuf {
        self.state.join("shell-hint-dismissed")
    }
    /// Exists once a GUI offered to install the missing tools: the hint in
    /// new shells stays quiet (the GUI asks instead).
    pub fn offer_marker(&self) -> PathBuf {
        self.state.join("shell-setup-offered")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_defaults_and_overrides() {
        let d = Dirs::from_vars(|k| (k == "HOME").then(|| "/home/u".into())).unwrap();
        assert_eq!(d.data, Path::new("/home/u/.local/share/goatty"));
        assert_eq!(d.state, Path::new("/home/u/.local/state/goatty"));
        assert_eq!(d.zsh_dir(), Path::new("/home/u/.local/share/goatty/current/zsh"));
        assert_eq!(d.user_zshrc(), Path::new("/home/u/.config/goatty/zshrc"));

        let d = Dirs::from_vars(|k| match k {
            "HOME" => Some("/home/u".into()),
            "XDG_DATA_HOME" => Some("/data".into()),
            "XDG_CACHE_HOME" => Some("relative/ignored".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(d.data, Path::new("/data/goatty"));
        assert_eq!(d.cache, Path::new("/home/u/.cache/goatty"));
        assert!(Dirs::from_vars(|_| None).is_none());
    }
}
