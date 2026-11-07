//! `goattyd proxy`: the remote end of every non-local transport.

use std::io::{Read, Write};

use anyhow::Result;

/// `goattyd proxy`: relay stdio ⇄ the daemon socket, starting the daemon if
/// needed. This is the remote end of every non-local transport
/// (`ssh host goattyd proxy`). It never parses the protocol.
pub fn proxy() -> Result<()> {
    use std::os::fd::FromRawFd;
    let exe = std::env::current_exe().ok();
    let sock = goatty_client::connect_or_spawn_daemon(exe)?;
    let mut to_daemon = sock.try_clone()?;
    let mut from_daemon = sock;
    // Raw fds: std's Stdout is line-buffered, which would hold binary
    // frames back until a 0x0A byte happens to pass through.
    // SAFETY: fds 0/1 are owned by this process for its whole life and
    // nothing else in `proxy` uses them.
    let mut stdin = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
    // SAFETY: as above.
    let mut stdout = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) });
    let up = std::thread::spawn(move || {
        relay(&mut *stdin, &mut to_daemon);
        let _ = to_daemon.shutdown(std::net::Shutdown::Write);
    });
    relay(&mut from_daemon, &mut *stdout);
    // Daemon closed the connection: we're done, whatever stdin is doing.
    drop(up);
    Ok(())
}

/// Plain read/write relay. Deliberately not `std::io::copy`: on Linux it
/// uses `splice()`, and data spliced from a pipe into a Unix socket can sit
/// in the kernel instead of being delivered (observed: the client's Hello
/// never reached the daemon), which would deadlock the handshake.
fn relay(from: &mut impl Read, to: &mut impl Write) {
    let mut buf = vec![0u8; 64 << 10];
    loop {
        match from.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}
