//! Which shell a new session runs, and with what environment.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use goatty_config::{Effective, HostSettings, store};

use crate::dirs::Dirs;
use crate::generation::{self, Activated, FileDiff};
use crate::zsh::{self, GeneratedFile, ZshConfig};

/// A shell to spawn: what goattyd and `goatty --local` hand to the PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// Added to the inherited environment.
    pub env: Vec<(String, String)>,
    /// Runs goatty's managed zsh configuration.
    pub managed: bool,
}

/// Builds [`Launch`]es and the generations behind them.
#[derive(Debug, Clone)]
pub struct Launcher {
    dirs: Option<Dirs>,
    daemon_exe: Option<PathBuf>,
}

impl Launcher {
    /// `dirs`: where the managed environment lives (`None`: no `$HOME`, so
    /// no managed zsh). `daemon_exe`: the goattyd that `goatty-setup` runs.
    pub fn new(dirs: Option<Dirs>, daemon_exe: Option<PathBuf>) -> Self {
        Self { dirs, daemon_exe }
    }

    pub fn from_env(daemon_exe: Option<PathBuf>) -> Self {
        Self::new(Dirs::from_env(), daemon_exe)
    }

    pub fn dirs(&self) -> Option<&Dirs> {
        self.dirs.as_ref()
    }

    fn render(&self, dirs: &Dirs, settings: &HostSettings) -> Vec<GeneratedFile> {
        let (plugins, _) = crate::plugins::active(dirs, settings);
        zsh::render(&ZshConfig { settings, dirs, daemon_exe: self.daemon_exe.as_deref(), plugins: &plugins })
    }

    /// Build and activate the generation for `host` (a no-op when the
    /// active one matches). The snapshot stored with it is the file's
    /// settings, without environment overrides, so a rollback restores
    /// exactly what was saved.
    pub fn apply(&self, host: &Effective<HostSettings>) -> Result<Activated> {
        let dirs = self.dirs.as_ref().context("no $HOME")?;
        let files = self.render(dirs, &host.value);
        generation::activate(dirs, &files, &store::to_json(&host.file.value, &host.file.unknown))
    }

    /// How the generated files would change if `settings` were applied.
    pub fn preview(&self, settings: &HostSettings) -> Vec<FileDiff> {
        match &self.dirs {
            Some(dirs) => generation::diff(dirs, &self.render(dirs, settings)),
            None => Vec::new(),
        }
    }

    /// The login shell for a new session. zsh gets goatty's managed
    /// configuration when `shell.managed` is on: the generation for these
    /// settings is built if needed. If that fails (say, validation), the
    /// last good generation is used, and with none, plain zsh. Other shells
    /// start as they are.
    pub fn launch(&self, host: &Effective<HostSettings>) -> Launch {
        let shell = &host.value.shell;
        let program = resolve_program(shell.program.as_deref());
        let mut launch = Launch { program, args: vec!["-l".into()], env: Vec::new(), managed: false };
        if !(shell.managed && is_zsh(&launch.program)) {
            return launch;
        }
        let Some(dirs) = &self.dirs else {
            log::warn!("no $HOME: starting plain zsh");
            return launch;
        };
        if let Err(e) = self.apply(host) {
            log::warn!("couldn't build the shell configuration: {e:#}");
        }
        if !dirs.zsh_dir().join(".zshrc").is_file() {
            log::warn!("no usable shell configuration generation: starting plain zsh");
            return launch;
        }
        let zdotdir = dirs.zsh_dir();
        launch.env.push(("ZDOTDIR".to_owned(), zdotdir.to_string_lossy().into_owned()));
        // The user's own ZDOTDIR, for .zshenv to put back. Ignore our own
        // (a daemon started from inside a goatty shell mid-startup).
        if let Some(orig) = std::env::var_os("ZDOTDIR").filter(|o| Path::new(o) != zdotdir) {
            launch.env.push(("GOATTY_ORIG_ZDOTDIR".to_owned(), orig.to_string_lossy().into_owned()));
        }
        launch.managed = true;
        launch
    }
}

/// `program` if given, else zsh if installed, else `$SHELL`, else /bin/sh.
pub fn resolve_program(program: Option<&str>) -> String {
    program
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .or_else(|| which("zsh"))
        .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "/bin/sh".into())
}

pub fn is_zsh(program: &str) -> bool {
    Path::new(program).file_name().is_some_and(|n| n == "zsh")
}

/// First `bin` in `$PATH`.
pub fn which(bin: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(program: &str) -> HostSettings {
        let mut h = HostSettings::default();
        h.shell.program = Some(program.into());
        h
    }

    #[test]
    fn zsh_detection() {
        assert!(is_zsh("/usr/bin/zsh"));
        assert!(is_zsh("zsh"));
        assert!(!is_zsh("/bin/zsh5"));
        assert!(!is_zsh("/bin/bash"));
    }

    #[test]
    fn non_zsh_starts_plain() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Launcher::new(Some(Dirs::under(tmp.path())), None);
        let launch = l.launch(&Effective::plain(host("/bin/sh")));
        assert_eq!(launch, Launch { program: "/bin/sh".into(), args: vec!["-l".into()], env: vec![], managed: false });
        assert!(!tmp.path().join("data").exists(), "nothing written for other shells");
    }

    #[test]
    fn managed_zsh_uses_the_current_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let l = Launcher::new(Some(dirs.clone()), None);
        let mut h = Effective::plain(host("/usr/bin/zsh"));
        let launch = l.launch(&h);
        assert!(launch.managed);
        assert_eq!(launch.env[0], ("ZDOTDIR".into(), dirs.zsh_dir().to_string_lossy().into_owned()));
        assert!(dirs.zsh_dir().join(".zshrc").is_file());
        assert_eq!(generation::current(&dirs), Some(1));

        // Same settings: same generation. Changed: preview shows it, the next launch builds it.
        l.launch(&h);
        assert_eq!(generation::current(&dirs), Some(1));
        h.value.history.size = 777;
        h.file.value.history.size = 777;
        let diff = l.preview(&h.value);
        assert_eq!(diff.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["settings.zsh"]);
        l.launch(&h);
        assert_eq!(generation::current(&dirs), Some(2));
        let snapshot = generation::list(&dirs).pop().unwrap().settings_json().unwrap();
        assert!(snapshot.contains("777"), "{snapshot}");

        let mut off = h.clone();
        off.value.shell.managed = false;
        assert!(!l.launch(&off).managed);
        assert!(!Launcher::new(None, None).launch(&h).managed, "no HOME: plain zsh");
    }
}
