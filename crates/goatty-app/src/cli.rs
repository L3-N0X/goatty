//! Command line of the `goatty` binary.

use anyhow::Result;
use clap::Parser;
use goatty_client::Endpoint;

use crate::session::AttachMode;

#[derive(Debug, Parser)]
#[command(name = "goatty", version, about = "A fast, batteries-included terminal")]
pub struct Cli {
    /// Always create a new session (default: reattach the newest detached one).
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
    #[arg(long, value_name = "PATH", requires = "ssh")]
    pub remote_daemon: Option<String>,

    /// Use goattyd behind any stdio command, e.g. 'docker exec -i box goattyd proxy'.
    #[arg(long, value_name = "CMD")]
    pub via: Option<String>,

    /// Also open the settings window, optionally at a page (appearance,
    /// shell, prompt, history, shortcuts, workspace, profile).
    #[arg(long, value_name = "PAGE", num_args = 0..=1, default_missing_value = "appearance")]
    pub settings: Option<String>,
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
    }
}
