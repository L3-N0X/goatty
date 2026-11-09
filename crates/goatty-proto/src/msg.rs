//! Message types. Any change here is a wire change: bump [`PROTOCOL_VERSION`].
//!
//! # Compatibility
//!
//! Peers support a *range* of protocol versions, [`MIN_PROTOCOL_VERSION`]
//! ..= [`PROTOCOL_VERSION`], and agree on one in the handshake:
//!
//! 1. The client sends `Hello { version: its newest }`.
//! 2. The daemon answers `Hello { version: v }` with
//!    `v = min(client, daemon newest)` ([`negotiate`]). If `v` is older than
//!    the daemon supports, it answers with its own newest version instead and
//!    closes the connection, so the client can tell "daemon too new" from
//!    "daemon too old".
//! 3. Both sides then use `v`: neither sends a message newer than `v`.
//!
//! That only works if the wire format grows **append-only**: new messages
//! are new variants at the *end* of [`ClientMsg`]/[`ServerMsg`] (postcard
//! encodes variants by index), existing variants never change their fields,
//! and each addition bumps [`PROTOCOL_VERSION`] and notes `vN:` in its doc.
//! A change that can't be made that way raises [`MIN_PROTOCOL_VERSION`].
//!
//! Daemons up to v5 required an exact match and answer any other version
//! with `Error { message: "protocol mismatch: daemon vN, client vM" }`. The
//! client recognizes that and reconnects announcing `vN` when it supports it.

use serde::{Deserialize, Serialize};

/// The newest protocol version this build speaks.
pub const PROTOCOL_VERSION: u32 = 6;

/// The oldest protocol version this build still speaks.
pub const MIN_PROTOCOL_VERSION: u32 = 5;

/// v6: `ClientMsg::Run` (a session running a given command).
pub const RUN_SINCE: u32 = 6;

/// The version to use with a peer whose newest version is `peer`, or
/// `None` when the ranges don't overlap.
pub fn negotiate(peer: u32) -> Option<u32> {
    let v = peer.min(PROTOCOL_VERSION);
    (v >= MIN_PROTOCOL_VERSION).then_some(v)
}

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
/// mismatch and report it instead of misparsing. Everything else is
/// append-only (see the module docs).
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
    /// The host's settings document (`host.json`). Reply: `HostSettings`.
    GetHostSettings {
        req: ReqId,
    },
    /// Replace the host settings with `json` (a settings document). The
    /// daemon validates it and builds the shell configuration first; with
    /// `apply` it then saves the file and activates the new generation,
    /// without it only reports issues and the diff. Reply: `HostSettingsApplied`.
    SetHostSettings {
        req: ReqId,
        json: String,
        apply: bool,
    },
    /// Reply: `Generations`.
    ListGenerations {
        req: ReqId,
    },
    /// Activate generation `generation` (default: the previous one) and
    /// restore its settings. Reply: `HostSettings`.
    Rollback {
        req: ReqId,
        generation: Option<u32>,
    },
    /// The client's colors for this session, so the daemon answers color
    /// queries (OSC 4/10/11) with what the user actually sees: 16 ANSI
    /// colors, then fg, bg, cursor, selection fg, selection bg (`0xRRGGBB`).
    SetPalette {
        session: SessionId,
        colors: Vec<u32>,
    },
    /// Ask the running managed shells to restart with the active
    /// generation (each at its next idle prompt). Reply: `ShellsReloaded`.
    ReloadShells {
        req: ReqId,
    },
    /// v5: a plugin operation (`goatty_plugin::api::Request` as JSON:
    /// catalog, plan, install, uninstall). Answered off the connection's
    /// reader thread, since installs download. Reply: `Plugin`.
    Plugin {
        req: ReqId,
        request: String,
    },
    /// v6: like `Create`, but the session runs `argv` (looked up in the
    /// daemon's `PATH`) instead of the user's shell, e.g. `ssh host` for a
    /// plain SSH session. The session ends when the program exits.
    Run {
        req: ReqId,
        size: WinSize,
        cwd: Option<String>,
        argv: Vec<String>,
        opts: StreamOpts,
    },
}

/// One changed file of the generated shell configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDiff {
    pub name: String,
    pub unified: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationInfo {
    pub number: u32,
    pub created_unix: u64,
    pub active: bool,
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
    HostSettings {
        req: ReqId,
        /// The settings file as stored (non-default values only).
        json: String,
        /// Problems found loading it (goatty used fallbacks).
        issues: Vec<String>,
        /// `GOATTY_*` variables overriding settings in the daemon's environment.
        env_overrides: Vec<String>,
        /// The active generation.
        generation: Option<u32>,
    },
    HostSettingsApplied {
        req: ReqId,
        issues: Vec<String>,
        /// Changes to the generated files relative to the active generation
        /// (computed before applying).
        diff: Vec<FileDiff>,
        applied: bool,
        generation: Option<u32>,
    },
    Generations {
        req: ReqId,
        list: Vec<GenerationInfo>,
    },
    ShellsReloaded {
        req: ReqId,
        /// How many shells were asked.
        count: u32,
    },
    /// v5: `goatty_plugin::api::Reply` as JSON.
    Plugin {
        req: ReqId,
        reply: String,
    },
}

impl ServerMsg {
    /// The request this message answers, if it's a reply.
    pub fn reply_to(&self) -> Option<ReqId> {
        match self {
            ServerMsg::Sessions { req, .. }
            | ServerMsg::Attached { req, .. }
            | ServerMsg::Ok { req }
            | ServerMsg::HostSettings { req, .. }
            | ServerMsg::HostSettingsApplied { req, .. }
            | ServerMsg::Generations { req, .. }
            | ServerMsg::ShellsReloaded { req, .. }
            | ServerMsg::Plugin { req, .. } => Some(*req),
            ServerMsg::Error { req, .. } => *req,
            _ => None,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_picks_the_newest_common_version() {
        assert_eq!(negotiate(PROTOCOL_VERSION), Some(PROTOCOL_VERSION));
        assert_eq!(negotiate(PROTOCOL_VERSION + 3), Some(PROTOCOL_VERSION));
        assert_eq!(negotiate(MIN_PROTOCOL_VERSION), Some(MIN_PROTOCOL_VERSION));
        assert_eq!(negotiate(MIN_PROTOCOL_VERSION - 1), None);
    }

    #[test]
    fn hello_layout_is_frozen() {
        // Variant 0, version first: what a peer of any version decodes.
        let bytes = postcard::to_allocvec(&ClientMsg::Hello { version: 9, client: "x".into() }).unwrap();
        assert_eq!(&bytes[..2], &[0, 9]);
        let bytes = postcard::to_allocvec(&ServerMsg::Hello { version: 9, daemon_pid: 1, host: HostInfo::default() }).unwrap();
        assert_eq!(&bytes[..2], &[0, 9]);
    }

    #[test]
    fn v5_messages_keep_their_variant_index() {
        // A v5 peer must still decode everything it knows.
        let bytes = postcard::to_allocvec(&ClientMsg::Plugin { req: 1, request: String::new() }).unwrap();
        assert_eq!(bytes[0], 18);
        let bytes = postcard::to_allocvec(&ServerMsg::Plugin { req: 1, reply: String::new() }).unwrap();
        assert_eq!(bytes[0], 13);
        // v6 additions come after them.
        let size = WinSize { cols: 1, rows: 1, cell_width: 1, cell_height: 1 };
        let run = ClientMsg::Run { req: 1, size, cwd: None, argv: vec![], opts: StreamOpts::LOCAL };
        assert_eq!(postcard::to_allocvec(&run).unwrap()[0], 19);
    }
}
