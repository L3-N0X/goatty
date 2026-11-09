//! The daemon process: socket setup, the accept loop, and the session
//! registry.

use std::collections::HashMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use goatty_proto::{HostInfo, SessionId, SessionInfo, WinSize};
use goatty_shell::HostService;
use goatty_term::pty::{Pty, ShellCommand};
use goatty_term::{Palette, Terminal};
use parking_lot::Mutex;

use crate::layouts::Layouts;
use crate::session::{Session, pump_output, to_size};

/// The session registry shared by every connection.
pub(crate) struct Daemon {
    /// Host settings, generations, and each session's shell. The settings
    /// file is re-read for every new session, so edits apply without
    /// restarting the daemon.
    pub(crate) host: HostService,
    /// Removed on shutdown.
    pub(crate) socket: PathBuf,
    pub(crate) sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    /// The windows' tabs and splits.
    pub(crate) layouts: Mutex<Layouts>,
    next_session: AtomicU64,
    pub(crate) next_conn: AtomicU64,
}

impl Daemon {
    fn new(socket: PathBuf) -> Self {
        Self {
            // `goatty-setup` in a managed shell runs this goattyd's `shell install`.
            host: HostService::from_env(std::env::current_exe().ok()),
            socket,
            sessions: Mutex::default(),
            layouts: Mutex::default(),
            next_session: AtomicU64::new(0),
            next_conn: AtomicU64::new(0),
        }
    }

    pub(crate) fn get(&self, id: SessionId) -> Option<Arc<Session>> {
        self.sessions.lock().get(&id).cloned()
    }

    /// A session running the user's shell (goatty's managed zsh by default).
    pub(crate) fn create(self: &Arc<Self>, ws: WinSize, cwd: Option<String>) -> Result<Arc<Session>> {
        let host = self.host.settings();
        for issue in &host.file.issues {
            log::warn!("host settings: {issue}");
        }
        let launch = self.host.launcher.launch(&host);
        let managed = launch.managed;
        self.spawn(ShellCommand::new(launch.program, launch.args, launch.env), managed, ws, cwd)
    }

    /// A session running `argv` instead of a shell.
    pub(crate) fn run(self: &Arc<Self>, ws: WinSize, cwd: Option<String>, mut argv: Vec<String>) -> Result<Arc<Session>> {
        if argv.is_empty() {
            bail!("nothing to run");
        }
        let program = argv.remove(0);
        self.spawn(ShellCommand::new(program, argv, Vec::new()), false, ws, cwd)
    }

    fn spawn(self: &Arc<Self>, mut cmd: ShellCommand, managed: bool, ws: WinSize, cwd: Option<String>) -> Result<Arc<Session>> {
        let size = to_size(ws);
        if let Some(cwd) = cwd.filter(|c| std::path::Path::new(c).is_dir()) {
            cmd.cwd = Some(cwd.into());
        }
        let (pty, reader) = Pty::spawn(&cmd, size).with_context(|| format!("couldn't start {}", cmd.program))?;
        // The daemon's terminal is authoritative and answers queries
        // (DA, DSR, OSC 4/10/11...). Clients run with answering disabled.
        // Over a remote link this also means query replies never pay the RTT.
        let (term, chans) = Terminal::new(size, true, Arc::new(Palette::default()));
        let id = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let session = Arc::new(Session {
            id,
            term,
            pty: Mutex::new(pty),
            subs: Mutex::new(Vec::new()),
            created_unix: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            process: Mutex::new(None),
            fg: OnceLock::new(),
        });
        if let Err(e) = session.watch_foreground() {
            log::warn!("session {id}: can't watch the foreground program: {e}");
        }
        let pid = session.pty.lock().pid();
        log::info!("session {id}: spawned {}{} (pid {pid:?})", cmd.program, if managed { " with goatty's managed config" } else { "" });
        // Only goatty's zsh handles the reload request.
        let managed_pid = pid.filter(|_| managed);
        if let Some(pid) = managed_pid {
            self.host.track_shell(pid);
        }

        // Query replies → PTY.
        {
            let s = Arc::downgrade(&session);
            std::thread::Builder::new().name(format!("s{id}-replies")).spawn(move || {
                while let Ok(bytes) = chans.replies.recv() {
                    let Some(s) = s.upgrade() else { break };
                    let _ = s.pty.lock().write(&bytes);
                }
            })?;
        }
        // PTY → terminal + subscribers.
        {
            let s = session.clone();
            let daemon = self.clone();
            std::thread::Builder::new().name(format!("s{id}-reader")).spawn(move || {
                pump_output(&s, reader);
                let code = s.pty.lock().try_wait();
                log::info!("session {}: exited ({code:?})", s.id);
                daemon.sessions.lock().remove(&s.id);
                if let Some(pid) = managed_pid {
                    daemon.host.untrack_shell(pid);
                }
                s.finish(code);
            })?;
        }
        self.sessions.lock().insert(id, session.clone());
        Ok(session)
    }

    /// Give connection `conn` the newest free window layout (see `Layouts`).
    pub(crate) fn claim_layout(&self, conn: u64) -> Option<goatty_proto::LayoutInfo> {
        let live: std::collections::HashSet<SessionId> = self.sessions.lock().keys().copied().collect();
        self.layouts.lock().claim(conn, |s| live.contains(&s))
    }

    pub(crate) fn list(&self) -> Vec<SessionInfo> {
        let mut list: Vec<_> = self.sessions.lock().values().map(|s| s.info()).collect();
        list.sort_by_key(|s| s.id);
        list
    }

    /// Kill every session, remove the socket and exit the process.
    pub(crate) fn shutdown(&self) -> ! {
        log::info!("shutdown requested");
        for s in self.sessions.lock().values() {
            s.pty.lock().kill();
        }
        // Give the connection writers a moment to flush the `Ok` reply.
        std::thread::sleep(Duration::from_millis(100));
        let _ = std::fs::remove_file(&self.socket);
        std::process::exit(0);
    }
}

pub(crate) fn host_info() -> HostInfo {
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    HostInfo {
        hostname,
        user: std::env::var("USER").unwrap_or_default(),
        os: std::env::consts::OS.into(),
        daemon_version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn bind(path: &Path) -> Result<(UnixListener, u64)> {
    let dir = path.parent().context("socket path has no parent")?;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("goattyd already running at {}", path.display());
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let ino = std::fs::metadata(path)?.ino();
    Ok((listener, ino))
}

fn accept_loop(daemon: Arc<Daemon>, listener: UnixListener) {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                log::warn!("accept: {e}");
                continue;
            }
        };
        let d = daemon.clone();
        std::thread::spawn(move || {
            if let Err(e) = crate::client::handle(d, stream) {
                log::debug!("client error: {e:#}");
            }
        });
    }
}

/// The listening socket systemd passed us (`goattyd.socket`, see
/// `goatty_client::service`), per `sd_listen_fds(3)`: `LISTEN_PID` is us and
/// `LISTEN_FDS` ≥ 1 means fd 3 is it. The variables are removed so the
/// shells don't inherit them, and the fd is closed on exec.
fn listener_from_systemd() -> Option<UnixListener> {
    use std::os::fd::FromRawFd;
    const SD_LISTEN_FDS_START: i32 = 3;
    let pid = std::env::var("LISTEN_PID").ok()?;
    let fds = std::env::var("LISTEN_FDS").ok();
    // SAFETY: called from `serve` before the daemon starts any thread.
    unsafe {
        std::env::remove_var("LISTEN_PID");
        std::env::remove_var("LISTEN_FDS");
        std::env::remove_var("LISTEN_FDNAMES");
    }
    if pid.parse::<u32>().ok()? != std::process::id() || fds?.parse::<u32>().ok()? < 1 {
        return None;
    }
    // SAFETY: fd 3 is ours: systemd passed it and nothing in this process
    // has opened or adopted it.
    unsafe {
        let flags = libc::fcntl(SD_LISTEN_FDS_START, libc::F_GETFD);
        if flags < 0 {
            return None;
        }
        libc::fcntl(SD_LISTEN_FDS_START, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        Some(UnixListener::from_raw_fd(SD_LISTEN_FDS_START))
    }
}

/// Run the daemon on the socket at `path` until `goattyd stop`. Never returns
/// `Ok`; startup errors (socket in use, permissions) are returned. Under
/// systemd socket activation it serves the socket it was given.
pub fn serve(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    let (listener, mut ino) = match listener_from_systemd() {
        Some(l) => {
            log::info!("using the socket from systemd");
            (l, std::fs::metadata(&path).map(|m| m.ino()).unwrap_or(0))
        }
        None => bind(&path)?,
    };
    log::info!("goattyd {} listening on {} (pid {})", env!("CARGO_PKG_VERSION"), path.display(), std::process::id());

    let daemon = Arc::new(Daemon::new(path.clone()));
    {
        let d = daemon.clone();
        std::thread::Builder::new().name("accept".into()).spawn(move || accept_loop(d, listener))?;
    }
    // Socket watchdog. On a server, systemd-logind deletes
    // /run/user/$UID when the user's last login session ends (unless
    // lingering is enabled). The daemon and its shells survive that, but the
    // socket path vanishes, and the next `ssh host goattyd proxy` would start
    // a second, empty daemon. Re-create the socket when it disappears.
    let mut taken_over = false;
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let current = std::fs::metadata(&path).ok().map(|m| m.ino());
        if current == Some(ino) {
            continue;
        }
        // Someone else listens there now (another daemon, or a systemd
        // socket installed meanwhile): leave it alone, say so once.
        if current.is_some() && UnixStream::connect(&path).is_ok() {
            if !taken_over {
                log::warn!("another listener took over {}; this daemon keeps serving its connected clients", path.display());
                taken_over = true;
            }
            continue;
        }
        log::warn!("socket {} disappeared; re-binding", path.display());
        match bind(&path) {
            Ok((l, i)) => {
                ino = i;
                taken_over = false;
                let d = daemon.clone();
                std::thread::Builder::new().name("accept".into()).spawn(move || accept_loop(d, l))?;
            }
            Err(e) => log::error!("re-bind failed: {e:#}"),
        }
    }
}
