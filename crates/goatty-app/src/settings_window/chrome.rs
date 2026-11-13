//! The settings window's frame: sidebar, page header, the floating bar for
//! unapplied host changes, dialogs, and `Render`.

use std::time::Duration;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, KeyDownEvent, ParentElement, Render, SharedString, Styled, Window, canvas, div,
    prelude::*, px, relative,
};

use super::kit::{self, search_field};
use super::{Page, SettingsWindow};
use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, Tone, banner, button, button_with, icon_button};
use crate::ui::focus::{self, Ring};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};
use gpui::Focusable;

/// Narrower windows get a navigation rail instead of the sidebar.
pub(super) const RAIL_BELOW: f32 = 920.;

/// How long a row opened from a search result stays highlighted.
const HIGHLIGHT: Duration = Duration::from_millis(1800);

impl SettingsWindow {
    /// Whether the draft changes something `page` shows (its sidebar dot).
    fn page_changed(&self, page: Page) -> bool {
        let (Some(d), Some(h)) = (&self.draft, &self.host) else { return false };
        let h = &h.loaded.value;
        let plugin = |id: &str| d.plugins.get(id) != h.plugins.get(id);
        match page {
            Page::Shell => d.shell != h.shell,
            Page::Prompt => d.prompt != h.prompt || plugin("starship"),
            Page::History => d.history != h.history || plugin("fzf") || plugin("fzf-tab"),
            Page::Aliases => {
                d.aliases != h.aliases
                    || d.plugins
                        .keys()
                        .chain(h.plugins.keys())
                        .any(|id| d.plugins.get(id).map(|p| &p.disabled_aliases) != h.plugins.get(id).map(|p| &p.disabled_aliases))
            }
            Page::Plugins => d.plugins != h.plugins,
            _ => false,
        }
    }

    /// The host the shell pages edit, for labels.
    pub(super) fn host_label(&self) -> String {
        self.link.as_ref().map(|l| l.label()).unwrap_or_else(|| "no host".into())
    }

    /// Up and Down on the sidebar (a single tab stop) change the page.
    fn nav_keys(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let k = &ev.keystroke;
        if k.modifiers.modified() {
            return;
        }
        let at = Page::ALL.iter().position(|p| *p == self.page).unwrap_or(0);
        let to = match k.key.as_str() {
            "up" => at.saturating_sub(1),
            "down" => (at + 1).min(Page::ALL.len() - 1),
            "home" => 0,
            "end" => Page::ALL.len() - 1,
            // Into the page.
            "enter" | "space" | "right" => {
                cx.stop_propagation();
                if self.drawer {
                    // The page's controls are behind the drawer and the drawer is
                    // drawn after them: focus waits on the page for its first control.
                    self.drawer = false;
                    focus::hand_over(&self.focus, focus::Handover::FirstOnPage, window, cx);
                } else {
                    window.focus_next(cx);
                }
                cx.notify();
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        self.search.update(cx, |i, cx| i.set_text("", cx));
        // Up and Down browse the pages: the drawer stays until a page is chosen.
        let drawer = self.drawer;
        self.go(Page::ALL[to], cx);
        self.drawer = drawer;
    }

    /// The pages as one tab stop around `items`: the focus ring is on the current page.
    fn nav_frame(&self, handle: &gpui::FocusHandle, id: &'static str, cx: &Context<'_, Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .role(gpui::Role::Navigation)
            .aria_label("Settings pages")
            .track_focus(&handle.clone().tab_stop(true))
            .on_key_down(cx.listener(Self::nav_keys))
    }

    fn nav_item(&self, ui: Ui, page: Page, keyboard: bool, cx: &Context<'_, Self>) -> impl IntoElement {
        let on = page == self.page && self.search.read(cx).text().is_empty();
        // While search results show no page is selected, but focus still has a place.
        let ring = keyboard && page == self.page;
        let changed = self.page_changed(page);
        div()
            .id(page.title())
            .role(gpui::Role::Link)
            .aria_label(page.title())
            .aria_selected(on)
            .flex()
            .items_center()
            .gap(px(12.))
            .h(px(38.))
            .px(px(12.))
            .rounded_full()
            .cursor_pointer()
            .when(ring, |d| d.shadow(focus::shadow(ui, Ring::Inner)))
            .text_size(px(text::BODY))
            .when(on, |d| d.bg(ui.accent_soft).text_color(ui.accent_text).font_weight(FontWeight::SEMIBOLD))
            .when(!on, |d| d.text_color(ui.text).hover(|s| s.bg(ui.field)))
            .child(page.icon().svg().size(px(18.)).text_color(if on { ui.accent_text } else { ui.muted }))
            .child(div().flex_1().child(page.title()))
            .when(changed, |d| d.child(div().size(px(8.)).rounded_full().bg(ui.warning)))
            .on_click(cx.listener(move |v, _, window, cx| {
                v.search.update(cx, |i, cx| i.set_text("", cx));
                v.go(page, cx);
                let nav = if v.drawer { v.drawer_focus.clone() } else { v.nav_focus.clone() };
                window.focus(&nav, cx);
            }))
    }

    /// The narrow window's navigation: icons with short labels (Material 3's rail).
    fn rail(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> impl IntoElement {
        let searching = !self.search.read(cx).text().is_empty();
        let keyboard = self.nav_focus.is_focused(window) && window.last_input_was_keyboard();
        let item = |page: Page| {
            let on = page == self.page && !searching;
            let changed = self.page_changed(page);
            div()
                .id(SharedString::from(format!("rail-{}", page.title())))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.))
                .w_full()
                .py(px(4.))
                .cursor_pointer()
                .role(gpui::Role::Link)
                .aria_label(page.title())
                .aria_selected(on)
                .when(keyboard && page == self.page, |d| d.rounded(px(radius::CONTROL)).shadow(focus::shadow(ui, Ring::Inner)))
                .group(SharedString::from(format!("rail-{}", page.title())))
                .child(
                    div()
                        .relative()
                        .w(px(52.))
                        .h(px(30.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(on, |d| d.bg(ui.accent_soft))
                        .when(!on, |d| d.group_hover(SharedString::from(format!("rail-{}", page.title())), |s| s.bg(ui.field)))
                        .child(page.icon().svg().size(px(18.)).text_color(if on { ui.accent_text } else { ui.muted }))
                        .when(changed, |d| d.child(div().absolute().top(px(3.)).right(px(10.)).size(px(7.)).rounded_full().bg(ui.warning))),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                        .text_color(if on { ui.accent_text } else { ui.muted })
                        .child(page.title()),
                )
                .on_click(cx.listener(move |v, _, window, cx| {
                    v.search.update(cx, |i, cx| i.set_text("", cx));
                    v.go(page, cx);
                    window.focus(&v.nav_focus, cx);
                }))
        };
        let open_drawer = |search: bool| {
            cx.listener(move |v: &mut Self, _: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<'_, Self>| {
                v.drawer = true;
                let handle = if search { v.search.focus_handle(cx) } else { v.drawer_focus.clone() };
                window.focus(&handle, cx);
                cx.notify();
            })
        };
        div()
            .w(px(80.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .bg(ui.sidebar)
            .border_r_1()
            .border_color(ui.divider)
            .pt(px(14.))
            .gap(px(4.))
            .child(icon_button("rail-menu", Icon::Menu, "Show the sidebar", ui, |_, _| {}).size(px(40.)).on_click(open_drawer(false)))
            .child(icon_button("rail-search", Icon::Search, "Search settings", ui, |_, _| {}).size(px(40.)).on_click(open_drawer(true)))
            .child(div().h(px(8.)))
            .child(
                self.nav_frame(&self.nav_focus, "rail-nav", cx)
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("rail-pages")
                            .flex_1()
                            .w_full()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(Page::ALL.into_iter().filter(|p| !matches!(p, Page::Profile | Page::About)).map(item)),
                    )
                    .child(
                        div()
                            .w_full()
                            .py(px(8.))
                            .border_t_1()
                            .border_color(ui.divider)
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(item(Page::Profile))
                            .child(item(Page::About)),
                    ),
            )
    }

    fn sidebar(&self, ui: Ui, handle: &gpui::FocusHandle, window: &Window, cx: &Context<'_, Self>) -> impl IntoElement {
        let keyboard = handle.is_focused(window) && window.last_input_was_keyboard();
        let (client, host, bottom): (Vec<Page>, Vec<Page>, Vec<Page>) = (
            Page::ALL.into_iter().filter(|p| !p.is_host() && !matches!(p, Page::Profile | Page::About)).collect(),
            Page::ALL.into_iter().filter(|p| p.is_host()).collect(),
            vec![Page::Profile, Page::About],
        );
        let caption = |t: String| {
            div()
                .px(px(12.))
                .pt(px(18.))
                .pb(px(6.))
                .text_size(px(text::CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui.faint)
                .truncate()
                .child(SharedString::from(t))
        };
        div()
            .w(px(252.))
            .flex_shrink_0()
            .h_full()
            .when(self.drawer, |d| d.w_full())
            .flex()
            .flex_col()
            .bg(ui.sidebar)
            .border_r_1()
            .border_color(ui.divider)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .px(px(14.))
                    .pt(px(20.))
                    .pb(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(6.))
                            .child(
                                div()
                                    .size(px(30.))
                                    .rounded(px(9.))
                                    .bg(ui.accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::Terminal.svg().size(px(17.)).text_color(ui.on_accent)),
                            )
                            .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child("Settings")),
                    )
                    .child(search_field(ui, "nav-search", &self.search, window, cx)),
            )
            .child(
                self.nav_frame(handle, if self.drawer { "drawer-nav" } else { "sidebar-nav" }, cx)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("nav")
                            .flex_1()
                            .overflow_y_scroll()
                            .px(px(10.))
                            .pb(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(caption("This computer".into()))
                            .children(client.into_iter().map(|p| self.nav_item(ui, p, keyboard, cx)))
                            .child(caption(format!("Shell on {}", self.host_label())))
                            .children(host.into_iter().map(|p| self.nav_item(ui, p, keyboard, cx))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .px(px(10.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(ui.divider)
                            .children(bottom.into_iter().map(|p| self.nav_item(ui, p, keyboard, cx))),
                    ),
            )
    }

    /// Title, summary (with the host for shell pages) and a back button in sub-views.
    fn header(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let searching = !self.search.read(cx).text().trim().is_empty();
        let (title, summary): (SharedString, SharedString) = if searching {
            ("Search".into(), "Settings on every page".into())
        } else if let Some((title, summary)) = self.sub_view_title() {
            (title, summary)
        } else {
            (self.page.title().into(), self.page.summary().into())
        };
        let back = !searching && self.sub_view_title().is_some();
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .when(back, |d| d.child(icon_button("back", Icon::ArrowLeft, "Back", ui, self.act(cx, |v, cx| v.back(cx))).size(px(36.))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .min_w_0()
                    .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).text_color(ui.text).child(title))
                    .child(div().flex().items_center().gap(px(8.)).text_size(px(text::LABEL)).text_color(ui.muted).child(summary).when(
                        self.page.is_host() && !searching,
                        |d| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .h(px(22.))
                                    .px(px(8.))
                                    .rounded_full()
                                    .bg(ui.card)
                                    .border_1()
                                    .border_color(ui.divider)
                                    .text_size(px(text::CAPTION))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.text)
                                    .child(Icon::Server.svg().size(px(12.)).text_color(ui.muted))
                                    .child(SharedString::from(self.host_label())),
                            )
                        },
                    )),
            )
    }

    /// A sub-view's title and summary (a plugin, an install review), if one is open.
    fn sub_view_title(&self) -> Option<(SharedString, SharedString)> {
        if self.page != Page::Plugins {
            return None;
        }
        if let Some(plan) = &self.plugins.plan {
            let verb = match &plan.replaces {
                Some(old) if *old != plan.version => "Update",
                Some(_) => "Reinstall",
                None => "Install",
            };
            return Some((format!("{verb} {}", plan.name).into(), "Review what it does before anything changes".into()));
        }
        let id = self.plugins.detail.as_ref()?;
        let inst = self.plugins.catalog.as_ref()?.installed.iter().find(|i| &i.id == id)?;
        Some((inst.manifest.name.clone().into(), inst.manifest.summary.clone().into()))
    }

    pub(super) fn back(&mut self, cx: &mut Context<'_, Self>) {
        if self.plugins.plan.take().is_some() {
            if let Some(page) = self.plugins.plan_return.take() {
                self.go(page, cx);
                return;
            }
        } else {
            self.plugins.detail = None;
        }
        self.menu = None;
        cx.notify();
    }

    // ---- Host changes ---------------------------------------------------------------

    pub(super) fn diff_view(&self, ui: Ui, cx: &Context<'_, Self>, diff: &[(String, String)]) -> impl IntoElement {
        div()
            .id("diff")
            .max_h(px(240.))
            .overflow_y_scroll()
            .rounded(px(10.))
            .bg(ui.bg)
            .border_1()
            .border_color(ui.divider)
            .p(px(10.))
            .font_family(kit::mono(cx))
            .text_size(px(text::CAPTION))
            .children(diff.iter().flat_map(|(_, text)| text.lines().map(str::to_owned).collect::<Vec<_>>()).map(|line| {
                let color = if line.starts_with("+++") || line.starts_with("---") {
                    ui.muted
                } else if line.starts_with('+') {
                    ui.success
                } else if line.starts_with('-') {
                    ui.danger
                } else if line.starts_with("@@") {
                    ui.accent_text
                } else {
                    ui.text
                };
                div().text_color(color).whitespace_nowrap().child(SharedString::from(if line.is_empty() { " ".to_string() } else { line }))
            }))
    }

    /// The bar below the page: unapplied host changes, or the
    /// result of the last preview/apply/rollback.
    fn bottom_bar(&self, ui: Ui, cx: &Context<'_, Self>) -> Option<AnyElement> {
        if self.draft.is_none() && self.outcome.is_none() {
            return None;
        }
        let mut stack = div().flex().flex_col().gap(px(10.)).w_full().max_w(px(720.));
        if let Some(o) = &self.outcome {
            let tone = if o.ok { Tone::Success } else { Tone::Danger };
            let mut lines = o.lines.clone();
            let title = if lines.is_empty() { String::new() } else { lines.remove(0) };
            let offer_reload = self.draft.is_none() && o.reload;
            let mut card = div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(6.))
                .rounded(px(20.))
                .bg(ui.menu)
                .border_1()
                .border_color(ui.border)
                .shadow_xl()
                .child(banner(ui, tone, title, lines));
            let mut actions = div().flex().items_center().justify_end().gap(px(8.)).px(px(8.)).pb(px(6.));
            if !o.diff.is_empty() {
                actions = actions.child(button_with(
                    "toggle-diff",
                    Some(Icon::Code),
                    if self.show_diff { "Hide changes" } else { "Show changes" },
                    ButtonKind::Ghost,
                    ui,
                    self.act(cx, |v, cx| {
                        v.show_diff = !v.show_diff;
                        cx.notify();
                    }),
                ));
            }
            actions = actions.child(div().flex_1());
            if offer_reload {
                actions = actions.child(button_with(
                    "reload-shells",
                    Some(Icon::Refresh),
                    "Reload running shells",
                    ButtonKind::Primary,
                    ui,
                    self.act(cx, |v, cx| v.reload_shells(cx)),
                ));
            }
            actions = actions.child(button(
                "dismiss",
                if offer_reload { "Not now" } else { "Dismiss" },
                ButtonKind::Normal,
                ui,
                self.act(cx, |v, cx| {
                    v.outcome = None;
                    cx.notify();
                }),
            ));
            if self.show_diff && !o.diff.is_empty() {
                card = card.child(div().px(px(8.)).child(self.diff_view(ui, cx, &o.diff)));
            }
            stack = stack.child(card.child(actions));
        }
        if self.draft.is_some() {
            stack = stack.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(56.))
                    .pl(px(20.))
                    .pr(px(10.))
                    .rounded_full()
                    .bg(ui.menu)
                    .border_1()
                    .border_color(ui.border)
                    .shadow_xl()
                    .child(div().size(px(8.)).rounded_full().bg(ui.warning))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child("Unapplied shell changes"))
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(ui.muted)
                                    .truncate()
                                    .child(SharedString::from(format!("For new sessions on {}", self.host_label()))),
                            ),
                    )
                    .when(self.busy, |d| d.child(div().text_size(px(text::SMALL)).text_color(ui.muted).child("Working…")))
                    .child(button("discard", "Discard", ButtonKind::Ghost, ui, self.act(cx, |v, cx| v.discard(cx))))
                    .child(button("preview", "Preview", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.preview(cx))))
                    .child(button_with("apply", Some(Icon::Check), "Apply", ButtonKind::Primary, ui, self.act(cx, |v, cx| v.apply(cx)))),
            );
        }
        // In the flow below the page, not floating over it: the page's scroll area ends
        // above the bar, so the last row is always reachable however tall the bar is. It
        // scrolls on its own if it would take more than most of the window.
        Some(
            div()
                .id("bottom-bar")
                .flex_none()
                .max_h(relative(0.6))
                .overflow_y_scroll()
                .track_scroll(&self.bar_scroll)
                .px(px(32.))
                .pt(px(10.))
                .pb(px(20.))
                .flex()
                // Not stretched: the stack keeps its content height, so the scroll range covers all of it.
                .items_start()
                .justify_center()
                .child(stack)
                .into_any_element(),
        )
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.record_key(ev, cx) {
            return;
        }
        let k = &ev.keystroke;
        if k.key == "escape" && !k.modifiers.modified() {
            if self.menu.is_some() {
                self.close_menu(window, cx);
            } else if self.drawer {
                self.drawer = false;
                window.focus(&self.nav_focus, cx);
            } else if self.aliases_modal_open() {
                self.close_alias_form(true, cx);
            } else if !self.search.read(cx).text().is_empty() {
                self.search.update(cx, |i, cx| i.set_text("", cx));
                window.focus(&self.focus, cx);
            } else if self.plugins.plan.is_some() || self.plugins.detail.is_some() {
                self.back(cx);
            } else {
                return;
            }
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// The row a search result opened, while it's still highlighted.
    fn highlighted(&self, cx: &mut Context<'_, Self>) -> Option<SharedString> {
        let (label, at) = self.highlight.clone()?;
        let left = HIGHLIGHT.checked_sub(at.elapsed())?;
        // Repaint once it's over, to drop the highlight.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(left).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        Some(label)
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        focus::begin(cx, window, &self.scroll, &self.menu_scroll);
        if let Some(back) = self.aliases.restore.take() {
            window.focus(&back, cx);
        }
        let focus_row = self.highlighted(cx);
        kit::set_focus_row(focus_row, &self.scroll);
        let query = self.search.read(cx).text().trim().to_lowercase();
        let notes = if query.is_empty() { self.row_notes(cx) } else { Default::default() };
        kit::set_row_notes(window.window_handle().window_id(), notes);
        let cx: &Context<'_, Self> = cx;
        let page = if !query.is_empty() {
            self.search_results(ui, &query, cx)
        } else {
            match self.page {
                Page::Appearance => self.appearance(ui, window, cx),
                Page::Terminal => self.terminal(ui, cx),
                Page::Workspace => self.workspace(ui, cx),
                Page::Shortcuts => self.shortcuts_page(ui, window, cx),
                Page::Hosts => self.hosts_page(ui, cx),
                Page::Shell => self.shell(ui, cx),
                Page::Prompt => self.prompt(ui, cx),
                Page::History => self.history(ui, cx),
                Page::Aliases => self.aliases_page(ui, window, cx),
                Page::Plugins => self.plugins_page(ui, window, cx),
                Page::Diagnostics => self.diagnostics(ui, cx),
                Page::Profile => self.profile(ui, cx),
                Page::About => self.about(ui, cx),
            }
        };
        let bar = self.bottom_bar(ui, cx);
        focus::bar(bar.is_some().then_some(&self.bar_scroll));
        let modal = self.alias_modal(ui, window, cx);
        let compact = f32::from(window.viewport_size().width) < RAIL_BELOW;
        let gutter = if compact { 24. } else { 40. };
        let nav = if compact {
            self.rail(ui, window, cx).into_any_element()
        } else {
            self.sidebar(ui, &self.nav_focus, window, cx).into_any_element()
        };
        // The drawer: the full sidebar over the page, closed by a click beside it.
        let drawer = (compact && self.drawer).then(|| {
            div()
                .id("drawer-scrim")
                .absolute()
                .inset_0()
                .bg(ui.scrim)
                .occlude()
                .on_click(cx.listener(|v, _, window, cx| {
                    v.drawer = false;
                    window.focus(&v.nav_focus, cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .id("drawer")
                        .track_focus(&self.drawer_trap)
                        .h_full()
                        .w(px(280.))
                        .occlude()
                        .shadow_2xl()
                        .child(self.sidebar(ui, &self.drawer_focus, window, cx))
                        .on_click(|_, _, cx| cx.stop_propagation()),
                )
        });
        let root = div()
            .size_full()
            .relative()
            .flex()
            .bg(ui.bg)
            .text_color(ui.text)
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .key_context(if self.recording.is_some() || self.shortcuts.listening { "SettingsWindow recording" } else { super::CONTEXT })
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::focus_next_control))
            .on_action(cx.listener(Self::focus_prev_control))
            .on_action(cx.listener(Self::next_page))
            .on_action(cx.listener(Self::prev_page))
            .on_key_down(cx.listener(Self::on_key))
            .child(nav)
            // After layout: a focused control that is gone gives its focus to the page.
            .child({
                let root = self.focus.clone();
                canvas(|_, _, _| (), move |_, _, window, cx| focus::rescue(&root, window, cx)).absolute().size_0()
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div().id("page").flex_1().overflow_y_scroll().track_scroll(&self.scroll).child(
                            div().flex().justify_center().px(px(gutter)).child(
                                div()
                                    .w_full()
                                    .max_w(px(780.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(28.))
                                    .pt(px(30.))
                                    .pb(px(48.))
                                    .child(self.header(ui, cx))
                                    .child(page),
                            ),
                        ),
                    )
                    .child(div().absolute().top(px(14.)).right(px(14.)).child(
                        icon_button("close-window", Icon::Close, "Close  (Ctrl+W)", ui, |window, _| window.remove_window()).size(px(34.)),
                    ))
                    .children(bar),
            )
            .children(drawer)
            .children(modal);
        if focus::end() {
            // Some controls had no focus handle: the next frame has more.
            window.refresh();
        }
        root
    }
}
