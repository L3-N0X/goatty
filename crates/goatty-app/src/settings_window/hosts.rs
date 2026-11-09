//! The settings window's Hosts page (#115): the hosts the command palette
//! offers to connect to. Hosts added here and the choices for hosts from
//! `~/.ssh/config` are client settings (`ssh` in `settings.json`), saved
//! right away.

use goatty_config::{SshHost, ssh_host_problem};
use gpui::{AnyElement, App, Context, Entity, IntoElement, ParentElement, Styled, Subscription, div, prelude::*};

use super::SettingsWindow;
use crate::hosts::{self, HostEntry, HostList};
use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, button, note, notice, row, section, toggle};
use crate::ui::style::Ui;
use crate::ui::text_input::{TextInput, TextInputEvent};

pub struct HostsPage {
    host: Entity<TextInput>,
    name: Entity<TextInput>,
    /// Why the last Add didn't go through.
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl HostsPage {
    pub fn new(cx: &mut Context<'_, SettingsWindow>) -> Self {
        let host = cx.new(|cx| TextInput::new("user@host, or an alias ssh knows", "", cx));
        let name = cx.new(|cx| TextInput::new("Name to show (optional)", "", cx));
        let mut subs = vec![cx.observe_global::<HostList>(|_, cx| cx.notify())];
        for input in [&host, &name] {
            subs.push(cx.subscribe(input, |view, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Changed(_) => {
                    if view.hosts.error.take().is_some() {
                        cx.notify();
                    }
                }
                TextInputEvent::Submitted(_) => view.add_host(cx),
            }));
        }
        Self { host, name, error: None, _subs: subs }
    }
}

/// Check a host before adding it: `Err` says why not.
pub fn check_new_host(existing: &[HostEntry], host: &str) -> Result<(), String> {
    if let Some(why) = ssh_host_problem(host) {
        return Err(format!("Host: {why}."));
    }
    if existing.iter().any(|e| e.host == host) {
        return Err(format!("{host} is in the list already."));
    }
    Ok(())
}

impl SettingsWindow {
    fn add_host(&mut self, cx: &mut Context<'_, Self>) {
        let text = |e: &Entity<TextInput>, cx: &Context<'_, Self>| e.read(cx).text().trim().to_owned();
        let (host, name) = (text(&self.hosts.host, cx), text(&self.hosts.name, cx));
        if let Err(e) = check_new_host(&hosts::current(cx), &host) {
            self.hosts.error = Some(e);
            cx.notify();
            return;
        }
        let name = (!name.is_empty() && name != host).then_some(name);
        AppSettings::update(cx, |s| s.ssh.hosts.push(SshHost { host, name, plain: false }));
        for input in [&self.hosts.host, &self.hosts.name] {
            input.update(cx, |i, cx| i.set_text("", cx));
        }
        self.hosts.error = None;
        cx.notify();
    }

    fn host_row(&self, ui: Ui, e: &HostEntry) -> AnyElement {
        let (host, discovered, plain) = (e.host.clone(), e.discovered, e.plain);
        let mut description: Vec<String> = Vec::new();
        if e.label != e.host {
            description.push(e.host.clone());
        }
        description.extend(e.detail.clone());
        let plain_toggle = {
            let host = host.clone();
            toggle(("host-plain", hash(&e.host)), plain, ui, move |_, cx| {
                AppSettings::update(cx, |s| s.ssh.edit(&host, !discovered, |h| h.plain = !plain));
            })
        };
        let connect = {
            let host = host.clone();
            button(("host-connect", hash(&e.host)), "Connect", ButtonKind::Normal, ui, move |_, cx| hosts::connect(cx, &host))
        };
        let controls = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_xs().text_color(ui.muted).child("Plain SSH"))
            .child(plain_toggle)
            .child(connect)
            .when(!discovered, |d| {
                d.child(button(("host-remove", hash(&e.host)), "Remove", ButtonKind::Danger, ui, move |_, cx: &mut App| {
                    AppSettings::update(cx, |s| s.ssh.hosts.retain(|h| h.host != host));
                }))
            });
        let description = (!description.is_empty()).then(|| description.join(" · "));
        row(ui, &e.label, description.as_deref(), controls).into_any_element()
    }

    pub(super) fn hosts_page(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let settings = &cx.global::<AppSettings>().file.value.ssh;
        let discover = settings.discover;
        let entries = hosts::current(cx);
        let (added, found): (Vec<_>, Vec<_>) = entries.iter().partition(|e| !e.discovered);
        let mut page = div().flex().flex_col().child(note(
            ui,
            "The hosts the command palette offers: Ctrl+Shift+H lists them (and connects to any user@host you type), or type a \
             host's name into Ctrl+Shift+P. Each host opens in a window of its own.",
        ));
        page = page.child(note(
            ui,
            "goatty runs its session daemon, goattyd, on the host, so shells survive disconnects and get goatty's shell setup. \
             When it isn't installed there, goatty asks before copying it over (to ~/.local/share/goatty, no sudo). Plain SSH \
             runs ssh in a local session instead and installs nothing.",
        ));

        page = page.child(section(ui, "Add a host")).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_2()
                .child(div().flex_1().child(self.hosts.host.clone()))
                .child(div().w(gpui::px(200.)).child(self.hosts.name.clone()))
                .child(button("host-add", "Add", ButtonKind::Primary, ui, self.act(cx, |v, cx| v.add_host(cx)))),
        );
        if let Some(e) = &self.hosts.error {
            page = page.child(notice(ui, ui.danger, vec![e.clone()]));
        }

        page = page.child(section(ui, "Your hosts"));
        page = if added.is_empty() {
            page.child(note(ui, "None added yet."))
        } else {
            page.children(added.iter().map(|e| self.host_row(ui, e)))
        };

        page = page.child(section(ui, "From ~/.ssh/config")).child(row(
            ui,
            "List the hosts in ~/.ssh/config",
            Some("Every Host entry without wildcards, including the files it includes"),
            toggle("ssh-discover", discover, ui, |_, cx| AppSettings::update(cx, |s| s.ssh.discover = !s.ssh.discover)),
        ));
        if discover {
            page = if found.is_empty() {
                page.child(note(ui, "No hosts found there."))
            } else {
                page.children(found.iter().map(|e| self.host_row(ui, e)))
            };
        }
        page.into_any_element()
    }
}

/// A stable element id for a host.
fn hash(s: &str) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_hosts_are_checked() {
        let existing = [HostEntry { host: "box".into(), label: "box".into(), detail: None, plain: false, discovered: true }];
        assert!(check_new_host(&existing, "pi@10.0.0.5").is_ok());
        assert!(check_new_host(&existing, "box").unwrap_err().contains("already"));
        assert!(check_new_host(&existing, "").is_err());
        assert!(check_new_host(&existing, "a b").is_err());
    }
}
