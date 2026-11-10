//! goattyd on other hosts: finding it, installing a matching one, and
//! telling apart "not installed" and "wrong version".
//!
//! Like VS Code's remote server, goatty brings its daemon along: every goatty
//! build knows the exact goattyd it wants on a host,
//! `~/.local/share/goatty/bin/goattyd-<version>-p<protocol>`, and can upload
//! it over the same ssh (after the user said yes). Nothing outside that
//! directory changes on the host: no PATH, no dotfiles, no sudo.
//!
//! The connect script tells the client what it found on stderr in one line,
//! `goatty-remote: <os> <arch> <goattyd path or ->`, before it starts the
//! proxy. That line is how the client learns the host's platform (to pick a
//! binary) without another ssh round trip.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use goatty_proto::PROTOCOL_VERSION;

use crate::handshake::VersionMismatch;
use crate::transport::{SshTarget, sh_quote};

/// Where installed daemons live on a host, relative to `$HOME`.
pub const BIN_DIR: &str = ".local/share/goatty/bin";

const MARKER: &str = "goatty-remote:";

/// The file name of the goattyd this build installs and prefers on hosts.
pub fn binary_name() -> String {
    format!("goattyd-{}-p{PROTOCOL_VERSION}", env!("CARGO_PKG_VERSION"))
}

/// Shell snippet run on the remote host: use this build's installed
/// goattyd, else one in `PATH`, `~/.local/bin` or `~/.cargo/bin`
/// (non-interactive SSH often lacks those in PATH). Plain POSIX sh, since
/// the user's login shell may be fish.
pub(crate) fn proxy_script(remote_daemon: Option<&str>) -> String {
    let candidates = match remote_daemon {
        Some(p) => sh_quote(p),
        None => format!("\"$HOME/{BIN_DIR}/{}\" goattyd \"$HOME/.local/bin/goattyd\" \"$HOME/.cargo/bin/goattyd\"", binary_name()),
    };
    format!(
        "p=\"$(uname -s) $(uname -m)\"; \
         for d in {candidates}; do if command -v \"$d\" >/dev/null 2>&1; then echo \"{MARKER} $p $d\" >&2; exec \"$d\" proxy; fi; done; \
         echo \"{MARKER} $p -\" >&2; \
         echo 'goatty: goattyd not found on remote host (install it or pass --remote-daemon PATH)' >&2; exit 127"
    )
}

/// A host's OS and CPU as `uname -s` / `uname -m` print them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    /// The goattyd build for this platform (`x86_64-linux`), if goatty has one.
    pub fn target(&self) -> Option<&'static str> {
        if !self.os.eq_ignore_ascii_case("linux") {
            return None;
        }
        match self.arch.as_str() {
            "x86_64" | "amd64" => Some("x86_64-linux"),
            "aarch64" | "arm64" => Some("aarch64-linux"),
            _ => None,
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.os, self.arch)
    }
}

/// What the connect script reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub platform: Platform,
    /// The goattyd it started, `None` when it found none.
    pub daemon: Option<String>,
}

impl Probe {
    /// The script ran this build's own installed goattyd.
    pub fn runs_ours(&self) -> bool {
        self.daemon.as_deref().is_some_and(|d| d.ends_with(&format!("/{BIN_DIR}/{}", binary_name())))
    }
}

/// Find the connect script's report in a transport's stderr.
pub fn parse_probe(lines: &[String]) -> Option<Probe> {
    lines.iter().rev().find_map(|l| {
        let rest = l.strip_prefix(MARKER)?.trim_start();
        let mut parts = rest.splitn(3, ' ');
        let os = parts.next()?.to_string();
        let arch = parts.next()?.to_string();
        let daemon = parts.next()?.trim();
        Some(Probe { platform: Platform { os, arch }, daemon: (daemon != "-").then(|| daemon.to_string()) })
    })
}

/// Why connecting to goattyd on a host failed, when goatty can fix it.
#[derive(Debug, Clone)]
pub enum RemoteProblem {
    /// No goattyd on the host.
    NotInstalled { host: String, platform: Platform },
    /// A goattyd answered (or the installed one is running), but it speaks
    /// no protocol version we do.
    Incompatible { host: String, mismatch: VersionMismatch, probe: Option<Probe> },
}

impl fmt::Display for RemoteProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteProblem::NotInstalled { host, platform } => write!(f, "goattyd isn't installed on {host} ({platform})"),
            RemoteProblem::Incompatible { host, mismatch, .. } => write!(f, "{host}: {mismatch}"),
        }
    }
}

impl std::error::Error for RemoteProblem {}

/// A goattyd binary for some target on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundled {
    pub path: PathBuf,
    /// The goattyd next to this goatty, used because the host has the same
    /// CPU: dynamically linked in development builds, so it may not run on
    /// a host with an older C library (the install checks that it runs).
    pub this_build: bool,
}

/// The goattyd to upload for `target`. Release archives carry static
/// builds in `remote/goattyd-<target>` next to the `goatty` binary
/// (packages may use `../lib/goatty/remote/`).
pub fn find_bundled(target: &str) -> Option<Bundled> {
    let exe = std::env::current_exe().ok()?;
    find_bundled_near(exe.parent()?, target)
}

fn find_bundled_near(dir: &Path, target: &str) -> Option<Bundled> {
    let name = format!("goattyd-{target}");
    for candidate in [dir.join("remote").join(&name), dir.join("../lib/goatty/remote").join(&name)] {
        if candidate.is_file() {
            return Some(Bundled { path: candidate, this_build: false });
        }
    }
    let own = dir.join("goattyd");
    (own.is_file() && target == format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS))
        .then_some(Bundled { path: own, this_build: true })
}

fn install_script(size: u64) -> String {
    let name = binary_name();
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"set -e
d="$HOME/{BIN_DIR}"; n="{name}"
mkdir -p "$d"
t="$d/.$n.$$"
trap 'rm -f "$t"' EXIT
cat > "$t"
s=$(($(wc -c < "$t")))
[ "$s" -eq {size} ] || {{ echo "upload incomplete ($s of {size} bytes)" >&2; exit 1; }}
chmod 755 "$t"
v=$("$t" --version 2>&1) || {{ echo "the uploaded goattyd doesn't run on this host: $v" >&2; exit 1; }}
case "$v" in "goattyd {version}"*) ;; *) echo "unexpected goattyd version: $v" >&2; exit 1;; esac
mv -f "$t" "$d/$n"
echo "installed $d/$n"
if [ -e /proc/self/exe ]; then
  pd=$(cd "$d" && pwd -P)
  for f in "$d"/goattyd-*; do
    b=${{f##*/}}
    [ -f "$f" ] && [ "$b" != "$n" ] || continue
    keep=
    for e in /proc/[0-9]*/exe; do
      [ "$(readlink "$e" 2>/dev/null)" = "$pd/$b" ] && {{ keep=1; break; }}
    done
    [ -n "$keep" ] || {{ rm -f "$f" && echo "removed $f"; }} || :
  done
fi
"#
    )
}

/// Upload `binary` to the host as this build's goattyd (see
/// [`binary_name`]), after checking it arrived whole and runs there.
/// Afterwards, older `goattyd-*` builds in [`BIN_DIR`] are deleted unless a
/// running process uses them (found through `/proc`; hosts without it keep
/// everything).
pub fn install(target: &SshTarget, binary: &Path) -> Result<()> {
    let file = std::fs::File::open(binary).with_context(|| format!("open {}", binary.display()))?;
    let size = file.metadata()?.len();
    let script = install_script(size);
    log::info!("installing {} on {} ({size} bytes)", binary_name(), target.host);
    let out = target
        .command(&format!("sh -c {}", sh_quote(&script)))
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("couldn't run {}", target.program))?;
    if !out.status.success() {
        bail!("installing goattyd on {} failed: {}", target.host, last_words(&out.stderr, &out.stdout));
    }
    Ok(())
}

/// Stop whatever goattyd owns the daemon socket on the host, using this
/// build's installed goattyd (`goattyd stop --force` works on daemons of any
/// version). Its sessions end.
pub fn stop_daemon(target: &SshTarget) -> Result<()> {
    let script = format!("exec \"$HOME/{BIN_DIR}/{}\" stop --force", binary_name());
    let out = target
        .command(&format!("sh -c {}", sh_quote(&script)))
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("couldn't run {}", target.program))?;
    if !out.status.success() {
        bail!("stopping goattyd on {} failed: {}", target.host, last_words(&out.stderr, &out.stdout));
    }
    Ok(())
}

/// The last few lines a command printed, for an error message. A backtrace
/// (goattyd's errors carry one when the host sets `RUST_BACKTRACE`) is cut
/// off, so the message itself is what's left.
fn last_words(stderr: &[u8], stdout: &[u8]) -> String {
    let text = if stderr.iter().any(|b| !b.is_ascii_whitespace()) { stderr } else { stdout };
    let text = String::from_utf8_lossy(text);
    let lines: Vec<&str> = text
        .lines()
        .take_while(|l| !l.trim_start().to_ascii_lowercase().starts_with("stack backtrace:"))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() { "no output".into() } else { lines[lines.len().saturating_sub(3)..].join(" / ") }
}

/// Classify a failed connect to `host` from the error and what the transport
/// printed on stderr.
pub(crate) fn diagnose(host: &str, err: anyhow::Error, stderr: &[String]) -> anyhow::Error {
    let probe = parse_probe(stderr);
    if let Some(mismatch) = err.downcast_ref::<VersionMismatch>() {
        return RemoteProblem::Incompatible { host: host.into(), mismatch: mismatch.clone(), probe }.into();
    }
    if let Some(Probe { platform, daemon: None }) = probe {
        return RemoteProblem::NotInstalled { host: host.into(), platform }.into();
    }
    with_stderr(err, stderr)
}

/// `err` with the last lines a transport command printed on stderr.
pub(crate) fn with_stderr(err: anyhow::Error, stderr: &[String]) -> anyhow::Error {
    let said: Vec<&str> = stderr.iter().map(|l| l.as_str()).filter(|l| !l.starts_with(MARKER) && !l.trim().is_empty()).collect();
    match said.len() {
        0 => err,
        n => err.context(said[n.saturating_sub(3)..].join(" / ")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn drain(mut r: impl Read) -> String {
        let mut s = String::new();
        let _ = r.read_to_string(&mut s);
        s
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn probe_lines_parse() {
        let p =
            parse_probe(&lines(&["Warning: Permanently added 'box'", "goatty-remote: Linux x86_64 /home/a b/.cargo/bin/goattyd"])).unwrap();
        assert_eq!(p.platform, Platform { os: "Linux".into(), arch: "x86_64".into() });
        assert_eq!(p.daemon.as_deref(), Some("/home/a b/.cargo/bin/goattyd"));
        assert!(!p.runs_ours());
        let p = parse_probe(&lines(&["goatty-remote: Linux aarch64 -"])).unwrap();
        assert_eq!(p.daemon, None);
        assert_eq!(p.platform.target(), Some("aarch64-linux"));
        let ours = format!("goatty-remote: Linux x86_64 /home/u/{BIN_DIR}/{}", binary_name());
        assert!(parse_probe(&lines(&[&ours])).unwrap().runs_ours());
        assert!(parse_probe(&lines(&["ssh: connect to host box port 22: Connection refused"])).is_none());
    }

    #[test]
    fn targets() {
        let t = |os: &str, arch: &str| Platform { os: os.into(), arch: arch.into() }.target();
        assert_eq!(t("Linux", "x86_64"), Some("x86_64-linux"));
        assert_eq!(t("Linux", "arm64"), Some("aarch64-linux"));
        assert_eq!(t("Linux", "armv7l"), None);
        assert_eq!(t("Darwin", "arm64"), None);
        assert_eq!(t("FreeBSD", "amd64"), None);
    }

    #[test]
    fn diagnose_tells_what_happened() {
        let eof = || anyhow::anyhow!("no valid greeting");
        let e = diagnose("box", eof(), &lines(&["goatty-remote: Linux x86_64 -", "goatty: goattyd not found"]));
        assert!(matches!(e.downcast_ref::<RemoteProblem>(), Some(RemoteProblem::NotInstalled { .. })));
        let m = VersionMismatch { daemon_protocol: Some(3), daemon_version: None, daemon_is_newer: false };
        let e = diagnose("box", m.into(), &lines(&["goatty-remote: Linux x86_64 /usr/bin/goattyd"]));
        let Some(RemoteProblem::Incompatible { probe: Some(p), .. }) = e.downcast_ref::<RemoteProblem>() else { panic!("{e:#}") };
        assert_eq!(p.daemon.as_deref(), Some("/usr/bin/goattyd"));
        let e = diagnose("box", eof(), &lines(&["user@box: Permission denied (publickey)."]));
        assert_eq!(format!("{e:#}"), "user@box: Permission denied (publickey).: no valid greeting");
    }

    #[test]
    fn scripts_run_in_posix_sh() {
        // Quoted paths survive, and the scripts parse in plain sh.
        let s = proxy_script(Some("/opt/it's/goattyd"));
        assert!(s.contains("'/opt/it'\\''s/goattyd'"));
        for script in [proxy_script(None), proxy_script(Some("/x y")), install_script(10)] {
            let out = std::process::Command::new("sh").args(["-n", "-c", &script]).output().unwrap();
            assert!(out.status.success(), "{script}\n{}", String::from_utf8_lossy(&out.stderr));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_prunes_old_daemons_but_not_running_ones() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(BIN_DIR);
        std::fs::create_dir_all(&bin).unwrap();
        let stale = bin.join("goattyd-0.0.1-p1");
        std::fs::write(&stale, "old").unwrap();
        // A "daemon" that's still running from an old build.
        let running = bin.join("goattyd-0.0.2-p2");
        std::fs::copy("/bin/sleep", &running).unwrap();
        let mut daemon = std::process::Command::new(&running).arg("30").spawn().unwrap();
        let unrelated = bin.join("notes.txt");
        std::fs::write(&unrelated, "x").unwrap();

        let fake = home.path().join("upload");
        std::fs::write(&fake, format!("#!/bin/sh\necho 'goattyd {}'\n", env!("CARGO_PKG_VERSION"))).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let size = std::fs::metadata(&fake).unwrap().len();
        let out = std::process::Command::new("sh")
            .args(["-c", &install_script(size)])
            .env("HOME", home.path())
            .stdin(std::fs::File::open(&fake).unwrap())
            .output()
            .unwrap();
        let _ = daemon.kill();
        let _ = daemon.wait();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(bin.join(binary_name()).is_file());
        assert!(!stale.exists(), "{}", String::from_utf8_lossy(&out.stdout));
        assert!(running.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn proxy_script_reports_a_missing_daemon() {
        let home = tempfile::tempdir().unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", &proxy_script(None)])
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = out;
        let err = drain(out.stderr.take().unwrap());
        assert_eq!(out.wait().unwrap().code(), Some(127));
        let probe = parse_probe(&err.lines().map(String::from).collect::<Vec<_>>()).unwrap();
        assert_eq!(probe.daemon, None);
        assert!(!probe.platform.os.is_empty() && !probe.platform.arch.is_empty());
    }

    #[test]
    fn bundled_binaries_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let here = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
        assert_eq!(find_bundled_near(dir.path(), "sparc-linux"), None);
        assert_eq!(find_bundled_near(dir.path(), &here), None);
        std::fs::create_dir(dir.path().join("remote")).unwrap();
        std::fs::write(dir.path().join("remote/goattyd-sparc-linux"), "x").unwrap();
        let b = find_bundled_near(dir.path(), "sparc-linux").unwrap();
        assert!(!b.this_build && b.path.ends_with("remote/goattyd-sparc-linux"));
        // The own goattyd stands in for a host with this computer's CPU.
        std::fs::write(dir.path().join("goattyd"), "x").unwrap();
        let own = find_bundled_near(dir.path(), &here).unwrap();
        assert!(own.this_build && own.path.ends_with("goattyd"));
        assert_eq!(find_bundled_near(dir.path(), "mips-linux"), None);
        assert_eq!(last_words(b"\n", b"a\nb\nc\nd\n"), "b / c / d");
        let with_backtrace = b"Error: not a goattyd\n\nStack backtrace:\n   0: main\n   1: _start\n";
        assert_eq!(last_words(with_backtrace, b""), "Error: not a goattyd");
    }
}
