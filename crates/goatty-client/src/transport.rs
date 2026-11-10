//! Byte-stream transports to a goattyd.
//!
//! Every remote transport has the same shape: run a command whose stdio is
//! piped to `goattyd proxy` on the target host. The proxy connects to (or
//! starts) that host's daemon and relays bytes. This one mechanism covers:
//!
//! * `ssh host` — reuses `~/.ssh/config`, agent, keys, ProxyJump,
//!   ControlMaster, known_hosts and Tailscale SSH, with no SSH code in goatty,
//! * `docker exec -i box`, `kubectl exec -i pod --`, `distrobox enter`, …,
//! * the local proxy itself (used by tests to cover the remote path end to end).
//!
//! The daemon socket stays private to its host (0600, 0700 dir), so goatty
//! never opens a network port.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use goatty_proto::StreamOpts;
use parking_lot::{Condvar, Mutex};

use crate::remote;

/// Where a goattyd lives and how to reach it.
#[derive(Debug, Clone)]
pub enum Endpoint {
    /// The daemon on this machine (auto-started if needed).
    Local,
    /// goattyd on another host, through the system's `ssh`.
    Ssh(SshTarget),
    /// `program args…` speaks the protocol on its stdio (normally it ends in
    /// `goattyd proxy`).
    Command { label: String, program: String, args: Vec<String> },
}

/// A host reached with the system's OpenSSH client.
#[derive(Debug, Clone)]
pub struct SshTarget {
    /// Anything ssh accepts: `user@host`, an alias from `~/.ssh/config`, a
    /// Tailscale MagicDNS name.
    pub host: String,
    /// goattyd's path on the host, instead of searching for it.
    pub remote_daemon: Option<String>,
    /// Program for ssh's prompts (passphrases, host keys) instead of a
    /// terminal: `SSH_ASKPASS` with `SSH_ASKPASS_REQUIRE=force`. The GUI
    /// passes itself (see [`crate::askpass`]).
    pub askpass: Option<PathBuf>,
    /// The ssh client to run (tests substitute a fake).
    pub program: String,
}

impl SshTarget {
    pub fn new(host: &str) -> Self {
        Self { host: host.into(), remote_daemon: None, askpass: None, program: "ssh".into() }
    }

    /// `ssh … host <remote>`: no remote PTY (we carry raw bytes), no `~`
    /// escapes, keepalives so a dead link is noticed, prompts through the
    /// askpass program when there is one. `remote` runs in the user's login
    /// shell on the host, so it must be valid there (we wrap ours in `sh -c`).
    pub fn command(&self, remote: &str) -> Command {
        self.command_with(remote, true)
    }

    /// Like [`Self::command`]. Not `interactive`: `BatchMode=yes`, so ssh
    /// fails instead of asking for a password or passphrase (automatic
    /// reconnects must not pop up a prompt on every attempt).
    pub fn command_with(&self, remote: &str, interactive: bool) -> Command {
        let mut c = Command::new(&self.program);
        if !interactive {
            c.args(["-o", "BatchMode=yes"]);
        }
        c.args(["-T", "-e", "none", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3", "-o", "ConnectTimeout=10"]);
        // `--`: a host that starts with `-` is a host, not an option.
        c.arg("--").arg(&self.host).arg(remote);
        if let Some(askpass) = &self.askpass {
            c.env("SSH_ASKPASS", askpass).env("SSH_ASKPASS_REQUIRE", "force").env(crate::askpass::ENV, "1");
        }
        c
    }
}

pub(crate) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Endpoint {
    /// goattyd on `host` via `ssh host … goattyd proxy`.
    pub fn ssh(host: &str, remote_daemon: Option<&str>) -> Self {
        Endpoint::Ssh(SshTarget { remote_daemon: remote_daemon.map(String::from), ..SshTarget::new(host) })
    }

    /// An arbitrary command line, split on whitespace (prototype-grade; use
    /// [`Endpoint::Command`] directly for arguments containing spaces).
    pub fn command_line(cmdline: &str) -> Result<Self> {
        let mut parts = cmdline.split_whitespace().map(String::from);
        let program = parts.next().context("empty --via command")?;
        Ok(Endpoint::Command { label: program.clone(), program, args: parts.collect() })
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Endpoint::Local)
    }

    pub fn label(&self) -> &str {
        match self {
            Endpoint::Local => "local",
            Endpoint::Ssh(t) => &t.host,
            Endpoint::Command { label, .. } => label,
        }
    }

    /// Stream parameters suited to this kind of link.
    pub fn default_opts(&self) -> StreamOpts {
        if self.is_local() { StreamOpts::LOCAL } else { StreamOpts::REMOTE }
    }

    /// `interactive: false` for automatic retries: ssh never prompts.
    pub fn connect(&self, interactive: bool) -> Result<Transport> {
        match self {
            Endpoint::Local => {
                let s = connect_or_spawn_daemon(None)?;
                let w = s.try_clone()?;
                let closer = s.try_clone()?;
                Ok(Transport {
                    reader: BufReader::with_capacity(256 << 10, Box::new(s)),
                    writer: Box::new(w),
                    closer: Closer(Arc::new(Mutex::new(Some(CloseHandle::Socket(closer))))),
                    stderr: StderrTail::default(),
                })
            }
            Endpoint::Ssh(t) => {
                let script = remote::proxy_script(t.remote_daemon.as_deref());
                spawn_stdio(t.command_with(&format!("sh -c {}", sh_quote(&script)), interactive), &t.host)
            }
            Endpoint::Command { program, args, label } => {
                let mut c = Command::new(program);
                c.args(args);
                spawn_stdio(c, label)
            }
        }
    }
}

/// Run `cmd` with the protocol on its stdin/stdout. Its stderr (ssh errors,
/// the remote script's messages) is logged and kept for error reports.
/// ssh asks for passphrases on `/dev/tty` or through askpass, never on
/// stderr, so capturing it doesn't hide prompts.
fn spawn_stdio(mut cmd: Command, label: &str) -> Result<Transport> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("couldn't run {program}"))?;
    let stdin = child.stdin.take().context("child stdin")?;
    let stdout = child.stdout.take().context("child stdout")?;
    let stderr = child.stderr.take().map(|e| StderrTail::spawn(e, label)).unwrap_or_default();
    Ok(Transport {
        reader: BufReader::with_capacity(256 << 10, Box::new(stdout)),
        writer: Box::new(stdin),
        closer: Closer(Arc::new(Mutex::new(Some(CloseHandle::Child(child))))),
        stderr,
    })
}

/// The last lines a transport command wrote to stderr.
#[derive(Clone, Default)]
pub struct StderrTail(Arc<(Mutex<TailState>, Condvar)>);

#[derive(Default)]
struct TailState {
    lines: VecDeque<String>,
    done: bool,
}

const TAIL_LINES: usize = 40;

impl StderrTail {
    fn spawn(stderr: ChildStderr, label: &str) -> Self {
        let tail = Self::default();
        let t = tail.clone();
        let label = label.to_string();
        let spawned = std::thread::Builder::new().name("transport-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                log::info!("{label}: {line}");
                let mut s = t.0.0.lock();
                if s.lines.len() == TAIL_LINES {
                    s.lines.pop_front();
                }
                s.lines.push_back(line);
            }
            t.0.0.lock().done = true;
            t.0.1.notify_all();
        });
        if spawned.is_err() {
            tail.0.0.lock().done = true;
        }
        tail
    }

    /// The lines so far.
    pub fn lines(&self) -> Vec<String> {
        self.0.0.lock().lines.iter().cloned().collect()
    }

    /// The lines once the command closed stderr (it exited), or after
    /// `timeout`.
    pub fn wait(&self, timeout: Duration) -> Vec<String> {
        let deadline = Instant::now() + timeout;
        let mut s = self.0.0.lock();
        while !s.done {
            if self.0.1.wait_until(&mut s, deadline).timed_out() {
                break;
            }
        }
        s.lines.iter().cloned().collect()
    }
}

enum CloseHandle {
    Socket(UnixStream),
    Child(Child),
}

/// Tears a transport down from any thread, unblocking its reader.
#[derive(Clone)]
pub struct Closer(Arc<Mutex<Option<CloseHandle>>>);

impl Closer {
    pub fn close(&self) {
        match self.0.lock().take() {
            Some(CloseHandle::Socket(s)) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            Some(CloseHandle::Child(mut c)) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            None => {}
        }
    }
}

pub struct Transport {
    /// Buffered so the client can tell when it has drained everything the
    /// transport delivered (that's when it acknowledges).
    pub reader: BufReader<Box<dyn Read + Send>>,
    pub writer: Box<dyn Write + Send>,
    pub closer: Closer,
    /// What the transport command said on stderr (empty for sockets).
    pub stderr: StderrTail,
}

/// Connect to the local goattyd, starting it if needed: through systemd
/// when its user units are installed ([`crate::service`]), else detached in
/// its own session. `daemon_exe` defaults to `goattyd` next to the current
/// binary.
pub fn connect_or_spawn_daemon(daemon_exe: Option<PathBuf>) -> Result<UnixStream> {
    let path = goatty_proto::socket_path();
    if let Ok(s) = UnixStream::connect(&path) {
        return Ok(s);
    }
    let dir = goatty_proto::runtime_dir();
    let log_path = dir.join("goattyd.log");
    // Only for the socket the units listen on (not a GOATTY_SOCKET sandbox).
    let via_systemd = crate::service::unit_socket_path().is_some_and(|p| p == path) && crate::service::installed() && {
        match crate::service::start_socket() {
            Ok(()) => true,
            Err(e) => {
                log::warn!("couldn't start goattyd's systemd socket, starting it directly: {e:#}");
                false
            }
        }
    };
    if !via_systemd {
        spawn_daemon(daemon_exe, &dir, &log_path)?;
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(s) = UnixStream::connect(&path) {
            return Ok(s);
        }
        if Instant::now() > deadline {
            if via_systemd {
                bail!("goattyd did not come up; see `journalctl --user -u goattyd`");
            }
            bail!("goattyd did not come up; see {}", log_path.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_daemon(daemon_exe: Option<PathBuf>, dir: &std::path::Path, log_path: &std::path::Path) -> Result<()> {
    let daemon = match daemon_exe {
        Some(p) => p,
        None => {
            let exe = std::env::current_exe()?;
            let d = exe.with_file_name("goattyd");
            if !d.exists() {
                bail!("goattyd not found next to {} (build it with `cargo build -p goatty-daemon`)", exe.display());
            }
            d
        }
    };
    std::fs::create_dir_all(dir)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(log_path)?;
    log::info!("starting {}", daemon.display());
    let mut cmd = Command::new(&daemon);
    cmd.arg("serve").stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    // SAFETY: setsid is async-signal-safe; detaches goattyd from our
    // session so it survives the GUI, its launching terminal, or the SSH
    // session that started it.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().context("spawn goattyd")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_command_is_safe() {
        let mut t = SshTarget::new("-oProxyCommand=evil");
        t.askpass = Some("/opt/goatty".into());
        let c = t.command("true");
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let host = args.iter().position(|a| a == "-oProxyCommand=evil").unwrap();
        assert_eq!(args[host - 1], "--", "the host can't be read as an option");
        assert_eq!(args.last().unwrap(), "true");
        let envs: Vec<_> =
            c.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert!(envs.contains(&("SSH_ASKPASS_REQUIRE".into(), Some("force".into()))));
        assert!(envs.contains(&("SSH_ASKPASS".into(), Some("/opt/goatty".into()))));
        assert!(!args.contains(&"BatchMode=yes".to_string()));
        let batch: Vec<_> = t.command_with("true", false).get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(batch.contains(&"BatchMode=yes".to_string()));
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn stderr_tail_keeps_the_last_lines() {
        let mut child = Command::new("sh")
            .args(["-c", "i=0; while [ $i -lt 50 ]; do echo line$i >&2; i=$((i+1)); done"])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let tail = StderrTail::spawn(child.stderr.take().unwrap(), "test");
        let lines = tail.wait(Duration::from_secs(5));
        child.wait().unwrap();
        assert_eq!(lines.len(), TAIL_LINES);
        assert_eq!(lines.last().unwrap(), "line49");
    }
}
