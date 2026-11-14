//! The Terminal page (links, selection, pasting) and the Workspace page
//! (startup, tabs, panes).

use goatty_config::{
    CopyOnSelect, LinkClick, MiddleClick, OnLaunch, PaneSettings, PasteConfirm, TabActivity, TabBarShow, TabSettings, TabStyle,
};
use gpui::{AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px};

use super::appearance::opts;
use super::kit::{choice_card, row, section};
use super::{Page, SettingsWindow, client};
use crate::settings::AppSettings;
use crate::ui::controls::{segmented, slider, toggle};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, text};

/// Linux and FreeBSD have a primary selection (select to copy, middle
/// click to paste); elsewhere those options collapse.
const PRIMARY_SELECTION: bool = cfg!(any(target_os = "linux", target_os = "freebsd"));

impl SettingsWindow {
    pub(super) fn terminal(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let s = &cx.global::<AppSettings>().file.value;
        let ctrl = if cfg!(target_os = "macos") { "Cmd+click" } else { "Ctrl+click" };
        let links = section(ui, "Links")
            .child(row(ui, "Open links").desc("Web addresses, file paths and links programs mark up").control(self.select(
                "links-open-on",
                "Open links",
                opts(&[
                    (LinkClick::CtrlClick, ctrl, "Hold the key to see where a link goes"),
                    (LinkClick::Click, "Click", "Any click on a link opens it"),
                    (LinkClick::Off, "Never", "Links are plain text"),
                ]),
                &s.links.open_on,
                cx,
                |v, _, cx| AppSettings::update(cx, |s| s.links.open_on = v),
            )))
            .child(
                row(ui, "Open files with")
                    .desc("A command, or empty for the desktop's default app. Enter saves; {path}, {line} and {column} are filled in")
                    .below(self.file_opener.clone()),
            )
            .footer("Paths count when the file exists. On a remote host, clicking one copies it instead.");

        let copy_opts = if PRIMARY_SELECTION {
            opts(&[
                (CopyOnSelect::Primary, "Primary selection", "Middle click pastes it, in any app"),
                (CopyOnSelect::Clipboard, "Primary and clipboard", "Ctrl+V pastes it too"),
                (CopyOnSelect::Off, "Off", "Copy with Ctrl+Shift+C"),
            ])
        } else {
            // Without a primary selection, `primary` copies nowhere.
            opts(&[(CopyOnSelect::Clipboard, "Clipboard", ""), (CopyOnSelect::Primary, "Off", "")])
        };
        let selection = section(ui, "Selection").child(row(ui, "Copy selected text").desc("When you select with the mouse").control(
            self.select("copy-on-select", "Copy selected text", copy_opts, &s.selection.copy_on_select, cx, |v, _, cx| {
                AppSettings::update(cx, |s| s.selection.copy_on_select = v)
            }),
        ));

        let middle_opts = if PRIMARY_SELECTION {
            opts(&[
                (MiddleClick::Primary, "Primary selection", "What you last selected anywhere"),
                (MiddleClick::Clipboard, "Clipboard", "What you last copied"),
                (MiddleClick::Off, "Nothing", ""),
            ])
        } else {
            // Without a primary selection, `primary` pastes the clipboard.
            opts(&[(MiddleClick::Primary, "Clipboard", ""), (MiddleClick::Off, "Nothing", "")])
        };
        let pasting = section(ui, "Pasting")
            .child(row(ui, "Middle click pastes").desc("Hold Shift in programs that use the mouse").control(self.select(
                "middle-click",
                "Middle click pastes",
                middle_opts,
                &s.paste.middle_click,
                cx,
                |v, _, cx| AppSettings::update(cx, |s| s.paste.middle_click = v),
            )))
            .child(row(ui, "Ask before pasting").desc("Shows what's in it, cleans it up or makes it one line").control(self.select(
                "paste-confirm",
                "Ask before pasting",
                opts(&[
                    (PasteConfirm::Suspicious, "When it looks risky", "Would run at once, hidden characters, curl | sh, over 256 KiB"),
                    (PasteConfirm::Multiline, "Risky or several lines", "Also any paste with line breaks"),
                    (PasteConfirm::Never, "Never", "Paste everything as is"),
                ]),
                &s.paste.confirm,
                cx,
                |v, _, cx| AppSettings::update(cx, |s| s.paste.confirm = v),
            )));
        div().flex().flex_col().gap(px(28.)).child(links).child(selection).child(pasting).into_any_element()
    }

    fn tab_style_cards(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        use crate::ui::style::{h, mix};
        let s = &cx.global::<AppSettings>().file.value;
        let t = cx.global::<AppSettings>().colors;
        let (bg, fg, bar) = (h(t.background), h(t.foreground), h(mix(t.background, t.foreground, 0.08)));
        let mono = super::kit::mono(cx);
        // The terminal under the tab bar: one prompt line.
        let screen = || {
            div()
                .flex_1()
                .bg(bg)
                .px(px(10.))
                .pt(px(6.))
                .flex()
                .gap(px(5.))
                .font_family(mono.clone())
                .text_size(px(10.))
                .child(div().text_color(h(t.ansi[4])).child("~"))
                .child(div().text_color(h(t.ansi[2])).child("❯"))
                .child(div().text_color(fg).child("nvim"))
        };
        let modern_bar = div().h(px(30.)).bg(bar).flex().items_end().px(px(8.)).gap(px(2.)).children([("zsh", true), ("nvim", false)].map(
            |(name, on)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .h(px(24.))
                    .px(px(9.))
                    .rounded_t(px(7.))
                    .when(on, |d| d.bg(bg))
                    .text_size(px(11.))
                    .text_color(if on { fg } else { fg.opacity(0.6) })
                    .child(Icon::Terminal.svg().size(px(11.)).text_color(if on { ui.accent } else { fg.opacity(0.5) }))
                    .child(name)
                    .child(Icon::Close.svg().size(px(9.)).text_color(fg.opacity(0.5)))
            },
        ));
        let compact_bar = div().h(px(18.)).bg(bar).flex().items_center().gap(px(10.)).px(px(10.)).text_size(px(10.)).children(
            [("1 zsh", true), ("2 nvim", false)].map(|(name, on)| {
                div().text_color(if on { fg } else { fg.opacity(0.55) }).when(on, |d| d.font_weight(FontWeight::BOLD)).child(name)
            }),
        );
        let modern = div().h(px(60.)).flex().flex_col().child(modern_bar).child(screen());
        let compact = div().h(px(60.)).flex().flex_col().child(compact_bar).child(screen());
        // A sidebar of rows, the active one raised, beside a rounded card.
        let side_row = |name: &'static str, on: bool| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(15.))
                .px(px(4.))
                .rounded(px(4.))
                .when(on, |d| d.bg(bg))
                .text_size(px(9.))
                .text_color(if on { fg } else { fg.opacity(0.6) })
                .child(Icon::Terminal.svg().size(px(8.)).text_color(if on { ui.accent } else { fg.opacity(0.5) }))
                .child(name)
        };
        let vertical = div()
            .h(px(60.))
            .flex()
            .bg(bar)
            .child(
                div()
                    .w(px(56.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(4.))
                    .child(side_row("zsh", true))
                    .child(side_row("nvim", false)),
            )
            .child(div().flex_1().flex().py(px(4.)).pr(px(4.)).child(screen().rounded(px(5.)).pt(px(5.))));
        div()
            .flex()
            .gap(px(10.))
            .child(
                choice_card(ui, "tabs-modern", "Modern", s.tabs.style == TabStyle::Modern, modern)
                    .on_click(|_, _, cx| AppSettings::update(cx, |s| s.tabs.style = TabStyle::Modern)),
            )
            .child(
                choice_card(ui, "tabs-compact", "Compact", s.tabs.style == TabStyle::Compact, compact)
                    .on_click(|_, _, cx| AppSettings::update(cx, |s| s.tabs.style = TabStyle::Compact)),
            )
            .child(
                choice_card(ui, "tabs-vertical", "Vertical", s.tabs.style == TabStyle::Vertical, vertical)
                    .on_click(|_, _, cx| AppSettings::update(cx, |s| s.tabs.style = TabStyle::Vertical)),
            )
    }

    pub(super) fn workspace(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let s = &cx.global::<AppSettings>().file.value;
        let dim = s.panes.dim_inactive;
        let vertical = s.tabs.style == TabStyle::Vertical;
        let (width, min_w, max_w) = (s.tabs.sidebar_width, TabSettings::MIN_SIDEBAR_WIDTH, TabSettings::MAX_SIDEBAR_WIDTH);
        let startup = section(ui, "Startup")
            .child(row(ui, "When goatty starts").desc("Sessions keep running when you close a window").control(self.select(
                "on-launch",
                "When goatty starts",
                opts(&[
                    (OnLaunch::Reattach, "Restore the last window", "Its tabs and splits, with their sessions"),
                    (OnLaunch::New, "Start a new session", "An empty window with a fresh shell"),
                ]),
                &s.workspace.on_launch,
                cx,
                |v, _, cx| AppSettings::update(cx, |s| s.workspace.on_launch = v),
            )))
            .footer("The --new and --attach command-line flags still win.");
        let tabs = section(ui, "Tabs")
            .child(row(ui, "Style").below(self.tab_style_cards(ui, cx)))
            .child(row(ui, "Show the tab bar").control(segmented(
                "tabs-show",
                &[(TabBarShow::Always, "Always"), (TabBarShow::Multiple, "With 2+ tabs")],
                &s.tabs.show,
                ui,
                |v, _, cx| AppSettings::update(cx, |s| s.tabs.show = v),
            )))
            .child(
                row(ui, "Sidebar width")
                    .desc(if vertical { "Or drag the sidebar's edge" } else { "Only with vertical tabs" })
                    .disabled(!vertical)
                    .control(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(slider("tabs-sidebar-width", (width - min_w) / (max_w - min_w), ui, move |t, done, _, cx| {
                                let v = (min_w + t * (max_w - min_w)).round().clamp(min_w, max_w);
                                if done {
                                    AppSettings::update(cx, |s| s.tabs.sidebar_width = v);
                                } else if cx.global::<AppSettings>().file.value.tabs.sidebar_width != v {
                                    AppSettings::preview(cx, |s| s.tabs.sidebar_width = v);
                                }
                            }))
                            .child(
                                div()
                                    .w(px(48.))
                                    .text_size(px(text::LABEL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(SharedString::from(format!("{width:.0} px"))),
                            ),
                    ),
            )
            .child(
                row(ui, "Fold the sidebar to icons")
                    .desc(if vertical { "Ctrl+Shift+B folds and unfolds it" } else { "Only with vertical tabs" })
                    .disabled(!vertical)
                    .control(toggle(
                        "tabs-sidebar-collapsed",
                        s.tabs.sidebar_collapsed,
                        ui,
                        client(|s| s.tabs.sidebar_collapsed = !s.tabs.sidebar_collapsed),
                    )),
            )
            .child(row(ui, "Switch tabs with the mouse wheel").desc("Scroll over the tabs to go to the next or previous one").control(
                toggle(
                    "tabs-scroll-to-switch",
                    s.tabs.scroll_to_switch,
                    ui,
                    client(|s| s.tabs.scroll_to_switch = !s.tabs.scroll_to_switch),
                ),
            ))
            .child(row(ui, "Program icons").desc("nvim, git, docker, python… in each tab").control(toggle(
                "tabs-icons",
                s.tabs.icons,
                ui,
                client(|s| s.tabs.icons = !s.tabs.icons),
            )))
            .child(row(ui, "Mark tabs that ring the bell").desc("A bell on a background tab whose program rang it").control(toggle(
                "tabs-mark-bell",
                s.tabs.mark_bell,
                ui,
                client(|s| s.tabs.mark_bell = !s.tabs.mark_bell),
            )))
            .child(row(ui, "Mark tabs with activity").desc("A dot on a background tab until you look at it").control(self.select(
                "tabs-mark-activity",
                "Mark tabs with activity",
                opts(&[
                    (TabActivity::Finished, "When a command finishes", "Red when it failed. Needs shell integration"),
                    (TabActivity::Output, "On any output", "Also finished commands; progress bars and clocks count too"),
                    (TabActivity::Off, "Never", "Only the bell, if that's on"),
                ]),
                &s.tabs.mark_activity,
                cx,
                |v, _, cx| AppSettings::update(cx, |s| s.tabs.mark_activity = v),
            )))
            .footer("Double-click a tab to rename it, middle-click to close it, right-click for more, drag to move it.");
        let max = PaneSettings::MAX_DIM;
        let panes = section(ui, "Panes")
            .child(
                row(ui, "Fade inactive panes").desc("How much the panes you're not typing in fade").control(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(slider("panes-dim", dim / max, ui, move |t, done, _, cx| {
                            let v = ((t * max * 20.0).round() / 20.0).clamp(0.0, max);
                            if done {
                                AppSettings::update(cx, |s| s.panes.dim_inactive = v);
                            } else if cx.global::<AppSettings>().file.value.panes.dim_inactive != v {
                                AppSettings::preview(cx, |s| s.panes.dim_inactive = v);
                            }
                        }))
                        .child(
                            div()
                                .w(px(40.))
                                .text_size(px(text::LABEL))
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(format!("{:.0} %", dim * 100.0))),
                        ),
                ),
            )
            .child(
                row(ui, "Ask before closing a running program")
                    .desc("When a pane still runs something besides its shell: a build, an editor, ssh")
                    .control(toggle(
                        "panes-confirm-close",
                        s.panes.confirm_close,
                        ui,
                        client(|s| s.panes.confirm_close = !s.panes.confirm_close),
                    )),
            )
            .child(
                row(ui, "Split and move between panes")
                    .desc("Ctrl+Shift+E splits right, Ctrl+Shift+O down, Alt+arrows move")
                    .icon(Icon::Keyboard)
                    .on_click("panes-shortcuts", self.listener_go(cx, Page::Shortcuts)),
            );
        div().flex().flex_col().gap(px(28.)).child(startup).child(tabs).child(panes).into_any_element()
    }
}
