//! Remote hosts end to end, without a real SSH server: a fake `ssh` runs
//! the remote command locally under a sandboxed `$HOME`, so this covers what
//! goatty runs on a host exactly:
//!
//! * no goattyd → `RemoteProblem::NotInstalled` with the host's platform,
//! * `remote::install` uploads goattyd, the connect script prefers it, and
//!   the session works; `remote::stop_daemon` stops it again,
//! * a daemon speaking a protocol we don't → `RemoteProblem::Incompatible`,
//!   and `stop --force` refuses to kill a socket owner that isn't a goattyd,
//! * protocol negotiation with older clients, and `Run` sessions,
//! * systemd socket activation (when `systemd-socket-activate` exists).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use goatty_client::remote::{self, RemoteProblem};
use goatty_client::{Connection, Endpoint, PaneEvent, PaneSink, SshTarget};
use goatty_proto::{ClientMsg, HostInfo, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, ServerMsg, StreamOpts, WinSize, encode, read_msg};
use parking_lot::Mutex;

const SIZE: WinSize = WinSize { cols: 80, rows: 24, cell_width: 8, cell_height: 16 };

/// A fake host: its own `$HOME`, runtime dir and daemon socket, reached
/// through a fake `ssh` that drops ssh's options and runs the command with
/// `sh -c`, like sshd runs it through the login shell.
struct Host {
    dir: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = tempfile::Builder::new().prefix("goatty-host-").tempdir().unwrap();
        for d in ["home", "run", "bin"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        let ssh = dir.path().join("bin/ssh");
        let p = dir.path().display();
        let script = format!(
            r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in --) shift; break;; -e|-o) shift 2;; -*) shift;; *) break;; esac
done
shift
export HOME="{p}/home" XDG_RUNTIME_DIR="{p}/run" GOATTY_SOCKET="{p}/run/d.sock" GOATTY_SHELL=/bin/sh PATH=/usr/bin:/bin RUST_LOG=warn,rio_vt=error
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
exec sh -c "$*"
"#
        );
        // Write and chmod in a short-lived child process so the test binary
        // never holds a write fd to `bin/ssh`; otherwise a concurrent test
        // thread calling `fork()` can inherit that fd and trigger ETXTBSY.
        let mut child =
            Command::new("sh").args(["-c", "cat > \"$1\" && chmod 0755 \"$1\"", "sh"]).arg(&ssh).stdin(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success(), "writing fake ssh failed: {status}");
        Self { dir }
    }

    fn target(&self) -> SshTarget {
        SshTarget { program: self.dir.path().join("bin/ssh").display().to_string(), ..SshTarget::new("fakehost") }
    }

    fn sock(&self) -> PathBuf {
        self.dir.path().join("run/d.sock")
    }

    fn installed(&self) -> PathBuf {
        self.dir.path().join("home").join(remote::BIN_DIR).join(remote::binary_name())
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Stop a daemon the test left running (it was started detached).
        if self.installed().exists() {
            let _ = self.target().command(&format!("{} stop", self.installed().display())).stderr(Stdio::null()).status();
        }
    }
}

#[derive(Default)]
struct Collect {
    bytes: Mutex<Vec<u8>>,
    exited: Mutex<bool>,
}

impl PaneSink for Collect {
    fn feed(&self, bytes: &[u8]) {
        self.bytes.lock().extend_from_slice(bytes);
    }
    fn event(&self, ev: PaneEvent) {
        if let PaneEvent::Exited(_) = ev {
            *self.exited.lock() = true;
        }
    }
}

impl Collect {
    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&self.bytes.lock()).contains(needle) {
            assert!(Instant::now() < deadline, "no {needle:?} in {:?}", String::from_utf8_lossy(&self.bytes.lock()));
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn ssh_host_without_goattyd_gets_it_installed() {
    let host = Host::new();
    let endpoint = Endpoint::Ssh(host.target());

    let err = Connection::open(endpoint.clone(), "test").err().expect("no goattyd yet");
    let Some(RemoteProblem::NotInstalled { host: name, platform }) = err.downcast_ref::<RemoteProblem>() else { panic!("{err:#}") };
    assert_eq!(name, "fakehost");
    assert_eq!(platform.os, "Linux");

    remote::install(&host.target(), Path::new(env!("CARGO_BIN_EXE_goattyd"))).unwrap();
    assert!(host.installed().is_file());
    let mode = std::fs::metadata(host.installed()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    // Installing again replaces it in place.
    remote::install(&host.target(), Path::new(env!("CARGO_BIN_EXE_goattyd"))).unwrap();

    let conn = Connection::open(endpoint, "test").unwrap();
    assert_eq!(conn.protocol_version(), PROTOCOL_VERSION);
    let sink = Arc::new(Collect::default());
    let id = conn.create(SIZE, None, sink.clone()).unwrap();
    conn.input(id, b"echo remote-$((40+2))\r");
    sink.wait_for("remote-42");
    drop(conn);

    remote::stop_daemon(&host.target()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(host.sock()).is_ok() {
        assert!(Instant::now() < deadline, "daemon still running");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Nothing to stop is fine for `stop --force` (the update path runs it blindly).
    remote::stop_daemon(&host.target()).unwrap();
}

#[test]
fn an_upload_that_does_not_run_is_rejected() {
    let host = Host::new();
    let junk = host.dir.path().join("junk");
    std::fs::write(&junk, "#!/bin/sh\necho 'goattyd 0.0.0-wrong'\n").unwrap();
    let err = remote::install(&host.target(), &junk).unwrap_err();
    assert!(format!("{err:#}").contains("unexpected goattyd version"), "{err:#}");
    assert!(!host.installed().exists());
    // No temp files left behind.
    let dir = host.installed().parent().unwrap().to_path_buf();
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0);
}

/// A "daemon" that answers every greeting with protocol `version`.
fn fake_daemon(sock: &Path, version: u32) {
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { return };
            let _: Result<ClientMsg, _> = read_msg(&mut s);
            let host = HostInfo { daemon_version: "0.0.0-old".into(), ..Default::default() };
            let _ = s.write_all(&encode(&ServerMsg::Hello { version, daemon_pid: 1, host }).unwrap());
        }
    });
}

#[test]
fn an_incompatible_running_daemon_is_reported_and_not_killed_blindly() {
    let host = Host::new();
    remote::install(&host.target(), Path::new(env!("CARGO_BIN_EXE_goattyd"))).unwrap();
    fake_daemon(&host.sock(), MIN_PROTOCOL_VERSION - 1);

    let err = Connection::open(Endpoint::Ssh(host.target()), "test").err().expect("incompatible");
    let Some(RemoteProblem::Incompatible { mismatch, probe: Some(probe), .. }) = err.downcast_ref::<RemoteProblem>() else {
        panic!("{err:#}")
    };
    assert!(!mismatch.daemon_is_newer);
    assert_eq!(mismatch.daemon_version.as_deref(), Some("0.0.0-old"));
    assert!(probe.runs_ours(), "{probe:?}");

    // The socket's owner here is this test process, not a goattyd.
    let err = remote::stop_daemon(&host.target()).unwrap_err();
    assert!(format!("{err:#}").contains("not a goattyd"), "{err:#}");
}

fn start_daemon(dir: &Path) -> (Child, PathBuf) {
    let sock = dir.join("d.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_goattyd"))
        .env("GOATTY_SOCKET", &sock)
        .env("GOATTY_SHELL", "/bin/sh")
        .env("RUST_LOG", "warn,rio_vt=error")
        .env("HOME", dir)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(&sock).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    (child, sock)
}

fn hello(sock: &Path, version: u32) -> (UnixStream, ServerMsg) {
    let mut s = UnixStream::connect(sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(&encode(&ClientMsg::Hello { version, client: "test".into() }).unwrap()).unwrap();
    let reply = read_msg(&mut s).unwrap();
    (s, reply)
}

#[test]
fn daemon_negotiates_with_older_and_newer_clients() {
    let dir = tempfile::tempdir().unwrap();
    let (mut child, sock) = start_daemon(dir.path());
    // An older client in range gets its own version back (a v5 client
    // checks for an exact match).
    let (mut s, reply) = hello(&sock, MIN_PROTOCOL_VERSION);
    assert!(matches!(reply, ServerMsg::Hello { version, .. } if version == MIN_PROTOCOL_VERSION));
    s.write_all(&encode(&ClientMsg::ListSessions { req: 1 }).unwrap()).unwrap();
    assert!(matches!(read_msg::<_, ServerMsg>(&mut s).unwrap(), ServerMsg::Sessions { req: 1, .. }));
    // A newer client gets ours.
    let (_, reply) = hello(&sock, PROTOCOL_VERSION + 5);
    assert!(matches!(reply, ServerMsg::Hello { version, .. } if version == PROTOCOL_VERSION));
    // Too old: our newest version, then the daemon hangs up.
    let (mut s, reply) = hello(&sock, MIN_PROTOCOL_VERSION - 1);
    assert!(matches!(reply, ServerMsg::Hello { version, .. } if version == PROTOCOL_VERSION));
    assert!(read_msg::<_, ServerMsg>(&mut s).is_err());
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn run_sessions_start_the_given_program() {
    let dir = tempfile::tempdir().unwrap();
    let (mut child, sock) = start_daemon(dir.path());
    let endpoint = Endpoint::Command {
        label: "proxy".into(),
        program: "env".into(),
        args: vec![format!("GOATTY_SOCKET={}", sock.display()), env!("CARGO_BIN_EXE_goattyd").into(), "proxy".into()],
    };
    let conn = Connection::open(endpoint, "test").unwrap();
    let sink = Arc::new(Collect::default());
    let argv = ["sh", "-c", "echo ran-$((6*7))"].map(String::from).to_vec();
    conn.run(SIZE, None, argv, sink.clone()).unwrap();
    sink.wait_for("ran-42");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !*sink.exited.lock() {
        assert!(Instant::now() < deadline, "the session didn't end with its program");
        std::thread::sleep(Duration::from_millis(20));
    }
    let err = conn.run(SIZE, None, vec!["/nonexistent/program".into()], Arc::new(Collect::default())).unwrap_err();
    assert!(format!("{err:#}").contains("/nonexistent/program"), "{err:#}");
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn daemon_takes_its_socket_from_systemd() {
    let Ok(activate) = which("systemd-socket-activate") else {
        eprintln!("skipped: no systemd-socket-activate");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("act.sock");
    let mut child = Command::new(activate)
        .arg("-l")
        .arg(&sock)
        .arg(env!("CARGO_BIN_EXE_goattyd"))
        .arg("serve")
        // The daemon's own path for its watchdog; the listener comes from fd 3.
        .env("GOATTY_SOCKET", &sock)
        .env("GOATTY_SHELL", "/bin/sh")
        .env("RUST_LOG", "warn,rio_vt=error")
        .env("HOME", dir.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !sock.exists() {
        assert!(Instant::now() < deadline, "systemd-socket-activate did not listen");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (mut s, reply) = hello(&sock, PROTOCOL_VERSION);
    assert!(matches!(reply, ServerMsg::Hello { .. }));
    // The shells don't inherit the activation variables.
    let argv = ["sh", "-c", "echo fds=${LISTEN_FDS:-none}"].map(String::from).to_vec();
    s.write_all(&encode(&ClientMsg::Run { req: 1, size: SIZE, cwd: None, argv, opts: StreamOpts::LOCAL }).unwrap()).unwrap();
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&out).contains("fds=none") {
        assert!(Instant::now() < deadline, "{:?}", String::from_utf8_lossy(&out));
        match read_msg::<_, ServerMsg>(&mut s).unwrap() {
            ServerMsg::Attached { snapshot, .. } => out.extend(snapshot),
            ServerMsg::Output { data, .. } => out.extend(data),
            ServerMsg::Exited { .. } => break,
            _ => {}
        }
    }
    assert!(String::from_utf8_lossy(&out).contains("fds=none"), "{:?}", String::from_utf8_lossy(&out));
    let _ = child.kill();
    let _ = child.wait();
}

fn which(name: &str) -> Result<PathBuf, ()> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|p| p.is_file())).ok_or(())
}
