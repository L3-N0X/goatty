//! goattyd — the goatty session daemon.
//!
//! Owns every PTY and a headless, authoritative terminal state per session.
//! Clients connect over a Unix socket, directly or through `goattyd proxy`
//! (which is what `ssh host goattyd proxy` runs). They get a VT snapshot,
//! then a live byte stream. Closing a client detaches; the shell keeps running.
//!
//! Usage:
//!   goattyd [serve]    run the daemon (foreground)
//!   goattyd proxy      relay stdin/stdout to the daemon, starting it if needed
//!   goattyd list       list sessions
//!   goattyd kill <id>  kill a session
//!   goattyd send <id> <text>  type into a session (\r = Enter)
//!   goattyd stop       stop the daemon and all sessions

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use goatty_proto::{
    ClientMsg, Frame, HostInfo, PROTOCOL_VERSION, ReqId, ServerMsg, SessionId, SessionInfo, StreamOpts, WinSize, encode, read_msg,
};
use goatty_term::pty::{Pty, ShellCommand};
use goatty_term::{Palette, TermSize, Terminal};
use parking_lot::Mutex;

/// One connection's subscription to one session.
///
/// Flow control: `sent` counts stream bytes queued for this subscriber,
/// `acked` what the client reported as consumed. When `sent - acked` would
/// exceed `window`, the subscriber goes `lagging`: raw output is skipped
/// (counted in `skipped`) until the client has consumed everything, then it
/// gets one `Resync` snapshot of the current screen and streaming resumes.
struct Subscriber {
    conn: u64,
    tx: flume::Sender<Frame>,
    window: u64,
    sent: AtomicU64,
    acked: AtomicU64,
    lagging: AtomicBool,
    skipped: AtomicU64,
}

impl Subscriber {
    fn push(&self, frame: Frame, stream_len: usize) -> bool {
        self.sent.fetch_add(stream_len as u64, Ordering::AcqRel);
        self.tx.send(frame).is_ok()
    }

    fn in_flight(&self) -> u64 {
        self.sent.load(Ordering::Acquire).saturating_sub(self.acked.load(Ordering::Acquire))
    }
}

struct Session {
    id: SessionId,
    term: Terminal,
    pty: Mutex<Pty>,
    /// Held while feeding + broadcasting, so taking a snapshot and
    /// (un)subscribing are atomic with respect to the output stream: no gap,
    /// no overlap.
    subs: Mutex<Vec<Arc<Subscriber>>>,
    created_unix: u64,
}

impl Session {
    fn info(&self) -> SessionInfo {
        let (cols, rows) = self.term.size();
        let pty = self.pty.lock();
        SessionInfo {
            id: self.id,
            pid: pty.pid(),
            title: self.term.title(),
            cwd: pty.cwd().map(|p| p.to_string_lossy().into_owned()),
            cols,
            rows,
            attached: self.subs.lock().len() as u32,
            created_unix: self.created_unix,
        }
    }

    fn resize(&self, ws: WinSize) {
        let size = to_size(ws);
        if self.term.size() != (size.cols, size.rows) {
            self.term.resize(size);
        }
        if let Err(e) = self.pty.lock().resize(size) {
            log::warn!("session {}: resize failed: {e}", self.id);
        }
    }

    /// Snapshot + subscribe under one lock. Returns the subscriber.
    fn subscribe(&self, conn: u64, req: ReqId, opts: StreamOpts, tx: &flume::Sender<Frame>) -> Arc<Subscriber> {
        let mut subs = self.subs.lock();
        subs.retain(|s| s.conn != conn); // re-attach on the same connection replaces
        let snapshot = self.term.reattach_snapshot_with_history(opts.history as usize);
        let sub = Arc::new(Subscriber {
            conn,
            tx: tx.clone(),
            window: opts.window.max(64 << 10) as u64,
            sent: AtomicU64::new(0),
            acked: AtomicU64::new(0),
            lagging: AtomicBool::new(false),
            skipped: AtomicU64::new(0),
        });
        let len = snapshot.len();
        if let Ok(f) = encode(&ServerMsg::Attached { req, session: self.id, snapshot }) {
            sub.push(f, len);
        }
        subs.push(sub.clone());
        sub
    }

    fn unsubscribe(&self, conn: u64) {
        self.subs.lock().retain(|s| s.conn != conn);
    }

    /// Client acknowledged `consumed` bytes. If it was lagging and has now
    /// caught up completely, send the current screen and resume streaming.
    fn ack(&self, sub: &Subscriber, consumed: u64) {
        sub.acked.fetch_max(consumed, Ordering::AcqRel);
        if sub.lagging.load(Ordering::Acquire) && sub.in_flight() == 0 {
            let _subs = self.subs.lock(); // order against the output pump
            if sub.lagging.load(Ordering::Acquire) && sub.in_flight() == 0 {
                self.resync(sub);
            }
        }
    }

    /// Caller holds `subs`.
    fn resync(&self, sub: &Subscriber) {
        let snapshot = self.term.resync_snapshot();
        let skipped = sub.skipped.swap(0, Ordering::AcqRel);
        let len = snapshot.len();
        if let Ok(f) = encode(&ServerMsg::Resync { session: self.id, skipped, snapshot }) {
            sub.push(f, len);
        }
        sub.lagging.store(false, Ordering::Release);
    }
}

fn to_size(ws: WinSize) -> TermSize {
    TermSize { cols: ws.cols.max(2), rows: ws.rows.max(1), cell_width: ws.cell_width, cell_height: ws.cell_height }
}

#[derive(Default)]
struct Daemon {
    sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    next_session: AtomicU64,
    next_conn: AtomicU64,
}

impl Daemon {
    fn get(&self, id: SessionId) -> Option<Arc<Session>> {
        self.sessions.lock().get(&id).cloned()
    }

    fn create(self: &Arc<Self>, ws: WinSize, cwd: Option<String>) -> Result<Arc<Session>> {
        let size = to_size(ws);
        let mut cmd = ShellCommand::default_shell();
        if let Some(cwd) = cwd.filter(|c| std::path::Path::new(c).is_dir()) {
            cmd.cwd = Some(cwd.into());
        }
        let (pty, reader) = Pty::spawn(&cmd, size)?;
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
        });
        log::info!("session {id}: spawned {} (pid {:?})", cmd.program, session.pty.lock().pid());

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
                let exited = encode(&ServerMsg::Exited { session: s.id, code }).ok();
                for sub in s.subs.lock().drain(..) {
                    // Let lagging clients see the final screen before the end.
                    if sub.lagging.load(Ordering::Acquire) {
                        s.resync(&sub);
                    }
                    if let Some(f) = &exited {
                        let _ = sub.tx.send(f.clone());
                    }
                }
            })?;
        }
        self.sessions.lock().insert(id, session.clone());
        Ok(session)
    }
}

fn pump_output(s: &Session, mut reader: Box<dyn Read + Send>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut subs = s.subs.lock();
                s.term.feed(&buf[..n]);
                // Encode once, lazily: nobody may be listening, or everyone
                // may be lagging.
                let mut frame: Option<Frame> = None;
                subs.retain(|sub| {
                    if sub.lagging.load(Ordering::Acquire) {
                        sub.skipped.fetch_add(n as u64, Ordering::Relaxed);
                        return true;
                    }
                    let in_flight = sub.in_flight();
                    if in_flight > 0 && in_flight + n as u64 > sub.window {
                        sub.lagging.store(true, Ordering::Release);
                        sub.skipped.fetch_add(n as u64, Ordering::Relaxed);
                        return true;
                    }
                    let f = frame
                        .get_or_insert_with(|| encode(&ServerMsg::Output { session: s.id, data: buf[..n].to_vec() }).expect("encode"))
                        .clone();
                    sub.push(f, n)
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break, // EIO when the child exits
        }
    }
}

fn host_info() -> HostInfo {
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

fn handle_client(daemon: Arc<Daemon>, stream: UnixStream) -> Result<()> {
    let conn = daemon.next_conn.fetch_add(1, Ordering::Relaxed);
    // Unbounded, but bounded in practice: stream frames are limited by each
    // subscriber's window, control replies are tiny.
    let (tx, rx) = flume::unbounded::<Frame>();
    let wstream = stream.try_clone()?;
    let writer = std::thread::Builder::new().name(format!("c{conn}-writer")).spawn(move || {
        let mut w = BufWriter::with_capacity(128 << 10, &wstream);
        while let Ok(f) = rx.recv() {
            if w.write_all(&f).is_err() {
                break;
            }
            // Batch under load; flush the moment the queue is idle.
            if rx.is_empty() && w.flush().is_err() {
                break;
            }
        }
        let _ = wstream.shutdown(std::net::Shutdown::Both);
    })?;
    let reply = |msg: ServerMsg| {
        if let Ok(f) = encode(&msg) {
            let _ = tx.send(f);
        }
    };

    let mut subs: HashMap<SessionId, (Arc<Session>, Arc<Subscriber>)> = HashMap::new();
    let mut rstream = BufReader::with_capacity(64 << 10, stream);
    let result = (|| -> Result<()> {
        loop {
            let msg: ClientMsg = match read_msg(&mut rstream) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            match msg {
                ClientMsg::Hello { version, client } => {
                    if version != PROTOCOL_VERSION {
                        reply(ServerMsg::Error {
                            req: None,
                            message: format!("protocol mismatch: daemon v{PROTOCOL_VERSION}, client v{version}"),
                        });
                        return Ok(());
                    }
                    log::debug!("connection {conn}: {client}");
                    reply(ServerMsg::Hello { version: PROTOCOL_VERSION, daemon_pid: std::process::id(), host: host_info() });
                }
                ClientMsg::ListSessions { req } => {
                    let mut list: Vec<_> = daemon.sessions.lock().values().map(|s| s.info()).collect();
                    list.sort_by_key(|s| s.id);
                    reply(ServerMsg::Sessions { req, list });
                }
                ClientMsg::Create { req, size, cwd, opts } => match daemon.create(size, cwd) {
                    Ok(s) => {
                        let sub = s.subscribe(conn, req, opts, &tx);
                        subs.insert(s.id, (s, sub));
                    }
                    Err(e) => reply(ServerMsg::Error { req: Some(req), message: format!("{e:#}") }),
                },
                ClientMsg::Attach { req, session, size, opts } => match daemon.get(session) {
                    Some(s) => {
                        // Resize first so the snapshot already has the
                        // client's geometry (reflowed by the VT core).
                        s.resize(size);
                        let sub = s.subscribe(conn, req, opts, &tx);
                        subs.insert(s.id, (s, sub));
                    }
                    None => reply(ServerMsg::Error { req: Some(req), message: format!("no session {session}") }),
                },
                ClientMsg::Detach { session } => {
                    if let Some((s, _)) = subs.remove(&session) {
                        s.unsubscribe(conn);
                    }
                }
                ClientMsg::Input { session, data } => {
                    if let Some((s, _)) = subs.get(&session) {
                        let _ = s.pty.lock().write(&data);
                    }
                }
                ClientMsg::Resize { session, size } => {
                    if let Some((s, _)) = subs.get(&session) {
                        s.resize(size);
                    }
                }
                ClientMsg::Ack { session, consumed } => {
                    if let Some((s, sub)) = subs.get(&session) {
                        s.ack(sub, consumed);
                    }
                }
                ClientMsg::Ping { nonce } => reply(ServerMsg::Pong { nonce }),
                ClientMsg::Kill { req, session } => match daemon.get(session) {
                    Some(s) => {
                        s.pty.lock().kill();
                        reply(ServerMsg::Ok { req });
                    }
                    None => reply(ServerMsg::Error { req: Some(req), message: format!("no session {session}") }),
                },
                ClientMsg::SendInput { req, session, data } => match daemon.get(session) {
                    Some(s) => {
                        let _ = s.pty.lock().write(&data);
                        reply(ServerMsg::Ok { req });
                    }
                    None => reply(ServerMsg::Error { req: Some(req), message: format!("no session {session}") }),
                },
                ClientMsg::Shutdown { req } => {
                    log::info!("shutdown requested");
                    for s in daemon.sessions.lock().values() {
                        s.pty.lock().kill();
                    }
                    reply(ServerMsg::Ok { req });
                    std::thread::sleep(Duration::from_millis(100));
                    let _ = std::fs::remove_file(goatty_proto::socket_path());
                    std::process::exit(0);
                }
            }
        }
    })();

    for (id, (s, _)) in subs {
        s.unsubscribe(conn);
        log::info!("connection {conn} detached from session {id}");
    }
    drop(tx);
    let _ = writer.join();
    result
}

fn bind(path: &std::path::Path) -> Result<(UnixListener, u64)> {
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
            if let Err(e) = handle_client(d, stream) {
                log::debug!("client error: {e:#}");
            }
        });
    }
}

fn serve() -> Result<()> {
    let path = goatty_proto::socket_path();
    let (listener, mut ino) = bind(&path)?;
    log::info!("goattyd {} listening on {} (pid {})", env!("CARGO_PKG_VERSION"), path.display(), std::process::id());

    let daemon = Arc::new(Daemon::default());
    {
        let d = daemon.clone();
        std::thread::Builder::new().name("accept".into()).spawn(move || accept_loop(d, listener))?;
    }
    // Socket watchdog. On a server, systemd-logind deletes
    // /run/user/$UID when the user's last login session ends (unless
    // lingering is enabled). The daemon and its shells survive that, but the
    // socket path vanishes, and the next `ssh host goattyd proxy` would start
    // a second, empty daemon. Re-create the socket when it disappears.
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let current = std::fs::metadata(&path).ok().map(|m| m.ino());
        if current == Some(ino) {
            continue;
        }
        log::warn!("socket {} disappeared; re-binding", path.display());
        match bind(&path) {
            Ok((l, i)) => {
                ino = i;
                let d = daemon.clone();
                std::thread::Builder::new().name("accept".into()).spawn(move || accept_loop(d, l))?;
            }
            Err(e) => log::error!("re-bind failed: {e:#}"),
        }
    }
}

/// `goattyd proxy`: relay stdio ⇄ the daemon socket, starting the daemon if
/// needed. This is the remote end of every non-local transport
/// (`ssh host goattyd proxy`). It never parses the protocol.
fn proxy() -> Result<()> {
    use std::os::fd::FromRawFd;
    let exe = std::env::current_exe().ok();
    let sock = goatty_client::connect_or_spawn_daemon(exe)?;
    let mut to_daemon = sock.try_clone()?;
    let mut from_daemon = sock;
    // Raw fds: std's Stdout is line-buffered, which would hold binary
    // frames back until a 0x0A byte happens to pass through.
    // SAFETY: fds 0/1 are owned by this process for its whole life and
    // nothing else in `proxy` uses them.
    let mut stdin = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
    let mut stdout = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) });
    let up = std::thread::spawn(move || {
        relay(&mut *stdin, &mut to_daemon);
        let _ = to_daemon.shutdown(std::net::Shutdown::Write);
    });
    relay(&mut from_daemon, &mut *stdout);
    // Daemon closed the connection: we're done, whatever stdin is doing.
    drop(up);
    Ok(())
}

/// Plain read/write relay. Deliberately not `std::io::copy`: on Linux it
/// uses `splice()`, and data spliced from a pipe into a Unix socket can sit
/// in the kernel instead of being delivered (observed: the client's Hello
/// never reached the daemon), which would deadlock the handshake.
fn relay(from: &mut impl Read, to: &mut impl Write) {
    let mut buf = vec![0u8; 64 << 10];
    loop {
        match from.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

fn control(msg: impl FnOnce(ReqId) -> ClientMsg) -> Result<ServerMsg> {
    let mut s = UnixStream::connect(goatty_proto::socket_path()).context("goattyd is not running")?;
    s.write_all(&encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, client: "goattyd-cli".into() })?)?;
    match read_msg::<_, ServerMsg>(&mut s) {
        Ok(ServerMsg::Hello { version: PROTOCOL_VERSION, .. }) => {}
        _ => bail!(
            "the goattyd at {} doesn't speak protocol v{PROTOCOL_VERSION} (an older build still running?). \
             Stop it with `pkill -x goattyd` (this ends its sessions)",
            goatty_proto::socket_path().display()
        ),
    }
    s.write_all(&encode(&msg(1))?)?;
    Ok(read_msg(&mut s)?)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str);
    // The proxy's stderr goes back over ssh into the GUI's log: keep it quiet.
    let default_level = if cmd == Some("proxy") { "warn" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level)).init();
    match cmd {
        None | Some("serve") => serve(),
        Some("proxy") => proxy(),
        Some("list") => {
            match control(|req| ClientMsg::ListSessions { req })? {
                ServerMsg::Sessions { list, .. } => {
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
                }
                other => println!("{other:?}"),
            }
            Ok(())
        }
        Some("kill") => {
            let session: SessionId = args.get(1).context("usage: goattyd kill <id>")?.parse()?;
            println!("{:?}", control(|req| ClientMsg::Kill { req, session })?);
            Ok(())
        }
        Some("send") => {
            let session: SessionId = args.get(1).context("usage: goattyd send <id> <text>")?.parse()?;
            let text = args.get(2).context("usage: goattyd send <id> <text>")?;
            // Allow \r / \n / \e escapes for scripting.
            let data = text.replace("\\r", "\r").replace("\\n", "\n").replace("\\e", "\x1b").into_bytes();
            println!("{:?}", control(|req| ClientMsg::SendInput { req, session, data })?);
            Ok(())
        }
        Some("stop") => {
            println!("{:?}", control(|req| ClientMsg::Shutdown { req })?);
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?} (expected: serve | proxy | list | kill <id> | send <id> <text> | stop)"),
    }
}
