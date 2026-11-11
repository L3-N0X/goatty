//! The settings window's Hosts page (#115): the hosts the command palette
//! offers to connect to. Hosts added here and the choices for hosts from
//! `~/.ssh/config` are client settings (`ssh` in `settings.json`), saved
//! right away.

use goatty_config::{OpenIn, SshHost, ssh_host_problem};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Subscription, div, prelude::*, px,
};

use super::SettingsWindow;
use super::kit::{self, card, divider, field, row, section};
use crate::hosts::{self, HostEntry, HostList};
use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, Tone, banner, button_with, icon_button, segmented, toggle, tooltip};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, text};
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
        let host = cx.new(|cx| TextInput::new("user@host, or a name from ~/.ssh/config", "", cx).label("Host"));
        let name = cx.new(|cx| TextInput::new("Optional", "", cx).label("Name"));
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
        let gid = SharedString::from(format!("host-row-{}", hash(&e.host)));
        let mut detail: Vec<String> = Vec::new();
        if e.label != e.host {
            detail.push(e.host.clone());
        }
        detail.extend(e.detail.clone());
        if plain {
            detail.push("plain SSH".into());
        }
        let plain_toggle = {
            let host = host.clone();
            div()
                .id(("host-plain-wrap", hash(&e.host)))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().text_size(px(text::SMALL)).text_color(ui.muted).child("Plain SSH"))
                .child(
                    toggle(("host-plain", hash(&e.host)), plain, ui, move |_, cx| {
                        AppSettings::update(cx, |s| s.ssh.edit(&host, !discovered, |h| h.plain = !plain));
                    })
                    .aria_label(SharedString::from(format!("Plain SSH for {}", e.label))),
                )
                .tooltip(tooltip("Run ssh in a local session instead of goattyd on the host: nothing gets installed there"))
        };
        let connect = {
            let host = host.clone();
            button_with(("host-connect", hash(&e.host)), Some(Icon::Terminal), "Connect", ButtonKind::Normal, ui, move |_, cx| {
                hosts::connect(cx, &host)
            })
        };
        div()
            .id(gid.clone())
            .group(gid.clone())
            .flex()
            .items_center()
            .gap(px(14.))
            .min_h(px(60.))
            .px(px(16.))
            .py(px(10.))
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(36.))
                    .rounded(px(10.))
                    .bg(ui.field)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::Server.svg().size(px(17.)).text_color(ui.muted)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(SharedString::from(e.label.clone())),
                    )
                    .when(!detail.is_empty(), |d| {
                        d.child(
                            div().text_size(px(text::SMALL)).text_color(ui.muted).truncate().child(SharedString::from(detail.join(" · "))),
                        )
                    }),
            )
            .child(plain_toggle)
            .child(connect)
            .when(!discovered, |d| {
                d.child(icon_button(("host-remove", hash(&e.host)), Icon::Trash, "Remove from the list", ui, move |_, cx: &mut App| {
                    AppSettings::update(cx, |s| s.ssh.hosts.retain(|h| h.host != host));
                }))
            })
            .into_any_element()
    }

    fn host_list(&self, ui: Ui, entries: &[&HostEntry]) -> gpui::Div {
        let mut c = card(ui);
        for (i, e) in entries.iter().enumerate() {
            if i > 0 {
                c = c.child(divider(ui));
            }
            c = c.child(self.host_row(ui, e));
        }
        c
    }

    pub(super) fn hosts_page(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let settings = &cx.global::<AppSettings>().file.value.ssh;
        let (discover, open_in) = (settings.discover, settings.open_in);
        let entries = hosts::current(cx);
        let (added, found): (Vec<&HostEntry>, Vec<&HostEntry>) = entries.iter().partition(|e| !e.discovered);

        let connecting = section(ui, "Connecting")
            .child(row(ui, "Open hosts in").desc("Shift+Enter in the session list opens it the other way").control(segmented(
                "ssh-open-in",
                &[(OpenIn::Window, "New window"), (OpenIn::Tab, "New tab")],
                &open_in,
                ui,
                |v, _, cx| AppSettings::update(cx, |s| s.ssh.open_in = v),
            )))
            .footer(
                "Ctrl+Shift+H lists your hosts and their sessions. goatty runs its session daemon on a host so shells survive \
                 disconnects; it asks before copying it there (to ~/.local/share/goatty, no sudo).",
            );

        let form = card(ui)
            .p(px(16.))
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(10.))
                    .child(field(ui, "Host", self.hosts.host.clone()).flex_1())
                    .child(field(ui, "Name to show", self.hosts.name.clone()).w(px(180.)))
                    .child(
                        button_with("host-add", Some(Icon::Plus), "Add", ButtonKind::Primary, ui, self.act(cx, |v, cx| v.add_host(cx)))
                            .h(px(36.)),
                    ),
            )
            .children(self.hosts.error.clone().map(|e| banner(ui, Tone::Danger, e, Vec::new())));
        let mut yours = section(ui, "Your hosts").plain().child(form);
        if !added.is_empty() {
            yours = yours.child(self.host_list(ui, &added));
        }

        let mut config = section(ui, "From ~/.ssh/config").child(
            row(ui, "List these hosts too").desc("Every Host entry without wildcards, with the files it includes").control(toggle(
                "ssh-discover",
                discover,
                ui,
                |_, cx| AppSettings::update(cx, |s| s.ssh.discover = !s.ssh.discover),
            )),
        );
        if discover {
            config = if found.is_empty() {
                config.child(kit::empty(ui, Icon::Server, "No hosts found", "~/.ssh/config has no Host entries without wildcards."))
            } else {
                config.children(found.iter().map(|e| self.host_row(ui, e)))
            };
        }
        div().flex().flex_col().gap(px(28.)).child(connecting).child(yours).child(config).into_any_element()
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
