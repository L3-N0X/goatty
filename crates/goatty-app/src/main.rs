//! goatty desktop client.
//!
//! The window is always a client of a goattyd (local or remote) unless
//! `--local` is given. See `goatty --help`, and `goatty_config` for the
//! `GOATTY_*` environment variables.

mod actions;
mod app;
mod cli;
mod command_palette;
mod commands;
mod session;
mod settings;
mod settings_window;
mod terminal;
mod ui;
mod workspace;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;

use goatty_client::Connection;
use goatty_config::OnLaunch;
use goatty_shell::HostService;
use goatty_term::TermSize;

use crate::cli::Cli;
use crate::session::Session;

const CLIENT: &str = concat!("goatty ", env!("CARGO_PKG_VERSION"));

fn main() -> Result<()> {
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
    let on_launch = goatty_config::load_client().value.workspace.on_launch;

    // Initial size is a guess; the terminal element resizes on first layout.
    let initial = TermSize::new(100, 30);
    let (session, link) = match cli.endpoint()? {
        None => {
            // No daemon: this process owns the shell and the host settings.
            let daemon = std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists());
            let host = Arc::new(HostService::from_env(daemon));
            (Session::local(initial, &host)?, settings_window::HostLink::Local(host))
        }
        Some(endpoint) => {
            let conn = Connection::open(endpoint, CLIENT)?;
            let session = Session::daemon(conn.clone(), initial, cli.attach_mode(on_launch == OnLaunch::New))?;
            (session, settings_window::HostLink::Daemon(conn))
        }
    };
    log::info!("backend: {}", session.backend_name());

    app::run(session, link, cli.settings.clone());
    Ok(())
}
