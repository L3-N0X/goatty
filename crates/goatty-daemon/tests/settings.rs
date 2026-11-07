//! Host settings over the protocol (v3), against a real goattyd with a
//! sandboxed `$HOME`: get, preview, apply (validate → generation → save),
//! bad values, generations, rollback, and `SetPalette` changing the answers
//! to color queries.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use goatty_client::Control;
use goatty_proto::{ClientMsg, PROTOCOL_VERSION, ServerMsg, StreamOpts, WinSize, encode, read_msg};

struct Daemon {
    child: Child,
    sock: PathBuf,
    home: PathBuf,
    _dir: tempfile::TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_daemon() -> Daemon {
    let dir = tempfile::Builder::new().prefix("goatty-settings-").tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_goattyd"))
        .env("GOATTY_SOCKET", &sock)
        .env("GOATTY_SHELL", "/bin/sh")
        .env("RUST_LOG", "warn")
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(&sock).is_err() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    Daemon { child, sock, home, _dir: dir }
}

fn request(sock: &Path, msg: impl FnOnce(u32) -> ClientMsg) -> ServerMsg {
    Control::connect(sock, "test").unwrap().request(msg).unwrap()
}

fn host_json(d: &Daemon) -> String {
    match request(&d.sock, |req| ClientMsg::GetHostSettings { req }) {
        ServerMsg::HostSettings { json, .. } => json,
        other => panic!("{other:?}"),
    }
}

fn set(d: &Daemon, json: &str, apply: bool) -> (Vec<String>, usize, bool, Option<u32>) {
    match request(&d.sock, |req| ClientMsg::SetHostSettings { req, json: json.into(), apply }) {
        ServerMsg::HostSettingsApplied { issues, diff, applied, generation, .. } => (issues, diff.len(), applied, generation),
        other => panic!("{other:?}"),
    }
}

#[test]
fn host_settings_round_trip_and_rollback() {
    let d = start_daemon();
    let file = d.home.join(".config/goatty/host.json");
    assert_eq!(host_json(&d), "{\n  \"version\": 1\n}\n");

    // Preview: nothing built yet, so every generated file is new; nothing saved.
    let (issues, diffs, applied, generation) = set(&d, r#"{"history": {"size": 4242}}"#, false);
    assert!(issues.is_empty() && !applied && generation.is_none());
    assert!(diffs >= 5, "{diffs}");
    assert!(!file.exists());

    let (_, _, applied, generation) = set(&d, r#"{"history": {"size": 4242}}"#, true);
    assert!(applied);
    assert_eq!(generation, Some(1));
    assert!(std::fs::read_to_string(&file).unwrap().contains("4242"));
    assert!(host_json(&d).contains("4242"));
    let settings_zsh = d.home.join(".local/share/goatty/current/zsh/settings.zsh");
    assert!(std::fs::read_to_string(&settings_zsh).unwrap().contains("_goatty_hist_size='4242'"));

    // A bad value is reported and falls back; the rest applies.
    let (issues, _, applied, generation) = set(&d, r#"{"history": {"size": "lots", "share_between_sessions": true}}"#, true);
    assert!(applied);
    assert_eq!(generation, Some(2));
    assert!(issues.iter().any(|i| i.starts_with("history.size")), "{issues:?}");
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(saved.contains("share_between_sessions") && !saved.contains("4242"), "{saved}");

    match request(&d.sock, |req| ClientMsg::ListGenerations { req }) {
        ServerMsg::Generations { list, .. } => {
            assert_eq!(list.iter().map(|g| (g.number, g.active)).collect::<Vec<_>>(), [(1, false), (2, true)]);
        }
        other => panic!("{other:?}"),
    }

    // Rollback: generation 1 and its settings are back, the replaced file is backed up.
    match request(&d.sock, |req| ClientMsg::Rollback { req, generation: None }) {
        ServerMsg::HostSettings { json, generation, .. } => {
            assert!(json.contains("4242"), "{json}");
            assert_eq!(generation, Some(1));
        }
        other => panic!("{other:?}"),
    }
    assert!(std::fs::read_to_string(&file).unwrap().contains("4242"));
    assert!(std::fs::read_to_string(&settings_zsh).unwrap().contains("_goatty_hist_size='4242'"));
    assert!(d.home.join(".config/goatty/backups").read_dir().unwrap().count() >= 2);
}

#[test]
fn palette_answers_color_queries() {
    let d = start_daemon();
    let mut s = UnixStream::connect(&d.sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let send = |s: &mut UnixStream, m: &ClientMsg| s.write_all(&encode(m).unwrap()).unwrap();
    send(&mut s, &ClientMsg::Hello { version: PROTOCOL_VERSION, client: "test".into() });
    assert!(matches!(read_msg::<_, ServerMsg>(&mut s).unwrap(), ServerMsg::Hello { .. }));
    let size = WinSize { cols: 80, rows: 10, cell_width: 8, cell_height: 16 };
    send(&mut s, &ClientMsg::Create { req: 1, size, cwd: None, opts: StreamOpts::LOCAL });
    let session = match read_msg::<_, ServerMsg>(&mut s).unwrap() {
        ServerMsg::Attached { session, .. } => session,
        other => panic!("{other:?}"),
    };
    let mut colors: Vec<u32> = (0..21).map(|i| i * 0x010101).collect();
    colors[17] = 0x123456; // background
    send(&mut s, &ClientMsg::SetPalette { session, colors });
    // The daemon's terminal answers OSC 11 into the PTY; `read` gets it and we echo it back hex-free.
    let script = "printf '\\033]11;?\\007'; read -r x; echo \"GOT:$x\" | tr -d '\\033\\007'\n";
    send(&mut s, &ClientMsg::Input { session, data: script.into() });
    std::thread::sleep(Duration::from_millis(300));
    send(&mut s, &ClientMsg::Input { session, data: b"\n".to_vec() });
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let needle = b"rgb:1212/3434/5656";
    while !out.windows(needle.len()).any(|w| w == needle) {
        assert!(Instant::now() < deadline, "no OSC 11 answer with the new background:\n{}", String::from_utf8_lossy(&out));
        if let ServerMsg::Output { data, .. } = read_msg::<_, ServerMsg>(&mut s).unwrap() {
            out.extend(data);
        }
    }
}
