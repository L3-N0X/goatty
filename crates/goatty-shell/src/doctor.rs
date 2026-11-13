//! Diagnostics: ask a real managed shell what it loaded.
//!
//! What goattyd sees in its own `PATH` isn't what the shell sees: the
//! user's `.zprofile` usually adds directories. So the probe starts the
//! managed zsh exactly like a session would (login + interactive, no TTY),
//! runs `goatty-status`, and reports each feature's state plus how long the
//! startup took.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::launch::Launch;

const SENTINEL: &str = "<<goatty-status>>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Loaded; the detail is the file or binary used.
    Ok(String),
    /// Enabled but its tool isn't installed.
    Missing,
    /// Turned off in the settings.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub name: String,
    pub state: State,
}

#[derive(Debug, Clone)]
pub struct Probe {
    /// Wall time from spawn to exit: startup files plus `goatty-status`.
    pub elapsed: Duration,
    pub features: Vec<Feature>,
    /// Anything the startup printed to stderr (errors in startup files).
    pub stderr: String,
}

impl Probe {
    pub fn get(&self, name: &str) -> Option<&State> {
        self.features.iter().find(|f| f.name == name).map(|f| &f.state)
    }

    pub fn missing(&self) -> impl Iterator<Item = &str> {
        self.features.iter().filter(|f| f.state == State::Missing).map(|f| f.name.as_str())
    }
}

/// Start the managed shell from `launch` and collect `goatty-status`.
pub fn probe(launch: &Launch, timeout: Duration) -> Result<Probe> {
    if !launch.managed {
        bail!("{} doesn't run goatty's managed configuration", launch.program);
    }
    let script = format!("builtin print -r -- '{SENTINEL}'; goatty-status");
    let start = Instant::now();
    let mut child = Command::new(&launch.program)
        .args(["-l", "-i", "-c", &script])
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .current_dir(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start {}", launch.program))?;
    let read_all = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = r.read_to_end(&mut s);
            String::from_utf8_lossy(&s).into_owned()
        })
    };
    let out = read_all(Box::new(child.stdout.take().context("stdout")?));
    let err = read_all(Box::new(child.stderr.take().context("stderr")?));
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the shell didn't finish starting within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let elapsed = start.elapsed();
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let Some((_, report)) = stdout.split_once(SENTINEL) else {
        bail!("the shell exited ({status}) before reporting its status\n{}", stderr.trim_end());
    };
    Ok(Probe { elapsed, features: parse_status(report), stderr })
}

/// Parse `goatty-status` lines: `name<TAB>ok<TAB>detail`, `name<TAB>missing`, `name<TAB>off`.
pub fn parse_status(text: &str) -> Vec<Feature> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let name = parts.next()?.trim();
            let state = match parts.next()? {
                "ok" => State::Ok(parts.next().unwrap_or_default().to_owned()),
                "missing" => State::Missing,
                "off" => State::Off,
                _ => return None,
            };
            (!name.is_empty()).then(|| Feature { name: name.to_owned(), state })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_lines() {
        let f = parse_status("\nfzf-tab\tmissing\nfzf\toff\nstarship\tok\t/usr/bin/starship\nnoise line\nx\tweird\n");
        assert_eq!(
            f,
            vec![
                Feature { name: "fzf-tab".into(), state: State::Missing },
                Feature { name: "fzf".into(), state: State::Off },
                Feature { name: "starship".into(), state: State::Ok("/usr/bin/starship".into()) },
            ]
        );
    }
}
