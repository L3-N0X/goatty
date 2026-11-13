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

use goatty_config::{Effective, HostSettings, ZshSettings};
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
        let mut settings = HostSettings::default();
        settings.shell.zsh = zsh;
        self.launch_with(settings, orig_zdotdir)
    }

    fn launch_with(&self, mut settings: HostSettings, orig_zdotdir: Option<&Path>) -> Launch {
        settings.shell.program = Some(self.zsh.clone());
        let mut l = Launcher::new(Some(self.dirs.clone()), Some(self.home.join("goattyd"))).launch(&Effective::plain(settings));
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

/// Default settings with the syntax highlighting plugin off (the host's
/// distro copy would otherwise load in some tests and not in others).
fn no_highlighting() -> HostSettings {
    let mut s = HostSettings::default();
    s.edit_plugin("syntax-highlighting", |p| p.enabled = false);
    s
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
    for f in [
        ".zshenv",
        ".zprofile",
        ".zshrc",
        "integration.zsh",
        "settings.zsh",
        "aliases.zsh",
        "plugins-early.zsh",
        "plugins-line-editor.zsh",
        "plugins-prompt.zsh",
        "plugins.zsh",
        "plugins-last.zsh",
    ] {
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

    let launch = sb.launch_with(no_highlighting(), None);
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
    // Alphabetical, like `goatty-status`. autosuggestions is missing on
    // hosts without the distro plugin (CI).
    let mut want = vec!["atuin"];
    if state("autosuggestions") == Some(State::Missing) {
        want.push("autosuggestions");
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
fn transient_prompt_shrinks_accepted_prompts() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let mut settings = HostSettings::default();
    settings.prompt.transient = true;
    settings.prompt.character.error = "✗".into();
    let launch = sb.launch_with(settings, None);
    let mut sh = PtyShell::start(&sb, &launch);
    // Accepting a line redraws its prompt as the bold prompt character
    // (marked as a prompt), colored by the status that prompt showed.
    sh.pty.write(b"false\r").unwrap();
    sh.wait_for(b"\x1b]133;A\x07\x1b[1m\x1b[32m\xe2\x9d\xaf");
    sh.wait_for(b"\x1b]133;D;1\x07");
    sh.pty.write(b"true\r").unwrap();
    sh.wait_for("\x1b[31m✗".as_bytes());
    sh.wait_for(b"\x1b]133;D;0\x07");
    // The next prompt is the full one again (it shows the directory).
    let n = sh.prompts() + 1;
    sh.wait_for_prompts(n);
    let out = sh.text();
    let last = &out[out.rfind("\x1b]133;D;0").unwrap()..];
    assert!(last.contains('~'), "the full prompt is back: {last:?}");
    assert!(!out.contains("command not found") && !out.contains("no such"), "{out:?}");
    let _ = sh.pty.write(b"exit\r");
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

#[test]
fn hint_stays_quiet_in_sessions_of_a_window_that_offers_the_setup() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let mut launch = sb.launch(ZshSettings::default(), None);
    goatty_shell::setup::gui_offers_setup(&mut launch);
    let out = pty_session(&sb, &launch, &["echo gui=${GOATTY_GUI_SETUP-unset}"]);
    assert!(!out.contains("goatty-setup"), "{out:?}");
    assert!(out.contains("gui=unset"), "programs don't inherit the marker: {out:?}");
}

#[test]
fn hint_stays_quiet_once_the_gui_offered_the_install() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    std::fs::create_dir_all(&sb.dirs.state).unwrap();
    std::fs::write(sb.dirs.offer_marker(), "").unwrap();
    let launch = sb.launch(ZshSettings::default(), None);
    let out = pty_session(&sb, &launch, &[]);
    assert!(!out.contains("goatty-setup"), "{out:?}");
}

/// An interactive managed shell in a PTY, driven prompt by prompt.
struct PtyShell {
    pty: Pty,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    out: Vec<u8>,
}

impl PtyShell {
    fn start(sb: &Sandbox, launch: &Launch) -> Self {
        let mut env = sb.base_env();
        env.extend(launch.env.iter().cloned());
        let mut cmd = ShellCommand::new(launch.program.clone(), launch.args.clone(), env);
        cmd.cwd = Some(sb.home.clone());
        let (pty, mut reader) = Pty::spawn(&cmd, TermSize::new(120, 40)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut sh = Self { pty, rx, out: Vec::new() };
        sh.wait_for_prompts(1);
        sh
    }

    fn prompts(&self) -> usize {
        self.out.windows(7).filter(|w| w == b"\x1b]133;B").count()
    }

    /// Wait until `n` prompts have been shown in total.
    fn wait_for_prompts(&mut self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.prompts() < n {
            match self.rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(chunk) => self.out.extend(chunk),
                Err(_) => panic!("waiting for prompt {n}:\n{}", String::from_utf8_lossy(&self.out)),
            }
        }
    }

    fn wait_for(&mut self, needle: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.out.windows(needle.len()).any(|w| w == needle) {
            match self.rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(chunk) => self.out.extend(chunk),
                Err(_) => panic!("waiting for {:?}:\n{}", String::from_utf8_lossy(needle), String::from_utf8_lossy(&self.out)),
            }
        }
    }

    /// Type `line` + Enter and wait for the next prompt.
    fn run(&mut self, line: &str) {
        let n = self.prompts() + 1;
        self.pty.write(format!("{line}\r").as_bytes()).unwrap();
        self.wait_for_prompts(n);
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out).into_owned()
    }
}

#[test]
fn reloads_on_request() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let launch = sb.launch(ZshSettings::default(), None);
    let svc = goatty_shell::HostService::new(Launcher::new(Some(sb.dirs.clone()), None), None);
    let mut sh = PtyShell::start(&sb, &launch);
    let pid = sh.pty.pid().unwrap();
    svc.track_shell(pid);

    // Idle at an empty prompt: reloads right away (same pid: exec).
    sh.run("x=1");
    let n = sh.prompts();
    assert_eq!(svc.reload_shells(), 1);
    sh.wait_for_prompts(n + 1);
    sh.run("print -r -- X=${x:-unset} P=$$");
    assert!(sh.text().contains(&format!("X=unset P={pid}")), "{}", sh.text());

    // Busy: reloads once the command is done.
    sh.run("y=1");
    let n = sh.prompts();
    sh.pty.write(b"/bin/sleep 0.5\r").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    svc.reload_shells();
    sh.wait_for_prompts(n + 1);
    sh.run("print -r -- Y=${y:-unset}");
    assert!(sh.text().contains("Y=unset"), "{}", sh.text());

    // Something typed but not entered: left alone until Enter. The signal
    // still arrives quickly; give slow runners a moment instead of a fixed
    // sleep, and assert it stayed untouched after that.
    sh.run("z=1");
    let n = sh.prompts();
    sh.pty.write(b"print -r -- typed").unwrap();
    sh.wait_for(b"typed");
    svc.reload_shells();
    let deadline = Instant::now() + Duration::from_secs(3);
    while sh.prompts() == n && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(sh.prompts(), n, "no reload while typing");
    sh.run("");
    sh.run("print -r -- Z=${z:-unset}");
    assert!(sh.text().contains("Z=unset"), "{}", sh.text());

    // Background jobs: only a reminder, the shell keeps its state.
    sh.run("w=1; /bin/sleep 30 &");
    let n = sh.prompts();
    svc.reload_shells();
    sh.wait_for_prompts(n + 1);
    sh.run("print -r -- W=${w:-unset}; kill %1");
    let text = sh.text();
    assert!(text.contains("once your jobs are done") && text.contains("W=1"), "{text}");

    // Gone: no longer signaled.
    sh.pty.write(b"exit\r").unwrap();
    while sh.rx.recv_timeout(Duration::from_secs(2)).is_ok() {}
    let _ = sh.pty.try_wait();
    svc.untrack_shell(pid);
    assert_eq!(svc.reload_shells(), 0);
}

/// Installed plugins load in a real managed shell: aliases with setting
/// arguments, variables, a generated config file, the plugin's own code
/// (seeing its settings), and `goatty-status` reporting them, including a
/// plugin whose required command is missing.
#[test]
fn plugins_load_in_a_real_shell() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let plugin = |id: &str, toml: &str, files: &[(&str, &str)]| {
        let dir = sb.home.join("plugins-src").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), toml).unwrap();
        for (rel, text) in files {
            std::fs::create_dir_all(dir.join(rel).parent().unwrap()).unwrap();
            std::fs::write(dir.join(rel), text).unwrap();
        }
        dir
    };
    let meow = plugin(
        "meow",
        r#"
schema = 1
id = "meow"
name = "Meow"
version = "1.0.0"
summary = "cat, with options"
publisher = "tests"
[requires]
commands = ["cat"]
[[setting]]
id = "numbers"
label = "Number lines"
type = "bool"
default = true
arg = "-n"
[[setting]]
id = "mode"
label = "Mode"
type = "choice"
default = "purr"
choices = [{ value = "purr", label = "Purr" }, { value = "hiss", label = "Hiss" }]
[[alias]]
name = "meow"
command = "cat"
args = ["{args}"]
[[env]]
name = "MEOW_MODE"
value = "it's {mode}"
[[file]]
name = "meowrc"
env = "MEOW_CONFIG"
lines = ["mode={mode}", "{numbers.arg}"]
[zsh]
init = "zsh/init.zsh"
[permissions]
shell-code = "defines the greet function the plugin is about"
"#,
        &[("zsh/init.zsh", "typeset -g _meow=\"${goatty_settings[mode]}:$+goatty_settings\"\ngreet() { print -r -- \"greet:$_meow\" }\n")],
    );
    let ghost = plugin(
        "ghost",
        "schema = 1\nid = \"ghost\"\nname = \"Ghost\"\nversion = \"1.0.0\"\nsummary = \"needs a missing command\"\npublisher = \"tests\"\n[requires]\ncommands = [\"ghost-cmd\"]\n[[alias]]\nname = \"boo\"\ncommand = \"ghost-cmd\"\n",
        &[],
    );
    let none = goatty_plugin::fetch::Fixed::default();
    let p = goatty_shell::plugins::Plugins { dirs: &sb.dirs, fetch: &none, builtin: &[] };
    for dir in [&meow, &ghost] {
        let c = goatty_shell::plugins::Candidate::local(dir).unwrap();
        let plan = p.plan(&c, &HostSettings::default()).unwrap();
        p.install(&c, &plan.digest, &HostSettings::default()).unwrap();
    }

    let launch = sb.launch_with(no_highlighting(), None);
    let out = sb.run(
        &launch,
        "print -r -- \"alias=$aliases[meow] mode=$MEOW_MODE boo=${+aliases[boo]}\"; command cat -- \"$MEOW_CONFIG\"; greet; print -r -- after=${+goatty_settings}; goatty-status",
    );
    let text = stdout(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.trim().is_empty(), "startup printed errors:\n{stderr}");
    assert!(text.contains("alias=cat -n mode=it's purr boo=0"), "{text}");
    assert!(text.contains("mode=purr\n-n\n"), "generated config file: {text}");
    assert!(text.contains("greet:purr:1"), "init code saw its settings: {text}");
    assert!(text.contains("after=0"), "settings are unset after the init code: {text}");
    let status = goatty_shell::doctor::parse_status(&text);
    let state = |n: &str| status.iter().find(|f| f.name == n).map(|f| f.state.clone());
    use goatty_shell::doctor::State;
    assert!(matches!(state("plugin:meow"), Some(State::Ok(d)) if d.ends_with("/cat")), "{status:?}");
    assert_eq!(state("plugin:ghost"), Some(State::Missing));
}

/// The built-in plugins in a real shell (#98): fzf, then Atuin (so Ctrl+R
/// stays Atuin's), Atuin's setting reaching its init, a zsh plugin found
/// through a managed install from before plugins existed, and the fallback
/// prompt with Starship turned off (reported as off).
#[test]
fn builtin_plugins_load_in_their_slots() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let sysbin = sb.home.join("sysbin");
    exe(
        &sysbin.join("fzf"),
        "#!/bin/sh
[ \"$1\" = --zsh ] && echo 'fzf_w() { :; }; zle -N fzf_w; bindkey \"^R\" fzf_w; bindkey \"^T\" fzf_w'\n",
    );
    exe(
        &sysbin.join("atuin"),
        "#!/bin/sh
echo \"print -r -- atuin-args:$*\"; echo 'atuin_w() { :; }; zle -N atuin_w; bindkey \"^R\" atuin_w'\n",
    );
    // goatty's own copy of autosuggestions, laid out like the old tool installer did.
    let root = sb.dirs.tools_dir().join("autosuggestions/0.7.1/zsh-autosuggestions-0.7.1");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("zsh-autosuggestions.zsh"), "typeset -g fake_suggest=\"${(j:,:)ZSH_AUTOSUGGEST_STRATEGY}\"\n").unwrap();
    std::fs::create_dir_all(sb.dirs.plugins_dir()).unwrap();
    std::os::unix::fs::symlink(&root, sb.dirs.plugins_dir().join("autosuggestions")).unwrap();

    let mut settings = no_highlighting();
    settings.edit_plugin("autosuggestions", |p| {
        p.settings.insert("strategy".into(), "history".into());
    });
    settings.edit_plugin("starship", |p| p.enabled = false);
    let launch = sb.launch_with(settings, None);
    let out = pty_session(
        &sb,
        &launch,
        &[
            "bindkey '^R'; bindkey '^T'",
            "[[ $PROMPT == *'%F{blue}%~%f'* ]] && p=fallback; print -r -- strat=${(j:,:)ZSH_AUTOSUGGEST_STRATEGY} suggest=$fake_suggest prompt=$p",
            "goatty-status",
        ],
    );
    assert!(out.contains("atuin-args:init zsh --disable-up-arrow"), "{out}");
    assert!(out.contains("\"^R\" atuin_w") && out.contains("\"^T\" fzf_w"), "Atuin loads after fzf: {out}");
    let host_copy = Path::new("/usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh").exists()
        || Path::new("/usr/share/zsh-autosuggestions/zsh-autosuggestions.zsh").exists();
    assert!(out.contains("strat=history "), "the plugin's setting reached it: {out}");
    if !host_copy {
        assert!(out.contains("suggest=history "), "the managed copy loaded after its options were set: {out}");
    }
    assert!(out.contains("prompt=fallback"), "goatty's fallback prompt: {out}");
    assert!(out.contains("starship\toff"), "{out}");
    assert!(out.contains("syntax-highlighting\toff"), "{out}");
    assert!(out.contains("fzf\tok\t") && out.contains("atuin\tok\t"), "{out}");
}

/// The user's aliases from host.json (#88): defined in a real shell,
/// quoted intact, a switched-off one left out, one with a plugin alias's
/// name replacing it, and a plugin alias the user switched off missing.
#[test]
fn user_aliases_in_a_real_shell() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    let dir = sb.home.join("plugins-src/kitty");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "schema = 1\nid = \"kitty\"\nname = \"Kitty\"\nversion = \"1.0.0\"\nsummary = \"s\"\npublisher = \"tests\"\n[requires]\ncommands = [\"cat\"]\n\
         [[alias]]\nname = \"meow\"\ncommand = \"cat\"\n[[alias]]\nname = \"purr\"\ncommand = \"cat\"\nargs = [\"-n\"]\n[[alias]]\nname = \"hiss\"\ncommand = \"cat\"\n",
    )
    .unwrap();
    let none = goatty_plugin::fetch::Fixed::default();
    let p = goatty_shell::plugins::Plugins { dirs: &sb.dirs, fetch: &none, builtin: &[] };
    let c = goatty_shell::plugins::Candidate::local(&dir).unwrap();
    let plan = p.plan(&c, &HostSettings::default()).unwrap();
    p.install(&c, &plan.digest, &HostSettings::default()).unwrap();

    let mut settings = no_highlighting();
    let alias = |command: &str, group: &str, enabled: bool| goatty_config::UserAlias {
        group: group.into(),
        enabled,
        ..goatty_config::UserAlias::new(command)
    };
    settings.aliases.insert("say".into(), alias(r#"print -r -- "it's ok" | cat"#, "Fun", true));
    settings.aliases.insert("meow".into(), alias("print -r -- mine", "", true));
    settings.aliases.insert("nope".into(), alias("print -r -- nope", "Fun", false));
    settings.aliases.insert("..".into(), alias("cd ..", "", true));
    settings.plugins.entry("kitty".into()).or_default().disabled_aliases.insert("hiss".into());
    let launch = sb.launch_with(settings, None);
    let out =
        sb.run(&launch, "say; meow; print -r -- \"purr=$aliases[purr] hiss=${+aliases[hiss]} nope=${+aliases[nope]} dots=$aliases[..]\"");
    let text = stdout(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.trim().is_empty(), "startup printed errors:\n{stderr}");
    assert!(text.contains("it's ok\nmine\n"), "{text}");
    assert!(text.contains("purr=cat -n hiss=0 nope=0 dots=cd .."), "{text}");
}

/// A reload request that arrives while the prompt is being drawn (after
/// precmd, before the line editor reads input) must still reload an idle
/// shell right away, not wait for the next command. The user's additions
/// slow the prompt down so the signal reliably lands in that window.
#[test]
fn reloads_when_signaled_while_the_prompt_draws() {
    let Some(zsh) = zsh() else { return };
    let sb = Sandbox::new(zsh);
    std::fs::create_dir_all(&sb.dirs.config).unwrap();
    std::fs::write(sb.dirs.user_zshrc(), "setopt prompt_subst\nPROMPT='$(/bin/sleep 0.6)'$PROMPT\n").unwrap();
    let launch = sb.launch_with(no_highlighting(), None);
    let svc = goatty_shell::HostService::new(Launcher::new(Some(sb.dirs.clone()), None), None);
    let mut sh = PtyShell::start(&sb, &launch);
    let pid = sh.pty.pid().unwrap();
    svc.track_shell(pid);

    sh.run("x=1");
    let n = sh.prompts();
    sh.pty.write(b"print -r -- drawing-next\r").unwrap();
    sh.wait_for(b"drawing-next\r\n");
    // The next prompt is now being drawn (its $(sleep) runs for 0.6 s).
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sh.prompts(), n, "the prompt is still being drawn");
    assert_eq!(svc.reload_shells(), 1);
    // That prompt, then the reloaded shell's.
    sh.wait_for_prompts(n + 2);
    sh.run("print -r -- X=${x:-unset} P=$$");
    assert!(sh.text().contains(&format!("X=unset P={pid}")), "{}", sh.text());
    sh.pty.write(b"exit\r").unwrap();
}
