//! goatty ⇄ goattyd wire protocol (v2).
//!
//! The protocol doesn't care where the daemon runs. Any reliable, ordered
//! byte stream works as a transport: the local Unix socket, `ssh host goattyd
//! proxy`, `docker exec … goattyd proxy`, and later TLS/QUIC.
//!
//! Framing: `u32` little-endian length prefix + postcard-encoded message.
//!
//! **One connection, many sessions.** A connection can attach to any number
//! of sessions. Streams are keyed by `SessionId`, so the daemon encodes each
//! output chunk *once* and shares the frame between every subscriber.
//! Requests that expect a reply carry a client-chosen `ReqId`.
//!
//! **Flow control is in the protocol, not the transport.** Over SSH, the
//! kernel, ssh and the TCP buffers can hold megabytes, so socket backpressure
//! tells the daemon nothing about what the client has *actually* drawn. The
//! client therefore acknowledges consumed stream bytes (`Ack`). When a
//! subscriber has more than its `window` of unacknowledged bytes, the daemon
//! stops sending raw output to it. Once the client catches up, it gets a
//! `Resync`: a VT snapshot of the *current* screen. A slow link skips
//! intermediate frames (like mosh) instead of queueing them without bound.
//! Locally the window is large enough that this never triggers, so local
//! sessions stay lossless.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 2;
const MAX_FRAME: usize = 64 * 1024 * 1024;

pub type SessionId = u64;
/// Client-chosen correlation id for request/response pairs.
pub type ReqId = u32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct WinSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

/// Per-subscription stream parameters, chosen by the client per connection
/// type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamOpts {
    /// Max unacknowledged stream bytes before the daemon switches this
    /// subscriber to skip-and-resync.
    pub window: u32,
    /// Max scrollback lines replayed in the attach snapshot.
    pub history: u32,
}

impl StreamOpts {
    /// Unix socket: effectively lossless and full history.
    pub const LOCAL: Self = Self { window: 8 << 20, history: 5_000 };
    /// SSH/network: bounded in-flight data, so Ctrl+C on a flood is visible
    /// within roughly one window's transfer time.
    pub const REMOTE: Self = Self { window: 256 << 10, history: 2_000 };
}

/// Compatibility rule: `Hello` stays variant 0 with `version` as its first
/// field in both enums forever, so peers of any version can detect a
/// mismatch and report it instead of misparsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello {
        version: u32,
        client: String,
    },
    ListSessions {
        req: ReqId,
    },
    /// Spawn a new shell session and subscribe this connection to it.
    /// `cwd` is a path *on the daemon's host*.
    Create {
        req: ReqId,
        size: WinSize,
        cwd: Option<String>,
        opts: StreamOpts,
    },
    /// Subscribe this connection to an existing session.
    Attach {
        req: ReqId,
        session: SessionId,
        size: WinSize,
        opts: StreamOpts,
    },
    Detach {
        session: SessionId,
    },
    Input {
        session: SessionId,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Resize {
        session: SessionId,
        size: WinSize,
    },
    /// Cumulative stream bytes (snapshot + output) consumed for `session`
    /// since this connection subscribed.
    Ack {
        session: SessionId,
        consumed: u64,
    },
    Kill {
        req: ReqId,
        session: SessionId,
    },
    /// Write input to a session without subscribing (scripting/tests).
    SendInput {
        req: ReqId,
        session: SessionId,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Ping {
        nonce: u64,
    },
    /// Stop the daemon (kills all sessions).
    Shutdown {
        req: ReqId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: SessionId,
    pub pid: Option<u32>,
    pub title: String,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub attached: u32,
    pub created_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostInfo {
    pub hostname: String,
    pub user: String,
    pub os: String,
    pub daemon_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    Hello {
        version: u32,
        daemon_pid: u32,
        host: HostInfo,
    },
    Sessions {
        req: ReqId,
        list: Vec<SessionInfo>,
    },
    /// Reply to Create/Attach. `snapshot` is a VT byte stream that rebuilds
    /// the current state in a *fresh* terminal (it starts with RIS). Live
    /// `Output` follows with no gap and no overlap.
    Attached {
        req: ReqId,
        session: SessionId,
        #[serde(with = "serde_bytes")]
        snapshot: Vec<u8>,
    },
    Output {
        session: SessionId,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// The subscriber fell behind and `skipped` bytes of output were not
    /// sent. `snapshot` brings an existing client terminal to the current
    /// state without resetting its scrollback.
    Resync {
        session: SessionId,
        skipped: u64,
        #[serde(with = "serde_bytes")]
        snapshot: Vec<u8>,
    },
    Exited {
        session: SessionId,
        code: Option<u32>,
    },
    Pong {
        nonce: u64,
    },
    Ok {
        req: ReqId,
    },
    Error {
        req: Option<ReqId>,
        message: String,
    },
}

impl ServerMsg {
    /// Stream payload size counted against the flow-control window.
    pub fn stream_len(&self) -> usize {
        match self {
            ServerMsg::Attached { snapshot, .. } | ServerMsg::Resync { snapshot, .. } => snapshot.len(),
            ServerMsg::Output { data, .. } => data.len(),
            _ => 0,
        }
    }
}

pub fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("goatty-{}", user_id())));
    base.join("goatty")
}

pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("GOATTY_SOCKET") {
        return PathBuf::from(p);
    }
    runtime_dir().join("goattyd.sock")
}

fn user_id() -> String {
    std::env::var("UID").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "user".into())
}

/// A complete, length-prefixed frame, ready to write. Shared between
/// subscribers so a broadcast costs one encode.
pub type Frame = Arc<[u8]>;

pub fn encode<M: Serialize>(msg: &M) -> io::Result<Frame> {
    let mut buf = vec![0u8; 4];
    let mut buf = postcard::to_extend(msg, std::mem::take(&mut buf)).map_err(io::Error::other)?;
    let len = (buf.len() - 4) as u32;
    buf[..4].copy_from_slice(&len.to_le_bytes());
    Ok(buf.into())
}

/// Encode and write one message (no flush; callers decide batching).
pub fn write_msg<W: Write, M: Serialize>(w: &mut W, msg: &M) -> io::Result<()> {
    w.write_all(&encode(msg)?)
}

pub fn read_msg<R: Read, M: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<M> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    postcard::from_bytes(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &ClientMsg::Input { session: 7, data: b"ls\r".to_vec() }).unwrap();
        write_msg(&mut buf, &ServerMsg::Exited { session: 3, code: Some(0) }).unwrap();
        let mut r = &buf[..];
        assert!(matches!(read_msg::<_, ClientMsg>(&mut r).unwrap(), ClientMsg::Input { session: 7, data } if data == b"ls\r"));
        assert!(matches!(read_msg::<_, ServerMsg>(&mut r).unwrap(), ServerMsg::Exited { session: 3, code: Some(0) }));
        assert!(r.is_empty());
    }

    #[test]
    fn bytes_are_compact() {
        // serde_bytes: a 64 KiB chunk costs only a few bytes of header.
        let f = encode(&ServerMsg::Output { session: 1, data: vec![b'x'; 65536] }).unwrap();
        assert!(f.len() < 65536 + 12, "{}", f.len());
    }
}
