//! One client connection: decode requests, manage its subscriptions, and
//! write replies and stream frames through a dedicated writer thread.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use goatty_proto::{ClientMsg, Frame, PROTOCOL_VERSION, ServerMsg, SessionId, encode, read_msg};

use crate::server::Daemon;
use crate::session::{Session, Subscriber};

/// Serve one client connection until it closes. Its subscriptions end with
/// it; the sessions keep running.
pub(crate) fn handle(daemon: Arc<Daemon>, stream: UnixStream) -> Result<()> {
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
                    reply(ServerMsg::Hello { version: PROTOCOL_VERSION, daemon_pid: std::process::id(), host: crate::server::host_info() });
                }
                ClientMsg::ListSessions { req } => reply(ServerMsg::Sessions { req, list: daemon.list() }),
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
                    reply(ServerMsg::Ok { req });
                    daemon.shutdown();
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
