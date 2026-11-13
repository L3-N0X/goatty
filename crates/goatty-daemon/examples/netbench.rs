//! Headless end-to-end benchmark of the client ⇄ daemon path: keystroke echo
//! latency and flood throughput, with the client feeding a real mirror VT
//! (same work the GUI does, minus rendering).
//!
//!   cargo run --release -p goatty-daemon --example netbench            # private local daemon
//!   cargo run --release -p goatty-daemon --example netbench -- proxy   # same, through `goattyd proxy` (remote code path)
//!   cargo run --release -p goatty-daemon --example netbench -- slow 2m  # proxy throttled to 2 MB/s by `pv -L`
//!   cargo run --release -p goatty-daemon --example netbench -- ssh HOST
//!
//! Env: FLOOD_MB (default 200).

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use goatty_client::{Connection, Endpoint, PaneEvent, PaneSink};
use goatty_proto::WinSize;
use goatty_term::{Palette, TermSize, Terminal};
use parking_lot::Mutex;

/// Kills the private daemon even if the benchmark panics.
struct Guard(std::process::Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Sink {
    term: Terminal,
    bytes: AtomicU64,
    resyncs: AtomicU64,
    skipped: AtomicU64,
    tail: Mutex<Vec<u8>>,
}

impl PaneSink for Sink {
    fn feed(&self, b: &[u8]) {
        self.term.feed(b);
        self.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
        let mut t = self.tail.lock();
        t.extend_from_slice(&b[b.len().saturating_sub(4096)..]);
        let excess = t.len().saturating_sub(4096);
        t.drain(..excess);
    }
    fn event(&self, ev: PaneEvent) {
        if let PaneEvent::Resynced { skipped } = ev {
            self.resyncs.fetch_add(1, Ordering::Relaxed);
            self.skipped.fetch_add(skipped, Ordering::Relaxed);
        }
    }
}

impl Sink {
    fn wait_tail(&self, needle: &[u8], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.tail.lock().windows(needle.len()).any(|w| w == needle) {
                return true;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        false
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("error,rio_vt=error")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flood_mb: u64 = std::env::var("FLOOD_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(200);

    // Removed when dropped at the end of main, after the daemon is killed.
    let dir = tempfile::Builder::new().prefix("goatty-netbench-").tempdir()?;
    let sock = dir.path().join("d.sock");
    let goattyd = std::env::current_exe()?.parent().unwrap().parent().unwrap().join("goattyd");
    let mut daemon = None;
    let endpoint = match args.first().map(String::as_str) {
        Some("ssh") => Endpoint::ssh(args.get(1).expect("ssh HOST"), None),
        mode => {
            daemon = Some(Guard(
                Command::new(&goattyd)
                    .arg("serve")
                    .env("GOATTY_SOCKET", &sock)
                    .env("GOATTY_SHELL", "/bin/sh")
                    .env("RUST_LOG", "error")
                    .stdout(Stdio::null())
                    .spawn()?,
            ));
            while std::os::unix::net::UnixStream::connect(&sock).is_err() {
                std::thread::sleep(Duration::from_millis(10));
            }
            if mode == Some("slow") {
                let rate = args.get(1).map(String::as_str).unwrap_or("2m");
                let script = format!("GOATTY_SOCKET={} {} proxy | pv -q -L {rate}", sock.display(), goattyd.display());
                Endpoint::Command { label: format!("proxy throttled to {rate}B/s"), program: "sh".into(), args: vec!["-c".into(), script] }
            } else if mode == Some("proxy") {
                let args = vec![format!("GOATTY_SOCKET={}", sock.display()), goattyd.display().to_string(), "proxy".into()];
                Endpoint::Command { label: "local proxy".into(), program: "env".into(), args }
            } else {
                // SAFETY: single-threaded at this point.
                unsafe { std::env::set_var("GOATTY_SOCKET", &sock) };
                Endpoint::Local
            }
        }
    };

    let label = endpoint.label().to_string();
    let opts = endpoint.default_opts();
    let t0 = Instant::now();
    let conn = Connection::open(endpoint, "netbench")?;
    let connect = t0.elapsed();
    let size = TermSize::new(200, 50);
    let (term, _ch) = Terminal::new(size, false, Arc::new(Palette::default()));
    let sink = Arc::new(Sink { term, bytes: 0.into(), resyncs: 0.into(), skipped: 0.into(), tail: Mutex::new(Vec::new()) });
    let ws = WinSize { cols: 200, rows: 50, cell_width: 8, cell_height: 16 };
    let t1 = Instant::now();
    let id = conn.create(ws, None, sink.clone())?;
    let attach = t1.elapsed();

    // Wait for a prompt, then switch the tty to raw/no-echo `cat` so each
    // keystroke comes straight back: pure transport + PTY round trip.
    conn.input(id, b"echo READY-$((1+1))\r");
    assert!(sink.wait_tail(b"READY-2", Duration::from_secs(10)), "shell did not start");
    // 50 markers x 4 bytes: `dd` exits by itself after exactly 200 bytes
    // (raw mode has no ^C/^D to stop a `cat`; `head` would stdio-buffer).
    conn.input(id, b"stty raw -echo; dd bs=1 count=200 status=none; stty sane; echo\r");
    std::thread::sleep(Duration::from_millis(300));
    let mut lat = Vec::new();
    for i in 0..50u8 {
        let marker = format!("[{i:02}]").into_bytes();
        let t = Instant::now();
        conn.input(id, &marker);
        assert!(sink.wait_tail(&marker, Duration::from_secs(5)), "echo timeout");
        lat.push(t.elapsed());
        sink.tail.lock().clear();
    }
    std::thread::sleep(Duration::from_millis(300));
    lat.sort();

    let before = sink.bytes.load(Ordering::Relaxed);
    let t2 = Instant::now();
    conn.input(id, format!("head -c {}000000 /dev/zero | tr '\\0' x; echo; echo FLOOD-$((40+2))\r", flood_mb).as_bytes());
    assert!(sink.wait_tail(b"FLOOD-42", Duration::from_secs(120)), "flood marker not seen");
    let flood = t2.elapsed();
    let received = sink.bytes.load(Ordering::Relaxed) - before;

    println!("endpoint            {label} (window {} KiB, history {})", opts.window >> 10, opts.history);
    println!("connect             {:>8.2} ms", connect.as_secs_f64() * 1e3);
    println!("create+snapshot     {:>8.2} ms", attach.as_secs_f64() * 1e3);
    println!(
        "keystroke echo      p50 {:.3} ms  p90 {:.3} ms  max {:.3} ms",
        lat[25].as_secs_f64() * 1e3,
        lat[45].as_secs_f64() * 1e3,
        lat[49].as_secs_f64() * 1e3
    );
    println!(
        "flood {flood_mb} MB        {:>8.2} s → {:.0} MB/s produced, {:.1} MB delivered, {} resyncs ({:.1} MB skipped)",
        flood.as_secs_f64(),
        flood_mb as f64 / flood.as_secs_f64(),
        received as f64 / 1e6,
        sink.resyncs.load(Ordering::Relaxed),
        sink.skipped.load(Ordering::Relaxed) as f64 / 1e6
    );
    if let Some(rtt) = conn.rtt() {
        println!("heartbeat rtt       {:>8.3} ms", rtt.as_secs_f64() * 1e3);
    }
    drop(conn);
    drop(daemon);
    Ok(())
}
