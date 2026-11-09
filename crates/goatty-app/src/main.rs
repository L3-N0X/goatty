//! goatty desktop client.
//!
//! The window is always a client of a goattyd (local or remote) unless
//! `--local` is given. See `goatty --help`, and `goatty_config` for the
//! `GOATTY_*` environment variables.

mod actions;
mod app;
mod askpass;
mod cli;
mod command_palette;
mod commands;
mod connect;
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

use goatty_client::{Connection, Endpoint};
use goatty_config::OnLaunch;
use goatty_shell::HostService;
use goatty_term::TermSize;

use crate::app::Start;
use crate::cli::Cli;
use crate::connect::ConnectPlan;
use crate::session::Session;

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
    let on_launch = goatty_config::load_client().value.workspace.on_launch;

    // Initial size is a guess; the terminal element resizes on first layout.
    let initial = TermSize::new(100, 30);
    let mode = cli.attach_mode(on_launch == OnLaunch::New);
    let open_settings = cli.settings.clone();
    let (session, link) = match cli.endpoint()? {
        None => {
            // No daemon: this process owns the shell and the host settings.
            let daemon = std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists());
            let host = Arc::new(HostService::from_env(daemon));
            (Session::local(initial, &host)?, settings_window::HostLink::Local(host))
        }
        Some(Endpoint::Local) => {
            let conn = Connection::open(Endpoint::Local, CLIENT)?;
            let session = Session::daemon(conn.clone(), initial, mode)?;
            (session, settings_window::HostLink::Daemon(conn))
        }
        Some(Endpoint::Ssh(target)) if cli.plain => connect::plain_ssh(&target.host)?,
        Some(mut endpoint) => {
            // Remote: connect in a window that can ask about installing
            // goattyd, with ssh's prompts in dialogs of their own.
            if let Endpoint::Ssh(target) = &mut endpoint {
                target.askpass = std::env::current_exe().ok();
            }
            app::run(Start::Connect(ConnectPlan { endpoint, mode, open_settings }));
            return Ok(());
        }
    };
    log::info!("backend: {}", session.backend_name());

    app::run(Start::Ready { session, link, open_settings });
    Ok(())
}
