//! Framing: `u32` little-endian length prefix + postcard body.

use std::io::{self, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Largest frame a peer accepts; protects against garbage length prefixes.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// A complete, length-prefixed frame, ready to write. Shared between
/// subscribers so a broadcast costs one encode.
pub type Frame = Arc<[u8]>;

pub fn encode<M: Serialize>(msg: &M) -> io::Result<Frame> {
    let mut buf = vec![0u8; 4];
    let mut buf = postcard::to_extend(msg, std::mem::take(&mut buf)).map_err(io::Error::other)?;
    let len = (buf.len() - 4) as u32;
    buf[..4].copy_from_slice(&len.to_le_bytes());
    Ok(buf.into())
}

/// Encode and write one message (no flush; callers decide batching).
pub fn write_msg<W: Write, M: Serialize>(w: &mut W, msg: &M) -> io::Result<()> {
    w.write_all(&encode(msg)?)
}

pub fn read_msg<R: Read, M: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<M> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    postcard::from_bytes(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientMsg, ServerMsg};

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &ClientMsg::Input { session: 7, data: b"ls\r".to_vec() }).unwrap();
        write_msg(&mut buf, &ServerMsg::Exited { session: 3, code: Some(0) }).unwrap();
        let mut r = &buf[..];
        assert!(matches!(read_msg::<_, ClientMsg>(&mut r).unwrap(), ClientMsg::Input { session: 7, data } if data == b"ls\r"));
        assert!(matches!(read_msg::<_, ServerMsg>(&mut r).unwrap(), ServerMsg::Exited { session: 3, code: Some(0) }));
        assert!(r.is_empty());
    }

    #[test]
    fn bytes_are_compact() {
        // serde_bytes: a 64 KiB chunk costs only a few bytes of header.
        let f = encode(&ServerMsg::Output { session: 1, data: vec![b'x'; 65536] }).unwrap();
        assert!(f.len() < 65536 + 12, "{}", f.len());
    }
}
