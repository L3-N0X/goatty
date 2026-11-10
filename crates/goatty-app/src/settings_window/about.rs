//! The About page: version, what this window is connected to, links, and
//! a "copy for a bug report" summary.

use std::time::Instant;

use gpui::{AnyElement, ClipboardItem, Context, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, px};

use super::kit::{card, row, section};
use super::{HostLink, SettingsWindow};
use crate::ui::controls::{ButtonKind, button_with};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, text};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");

impl SettingsWindow {
    /// (label, value) facts about this goatty and its host.
    fn about_facts(&self) -> Vec<(&'static str, String)> {
        let mut facts = vec![
            ("goatty", VERSION.to_string()),
            ("Platform", format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
            ("Build", if cfg!(debug_assertions) { "debug".into() } else { "release".into() }),
        ];
        match &self.link {
            Some(HostLink::Daemon(c)) => {
                let h = c.host();
                facts.push(("Connected to", format!("goattyd {} on {}@{} ({})", h.daemon_version, h.user, h.hostname, h.os)));
                facts.push(("Protocol", format!("version {}", c.protocol_version())));
            }
            Some(HostLink::Local(_)) => facts.push(("Connected to", "this process (--local, no goattyd)".into())),
            None => facts.push(("Connected to", "nothing yet".into())),
        }
        facts
    }

    pub(super) fn about(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let facts = self.about_facts();
        let report = facts.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
        let copied = self.copied.as_ref().is_some_and(|(p, at)| p.as_ref() == "about" && at.elapsed().as_millis() < 1500);
        let hero = card(ui)
            .items_center()
            .gap(px(10.))
            .py(px(32.))
            .px(px(24.))
            .child(
                div()
                    .size(px(72.))
                    .rounded(px(22.))
                    .bg(ui.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .shadow_md()
                    .child(Icon::Terminal.svg().size(px(38.)).text_color(ui.on_accent)),
            )
            .child(div().pt(px(6.)).text_size(px(26.)).font_weight(FontWeight::BOLD).child("goatty"))
            .child(div().text_size(px(text::BODY)).text_color(ui.muted).child("A fast terminal with batteries included"))
            .child(
                div()
                    .mt(px(4.))
                    .h(px(26.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(ui.accent_soft)
                    .text_size(px(text::SMALL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.accent_text)
                    .child(SharedString::from(format!("Version {VERSION}"))),
            );
        let details = section(ui, "Details")
            .action(button_with(
                "about-copy",
                Some(if copied { Icon::Check } else { Icon::Copy }),
                if copied { "Copied" } else { "Copy for a bug report" },
                ButtonKind::Ghost,
                ui,
                self.act(cx, move |v, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(report.clone()));
                    v.copied = Some(("about".into(), Instant::now()));
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                        let _ = this.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                    cx.notify();
                }),
            ))
            .children(facts.into_iter().map(|(k, v)| {
                row(ui, k)
                    .control(div().max_w(px(380.)).truncate().text_size(px(text::LABEL)).text_color(ui.muted).child(SharedString::from(v)))
            }));
        let link = |id: &'static str, icon: Icon, label: &'static str, desc: &'static str, url: String| {
            row(ui, label).icon(icon).desc(desc).on_click(id, move |_, _, cx| cx.open_url(&url))
        };
        let links = section(ui, "Links")
            .child(link("about-source", Icon::Code, "Source code", "goatty on GitHub", REPO.to_string()))
            .child(link(
                "about-docs",
                Icon::BookOpen,
                "Documentation",
                "How goatty works, settings, plugins",
                format!("{REPO}/tree/main/docs"),
            ))
            .child(link(
                "about-issue",
                Icon::Bug,
                "Report a problem",
                "Copy the details above into the report",
                format!("{REPO}/issues/new"),
            ));
        div()
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(hero)
            .child(details)
            .child(links)
            .child(
                div()
                    .text_center()
                    .text_size(px(text::SMALL))
                    .text_color(ui.faint)
                    .child("MIT or Apache-2.0 · built with GPUI, rio's VT parser and zsh"),
            )
            .into_any_element()
    }
}
