//! goattyd as a systemd user service (opt-in: `goattyd service install`).
//!
//! Two units in `~/.config/systemd/user/`:
//!
//! * `goattyd.socket` listens on `%t/goatty/goattyd.sock` (the default
//!   socket path) and starts the daemon on the first connection,
//! * `goattyd.service` runs `goattyd serve`, which takes the socket from
//!   systemd (`LISTEN_FDS`).
//!
//! Without the units, clients start a detached daemon themselves (`setsid`),
//! which inherits the GUI's environment and survives logout unless logind's
//! `KillUserProcesses=yes`. With them, systemd supervises the daemon, logs go
//! to the journal, and on a server with lingering enabled
//! (`loginctl enable-linger`) the daemon and its shells survive the last SSH
//! session closing. **Without lingering, systemd stops the user's services
//! at logout, sessions included**, so `install` warns about it.
//!
//! The service has no `Requires=goattyd.socket`, so stopping or removing the
//! socket never takes running sessions with it; only stopping the service
//! (or `goattyd stop`) does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub const SOCKET_UNIT: &str = "goattyd.socket";
pub const SERVICE_UNIT: &str = "goattyd.service";

/// First line of the units we write, so we never overwrite someone else's.
const HEADER: &str = "# Written by `goattyd service install`; `goattyd service uninstall` removes it.";

/// `$XDG_CONFIG_HOME/systemd/user`.
pub fn unit_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("systemd/user"))
}

/// The socket the units listen on: `$XDG_RUNTIME_DIR/goatty/goattyd.sock`.
pub fn unit_socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|r| PathBuf::from(r).join("goatty/goattyd.sock"))
}

/// The socket unit is installed (whether or not it's running).
pub fn installed() -> bool {
    unit_dir().is_some_and(|d| d.join(SOCKET_UNIT).is_file())
}

/// Quote a word for a systemd `ExecStart=` line.
fn systemd_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$");
    format!("\"{escaped}\"")
}

/// The program in an `ExecStart=` line we wrote (the reverse of
/// [`systemd_quote`] for its first word).
fn exec_program(exec_start: &str) -> Option<PathBuf> {
    let rest = exec_start.strip_prefix('"')?;
    let (mut out, mut chars) = (String::new(), rest.chars());
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(PathBuf::from(out)),
            '\\' => out.push(chars.next()?),
            '%' | '$' => {
                chars.next().filter(|n| *n == c)?;
                out.push(c);
            }
            c => out.push(c),
        }
    }
    None
}

/// The goattyd the installed service runs.
pub fn service_program() -> Option<PathBuf> {
    let text = std::fs::read_to_string(unit_dir()?.join(SERVICE_UNIT)).ok()?;
    text.lines().find_map(|l| l.strip_prefix("ExecStart=")).and_then(exec_program)
}

pub fn render_socket() -> String {
    format!(
        "{HEADER}
[Unit]
Description=goatty session daemon socket
Documentation=https://github.com/goatty-dev/goatty/blob/main/docs/remote-architecture.md

[Socket]
ListenStream=%t/goatty/goattyd.sock
SocketMode=0600
DirectoryMode=0700

[Install]
WantedBy=sockets.target
"
    )
}

pub fn render_service(exe: &Path) -> String {
    format!(
        "{HEADER}
[Unit]
Description=goatty session daemon
Documentation=https://github.com/goatty-dev/goatty/blob/main/docs/remote-architecture.md
After={SOCKET_UNIT}

[Service]
ExecStart={} serve
# The shells are the daemon's children: stopping the service ends them, like
# `goattyd stop`. A crashed daemon has lost its sessions anyway; start fresh.
Restart=on-failure
RestartSec=2
",
        systemd_quote(&exe.to_string_lossy())
    )
}

fn systemctl(args: &[&str]) -> Result<String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("couldn't run systemctl (is systemd installed?)")?;
    if !out.status.success() {
        bail!("systemctl --user {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn is_active(unit: &str) -> bool {
    systemctl(&["is-active", "--quiet", unit]).is_ok()
}

/// Start the installed socket unit (clients do this instead of spawning a
/// daemon when the units are installed).
pub fn start_socket() -> Result<()> {
    systemctl(&["start", SOCKET_UNIT]).map(drop)
}

/// Whether logind keeps this user's services running without a login
/// session. `None`: couldn't tell (no logind).
pub fn linger() -> Option<bool> {
    let user = std::env::var("USER").ok()?;
    let out = Command::new("loginctl").args(["show-user", &user, "--property=Linger", "--value"]).stdin(Stdio::null()).output().ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub unit_dir: Option<PathBuf>,
    pub installed: bool,
    /// The `ExecStart=` of the installed service.
    pub exec_start: Option<String>,
    pub socket_active: bool,
    pub service_active: bool,
    pub linger: Option<bool>,
}

pub fn status() -> Status {
    let unit_dir = unit_dir();
    let installed = installed();
    let exec_start = unit_dir
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join(SERVICE_UNIT)).ok())
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("ExecStart=").map(String::from)));
    Status {
        unit_dir,
        installed,
        exec_start,
        socket_active: is_active(SOCKET_UNIT),
        service_active: is_active(SERVICE_UNIT),
        linger: linger(),
    }
}

/// What [`install`] did.
#[derive(Debug, Clone)]
pub struct Installed {
    pub unit_dir: PathBuf,
    /// The socket is listening now. `false` when a daemon started outside
    /// systemd still owns the socket path: the units take over once it stops.
    pub started: bool,
    pub linger: Option<bool>,
}

/// Write the units for `exe` (`goattyd`), enable the socket and start it
/// unless another daemon is serving the socket path right now.
/// `socket_path`: the path clients use, which must be the units' path.
pub fn install(exe: &Path, socket_path: &Path) -> Result<Installed> {
    let unit_socket = unit_socket_path().context("XDG_RUNTIME_DIR is not set (no systemd user session?)")?;
    if socket_path != unit_socket {
        bail!("goattyd uses {} (GOATTY_SOCKET?), but the systemd socket unit listens on {}", socket_path.display(), unit_socket.display());
    }
    systemctl(&["show-environment"]).context("no systemd user manager is running for this user")?;
    let dir = unit_dir().context("$HOME is not set")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for (name, text) in [(SOCKET_UNIT, render_socket()), (SERVICE_UNIT, render_service(exe))] {
        let path = dir.join(name);
        if let Ok(existing) = std::fs::read_to_string(&path)
            && !existing.starts_with(HEADER)
        {
            bail!("{} exists and wasn't written by goatty; remove it first", path.display());
        }
        std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    }
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", SOCKET_UNIT])?;
    // A daemon started the old way (setsid) owns the socket path; binding the
    // unit's socket over it would orphan it. Let it finish first.
    let foreign = !is_active(SERVICE_UNIT) && std::os::unix::net::UnixStream::connect(socket_path).is_ok();
    if !foreign {
        start_socket()?;
    }
    Ok(Installed { unit_dir: dir, started: !foreign, linger: linger() })
}

/// Disable and remove the units. A running daemon keeps running (with its
/// sessions) until `goattyd stop`.
pub fn uninstall() -> Result<bool> {
    let dir = unit_dir().context("$HOME is not set")?;
    let mut removed = false;
    if dir.join(SOCKET_UNIT).exists() {
        let _ = systemctl(&["disable", SOCKET_UNIT]);
        let _ = systemctl(&["stop", SOCKET_UNIT]);
    }
    for name in [SOCKET_UNIT, SERVICE_UNIT] {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) if text.starts_with(HEADER) => {
                std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
                removed = true;
            }
            Ok(_) => bail!("{} wasn't written by goatty; leaving it alone", path.display()),
            Err(_) => {}
        }
    }
    if removed {
        let _ = systemctl(&["daemon-reload"]);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_are_well_formed() {
        let socket = render_socket();
        assert!(socket.starts_with(HEADER));
        assert!(socket.contains("ListenStream=%t/goatty/goattyd.sock\nSocketMode=0600\nDirectoryMode=0700"));
        let service = render_service(Path::new("/home/a b/100%/$x/goattyd"));
        assert!(service.contains("ExecStart=\"/home/a b/100%%/$$x/goattyd\" serve\n"), "{service}");
        // Stopping the socket must not stop the sessions.
        assert!(!service.contains("Requires="));
        assert_eq!(systemd_quote(r#"a"b\c"#), r#""a\"b\\c""#);
        let exec = service.lines().find_map(|l| l.strip_prefix("ExecStart=")).unwrap();
        assert_eq!(exec_program(exec).unwrap(), Path::new("/home/a b/100%/$x/goattyd"));
        assert_eq!(exec_program(&format!("{} serve", systemd_quote(r#"/a"b\c"#))).unwrap(), Path::new(r#"/a"b\c"#));
        assert_eq!(exec_program("/usr/bin/goattyd serve"), None);
    }
}
