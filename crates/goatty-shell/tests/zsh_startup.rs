//! The managed zsh, run for real in a sandboxed `$HOME`: startup order,
//! the user's files, ZDOTDIR hand-back, feature detection, `goatty-setup`,
//! and the OSC 133 / OSC 7 stream through a PTY.
//!
//! Skipped (with a note) when zsh isn't installed; CI installs it.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use goatty_config::{ShellSettings, ZshSettings};
use goatty_shell::{Dirs, Launch, Launcher};
use goatty_term::TermSize;
use goatty_term::pty::{Pty, ShellCommand};

fn zsh() -> Option<String> {
    let found = goatty_shell::launch::which("zsh");
    if found.is_none() {
        eprintln!("zsh not installed; skipping");
    }
    found
}

const BASIC_UTILS: &[&str] = &[
    "awk",
    "basename",
    "cat",
    "chmod",
    "cp",
    "cut",
    "date",
    "dircolors",
    "dirname",
    "env",
    "getent",
    "grep",
    "head",
    "id",
    "ln",
    "ls",
    "mkdir",
    "mktemp",
    "mv",
    "rm",
    "sed",
    "sort",
    "stat",
    "tail",
    "touch",
    "tput",
    "tr",
    "uname",
    "wc",
];

struct Sandbox {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    dirs: Dirs,
    zsh: String,
}

impl Sandbox {
    fn new(zsh: String) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join("userbin")).unwrap();
        // PATH holds zsh and basic utilities (compinit needs mv, grep, ...)
        // but not the host's starship/atuin/fzf, which would change the
        // results. Some distros' /etc/profile still appends /usr/bin.
        let sysbin = home.join("sysbin");
        std::fs::create_dir_all(&sysbin).unwrap();
        std::os::unix::fs::symlink(&zsh, sysbin.join("zsh")).unwrap();
        for tool in BASIC_UTILS {
            if let Some(p) = ["/usr/bin", "/bin"].iter().map(|d| Path::new(d).join(tool)).find(|p| p.exists()) {
                std::os::unix::fs::symlink(p, sysbin.join(tool)).unwrap();
            }
        }
        let dirs = Dirs::under(&tmp.path().join("xdg"));
        // A fake starship on a PATH entry that only the user's .zprofile adds.
        exe(&home.join("userbin/starship"), "#!/bin/sh\n[ \"$1\" = init ] && echo 'PROMPT=\"fake-starship> \"'\n");
        // A fake goattyd: records what goatty-setup asks for, then fails so
        // goatty-setup doesn't go on to exec a new shell.
        exe(&home.join("goattyd"), "#!/bin/sh\necho \"goattyd $*\" >> \"$HOME/trace\"\nexit 1\n");
        Self { _tmp: tmp, home, dirs, zsh }
    }

    fn file(&self, rel: &str, contents: &str) {
        let p = self.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }

    fn launch(&self, zsh: ZshSettings, orig_zdotdir: Option<&Path>) -> Launch {
        let settings = ShellSettings { program: Some(self.zsh.clone()), managed: true, zsh };
        let mut l = Launcher::new(Some(self.dirs.clone()), Some(self.home.join("goattyd"))).launch(&settings);
        assert!(l.managed);
        // Don't inherit the test runner's ZDOTDIR.
        l.env.retain(|(k, _)| k != "GOATTY_ORIG_ZDOTDIR");
        if let Some(o) = orig_zdotdir {
            l.env.push(("GOATTY_ORIG_ZDOTDIR".into(), o.to_string_lossy().into_owned()));
        }
        l
    }

    /// A clean environment, as goattyd would hand to a new session.
    fn base_env(&self) -> Vec<(String, String)> {
        vec![
            ("HOME".into(), self.home.to_string_lossy().into_owned()),
            ("PATH".into(), self.home.join("sysbin").to_string_lossy().into_owned()),
            ("TERM".into(), "xterm-256color".into()),
            ("USER".into(), "tester".into()),
        ]
    }

    /// `zsh -l -i -c script` with the managed config.
    fn run(&self, launch: &Launch, script: &str) -> Output {
        Command::new(&launch.program)
            .args(["-l", "-i", "-c", script])
            .env_clear()
            .envs(self.base_env())
            .envs(launch.env.iter().cloned())
            .current_dir(&self.home)
            .output()
            .unwrap()
    }

    fn trace(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.join("trace")).unwrap_or_default().lines().map(str::to_owned).collect()
    }
}

fn exe(p: &Path, body: &str) {
    std::fs::write(p, body).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// User startup files that log themselves to `$HOME/trace`.
const TRACE: &str = "print -r -- \"$NAME:${ZDOTDIR-unset}\" >> $HOME/trace\n";

fn traced(name: &str, extra: &str) -> String {
    format!("{}{extra}", TRACE.replace("$NAME", name))
}

#[test]
fn generated_files_parse() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh.clone());
    sb.launch(ZshSettings::default(), None);
    for f in [".zshenv", ".zprofile", ".zshrc", "integration.zsh", "settings.zsh"] {
        let out = Command::new(&zsh).arg("-n").arg(sb.dirs.zsh_dir().join(f)).output().unwrap();
        assert!(out.status.success(), "{f}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn default_startup_sources_user_env_not_rc() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    sb.file(".zshenv", &traced("zshenv", "export FROM_ZSHENV=1\n"));
    sb.file(".zprofile", &traced("zprofile", "path=($HOME/userbin $path)\n"));
    sb.file(".zshrc", &traced("zshrc", ""));
    sb.file(".zlogin", &traced("zlogin", ""));
    std::fs::create_dir_all(&sb.dirs.config).unwrap();
    std::fs::write(sb.dirs.user_zshrc(), TRACE.replace("$NAME", "additions")).unwrap();

    let launch = sb.launch(ZshSettings { syntax_highlighting: false, ..Default::default() }, None);
    let out = sb.run(
        &launch,
        "print -r -- \"Z=${ZDOTDIR-unset} E=$FROM_ZSHENV\"; goatty-status; zsh -c 'print -r -- nested=${ZDOTDIR-unset}'; goatty-setup",
    );
    let text = stdout(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.trim().is_empty(), "startup printed errors:\n{stderr}");

    // The user's files ran with their own (unset) ZDOTDIR; .zshrc didn't;
    // goatty's additions file did; .zlogin came from the user's dir after
    // goatty's .zshrc handed ZDOTDIR back.
    let trace = sb.trace();
    assert_eq!(&trace[..4], ["zshenv:unset", "zprofile:unset", "additions:unset", "zlogin:unset"], "{trace:?}");
    assert!(!trace.iter().any(|l| l.starts_with("zshrc")), "{trace:?}");

    assert!(text.contains("Z=unset E=1"), "{text}");
    assert!(text.contains("nested=unset"), "nested shells are plain zsh: {text}");
    let status = goatty_shell::doctor::parse_status(&text);
    let state = |n: &str| status.iter().find(|f| f.name == n).map(|f| f.state.clone());
    use goatty_shell::doctor::State;
    // Found through the PATH entry the user's .zprofile added.
    assert_eq!(state("starship"), Some(State::Ok(sb.home.join("userbin/starship").to_string_lossy().into_owned())));
    assert_eq!(state("atuin"), Some(State::Missing));
    assert_eq!(state("syntax-highlighting"), Some(State::Off));
    assert!(matches!(state("integration"), Some(State::Ok(_))));

    // goatty-setup asks for exactly the missing default tools (not fzf).
    let setup = trace.iter().find(|l| l.starts_with("goattyd ")).expect("goatty-setup ran goattyd");
    let mut want = vec!["atuin"];
    if state("autosuggestions") == Some(State::Missing) {
        want.insert(0, "autosuggestions");
    }
    assert_eq!(setup, &format!("goattyd shell install -- {}", want.join(" ")));
}

#[test]
fn zdotdir_set_by_user_zshenv_and_user_rc() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let xdg_zsh = sb.home.join(".config/zsh");
    // The common pattern: ~/.zshenv moves everything else to ~/.config/zsh.
    sb.file(".zshenv", &traced("zshenv", "export ZDOTDIR=$HOME/.config/zsh\n"));
    sb.file(".config/zsh/.zprofile", &traced("zprofile", ""));
    sb.file(".config/zsh/.zshrc", &traced("zshrc", "user_rc_var=1\n"));
    sb.file(".zprofile", &traced("wrong-zprofile", ""));

    let launch = sb.launch(ZshSettings { user_rc: true, ..Default::default() }, None);
    let out = sb.run(&launch, "print -r -- \"Z=$ZDOTDIR rc=$user_rc_var\"; zsh -c 'print -r -- nested=$ZDOTDIR'");
    let text = stdout(&out);
    let xdg = xdg_zsh.to_string_lossy();
    assert_eq!(sb.trace(), ["zshenv:unset".to_string(), format!("zprofile:{xdg}"), format!("zshrc:{xdg}")]);
    assert!(text.contains(&format!("Z={xdg} rc=1")), "user rc vars are global; ZDOTDIR restored: {text}");
    assert!(text.contains(&format!("nested={xdg}")), "{text}");
}

#[test]
fn orig_zdotdir_restored_without_user_env() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let mine = sb.home.join("myzsh");
    sb.file("myzsh/.zshenv", &traced("zshenv", ""));
    sb.file("myzsh/.zlogin", &traced("zlogin", ""));
    let launch = sb.launch(ZshSettings { user_env: false, ..Default::default() }, Some(&mine));
    let out = sb.run(&launch, "print -r -- Z=$ZDOTDIR");
    assert!(stdout(&out).contains(&format!("Z={}", mine.display())), "{}", stdout(&out));
    // .zshenv skipped (user_env off); .zlogin is zsh's own doing, from the
    // restored ZDOTDIR.
    assert_eq!(sb.trace(), [format!("zlogin:{}", mine.display())]);
}

/// Run an interactive session in a PTY, typing `input`, and return the output.
fn pty_session(sb: &Sandbox, launch: &Launch, input: &[&str]) -> String {
    let mut env = sb.base_env();
    env.extend(launch.env.iter().cloned());
    let mut cmd = ShellCommand::new(launch.program.clone(), launch.args.clone(), env);
    cmd.cwd = Some(sb.home.clone());
    let (mut pty, mut reader) = Pty::spawn(&cmd, TermSize::new(120, 40)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut out = Vec::new();
    let wait_for = |needle: &[u8], out: &mut Vec<u8>| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !out.windows(needle.len()).any(|w| w == needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(chunk) => out.extend(chunk),
                Err(_) => panic!("timed out waiting for {:?} in:\n{}", String::from_utf8_lossy(needle), String::from_utf8_lossy(out)),
            }
        }
    };
    // Each prompt ends with OSC 133;B.
    let mut prompts = 1;
    wait_for(b"\x1b]133;B", &mut out);
    for line in input {
        pty.write(format!("{line}\r").as_bytes()).unwrap();
        prompts += 1;
        let deadline = Instant::now() + Duration::from_secs(10);
        while out.windows(7).filter(|w| w == b"\x1b]133;B").count() < prompts {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(chunk) => out.extend(chunk),
                Err(_) => panic!("no prompt after {line:?}:\n{}", String::from_utf8_lossy(&out)),
            }
        }
    }
    pty.write(b"exit\r").unwrap();
    while let Ok(chunk) = rx.recv_timeout(Duration::from_secs(5)) {
        out.extend(chunk);
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn integration_marks_prompts_and_reports_cwd() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    std::fs::create_dir_all(sb.home.join("a b")).unwrap();
    let launch = sb.launch(ZshSettings::default(), None);
    let out = pty_session(&sb, &launch, &["cd 'a b'", "false"]);

    assert!(out.contains("\x1b]133;A"), "prompt start mark");
    assert!(out.contains("\x1b]133;C"), "command start mark");
    assert!(out.contains("\x1b]133;D;0\x07"), "cd succeeded");
    assert!(out.contains("\x1b]133;D;1\x07"), "false failed");
    let cwd = format!("{}/a%20b\x1b\\", sb.home.display());
    assert!(out.contains("\x1b]7;file://") && out.contains(&cwd), "OSC 7 with a percent-encoded path; got:\n{out:?}");
    // atuin is missing and the hint isn't dismissed.
    assert!(out.contains("goatty-setup"), "missing-tools hint");
}

#[test]
fn dismissed_hint_stays_quiet() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    std::fs::create_dir_all(&sb.dirs.state).unwrap();
    std::fs::write(sb.dirs.hint_marker(), "").unwrap();
    let launch = sb.launch(ZshSettings::default(), None);
    let out = pty_session(&sb, &launch, &[]);
    assert!(!out.contains("goatty-setup"), "{out:?}");
}
