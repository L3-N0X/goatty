//! The tabs, drawn by goatty (not the window manager). This module draws
//! the horizontal strip in two styles (`tabs.style`) and holds what every
//! style shares; the vertical style is in `sidebar`.
//!
//! - **modern**: browser-like tabs with rounded tops; the active one has the
//!   terminal's background and merges into it. Icon, title, close button
//!   (always on the active tab, on hover for the others), a + button.
//! - **compact**: one line of small monospace text, kitty-style: number,
//!   icon and title; the active tab in the accent color.
//!
//! Every style: click to switch, middle-click to close, double-click to
//! rename, right-click for the tab's menu (`tab_menu`), double-click the
//! empty space for a new tab. Dragging a tab moves it live: the others make
//! room as it passes their middle (`gestures::reorder_target`). With
//! `tabs.scroll_to_switch` the mouse wheel over the tabs switches tabs;
//! otherwise it scrolls tabs that don't fit. The active tab is always
//! scrolled into view. A background tab shows what happened in it since it
//! was last shown (#122, `tabs.mark_bell` / `tabs.mark_activity`): a bell,
//! or a dot for a finished command (red when it failed) or for new output.

use std::time::{Duration, Instant};

use goatty_config::TabStyle;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, DragMoveEvent, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Render, ScrollDelta, ScrollEvent, SharedString, StatefulInteractiveElement, Styled, Transformation, Window, div, percentage,
    prelude::*, px, rgb,
};

use super::gestures::{Wheel, reorder_target};
use super::icons::{IconColor, icon_for};
use super::{DraggedTab, Tab, Workspace, marks};
use crate::actions::NewTab;
use crate::settings::AppSettings;
use crate::terminal::Activity;
use crate::ui::controls::tooltip;
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};

const MODERN_HEIGHT: f32 = 38.0;
const MODERN_TAB_HEIGHT: f32 = 32.0;
const COMPACT_HEIGHT: f32 = 24.0;
/// Modern tabs share the strip down to this width; then it scrolls.
const MIN_TAB: f32 = 96.0;
const MAX_TAB: f32 = 220.0;
/// Narrower modern tabs only show a close button when active.
const NARROW_TAB: f32 = 120.0;

/// Colors of the tabs, from the theme.
#[derive(Clone, Copy)]
pub(super) struct Colors {
    pub ui: Ui,
    /// Behind the tabs: the strip, the sidebar, the frame around the card.
    pub bar: Hsla,
    /// The terminal's background (the active tab).
    pub terminal: Hsla,
    pub hover: Hsla,
    /// A faint line around raised things (the active tab in the sidebar,
    /// the panes card): lighter in dark themes, darker in light ones.
    pub edge: Hsla,
}

impl Colors {
    pub fn get(cx: &App) -> Self {
        let ui = Ui::get(cx);
        let colors = &cx.global::<AppSettings>().colors;
        let terminal: Hsla = rgb(colors.background.0).into();
        // A bit darker than the terminal in dark themes, a bit greyer in light ones.
        let bar =
            if colors.is_dark() { Hsla { l: terminal.l * 0.7, ..terminal } } else { Hsla { l: (terminal.l - 0.06).max(0.0), ..terminal } };
        let hover = if colors.is_dark() { Hsla { l: (bar.l + 0.06).min(1.0), ..bar } } else { Hsla { l: (bar.l - 0.05).max(0.0), ..bar } };
        let edge = ui.text.opacity(if colors.is_dark() { 0.07 } else { 0.1 });
        Self { ui, bar, terminal, hover, edge }
    }

    pub fn icon(&self, color: IconColor, cx: &App) -> Hsla {
        match color {
            IconColor::Text => self.ui.text,
            IconColor::Ansi(i) => rgb(cx.global::<AppSettings>().colors.ansi[i as usize % 16].0).into(),
        }
    }
}

/// The tab under the pointer while dragging: its icon and title on a
/// raised card.
impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let c = Colors::get(cx);
        let icon = match self.icon {
            Some((glyph, color)) => div()
                .font(crate::fonts::icons())
                .text_size(px(14.))
                .text_color(color)
                .child(SharedString::from(glyph.to_string()))
                .into_any_element(),
            None => Icon::Terminal.svg().size(px(14.)).text_color(c.ui.muted).into_any_element(),
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .max_w(px(260.))
            .h(px(32.))
            .px(px(12.))
            .rounded(px(radius::CONTROL))
            .bg(c.terminal)
            .border_1()
            .border_color(c.ui.accent.opacity(0.6))
            .shadow_md()
            .text_size(px(text::LABEL))
            .text_color(c.ui.text)
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .child(icon)
            .child(div().min_w_0().truncate().child(self.label.clone()))
    }
}

/// What one tab shows.
pub(super) struct TabView {
    pub index: usize,
    pub id: u64,
    pub label: SharedString,
    pub active: bool,
    pub process: Option<String>,
    pub panes: usize,
    /// What happened in it while in the background.
    pub activity: Option<Activity>,
    /// The sidebar's second line: the directory, and the host when remote.
    pub detail: Option<SharedString>,
    /// Still connecting (no session yet).
    pub connecting: bool,
    /// Being dragged: drawn faded where it is.
    pub dragged: bool,
}

/// Which way tabs are laid out (for dragging).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    Row,
    Column,
}

impl Workspace {
    pub(super) fn render_tab_bar(&self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
        let tabs = self.tab_views(cx);
        let c = Colors::get(cx);
        let style = cx.global::<AppSettings>().client.tabs.style;
        let width = modern_tab_width(f32::from(window.viewport_size().width), tabs.len());
        let (height, items) = match style {
            TabStyle::Compact => (COMPACT_HEIGHT, tabs.into_iter().map(|t| self.compact_tab(t, c, cx)).collect::<Vec<_>>()),
            _ => (MODERN_HEIGHT, tabs.into_iter().map(|t| self.modern_tab(t, width, c, cx)).collect()),
        };
        let modern = style != TabStyle::Compact;
        let new_tab = div()
            .id("tab-new")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(c.ui.muted)
            .hover(|s| s.bg(c.hover).text_color(c.ui.text))
            .tooltip(tooltip("New tab"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTab), cx));
        let new_tab = if modern {
            new_tab.size(px(28.)).mb(px(2.)).rounded_md().child(Icon::Plus.svg().size(px(15.)).text_color(c.ui.muted))
        } else {
            new_tab.h_full().px(px(10.)).child("+")
        };
        div()
            .id("tab-bar")
            .flex_none()
            .h(px(height))
            .w_full()
            .flex()
            .bg(c.bar)
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .on_scroll(cx.listener(Self::on_tabs_scroll))
            .when(modern, |d| d.items_end().px(px(6.)).gap(px(2.)))
            .when(!modern, |d| d.items_stretch().text_size(px(12.)))
            .child(
                // The tabs scroll sideways when they don't fit.
                div()
                    .id("tab-strip")
                    .flex()
                    .min_w_0()
                    .flex_shrink(1.)
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .when(modern, |d| d.items_end().gap(px(2.)))
                    .children(items),
            )
            .child(new_tab)
            .child(self.tabs_rest("tab-bar-rest", cx).flex_1().h_full())
            .into_any_element()
    }

    /// The empty space after the tabs: double-click for a new tab, drag a
    /// tab here to move it to the end.
    pub(super) fn tabs_rest(&self, id: &'static str, cx: &mut Context<'_, Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .on_click(|ev, window, cx| {
                if ev.click_count() == 2 {
                    window.dispatch_action(Box::new(NewTab), cx);
                }
            })
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<DraggedTab>, window, cx| {
                if !this.moved_unpainted && ev.bounds.contains(&ev.event.position) {
                    let id = ev.drag(cx).id;
                    this.dragging = Some(id);
                    this.move_tab_to(id, usize::MAX, window, cx);
                    this.moved_unpainted = true;
                }
            }))
            .on_drop(cx.listener(|this, d: &DraggedTab, window, cx| this.move_tab_to(d.id, usize::MAX, window, cx)))
    }

    pub(super) fn tab_views(&self, cx: &App) -> Vec<TabView> {
        self.tabs.iter().enumerate().map(|(i, t)| self.tab_view(i, t, cx)).collect()
    }

    fn tab_view(&self, index: usize, tab: &Tab, cx: &App) -> TabView {
        let view = tab.focused_view().map(|v| v.read(cx));
        let process = view.and_then(|v| v.process().map(str::to_string));
        let active = index == self.active;
        // Turning a mark off in the settings hides the ones already there.
        let activity = tab.activity.filter(|&a| !active && marks(&cx.global::<AppSettings>().client.tabs, a));
        let connecting = tab.connecting.is_some();
        let detail = if connecting {
            Some("Connecting…".into())
        } else {
            let host = view.and_then(|v| v.session().host_label.clone()).or_else(|| match &tab.host {
                Some(crate::session::Spawner::Plain { host, .. }) => Some(host.clone()),
                _ => None,
            });
            let home = std::env::var_os("HOME");
            let dir = view.and_then(|v| v.cwd()).map(|d| super::short_dir(&d, home.as_deref().map(std::path::Path::new)));
            match (dir, host) {
                (Some(d), Some(h)) => Some(format!("{d} · {h}").into()),
                (Some(d), None) => Some(d.into()),
                (None, Some(h)) => Some(h.into()),
                (None, None) => None,
            }
        };
        TabView {
            index,
            id: tab.id,
            label: self.tab_label(tab, cx).into(),
            active,
            process,
            panes: tab.panes.len(),
            activity,
            detail,
            connecting,
            dragged: self.dragging == Some(tab.id),
        }
    }

    /// The mouse wheel over the tabs: switch tabs (`tabs.scroll_to_switch`),
    /// else let the tabs scroll.
    pub(super) fn on_tabs_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !cx.global::<AppSettings>().client.tabs.scroll_to_switch {
            return;
        }
        // Not the strip's own scrolling (the active tab stays in view).
        cx.stop_propagation();
        let dominant = |x: f32, y: f32| if y.abs() >= x.abs() { y } else { x };
        let wheel = match ev.delta {
            ScrollDelta::Lines(p) => Wheel::Lines(dominant(p.x, p.y)),
            ScrollDelta::Pixels(p) => Wheel::Pixels(dominant(f32::from(p.x), f32::from(p.y))),
        };
        let steps = self.wheel.feed(wheel, Instant::now());
        if steps != 0 {
            self.step_tab(steps, window, cx);
        }
    }

    /// The activity marker, `size` px across: a bell, or a dot colored by
    /// what happened. Hovering it says what it means.
    pub(super) fn activity_marker(&self, t: &TabView, size: f32, c: Colors) -> Option<AnyElement> {
        let activity = t.activity?;
        let (tip, color) = activity_look(activity, c);
        let mark = match activity {
            Activity::Bell => Icon::Bell.svg().size(px(size)).text_color(color).into_any_element(),
            _ => div().size(px(size * 0.55)).rounded_full().bg(color).into_any_element(),
        };
        Some(
            div()
                .id(gpui::ElementId::NamedInteger("tab-activity".into(), t.id))
                .flex_none()
                .size(px(size))
                .flex()
                .items_center()
                .justify_center()
                .tooltip(tooltip(tip))
                .child(mark)
                .into_any_element(),
        )
    }

    /// The program icon in `size` px, or `None` (unknown program, icons
    /// off). `color` overrides the icon's own color.
    pub(super) fn process_icon(&self, process: Option<&str>, size: f32, color: Option<Hsla>, c: Colors, cx: &App) -> Option<AnyElement> {
        let (glyph, own) = self.process_glyph(process, c, cx)?;
        Some(
            div()
                .flex_none()
                .w(px(size + 2.))
                .flex()
                .justify_center()
                .font(crate::fonts::icons())
                .text_size(px(size))
                .line_height(px(size + 2.))
                .text_color(color.unwrap_or(own))
                .child(SharedString::from(glyph.to_string()))
                .into_any_element(),
        )
    }

    /// The program's glyph and color, unless icons are off or it has none.
    fn process_glyph(&self, process: Option<&str>, c: Colors, cx: &App) -> Option<(char, Hsla)> {
        if !cx.global::<AppSettings>().client.tabs.icons {
            return None;
        }
        let icon = icon_for(process?)?;
        Some((icon.glyph, c.icon(icon.color, cx)))
    }

    /// The tab's icon: a spinner while it connects, the program's icon, or
    /// `fallback`.
    pub(super) fn tab_icon(&self, t: &TabView, size: f32, c: Colors, cx: &App, fallback: impl FnOnce() -> AnyElement) -> AnyElement {
        if t.connecting {
            return Icon::Loader
                .svg()
                .size(px(size))
                .text_color(c.ui.muted)
                .with_animation(("tab-connecting", t.id), Animation::new(Duration::from_millis(900)).repeat(), |s, t| {
                    s.with_transformation(Transformation::rotate(percentage(t)))
                })
                .into_any_element();
        }
        self.process_icon(t.process.as_deref(), size, None, c, cx).unwrap_or_else(fallback)
    }

    /// What every style's tab does: switch, close, rename, open its menu,
    /// move while dragged (along `axis`), switch with the wheel.
    pub(super) fn tab_shell(&self, t: &TabView, axis: Axis, cx: &mut Context<'_, Self>) -> gpui::Stateful<gpui::Div> {
        let (index, id) = (t.index, t.id);
        let c = Colors::get(cx);
        let drag = DraggedTab { id, label: t.label.clone(), icon: self.process_glyph(t.process.as_deref(), c, cx) };
        let renaming = self.rename.as_ref().is_some_and(|r| r.tab == id);
        div()
            .id(gpui::ElementId::NamedInteger("tab".into(), id))
            .group(SharedString::from(format!("tab-{id}")))
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .cursor_pointer()
            .when(t.dragged, |d| d.opacity(0.45))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    // The active tab: leave focus alone (it may be in the rename field).
                    if this.active != index {
                        this.activate(index, window, cx);
                    }
                }),
            )
            .on_mouse_up(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_tab_at(index, window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                    this.open_tab_menu(id, ev.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                if ev.click_count() == 2 {
                    this.start_rename(index, window, cx);
                }
            }))
            .on_scroll(cx.listener(Self::on_tabs_scroll))
            // Dragging in the rename field selects text instead.
            .when(!renaming, |d| d.on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone())))
            .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<DraggedTab>, window, cx| {
                // Every tab hears every move: only the one under the pointer
                // acts, and only on bounds from after the last move.
                if this.moved_unpainted || !ev.bounds.contains(&ev.event.position) {
                    return;
                }
                let dragged = ev.drag(cx).id;
                if this.dragging != Some(dragged) {
                    this.dragging = Some(dragged);
                    cx.notify();
                }
                let (Some(from), Some(over)) = (this.tab_index(dragged), this.tab_index(id)) else { return };
                let (pos, start, end) = match axis {
                    Axis::Row => (ev.event.position.x, ev.bounds.left(), ev.bounds.right()),
                    Axis::Column => (ev.event.position.y, ev.bounds.top(), ev.bounds.bottom()),
                };
                if let Some(to) = reorder_target(from, over, f32::from(pos), f32::from(start), f32::from(end)) {
                    this.move_tab_to(dragged, to, window, cx);
                    this.moved_unpainted = true;
                }
            }))
    }

    /// The tab's title, or the rename field while renaming it.
    pub(super) fn tab_title(&self, t: &TabView, cx: &mut Context<'_, Self>) -> AnyElement {
        match self.rename.as_ref().filter(|r| r.tab == t.id) {
            Some(r) => {
                div().flex_1().min_w(px(80.)).on_key_down(cx.listener(Self::on_rename_key)).child(r.input.clone()).into_any_element()
            }
            None => div().flex_1().min_w_0().truncate().child(t.label.clone()).into_any_element(),
        }
    }

    /// The close button, `size` px: always on the active tab, on hover on
    /// the others.
    pub(super) fn close_button(&self, t: &TabView, size: f32, c: Colors, cx: &mut Context<'_, Self>) -> AnyElement {
        let index = t.index;
        let group = SharedString::from(format!("tab-{}", t.id));
        div()
            .id(gpui::ElementId::NamedInteger("tab-close".into(), t.id))
            .flex_none()
            .size(px(size))
            .rounded(px(radius::SMALL))
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.bg(c.ui.text.opacity(0.1)))
            .tooltip(tooltip("Close tab"))
            .child(Icon::Close.svg().size(px(size * 0.66)).text_color(c.ui.muted))
            .when(!t.active, |d| d.invisible().group_hover(group, |s| s.visible()))
            .on_click(cx.listener(move |this, _, window, cx| this.close_tab_at(index, window, cx)))
            .into_any_element()
    }

    fn modern_tab(&self, t: TabView, width: f32, c: Colors, cx: &mut Context<'_, Self>) -> AnyElement {
        let icon = self.tab_icon(&t, 14.0, c, cx, || Icon::Terminal.svg().size(px(14.)).text_color(c.ui.muted).into_any_element());
        // Narrow background tabs leave the room to the title (middle-click
        // or the menu still close them).
        let close = (t.active || width >= NARROW_TAB).then(|| self.close_button(&t, 18.0, c, cx));
        let title = self.tab_title(&t, cx);
        let marker = self.activity_marker(&t, 13.0, c);
        self.tab_shell(&t, Axis::Row, cx)
            .flex_none()
            .w(px(width))
            .h(px(MODERN_TAB_HEIGHT))
            .pl(px(10.))
            .pr(px(6.))
            .gap(px(if width < NARROW_TAB { 6. } else { 8. }))
            .rounded_t(px(8.))
            .text_sm()
            .when(t.active, |d| d.bg(c.terminal).text_color(c.ui.text))
            .when(!t.active, |d| d.text_color(c.ui.muted).hover(|s| s.bg(c.hover).text_color(c.ui.text)))
            .child(icon)
            .child(title)
            .children(marker)
            .when(t.panes > 1, |d| {
                d.child(div().flex_none().text_xs().text_color(c.ui.muted).child(SharedString::from(format!("{}", t.panes))))
            })
            .children(close)
            .into_any_element()
    }

    fn compact_tab(&self, t: TabView, c: Colors, cx: &mut Context<'_, Self>) -> AnyElement {
        let mono = cx.global::<AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into());
        let fg = if t.active { c.ui.on_accent } else { c.ui.muted };
        let icon = self.process_icon(t.process.as_deref(), 12.0, t.active.then_some(fg), c, cx);
        let title = self.tab_title(&t, cx);
        let marker = self.activity_marker(&t, 11.0, c);
        let number = SharedString::from(format!("{}", t.index + 1));
        self.tab_shell(&t, Axis::Row, cx)
            .flex_none()
            .max_w(px(220.))
            .px(px(10.))
            .gap(px(6.))
            .font_family(mono)
            .text_color(fg)
            .when(t.active, |d| d.bg(c.ui.accent))
            .when(!t.active, |d| d.hover(|s| s.bg(c.hover).text_color(c.ui.text)))
            .child(div().flex_none().opacity(0.7).child(number))
            .children(icon)
            .child(title)
            .children(marker)
            .when(t.panes > 1, |d| d.child(div().flex_none().opacity(0.7).child(SharedString::from(format!("[{}]", t.panes)))))
            .into_any_element()
    }
}

/// What an activity marker says and its color.
pub(super) fn activity_look(activity: Activity, c: Colors) -> (&'static str, Hsla) {
    match activity {
        Activity::Bell => ("Rang the bell", c.ui.accent),
        Activity::Failed => ("A command failed", c.ui.danger),
        Activity::Finished => ("A command finished", c.ui.success),
        Activity::Output => ("New output", c.ui.muted),
    }
}

/// A modern tab's width in a window `window` px wide with `tabs` tabs:
/// they share the strip (less the + button and some empty space to
/// double-click), between [`MIN_TAB`] and [`MAX_TAB`]; past that the strip
/// scrolls.
fn modern_tab_width(window: f32, tabs: usize) -> f32 {
    const RESERVED: f32 = 12.0 + 30.0 + 48.0;
    let gaps = 2.0 * tabs.saturating_sub(1) as f32;
    ((window - RESERVED - gaps) / tabs.max(1) as f32).clamp(MIN_TAB, MAX_TAB)
}

#[cfg(test)]
mod tests {
    #[test]
    fn modern_tabs_share_the_strip() {
        use super::modern_tab_width as w;
        assert_eq!(w(1200.0, 1), 220.0);
        assert_eq!(w(1200.0, 0), 220.0);
        // 1200 - 90 reserved - 18 gaps = 1092 for 10 tabs.
        assert!((w(1200.0, 10) - 109.2).abs() < 0.01);
        assert_eq!(w(1200.0, 40), super::MIN_TAB, "then the strip scrolls");
    }
}
