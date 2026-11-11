//! Where host settings live: the connected goattyd (local or remote), or,
//! with `--local`, this process. All calls block; run them off the UI thread.

use std::sync::Arc;

use anyhow::{Result, bail};
use goatty_client::Connection;
use goatty_shell::HostService;
use goatty_shell::{history, setup};
use gpui::Global;

#[derive(Clone)]
pub enum HostLink {
    Daemon(Arc<Connection>),
    Local(Arc<HostService>),
}

/// The settings window's host link.
pub struct HostLinkGlobal(pub Option<HostLink>);

impl Global for HostLinkGlobal {}

pub struct Snapshot {
    pub json: String,
    pub issues: Vec<String>,
    pub env_overrides: Vec<String>,
    pub generation: Option<u32>,
}

pub struct Applied {
    pub issues: Vec<String>,
    pub diff: Vec<(String, String)>,
    pub applied: bool,
    pub generation: Option<u32>,
}

/// A generation as the Profile page lists it.
#[derive(Debug, Clone)]
pub struct GenRow {
    pub entry: history::Entry,
    /// The host can name generations and show their file changes (a
    /// goattyd from before #55 only lists them).
    pub rich: bool,
}

impl HostLink {
    /// The same connection (or the same local host).
    pub fn same(&self, other: &HostLink) -> bool {
        match (self, other) {
            (HostLink::Daemon(a), HostLink::Daemon(b)) => Arc::ptr_eq(a, b),
            (HostLink::Local(a), HostLink::Local(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The host is this computer (its files are ours to show).
    pub fn is_local(&self) -> bool {
        match self {
            HostLink::Daemon(c) => c.endpoint().is_local(),
            HostLink::Local(_) => true,
        }
    }

    /// "this computer" or `user@host`.
    pub fn label(&self) -> String {
        match self {
            HostLink::Daemon(c) if !c.endpoint().is_local() => {
                let h = c.host();
                format!("{}@{}", h.user, h.hostname)
            }
            _ => "this computer".into(),
        }
    }

    pub fn get(&self) -> Result<Snapshot> {
        Ok(match self {
            HostLink::Daemon(c) => {
                let r = c.host_settings()?;
                Snapshot { json: r.json, issues: r.issues, env_overrides: r.env_overrides, generation: r.generation }
            }
            HostLink::Local(s) => {
                let r = s.snapshot();
                Snapshot { json: r.json, issues: r.issues, env_overrides: r.env_overrides, generation: r.generation }
            }
        })
    }

    pub fn set(&self, json: String, apply: bool) -> Result<Applied> {
        Ok(match self {
            HostLink::Daemon(c) => {
                let r = c.set_host_settings(json, apply)?;
                Applied {
                    issues: r.issues,
                    diff: r.diff.into_iter().map(|d| (d.name, d.unified)).collect(),
                    applied: r.applied,
                    generation: r.generation,
                }
            }
            HostLink::Local(s) => {
                let r = s.set(&json, apply)?;
                Applied {
                    issues: r.issues,
                    diff: r.diff.into_iter().map(|d| (d.name, d.unified)).collect(),
                    applied: r.applied,
                    generation: r.generation,
                }
            }
        })
    }

    /// The generations with names and what each changed; only numbers and
    /// ages from an older goattyd.
    pub fn generations(&self) -> Result<Vec<GenRow>> {
        if let Ok(setup::Reply::Generations { list }) = self.shell(setup::Request::Generations) {
            return Ok(list.into_iter().map(|entry| GenRow { entry, rich: true }).collect());
        }
        let HostLink::Daemon(c) = self else { bail!("no generations") };
        Ok(c.generations()?
            .into_iter()
            .map(|g| GenRow {
                entry: history::Entry { number: g.number, created_unix: g.created_unix, active: g.active, name: None, changes: None },
                rich: false,
            })
            .collect())
    }

    /// Name generation `number` (`None`: remove the name).
    pub fn name_generation(&self, number: u32, name: Option<String>) -> Result<()> {
        match self.shell(setup::Request::NameGeneration { number, name })? {
            setup::Reply::Done => Ok(()),
            other => bail!("unexpected reply from the host: {other:?}"),
        }
    }

    /// The generated files generation `number` changed: (file name, unified diff).
    pub fn generation_diff(&self, number: u32) -> Result<Vec<(String, String)>> {
        match self.shell(setup::Request::GenerationDiff { number })? {
            setup::Reply::Diff { files } => Ok(files.into_iter().map(|d| (d.name, d.unified)).collect()),
            other => bail!("unexpected reply from the host: {other:?}"),
        }
    }

    pub fn rollback(&self, to: Option<u32>) -> Result<Snapshot> {
        Ok(match self {
            HostLink::Daemon(c) => {
                let r = c.rollback(to)?;
                Snapshot { json: r.json, issues: r.issues, env_overrides: r.env_overrides, generation: r.generation }
            }
            HostLink::Local(s) => {
                let r = s.rollback(to)?;
                Snapshot { json: r.json, issues: r.issues, env_overrides: r.env_overrides, generation: r.generation }
            }
        })
    }

    /// A plugin operation on the host (downloads for installs: can take a while).
    pub fn plugin(&self, req: goatty_plugin::api::Request) -> Result<goatty_plugin::api::Reply> {
        match self {
            HostLink::Daemon(c) => Ok(serde_json::from_str(&c.plugin(serde_json::to_string(&req)?)?)?),
            HostLink::Local(s) => s.plugin_request(req),
        }
    }

    /// A shell setup or diagnostics operation on the host (the status
    /// starts a shell, installs download: can take a while).
    pub fn shell(&self, req: setup::Request) -> Result<setup::Reply> {
        match self {
            HostLink::Daemon(c) => Ok(serde_json::from_str(&c.shell(serde_json::to_string(&req)?)?)?),
            HostLink::Local(s) => s.shell_request(req),
        }
    }

    /// The host's shell diagnostics (starts a shell to ask it).
    pub fn shell_status(&self) -> Result<setup::Status> {
        match self.shell(setup::Request::Status)? {
            setup::Reply::Status(s) => Ok(*s),
            other => bail!("unexpected reply from the host: {other:?}"),
        }
    }

    /// Ask the host's running managed shells to reload. Returns how many were asked.
    pub fn reload_shells(&self) -> Result<u32> {
        Ok(match self {
            HostLink::Daemon(c) => c.reload_shells()?,
            HostLink::Local(s) => u32::try_from(s.reload_shells()).unwrap_or(u32::MAX),
        })
    }
}
