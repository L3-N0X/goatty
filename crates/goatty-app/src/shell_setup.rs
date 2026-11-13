//! Setting up a host's shell from the GUI (#40): installing goatty's
//! built-in tools with progress, and offering that when a tab connects to
//! a host that's missing them. The work happens on the host
//! (`goatty_shell::setup`, through its goattyd or in-process with
//! `--local`); the welcome window and the Diagnostics page use the same
//! pieces.
//!
//! The offer is a sticky toast, once per host per run, unless the user
//! chose "Don't ask again" (the host's `goatty-setup --dismiss` marker).
//! Showing it also tells the host, so the hint in new shells stays quiet.

use std::collections::HashSet;

use goatty_shell::setup::{self, Status, Tool};
use gpui::{App, AsyncApp, Global};

use crate::settings_window::{self, HostLink, Page};
use crate::toast::{self, Toast};

/// One tool's install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Waiting,
    Running,
    Done,
    Failed(String),
}

/// Installing a few tools, one at a time.
#[derive(Debug, Clone)]
pub struct Progress {
    /// (id, name, state)
    pub tools: Vec<(String, String, Step)>,
    /// All done (and shells asked to reload).
    pub finished: bool,
}

impl Progress {
    pub fn new<'a>(tools: impl IntoIterator<Item = &'a Tool>) -> Self {
        Self { tools: tools.into_iter().map(|t| (t.id.clone(), t.name.clone(), Step::Waiting)).collect(), finished: false }
    }

    pub fn done(&self) -> usize {
        self.tools.iter().filter(|t| t.2 == Step::Done).count()
    }

    pub fn failures(&self) -> Vec<String> {
        self.tools.iter().filter_map(|(_, _, s)| if let Step::Failed(e) = s { Some(e.clone()) } else { None }).collect()
    }

    /// The tool being installed now, as (position from 1, name).
    pub fn current(&self) -> Option<(usize, &str)> {
        self.tools.iter().position(|t| t.2 == Step::Running).map(|i| (i + 1, self.tools[i].1.as_str()))
    }

    /// "Installing fzf… (2 of 3)".
    pub fn status_line(&self) -> String {
        match self.current() {
            Some((i, name)) => format!("Installing {name}… ({i} of {})", self.tools.len()),
            None => "Installing…".into(),
        }
    }
}

/// "Starship", "Starship and fzf", "Starship, fzf and fzf-tab",
/// "Starship, fzf and 2 more".
pub fn names(list: &[&str]) -> String {
    match list {
        [] => String::new(),
        [a] => (*a).to_owned(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

/// Install `progress.tools` on the host one by one, calling `report` after
/// each change, then ask the host's running shells to reload so they pick
/// the tools up right away (an idle prompt restarts at once).
pub async fn install(link: HostLink, mut progress: Progress, cx: &mut AsyncApp, report: impl Fn(&Progress, &mut App)) -> Progress {
    for i in 0..progress.tools.len() {
        progress.tools[i].2 = Step::Running;
        cx.update(|cx| report(&progress, cx));
        let (l, id) = (link.clone(), progress.tools[i].0.clone());
        let result = cx.background_executor().spawn(async move { l.shell(setup::Request::Install { tools: vec![id] }) }).await;
        progress.tools[i].2 = match result {
            Ok(setup::Reply::Installed(r)) if r.failed == 0 => Step::Done,
            Ok(setup::Reply::Installed(r)) => Step::Failed(r.lines.join(" ")),
            Ok(other) => Step::Failed(format!("unexpected reply from the host: {other:?}")),
            Err(e) => Step::Failed(format!("{e:#}")),
        };
    }
    if progress.done() > 0 {
        let l = link.clone();
        if let Err(e) = cx.background_executor().spawn(async move { l.reload_shells() }).await {
            log::warn!("installed shell tools, but couldn't reload the shells: {e:#}");
        }
    }
    progress.finished = true;
    cx.update(|cx| report(&progress, cx));
    progress
}

/// Hosts already asked in this run (by label).
#[derive(Default)]
struct Asked(HashSet<String>);

impl Global for Asked {}

/// Remember that `link`'s host was asked; `false` if it already was.
pub fn mark_asked(link: &HostLink, cx: &mut App) -> bool {
    cx.default_global::<Asked>().0.insert(link.label())
}

fn toast_key(link: &HostLink) -> String {
    format!("shell.setup:{}", link.label())
}

/// A tab connected to `link`'s host: if its shell misses goatty's default
/// tools, offer to install them. Once per host per run; never while the
/// welcome window is open (it asks itself), never after "Don't ask again".
/// Hosts whose goattyd is too old for this stay quiet.
pub fn offer(link: HostLink, cx: &mut App) {
    if crate::welcome::is_open(cx) || !mark_asked(&link, cx) {
        return;
    }
    cx.spawn(async move |cx| {
        let l = link.clone();
        let status = match cx.background_executor().spawn(async move { l.shell_status() }).await {
            Ok(s) => s,
            Err(e) => {
                log::info!("not offering the shell setup on {}: {e:#}", link.label());
                return;
            }
        };
        if !status.should_offer() {
            return;
        }
        if !status.offered {
            let l = link.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(e) = l.shell(setup::Request::Offered) {
                        log::warn!("couldn't tell the host the setup was offered: {e:#}");
                    }
                })
                .detach();
        }
        cx.update(|cx| show_offer(link, &status, cx));
    })
    .detach();
}

fn show_offer(link: HostLink, status: &Status, cx: &mut App) {
    let tools: Vec<Tool> = status.offer().cloned().collect();
    let list: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let bytes: u64 = tools.iter().map(|t| t.download).sum();
    let title = if link.is_local() { "Finish setting up your shell".to_string() } else { format!("Set up the shell on {}", link.label()) };
    let verb = if tools.len() == 1 { "isn't" } else { "aren't" };
    let body = format!(
        "{} {verb} installed yet. goatty can add them: {}, no sudo, nothing outside goatty's folder.",
        names(&list),
        setup::size(bytes)
    );
    let (install_link, dismiss_link) = (link.clone(), link.clone());
    let t = Toast::info(title)
        .body(body)
        .key(toast_key(&link))
        .sticky()
        .action("Install", move |_, cx| start_install(install_link.clone(), tools.clone(), cx))
        .action("Not now", |_, _| {})
        .action("Don't ask again", move |_, cx| dismiss(dismiss_link.clone(), cx));
    toast::show(cx, t);
}

/// "Don't ask again": the host remembers (no offer, no shell hint).
pub fn dismiss(link: HostLink, cx: &mut App) {
    cx.spawn(async move |cx| {
        let l = link.clone();
        let result = cx.background_executor().spawn(async move { l.shell(setup::Request::Dismiss) }).await;
        cx.update(|cx| {
            let t = match result {
                Ok(_) => Toast::info("goatty won't ask again")
                    .body("Install the tools any time from Settings → Diagnostics, or with goatty-setup in a shell.")
                    .action("Diagnostics", |_, cx| settings_window::open_at(cx, Some(Page::Diagnostics))),
                Err(e) => Toast::error("Couldn't save that").body(format!("{e:#}")),
            };
            toast::show(cx, t.key(toast_key(&link)));
        });
    })
    .detach();
}

/// Install `tools` with a toast showing the progress and the result.
pub fn start_install(link: HostLink, tools: Vec<Tool>, cx: &mut App) {
    let key = toast_key(&link);
    let progress = Progress::new(&tools);
    let id = toast::show(cx, Toast::loading(progress.status_line()).key(key.clone()));
    cx.spawn(async move |cx| {
        let k = key.clone();
        let retry_link = link.clone();
        let p = install(link, progress, cx, move |p, cx| {
            if !p.finished {
                toast::update(cx, id, Toast::loading(p.status_line()).key(k.clone()));
            }
        })
        .await;
        cx.update(|cx| {
            let failures = p.failures();
            let installed: Vec<&str> = p.tools.iter().filter(|t| t.2 == Step::Done).map(|t| t.1.as_str()).collect();
            let t = if failures.is_empty() {
                Toast::success(format!("Installed {}", names(&installed)))
                    .body("Your shells reload with them at their next idle prompt; new tabs have them right away.")
            } else {
                let retry: Vec<Tool> =
                    tools.iter().filter(|t| p.tools.iter().any(|(id, _, s)| id == &t.id && *s != Step::Done)).cloned().collect();
                Toast::error(if installed.is_empty() {
                    "Couldn't install the tools".to_string()
                } else {
                    format!("Installed {}, but not everything", names(&installed))
                })
                .body(failures.join("\n"))
                .action("Retry", move |_, cx| start_install(retry_link.clone(), retry.clone(), cx))
            };
            toast::update(cx, id, t.key(key).action("Diagnostics", |_, cx| settings_window::open_at(cx, Some(Page::Diagnostics))));
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_names_like_a_sentence() {
        assert_eq!(names(&[]), "");
        assert_eq!(names(&["Starship"]), "Starship");
        assert_eq!(names(&["Starship", "fzf"]), "Starship and fzf");
        assert_eq!(names(&["A", "B", "C"]), "A, B and C");
        assert_eq!(names(&["A", "B", "C", "D"]), "A, B and 2 more");
    }

    #[test]
    fn progress_reports_the_running_tool() {
        let tool = |id: &str| Tool {
            id: id.into(),
            name: id.to_uppercase(),
            summary: String::new(),
            enabled: true,
            default_install: true,
            location: setup::ToolLocation::Missing,
            download: 0,
            version: String::new(),
            unavailable: None,
        };
        let (a, b) = (tool("a"), tool("b"));
        let mut p = Progress::new([&a, &b]);
        p.tools[0].2 = Step::Done;
        p.tools[1].2 = Step::Running;
        assert_eq!(p.status_line(), "Installing B… (2 of 2)");
        p.tools[1].2 = Step::Failed("no network".into());
        assert_eq!((p.done(), p.failures()), (1, vec!["no network".to_string()]));
    }
}
