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
    Stop {
        /// Also stop a daemon this goattyd can't talk to (another protocol
        /// version), found through its socket.
        #[arg(long)]
        force: bool,
    },
    /// Run goattyd as a systemd user service (socket-activated).
    #[command(subcommand)]
    Service(ServiceCmd),
    /// goatty's managed zsh on this host: status, tool installs.
    #[command(subcommand)]
    Shell(ShellCmd),
    /// This host's settings (host.json: shell, prompt, history).
    #[command(subcommand)]
    Settings(SettingsCmd),
    /// Plugins on this host: search, install (with a review of what they do), configure, remove.
    #[command(subcommand)]
    Plugin(PluginCmd),
    /// Aliases in goatty's shells on this host: yours and the ones plugins bring.
    #[command(subcommand)]
    Alias(AliasCmd),
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Install and enable goattyd.socket + goattyd.service in ~/.config/systemd/user.
    Install,
    /// Disable and remove the units (a running daemon keeps its sessions).
    Uninstall,
    /// Whether the units are installed and running, and whether lingering is on.
    Status,
}

#[derive(Subcommand)]
enum AliasCmd {
    /// Your aliases by group, then each plugin's, with what they run.
    List,
    /// Add an alias: `goattyd alias add gs git status`. Quote a command with options, pipes or
    /// quotes (`'git log --oneline'`), or put it after `--`.
    Add {
        name: String,
        /// What typing NAME runs.
        #[arg(required = true)]
        command: Vec<String>,
        /// List it under this heading.
        #[arg(short, long)]
        group: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
        /// Add it switched off.
        #[arg(long)]
        disabled: bool,
        /// Replace an alias of yours with that name.
        #[arg(short, long)]
        force: bool,
    },
    /// Change one of your aliases.
    Edit {
        name: String,
        /// New command.
        #[arg(short, long, allow_hyphen_values = true)]
        command: Option<String>,
        /// Rename it.
        #[arg(short = 'n', long = "name", value_name = "NEW")]
        rename: Option<String>,
        /// Move it to another group ("" = none).
        #[arg(short, long)]
        group: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Remove aliases of yours.
    #[command(visible_alias = "rm")]
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Switch aliases on: yours or plugins' by name, a group of yours, or a plugin's.
    Enable {
        names: Vec<String>,
        #[arg(short, long, conflicts_with = "plugin")]
        group: Option<String>,
        /// That plugin's aliases (all of them without names).
        #[arg(short, long)]
        plugin: Option<String>,
    },
    /// Switch aliases off without removing them (plugin aliases too).
    Disable {
        names: Vec<String>,
        #[arg(short, long, conflicts_with = "plugin")]
        group: Option<String>,
        #[arg(short, long)]
        plugin: Option<String>,
    },
}

#[derive(Subcommand)]
enum PluginCmd {
    /// What the registries offer (QUERY matches names, summaries, keywords).
    Search {
        query: Option<String>,
        /// Fetch the registries first.
        #[arg(long)]
        refresh: bool,
    },
    /// Details, settings and permissions of a plugin.
    Info { id: String },
    /// Review and install plugins (ID or ID@VERSION), or one from a directory.
    Install {
        plugins: Vec<String>,
        /// Install the plugin in DIR (development; not verified by any registry).
        #[arg(long, value_name = "DIR")]
        path: Option<std::path::PathBuf>,
        /// Approve the plan without asking (it's still printed).
        #[arg(short, long)]
        yes: bool,
    },
    /// Install newer versions from the registries (default: all plugins).
    Update {
        plugins: Vec<String>,
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove plugins, their programs and their settings.
    Uninstall { plugins: Vec<String> },
    /// Installed plugins and their state.
    List,
    /// Load an installed plugin in new shells again.
    Enable { id: String },
    /// Keep a plugin installed but out of the shell.
    Disable { id: String },
    /// Change a plugin setting (checked against the plugin's settings).
    Set { id: String, key: String, value: String },
    /// Reset a plugin setting to its default.
    Unset { id: String, key: String },
    /// Plugin registries.
    #[command(subcommand)]
    Registry(RegistryCmd),
}

#[derive(Subcommand)]
enum RegistryCmd {
    /// The registries and their state.
    List {
        #[arg(long)]
        refresh: bool,
    },
    /// Fetch every registry now.
    Refresh,
    /// Trust another registry: URL of its .tar.gz (or an absolute directory) and its signing key.
    Add {
        name: String,
        url: String,
        /// Public key (hex) that signs its index (repeatable).
        #[arg(long = "key", required = true)]
        keys: Vec<String>,
    },
    /// Stop using an added registry (installed plugins stay).
    Remove { name: String },
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
    /// Install the tools of goatty's built-in plugins into goatty's data
    /// directory (no sudo). Without names: the default ones the shell is
    /// missing (what `goatty-setup` runs).
    Install {
        /// Built-in plugins: autosuggestions, syntax-highlighting, starship, atuin, fzf.
        tools: Vec<String>,
        /// Don't ask before downloading.
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove goatty's downloads of built-in plugins' tools (the plugins stay on; system copies still work).
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
    let default_level =
        if matches!(cmd, Cmd::Proxy | Cmd::Shell(_) | Cmd::Settings(_) | Cmd::Plugin(_) | Cmd::Alias(_)) { "warn" } else { "info" };
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
                    "{:>3}  pid {:<8} {}x{:<4} attached {}  running {:<10} cwd {}  {}",
                    s.id,
                    s.pid.map(|p| p.to_string()).unwrap_or_default(),
                    s.cols,
                    s.rows,
                    s.attached,
                    s.process.unwrap_or_else(|| "?".into()),
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
        Cmd::Stop { force } => stop(force),
        Cmd::Service(cmd) => match cmd {
            ServiceCmd::Install => service_install(),
            ServiceCmd::Uninstall => service_uninstall(),
            ServiceCmd::Status => service_status(),
        },
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
        Cmd::Plugin(cmd) => {
            use goatty_shell::plugins::cmd;
            let svc = goatty_shell::HostService::from_env(std::env::current_exe().ok());
            match cmd {
                PluginCmd::Search { query, refresh } => cmd::search(&svc, query.as_deref(), refresh),
                PluginCmd::Info { id } => cmd::info(&svc, &id),
                PluginCmd::Install { plugins, path, yes } => cmd::install(&svc, &plugins, path.as_deref(), yes),
                PluginCmd::Update { plugins, yes } => cmd::update(&svc, &plugins, yes),
                PluginCmd::Uninstall { plugins } => cmd::uninstall(&svc, &plugins),
                PluginCmd::List => cmd::list(&svc),
                PluginCmd::Enable { id } => cmd::set_enabled(&svc, &id, true),
                PluginCmd::Disable { id } => cmd::set_enabled(&svc, &id, false),
                PluginCmd::Set { id, key, value } => cmd::set(&svc, &id, &key, Some(&value)),
                PluginCmd::Unset { id, key } => cmd::set(&svc, &id, &key, None),
                PluginCmd::Registry(r) => match r {
                    RegistryCmd::List { refresh } => cmd::registries(&svc, refresh),
                    RegistryCmd::Refresh => cmd::registries(&svc, true),
                    RegistryCmd::Add { name, url, keys } => cmd::registry_add(&svc, &name, &url, &keys),
                    RegistryCmd::Remove { name } => cmd::registry_remove(&svc, &name),
                },
            }
        }
        Cmd::Alias(cmd) => {
            use goatty_shell::aliases::cmd;
            let svc = goatty_shell::HostService::from_env(std::env::current_exe().ok());
            match cmd {
                AliasCmd::List => cmd::list(&svc),
                AliasCmd::Add { name, command, group, description, disabled, force } => {
                    cmd::add(&svc, &name, &cmd::command_from(&command), group.as_deref(), description.as_deref(), !disabled, force)
                }
                AliasCmd::Edit { name, command, rename, group, description } => {
                    cmd::edit(&svc, &name, cmd::Edit { rename, command, group, description })
                }
                AliasCmd::Remove { names } => cmd::remove(&svc, &names),
                AliasCmd::Enable { names, group, plugin } => cmd::set_enabled(&svc, cmd::Selection { names, group, plugin }, true),
                AliasCmd::Disable { names, group, plugin } => cmd::set_enabled(&svc, cmd::Selection { names, group, plugin }, false),
            }
        }
        Cmd::Shell(cmd) => {
            use goatty_shell::cmd;
            let host = goatty_config::load_host();
            let launcher = goatty_shell::Launcher::from_env(std::env::current_exe().ok());
            let host_file = || goatty_config::host_file().ok_or_else(|| anyhow::anyhow!("$HOME is not set"));
            match cmd {
                ShellCmd::Status => cmd::status(&goatty_shell::HostService::from_env(std::env::current_exe().ok())),
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

/// `goattyd stop`: ask the daemon to shut down; with `force`, stop one that
/// doesn't speak our protocol too (old versions, after an upgrade).
fn stop(force: bool) -> Result<()> {
    let err = match Control::connect_local(CLIENT).and_then(|mut c| c.shutdown()) {
        Ok(()) => return Ok(()),
        Err(e) if force => e,
        Err(e) => return Err(e),
    };
    let path = goatty_proto::socket_path();
    // Nothing listening: nothing to stop (remote updates run this blindly).
    if std::os::unix::net::UnixStream::connect(&path).is_err() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        eprintln!("goattyd isn't running");
        return Ok(());
    }
    eprintln!("goattyd: {err:#}");
    if goatty_client::service::status().service_active {
        eprintln!("stopping goattyd.service");
        let ok = std::process::Command::new("systemctl").args(["--user", "stop", goatty_client::service::SERVICE_UNIT]).status()?.success();
        anyhow::ensure!(ok, "systemctl --user stop goattyd.service failed");
        return Ok(());
    }
    let pid = goatty_client::control::socket_owner(&path)?;
    // Under socket activation the listener belongs to systemd, never kill that.
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    anyhow::ensure!(comm.starts_with("goattyd"), "{} belongs to pid {pid} ({}), not a goattyd", path.display(), comm.trim());
    eprintln!("stopping pid {pid}");
    let alive = || std::path::Path::new(&format!("/proc/{pid}")).exists();
    // SAFETY: plain syscalls on a pid we just checked is our goattyd.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if alive() {
        // SAFETY: as above.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
    // The old daemon didn't get to remove its socket.
    if goatty_client::control::socket_owner(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

fn service_install() -> Result<()> {
    use goatty_client::service;
    let exe = std::env::current_exe()?;
    let done = service::install(&exe, &goatty_proto::socket_path())?;
    println!("installed {} and {} in {}", service::SOCKET_UNIT, service::SERVICE_UNIT, done.unit_dir.display());
    if done.started {
        println!("goattyd now starts through systemd on the first connection (logs: journalctl --user -u goattyd)");
    } else {
        println!(
            "a goattyd started outside systemd is still running; systemd takes over once it stops\n\
             (`goattyd stop` ends its sessions now, or it happens at your next login)"
        );
    }
    print_linger(done.linger);
    Ok(())
}

fn print_linger(linger: Option<bool>) {
    match linger {
        Some(true) => println!("lingering is on: goattyd and its sessions keep running after you log out"),
        Some(false) => println!(
            "lingering is off: systemd stops goattyd, and every session in it, when your last login session ends.\n\
             On a server, keep them with `loginctl enable-linger` (may ask for your password)."
        ),
        None => println!("couldn't check lingering (`loginctl show-user $USER -p Linger`)"),
    }
}

fn service_uninstall() -> Result<()> {
    if goatty_client::service::uninstall()? {
        println!("removed goattyd's systemd units; a running goattyd keeps its sessions until `goattyd stop`");
    } else {
        println!("goattyd's systemd units aren't installed");
    }
    Ok(())
}

fn service_status() -> Result<()> {
    let s = goatty_client::service::status();
    let dir = s.unit_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "?".into());
    if !s.installed {
        println!("not installed (units would go to {dir}); `goattyd service install` sets them up");
        return Ok(());
    }
    println!("installed in {dir}");
    if let Some(exec) = &s.exec_start {
        println!("runs      {exec}");
    }
    let state = |on: bool| if on { "active" } else { "inactive" };
    println!("socket    {}", state(s.socket_active));
    println!("service   {}", state(s.service_active));
    print_linger(s.linger);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn cli_is_consistent() {
        use clap::CommandFactory;
        super::Cli::command().debug_assert();
    }
}
