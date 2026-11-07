//! Client side of the goatty protocol, independent of any UI.
//!
//! * [`Control`] is a blocking request/response client for CLIs and scripts
//!   (`goattyd list`). It never starts a daemon and never reconnects.
//! * [`Connection`] is one link to one goattyd (local or remote) carrying any
//!   number of attached sessions ("panes"). It owns three threads:
//!
//!   * **reader**: decodes frames, feeds each pane's [`PaneSink`], sends acks,
//!     and transparently reconnects + reattaches when the link drops,
//!   * **writer**: the only thread that writes to the transport, so the UI
//!     thread never blocks on a congested or dead link,
//!   * **heartbeat** (remote only): measures RTT and tears down links that
//!     have gone silent, which triggers the reconnect.
//! * [`transport`] turns an [`Endpoint`] (local socket, `ssh host`, any
//!   stdio command) into a byte stream.

mod connection;
pub mod control;
mod handshake;
pub mod transport;

pub use connection::Connection;
pub use control::Control;
pub use transport::{Endpoint, connect_or_spawn_daemon};

/// Receives one session's terminal stream. Called on the connection's
/// reader thread; keep it fast (feed a VT, wake the UI).
pub trait PaneSink: Send + Sync + 'static {
    /// VT bytes in stream order: attach snapshots, live output and resync
    /// snapshots alike. Feeding them into a terminal reproduces the session.
    fn feed(&self, bytes: &[u8]);
    fn event(&self, ev: PaneEvent);
}

#[derive(Debug, Clone)]
pub enum PaneEvent {
    /// Flow control skipped `skipped` bytes; the screen is current again.
    Resynced { skipped: u64 },
    /// The link dropped; the session keeps running on the host.
    Disconnected(String),
    /// Reattached after a reconnect (a full snapshot was fed).
    Reconnected,
    /// The session's process exited.
    Exited(Option<u32>),
    /// The session can't be reattached (daemon restarted, or gave up).
    Lost(String),
}
