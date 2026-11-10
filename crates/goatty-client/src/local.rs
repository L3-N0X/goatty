//! This computer's goattyd: is the running daemon the build goatty would
//! start now, and stopping it (whatever protocol it speaks).
//!
//! After an update (or a `cargo build`) the daemon that's still running is
//! the old build. It keeps the sessions, so nothing replaces it silently:
//! [`outdated`] tells the GUI, which restarts it right away when it holds no
//! sessions and asks otherwise (see `goatty-app/src/connect.rs`).

use std::os::unix::fs::MetadataExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;

use crate::{Control, VersionMismatch, service};

/// The goattyd a fresh start of the local daemon runs
/// ([`crate::connect_or_spawn_daemon`]): the systemd service's when its
/// units serve this socket, else `goattyd` next to the current binary.
pub fn installed_daemon() -> Option<PathBuf> {
    let path = goatty_proto::socket_path();
    if service::unit_socket_path().is_some_and(|p| p == path) && service::installed() {
        return service::service_program();
    }
    std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists())
}

/// The running local daemon is an older build than [`installed_daemon`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outdated {
    pub pid: u32,
    /// What it runs, as `/proc` says (` (deleted)` once the file was replaced).
    pub running: PathBuf,
    pub installed: PathBuf,
    /// Identifies the installed build, for [`decline`].
    installed_ino: u64,
}

/// A file's identity and age.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Build {
    dev: u64,
    ino: u64,
    modified: SystemTime,
}

impl Build {
    fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self { dev: m.dev(), ino: m.ino(), modified: m.modified().ok()? })
    }
}

/// `installed` is another file than `running`, and not older: a rebuild or
/// an update. (An older one, e.g. after checking out an older branch, isn't
/// worth restarting for.)
fn is_newer(running: Build, installed: Build) -> bool {
    (running.dev, running.ino) != (installed.dev, installed.ino) && installed.modified >= running.modified
}

/// `Some` when the local daemon with `pid` (from its `Hello`) runs an older
/// build than a restart would start. Cheap: a readlink and two stats.
pub fn outdated(pid: u32) -> Option<Outdated> {
    outdated_than(pid, installed_daemon()?)
}

/// [`outdated`] against a given `installed` goattyd.
pub fn outdated_than(pid: u32, installed: PathBuf) -> Option<Outdated> {
    let exe = PathBuf::from(format!("/proc/{pid}/exe"));
    let running = std::fs::read_link(&exe).ok()?;
    // stat follows /proc's link to the file the process runs, even deleted.
    let (old, new) = (Build::of(&exe)?, Build::of(&installed)?);
    is_newer(old, new).then_some(Outdated { pid, running, installed, installed_ino: new.ino })
}

fn declined_file() -> PathBuf {
    goatty_proto::runtime_dir().join("restart-declined")
}

fn decline_key(o: &Outdated) -> String {
    format!("{} {}", o.pid, o.installed_ino)
}

/// The user chose to keep this daemon running: don't ask again until it or
/// the installed build changes (or the next login: the runtime dir).
pub fn decline(o: &Outdated) {
    let _ = std::fs::create_dir_all(goatty_proto::runtime_dir());
    if let Err(e) = std::fs::write(declined_file(), decline_key(o)) {
        log::warn!("couldn't remember keeping goattyd running: {e}");
    }
}

/// See [`decline`].
pub fn declined(o: &Outdated) -> bool {
    std::fs::read_to_string(declined_file()).is_ok_and(|s| s.trim() == decline_key(o))
}

/// What the local daemon is wrong about; the GUI asks before restarting.
#[derive(Debug, Clone)]
pub enum LocalProblem {
    /// It speaks no protocol this goatty does.
    Incompatible(VersionMismatch),
    /// It works, but an older build than the installed one is running.
    Outdated { outdated: Outdated, sessions: usize },
}

impl std::fmt::Display for LocalProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocalProblem::Incompatible(m) => write!(f, "the local {m}"),
            LocalProblem::Outdated { outdated, .. } => {
                write!(f, "the local goattyd (pid {}) runs an older build than {}", outdated.pid, outdated.installed.display())
            }
        }
    }
}

impl std::error::Error for LocalProblem {}

/// A connect error, as a [`LocalProblem`] when it is one.
pub fn diagnose(err: anyhow::Error) -> anyhow::Error {
    match err.downcast_ref::<VersionMismatch>() {
        Some(m) => LocalProblem::Incompatible(m.clone()).into(),
        None => err,
    }
}

/// How [`stop_daemon`] stopped it.
#[derive(Debug)]
pub enum Stopped {
    NotRunning,
    /// It shut down when asked.
    Asked,
    /// It didn't understand the request (`why`); systemd stopped the service.
    Service {
        why: String,
    },
    /// It didn't understand the request (`why`); it was signalled.
    Killed {
        pid: u32,
        why: String,
    },
}

/// The process exists and isn't a zombie. Reaps it if it was our child
/// (goatty started the daemon it now stops).
fn alive(pid: u32) -> bool {
    // SAFETY: plain syscall; for a pid that isn't our child it fails with
    // ECHILD and changes nothing.
    unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| s.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().chars().next()) != Some('Z'))
}

fn wait_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while alive(pid) {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// Stop the local daemon and wait until it's gone. This ends its sessions:
/// only after the user agreed (or when it has none).
///
/// It's asked first (`Shutdown`). With `force`, one that doesn't speak our
/// protocol is stopped anyway: through systemd when the service runs it,
/// else SIGTERM (SIGKILL after 5 s) to the socket's owner once it's checked
/// to be a goattyd (`SO_PEERCRED`).
pub fn stop_daemon(force: bool) -> Result<Stopped> {
    let path = goatty_proto::socket_path();
    let err = match Control::connect_local("goatty-stop").and_then(|mut c| c.shutdown().map(|()| c.pid())) {
        Ok(pid) => {
            if !wait_gone(pid, Duration::from_secs(5)) {
                anyhow::bail!("goattyd (pid {pid}) is still running 5 s after it was asked to stop");
            }
            return Ok(Stopped::Asked);
        }
        Err(e) if force => e,
        Err(e) => return Err(e),
    };
    // Nothing listening: nothing to stop (remote updates run this blindly).
    if UnixStream::connect(&path).is_err() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        return Ok(Stopped::NotRunning);
    }
    let why = format!("{err:#}");
    if service::status().service_active {
        let ok = std::process::Command::new("systemctl").args(["--user", "stop", service::SERVICE_UNIT]).status()?.success();
        anyhow::ensure!(ok, "systemctl --user stop {} failed", service::SERVICE_UNIT);
        return Ok(Stopped::Service { why });
    }
    let pid = crate::control::socket_owner(&path)?;
    // Under socket activation the listener belongs to systemd, never kill that.
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    anyhow::ensure!(comm.starts_with("goattyd"), "{} belongs to pid {pid} ({}), not a goattyd", path.display(), comm.trim());
    log::info!("stopping goattyd (pid {pid}): {why}");
    // SAFETY: plain syscall on a pid we just checked is our goattyd.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if !wait_gone(pid, Duration::from_secs(5)) {
        // SAFETY: as above.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        wait_gone(pid, Duration::from_secs(2));
    }
    // The old daemon didn't get to remove its socket.
    if crate::control::socket_owner(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    Ok(Stopped::Killed { pid, why })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(ino: u64, secs: u64) -> Build {
        Build { dev: 1, ino, modified: SystemTime::UNIX_EPOCH + Duration::from_secs(secs) }
    }

    #[test]
    fn only_a_different_newer_file_counts() {
        assert!(is_newer(build(1, 10), build(2, 20)));
        assert!(is_newer(build(1, 10), build(2, 10)));
        // The very file it runs.
        assert!(!is_newer(build(1, 10), build(1, 10)));
        // An older build (an older branch checked out).
        assert!(!is_newer(build(1, 20), build(2, 10)));
    }

    #[test]
    fn this_process_is_not_outdated_against_itself() {
        // Our own exe vs itself: same file.
        let me = Build::of(Path::new("/proc/self/exe")).unwrap();
        assert!(!is_newer(me, Build::of(&std::env::current_exe().unwrap()).unwrap()));
        assert!(alive(std::process::id()));
        assert!(!alive(u32::MAX >> 1));
    }
}
