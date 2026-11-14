//! Connecting to a goattyd, shown in the tab that waits for it (#37).
//!
//! Every terminal window opens right away. A tab whose session isn't there
//! yet shows a [`ConnectView`]: it connects off the UI thread (to the local
//! daemon, or over SSH, which can take seconds and ask questions) and, when
//! that works, hands the workspace the session or the restored layout
//! ([`ConnectEvent::Ready`]), which replaces the view in the tab. When it
//! doesn't, it explains what's wrong and offers what goatty can do about it:
//!
//! * goattyd missing → install it (`goatty_client::remote::install`),
//! * goattyd too old → install this build's goattyd and restart the daemon
//!   (that ends the host's sessions, so the button says so),
//! * either way, or when goattyd can't run there → **plain SSH**: `ssh host`
//!   in a session of the *local* goattyd. Nothing is installed on the host,
//!   and nothing of goatty runs there; the session survives closing the
//!   window, but not the network dropping.
//!
//! The local daemon gets the same treatment: one that speaks no protocol
//! this goatty does, or that runs an older build than the installed goattyd
//! (after an update or a `cargo build`), is restarted with the installed
//! one. Right away when it holds no sessions (nothing is lost), else only
//! after asking; "Keep it running" is remembered for that daemon
//! (`goatty_client::local`).
//!
//! ssh's own questions (passphrases, host keys) come up as separate windows
//! from `crate::askpass`. Connections come from `crate::hosts::pool`, so a
//! second tab on a host reuses the first one's link.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use goatty_client::local::{self, LocalProblem, Outdated};
use goatty_client::remote::{self, Bundled, Platform, Probe};
use goatty_client::{Connection, Endpoint, RemoteProblem, SshTarget};
use goatty_shell::HostService;
use goatty_term::TermSize;
use gpui::{
    App, AsyncWindowContext, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, Render, SharedString, WeakEntity,
    Window, div, prelude::*, px,
};

use crate::hosts::pool;
use crate::session::{AttachMode, Session, Spawner, plain_ssh_argv};
use crate::settings::AppSettings;
use crate::toast::{self, Toast};
use crate::ui::controls::{ButtonKind, button};
use crate::ui::icons::Icon;
use crate::ui::style::Ui;
use crate::workspace::saved::{self, Startup};

/// What `goatty` was asked to connect to.
#[derive(Clone)]
pub struct ConnectPlan {
    pub endpoint: Endpoint,
    pub mode: AttachMode,
    /// `--settings[=PAGE]`, opened with the terminal window.
    pub open_settings: Option<String>,
    /// Plain SSH right away (the host is set to it): `ssh host` in a local
    /// session instead of goattyd on the host.
    pub plain: bool,
    /// A connection to the endpoint made already (the session picker's).
    pub conn: Option<Arc<Connection>>,
}

/// The terminal window's first pane size; it resizes on first layout.
fn initial_size() -> TermSize {
    TermSize::new(100, 30)
}

enum Step {
    Working(String),
    Ask(Ask),
}

/// What the user can do about a failed connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Upload goattyd, then connect.
    Install,
    /// Upload goattyd if needed, stop the running (incompatible or outdated)
    /// daemon, connect.
    Restart,
    /// Use the outdated local daemon anyway (and don't ask again for it).
    Keep,
    Plain,
    Retry,
    Cancel,
}

/// A question for the user: what happened and the possible ways on.
#[derive(Debug, Clone)]
pub struct Ask {
    pub title: String,
    pub body: Vec<String>,
    /// Shown highlighted: what a choice costs.
    pub warning: Option<String>,
    /// In button order; the last one is the default (Enter).
    pub choices: Vec<(Choice, String)>,
    /// What [`Choice::Install`]/[`Choice::Restart`] upload (`None`: nothing,
    /// the right goattyd is installed already).
    pub upload: Option<Bundled>,
}

const PLAIN_NOTE: &str = "Plain SSH opens a normal ssh session in a local goatty session instead. Nothing is installed on the host, \
                          and goatty's shell setup, settings and plugins don't reach it there.";

const SETUP_NOTE: &str = "Your shell setup comes along: this computer's prompt, aliases, plugins and their settings, and goatty's \
                          tools, so the shell there feels like the one here (Settings → Hosts can turn that off).";

fn mib(path: &std::path::Path) -> String {
    std::fs::metadata(path).map(|m| format!("{:.1} MiB", m.len() as f64 / (1024. * 1024.))).unwrap_or_else(|_| "? MiB".into())
}

/// Turn a connect problem into a question. `find` looks up a goattyd to
/// upload for a target (`remote::find_bundled` outside tests).
pub fn describe(problem: &RemoteProblem, find: impl Fn(&str) -> Option<Bundled>) -> Ask {
    let ver = env!("CARGO_PKG_VERSION");
    let dest = format!("~/{}", remote::BIN_DIR);
    let cancel = (Choice::Cancel, "Cancel".to_string());
    let plain = (Choice::Plain, "Plain SSH".to_string());
    match problem {
        RemoteProblem::NotInstalled { host, platform } => match platform.target() {
            None => Ask {
                title: format!("goattyd doesn't run on {host}"),
                body: vec![
                    format!("{host} is {platform}. goattyd, goatty's session daemon, is available for Linux on x86_64 and aarch64 so far."),
                    PLAIN_NOTE.into(),
                ],
                warning: None,
                choices: vec![cancel, plain],
                upload: None,
            },
            Some(target) => match find(target) {
                Some(bundled) => {
                    let mut body = vec![
                        format!(
                            "goatty keeps your shells on {host} in goattyd, its session daemon: they survive disconnects and closing \
                             the window, and get goatty's shell setup. It isn't installed there yet."
                        ),
                        format!(
                            "goatty can copy goattyd {ver} for {platform} ({}) to {dest} on {host}. Nothing else changes: no sudo, \
                             no PATH or dotfile edits.",
                            mib(&bundled.path)
                        ),
                    ];
                    body.push(SETUP_NOTE.into());
                    if bundled.this_build {
                        body.push(
                            "This development build copies its own goattyd, which may not run on a host with an older C library. \
                             goatty checks that it runs before using it."
                                .into(),
                        );
                    }
                    body.push(PLAIN_NOTE.into());
                    Ask {
                        title: format!("Install goattyd on {host}?"),
                        body,
                        warning: None,
                        choices: vec![cancel, plain, (Choice::Install, "Install goattyd".into())],
                        upload: Some(bundled),
                    }
                }
                None => Ask {
                    title: format!("No goattyd to install on {host}"),
                    body: vec![
                        format!(
                            "{host} needs goattyd for {platform}, and this goatty doesn't include one (release archives carry them in \
                             remote/). Install goattyd there yourself (`cargo install --path crates/goatty-daemon`), then retry."
                        ),
                        PLAIN_NOTE.into(),
                    ],
                    warning: None,
                    choices: vec![cancel, plain, (Choice::Retry, "Retry".into())],
                    upload: None,
                },
            },
        },
        RemoteProblem::Incompatible { host, mismatch, probe } => {
            if mismatch.daemon_is_newer {
                return Ask {
                    title: format!("goattyd on {host} is newer than this goatty"),
                    body: vec![format!("{mismatch}."), "Plain SSH works in the meantime.".into()],
                    warning: None,
                    choices: vec![cancel, plain],
                    upload: None,
                };
            }
            let warning = Some(format!("Restarting goattyd ends the sessions running on {host}."));
            if probe.as_ref().is_some_and(Probe::runs_ours) {
                return Ask {
                    title: format!("Restart goattyd on {host}?"),
                    body: vec![format!("{mismatch}."), format!("goattyd {ver} is installed there, but an older daemon is still running.")],
                    warning,
                    choices: vec![cancel, plain, (Choice::Restart, "Restart goattyd".into())],
                    upload: None,
                };
            }
            let platform: Option<&Platform> = probe.as_ref().map(|p| &p.platform);
            match platform.and_then(|p| Some((p, find(p.target()?)?))) {
                Some((platform, bundled)) => Ask {
                    title: format!("Update goattyd on {host}?"),
                    body: vec![
                        format!("{mismatch}."),
                        format!(
                            "goatty can copy goattyd {ver} for {platform} ({}) to {dest} on {host} and restart the daemon with it.",
                            mib(&bundled.path)
                        ),
                    ],
                    warning,
                    choices: vec![cancel, plain, (Choice::Restart, "Update and restart".into())],
                    upload: Some(bundled),
                },
                None => Ask {
                    title: format!("goattyd on {host} is too old"),
                    body: vec![format!("{mismatch}."), "Update goattyd there and retry, or use plain SSH.".into()],
                    warning: None,
                    choices: vec![cancel, plain, (Choice::Retry, "Retry".into())],
                    upload: None,
                },
            }
        }
    }
}

/// A connect failure goatty can't fix.
fn failed(label: &str, error: &anyhow::Error, ssh: bool) -> Ask {
    let mut choices = vec![(Choice::Cancel, "Close".to_string())];
    if ssh {
        choices.push((Choice::Plain, "Plain SSH".into()));
    }
    choices.push((Choice::Retry, "Retry".into()));
    Ask { title: format!("Couldn't connect to {label}"), body: vec![format!("{error:#}")], warning: None, choices, upload: None }
}

/// Turn a problem with the local daemon into a question. `installed`: the
/// goattyd a restart starts.
pub fn describe_local(problem: &LocalProblem, installed: Option<&std::path::Path>) -> Ask {
    let close = (Choice::Cancel, "Close".to_string());
    let restart = (Choice::Restart, "Restart goattyd".to_string());
    let starts = match installed {
        Some(p) => format!("Restarting stops it and starts {}.", p.display()),
        None => "Restarting stops it and starts goattyd again.".into(),
    };
    match problem {
        LocalProblem::Outdated { outdated, sessions } => {
            let n = match sessions {
                1 => "1 session".to_string(),
                n => format!("{n} sessions"),
            };
            Ask {
                title: "Restart goattyd with the new build?".into(),
                body: vec![
                    format!(
                        "goattyd, which keeps your sessions, is still running an older build ({}, pid {}). {starts}",
                        outdated.running.display(),
                        outdated.pid
                    ),
                    "Keeping it works, but without what changed since. goatty won't ask again until it restarts.".into(),
                ],
                warning: Some(format!("Restarting goattyd ends the {n} running in it.")),
                choices: vec![(Choice::Keep, "Keep it running".into()), restart],
                upload: None,
            }
        }
        LocalProblem::Incompatible(mismatch) => {
            let daemon = match (&mismatch.daemon_version, mismatch.daemon_protocol) {
                (Some(v), Some(p)) => format!("goattyd {v} (protocol v{p})"),
                (None, Some(p)) => format!("a goattyd with protocol v{p}"),
                _ => "a goattyd".into(),
            };
            let (min, max) = (goatty_proto::MIN_PROTOCOL_VERSION, goatty_proto::PROTOCOL_VERSION);
            let age = if mismatch.daemon_is_newer { "newer" } else { "older" };
            Ask {
                title: "Restart goattyd?".into(),
                body: vec![format!("The running {daemon} is {age} than this goatty can talk to (protocol v{min}–v{max}). {starts}")],
                warning: Some("Restarting goattyd ends the sessions running in it.".into()),
                choices: vec![close, (Choice::Retry, "Retry".into()), restart],
                upload: None,
            }
        }
    }
}

/// Stop the local daemon, then connect to the one that starts (the installed
/// build). A connection to it that's in use (`conn`, else the pooled one)
/// reconnects by itself; wait for that rather than racing it to start one.
fn restart_local(conn: Option<Arc<Connection>>) -> Result<Arc<Connection>> {
    let conn = conn.or_else(|| pool::live(&Endpoint::Local));
    let old = conn.as_ref().map(|c| c.daemon_pid());
    local::stop_daemon(true).context("couldn't stop the running goattyd")?;
    let Some(conn) = conn else { return pool::open(Endpoint::Local) };
    conn.reconnect();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(conn.is_connected() && Some(conn.daemon_pid()) != old) {
        anyhow::ensure!(Instant::now() < deadline, "the new goattyd didn't come up");
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(conn)
}

/// Check the local daemon is the installed build: restart it right away
/// when it holds no sessions, else ask ([`LocalProblem::Outdated`]).
/// `restarted` is set when it was restarted.
fn check_local(conn: Arc<Connection>, restarted: &AtomicBool) -> Result<Arc<Connection>> {
    let Some(outdated) = local::outdated(conn.daemon_pid()) else { return Ok(conn) };
    if local::declined(&outdated) {
        return Ok(conn);
    }
    let sessions = conn.list_sessions()?.len();
    if sessions > 0 {
        return Err(LocalProblem::Outdated { outdated, sessions }.into());
    }
    log::info!("restarting goattyd (no sessions): {} replaces {}", outdated.installed.display(), outdated.running.display());
    let conn = restart_local(Some(conn))?;
    restarted.store(true, Ordering::Relaxed);
    Ok(conn)
}

/// What the connect view tells the tab it sits in.
pub enum ConnectEvent {
    /// The session (or layout) is there: take it with [`ConnectView::take_ready`].
    Ready,
    /// The user gave up: close the tab.
    Cancelled,
    /// What it shows changed (the window title follows it).
    Changed,
}

pub struct ConnectView {
    plan: ConnectPlan,
    step: Step,
    focus: FocusHandle,
    ready: Option<Ready>,
    /// Check that the local daemon is the installed build (off once the
    /// user chose, or it was restarted).
    check_local: bool,
    /// The outdated local daemon being asked about.
    outdated: Option<Outdated>,
    /// The local daemon was restarted on the way (tell the user).
    restarted: Arc<AtomicBool>,
    /// goattyd is being installed on the host (a fresh install, not an update).
    installing: bool,
    /// It was, and the connection works: the shell setup comes along.
    installed: bool,
}

impl EventEmitter<ConnectEvent> for ConnectView {}

impl Focusable for ConnectView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A connected tab: what it starts with, and where its next sessions come from.
pub type Ready = (Startup, Spawner);

impl ConnectView {
    /// A view connecting for `plan`, started right away.
    pub fn new(plan: ConnectPlan, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let mut view = Self {
            plan,
            step: Step::Working(String::new()),
            focus: cx.focus_handle(),
            ready: None,
            check_local: true,
            outdated: None,
            restarted: Arc::default(),
            installing: false,
            installed: false,
        };
        view.connect(window, cx);
        view
    }

    /// The host, for the tab's label.
    pub fn host_label(&self) -> &str {
        match &self.plan.endpoint {
            Endpoint::Local => "local",
            e => e.label(),
        }
    }

    /// The window title while this tab is active.
    pub fn title(&self) -> String {
        match &self.step {
            Step::Working(_) => format!("Connecting to {} — goatty", self.host_label()),
            Step::Ask(ask) => format!("{} — goatty", ask.title),
        }
    }

    /// `--settings`, to open once connected.
    pub fn take_open_settings(&mut self) -> Option<String> {
        self.plan.open_settings.take()
    }

    /// goattyd was freshly installed on the host for this connection (then
    /// the shell setup is synced without asking again: the install
    /// question said it comes along).
    pub fn take_installed(&mut self) -> bool {
        std::mem::take(&mut self.installed)
    }

    /// The result after [`ConnectEvent::Ready`].
    pub fn take_ready(&mut self) -> Option<Ready> {
        self.ready.take()
    }

    fn ssh_target(&self) -> Option<SshTarget> {
        match &self.plan.endpoint {
            Endpoint::Ssh(t) => Some(t.clone()),
            _ => None,
        }
    }

    /// Run `job` off the UI thread with `label` showing; then open the
    /// terminal or ask what to do.
    fn work(
        &mut self,
        label: String,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        job: impl FnOnce() -> Result<Ready> + Send + 'static,
    ) {
        self.step = Step::Working(label);
        cx.emit(ConnectEvent::Changed);
        cx.notify();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let result = cx.background_executor().spawn(async move { job() }).await;
            let _ = this.update_in(cx, |view, window, cx| view.finish(result, window, cx));
        })
        .detach();
    }

    fn finish(&mut self, result: Result<Ready>, _window: &mut Window, cx: &mut Context<'_, Self>) {
        match result {
            Ok(ready) => {
                if self.restarted.swap(false, Ordering::Relaxed) {
                    toast::show(
                        cx,
                        Toast::success("Restarted goattyd").body("It was an older build and had no sessions.").key("goattyd-restarted"),
                    );
                }
                self.installed = std::mem::take(&mut self.installing);
                self.ready = Some(ready);
                cx.emit(ConnectEvent::Ready);
            }
            Err(e) => {
                log::warn!("connecting to {}: {e:#}", self.plan.endpoint.label());
                self.outdated = None;
                self.installing = false;
                let ask = if let Some(problem) = e.downcast_ref::<RemoteProblem>() {
                    describe(problem, remote::find_bundled)
                } else if let Some(problem) = e.downcast_ref::<LocalProblem>() {
                    if let LocalProblem::Outdated { outdated, .. } = problem {
                        self.outdated = Some(outdated.clone());
                    }
                    describe_local(problem, local::installed_daemon().as_deref())
                } else {
                    failed(self.plan.endpoint.label(), &e, self.ssh_target().is_some())
                };
                self.step = Step::Ask(ask);
                cx.emit(ConnectEvent::Changed);
                cx.notify();
            }
        }
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.plan.plain
            && let Some(target) = self.ssh_target()
        {
            self.work(format!("Opening ssh {}…", target.host), window, cx, move || plain_ssh(&target.host));
            return;
        }
        let (endpoint, mode, conn) = (self.plan.endpoint.clone(), self.plan.mode, self.plan.conn.take());
        let label = if endpoint.is_local() { "Starting a shell…".to_string() } else { format!("Connecting to {}…", endpoint.label()) };
        let check = endpoint.is_local() && self.check_local;
        let restarted = self.restarted.clone();
        self.work(label, window, cx, move || {
            let mut conn = match conn {
                Some(c) => c,
                None => pool::open(endpoint)?,
            };
            if check {
                conn = check_local(conn, &restarted)?;
            }
            let startup = saved::startup(&conn, initial_size(), mode)?;
            Ok((startup, Spawner::Daemon(conn)))
        });
    }

    fn choose(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<'_, Self>) {
        let upload = match &self.step {
            Step::Ask(ask) => ask.upload.clone(),
            Step::Working(_) => return,
        };
        match choice {
            Choice::Cancel => cx.emit(ConnectEvent::Cancelled),
            Choice::Retry => self.connect(window, cx),
            Choice::Keep => {
                self.check_local = false;
                if let Some(o) = self.outdated.take() {
                    cx.background_executor().spawn(async move { local::decline(&o) }).detach();
                }
                self.connect(window, cx);
            }
            Choice::Restart if self.plan.endpoint.is_local() => {
                self.check_local = false;
                self.outdated = None;
                let mode = self.plan.mode;
                self.work("Restarting goattyd…".into(), window, cx, move || {
                    let conn = restart_local(None)?;
                    let startup = saved::startup(&conn, initial_size(), mode)?;
                    Ok((startup, Spawner::Daemon(conn)))
                });
            }
            Choice::Plain => {
                let Some(target) = self.ssh_target() else { return };
                self.work(format!("Opening ssh {}…", target.host), window, cx, move || plain_ssh(&target.host));
            }
            Choice::Install | Choice::Restart => {
                let Some(target) = self.ssh_target() else { return };
                let (endpoint, mode) = (self.plan.endpoint.clone(), self.plan.mode);
                let restart = choice == Choice::Restart;
                self.installing = choice == Choice::Install;
                let label = if upload.is_some() {
                    format!("Installing goattyd on {}…", target.host)
                } else {
                    format!("Restarting goattyd on {}…", target.host)
                };
                self.work(label, window, cx, move || {
                    if let Some(bundled) = upload {
                        remote::install(&target, &bundled.path)?;
                    }
                    if restart {
                        remote::stop_daemon(&target)?;
                    }
                    let conn = Connection::open(endpoint, crate::CLIENT)?;
                    pool::add(&conn);
                    let startup = saved::startup(&conn, initial_size(), mode)?;
                    Ok((startup, Spawner::Daemon(conn)))
                });
            }
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Step::Ask(ask) = &self.step else { return };
        let choice = match ev.keystroke.key.as_str() {
            "enter" => ask.choices.last().map(|c| c.0),
            "escape" => Some(Choice::Cancel),
            _ => None,
        };
        if let Some(c) = choice {
            cx.stop_propagation();
            self.choose(c, window, cx);
        }
    }
}

/// `ssh host` in a new session of the local goattyd (it survives closing the
/// window). If the local daemon can't (too old, won't start), in a PTY of
/// this window instead. New tabs from it run `ssh host` again.
pub fn plain_ssh(host: &str) -> Result<Ready> {
    let argv = plain_ssh_argv(host);
    match pool::open(Endpoint::Local) {
        Ok(conn) => match Session::daemon_run(conn.clone(), initial_size(), argv.clone()) {
            Ok(session) => {
                let spawner = Spawner::Plain { host: host.to_string(), base: Box::new(Spawner::Daemon(conn)) };
                return Ok((Startup::Session(session), spawner));
            }
            Err(e) => log::warn!("the local goattyd can't run ssh ({e:#}); running it in this window"),
        },
        Err(e) => log::warn!("no local goattyd ({e:#}); running ssh in this window"),
    }
    let session = Session::local_command(initial_size(), argv).context("couldn't start ssh")?;
    let daemon = std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists());
    let base = Spawner::Local(Arc::new(HostService::from_env(daemon)));
    Ok((Startup::Session(session), Spawner::Plain { host: host.to_string(), base: Box::new(base) }))
}

impl Render for ConnectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let ui_font = cx.global::<AppSettings>().ui_font.clone();
        let outer = div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(ui.bg)
            .font_family(ui_font)
            .text_sm()
            .text_color(ui.text);
        let card = div().w(px(560.)).max_w_full().flex().flex_col().gap_3();
        let card = match &self.step {
            Step::Working(label) => card.items_center().child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::Loader.svg().size(px(18.)).text_color(ui.accent))
                    .child(div().text_base().child(SharedString::from(label.clone()))),
            ),
            Step::Ask(ask) => {
                let last = ask.choices.len().saturating_sub(1);
                let buttons = ask.choices.iter().enumerate().map(|(i, (choice, label))| {
                    let kind = match choice {
                        Choice::Restart => ButtonKind::Danger,
                        _ if i == last => ButtonKind::Primary,
                        _ => ButtonKind::Normal,
                    };
                    let (choice, view) = (*choice, cx.entity().downgrade());
                    button(SharedString::from(format!("connect-{i}")), label.clone(), kind, ui, move |window, cx| {
                        let _ = view.update(cx, |v, cx| v.choose(choice, window, cx));
                    })
                });
                let icon = if ask.warning.is_some() { (Icon::TriangleAlert, ui.warning) } else { (Icon::Info, ui.accent) };
                card.p_5()
                    .border_1()
                    .border_color(ui.border)
                    .rounded_lg()
                    .bg(ui.surface)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon.0.svg().size(px(18.)).text_color(icon.1))
                            .child(div().text_base().font_weight(FontWeight::BOLD).child(SharedString::from(ask.title.clone()))),
                    )
                    .children(ask.body.iter().map(|p| div().text_color(ui.text).child(SharedString::from(p.clone()))))
                    .when_some(ask.warning.clone(), |d, w| d.child(div().text_color(ui.warning).font_weight(FontWeight::SEMIBOLD).child(w)))
                    .child(div().flex().justify_end().gap_2().pt_2().children(buttons))
            }
        };
        outer.child(card)
    }
}

#[cfg(test)]
mod tests {
    use goatty_client::VersionMismatch;

    use super::*;

    fn linux(arch: &str) -> Platform {
        Platform { os: "Linux".into(), arch: arch.into() }
    }

    fn bundle(this_build: bool) -> impl Fn(&str) -> Option<Bundled> {
        move |t: &str| Some(Bundled { path: format!("/nonexistent/goattyd-{t}").into(), this_build })
    }

    fn choices(a: &Ask) -> Vec<Choice> {
        a.choices.iter().map(|c| c.0).collect()
    }

    #[test]
    fn missing_goattyd_offers_install_or_plain_ssh() {
        let p = RemoteProblem::NotInstalled { host: "box".into(), platform: linux("x86_64") };
        let a = describe(&p, bundle(false));
        assert_eq!(choices(&a), [Choice::Cancel, Choice::Plain, Choice::Install]);
        assert!(a.upload.is_some() && a.warning.is_none());
        assert!(a.body.iter().any(|b| b.contains("no sudo")));
        assert!(describe(&p, bundle(true)).body.iter().any(|b| b.contains("older C library")));
        // Nothing to upload: retry after installing by hand.
        assert_eq!(choices(&describe(&p, |_| None)), [Choice::Cancel, Choice::Plain, Choice::Retry]);
        // No goattyd for that platform at all.
        let mac = RemoteProblem::NotInstalled { host: "mac".into(), platform: Platform { os: "Darwin".into(), arch: "arm64".into() } };
        assert_eq!(choices(&describe(&mac, bundle(false))), [Choice::Cancel, Choice::Plain]);
    }

    #[test]
    fn old_goattyd_offers_a_restart_that_says_what_it_costs() {
        let older = VersionMismatch { daemon_protocol: Some(3), daemon_version: Some("0.0.0".into()), daemon_is_newer: false };
        let probe = |daemon: &str| Some(Probe { platform: linux("aarch64"), daemon: Some(daemon.into()) });
        let p = RemoteProblem::Incompatible { host: "box".into(), mismatch: older.clone(), probe: probe("/usr/bin/goattyd") };
        let a = describe(&p, bundle(false));
        assert_eq!(choices(&a), [Choice::Cancel, Choice::Plain, Choice::Restart]);
        assert!(a.upload.as_ref().unwrap().path.ends_with("goattyd-aarch64-linux"));
        assert!(a.warning.as_ref().unwrap().contains("ends the sessions"));
        // This build's goattyd is installed; only the running daemon is old.
        let ours = format!("/home/u/{}/{}", remote::BIN_DIR, remote::binary_name());
        let p = RemoteProblem::Incompatible { host: "box".into(), mismatch: older, probe: probe(&ours) };
        let a = describe(&p, bundle(false));
        assert!(a.upload.is_none());
        assert_eq!(a.choices.last().unwrap().1, "Restart goattyd");
        // A newer daemon: never offer to replace it from here.
        let newer = VersionMismatch { daemon_protocol: Some(99), daemon_version: None, daemon_is_newer: true };
        let p = RemoteProblem::Incompatible { host: "box".into(), mismatch: newer, probe: None };
        assert_eq!(choices(&describe(&p, bundle(false))), [Choice::Cancel, Choice::Plain]);
    }

    #[test]
    fn local_daemon_problems_offer_a_restart() {
        let older = VersionMismatch { daemon_protocol: Some(3), daemon_version: Some("0.0.0".into()), daemon_is_newer: false };
        let a = describe_local(&LocalProblem::Incompatible(older), Some(std::path::Path::new("/x/goattyd")));
        assert_eq!(choices(&a), [Choice::Cancel, Choice::Retry, Choice::Restart]);
        assert!(a.body[0].contains("older") && a.body[0].contains("/x/goattyd"), "{:?}", a.body);
        assert!(a.warning.unwrap().contains("ends the sessions"));
    }
}
