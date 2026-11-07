//! Message types. Any change here is a wire change: bump [`PROTOCOL_VERSION`].

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 3;

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
            | ServerMsg::Generations { req, .. } => Some(*req),
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
