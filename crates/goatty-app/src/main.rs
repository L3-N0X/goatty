//! goatty desktop client.
//!
//! The window is always a client of a goattyd (local or remote) unless
//! `--local` is given. See `goatty --help`, and `goatty_config` for the
//! `GOATTY_*` environment variables.

mod actions;
mod app;
mod askpass;
mod cli;
mod close_confirm;
mod command_palette;
mod commands;
mod connect;
mod hosts;
mod paste_confirm;
mod session;
mod settings;
mod settings_window;
mod shell_setup;
mod terminal;
mod toast;
mod ui;
mod welcome;
mod workspace;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;

use goatty_client::Endpoint;
use goatty_config::OnLaunch;
use goatty_shell::HostService;
use goatty_term::TermSize;

use crate::app::Initial;
use crate::cli::Cli;
use crate::connect::ConnectPlan;
use crate::session::{Session, Spawner};
use crate::workspace::saved::Startup;

pub const CLIENT: &str = concat!("goatty ", env!("CARGO_PKG_VERSION"));

fn main() -> Result<()> {
    // Started by ssh to ask a question (we set SSH_ASKPASS to ourselves).
    if let Some(req) = goatty_client::askpass::request_from_env() {
        askpass::run(req);
    }
    let mut cli = Cli::parse();
    // Subcommands work without a window (and without a display).
    match cli.cmd.take() {
        Some(cli::Command::Settings(c)) => return commands::settings(c),
        Some(cli::Command::Theme(c)) => return commands::theme(c),
        None => {}
    }
    // goatty's own crates at info; dependencies (wgpu, zbus, ...) only warn.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,goatty=info,goatty_client=info,goatty_term=info"))
        .init();
    let client = goatty_config::load_client().value;
    let on_launch = client.workspace.on_launch;

    let mode = cli.attach_mode(on_launch == OnLaunch::New);
    let open_settings = cli.settings.clone();
    let initial = match cli.endpoint()? {
        None => {
            // No daemon: this process owns the shell and the host settings.
            // Initial size is a guess; the terminal resizes on first layout.
            let daemon = std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists());
            let host = Arc::new(HostService::from_env(daemon));
            log::info!("backend: local pty");
            let session = Session::local(TermSize::new(100, 30), &host, None)?;
            Initial::Ready { startup: Startup::Session(session), spawner: Spawner::Local(host) }
        }
        // Connecting (and creating or attaching the session) happens in the
        // window, which opens right away (#37). `--plain`, or the host set to
        // plain SSH (Settings → Hosts), runs `ssh host` in a local session.
        Some(mut endpoint) => {
            let plain = match &mut endpoint {
                Endpoint::Ssh(target) => {
                    // ssh's prompts come up in goatty's dialogs.
                    target.askpass = std::env::current_exe().ok();
                    cli.plain || client.ssh.plain(&target.host)
                }
                _ => false,
            };
            log::info!("backend: goattyd via {}", endpoint.label());
            Initial::Connect(ConnectPlan { endpoint, mode, open_settings: None, plain, conn: None })
        }
    };
    app::run(initial, open_settings);
    Ok(())
}
