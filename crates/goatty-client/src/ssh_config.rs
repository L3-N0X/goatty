//! The hosts in OpenSSH's client configuration, for goatty's host list.
//!
//! Only concrete `Host` names are listed (no `*`, `?` or `!` patterns),
//! each with the `HostName` and `User` its blocks set. `Include` is
//! followed (relative paths against `~/.ssh`, wildcards in the file name)
//! and `Match` blocks are skipped. This is for showing hosts only: when
//! goatty connects, ssh reads its configuration itself.

use std::path::{Path, PathBuf};

/// ssh's own limit on nested `Include`s.
const MAX_DEPTH: usize = 16;

/// A host from the ssh configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigHost {
    /// The `Host` name: what goatty passes to ssh.
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
}

impl ConfigHost {
    /// `user@hostname` (or as much of it as the configuration sets), when it
    /// says more than the alias.
    pub fn detail(&self) -> Option<String> {
        let host = self.hostname.as_deref().unwrap_or(&self.alias);
        let d = match &self.user {
            Some(u) => format!("{u}@{host}"),
            None => host.to_string(),
        };
        (d != self.alias).then_some(d)
    }
}

/// `~/.ssh`, if `$HOME` is set.
pub fn ssh_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".ssh"))
}

/// The hosts in `~/.ssh/config` and the files it includes (none when it
/// doesn't exist).
pub fn user_hosts() -> Vec<ConfigHost> {
    match ssh_dir() {
        Some(dir) => load(&dir.join("config"), &dir),
        None => Vec::new(),
    }
}

/// The hosts in the configuration file `path`; relative `Include`s are
/// resolved against `ssh_dir`. Unreadable files count as empty.
pub fn load(path: &Path, ssh_dir: &Path) -> Vec<ConfigHost> {
    let mut p = Parser { ssh_dir, hosts: Vec::new() };
    p.file(path, &mut Vec::new(), 0);
    p.hosts
}

struct Parser<'a> {
    ssh_dir: &'a Path,
    hosts: Vec<ConfigHost>,
}

impl Parser<'_> {
    fn file(&mut self, path: &Path, targets: &mut Vec<usize>, depth: usize) {
        if depth > MAX_DEPTH {
            log::debug!("ssh config: includes nested too deeply at {}", path.display());
            return;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => self.text(&text, targets, depth),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::debug!("ssh config: {}: {e}", path.display()),
        }
    }

    /// `targets`: the hosts the current block applies to (indices into
    /// `hosts`); empty outside a `Host` block and in a `Match` block.
    fn text(&mut self, text: &str, targets: &mut Vec<usize>, depth: usize) {
        for line in text.lines() {
            let Some((keyword, args)) = split_line(line) else { continue };
            match keyword.to_ascii_lowercase().as_str() {
                "host" => {
                    targets.clear();
                    for name in args.iter().filter(|a| is_concrete(a)) {
                        let i = match self.hosts.iter().position(|h| &h.alias == name) {
                            Some(i) => i,
                            None => {
                                self.hosts.push(ConfigHost { alias: name.clone(), hostname: None, user: None });
                                self.hosts.len() - 1
                            }
                        };
                        targets.push(i);
                    }
                }
                "match" => targets.clear(),
                // ssh uses the first value it finds for each option.
                "hostname" => {
                    for &i in targets.iter() {
                        let h = &mut self.hosts[i];
                        if h.hostname.is_none()
                            && let Some(v) = args.first()
                        {
                            h.hostname = Some(expand_host_tokens(v, &h.alias));
                        }
                    }
                }
                "user" => {
                    for &i in targets.iter() {
                        let h = &mut self.hosts[i];
                        if h.user.is_none() {
                            h.user = args.first().cloned();
                        }
                    }
                }
                "include" => {
                    for pattern in &args {
                        for path in self.include_paths(pattern) {
                            // An included file's `Host` lines don't change the
                            // block the `Include` itself is in.
                            let mut inner = targets.clone();
                            self.file(&path, &mut inner, depth + 1);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The files an `Include` argument names, sorted like ssh's glob.
    fn include_paths(&self, pattern: &str) -> Vec<PathBuf> {
        let path = if let Some(rest) = pattern.strip_prefix("~/") {
            match self.ssh_dir.parent() {
                Some(home) => home.join(rest),
                None => return Vec::new(),
            }
        } else if Path::new(pattern).is_absolute() {
            PathBuf::from(pattern)
        } else {
            self.ssh_dir.join(pattern)
        };
        let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else { return Vec::new() };
        if !has_wildcard(name) {
            return vec![path];
        }
        if has_wildcard(&dir.to_string_lossy()) {
            log::debug!("ssh config: wildcards in directories aren't followed: {pattern}");
            return Vec::new();
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_str().is_some_and(|n| !n.starts_with('.') && wildcard_match(name, n)))
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        found.sort();
        found
    }
}

/// `keyword args…` or `keyword=args…`; `None` for blank lines and comments.
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    Some((keyword.to_string(), split_args(rest)))
}

/// Whitespace-separated arguments; double quotes group.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut in_word) = (false, false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?'])
}

/// A name ssh can connect to as written (not a pattern or a negation).
fn is_concrete(name: &str) -> bool {
    !name.is_empty() && !has_wildcard(name) && !name.starts_with('!')
}

/// `HostName`'s `%h` (the alias) and `%%`.
fn expand_host_tokens(v: &str, alias: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(alias),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

/// `*` and `?` wildcards against a whole name.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str, hostname: Option<&str>, user: Option<&str>) -> ConfigHost {
        ConfigHost { alias: alias.into(), hostname: hostname.map(Into::into), user: user.map(Into::into) }
    }

    #[test]
    fn lists_concrete_hosts_with_their_options() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            "# comment\n\
             User everyone\n\
             Host box pi \"quoted name\"\n\
             \tHostName=10.0.0.5\n\
             \tUser   pi\n\
             Host *.internal !secret\n\
             \tUser admin\n\
             host Box2\n\
             \thostname %h.example.com\n\
             Match host box\n\
             \tUser nope\n\
             Host box\n\
             \tHostName ignored\n\
             \tUser later\n",
        )
        .unwrap();
        assert_eq!(
            load(&config, dir.path()),
            [
                host("box", Some("10.0.0.5"), Some("pi")),
                host("pi", Some("10.0.0.5"), Some("pi")),
                host("quoted name", Some("10.0.0.5"), Some("pi")),
                host("Box2", Some("Box2.example.com"), None),
            ]
        );
    }

    #[test]
    fn follows_includes() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(ssh.join("config"), "Include config.d/*.conf ~/other\nHost main\n  User me\n").unwrap();
        std::fs::write(ssh.join("config.d/b.conf"), "Host b\n").unwrap();
        std::fs::write(ssh.join("config.d/a.conf"), "Host a\n  HostName a.lan\n").unwrap();
        std::fs::write(ssh.join("config.d/skip.txt"), "Host skipped\n").unwrap();
        std::fs::write(dir.path().join("other"), "Host other\nInclude ~/other\n").unwrap();
        let aliases: Vec<String> = load(&ssh.join("config"), &ssh).into_iter().map(|h| h.alias).collect();
        // Includes come first (they're on the first line); the loop stops at the depth limit.
        assert_eq!(aliases, ["a", "b", "other", "main"]);
        assert!(load(&dir.path().join("missing"), &ssh).is_empty());
    }

    #[test]
    fn include_inside_a_block_keeps_the_block() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("inc"), "Host inner\n").unwrap();
        std::fs::write(dir.path().join("config"), "Host outer\nInclude inc\nUser u\n").unwrap();
        let hosts = load(&dir.path().join("config"), dir.path());
        assert_eq!(hosts, [host("outer", None, Some("u")), host("inner", None, None)]);
    }

    #[test]
    fn details_and_wildcards() {
        assert_eq!(host("box", Some("10.0.0.5"), Some("pi")).detail().as_deref(), Some("pi@10.0.0.5"));
        assert_eq!(host("box", None, None).detail(), None);
        assert_eq!(host("box", Some("box"), None).detail(), None);
        assert!(wildcard_match("*.conf", "a.conf"));
        assert!(wildcard_match("a?c*", "abcdef"));
        assert!(!wildcard_match("*.conf", "a.txt"));
        assert_eq!(expand_host_tokens("%h.%%.%x", "n"), "n.%.%x");
    }
}
