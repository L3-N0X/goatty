//! Blocking request/response access to a local goattyd, for CLIs and
//! scripts. One request at a time; no threads, no reconnect, and unlike
//! [`crate::Connection`] it never starts a daemon.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use goatty_proto::{ClientMsg, HostInfo, ReqId, ServerMsg, SessionId, SessionInfo, encode, read_msg};

use crate::handshake;

pub struct Control {
    stream: UnixStream,
    host: HostInfo,
    next_req: ReqId,
}

impl Control {
    /// Connect to the daemon socket at `path` and perform the handshake.
    pub fn connect(path: &Path, client: &str) -> Result<Self> {
        let mut stream = UnixStream::connect(path).with_context(|| format!("goattyd is not running (no socket at {})", path.display()))?;
        stream.write_all(&handshake::hello(client)?)?;
        let (host, _pid) = handshake::read_greeting(&mut stream, &path.display().to_string())?;
        Ok(Self { stream, host, next_req: 1 })
    }

    /// Connect to this user's daemon (`goatty_proto::socket_path`).
    pub fn connect_local(client: &str) -> Result<Self> {
        Self::connect(&goatty_proto::socket_path(), client)
    }

    pub fn host(&self) -> &HostInfo {
        &self.host
    }

    /// Send one request and wait for its reply. A daemon `Error` becomes `Err`.
    pub fn request(&mut self, msg: impl FnOnce(ReqId) -> ClientMsg) -> Result<ServerMsg> {
        let req = self.next_req;
        self.next_req += 1;
        self.stream.write_all(&encode(&msg(req))?)?;
        loop {
            let reply: ServerMsg = read_msg(&mut self.stream).context("goattyd closed the connection")?;
            match reply {
                ServerMsg::Error { req: r, message } if r.is_none() || r == Some(req) => return Err(anyhow!("goattyd: {message}")),
                _ if reply.reply_to() == Some(req) => return Ok(reply),
                _ => continue, // not ours (e.g. a late stream frame)
            }
        }
    }

    pub fn list_sessions(&mut self) -> Result<Vec<SessionInfo>> {
        match self.request(|req| ClientMsg::ListSessions { req })? {
            ServerMsg::Sessions { list, .. } => Ok(list),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub fn kill(&mut self, session: SessionId) -> Result<()> {
        self.request(|req| ClientMsg::Kill { req, session }).map(drop)
    }

    /// Type `data` into a session without attaching to it.
    pub fn send_input(&mut self, session: SessionId, data: Vec<u8>) -> Result<()> {
        self.request(|req| ClientMsg::SendInput { req, session, data }).map(drop)
    }

    /// The host settings document (`host.json` as the daemon sees it).
    pub fn host_settings_json(&mut self) -> Result<String> {
        match self.request(|req| ClientMsg::GetHostSettings { req })? {
            ServerMsg::HostSettings { json, .. } => Ok(json),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Validate, activate and save host settings. Returns the issues found.
    pub fn apply_host_settings(&mut self, json: String) -> Result<Vec<String>> {
        match self.request(|req| ClientMsg::SetHostSettings { req, json, apply: true })? {
            ServerMsg::HostSettingsApplied { issues, .. } => Ok(issues),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Ask the running managed shells to reload. Returns how many were asked.
    pub fn reload_shells(&mut self) -> Result<u32> {
        match self.request(|req| ClientMsg::ReloadShells { req })? {
            ServerMsg::ShellsReloaded { count, .. } => Ok(count),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Stop the daemon and every session it owns.
    pub fn shutdown(&mut self) -> Result<()> {
        self.request(|req| ClientMsg::Shutdown { req }).map(drop)
    }
}
