//! Host settings over the protocol (v4), against a real goattyd with a
//! sandboxed `$HOME`: get, preview, apply (validate → generation → save),
//! bad values, generations, rollback, `SetPalette` changing the answers
//! to color queries, and `ReloadShells` reaching only managed shells.

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
    start_daemon_with("/bin/sh")
}

fn start_daemon_with(shell: &str) -> Daemon {
    let dir = tempfile::Builder::new().prefix("goatty-settings-").tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_goattyd"))
        .env("GOATTY_SOCKET", &sock)
        .env("GOATTY_SHELL", shell)
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
    assert_eq!(host_json(&d), "{\n  \"version\": 3\n}\n");

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

fn reload_until(d: &Daemon, want: u32, what: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut n = reload(d);
    while n != want && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        n = reload(d);
    }
    assert_eq!(n, want, "{what}");
    n
}

/// Start a session and leave it running detached.
fn create_session(d: &Daemon) -> u64 {
    let mut s = UnixStream::connect(&d.sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(&encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, client: "test".into() }).unwrap()).unwrap();
    assert!(matches!(read_msg::<_, ServerMsg>(&mut s).unwrap(), ServerMsg::Hello { .. }));
    let size = WinSize { cols: 80, rows: 10, cell_width: 8, cell_height: 16 };
    s.write_all(&encode(&ClientMsg::Create { req: 1, size, cwd: None, opts: StreamOpts::LOCAL }).unwrap()).unwrap();
    match read_msg::<_, ServerMsg>(&mut s).unwrap() {
        ServerMsg::Attached { session, .. } => session,
        other => panic!("{other:?}"),
    }
}

fn reload(d: &Daemon) -> u32 {
    match request(&d.sock, |req| ClientMsg::ReloadShells { req }) {
        ServerMsg::ShellsReloaded { count, .. } => count,
        other => panic!("{other:?}"),
    }
}

fn session_ids(d: &Daemon) -> Vec<u64> {
    Control::connect(&d.sock, "test").unwrap().list_sessions().unwrap().into_iter().map(|s| s.id).collect()
}

#[test]
fn reload_skips_unmanaged_shells() {
    let d = start_daemon();
    let id = create_session(&d);
    assert_eq!(reload(&d), 0);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(session_ids(&d), [id], "/bin/sh is left alone");
}

#[test]
fn reload_reaches_managed_zsh() {
    let Some(zsh) = ["/usr/bin/zsh", "/bin/zsh"].into_iter().find(|p| Path::new(p).exists()) else {
        eprintln!("zsh not installed; skipping");
        return;
    };
    let d = start_daemon_with(zsh);
    let id = create_session(&d);
    // A just-spawned session is registered before the reply ships, but under
    // load the daemon may still be between the fork and our Attached read;
    // wait briefly instead of assuming the first request is already settled.
    reload_until(&d, 1, "the managed zsh is counted");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(session_ids(&d), [id], "the session survives the reload");
    Control::connect(&d.sock, "test").unwrap().kill(id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session_ids(&d).is_empty() {
        assert!(Instant::now() < deadline, "session did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
    reload_until(&d, 0, "exited shells are forgotten");
}

/// Plugins over the protocol (v5): catalog, plan, install as approved,
/// the shell configuration, uninstall. The registry is a signed local
/// directory added in the sandbox's `registries.json` (built-in registries
/// off, so nothing goes to the network).
#[test]
fn plugins_over_the_protocol() {
    let d = start_daemon();
    let reg = d.home.join("registry");
    let key_file = d.home.join("signing.key");
    let public = goatty_plugin::sign::keygen_file(&key_file).unwrap();
    std::fs::create_dir_all(reg.join("plugins/greet/0.1.0/zsh")).unwrap();
    std::fs::write(reg.join("registry.toml"), format!("name = \"local\"\nkeys = [\"{public}\"]\n")).unwrap();
    std::fs::write(
        reg.join("plugins/greet/0.1.0/plugin.toml"),
        "schema = 1\nid = \"greet\"\nname = \"Greet\"\nversion = \"0.1.0\"\nsummary = \"Says hello\"\npublisher = \"tests\"\n\
         [[env]]\nname = \"GREETING\"\nvalue = \"hi\"\n[zsh]\ninit = \"zsh/init.zsh\"\n\
         [permissions]\nshell-code = \"defines the greet function\"\n",
    )
    .unwrap();
    std::fs::write(reg.join("plugins/greet/0.1.0/zsh/init.zsh"), "greet() { print -r -- $GREETING }\n").unwrap();
    let idx = goatty_plugin::index::build(&reg).unwrap();
    std::fs::write(reg.join("index.json"), idx.to_bytes()).unwrap();
    goatty_plugin::index::sign_dir(&reg, &goatty_plugin::sign::load_secret(&key_file).unwrap()).unwrap();
    let config = d.home.join(".config/goatty");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("registries.json"),
        format!(r#"{{"builtin": false, "registries": [{{"name": "local", "url": "{}", "keys": ["{public}"]}}]}}"#, reg.display()),
    )
    .unwrap();

    let call = |r: goatty_plugin::api::Request| -> goatty_plugin::api::Reply {
        let request = serde_json::to_string(&r).unwrap();
        match request_msg(&d.sock, |req| ClientMsg::Plugin { req, request }) {
            ServerMsg::Plugin { reply, .. } => serde_json::from_str(&reply).unwrap(),
            other => panic!("{other:?}"),
        }
    };
    use goatty_plugin::api::{Reply, Request};
    let Reply::Catalog(cat) = call(Request::Catalog { refresh: true }) else { panic!() };
    assert_eq!(cat.registries.len(), 1, "{:?}", cat.registries);
    assert_eq!(cat.available[0].id, "greet");

    let Reply::Plan(plan) = call(Request::Plan { id: "greet".into(), version: None }) else { panic!() };
    assert!(plan.verified);
    assert!(plan.shell_code.as_ref().unwrap().1.contains("greet()"));

    // A digest that doesn't match is an error reply, not an install.
    let bad = serde_json::to_string(&Request::Install { id: "greet".into(), version: None, approved: "x".into() }).unwrap();
    assert!(
        matches!(request_msg(&d.sock, |req| ClientMsg::Plugin { req, request: bad }), ServerMsg::Error { message, .. } if message.contains("review"))
    );

    let Reply::Changed(c) = call(Request::Install { id: "greet".into(), version: None, approved: plan.digest.clone() }) else { panic!() };
    assert!(c.reload && c.lines[0].starts_with("Installed Greet 0.1.0"), "{c:?}");
    assert!(host_json(&d).contains("\"greet\""));
    let zdir = d.home.join(".local/share/goatty/current/zsh");
    assert!(std::fs::read_to_string(zdir.join("plugins-early.zsh")).unwrap().contains("GREETING='hi'"));
    assert!(zdir.join("plugins/greet/zsh/init.zsh").is_file());

    let Reply::Changed(_) = call(Request::Uninstall { id: "greet".into() }) else { panic!() };
    assert!(!host_json(&d).contains("greet"));
    assert!(!zdir.join("plugins/greet").exists());
}

fn request_msg(sock: &Path, msg: impl FnOnce(u32) -> ClientMsg) -> ServerMsg {
    let mut c = Control::connect(sock, "test").unwrap();
    match c.request(msg) {
        Ok(m) => m,
        Err(e) => ServerMsg::Error { req: None, message: format!("{e:#}") },
    }
}

/// Shell setup and diagnostics over the protocol (v9): the status of a
/// managed zsh (a real startup, the built-in tools) and of an unmanaged
/// shell, and the "offered" / "don't ask again" markers.
#[test]
fn shell_status_over_the_protocol() {
    use goatty_shell::setup::{FeatureState, Reply, Request};
    let call = |d: &Daemon, r: Request| -> Reply {
        let request = serde_json::to_string(&r).unwrap();
        match request_msg(&d.sock, |req| ClientMsg::Shell { req, request }) {
            ServerMsg::Shell { reply, .. } => serde_json::from_str(&reply).unwrap(),
            other => panic!("{other:?}"),
        }
    };

    let d = start_daemon();
    let Reply::Status(s) = call(&d, Request::Status) else { panic!() };
    assert!(!s.managed && s.unmanaged.is_some() && s.startup.is_none(), "{s:?}");
    assert_eq!(s.tools.len(), 5, "every built-in plugin is listed");
    assert!(!s.should_offer(), "nothing to offer for a shell goatty doesn't manage");
    assert_eq!(call(&d, Request::Offered), Reply::Done);
    assert!(d.home.join(".local/state/goatty/shell-setup-offered").exists());
    assert_eq!(call(&d, Request::Dismiss), Reply::Done);
    let Reply::Status(s) = call(&d, Request::Status) else { panic!() };
    assert!(s.offered && s.dismissed);

    let Some(zsh) = ["/usr/bin/zsh", "/bin/zsh"].into_iter().find(|p| Path::new(p).exists()) else {
        eprintln!("zsh not installed; skipping the managed half");
        return;
    };
    let d = start_daemon_with(zsh);
    let Reply::Status(s) = call(&d, Request::Status) else { panic!() };
    assert!(s.managed, "{s:?}");
    let startup = s.startup.as_ref().unwrap_or_else(|| panic!("no startup: {:?}", s.startup_error));
    assert!(startup.features.iter().any(|f| f.id == "integration" && matches!(f.state, FeatureState::Loaded { .. })), "{startup:?}");
    assert!(s.generation.is_some(), "the probe built the configuration");
}
