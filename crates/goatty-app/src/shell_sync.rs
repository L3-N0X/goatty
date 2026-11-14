//! Bringing this computer's shell setup to the hosts goatty connects to
//! (#159): the same prompt, aliases, plugins and tools everywhere.
//!
//! When a tab connects to another host's goattyd, goatty compares this
//! computer's setup with the host's (`goatty_shell::sync`, off the UI
//! thread). If they differ, it asks with a sticky toast: **Sync**, **Always
//! sync this host** (`ssh.hosts[].sync`, also on the Hosts page) or **Not
//! now**. It asks once per change made here: the answer is kept with the
//! setup's fingerprint (`goatty_config::sync_state`), so editing the setup
//! here asks again, while a host whose setup was changed on purpose isn't
//! nagged. Hosts set to always sync are synced without asking whenever the
//! setup here changed since their last sync, and a host
//! goatty just installed goattyd on gets the setup with it (the install
//! question says so). "Never" leaves a host alone and falls back to the
//! plain offer of missing tools (`shell_setup`).
//!
//! The sync runs on the host through its goattyd: plugins (each plan checked
//! against the one approved here, `sync::check_plan`), the built-in tools,
//! then the settings as one new generation, then a reload of its shells.

use std::collections::HashSet;

use anyhow::Result;
use goatty_config::SyncMode;
use goatty_config::sync_state::{self, SyncState};
use goatty_shell::HostService;
use goatty_shell::setup;
use goatty_shell::sync::{self, LocalSetup, Plan};
use gpui::{App, AsyncApp, Global};

use crate::settings::AppSettings;
use crate::settings_window::{self, HostLink, Page};
use crate::shell_setup;
use crate::toast::{self, Toast};

/// Hosts checked in this run (by [`HostLink::remote_key`]).
#[derive(Default)]
struct Checked(HashSet<String>);

impl Global for Checked {}

fn toast_key(key: &str) -> String {
    format!("shell.sync:{key}")
}

/// What the host needs, and this computer's setup's fingerprint.
struct Check {
    plan: Plan,
    fingerprint: String,
    /// The user answered for this fingerprint already.
    answered: bool,
    /// This fingerprint was synced there already.
    synced: bool,
}

/// Compare this computer's setup with the host's. Blocks: run it off the
/// UI thread.
fn check(link: &HostLink, key: &str) -> Result<Check> {
    let local = LocalSetup::read(&HostService::from_env(None))?;
    let plan = sync::plan(&local, &sync::fetch(link)?);
    let fingerprint = local.fingerprint();
    let state = sync_state::state_file().map(|f| SyncState::load(&f)).unwrap_or_default();
    let (answered, synced) = (state.answered(key, &fingerprint), state.synced(key, &fingerprint));
    Ok(Check { plan, fingerprint, answered, synced })
}

/// Remember the answer for this version of the setup (off the UI thread).
fn record(key: String, fingerprint: String, synced: bool, cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            let Some(file) = sync_state::state_file() else { return };
            let mut state = SyncState::load(&file);
            state.record(&key, &fingerprint, synced);
            if let Err(e) = state.save(&file) {
                log::warn!("couldn't save {}: {e}", file.display());
            }
        })
        .detach();
}

/// A tab connected to `link`'s host. For another host's goattyd: bring
/// the setup there as `ssh.sync`/`ssh.hosts[].sync` say (`after_install`:
/// goatty just installed goattyd there, and the question said the setup
/// comes along). For this computer, or a host that's never synced: offer
/// missing tools only.
pub fn on_connect(link: HostLink, after_install: bool, cx: &mut App) {
    let Some(key) = link.remote_key() else { return shell_setup::offer(link, cx) };
    let mode = cx.global::<AppSettings>().file.value.ssh.sync_mode(&key);
    if mode == SyncMode::Never {
        return shell_setup::offer(link, cx);
    }
    if !cx.default_global::<Checked>().0.insert(key.clone()) && !after_install {
        return;
    }
    cx.spawn(async move |cx| {
        let (l, k) = (link.clone(), key.clone());
        let checked = cx.background_executor().spawn(async move { check(&l, &k) }).await;
        cx.update(|cx| {
            let c = match checked {
                Ok(c) => c,
                Err(e) => {
                    log::info!("not syncing the shell setup to {key}: {e:#}");
                    return shell_setup::offer(link, cx);
                }
            };
            // The sync covers missing tools: no second offer for them.
            shell_setup::mark_asked(&link, cx);
            if c.plan.is_empty() {
                record(key, c.fingerprint, true, cx);
            } else if after_install || (mode == SyncMode::Always && !c.synced) {
                start(link, key, c.plan, c.fingerprint, cx);
            } else if !c.answered {
                tell_offered(&link, &c.plan, cx);
                show_offer(link, key, c.plan, c.fingerprint, cx);
            }
        });
    })
    .detach();
}

/// Showing the tools in an offer quiets the hint in the host's new shells,
/// like `shell_setup`'s offer.
fn tell_offered(link: &HostLink, plan: &Plan, cx: &mut App) {
    if plan.tools.is_empty() {
        return;
    }
    let l = link.clone();
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = l.shell(setup::Request::Offered) {
                log::info!("couldn't tell the host the setup was offered: {e:#}");
            }
        })
        .detach();
}

/// "Left out: mine (installed from a folder on this computer)".
fn skipped_note(plan: &Plan) -> Option<String> {
    match plan.skipped.as_slice() {
        [] => None,
        [one] => Some(format!("Left out: {} ({}).", one.id, one.reason)),
        many => {
            let ids: Vec<&str> = many.iter().map(|s| s.id.as_str()).collect();
            Some(format!("Left out: {} (see the log for why).", shell_setup::names(&ids)))
        }
    }
}

fn show_offer(link: HostLink, key: String, plan: Plan, fingerprint: String, cx: &mut App) {
    for line in plan.lines() {
        log::info!("shell sync to {key}: {line}");
    }
    let mut body = format!("From this computer: {}.", plan.parts().join(", "));
    if let Some(note) = skipped_note(&plan) {
        body = format!("{body}\n{note}");
    }
    let (sync_args, always_args) =
        ((link.clone(), key.clone(), plan.clone(), fingerprint.clone()), (link.clone(), key.clone(), plan, fingerprint.clone()));
    let not_now_key = key.clone();
    let t = Toast::info(format!("Bring your shell setup to {}?", link.label()))
        .body(body)
        .key(toast_key(&key))
        .sticky()
        .action("Sync", move |_, cx| {
            let (l, k, p, f) = sync_args.clone();
            start(l, k, p, f, cx);
        })
        .action("Always sync this host", move |_, cx| {
            let (l, k, p, f) = always_args.clone();
            AppSettings::update(cx, |s| s.ssh.edit(&k, false, |h| h.sync = Some(SyncMode::Always)));
            start(l, k, p, f, cx);
        })
        .action("Not now", move |_, cx| record(not_now_key.clone(), fingerprint.clone(), false, cx));
    toast::show(cx, t);
}

/// The palette's "Sync shell setup to this host": compare and sync the
/// active tab's host right away.
pub fn sync_now(cx: &mut App) {
    let Some(link) = cx.try_global::<settings_window::HostLinkGlobal>().and_then(|g| g.0.clone()) else {
        toast::show(cx, Toast::warning("Can't sync the shell setup").body("This window isn't connected to a goatty host."));
        return;
    };
    sync_link(link, cx);
}

/// Compare `link`'s host with this computer and sync it right away.
fn sync_link(link: HostLink, cx: &mut App) {
    let Some(key) = link.remote_key() else {
        toast::show(
            cx,
            Toast::info("This is the computer the setup comes from")
                .body("Open a tab on another host (Ctrl+Shift+H), then sync from there.")
                .key("shell.sync"),
        );
        return;
    };
    let id = toast::show(cx, Toast::loading(format!("Comparing your shell setup with {}…", link.label())).key(toast_key(&key)));
    cx.spawn(async move |cx| {
        let (l, k) = (link.clone(), key.clone());
        let checked = cx.background_executor().spawn(async move { check(&l, &k) }).await;
        cx.update(|cx| match checked {
            Ok(c) if c.plan.is_empty() => {
                let body = skipped_note(&c.plan).unwrap_or_else(|| "Settings, plugins and tools match this computer's.".into());
                toast::update(cx, id, Toast::success(format!("{} has your shell setup", link.label())).body(body).key(toast_key(&key)));
                record(key, c.fingerprint, true, cx);
            }
            Ok(c) => start(link, key, c.plan, c.fingerprint, cx),
            Err(e) => toast::update(
                cx,
                id,
                Toast::error(format!("Couldn't compare the shell setup with {}", link.label())).body(format!("{e:#}")).key(toast_key(&key)),
            ),
        });
    })
    .detach();
}

/// Run the sync with a toast showing the progress and the result.
fn start(link: HostLink, key: String, plan: Plan, fingerprint: String, cx: &mut App) {
    let steps = plan.steps();
    let label = link.label();
    let tk = toast_key(&key);
    let id = toast::show(cx, Toast::loading(format!("Syncing your shell setup to {label}…")).key(tk.clone()));
    cx.spawn(async move |cx: &mut AsyncApp| {
        let total = steps.len();
        let mut failures = Vec::new();
        for (i, step) in steps.into_iter().enumerate() {
            let status = format!("{}… ({} of {total})", step.label(), i + 1);
            let tk2 = tk.clone();
            cx.update(|cx| toast::update(cx, id, Toast::loading(status).body(format!("Syncing your shell setup to {label}")).key(tk2)));
            let l = link.clone();
            let name = step.label();
            if let Err(e) = cx.background_executor().spawn(async move { step.run(&l) }).await {
                log::warn!("shell sync to {key}: {name}: {e:#}");
                failures.push(format!("{name}: {e:#}"));
            }
        }
        if failures.len() < total {
            let l = link.clone();
            if let Err(e) = cx.background_executor().spawn(async move { l.reload_shells() }).await {
                log::warn!("synced the shell setup to {key}, but couldn't reload its shells: {e:#}");
            }
        }
        cx.update(|cx| {
            // Also after failures: the toast offers Retry, and asking again
            // on every connect wouldn't fix what failed.
            record(key.clone(), fingerprint, true, cx);
            let t = if failures.is_empty() {
                let mut body = format!("{}. Shells there reload at their next idle prompt.", plan.parts().join(", "));
                if let Some(note) = skipped_note(&plan) {
                    body = format!("{body}\n{note}");
                }
                Toast::success(format!("Synced your shell setup to {label}")).body(body)
            } else {
                let retry = link.clone();
                Toast::error(if failures.len() == total {
                    format!("Couldn't sync your shell setup to {label}")
                } else {
                    format!("Synced part of your shell setup to {label}")
                })
                .body(failures.join("\n"))
                .action("Retry", move |_, cx| sync_link(retry.clone(), cx))
                .action("Plugins", |_, cx| settings_window::open_at(cx, Some(Page::Plugins)))
            };
            toast::update(cx, id, t.key(tk));
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use goatty_shell::sync::Skipped;

    use super::*;

    #[test]
    fn skipped_plugins_are_named() {
        let mut p = Plan::default();
        assert_eq!(skipped_note(&p), None);
        p.skipped.push(Skipped { id: "mine".into(), reason: "installed from a folder".into() });
        assert_eq!(skipped_note(&p).unwrap(), "Left out: mine (installed from a folder).");
        p.skipped.push(Skipped { id: "acme".into(), reason: "x".into() });
        assert!(skipped_note(&p).unwrap().starts_with("Left out: mine and acme"));
    }
}
