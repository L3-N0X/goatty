//! Host settings over the protocol: the settings window edits a host's
//! `host.json` through its daemon, so a remote host keeps its own. The
//! logic is `goatty_shell::HostService`; this maps it to wire messages.

use goatty_proto::{FileDiff, GenerationInfo, ReqId, ServerMsg};
use goatty_shell::service::HostSnapshot;

use crate::server::Daemon;

fn snapshot_msg(req: ReqId, s: HostSnapshot) -> ServerMsg {
    ServerMsg::HostSettings { req, json: s.json, issues: s.issues, env_overrides: s.env_overrides, generation: s.generation }
}

fn error(req: ReqId, e: anyhow::Error) -> ServerMsg {
    ServerMsg::Error { req: Some(req), message: format!("{e:#}") }
}

pub(crate) fn get(daemon: &Daemon, req: ReqId) -> ServerMsg {
    snapshot_msg(req, daemon.host.snapshot())
}

pub(crate) fn set(daemon: &Daemon, req: ReqId, json: &str, apply: bool) -> ServerMsg {
    match daemon.host.set(json, apply) {
        Ok(o) => ServerMsg::HostSettingsApplied {
            req,
            issues: o.issues,
            diff: o.diff.into_iter().map(|d| FileDiff { name: d.name, unified: d.unified }).collect(),
            applied: o.applied,
            generation: o.generation,
        },
        Err(e) => error(req, e),
    }
}

pub(crate) fn generations(daemon: &Daemon, req: ReqId) -> ServerMsg {
    let list = daemon
        .host
        .generations()
        .into_iter()
        .map(|g| GenerationInfo { number: g.number, created_unix: g.created_unix, active: g.active })
        .collect();
    ServerMsg::Generations { req, list }
}

pub(crate) fn rollback(daemon: &Daemon, req: ReqId, to: Option<u32>) -> ServerMsg {
    match daemon.host.rollback(to) {
        Ok(s) => snapshot_msg(req, s),
        Err(e) => error(req, e),
    }
}

/// A plugin operation: JSON in, JSON out (`goatty_plugin::api`).
pub(crate) fn plugin(daemon: &Daemon, req: ReqId, request: &str) -> ServerMsg {
    let parsed: Result<goatty_plugin::api::Request, _> = serde_json::from_str(request);
    let result = match parsed {
        Ok(r) => daemon.host.plugin_request(r),
        Err(e) => Err(anyhow::anyhow!("unknown plugin request ({e}); is goattyd older than this goatty?")),
    };
    match result.and_then(|r| serde_json::to_string(&r).map_err(Into::into)) {
        Ok(reply) => ServerMsg::Plugin { req, reply },
        Err(e) => error(req, e),
    }
}

/// A shell setup or diagnostics operation: JSON in, JSON out (`goatty_shell::setup`).
pub(crate) fn shell(daemon: &Daemon, req: ReqId, request: &str) -> ServerMsg {
    let parsed: Result<goatty_shell::setup::Request, _> = serde_json::from_str(request);
    let result = match parsed {
        Ok(r) => daemon.host.shell_request(r),
        Err(e) => Err(anyhow::anyhow!("unknown shell request ({e}); is goattyd older than this goatty?")),
    };
    match result.and_then(|r| serde_json::to_string(&r).map_err(Into::into)) {
        Ok(reply) => ServerMsg::Shell { req, reply },
        Err(e) => error(req, e),
    }
}
