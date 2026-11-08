//! What a link under the mouse points at, and opening it.
//!
//! `goatty_term` finds links in the grid ([`Link`]); this module decides
//! what they mean on *this* machine: a relative path is resolved against the
//! shell's directory (OSC 7) and only counts if the file exists, paths on a
//! remote host can be copied but not opened, and schemes other than the
//! everyday ones (http, https, mailto, ftp, file) ask before launching
//! another application.

use std::path::{Path, PathBuf};

use goatty_term::{Link, LinkKind};

/// What clicking a link does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Open in the browser / mail client.
    Url(String),
    /// A URL with a scheme that launches some other application: ask first.
    Other(String),
    /// A file or directory on this machine.
    File { path: PathBuf, line: Option<u32>, column: Option<u32> },
    /// A path on the session's (remote) host: copy it instead.
    Remote { path: String, host: String },
}

impl Target {
    /// What the hint at the bottom of the pane shows.
    pub fn describe(&self) -> String {
        match self {
            Target::Url(u) | Target::Other(u) => u.clone(),
            Target::File { path, line, column } => {
                let mut s = path.display().to_string();
                if let Some(l) = line {
                    s.push_str(&format!(":{l}"));
                    if let Some(c) = column {
                        s.push_str(&format!(":{c}"));
                    }
                }
                s
            }
            Target::Remote { path, host } => format!("{host}:{path}"),
        }
    }
}

/// Where the session runs, for resolving paths.
pub struct Place<'a> {
    /// The shell's working directory (OSC 7), local sessions only.
    pub cwd: Option<&'a Path>,
    pub home: Option<&'a Path>,
    /// `Some(label)` when the session runs on another host.
    pub remote_host: Option<&'a str>,
    /// This machine's host name (for `file://host/…`).
    pub hostname: Option<&'a str>,
    /// Does a path exist? (`Path::exists` outside tests.)
    pub exists: &'a dyn Fn(&Path) -> bool,
}

const EVERYDAY_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp"];

/// What `link` points at, or `None` if it isn't really a link here (a path
/// that doesn't exist, a relative path with no known directory).
pub fn resolve(link: &Link, place: &Place<'_>) -> Option<Target> {
    match link.kind {
        LinkKind::Hyperlink | LinkKind::Url => resolve_uri(&link.target, place),
        LinkKind::Path => resolve_path(&link.target, link.line, link.column, place),
    }
}

fn resolve_uri(uri: &str, place: &Place<'_>) -> Option<Target> {
    let (scheme, rest) = uri.split_once(':')?;
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    if EVERYDAY_SCHEMES.contains(&scheme.as_str()) {
        return Some(Target::Url(uri.to_string()));
    }
    if scheme != "file" {
        return Some(Target::Other(uri.to_string()));
    }
    // file://host/path or file:/path
    let after = rest.strip_prefix("//").unwrap_or(rest);
    let (host, path) = match after.find('/') {
        Some(i) if rest.starts_with("//") => (&after[..i], &after[i..]),
        _ if rest.starts_with("//") => return None,
        _ => ("", after),
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = percent_decode(path)?;
    let local_host =
        host.is_empty() || host.eq_ignore_ascii_case("localhost") || place.hostname.is_some_and(|h| h.eq_ignore_ascii_case(host));
    match place.remote_host {
        Some(remote) => Some(Target::Remote { path, host: if local_host { remote.to_string() } else { host.to_string() } }),
        None if local_host => Some(Target::File { path: PathBuf::from(path), line: None, column: None }),
        None => Some(Target::Remote { path, host: host.to_string() }),
    }
}

fn resolve_path(text: &str, line: Option<u32>, column: Option<u32>, place: &Place<'_>) -> Option<Target> {
    if let Some(host) = place.remote_host {
        // Nothing to check against, so only paths that can't be misread.
        return (text.starts_with('/') || text.starts_with("~/"))
            .then(|| Target::Remote { path: text.to_string(), host: host.to_string() });
    }
    let candidates: Vec<PathBuf> = if let Some(rest) = text.strip_prefix("~/") {
        vec![place.home?.join(rest)]
    } else if text.starts_with('/') {
        vec![PathBuf::from(text)]
    } else {
        let cwd = place.cwd?;
        let mut v = vec![cwd.join(text)];
        // `git diff` prefixes: a/src/x.rs, b/src/x.rs.
        if let Some(rest) = text.strip_prefix("a/").or_else(|| text.strip_prefix("b/")) {
            v.push(cwd.join(rest));
        }
        v
    };
    let path = candidates.into_iter().map(|p| normalize(&p)).find(|p| (place.exists)(p))?;
    Some(Target::File { path, line, column })
}

/// Drop `.` and resolve `..` lexically (no symlink lookups).
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// `%20` → ' '. `None` for malformed escapes or non-UTF-8 results.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// This machine's host name, if the OS tells us cheaply.
pub fn hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Run a file opener command (argv from `LinkSettings::file_command`)
/// detached from goatty, reaping it in the background.
pub fn spawn_opener(argv: &[String]) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let (program, args) = argv.split_first().ok_or_else(|| std::io::Error::other("empty command"))?;
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::Builder::new().name("link-opener".into()).spawn(move || {
        let _ = child.wait();
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(kind: LinkKind, target: &str) -> Link {
        Link { kind, target: target.into(), line: None, column: None, spans: Vec::new() }
    }

    fn local<'a>(exists: &'a dyn Fn(&Path) -> bool) -> Place<'a> {
        Place { cwd: Some(Path::new("/work/proj")), home: Some(Path::new("/home/me")), remote_host: None, hostname: Some("box"), exists }
    }

    fn file(p: &str) -> Option<Target> {
        Some(Target::File { path: PathBuf::from(p), line: None, column: None })
    }

    #[test]
    fn urls_by_scheme() {
        let all = |_: &Path| true;
        let place = local(&all);
        let url = |u: &str| resolve(&link(LinkKind::Url, u), &place);
        assert_eq!(url("https://x.org/a"), Some(Target::Url("https://x.org/a".into())));
        assert_eq!(url("MAILTO:me@x.org"), Some(Target::Url("MAILTO:me@x.org".into())));
        assert_eq!(url("ssh://host"), Some(Target::Other("ssh://host".into())));
        let osc8 = |u: &str| resolve(&link(LinkKind::Hyperlink, u), &place);
        assert_eq!(osc8("vscode://file/x"), Some(Target::Other("vscode://file/x".into())));
        assert_eq!(osc8("javascript alert"), None);
        assert_eq!(osc8("file:///tmp/a%20b.txt"), file("/tmp/a b.txt"));
        assert_eq!(osc8("file://localhost/etc/hosts"), file("/etc/hosts"));
        assert_eq!(osc8("file://box/etc/hosts?x#y"), file("/etc/hosts"));
        assert_eq!(osc8("file:/etc/hosts"), file("/etc/hosts"));
        assert_eq!(osc8("file://other/etc/hosts"), Some(Target::Remote { path: "/etc/hosts".into(), host: "other".into() }));
        assert_eq!(osc8("file:///bad%zz"), None);
    }

    #[test]
    fn local_paths_must_exist() {
        let exists = |p: &Path| ["/work/proj/src/main.rs", "/home/me/notes.md", "/etc/hosts", "/work/x"].iter().any(|e| Path::new(e) == p);
        let place = local(&exists);
        let path = |t: &str| resolve(&link(LinkKind::Path, t), &place);
        assert_eq!(path("src/main.rs"), file("/work/proj/src/main.rs"));
        assert_eq!(path("./src/main.rs"), file("/work/proj/src/main.rs"));
        assert_eq!(path("../x"), file("/work/x"));
        assert_eq!(path("b/src/main.rs"), file("/work/proj/src/main.rs"), "git diff prefix");
        assert_eq!(path("~/notes.md"), file("/home/me/notes.md"));
        assert_eq!(path("/etc/hosts"), file("/etc/hosts"));
        assert_eq!(path("src/nope.rs"), None);
        let mut with_pos = link(LinkKind::Path, "src/main.rs");
        (with_pos.line, with_pos.column) = (Some(3), Some(9));
        let t = resolve(&with_pos, &place).unwrap();
        assert_eq!(t.describe(), "/work/proj/src/main.rs:3:9");

        let no_cwd = Place { cwd: None, ..local(&exists) };
        assert_eq!(resolve(&link(LinkKind::Path, "src/main.rs"), &no_cwd), None, "relative needs a directory");
        assert_eq!(resolve(&link(LinkKind::Path, "/etc/hosts"), &no_cwd), file("/etc/hosts"));
    }

    #[test]
    fn remote_paths_are_copied_not_opened() {
        let never = |_: &Path| -> bool { panic!("no local checks for remote sessions") };
        let place = Place { remote_host: Some("me@server"), ..local(&never) };
        let path = |t: &str| resolve(&link(LinkKind::Path, t), &place);
        assert_eq!(path("/var/log/syslog"), Some(Target::Remote { path: "/var/log/syslog".into(), host: "me@server".into() }));
        assert_eq!(path("src/main.rs"), None);
        let osc8 = resolve(&link(LinkKind::Hyperlink, "file:///srv/x"), &place);
        assert_eq!(osc8, Some(Target::Remote { path: "/srv/x".into(), host: "me@server".into() }));
        assert_eq!(resolve(&link(LinkKind::Url, "https://x.org"), &place), Some(Target::Url("https://x.org".into())));
    }
}
