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

use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use goatty_proto::StreamOpts;
use parking_lot::Mutex;

/// Where a goattyd lives and how to reach it.
#[derive(Debug, Clone)]
pub enum Endpoint {
    /// The daemon on this machine (auto-started if needed).
    Local,
    /// `program args…` speaks the protocol on its stdio (normally it ends in
    /// `goattyd proxy`).
    Command { label: String, program: String, args: Vec<String> },
}

/// Shell snippet run on the remote host. Non-interactive SSH sessions often
/// lack `~/.cargo/bin` / `~/.local/bin` in PATH, so look there too. Plain
/// POSIX sh, since the user's login shell may be fish.
fn remote_proxy_script(remote_daemon: Option<&str>) -> String {
    let candidates = match remote_daemon {
        Some(p) => sh_quote(p),
        None => "goattyd \"$HOME/.local/bin/goattyd\" \"$HOME/.cargo/bin/goattyd\"".into(),
    };
    format!(
        "for d in {candidates}; do command -v \"$d\" >/dev/null 2>&1 && exec \"$d\" proxy; done; \
         echo 'goatty: goattyd not found on remote host (install it or pass --remote-daemon PATH)' >&2; exit 127"
    )
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Endpoint {
    /// `ssh <host> goattyd proxy`. `host` is anything ssh accepts, including
    /// `user@host`, an alias from `~/.ssh/config` or a Tailscale MagicDNS name.
    pub fn ssh(host: &str, remote_daemon: Option<&str>) -> Self {
        let args = vec![
            "-T".into(), // no remote PTY: we carry raw protocol bytes
            "-e".into(),
            "none".into(), // no ~ escape processing on a binary stream
            "-o".into(),
            "ServerAliveInterval=10".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            host.into(),
            format!("sh -c {}", sh_quote(&remote_proxy_script(remote_daemon))),
        ];
        Endpoint::Command { label: host.into(), program: "ssh".into(), args }
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
            Endpoint::Command { label, .. } => label,
        }
    }

    /// Stream parameters suited to this kind of link.
    pub fn default_opts(&self) -> StreamOpts {
        if self.is_local() { StreamOpts::LOCAL } else { StreamOpts::REMOTE }
    }

    pub fn connect(&self) -> Result<Transport> {
        match self {
            Endpoint::Local => {
                let s = connect_or_spawn_daemon(None)?;
                let w = s.try_clone()?;
                let closer = s.try_clone()?;
                Ok(Transport {
                    reader: BufReader::with_capacity(256 << 10, Box::new(s)),
                    writer: Box::new(w),
                    closer: Closer(Arc::new(Mutex::new(Some(CloseHandle::Socket(closer))))),
                })
            }
            Endpoint::Command { program, args, .. } => {
                let mut child = Command::new(program)
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    // ssh prompts (host key, passphrase) and remote errors
                    // go to our stderr.
                    .stderr(Stdio::inherit())
                    .spawn()
                    .with_context(|| format!("spawn {program}"))?;
                let stdin = child.stdin.take().context("child stdin")?;
                let stdout = child.stdout.take().context("child stdout")?;
                Ok(Transport {
                    reader: BufReader::with_capacity(256 << 10, Box::new(stdout)),
                    writer: Box::new(stdin),
                    closer: Closer(Arc::new(Mutex::new(Some(CloseHandle::Child(child))))),
                })
            }
        }
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
}

/// Connect to the local goattyd, starting it (detached, in its own session)
/// if needed. `daemon_exe` defaults to `goattyd` next to the current binary.
pub fn connect_or_spawn_daemon(daemon_exe: Option<PathBuf>) -> Result<UnixStream> {
    let path = goatty_proto::socket_path();
    if let Ok(s) = UnixStream::connect(&path) {
        return Ok(s);
    }
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
    let dir = goatty_proto::runtime_dir();
    std::fs::create_dir_all(&dir)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("goattyd.log"))?;
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
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(s) = UnixStream::connect(&path) {
            return Ok(s);
        }
        if Instant::now() > deadline {
            bail!("goattyd did not come up; see {}", dir.join("goattyd.log").display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_script_is_posix_and_quoted() {
        let s = remote_proxy_script(Some("/opt/it's/goattyd"));
        assert!(s.contains("'/opt/it'\\''s/goattyd'"));
        let ssh = Endpoint::ssh("box", None);
        let Endpoint::Command { args, .. } = ssh else { panic!() };
        assert!(args.last().unwrap().starts_with("sh -c '"));
    }
}
