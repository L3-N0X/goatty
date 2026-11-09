//! Connecting to goattyd on another host, in a small window of its own.
//!
//! `goatty --ssh host` (and `--via`) opens this window instead of blocking
//! before the GUI starts, and so does picking a host in the command palette
//! (`crate::hosts`). It connects off the UI thread and, when that
//! works, opens the terminal window and closes itself. When it doesn't, it
//! explains what's wrong and offers what goatty can do about it:
//!
//! * goattyd missing → install it (`goatty_client::remote::install`),
//! * goattyd too old → install this build's goattyd and restart the daemon
//!   (that ends the host's sessions, so the button says so),
//! * either way, or when goattyd can't run there → **plain SSH**: `ssh host`
//!   in a session of the *local* goattyd. Nothing is installed on the host,
//!   and nothing of goatty runs there; the session survives closing the
//!   window, but not the network dropping.
//!
//! ssh's own questions (passphrases, host keys) come up as separate windows
//! from `crate::askpass`. Each host gets a terminal window of its own;
//! tabs for several hosts in one window are #37/#4.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use goatty_client::remote::{self, Bundled, Platform, Probe};
use goatty_client::{Connection, Endpoint, RemoteProblem, SshTarget};
use goatty_shell::HostService;
use goatty_term::TermSize;
use gpui::{
    App, AppContext, AsyncWindowContext, Bounds, Context, FocusHandle, Focusable, FontWeight, KeyDownEvent, Render, SharedString,
    WeakEntity, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::session::{AttachMode, Session};
use crate::settings::AppSettings;
use crate::settings_window::HostLink;
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
}

/// The terminal window's first pane size; it resizes on first layout.
fn initial_size() -> TermSize {
    TermSize::new(100, 30)
}

/// Open the connect window and start connecting.
pub fn open(cx: &mut App, plan: ConnectPlan) {
    let bounds = Bounds::centered(None, size(px(580.), px(340.)), cx);
    let label = plan.endpoint.label().to_string();
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("dev.goatty.goatty".into()),
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title(&format!("Connecting to {label} — goatty"));
            cx.new(|cx| {
                let mut view = ConnectView { plan, step: Step::Working(String::new()), focus: cx.focus_handle() };
                view.focus.focus(window, cx);
                view.connect(window, cx);
                view
            })
        },
    );
    if let Err(e) = opened {
        log::error!("couldn't open the connect window: {e:#}");
        // From the command line there's nothing else; from the palette the
        // other windows stay.
        if cx.windows().is_empty() {
            cx.quit();
        }
    }
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
    /// Upload goattyd if needed, stop the running (incompatible) daemon, connect.
    Restart,
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

struct ConnectView {
    plan: ConnectPlan,
    step: Step,
    focus: FocusHandle,
}

impl Focusable for ConnectView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

type Ready = (Startup, HostLink);

impl ConnectView {
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
        cx.notify();
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let result = cx.background_executor().spawn(async move { job() }).await;
            let _ = this.update_in(cx, |view, window, cx| view.finish(result, window, cx));
        })
        .detach();
    }

    fn finish(&mut self, result: Result<Ready>, window: &mut Window, cx: &mut Context<'_, Self>) {
        match result {
            Ok((startup, link)) => {
                crate::app::open_terminal_window(cx, startup, link, self.plan.open_settings.take());
                window.remove_window();
            }
            Err(e) => {
                log::warn!("connecting to {}: {e:#}", self.plan.endpoint.label());
                let ask = match e.downcast_ref::<RemoteProblem>() {
                    Some(problem) => describe(problem, remote::find_bundled),
                    None => failed(self.plan.endpoint.label(), &e, self.ssh_target().is_some()),
                };
                self.step = Step::Ask(ask);
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
        let (endpoint, mode) = (self.plan.endpoint.clone(), self.plan.mode);
        self.work(format!("Connecting to {}…", endpoint.label()), window, cx, move || {
            let conn = Connection::open(endpoint, crate::CLIENT)?;
            let startup = saved::startup(&conn, initial_size(), mode)?;
            Ok((startup, HostLink::Daemon(conn)))
        });
    }

    fn choose(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<'_, Self>) {
        let upload = match &self.step {
            Step::Ask(ask) => ask.upload.clone(),
            Step::Working(_) => return,
        };
        match choice {
            Choice::Cancel => window.remove_window(),
            Choice::Retry => self.connect(window, cx),
            Choice::Plain => {
                let Some(target) = self.ssh_target() else { return };
                self.work(format!("Opening ssh {}…", target.host), window, cx, move || plain_ssh(&target.host));
            }
            Choice::Install | Choice::Restart => {
                let Some(target) = self.ssh_target() else { return };
                let (endpoint, mode) = (self.plan.endpoint.clone(), self.plan.mode);
                let restart = choice == Choice::Restart;
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
                    let startup = saved::startup(&conn, initial_size(), mode)?;
                    Ok((startup, HostLink::Daemon(conn)))
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
/// this window instead.
pub fn plain_ssh(host: &str) -> Result<Ready> {
    let argv = vec!["ssh".to_string(), "--".into(), host.to_string()];
    match Connection::open(Endpoint::Local, crate::CLIENT) {
        Ok(conn) => match Session::daemon_run(conn.clone(), initial_size(), argv.clone()) {
            Ok(session) => return Ok((Startup::Session(session), HostLink::Daemon(conn))),
            Err(e) => log::warn!("the local goattyd can't run ssh ({e:#}); running it in this window"),
        },
        Err(e) => log::warn!("no local goattyd ({e:#}); running ssh in this window"),
    }
    let session = Session::local_command(initial_size(), argv).context("couldn't start ssh")?;
    let daemon = std::env::current_exe().ok().map(|e| e.with_file_name("goattyd")).filter(|d| d.exists());
    Ok((Startup::Session(session), HostLink::Local(Arc::new(HostService::from_env(daemon)))))
}

impl Render for ConnectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let ui_font = cx.global::<AppSettings>().ui_font.clone();
        let root = div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .bg(ui.bg)
            .font_family(ui_font)
            .text_sm()
            .text_color(ui.text);
        match &self.step {
            Step::Working(label) => root.items_center().justify_center().child(
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
                root.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon.0.svg().size(px(18.)).text_color(icon.1))
                        .child(div().text_base().font_weight(FontWeight::BOLD).child(SharedString::from(ask.title.clone()))),
                )
                .children(ask.body.iter().map(|p| div().text_color(ui.text).child(SharedString::from(p.clone()))))
                .when_some(ask.warning.clone(), |d, w| d.child(div().text_color(ui.warning).font_weight(FontWeight::SEMIBOLD).child(w)))
                .child(div().flex_1())
                .child(div().flex().justify_end().gap_2().children(buttons))
            }
        }
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
}
