//! One shell session: its PTY, its authoritative terminal, and the
//! connections subscribed to its output (with per-subscriber flow control).

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use goatty_proto::{Frame, ReqId, ServerMsg, SessionId, SessionInfo, StreamOpts, WinSize, encode};
use goatty_term::pty::Pty;
use goatty_term::{TermSize, Terminal};
use parking_lot::Mutex;

/// One connection's subscription to one session.
///
/// Flow control: `sent` counts stream bytes queued for this subscriber,
/// `acked` what the client reported as consumed. When `sent - acked` would
/// exceed `window`, the subscriber goes `lagging`: raw output is skipped
/// (counted in `skipped`) until the client has consumed everything, then it
/// gets one `Resync` snapshot of the current screen and streaming resumes.
pub(crate) struct Subscriber {
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

pub(crate) struct Session {
    pub(crate) id: SessionId,
    pub(crate) term: Terminal,
    pub(crate) pty: Mutex<Pty>,
    /// Held while feeding + broadcasting, so taking a snapshot and
    /// (un)subscribing are atomic with respect to the output stream: no gap,
    /// no overlap.
    pub(crate) subs: Mutex<Vec<Arc<Subscriber>>>,
    pub(crate) created_unix: u64,
}

impl Session {
    pub(crate) fn info(&self) -> SessionInfo {
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

    pub(crate) fn resize(&self, ws: WinSize) {
        let size = to_size(ws);
        if self.term.size() != (size.cols, size.rows) {
            self.term.resize(size);
        }
        if let Err(e) = self.pty.lock().resize(size) {
            log::warn!("session {}: resize failed: {e}", self.id);
        }
    }

    /// Snapshot + subscribe under one lock. Returns the subscriber.
    pub(crate) fn subscribe(&self, conn: u64, req: ReqId, opts: StreamOpts, tx: &flume::Sender<Frame>) -> Arc<Subscriber> {
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

    pub(crate) fn unsubscribe(&self, conn: u64) {
        self.subs.lock().retain(|s| s.conn != conn);
    }

    /// Client acknowledged `consumed` bytes. If it was lagging and has now
    /// caught up completely, send the current screen and resume streaming.
    pub(crate) fn ack(&self, sub: &Subscriber, consumed: u64) {
        sub.acked.fetch_max(consumed, Ordering::AcqRel);
        if sub.lagging.load(Ordering::Acquire) && sub.in_flight() == 0 {
            let _subs = self.subs.lock(); // order against the output pump
            if sub.lagging.load(Ordering::Acquire) && sub.in_flight() == 0 {
                self.resync(sub);
            }
        }
    }

    /// Caller holds `subs`.
    pub(crate) fn resync(&self, sub: &Subscriber) {
        let snapshot = self.term.resync_snapshot();
        let skipped = sub.skipped.swap(0, Ordering::AcqRel);
        let len = snapshot.len();
        if let Ok(f) = encode(&ServerMsg::Resync { session: self.id, skipped, snapshot }) {
            sub.push(f, len);
        }
        sub.lagging.store(false, Ordering::Release);
    }
    /// The shell exited: let every subscriber see the final screen, then
    /// `Exited`. Called once by the reader thread.
    pub(crate) fn finish(&self, code: Option<u32>) {
        let exited = encode(&ServerMsg::Exited { session: self.id, code }).ok();
        for sub in self.subs.lock().drain(..) {
            // Let lagging clients see the final screen before the end.
            if sub.lagging.load(Ordering::Acquire) {
                self.resync(&sub);
            }
            if let Some(f) = &exited {
                let _ = sub.tx.send(f.clone());
            }
        }
    }
}

pub(crate) fn to_size(ws: WinSize) -> TermSize {
    TermSize { cols: ws.cols.max(2), rows: ws.rows.max(1), cell_width: ws.cell_width, cell_height: ws.cell_height }
}

/// Read PTY output until EOF, feeding the authoritative terminal and every
/// subscriber. Runs on the session's reader thread.
pub(crate) fn pump_output(s: &Session, mut reader: Box<dyn Read + Send>) {
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
