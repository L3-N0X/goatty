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

mod codec;
mod msg;
mod paths;

pub use codec::{Frame, MAX_FRAME, encode, read_msg, write_msg};
pub use msg::*;
pub use paths::{runtime_dir, socket_path};
