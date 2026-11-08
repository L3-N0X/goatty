//! `goattyd`: run the goatty session daemon, or talk to a running one.

use anyhow::Result;
use clap::{Parser, Subcommand};
use goatty_client::Control;
use goatty_proto::SessionId;

#[derive(Parser)]
#[command(name = "goattyd", version, about = "goatty session daemon: owns shells so they outlive the window")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon in the foreground (default).
    Serve,
    /// Relay stdin/stdout to the daemon, starting it if needed (`ssh host goattyd proxy`).
    Proxy,
    /// List sessions.
    List,
    /// Kill a session.
    Kill { id: SessionId },
    /// Type text into a session. `\r`, `\n` and `\e` are unescaped (`\r` = Enter).
    Send { id: SessionId, text: String },
    /// Stop the daemon and all its sessions.
    Stop,
    /// goatty's managed zsh on this host: status, tool installs.
    #[command(subcommand)]
    Shell(ShellCmd),
    /// This host's settings (host.json: shell, prompt, history).
    #[command(subcommand)]
    Settings(SettingsCmd),
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Where host.json is.
    Path,
    /// Print what's stored, `--all` values, or one dotted KEY.
    Show {
        key: Option<String>,
        #[arg(long, conflicts_with = "key")]
        all: bool,
    },
    /// Check host.json (or FILE); exit status 1 on problems.
    Validate { file: Option<std::path::PathBuf> },
    /// Set a dotted KEY (e.g. history.size 5000); builds and activates the shell configuration.
    Set { key: String, value: String },
    /// Reset KEY to its default.
    Unset { key: String },
}

#[derive(Subcommand)]
enum ShellCmd {
    /// What the managed shell loads, what's missing, and its startup time.
    Status,
    /// Install tools into goatty's data directory (no sudo). Without names:
    /// the default tools the shell is missing.
    Install {
        /// Tools: autosuggestions, syntax-highlighting, starship, atuin, fzf.
        tools: Vec<String>,
        /// Don't ask before downloading.
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove goatty's copies of tools.
    Uninstall {
        tools: Vec<String>,
        #[arg(long, conflicts_with = "tools")]
        all: bool,
    },
    /// Stop new shells from mentioning missing tools.
    Dismiss,
    /// List the built shell configurations (generations).
    Generations,
    /// Show how the current settings would change the generated files.
    Diff,
    /// Activate an older generation (default: the previous one) and restore
    /// the settings it was built from.
    Rollback { generation: Option<u32> },
    /// Ask the daemon's running managed shells to restart with the active
    /// configuration (idle ones now, busy ones at their next prompt).
    Reload,
}

const CLIENT: &str = concat!("goattyd-cli ", env!("CARGO_PKG_VERSION"));

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cmd = cli.cmd.unwrap_or(Cmd::Serve);
    // The proxy's stderr goes back over ssh into the GUI's log: keep it quiet.
    let default_level = if matches!(cmd, Cmd::Proxy | Cmd::Shell(_) | Cmd::Settings(_)) { "warn" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level)).init();

    match cmd {
        Cmd::Serve => goatty_daemon::serve(&goatty_proto::socket_path()),
        Cmd::Proxy => goatty_daemon::proxy(),
        Cmd::List => {
            let list = Control::connect_local(CLIENT)?.list_sessions()?;
            if list.is_empty() {
                println!("no sessions");
            }
            for s in list {
                println!(
                    "{:>3}  pid {:<8} {}x{:<4} attached {}  cwd {}  {}",
                    s.id,
                    s.pid.map(|p| p.to_string()).unwrap_or_default(),
                    s.cols,
                    s.rows,
                    s.attached,
                    s.cwd.unwrap_or_default(),
                    s.title
                );
            }
            Ok(())
        }
        Cmd::Kill { id } => Control::connect_local(CLIENT)?.kill(id),
        Cmd::Send { id, text } => {
            let data = text.replace("\\r", "\r").replace("\\n", "\n").replace("\\e", "\x1b").into_bytes();
            Control::connect_local(CLIENT)?.send_input(id, data)
        }
        Cmd::Stop => Control::connect_local(CLIENT)?.shutdown(),
        Cmd::Settings(cmd) => {
            use goatty_shell::cmd;
            let svc = goatty_shell::HostService::from_env(std::env::current_exe().ok());
            match cmd {
                SettingsCmd::Path => cmd::settings_path(&svc),
                SettingsCmd::Show { key, all } => cmd::settings_show(&svc, key.as_deref(), all),
                SettingsCmd::Validate { file } => cmd::settings_validate(&svc, file.as_deref()),
                SettingsCmd::Set { key, value } => cmd::settings_set(&svc, &key, &value),
                SettingsCmd::Unset { key } => cmd::settings_unset(&svc, &key),
            }
        }
        Cmd::Shell(cmd) => {
            use goatty_shell::cmd;
            let host = goatty_config::load_host();
            let launcher = goatty_shell::Launcher::from_env(std::env::current_exe().ok());
            let host_file = || goatty_config::host_file().ok_or_else(|| anyhow::anyhow!("$HOME is not set"));
            match cmd {
                ShellCmd::Status => cmd::status(&host, &launcher),
                ShellCmd::Install { tools, yes } => cmd::install(&host, &launcher, &tools, yes),
                ShellCmd::Generations => cmd::generations(&launcher),
                ShellCmd::Diff => cmd::diff(&host, &launcher),
                ShellCmd::Rollback { generation } => cmd::rollback(&launcher, &host_file()?, generation),
                ShellCmd::Uninstall { tools, all } => cmd::uninstall(&launcher, &tools, all),
                ShellCmd::Dismiss => cmd::dismiss(&launcher),
                ShellCmd::Reload => {
                    let n = Control::connect_local(CLIENT)?.reload_shells()?;
                    println!("asked {n} running shell(s) to reload");
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cli_is_consistent() {
        use clap::CommandFactory;
        super::Cli::command().debug_assert();
    }
}
