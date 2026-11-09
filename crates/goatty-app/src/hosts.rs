//! The hosts goatty offers to connect to (#115): the ones added in the
//! settings (`ssh.hosts`) and, with `ssh.discover`, the ones in
//! `~/.ssh/config`.
//!
//! The command palette lists them (Ctrl+Shift+H shows only them, and
//! connects to whatever `user@host` is typed), and so does the settings
//! window's Hosts page. Connecting opens the connect window
//! (`crate::connect`) for a new terminal window: goattyd on the host, with
//! the offer to install it or to use plain SSH when it isn't there, or
//! plain SSH right away for hosts set to it.
//!
//! `~/.ssh/config` is read off the UI thread into the [`HostList`] global,
//! when the app starts and whenever the palette or the Hosts page opens;
//! views that show hosts observe the global.

use std::sync::Arc;

use goatty_client::ssh_config::{self, ConfigHost};
use goatty_client::{Endpoint, SshTarget};
use goatty_config::{ClientSettings, OnLaunch};
use gpui::{App, Global};

use crate::command_palette::Command;
use crate::connect::{self, ConnectPlan};
use crate::session::AttachMode;
use crate::settings::AppSettings;

/// The hosts found in `~/.ssh/config`.
#[derive(Default)]
pub struct HostList {
    pub discovered: Arc<Vec<ConfigHost>>,
    reading: bool,
}

impl Global for HostList {}

/// Re-read `~/.ssh/config` in the background; observers of [`HostList`]
/// hear about it when it changed.
pub fn refresh(cx: &mut App) {
    let list = cx.default_global::<HostList>();
    if list.reading {
        return;
    }
    list.reading = true;
    cx.spawn(async move |cx| {
        let found = cx.background_executor().spawn(async move { ssh_config::user_hosts() }).await;
        cx.update(|cx| {
            if *cx.global::<HostList>().discovered == found {
                cx.global_mut::<HostList>().reading = false;
            } else {
                log::debug!("ssh config: {} hosts", found.len());
                cx.set_global(HostList { discovered: Arc::new(found), reading: false });
            }
        });
    })
    .detach();
}

/// A host to offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    /// What ssh is given.
    pub host: String,
    pub label: String,
    /// `user@hostname` from the ssh configuration.
    pub detail: Option<String>,
    pub plain: bool,
    /// Found in `~/.ssh/config` (else added in the settings).
    pub discovered: bool,
}

/// Every host to offer: the ones added in the settings, then the ones from
/// `~/.ssh/config` (with their settings, when they have some).
pub fn entries(settings: &ClientSettings, discovered: &[ConfigHost]) -> Vec<HostEntry> {
    let ssh = &settings.ssh;
    let discovered = if ssh.discover { discovered } else { &[] };
    let added = ssh.hosts.iter().filter(|h| !discovered.iter().any(|d| d.alias == h.host)).map(|h| HostEntry {
        host: h.host.clone(),
        label: h.label().to_string(),
        detail: None,
        plain: h.plain,
        discovered: false,
    });
    let found = discovered.iter().map(|d| {
        let own = ssh.get(&d.alias);
        HostEntry {
            host: d.alias.clone(),
            label: own.map_or_else(|| d.alias.clone(), |h| h.label().to_string()),
            detail: d.detail(),
            plain: own.is_some_and(|h| h.plain),
            discovered: true,
        }
    });
    added.chain(found).collect()
}

/// [`entries`] for the app's settings and the last read of `~/.ssh/config`.
pub fn current(cx: &App) -> Vec<HostEntry> {
    let discovered = cx.try_global::<HostList>().map(|l| l.discovered.clone()).unwrap_or_default();
    entries(&cx.global::<AppSettings>().client, &discovered)
}

/// Open a window connected to `host` (an alias or `user@host`): through
/// the connect window, which handles a missing or old goattyd, ssh's
/// prompts, and plain SSH.
pub fn connect(cx: &mut App, host: &str) {
    let settings = &cx.global::<AppSettings>().client;
    let plain = settings.ssh.plain(host);
    let mode = if settings.workspace.on_launch == OnLaunch::New { AttachMode::New } else { AttachMode::Auto };
    let mut target = SshTarget::new(host);
    target.askpass = std::env::current_exe().ok();
    log::info!("connecting to {host}{}", if plain { " (plain SSH)" } else { "" });
    connect::open(cx, ConnectPlan { endpoint: Endpoint::Ssh(target), mode, open_settings: None, plain });
}

/// The palette's command for `e`. Its id carries the host, so recently
/// used hosts come first.
pub fn command(e: &HostEntry) -> Command {
    let host = e.host.clone();
    let mut c =
        Command::handler(&format!("ssh.host:{}", e.host), "SSH", &format!("Connect to {}", e.label), move |_, cx| connect(cx, &host));
    let mut words: Vec<&str> = vec!["remote", "server"];
    words.extend([e.host.as_str()].into_iter().filter(|h| *h != e.label));
    words.extend(e.detail.as_deref());
    c = c.keywords(&words);
    let detail = match (&e.detail, e.plain) {
        (Some(d), true) => Some(format!("{d} · plain SSH")),
        (Some(d), false) => Some(d.clone()),
        (None, true) => Some("plain SSH".into()),
        (None, false) => (e.label != e.host).then(|| e.host.clone()),
    };
    c.detail = detail.map(Into::into);
    c.only_when_searched = true;
    c
}

/// The palette's command for a host that was typed rather than listed.
pub fn typed_command(host: &str) -> Command {
    let h = host.to_string();
    let mut c = Command::handler(&format!("ssh.host:{host}"), "SSH", &format!("Connect to {host}"), move |_, cx| connect(cx, &h));
    c.detail = Some("not in your hosts".into());
    c
}

#[cfg(test)]
mod tests {
    use goatty_config::SshHost;

    use super::*;

    #[test]
    fn added_hosts_and_discovered_ones_merge() {
        let mut s = ClientSettings::default();
        s.ssh.hosts = vec![
            SshHost { host: "pi@10.0.0.5".into(), name: Some("Pi".into()), plain: false },
            SshHost { host: "nas".into(), name: None, plain: true },
        ];
        let found = vec![
            ConfigHost { alias: "nas".into(), hostname: Some("192.168.1.2".into()), user: Some("admin".into()) },
            ConfigHost { alias: "box".into(), hostname: None, user: None },
        ];
        let e = entries(&s, &found);
        let summary: Vec<_> = e.iter().map(|e| (e.host.as_str(), e.label.as_str(), e.plain, e.discovered)).collect();
        assert_eq!(summary, [("pi@10.0.0.5", "Pi", false, false), ("nas", "nas", true, true), ("box", "box", false, true)]);
        assert_eq!(e[1].detail.as_deref(), Some("admin@192.168.1.2"));
        // Without discovery, a discovered host's settings make it a host of its own.
        s.ssh.discover = false;
        let hosts: Vec<_> = entries(&s, &found).into_iter().map(|e| (e.host, e.discovered)).collect();
        assert_eq!(hosts, [("pi@10.0.0.5".to_string(), false), ("nas".to_string(), false)]);
    }
}
