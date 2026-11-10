//! Command line of the `goatty` binary.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use goatty_client::Endpoint;

use crate::session::AttachMode;

#[derive(Debug, Parser)]
#[command(name = "goatty", version, about = "A fast, batteries-included terminal")]
pub struct Cli {
    /// Always create a new session (default: restore the last closed window, else reattach the newest detached session).
    #[arg(long, conflicts_with_all = ["attach", "local"])]
    pub new: bool,

    /// Attach to a specific session (see `goattyd list`).
    #[arg(long, value_name = "ID", conflicts_with = "local")]
    pub attach: Option<u64>,

    /// No daemon: this window owns the shell, which ends when it closes.
    #[arg(long, conflicts_with_all = ["ssh", "via"])]
    pub local: bool,

    /// Use goattyd on HOST, via `ssh HOST goattyd proxy`.
    #[arg(long, value_name = "HOST", conflicts_with = "via")]
    pub ssh: Option<String>,

    /// Path of goattyd on the SSH host, if it isn't in PATH.
    #[arg(long, value_name = "PATH", requires = "ssh", conflicts_with = "plain")]
    pub remote_daemon: Option<String>,

    /// With --ssh: a plain `ssh HOST` in a local session instead of goattyd on HOST.
    /// Nothing is installed there, and goatty's shell setup doesn't reach it.
    #[arg(long, requires = "ssh", conflicts_with_all = ["new", "attach"])]
    pub plain: bool,

    /// Use goattyd behind any stdio command, e.g. 'docker exec -i box goattyd proxy'.
    /// It runs under `sh -c`, so quote arguments as in a shell.
    #[arg(long, value_name = "CMD")]
    pub via: Option<String>,

    /// Also open the settings window, optionally at a page (appearance,
    /// shell, prompt, history, aliases, plugins, shortcuts, workspace, profile).
    #[arg(long, value_name = "PAGE", num_args = 0..=1, default_missing_value = "appearance")]
    pub settings: Option<String>,

    #[command(subcommand)]
    pub cmd: Option<Command>,
}

/// Work without a window.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect and change settings, export and import profiles.
    #[command(subcommand)]
    Settings(SettingsCmd),
    /// List, import, export and pick color themes.
    #[command(subcommand)]
    Theme(ThemeCmd),
}

#[derive(Debug, Subcommand)]
pub enum SettingsCmd {
    /// Where the settings files are.
    Path,
    /// Print this computer's settings: what's stored, `--all` values, or one KEY.
    Show {
        key: Option<String>,
        #[arg(long, conflicts_with = "key")]
        all: bool,
    },
    /// Check settings.json (or FILE) and the themes; exit status 1 on problems.
    Validate { file: Option<PathBuf> },
    /// Set a dotted KEY (e.g. font.size 15, theme.name nord). The running app picks it up.
    Set { key: String, value: String },
    /// Reset KEY to its default.
    Unset { key: String },
    /// Write a profile (these settings + the host's shell settings) to FILE, `-` for stdout.
    Export { file: PathBuf },
    /// Apply a profile from FILE (`-` for stdin). Host settings go to the local goattyd.
    Import {
        file: PathBuf,
        /// Don't ask.
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThemeFor {
    Light,
    Dark,
}

#[derive(Debug, Subcommand)]
pub enum ThemeCmd {
    /// Built-in and your own themes.
    List,
    /// Import color schemes (base16 YAML/JSON, iTerm2 .itermcolors, Windows Terminal, goatty).
    Import {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Name for the imported theme (one scheme only).
        #[arg(long)]
        name: Option<String>,
    },
    /// Write a theme as a goatty theme file (stdout without FILE).
    Export { id: String, file: Option<PathBuf> },
    /// Use theme ID; `--for light|dark` sets that side and follows the system.
    Set {
        id: String,
        #[arg(long = "for", value_enum)]
        target: Option<ThemeFor>,
    },
    /// Delete one of your themes.
    Delete { id: String },
}

impl Cli {
    /// `prefer_new`: the workspace setting asks for a new session by default.
    pub fn attach_mode(&self, prefer_new: bool) -> AttachMode {
        match (self.new, self.attach) {
            (true, _) => AttachMode::New,
            (false, Some(id)) => AttachMode::Id(id),
            (false, None) if prefer_new => AttachMode::New,
            (false, None) => AttachMode::Auto,
        }
    }

    /// The daemon to use, or `None` for `--local`.
    pub fn endpoint(&self) -> Result<Option<Endpoint>> {
        Ok(match (&self.ssh, &self.via) {
            _ if self.local => None,
            (Some(host), _) => Some(Endpoint::ssh(host, self.remote_daemon.as_deref())),
            (None, Some(cmd)) => Some(Endpoint::command_line(cmd)?),
            (None, None) => Some(Endpoint::Local),
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn modes() {
        let cli = Cli::parse_from(["goatty", "--attach", "3"]);
        assert!(matches!(cli.attach_mode(true), AttachMode::Id(3)));
        assert!(matches!(Cli::parse_from(["goatty"]).attach_mode(true), AttachMode::New));
        assert!(matches!(Cli::parse_from(["goatty"]).attach_mode(false), AttachMode::Auto));
        assert!(matches!(cli.endpoint().unwrap(), Some(Endpoint::Local)));
        assert!(Cli::parse_from(["goatty", "--local"]).endpoint().unwrap().is_none());
        assert!(Cli::try_parse_from(["goatty", "--local", "--ssh", "h"]).is_err());
        assert!(Cli::try_parse_from(["goatty", "--remote-daemon", "/x"]).is_err());
        assert!(Cli::parse_from(["goatty", "--ssh", "box", "--plain"]).plain);
        assert!(Cli::try_parse_from(["goatty", "--plain"]).is_err());
        assert!(matches!(Cli::parse_from(["goatty", "--ssh", "box"]).endpoint().unwrap(), Some(Endpoint::Ssh(t)) if t.host == "box"));
        let cli = Cli::parse_from(["goatty", "settings", "set", "font.size", "15"]);
        assert!(matches!(cli.cmd, Some(Command::Settings(SettingsCmd::Set { .. }))));
        let cli = Cli::parse_from(["goatty", "theme", "set", "nord", "--for", "dark"]);
        assert!(matches!(cli.cmd, Some(Command::Theme(ThemeCmd::Set { target: Some(ThemeFor::Dark), .. }))));
    }
}
