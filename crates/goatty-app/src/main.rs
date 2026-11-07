//! goatty desktop client — Stage 0 prototype.
//!
//!   goatty                 attach to newest detached goattyd session, or create one
//!   goatty --new           always create a new daemon session
//!   goatty --attach <id>   attach to a specific session
//!   goatty --local         no daemon: PTY owned by this process
//!   goatty --ssh <host>    use goattyd on <host> (via `ssh <host> goattyd proxy`)
//!   goatty --via '<cmd>'   use goattyd behind any stdio command, e.g.
//!                          'docker exec -i box goattyd proxy'
//!   --remote-daemon <path> goattyd path on the SSH host if not in PATH
//!
//! Env: GOATTY_FONT, GOATTY_FONT_SIZE, GOATTY_SHELL, GOATTY_STATS=1,
//!      GOATTY_RENDER=shaped (line shaping w/ ligatures instead of glyph cache)

mod element;
mod glyphs;
mod session;
mod view;

use anyhow::{Context as _, Result};
use goatty_term::TermSize;
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use goatty_client::{Connection, Endpoint};

use crate::session::{AttachMode, Session, Waker};
use crate::view::TerminalView;

/// Preferred monospace families, first installed one wins.
const FONT_CANDIDATES: &[&str] = &[
    "JetBrainsMono Nerd Font",
    "JetBrains Mono",
    "FiraCode Nerd Font Mono",
    "FiraCode Nerd Font",
    "CommitMono Nerd Font Mono",
    "Fira Code",
    "DejaVu Sans Mono",
    "Adwaita Mono",
    "Noto Sans Mono",
    "Liberation Mono",
];

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut local = false;
    let mut mode = AttachMode::Auto;
    let mut ssh_host: Option<String> = None;
    let mut via: Option<String> = None;
    let mut remote_daemon: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--local" => local = true,
            "--new" => mode = AttachMode::New,
            "--attach" => mode = AttachMode::Id(it.next().context("--attach <id>")?.parse()?),
            "--ssh" => ssh_host = Some(it.next().context("--ssh <host>")?.clone()),
            "--via" => via = Some(it.next().context("--via '<command>'")?.clone()),
            "--remote-daemon" => remote_daemon = Some(it.next().context("--remote-daemon <path>")?.clone()),
            "-h" | "--help" => {
                println!("goatty [--local | --new | --attach <id>] [--ssh <host> [--remote-daemon <path>] | --via '<cmd>']");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let endpoint = match (ssh_host, via) {
        (Some(h), _) => Endpoint::ssh(&h, remote_daemon.as_deref()),
        (None, Some(cmd)) => Endpoint::command_line(&cmd)?,
        (None, None) => Endpoint::Local,
    };

    // Initial size is a guess; the element resizes on first layout.
    let initial = TermSize::new(100, 30);
    let (waker, wake_rx) = Waker::new();
    let session = if local {
        Session::local(initial, waker.clone())?
    } else {
        let conn = Connection::open(endpoint, concat!("goatty ", env!("CARGO_PKG_VERSION")))?;
        Session::daemon(conn, initial, mode, waker.clone())?
    };
    log::info!("backend: {}", session.backend_name());

    let font_size: f32 = std::env::var("GOATTY_FONT_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(14.0);

    gpui_platform::application().run(move |cx: &mut App| {
        let installed = cx.text_system().all_font_names();
        let family = std::env::var("GOATTY_FONT")
            .ok()
            .or_else(|| FONT_CANDIDATES.iter().find(|f| installed.iter().any(|i| i == *f)).map(|s| s.to_string()))
            .unwrap_or_else(|| "monospace".into());
        log::info!("font: {family} {font_size}px");
        let font = gpui::font(family);

        let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("dev.goatty.goatty".into()),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title("goatty");
                cx.new(|cx| TerminalView::new(session, waker, wake_rx, font, px(font_size), window, cx))
            },
        )
        .expect("open window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
    Ok(())
}
