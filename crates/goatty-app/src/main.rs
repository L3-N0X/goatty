//! goatty desktop client.
//!
//! The window is always a client of a goattyd (local or remote) unless
//! `--local` is given. See `goatty --help`, and `goatty_config` for the
//! `GOATTY_*` environment variables.

mod actions;
mod app;
mod cli;
mod close_confirm;
mod command_palette;
mod commands;
mod paste_confirm;
mod session;
mod settings;
mod settings_window;
mod terminal;
mod toast;
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
use crate::session::{AttachMode, Session};
use crate::workspace::saved::{self, Startup};

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
    let (startup, link) = match cli.endpoint()? {
        None => {
            // No daemon: this process owns the shell and the host settings.
            let daemon = std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists());
            let host = Arc::new(HostService::from_env(daemon));
            log::info!("backend: local pty");
            (Startup::Session(Session::local(initial, &host, None)?), settings_window::HostLink::Local(host))
        }
        Some(endpoint) => {
            let conn = Connection::open(endpoint, CLIENT)?;
            log::info!("backend: goattyd via {}", conn.endpoint().label());
            let mode = cli.attach_mode(on_launch == OnLaunch::New);
            // Reattaching brings back the last window's tabs and splits;
            // without a saved layout, the newest detached session.
            let restored = match mode {
                AttachMode::Auto => saved::restore(&conn, initial).unwrap_or_else(|e| {
                    log::warn!("restoring the last window: {e:#}");
                    None
                }),
                _ => None,
            };
            let startup = match restored {
                Some(r) => Startup::Restore(r),
                None => Startup::Session(Session::daemon(conn.clone(), initial, mode, None)?),
            };
            (startup, settings_window::HostLink::Daemon(conn))
        }
    };

    let spawner = match &link {
        settings_window::HostLink::Local(host) => session::Spawner::Local(host.clone()),
        settings_window::HostLink::Daemon(conn) => session::Spawner::Daemon(conn.clone()),
    };
    app::run(startup, spawner, link, cli.settings.clone());
    Ok(())
}
