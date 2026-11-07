//! Where host settings live: the connected goattyd (local or remote), or,
//! with `--local`, this process. All calls block; run them off the UI thread.

use std::sync::Arc;

use anyhow::Result;
use goatty_client::Connection;
use goatty_shell::HostService;
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

#[derive(Debug, Clone)]
pub struct GenRow {
    pub number: u32,
    pub created_unix: u64,
    pub active: bool,
}

impl HostLink {
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

    pub fn generations(&self) -> Result<Vec<GenRow>> {
        Ok(match self {
            HostLink::Daemon(c) => {
                c.generations()?.into_iter().map(|g| GenRow { number: g.number, created_unix: g.created_unix, active: g.active }).collect()
            }
            HostLink::Local(s) => {
                s.generations().into_iter().map(|g| GenRow { number: g.number, created_unix: g.created_unix, active: g.active }).collect()
            }
        })
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
}
