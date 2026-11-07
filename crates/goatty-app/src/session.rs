//! Where a pane's bytes come from: an in-process PTY (`--local`) or a
//! goattyd session over a [`Connection`], local or remote. Both feed a
//! client-side [`Terminal`] that the renderer reads; the rest of the UI
//! doesn't care which backend is in use.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use goatty_client::{Connection, PaneEvent, PaneSink};
use goatty_proto::WinSize;
use goatty_shell::HostService;
use goatty_term::pty::{Pty, ShellCommand};
use goatty_term::{Palette, TermSize, Terminal, UiEvent};
use parking_lot::Mutex;

/// Coalescing wake-up: many output chunks → one UI notification per frame.
struct Waker {
    pending: AtomicBool,
    tx: flume::Sender<()>,
}

impl Waker {
    fn new() -> (Arc<Self>, flume::Receiver<()>) {
        let (tx, rx) = flume::unbounded();
        (Arc::new(Self { pending: AtomicBool::new(false), tx }), rx)
    }
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.tx.send(());
        }
    }
    fn clear(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AttachMode {
    /// Reattach to the newest detached session, else create one.
    Auto,
    New,
    Id(u64),
}

/// Link status shown in the title bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkStatus {
    Connected,
    Reconnecting,
    Lost(String),
}

enum Backend {
    Local(Mutex<Pty>),
    Daemon { conn: Arc<Connection>, id: AtomicU64 },
}

pub struct Session {
    pub terminal: Terminal,
    /// Fires (coalesced) when the terminal changed or the link state did.
    pub wake_rx: flume::Receiver<()>,
    backend: Backend,
    pub ui_events: flume::Receiver<UiEvent>,
    pub exited: AtomicBool,
    pub bytes_in: AtomicU64,
    pub status: Mutex<LinkStatus>,
    /// `user@host` for remote daemons, `None` locally.
    pub host_label: Option<String>,
    waker: Arc<Waker>,
}

fn palette_list(p: &Palette) -> Vec<u32> {
    let mut v: Vec<u32> = p.ansi.iter().map(|c| c.to_u32()).collect();
    v.extend([p.foreground, p.background, p.cursor, p.selection_fg, p.selection_bg].map(|c| c.to_u32()));
    v
}

fn win(size: TermSize) -> WinSize {
    WinSize { cols: size.cols, rows: size.rows, cell_width: size.cell_width, cell_height: size.cell_height }
}

/// Adapter: the connection's reader thread feeds our mirror terminal.
struct Sink(std::sync::Weak<Session>);

impl PaneSink for Sink {
    fn feed(&self, bytes: &[u8]) {
        if let Some(s) = self.0.upgrade() {
            s.ingest(bytes);
        }
    }

    fn event(&self, ev: PaneEvent) {
        let Some(s) = self.0.upgrade() else { return };
        match ev {
            PaneEvent::Resynced { skipped } => log::debug!("resynced, {skipped} bytes skipped"),
            PaneEvent::Disconnected(why) => {
                *s.status.lock() = LinkStatus::Reconnecting;
                // Shown locally only: the reattach snapshot starts with RIS
                // and wipes this line again.
                s.terminal.feed(format!("\r\n\x1b[0;2m[goatty: connection lost ({why}); reconnecting…]\x1b[0m").as_bytes());
            }
            PaneEvent::Reconnected => *s.status.lock() = LinkStatus::Connected,
            PaneEvent::Lost(why) => {
                s.terminal.feed(format!("\r\n\x1b[0;31m[goatty: session lost: {why}]\x1b[0m\r\n").as_bytes());
                *s.status.lock() = LinkStatus::Lost(why);
            }
            PaneEvent::Exited(_) => s.exited.store(true, Ordering::Release),
        }
        s.waker.wake();
    }
}

impl Session {
    fn new(terminal: Terminal, ui: flume::Receiver<UiEvent>, backend: Backend, host_label: Option<String>) -> Self {
        let (waker, wake_rx) = Waker::new();
        Self {
            terminal,
            wake_rx,
            backend,
            ui_events: ui,
            exited: AtomicBool::new(false),
            bytes_in: AtomicU64::new(0),
            status: Mutex::new(LinkStatus::Connected),
            host_label,
            waker,
        }
    }

    /// A shell owned by this process (`--local`): no daemon, so it ends
    /// with the window. Same shell choice and managed zsh as goattyd.
    pub fn local(size: TermSize, host: &HostService) -> Result<Arc<Self>> {
        let launch = host.launcher.launch(&host.settings());
        let (pty, reader) = Pty::spawn(&ShellCommand::new(launch.program, launch.args, launch.env), size)?;
        let (terminal, chans) = Terminal::new(size, true, Arc::new(Palette::default()));
        let s = Arc::new(Self::new(terminal, chans.ui, Backend::Local(Mutex::new(pty)), None));
        let weak = Arc::downgrade(&s);
        std::thread::Builder::new().name("pty-replies".into()).spawn(move || {
            while let Ok(bytes) = chans.replies.recv() {
                let Some(s) = weak.upgrade() else { break };
                s.write(&bytes);
            }
        })?;
        let s2 = s.clone();
        std::thread::Builder::new().name("pty-reader".into()).spawn(move || {
            let mut reader = reader;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => s2.ingest(&buf[..n]),
                }
            }
            s2.exited.store(true, Ordering::Release);
            s2.waker.wake();
        })?;
        Ok(s)
    }

    /// Attach to (or create) a session on the daemon behind `conn`.
    pub fn daemon(conn: Arc<Connection>, size: TermSize, mode: AttachMode) -> Result<Arc<Self>> {
        let target = match mode {
            AttachMode::New => None,
            AttachMode::Id(id) => Some(id),
            AttachMode::Auto => conn.list_sessions()?.iter().filter(|s| s.attached == 0).map(|s| s.id).max(),
        };
        let host_label = (!conn.endpoint().is_local()).then(|| {
            let h = conn.host();
            format!("{}@{}", h.user, h.hostname)
        });
        // Client-side terminal never answers queries; the daemon's does.
        let (terminal, chans) = Terminal::new(size, false, Arc::new(Palette::default()));
        let s = Arc::new(Self::new(terminal, chans.ui, Backend::Daemon { conn: conn.clone(), id: AtomicU64::new(0) }, host_label));
        let sink = Arc::new(Sink(Arc::downgrade(&s)));
        let id = match target {
            Some(id) => {
                conn.attach(id, win(size), sink)?;
                id
            }
            None => {
                // A local cwd means nothing on another host.
                let cwd =
                    conn.endpoint().is_local().then(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned())).flatten();
                conn.create(win(size), cwd, sink)?
            }
        };
        log::info!("attached to session {id}");
        if let Backend::Daemon { id: slot, .. } = &s.backend {
            slot.store(id, Ordering::Relaxed);
        }
        Ok(s)
    }

    /// Re-arm the wake-up. Call when handling a wake, before reading state.
    pub fn clear_wake(&self) {
        self.waker.clear();
    }

    fn ingest(&self, bytes: &[u8]) {
        self.bytes_in.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if self.terminal.feed(bytes) {
            self.waker.wake();
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        match &self.backend {
            Backend::Local(pty) => {
                let _ = pty.lock().write(bytes);
            }
            Backend::Daemon { conn, id } => conn.input(id.load(Ordering::Relaxed), bytes),
        }
    }

    pub fn resize(&self, size: TermSize) {
        self.terminal.resize(size);
        match &self.backend {
            Backend::Local(pty) => {
                let _ = pty.lock().resize(size);
            }
            Backend::Daemon { conn, id } => conn.resize(id.load(Ordering::Relaxed), win(size)),
        }
    }

    /// Show the session with `palette`: our terminal renders with it, and
    /// the daemon answers color queries (OSC 4/10/11) with it.
    pub fn set_palette(&self, palette: Palette) {
        let colors = palette_list(&palette);
        self.terminal.set_palette(Arc::new(palette));
        if let Backend::Daemon { conn, id } = &self.backend {
            conn.set_palette(id.load(Ordering::Relaxed), colors);
        }
    }

    pub fn backend_name(&self) -> String {
        match &self.backend {
            Backend::Local(_) => "local pty".into(),
            Backend::Daemon { conn, .. } => format!("goattyd via {}", conn.endpoint().label()),
        }
    }

    /// Window title: the app's title plus host and link state when remote.
    pub fn decorate_title(&self, title: &str) -> String {
        let mut t = title.to_string();
        if let Some(h) = &self.host_label {
            t = format!("{t} — {h}");
        }
        match &*self.status.lock() {
            LinkStatus::Connected => t,
            LinkStatus::Reconnecting => format!("{t} (reconnecting…)"),
            LinkStatus::Lost(_) => format!("{t} (disconnected)"),
        }
    }
}
