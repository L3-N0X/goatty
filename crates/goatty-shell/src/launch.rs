//! Which shell a new session runs, and with what environment.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use goatty_config::ShellSettings;

use crate::dirs::Dirs;
use crate::zsh::{self, ZshConfig};

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

/// Builds [`Launch`]es. Holds what doesn't change between sessions.
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

    /// The login shell for a new session. zsh gets goatty's managed
    /// configuration when `settings.managed` is on; if that can't be
    /// written, it starts plain (with a warning in the log) rather than not
    /// at all. Other shells start as they are.
    pub fn launch(&self, settings: &ShellSettings) -> Launch {
        let program = resolve_program(settings.program.as_deref());
        let mut launch = Launch { program, args: vec!["-l".into()], env: Vec::new(), managed: false };
        if settings.managed && is_zsh(&launch.program) {
            match self.prepare_zsh(settings) {
                Ok(env) => {
                    launch.env = env;
                    launch.managed = true;
                }
                Err(e) => log::warn!("managed zsh unavailable, starting plain zsh: {e:#}"),
            }
        }
        launch
    }

    /// Write the managed ZDOTDIR and return the environment that selects it.
    fn prepare_zsh(&self, settings: &ShellSettings) -> Result<Vec<(String, String)>> {
        let dirs = self.dirs.as_ref().context("no $HOME")?;
        let files = zsh::render(&ZshConfig { settings: &settings.zsh, dirs, daemon_exe: self.daemon_exe.as_deref() });
        zsh::write(dirs, &files)?;
        let zdotdir = dirs.zsh_dir();
        let mut env = vec![("ZDOTDIR".to_owned(), zdotdir.to_string_lossy().into_owned())];
        // The user's own ZDOTDIR, for .zshenv to put back. Ignore our own
        // (a daemon started from inside a goatty shell mid-startup).
        if let Some(orig) = std::env::var_os("ZDOTDIR").filter(|o| Path::new(o) != zdotdir) {
            env.push(("GOATTY_ORIG_ZDOTDIR".to_owned(), orig.to_string_lossy().into_owned()));
        }
        Ok(env)
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
        let s = ShellSettings { program: Some("/bin/sh".into()), ..Default::default() };
        let launch = l.launch(&s);
        assert_eq!(launch, Launch { program: "/bin/sh".into(), args: vec!["-l".into()], env: vec![], managed: false });
        assert!(!tmp.path().join("data").exists(), "nothing written for other shells");
    }

    #[test]
    fn managed_zsh_sets_zdotdir() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let l = Launcher::new(Some(dirs.clone()), None);
        let s = ShellSettings { program: Some("/usr/bin/zsh".into()), ..Default::default() };
        let launch = l.launch(&s);
        assert!(launch.managed);
        assert_eq!(launch.env[0], ("ZDOTDIR".into(), dirs.zsh_dir().to_string_lossy().into_owned()));
        assert!(dirs.zsh_dir().join(".zshrc").is_file());

        let off = ShellSettings { managed: false, ..s.clone() };
        assert!(!l.launch(&off).managed);
        assert!(!Launcher::new(None, None).launch(&s).managed, "no HOME: plain zsh");
    }
}
