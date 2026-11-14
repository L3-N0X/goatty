//! The hosts goatty offers to connect to (#115, #4): this computer, the
//! ones added in the settings (`ssh.hosts`) and, with `ssh.discover`, the
//! ones in `~/.ssh/config`.
//!
//! The command palette lists them. Ctrl+Shift+H is the host switcher: the
//! hosts with user@host, OS and round-trip time for the ones goatty is
//! connected to, and for each host its sessions to attach to or a new one
//! (`command_palette::Scope::Sessions`). The settings window's Hosts page
//! lists them too.
//!
//! [`open`] puts a connection in a new tab of the last active terminal
//! window or in a window of its own (`ssh.open_in`). Either way the tab
//! shows the connect screen (`crate::connect`) until the session is ready:
//! goattyd on the host, with the offer to install it or to use plain SSH
//! when it isn't there, or plain SSH right away for hosts set to it.
//!
//! Tabs on the same host share one [`Connection`] ([`pool`]): a second tab
//! on a host doesn't log in again, and the switcher can show that host's
//! link while any tab uses it.
//!
//! `~/.ssh/config` is read off the UI thread into the [`HostList`] global,
//! when the app starts and whenever the palette or the Hosts page opens;
//! views that show hosts observe the global.

use std::sync::Arc;

use goatty_client::ssh_config::{self, ConfigHost};
use goatty_client::{Connection, Endpoint, SshTarget};
use goatty_config::{ClientSettings, OnLaunch, OpenIn};
use gpui::{AnyWindowHandle, App, Global};

use crate::command_palette::Command;
use crate::connect::ConnectPlan;
use crate::session::AttachMode;
use crate::settings::AppSettings;

/// The goattyd connections in use, so tabs on the same host share one.
/// Weak: a connection closes with the last tab (or picker) holding it.
pub mod pool {
    use std::sync::{Arc, Weak};

    use anyhow::Result;
    use goatty_client::{Connection, Endpoint};
    use parking_lot::Mutex;

    static LIVE: Mutex<Vec<(String, Weak<Connection>)>> = Mutex::new(Vec::new());

    /// Which connections can be shared: the local daemon's and SSH hosts'
    /// (`--via` commands are one-offs).
    fn key(endpoint: &Endpoint) -> Option<String> {
        match endpoint {
            Endpoint::Local => Some("local".into()),
            Endpoint::Ssh(t) if t.remote_daemon.is_none() => Some(format!("ssh:{}", t.host)),
            _ => None,
        }
    }

    /// The open connection to `endpoint`, if a tab uses one.
    pub fn live(endpoint: &Endpoint) -> Option<Arc<Connection>> {
        let key = key(endpoint)?;
        let mut live = LIVE.lock();
        live.retain(|(_, w)| w.strong_count() > 0);
        live.iter().find(|(k, _)| *k == key).and_then(|(_, w)| w.upgrade())
    }

    /// The open connection to `endpoint`, else a new one. Blocks (ssh can
    /// take seconds): call it off the UI thread.
    pub fn open(endpoint: Endpoint) -> Result<Arc<Connection>> {
        if let Some(conn) = live(&endpoint) {
            return Ok(conn);
        }
        let conn = Connection::open(endpoint, crate::CLIENT)?;
        add(&conn);
        Ok(conn)
    }

    /// Share `conn` from now on (opened elsewhere, e.g. after installing goattyd).
    pub fn add(conn: &Arc<Connection>) {
        let Some(key) = key(conn.endpoint()) else { return };
        let mut live = LIVE.lock();
        live.retain(|(k, w)| w.strong_count() > 0 && *k != key);
        live.push((key, Arc::downgrade(conn)));
    }
}

/// A host to open sessions on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRef {
    /// This computer's goattyd.
    Local,
    /// What ssh is given: an alias or `user@host`.
    Ssh(String),
}

impl HostRef {
    /// How to reach goattyd there (ssh's prompts come up in goatty's dialogs).
    pub fn endpoint(&self) -> Endpoint {
        match self {
            HostRef::Local => Endpoint::Local,
            HostRef::Ssh(host) => {
                let mut target = SshTarget::new(host);
                target.askpass = std::env::current_exe().ok();
                Endpoint::Ssh(target)
            }
        }
    }

    pub fn label(&self) -> &str {
        match self {
            HostRef::Local => "this computer",
            HostRef::Ssh(host) => host,
        }
    }

    /// The host behind a palette command's id ([`command`], [`local_command`],
    /// [`typed_command`]).
    pub fn from_command_id(id: &str) -> Option<Self> {
        if id == LOCAL_ID {
            return Some(HostRef::Local);
        }
        id.strip_prefix("ssh.host:").map(|h| HostRef::Ssh(h.to_string()))
    }
}

const LOCAL_ID: &str = "host.local";

/// The terminal window that was active last: where [`OpenIn::Tab`] opens.
pub struct LastWorkspace(pub AnyWindowHandle);

impl Global for LastWorkspace {}

/// Something to open: a session on a host, in a tab or a window.
pub struct OpenRequest {
    pub host: HostRef,
    pub attach: AttachMode,
    /// A connection to the host made already (the session picker's).
    pub conn: Option<Arc<Connection>>,
    pub place: OpenIn,
}

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

/// The attach mode for a connection without an explicit session: the
/// last window's layout, or a new session (`workspace.on_launch`).
pub fn default_attach(cx: &App) -> AttachMode {
    if cx.global::<AppSettings>().client.workspace.on_launch == OnLaunch::New { AttachMode::New } else { AttachMode::Auto }
}

/// Connect to `host` (an alias or `user@host`) the way the settings say:
/// in a new tab or window (`ssh.open_in`), with plain SSH for hosts set to
/// it.
pub fn connect(cx: &mut App, host: &str) {
    let place = cx.global::<AppSettings>().client.ssh.open_in;
    open(cx, OpenRequest { host: HostRef::Ssh(host.to_string()), attach: default_attach(cx), conn: None, place });
}

/// Open `req`: a tab of the last active terminal window that shows the
/// connect screen until the session is there, or a new window doing the
/// same. Deferred, so it can be called while a workspace is being updated
/// (the palette runs its commands from one).
pub fn open(cx: &mut App, req: OpenRequest) {
    let plain = match &req.host {
        HostRef::Ssh(host) => cx.global::<AppSettings>().client.ssh.plain(host),
        HostRef::Local => false,
    };
    log::info!("opening a session on {}{}", req.host.label(), if plain { " (plain SSH)" } else { "" });
    let plan = ConnectPlan { endpoint: req.host.endpoint(), mode: req.attach, open_settings: None, plain, conn: req.conn };
    let window = cx.try_global::<LastWorkspace>().map(|w| w.0).filter(|_| req.place == OpenIn::Tab);
    cx.defer(move |cx| {
        if let Some(handle) = window {
            let opened = handle.update(cx, |root, window, cx| {
                let Ok(ws) = root.downcast::<crate::workspace::Workspace>() else { return false };
                window.activate_window();
                ws.update(cx, |ws, cx| ws.open_connect_tab(plan.clone(), window, cx));
                true
            });
            if matches!(opened, Ok(true)) {
                return;
            }
        }
        crate::app::open_terminal_window(cx, crate::app::Initial::Connect(plan), None);
    });
}

/// What the switcher shows for a host goatty is connected to:
/// `user@hostname · OS · 23 ms`.
pub fn live_detail(host: &HostRef) -> Option<String> {
    let conn = pool::live(&host.endpoint())?;
    let info = conn.host();
    let mut parts = vec![format!("{}@{}", info.user, info.hostname)];
    if !info.os.is_empty() {
        parts.push(info.os.clone());
    }
    if !conn.is_connected() {
        parts.push("reconnecting…".into());
    } else if let Some(rtt) = conn.rtt() {
        parts.push(format_rtt(rtt));
    }
    Some(parts.join(" · "))
}

/// `23 ms`, `0.4 ms`, `1.2 s`.
pub fn format_rtt(rtt: std::time::Duration) -> String {
    let ms = rtt.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        format!("{ms:.1} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// The switcher's entry for this computer.
pub fn local_command() -> Command {
    let mut c = Command::handler(LOCAL_ID, "Local", "This computer", |_, cx| {
        let place = cx.global::<AppSettings>().client.ssh.open_in;
        open(cx, OpenRequest { host: HostRef::Local, attach: default_attach(cx), conn: None, place });
    });
    c = c.keywords(&["local", "localhost", "here"]);
    c.detail = Some(live_detail(&HostRef::Local).unwrap_or_else(|| "goattyd on this computer".into()).into());
    c
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
    let live = if e.plain { None } else { live_detail(&HostRef::Ssh(e.host.clone())) };
    let detail = match (&e.detail, e.plain) {
        _ if live.is_some() => live,
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
    fn round_trip_times() {
        use std::time::Duration;
        assert_eq!(format_rtt(Duration::from_micros(420)), "0.4 ms");
        assert_eq!(format_rtt(Duration::from_micros(23_400)), "23 ms");
        assert_eq!(format_rtt(Duration::from_millis(1234)), "1.2 s");
    }

    #[test]
    fn hosts_behind_command_ids() {
        assert_eq!(HostRef::from_command_id(LOCAL_ID), Some(HostRef::Local));
        assert_eq!(HostRef::from_command_id("ssh.host:pi@10.0.0.5"), Some(HostRef::Ssh("pi@10.0.0.5".into())));
        assert_eq!(HostRef::from_command_id("tabs.new"), None);
        assert_eq!(HostRef::from_command_id(&command(&entry("box")).id), Some(HostRef::Ssh("box".into())));
        assert_eq!(HostRef::from_command_id(&typed_command("u@h").id), Some(HostRef::Ssh("u@h".into())));
    }

    fn entry(host: &str) -> HostEntry {
        HostEntry { host: host.into(), label: host.into(), detail: None, plain: false, discovered: true }
    }

    #[test]
    fn added_hosts_and_discovered_ones_merge() {
        let mut s = ClientSettings::default();
        s.ssh.hosts = vec![
            SshHost { host: "pi@10.0.0.5".into(), name: Some("Pi".into()), ..Default::default() },
            SshHost { host: "nas".into(), name: None, plain: true, ..Default::default() },
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
