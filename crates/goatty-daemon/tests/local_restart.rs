//! The local daemon after a rebuild: `goatty_client::local` notices that the
//! running goattyd is an older build than the file now installed, and
//! `goattyd stop --force` (the same `local::stop_daemon` the GUI uses) stops
//! it. Sandboxed: its own `GOATTY_SOCKET` and `HOME`.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use goatty_client::local;

fn copy_goattyd(to: &Path) {
    // Copy and rename, as cargo and package managers replace binaries: the
    // running file keeps its inode, the path gets a new one.
    let tmp = to.with_extension("new");
    std::fs::copy(env!("CARGO_BIN_EXE_goattyd"), &tmp).unwrap();
    std::fs::rename(&tmp, to).unwrap();
}

fn goattyd(exe: &Path, dir: &Path) -> Command {
    let mut c = Command::new(exe);
    c.env("GOATTY_SOCKET", dir.join("d.sock"))
        .env("GOATTY_SHELL", "/bin/sh")
        .env("RUST_LOG", "warn")
        .env("HOME", dir)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME");
    c
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_rebuilt_goattyd_is_noticed_and_the_old_one_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("goattyd");
    copy_goattyd(&exe);
    let mut daemon = Daemon(goattyd(&exe, dir.path()).arg("serve").spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(dir.path().join("d.sock")).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = daemon.0.id();

    // It runs the installed file.
    assert_eq!(local::outdated_than(pid, exe.clone()), None);

    // Rebuilt: a new file at the same path.
    copy_goattyd(&exe);
    let o = local::outdated_than(pid, exe.clone()).expect("the running build is older now");
    assert_eq!(o.pid, pid);
    assert!(o.running.to_string_lossy().ends_with(" (deleted)"), "{}", o.running.display());

    // `stop --force` from the new build stops it.
    let out = goattyd(&exe, dir.path()).args(["stop", "--force"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "old daemon still running");
        std::thread::sleep(Duration::from_millis(20));
    }
}
