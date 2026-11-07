//! The `Hello` exchange every client performs first.

use std::io::Read;

use anyhow::{Context, Result, bail};
use goatty_proto::{ClientMsg, Frame, HostInfo, PROTOCOL_VERSION, ServerMsg, encode, read_msg};

/// The client's greeting frame.
pub(crate) fn hello(client: &str) -> Result<Frame> {
    Ok(encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, client: client.into() })?)
}

/// Read and validate the daemon's greeting. Returns its host info and pid.
///
/// `Hello` is variant 0 with `version` first in every protocol version, so
/// even an old daemon's greeting decodes far enough to report the mismatch.
/// Anything else: not a (compatible) goattyd.
pub(crate) fn read_greeting(reader: &mut impl Read, via: &str) -> Result<(HostInfo, u32)> {
    let reply: ServerMsg = read_msg(reader).with_context(|| {
        format!(
            "no valid greeting from goattyd via {via}: not installed on the host, or an older incompatible goattyd is running \
             (stop it with `pkill -x goattyd`; this ends its sessions)"
        )
    })?;
    match reply {
        ServerMsg::Hello { version, daemon_pid, host } if version == PROTOCOL_VERSION => Ok((host, daemon_pid)),
        ServerMsg::Hello { version, .. } => {
            bail!("goattyd speaks protocol v{version}, this client v{PROTOCOL_VERSION}: update goatty on both ends")
        }
        ServerMsg::Error { message, .. } => bail!("goattyd: {message}"),
        other => bail!("unexpected greeting {other:?}"),
    }
}
