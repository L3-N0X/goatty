//! Blocking request/response access to a local goattyd, for CLIs and
//! scripts. One request at a time; no threads, no reconnect, and unlike
//! [`crate::Connection`] it never starts a daemon.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use goatty_proto::{ClientMsg, HostInfo, PROTOCOL_VERSION, ReqId, ServerMsg, SessionId, SessionInfo, encode, read_msg};

use crate::handshake::{self, Outcome};

pub struct Control {
    stream: UnixStream,
    host: HostInfo,
    pid: u32,
    next_req: ReqId,
}

/// The pid of the process listening on the Unix socket at `path`
/// (`SO_PEERCRED`), whatever protocol it speaks. Only processes of this
/// user count (the socket directory is 0700 anyway).
pub fn socket_owner(path: &Path) -> Result<u32> {
    use std::os::fd::AsRawFd;
    let stream = UnixStream::connect(path).with_context(|| format!("nothing is listening on {}", path.display()))?;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for writes of the sizes passed, and
    // the fd stays open for the duration of the call.
    let rc = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &raw mut len) };
    if rc != 0 {
        bail!("SO_PEERCRED: {}", std::io::Error::last_os_error());
    }
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    if cred.uid != me || cred.pid <= 0 {
        bail!("{} belongs to another user (uid {})", path.display(), cred.uid);
    }
    Ok(cred.pid as u32)
}

impl Control {
    /// Connect to the daemon socket at `path` and perform the handshake.
    pub fn connect(path: &Path, client: &str) -> Result<Self> {
        let mut announce = PROTOCOL_VERSION;
        loop {
            let mut stream =
                UnixStream::connect(path).with_context(|| format!("goattyd is not running (no socket at {})", path.display()))?;
            stream.write_all(&handshake::hello(client, announce)?)?;
            match handshake::read_greeting(&mut stream, &path.display().to_string(), announce)? {
                Outcome::Ok(g) => return Ok(Self { stream, host: g.host, pid: g.pid, next_req: 1 }),
                // An old exact-match daemon: speak its version (only ever lower).
                Outcome::RetryWith(v) => announce = v,
            }
        }
    }

    /// Connect to this user's daemon (`goatty_proto::socket_path`).
    pub fn connect_local(client: &str) -> Result<Self> {
        Self::connect(&goatty_proto::socket_path(), client)
    }

    pub fn host(&self) -> &HostInfo {
        &self.host
    }

    /// The daemon's pid.
    pub fn pid(&self) -> u32 {
        self.pid
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

    /// A plugin operation (`goatty_plugin::api::Request` as JSON); returns
    /// the reply JSON.
    pub fn plugin(&mut self, request: String) -> Result<String> {
        match self.request(|req| ClientMsg::Plugin { req, request })? {
            ServerMsg::Plugin { reply, .. } => Ok(reply),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Stop the daemon and every session it owns.
    pub fn shutdown(&mut self) -> Result<()> {
        self.request(|req| ClientMsg::Shutdown { req }).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_owner_is_the_listener() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert_eq!(socket_owner(&path).unwrap(), std::process::id());
        assert!(socket_owner(&dir.path().join("none.sock")).is_err());
    }
}
