//! The vertical tab style (`tabs.style = "vertical"`, #168), in the manner
//! of Arc and Zen: a sidebar on the left in the tab strip's color, a row
//! per tab, and the panes as a rounded card beside it.
//!
//! - **Rows**: the program's icon, the title and, below it, the directory
//!   (plus the host when remote); activity markers, the pane count, a close
//!   button on hover. The active row is raised in the terminal's color.
//!   "New tab" ends the list; the list scrolls and keeps the active tab in
//!   view.
//! - **Rail**: folded (`tabs.sidebar_collapsed`, Ctrl+Shift+B, the button in
//!   the footer, or dragging the edge far left) it's a column of icon tiles
//!   with their markers as dots and the title as a tooltip. A tab without
//!   an icon shows its title's first letter.
//! - **Edge**: drag it to resize (`tabs.sidebar_width`; at most 45 % of the
//!   window), past [`gestures::RAIL_WIDTH`]'s neighbourhood to fold or
//!   unfold, double-click to fold or unfold. The width is saved shortly
//!   after the drag stops, not every frame.
//! - **Card**: the panes get a margin and rounded corners. The terminal
//!   draws square, so a ring in the sidebar's color is drawn over its
//!   corners: a border as wide as the margin whose outer radius is the
//!   card's radius plus the margin, which leaves exactly the card's radius
//!   on its inner edge (GPUI's quads shrink the inner radius by the border
//!   width).

use std::time::Duration;

use gpui::{
    AnyElement, Context, DragMoveEvent, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::*, px,
};

use super::Workspace;
use super::gestures::{RAIL_WIDTH, Sidebar};
use super::tab_bar::{Axis, Colors, TabView, activity_look};
use crate::actions::{NewTab, OpenSettings, ToggleTabSidebar};
use crate::settings::AppSettings;
use crate::ui::controls::tooltip;
use crate::ui::icons::Icon;
use crate::ui::style::{radius, text};

/// The sidebar's padding; on its right, the gap to the card.
const PAD: f32 = 8.0;
/// The card's margin at the top, right and bottom of the window.
const MARGIN: f32 = 8.0;
/// The card's corner radius.
const CARD_RADIUS: f32 = 10.0;
/// The resize handle's width, centered on the sidebar's edge.
const HANDLE: f32 = 8.0;
/// A rail tile's size.
const TILE: f32 = 38.0;
/// How long the sidebar's size has to stay put before it's saved.
const SAVE_AFTER: Duration = Duration::from_millis(400);

/// Dragging the sidebar's edge (a GPUI drag; see `on_sidebar_drag`).
#[derive(Clone)]
pub(super) struct SidebarDrag;

impl Workspace {
    /// The sidebar's size: while its edge is dragged, the dragged one.
    fn sidebar(&self, cx: &gpui::App) -> Sidebar {
        self.sidebar_drag.map_or_else(|| saved_sidebar(cx), |(_, now)| now)
    }

    /// How wide the sidebar is drawn in this window.
    pub(super) fn sidebar_width(&self, window: &Window, cx: &gpui::App) -> f32 {
        self.sidebar(cx).drawn_width(f32::from(window.viewport_size().width))
    }

    /// The window's root in the vertical style: a row in the sidebar's
    /// color that follows a drag of the sidebar's edge.
    pub(super) fn sidebar_frame(&self, root: gpui::Stateful<gpui::Div>, cx: &mut Context<'_, Self>) -> gpui::Stateful<gpui::Div> {
        root.flex_row().bg(Colors::get(cx).bar).on_drag_move(cx.listener(Self::on_sidebar_drag)).on_drop(cx.listener(
            |this, _: &SidebarDrag, _, cx| {
                this.save_sidebar = None;
                this.save_sidebar_size(true, cx);
            },
        ))
    }

    fn on_sidebar_drag(&mut self, ev: &DragMoveEvent<SidebarDrag>, window: &mut Window, cx: &mut Context<'_, Self>) {
        let x = f32::from(ev.event.position.x - ev.bounds.left());
        let (start, so_far) = self.sidebar_drag.unwrap_or_else(|| (saved_sidebar(cx), saved_sidebar(cx)));
        let now = so_far.dragged_to(x, f32::from(window.viewport_size().width), start);
        if now == so_far && self.sidebar_drag.is_some() {
            return;
        }
        self.sidebar_drag = Some((start, now));
        cx.notify();
        self.save_sidebar = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            this.update(cx, |this, cx| this.save_sidebar_size(false, cx)).ok();
        }));
    }

    /// Save the dragged size to the settings. `done`: the drag ended.
    fn save_sidebar_size(&mut self, done: bool, cx: &mut Context<'_, Self>) {
        let Some((_, s)) = self.sidebar_drag else { return };
        let tabs = &cx.global::<AppSettings>().client.tabs;
        if tabs.sidebar_width != s.width || tabs.sidebar_collapsed != s.folded {
            AppSettings::update(cx, |c| {
                c.tabs.sidebar_width = s.width;
                c.tabs.sidebar_collapsed = s.folded;
            });
        }
        // Released outside the window: no drop, but no drag either.
        if done || !cx.has_active_drag() {
            self.sidebar_drag = None;
            cx.notify();
        }
    }

    pub(super) fn render_sidebar(&self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
        let sidebar = self.sidebar(cx);
        let width = sidebar.drawn_width(f32::from(window.viewport_size().width));
        let c = Colors::get(cx);
        let tabs = self.tab_views(cx);
        let ui_font = cx.global::<AppSettings>().ui_font.clone();
        let frame = div()
            .id("tab-sidebar")
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .font_family(ui_font)
            .text_color(c.ui.text)
            .on_scroll(cx.listener(Self::on_tabs_scroll));
        if sidebar.folded {
            let tiles: Vec<_> = tabs.into_iter().map(|t| self.rail_tile(t, c, cx)).collect();
            let new_tab = sidebar_button("tab-rail-new", Icon::Plus, "New tab", c).size(px(TILE)).on_click(dispatch(NewTab));
            frame
                .px(px((RAIL_WIDTH - TILE) / 2.))
                .py(px(PAD))
                .child(
                    div()
                        .id("tab-list")
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.))
                        .overflow_y_scroll()
                        .track_scroll(&self.tab_scroll)
                        .children(tiles)
                        .child(new_tab)
                        .child(self.tabs_rest("tab-rail-rest", cx).w_full().flex_1().min_h(px(TILE))),
                )
                .child(
                    div().flex_none().flex().justify_center().pt(px(6.)).child(
                        sidebar_button("tab-sidebar-unfold", Icon::PanelLeftOpen, "Unfold the tab sidebar", c)
                            .size(px(TILE))
                            .on_click(dispatch(ToggleTabSidebar)),
                    ),
                )
                .into_any_element()
        } else {
            let rows: Vec<_> = tabs.into_iter().map(|t| self.sidebar_row(t, c, cx)).collect();
            let new_tab = div()
                .id("tab-sidebar-new")
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(34.))
                .px(px(10.))
                .rounded(px(radius::CONTROL))
                .cursor_pointer()
                .text_size(px(text::LABEL))
                .text_color(c.ui.muted)
                .hover(|s| s.bg(c.hover).text_color(c.ui.text))
                .child(Icon::Plus.svg().size(px(16.)).text_color(c.ui.muted))
                .child("New tab")
                .on_click(dispatch(NewTab));
            let footer = div()
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .pt(px(6.))
                .child(
                    sidebar_button("tab-sidebar-fold", Icon::PanelLeftClose, "Fold the tab sidebar", c)
                        .size(px(30.))
                        .on_click(dispatch(ToggleTabSidebar)),
                )
                .child(sidebar_button("tab-sidebar-settings", Icon::Sliders, "Settings", c).size(px(30.)).on_click(dispatch(OpenSettings)));
            frame
                .p(px(PAD))
                .child(
                    div()
                        .id("tab-list")
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .overflow_y_scroll()
                        .track_scroll(&self.tab_scroll)
                        .children(rows)
                        .child(new_tab)
                        .child(self.tabs_rest("tab-sidebar-rest", cx).flex_1().min_h(px(24.))),
                )
                .child(footer)
                .into_any_element()
        }
    }

    /// A tab in the unfolded sidebar.
    fn sidebar_row(&self, t: TabView, c: Colors, cx: &mut Context<'_, Self>) -> AnyElement {
        let icon = self.tab_icon(&t, 15.0, c, cx, || Icon::Terminal.svg().size(px(15.)).text_color(c.ui.muted).into_any_element());
        let title = self.tab_title(&t, cx);
        let detail = t.detail.clone().filter(|d| *d != t.label && self.rename.as_ref().is_none_or(|r| r.tab != t.id));
        let marker = self.activity_marker(&t, 13.0, c);
        let close = self.close_button(&t, 22.0, c, cx);
        let panes = (t.panes > 1).then(|| {
            div()
                .flex_none()
                .px(px(6.))
                .rounded_full()
                .bg(c.ui.text.opacity(0.08))
                .text_size(px(text::CAPTION))
                .text_color(c.ui.muted)
                .child(SharedString::from(format!("{}", t.panes)))
        });
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(div().flex().min_w_0().text_size(px(text::LABEL)).child(title))
            .when_some(detail, |d, detail| {
                d.child(div().min_w_0().truncate().text_size(px(text::CAPTION)).text_color(c.ui.faint).child(detail))
            });
        self.tab_shell(&t, Axis::Column, cx)
            .flex_none()
            .min_h(px(36.))
            .py(px(5.))
            .pl(px(10.))
            .pr(px(6.))
            .gap(px(10.))
            .rounded(px(radius::CONTROL))
            .border_1()
            .when(t.active, |d| d.bg(c.terminal).border_color(c.edge).shadow_sm().text_color(c.ui.text))
            .when(!t.active, |d| {
                d.border_color(gpui::transparent_black()).text_color(c.ui.muted).hover(|s| s.bg(c.hover).text_color(c.ui.text))
            })
            .child(icon)
            .child(text)
            .children(marker)
            .children(panes)
            .child(close)
            .into_any_element()
    }

    /// A tab in the folded sidebar: its icon (or first letter), a dot for
    /// activity, the title as a tooltip.
    fn rail_tile(&self, t: TabView, c: Colors, cx: &mut Context<'_, Self>) -> AnyElement {
        let letter = t.label.chars().find(|ch| ch.is_alphanumeric()).unwrap_or('~').to_uppercase().to_string();
        let icon = self.tab_icon(&t, 17.0, c, cx, || {
            div()
                .text_size(px(text::BODY))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c.ui.muted)
                .child(SharedString::from(letter))
                .into_any_element()
        });
        let tip = match (&t.detail, t.panes) {
            (Some(d), 1) => format!("{}\n{d}", t.label),
            (Some(d), n) => format!("{}\n{d} · {n} panes", t.label),
            (None, 1) => t.label.to_string(),
            (None, n) => format!("{} · {n} panes", t.label),
        };
        let dot = t.activity.map(|a| {
            div().absolute().top(px(3.)).right(px(3.)).size(px(9.)).rounded_full().border_2().border_color(c.bar).bg(activity_look(a, c).1)
        });
        self.tab_shell(&t, Axis::Column, cx)
            .flex_none()
            .size(px(TILE))
            .justify_center()
            .rounded(px(radius::CONTROL))
            .border_1()
            .when(t.active, |d| d.bg(c.terminal).border_color(c.edge).shadow_sm())
            .when(!t.active, |d| d.border_color(gpui::transparent_black()).hover(|s| s.bg(c.hover)))
            .tooltip(tooltip(tip))
            .child(icon)
            .children(dot)
            .into_any_element()
    }

    /// The panes beside the sidebar, as a rounded card.
    pub(super) fn render_card(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let c = Colors::get(cx);
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .pt(px(MARGIN))
            .pr(px(MARGIN))
            .pb(px(MARGIN))
            .child(self.render_panes(cx))
            // The corners: see the module docs. The ring reaches left over
            // the sidebar's padding, which is the same color.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left(px(-MARGIN))
                    .border(px(MARGIN))
                    .border_color(c.bar)
                    .rounded(px(CARD_RADIUS + MARGIN)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(MARGIN))
                    .right(px(MARGIN))
                    .bottom(px(MARGIN))
                    .left_0()
                    .rounded(px(CARD_RADIUS))
                    .border_1()
                    .border_color(c.edge),
            )
            .into_any_element()
    }

    /// The handle on the sidebar's edge: drag to resize or fold,
    /// double-click to fold or unfold.
    pub(super) fn render_sidebar_handle(&self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
        let c = Colors::get(cx);
        let x = self.sidebar_width(window, cx);
        let dragging = self.sidebar_drag.is_some();
        let line = div().w(px(2.)).h_full().rounded_full().when(dragging, |d| d.bg(c.ui.accent.opacity(0.7)));
        div()
            .id("tab-sidebar-edge")
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(x - HANDLE / 2.))
            .w(px(HANDLE))
            .flex()
            .justify_center()
            .py(px(MARGIN + CARD_RADIUS))
            .occlude()
            .cursor_col_resize()
            .group("tab-sidebar-edge")
            .child(line.group_hover("tab-sidebar-edge", |s| s.bg(c.ui.accent.opacity(0.5))))
            .on_drag(SidebarDrag, |_, _, _, cx| cx.new(|_| gpui::EmptyView))
            .on_click(|ev, window, cx| {
                if ev.click_count() == 2 {
                    window.dispatch_action(Box::new(ToggleTabSidebar), cx);
                }
            })
            .into_any_element()
    }
}

/// The sidebar's size in the settings.
fn saved_sidebar(cx: &gpui::App) -> Sidebar {
    let tabs = &cx.global::<AppSettings>().client.tabs;
    Sidebar { width: tabs.sidebar_width, folded: tabs.sidebar_collapsed }
}

/// A square icon button in the sidebar's colors (size is the caller's).
fn sidebar_button(id: &'static str, icon: Icon, tip: &'static str, c: Colors) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(id);
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::CONTROL))
        .cursor_pointer()
        .hover(|s| s.bg(c.hover))
        .tooltip(tooltip(tip))
        .child(icon.svg().size(px(16.)).text_color(c.ui.muted).group_hover(group, |s| s.text_color(c.ui.text)))
}

/// A click handler that dispatches `action` from the clicked element.
fn dispatch<A: gpui::Action + Clone>(action: A) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static {
    move |_, window, cx| window.dispatch_action(Box::new(action.clone()), cx)
}
