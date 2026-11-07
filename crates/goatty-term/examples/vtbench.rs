//! Headless VT throughput: how fast can goatty-term ingest PTY output?
//! `cargo run --release -p goatty-term --example vtbench`
use std::sync::Arc;
use std::time::Instant;

use goatty_term::{Frame, Palette, TermSize, Terminal};

fn corpus(kind: &str) -> Vec<u8> {
    let mut v = Vec::new();
    match kind {
        "ascii" => {
            for i in 0..200_000 {
                v.extend_from_slice(format!("{i:>8} the quick brown fox jumps over the lazy dog 0123456789\r\n").as_bytes());
            }
        }
        "sgr" => {
            for i in 0..200_000u32 {
                v.extend_from_slice(
                    format!(
                        "\x1b[38;5;{}m{i:>8}\x1b[1;38;2;{};{};200m colored \x1b[0;4mtext\x1b[0m end\r\n",
                        i % 256,
                        i % 255,
                        (i * 7) % 255
                    )
                    .as_bytes(),
                );
            }
        }
        "unicode" => {
            for i in 0..100_000 {
                v.extend_from_slice(format!("{i:>6} 日本語テキスト émojis 🦀🐐 combining e\u{301}\u{302} box ─│┌┐\r\n").as_bytes());
            }
        }
        _ => unreachable!(),
    }
    v
}

fn main() {
    let size = TermSize::new(200, 60);
    for kind in ["ascii", "sgr", "unicode"] {
        let data = corpus(kind);
        let (t, _ch) = Terminal::new(size, true, Arc::new(Palette::default()));
        let start = Instant::now();
        for chunk in data.chunks(64 * 1024) {
            t.feed(chunk);
        }
        let dt = start.elapsed();
        let mb = data.len() as f64 / 1e6;
        println!("{kind:>8}: {mb:7.1} MB in {:7.1} ms  -> {:7.1} MB/s", dt.as_secs_f64() * 1e3, mb / dt.as_secs_f64());
    }
    // Frame snapshot cost (what the renderer pays per frame under the lock).
    let (t, _ch) = Terminal::new(size, true, Arc::new(Palette::default()));
    t.feed(&corpus("sgr")[..2_000_000]);
    let mut f = Frame::default();
    let n = 2000;
    let start = Instant::now();
    for _ in 0..n {
        t.snapshot_into(&mut f);
    }
    println!("snapshot 200x60: {:.1} µs/frame", start.elapsed().as_secs_f64() * 1e6 / n as f64);
}
