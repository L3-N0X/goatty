//! [`Connection`]: a reconnecting, multiplexed link to one goattyd.

use std::collections::HashMap;
use std::io::{BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use goatty_proto::{
    ClientMsg, FileDiff, Frame, GenerationInfo, HostInfo, LayoutId, LayoutInfo, PROTOCOL_VERSION, RUN_SINCE, ReqId, ServerMsg, SessionId,
    SessionInfo, StreamOpts, WinSize, encode, read_msg,
};
use parking_lot::Mutex;

use crate::handshake::Outcome;
use crate::transport::{Closer, Endpoint, Transport};
use crate::{PaneEvent, PaneSink, handshake, remote};

/// How long to wait for a failed transport command's last words on stderr.
const STDERR_GRACE: Duration = Duration::from_secs(2);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Plugin operations download releases and registries.
pub const PLUGIN_TIMEOUT: Duration = Duration::from_secs(600);
const HEARTBEAT: Duration = Duration::from_secs(5);
const DEAD_AFTER: Duration = Duration::from_secs(20);

enum WriterCmd {
    Frame(Frame),
    Replace(Box<dyn Write + Send>),
}

struct Pane {
    sink: Arc<dyn PaneSink>,
    size: WinSize,
    consumed: u64,
    acked: u64,
    /// Last `SetPalette` colors, re-sent after a reattach.
    palette: Option<Vec<u32>>,
}

/// The host's settings, as the daemon reports them.
#[derive(Debug, Clone)]
pub struct HostSettingsReply {
    /// The settings document (`goatty_config::store::parse` reads it).
    pub json: String,
    pub issues: Vec<String>,
    pub env_overrides: Vec<String>,
    pub generation: Option<u32>,
}

/// Result of [`Connection::set_host_settings`].
#[derive(Debug, Clone)]
pub struct HostSettingsApplied {
    pub issues: Vec<String>,
    pub diff: Vec<FileDiff>,
    pub applied: bool,
    pub generation: Option<u32>,
}

enum Pending {
    Reply(flume::Sender<ServerMsg>),
    Attach { tx: flume::Sender<ServerMsg>, sink: Arc<dyn PaneSink>, size: WinSize },
    Reattach(SessionId),
}

type Reader = std::io::BufReader<Box<dyn Read + Send>>;

struct Shared {
    endpoint: Endpoint,
    opts: StreamOpts,
    client_name: String,
    out: flume::Sender<WriterCmd>,
    panes: Mutex<HashMap<SessionId, Pane>>,
    /// The window's last saved layout, saved again after a reconnect (the
    /// daemon frees a layout when its connection drops).
    layout: Mutex<Option<ClientMsg>>,
    pending: Mutex<HashMap<ReqId, Pending>>,
    next_req: AtomicU32,
    host: Mutex<HostInfo>,
    /// The protocol version we announce: ours, or lower after an old
    /// exact-match daemon told us its version.
    announce: AtomicU32,
    /// The version agreed on the current link.
    version: AtomicU32,
    connected: AtomicBool,
    closed: AtomicBool,
    closer: Mutex<Option<Closer>>,
    epoch: Instant,
    last_rx_us: AtomicU64,
    rtt_us: AtomicU64,
}

impl Shared {
    fn send(&self, msg: &ClientMsg) {
        match encode(msg) {
            Ok(f) => {
                let _ = self.out.send(WriterCmd::Frame(f));
            }
            Err(e) => log::error!("encode: {e}"),
        }
    }

    fn req_id(&self) -> ReqId {
        self.next_req.fetch_add(1, Ordering::Relaxed)
    }

    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    fn close_transport(&self) {
        if let Some(c) = self.closer.lock().take() {
            c.close();
        }
    }

    /// Open a transport and shake hands, announcing a lower version once if
    /// an old daemon asks for it.
    /// `interactive: false` for automatic reconnects (ssh must not prompt).
    fn connect(&self, interactive: bool) -> Result<(Reader, HostInfo, u32)> {
        for _ in 0..2 {
            if let Some(ok) = self.handshake(self.endpoint.connect(interactive)?)? {
                return Ok(ok);
            }
        }
        bail!("goattyd via {} keeps rejecting the handshake", self.endpoint.label())
    }

    /// `None`: try again with the (now lowered) announced version.
    fn handshake(&self, mut t: Transport) -> Result<Option<(Reader, HostInfo, u32)>> {
        let announced = self.announce.load(Ordering::Relaxed);
        let stderr = t.stderr.clone();
        // A transport that never answers (hung ssh) is torn down by the caller
        // via the stored closer, which unblocks the read.
        *self.closer.lock() = Some(t.closer.clone());
        let greeting = t
            .writer
            .write_all(&handshake::hello(&self.client_name, announced)?)
            .and_then(|()| t.writer.flush())
            .map_err(anyhow::Error::from)
            .and_then(|()| handshake::read_greeting(&mut t.reader, self.endpoint.label(), announced));
        match greeting {
            Ok(Outcome::Ok(g)) => {
                let _ = self.out.send(WriterCmd::Replace(t.writer));
                self.version.store(g.version, Ordering::Relaxed);
                self.last_rx_us.store(self.now_us(), Ordering::Relaxed);
                self.connected.store(true, Ordering::Release);
                Ok(Some((t.reader, g.host, g.pid)))
            }
            Ok(Outcome::RetryWith(v)) => {
                log::info!("goattyd via {} only speaks protocol v{v}; reconnecting with that", self.endpoint.label());
                self.close_transport();
                self.announce.store(v, Ordering::Relaxed);
                Ok(None)
            }
            Err(e) => {
                self.close_transport();
                let lines = stderr.wait(STDERR_GRACE);
                Err(match &self.endpoint {
                    Endpoint::Ssh(target) => remote::diagnose(&target.host, e, &lines),
                    _ => remote::with_stderr(e, &lines),
                })
            }
        }
    }
}

/// One link to one goattyd. Cheap to share (`Arc`); dropping the last handle
/// closes the link (sessions keep running on the host).
pub struct Connection {
    shared: Arc<Shared>,
}

impl Connection {
    pub fn open(endpoint: Endpoint, client_name: &str) -> Result<Arc<Self>> {
        let opts = endpoint.default_opts();
        let (out, out_rx) = flume::unbounded();
        let shared = Arc::new(Shared {
            endpoint,
            opts,
            client_name: client_name.into(),
            out,
            panes: Mutex::new(HashMap::new()),
            layout: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_req: AtomicU32::new(1),
            host: Mutex::new(HostInfo::default()),
            announce: AtomicU32::new(PROTOCOL_VERSION),
            version: AtomicU32::new(0),
            connected: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            closer: Mutex::new(None),
            epoch: Instant::now(),
            last_rx_us: AtomicU64::new(0),
            rtt_us: AtomicU64::new(0),
        });

        std::thread::Builder::new().name("goatty-writer".into()).spawn(move || writer_loop(out_rx))?;

        let (reader, host, pid) = shared.connect(true)?;
        log::info!(
            "connected to goattyd {} on {}@{} (pid {pid}, via {}, protocol v{})",
            host.daemon_version,
            host.user,
            host.hostname,
            shared.endpoint.label(),
            shared.version.load(Ordering::Relaxed)
        );
        *shared.host.lock() = host;

        let s = shared.clone();
        std::thread::Builder::new().name("goatty-reader".into()).spawn(move || reader_main(s, reader))?;
        if !shared.endpoint.is_local() {
            let weak = Arc::downgrade(&shared);
            std::thread::Builder::new().name("goatty-heartbeat".into()).spawn(move || heartbeat(weak))?;
        }
        Ok(Arc::new(Self { shared }))
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.shared.endpoint
    }

    pub fn host(&self) -> HostInfo {
        self.shared.host.lock().clone()
    }

    /// The protocol version agreed with the daemon on the current link.
    pub fn protocol_version(&self) -> u32 {
        self.shared.version.load(Ordering::Relaxed)
    }

    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    /// Last measured round-trip time (remote links only).
    pub fn rtt(&self) -> Option<Duration> {
        match self.shared.rtt_us.load(Ordering::Relaxed) {
            0 => None,
            us => Some(Duration::from_micros(us)),
        }
    }

    fn request(
        &self,
        pending: impl FnOnce(flume::Sender<ServerMsg>) -> Pending,
        msg: impl FnOnce(ReqId) -> ClientMsg,
    ) -> Result<ServerMsg> {
        self.request_within(REQUEST_TIMEOUT, pending, msg)
    }

    fn request_within(
        &self,
        timeout: Duration,
        pending: impl FnOnce(flume::Sender<ServerMsg>) -> Pending,
        msg: impl FnOnce(ReqId) -> ClientMsg,
    ) -> Result<ServerMsg> {
        if !self.is_connected() {
            bail!("not connected to goattyd");
        }
        let req = self.shared.req_id();
        let (tx, rx) = flume::bounded(1);
        self.shared.pending.lock().insert(req, pending(tx));
        self.shared.send(&msg(req));
        let reply = rx.recv_timeout(timeout);
        self.shared.pending.lock().remove(&req);
        match reply {
            Ok(ServerMsg::Error { message, .. }) => Err(anyhow!("goattyd: {message}")),
            Ok(m) => Ok(m),
            Err(_) => Err(anyhow!("goattyd did not answer (connection lost?)")),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        match self.request(Pending::Reply, |req| ClientMsg::ListSessions { req })? {
            ServerMsg::Sessions { list, .. } => Ok(list),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Spawn a shell on the host and attach `sink` to it. `cwd` is a path on
    /// the host; pass `None` for remote endpoints.
    pub fn create(&self, size: WinSize, cwd: Option<String>, sink: Arc<dyn PaneSink>) -> Result<SessionId> {
        let opts = self.shared.opts;
        match self.request(|tx| Pending::Attach { tx, sink, size }, |req| ClientMsg::Create { req, size, cwd, opts })? {
            ServerMsg::Attached { session, .. } => Ok(session),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Start `argv` in a new session on the host (not the user's shell) and
    /// attach `sink` to it. Needs a daemon with protocol v6.
    pub fn run(&self, size: WinSize, cwd: Option<String>, argv: Vec<String>, sink: Arc<dyn PaneSink>) -> Result<SessionId> {
        let version = self.protocol_version();
        if version < RUN_SINCE {
            bail!("goattyd via {} is too old to run commands in sessions (protocol v{version}); restart it", self.endpoint().label());
        }
        let opts = self.shared.opts;
        match self.request(|tx| Pending::Attach { tx, sink, size }, |req| ClientMsg::Run { req, size, cwd, argv, opts })? {
            ServerMsg::Attached { session, .. } => Ok(session),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub fn attach(&self, session: SessionId, size: WinSize, sink: Arc<dyn PaneSink>) -> Result<()> {
        let opts = self.shared.opts;
        match self.request(|tx| Pending::Attach { tx, sink, size }, |req| ClientMsg::Attach { req, session, size, opts })? {
            ServerMsg::Attached { .. } => Ok(()),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub fn detach(&self, session: SessionId) {
        self.shared.panes.lock().remove(&session);
        self.shared.send(&ClientMsg::Detach { session });
    }

    pub fn kill(&self, session: SessionId) -> Result<()> {
        self.request(Pending::Reply, |req| ClientMsg::Kill { req, session }).map(drop)
    }

    /// Never blocks: queued for the writer thread. Input typed while the link
    /// is down is dropped (the UI shows the disconnected state).
    pub fn input(&self, session: SessionId, data: &[u8]) {
        if self.is_connected() {
            self.shared.send(&ClientMsg::Input { session, data: data.to_vec() });
        }
    }

    pub fn resize(&self, session: SessionId, size: WinSize) {
        if let Some(p) = self.shared.panes.lock().get_mut(&session) {
            p.size = size;
        }
        self.shared.send(&ClientMsg::Resize { session, size });
    }

    /// Tell the daemon which colors this session is shown with, so color
    /// queries get matching answers. Remembered and re-sent on reconnect.
    pub fn set_palette(&self, session: SessionId, colors: Vec<u32>) {
        if let Some(p) = self.shared.panes.lock().get_mut(&session) {
            p.palette = Some(colors.clone());
        }
        if self.is_connected() {
            self.shared.send(&ClientMsg::SetPalette { session, colors });
        }
    }

    /// The host's settings document. Blocks: call off the UI thread.
    pub fn host_settings(&self) -> Result<HostSettingsReply> {
        match self.request(Pending::Reply, |req| ClientMsg::GetHostSettings { req })? {
            ServerMsg::HostSettings { json, issues, env_overrides, generation, .. } => {
                Ok(HostSettingsReply { json, issues, env_overrides, generation })
            }
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Validate `json` and preview the generated-file changes; with `apply`,
    /// also activate and save. Blocks: call off the UI thread.
    pub fn set_host_settings(&self, json: String, apply: bool) -> Result<HostSettingsApplied> {
        match self.request(Pending::Reply, |req| ClientMsg::SetHostSettings { req, json, apply })? {
            ServerMsg::HostSettingsApplied { issues, diff, applied, generation, .. } => {
                Ok(HostSettingsApplied { issues, diff, applied, generation })
            }
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub fn generations(&self) -> Result<Vec<GenerationInfo>> {
        match self.request(Pending::Reply, |req| ClientMsg::ListGenerations { req })? {
            ServerMsg::Generations { list, .. } => Ok(list),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Roll the host back to `generation` (default: the previous one).
    pub fn rollback(&self, generation: Option<u32>) -> Result<HostSettingsReply> {
        match self.request(Pending::Reply, |req| ClientMsg::Rollback { req, generation })? {
            ServerMsg::HostSettings { json, issues, env_overrides, generation, .. } => {
                Ok(HostSettingsReply { json, issues, env_overrides, generation })
            }
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Ask the host's running managed shells to reload their configuration.
    /// Returns how many were asked. Blocks: call off the UI thread.
    pub fn reload_shells(&self) -> Result<u32> {
        match self.request(Pending::Reply, |req| ClientMsg::ReloadShells { req })? {
            ServerMsg::ShellsReloaded { count, .. } => Ok(count),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// A plugin operation (`goatty_plugin::api::Request` as JSON); returns
    /// the reply JSON. Installs download, so this waits up to
    /// [`PLUGIN_TIMEOUT`]. Blocks: call off the UI thread.
    pub fn plugin(&self, request: String) -> Result<String> {
        match self.request_within(PLUGIN_TIMEOUT, Pending::Reply, |req| ClientMsg::Plugin { req, request })? {
            ServerMsg::Plugin { reply, .. } => Ok(reply),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Remember the window's layout (tabs and splits; `json` is the
    /// client's format) on the host, so a later window can restore it. No
    /// `sessions`: forget it. Never blocks; saved again after a reconnect.
    pub fn save_layout(&self, id: LayoutId, sessions: Vec<SessionId>, json: String) {
        let msg = ClientMsg::SaveLayout { id, sessions, json };
        if self.is_connected() {
            self.shared.send(&msg);
        }
        *self.shared.layout.lock() = Some(msg);
    }

    /// Take over the newest layout no window holds (see
    /// `ClientMsg::ClaimLayout`). Blocks: call off the UI thread.
    pub fn claim_layout(&self) -> Result<Option<LayoutInfo>> {
        match self.request(Pending::Reply, |req| ClientMsg::ClaimLayout { req })? {
            ServerMsg::Layout { layout, .. } => Ok(layout),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Drop the current link as if the network failed (debugging/tests).
    pub fn force_disconnect(&self) {
        self.shared.close_transport();
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.close_transport();
    }
}

fn writer_loop(rx: flume::Receiver<WriterCmd>) {
    let mut w: Option<BufWriter<Box<dyn Write + Send>>> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            WriterCmd::Replace(nw) => w = Some(BufWriter::with_capacity(64 << 10, nw)),
            WriterCmd::Frame(f) => {
                if let Some(ww) = &mut w
                    && ww.write_all(&f).is_err()
                {
                    w = None; // the reader notices too and reconnects
                }
            }
        }
        // Batch under load, flush as soon as the queue is idle (keystrokes
        // go out immediately).
        if rx.is_empty()
            && let Some(ww) = &mut w
            && ww.flush().is_err()
        {
            w = None;
        }
    }
}

fn heartbeat(weak: Weak<Shared>) {
    loop {
        std::thread::sleep(HEARTBEAT);
        let Some(s) = weak.upgrade() else { return };
        if s.closed.load(Ordering::Acquire) {
            return;
        }
        if !s.connected.load(Ordering::Acquire) {
            continue;
        }
        let silent = Duration::from_micros(s.now_us().saturating_sub(s.last_rx_us.load(Ordering::Relaxed)));
        if silent > DEAD_AFTER {
            log::warn!("no data from goattyd for {silent:?}; dropping link");
            s.close_transport();
            continue;
        }
        s.send(&ClientMsg::Ping { nonce: s.now_us() });
    }
}

fn reader_main(s: Arc<Shared>, mut reader: Reader) {
    let mut backoff = [200u64, 500, 1000, 2000, 5000].iter().copied().chain(std::iter::repeat(5000));
    loop {
        let err = read_loop(&s, &mut reader);
        s.connected.store(false, Ordering::Release);
        s.close_transport();
        if s.closed.load(Ordering::Acquire) {
            return;
        }
        log::warn!("link to goattyd lost: {err}");
        // Fail in-flight requests; keep panes for reattach.
        s.pending.lock().clear();
        let sinks: Vec<_> = s.panes.lock().values().map(|p| p.sink.clone()).collect();
        for sink in &sinks {
            sink.event(PaneEvent::Disconnected(err.to_string()));
        }
        loop {
            if s.closed.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(backoff.next().unwrap_or(5000)));
            match s.connect(false) {
                Ok((r, host, pid)) => {
                    log::info!("reconnected to goattyd (pid {pid})");
                    *s.host.lock() = host;
                    reader = r;
                    backoff = [200u64, 500, 1000, 2000, 5000].iter().copied().chain(std::iter::repeat(5000));
                    let panes: Vec<(SessionId, WinSize)> = s.panes.lock().iter().map(|(id, p)| (*id, p.size)).collect();
                    for (session, size) in panes {
                        let req = s.req_id();
                        s.pending.lock().insert(req, Pending::Reattach(session));
                        s.send(&ClientMsg::Attach { req, session, size, opts: s.opts });
                    }
                    if let Some(layout) = &*s.layout.lock() {
                        s.send(layout);
                    }
                    break;
                }
                Err(e) => {
                    s.close_transport();
                    log::info!("reconnect failed: {e:#}");
                }
            }
        }
    }
}

fn read_loop(s: &Shared, reader: &mut Reader) -> anyhow::Error {
    loop {
        let msg: ServerMsg = match read_msg(reader) {
            Ok(m) => m,
            Err(e) => return e.into(),
        };
        s.last_rx_us.store(s.now_us(), Ordering::Relaxed);
        dispatch(s, msg);
        // Ack policy: always once the transport buffer is drained (so a
        // lagging subscriber is never left waiting on a partial ack), and in
        // between every quarter window so the stream keeps flowing.
        let drained = reader.buffer().is_empty();
        let quarter = (s.opts.window / 4) as u64;
        let mut acks = Vec::new();
        for (id, p) in s.panes.lock().iter_mut() {
            let unacked = p.consumed - p.acked;
            if unacked > 0 && (drained || unacked >= quarter) {
                p.acked = p.consumed;
                acks.push((*id, p.consumed));
            }
        }
        for (session, consumed) in acks {
            s.send(&ClientMsg::Ack { session, consumed });
        }
    }
}

/// Feed `bytes` to the pane's sink outside the lock, then count them.
fn feed_pane(s: &Shared, session: SessionId, bytes: &[u8]) -> Option<Arc<dyn PaneSink>> {
    let sink = s.panes.lock().get(&session).map(|p| p.sink.clone())?;
    sink.feed(bytes);
    if let Some(p) = s.panes.lock().get_mut(&session) {
        p.consumed += bytes.len() as u64;
    }
    Some(sink)
}

fn dispatch(s: &Shared, msg: ServerMsg) {
    match msg {
        ServerMsg::Output { session, data } => {
            feed_pane(s, session, &data);
        }
        ServerMsg::Resync { session, skipped, snapshot } => {
            log::debug!("session {session}: resync after skipping {skipped} bytes ({} byte snapshot)", snapshot.len());
            if let Some(sink) = feed_pane(s, session, &snapshot) {
                sink.event(PaneEvent::Resynced { skipped });
            }
        }
        ServerMsg::Attached { req, session, snapshot } => {
            let pending = s.pending.lock().remove(&req);
            match pending {
                Some(Pending::Attach { tx, sink, size }) => {
                    sink.feed(&snapshot);
                    let n = snapshot.len() as u64;
                    s.panes.lock().insert(session, Pane { sink, size, consumed: n, acked: 0, palette: None });
                    let _ = tx.send(ServerMsg::Attached { req, session, snapshot: Vec::new() });
                }
                Some(Pending::Reattach(_)) => {
                    let pane = s.panes.lock().get_mut(&session).map(|p| {
                        // New subscription: stream offsets restart at zero.
                        p.consumed = snapshot.len() as u64;
                        p.acked = 0;
                        (p.sink.clone(), p.palette.clone())
                    });
                    if let Some((sink, palette)) = pane {
                        if let Some(colors) = palette {
                            s.send(&ClientMsg::SetPalette { session, colors });
                        }
                        sink.feed(&snapshot);
                        sink.event(PaneEvent::Reconnected);
                    }
                }
                _ => log::warn!("unsolicited Attached for session {session}"),
            }
        }
        ServerMsg::Exited { session, code } => {
            if let Some(p) = s.panes.lock().remove(&session) {
                p.sink.event(PaneEvent::Exited(code));
            }
        }
        ServerMsg::Process { session, name } => {
            let sink = s.panes.lock().get(&session).map(|p| p.sink.clone());
            if let Some(sink) = sink {
                sink.event(PaneEvent::Process(name));
            }
        }
        ServerMsg::Pong { nonce } => {
            s.rtt_us.store(s.now_us().saturating_sub(nonce).max(1), Ordering::Relaxed);
        }
        ServerMsg::Sessions { req, .. }
        | ServerMsg::Ok { req }
        | ServerMsg::HostSettings { req, .. }
        | ServerMsg::HostSettingsApplied { req, .. }
        | ServerMsg::Generations { req, .. }
        | ServerMsg::ShellsReloaded { req, .. }
        | ServerMsg::Plugin { req, .. }
        | ServerMsg::Layout { req, .. } => {
            if let Some(Pending::Reply(tx) | Pending::Attach { tx, .. }) = s.pending.lock().remove(&req) {
                let _ = tx.send(msg);
            }
        }
        ServerMsg::Error { req: Some(req), ref message } => {
            let pending = s.pending.lock().remove(&req);
            match pending {
                Some(Pending::Reply(tx) | Pending::Attach { tx, .. }) => {
                    let _ = tx.send(msg);
                }
                Some(Pending::Reattach(session)) => {
                    if let Some(p) = s.panes.lock().remove(&session) {
                        p.sink.event(PaneEvent::Lost(message.clone()));
                    }
                }
                None => {}
            }
        }
        ServerMsg::Error { req: None, message } => log::error!("goattyd: {message}"),
        ServerMsg::Hello { .. } => log::warn!("unexpected second Hello"),
    }
}
