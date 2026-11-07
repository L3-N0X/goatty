//! goatty desktop client.
//!
//! The window is always a client of a goattyd (local or remote) unless
//! `--local` is given. See `goatty --help`, and `goatty_config` for the
//! `GOATTY_*` environment variables.

mod actions;
mod app;
mod cli;
mod session;
mod terminal;
mod workspace;

use anyhow::Result;
use clap::Parser;
use goatty_client::Connection;
use goatty_config::Settings;
use goatty_term::TermSize;

use crate::cli::Cli;
use crate::session::Session;

const CLIENT: &str = concat!("goatty ", env!("CARGO_PKG_VERSION"));

fn main() -> Result<()> {
    let cli = Cli::parse();
    // goatty's own crates at info; dependencies (wgpu, zbus, ...) only warn.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,goatty=info,goatty_client=info,goatty_term=info"))
        .init();
    let settings = Settings::from_env();

    // Initial size is a guess; the terminal element resizes on first layout.
    let initial = TermSize::new(100, 30);
    let session = match cli.endpoint()? {
        None => Session::local(initial, settings.shell.program.as_deref())?,
        Some(endpoint) => Session::daemon(Connection::open(endpoint, CLIENT)?, initial, cli.attach_mode())?,
    };
    log::info!("backend: {}", session.backend_name());

    app::run(settings, session);
    Ok(())
}
