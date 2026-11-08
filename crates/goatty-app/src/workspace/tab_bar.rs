//! The tab strip, drawn by goatty (not the window manager), in two styles
//! (`tabs.style`):
//!
//! - **modern**: browser-like tabs with rounded tops; the active one has the
//!   terminal's background and merges into it. Icon, title, close button
//!   (always on the active tab, on hover for the others), a + button.
//! - **compact**: one line of small monospace text, kitty-style: number,
//!   icon and title; the active tab in the accent color.
//!
//! Both: click to switch, middle-click to close, double-click to rename,
//! drag to reorder, double-click the empty space for a new tab.

use goatty_config::TabStyle;
use gpui::{
    AnyElement, App, Context, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::*, px, rgb,
};

use super::icons::IconColor;
use super::{DraggedTab, Tab, Workspace};
use crate::actions::NewTab;
use crate::settings::AppSettings;
use crate::ui::icons::Icon;
use crate::ui::style::Ui;

const MODERN_HEIGHT: f32 = 38.0;
const MODERN_TAB_HEIGHT: f32 = 32.0;
const COMPACT_HEIGHT: f32 = 24.0;

/// Colors of the strip, from the theme.
#[derive(Clone, Copy)]
struct Colors {
    ui: Ui,
    /// The strip behind the tabs.
    bar: Hsla,
    /// The terminal's background (the active modern tab).
    terminal: Hsla,
    hover: Hsla,
}

impl Colors {
    fn get(cx: &App) -> Self {
        let ui = Ui::get(cx);
        let colors = &cx.global::<AppSettings>().colors;
        let terminal: Hsla = rgb(colors.background.0).into();
        // A bit darker than the terminal in dark themes, a bit greyer in light ones.
        let bar =
            if colors.is_dark() { Hsla { l: terminal.l * 0.7, ..terminal } } else { Hsla { l: (terminal.l - 0.06).max(0.0), ..terminal } };
        let hover = if colors.is_dark() { Hsla { l: (bar.l + 0.06).min(1.0), ..bar } } else { Hsla { l: (bar.l - 0.05).max(0.0), ..bar } };
        Self { ui, bar, terminal, hover }
    }

    fn icon(&self, color: IconColor, cx: &App) -> Hsla {
        match color {
            IconColor::Text => self.ui.text,
            IconColor::Ansi(i) => rgb(cx.global::<AppSettings>().colors.ansi[i as usize % 16].0).into(),
        }
    }
}

impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let c = Colors::get(cx);
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(c.terminal)
            .border_1()
            .border_color(c.ui.accent)
            .text_sm()
            .text_color(c.ui.text)
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .child(self.label.clone())
    }
}

/// What one tab shows.
struct TabView {
    index: usize,
    id: u64,
    label: SharedString,
    active: bool,
    process: Option<String>,
    panes: usize,
}

impl Workspace {
    pub(super) fn render_tab_bar(&self, _window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
        let tabs: Vec<TabView> = self.tabs.iter().enumerate().map(|(i, t)| self.tab_view(i, t, cx)).collect();
        let c = Colors::get(cx);
        let style = cx.global::<AppSettings>().client.tabs.style;
        let (height, items) = match style {
            TabStyle::Modern => (MODERN_HEIGHT, tabs.into_iter().map(|t| self.modern_tab(t, c, cx)).collect::<Vec<_>>()),
            TabStyle::Compact => (COMPACT_HEIGHT, tabs.into_iter().map(|t| self.compact_tab(t, c, cx)).collect()),
        };
        let last = self.tabs.len().saturating_sub(1);
        let new_tab = div()
            .id("tab-new")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(c.ui.muted)
            .hover(|s| s.bg(c.hover).text_color(c.ui.text))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTab), cx));
        let new_tab = match style {
            TabStyle::Modern => new_tab.size(px(28.)).mb(px(2.)).rounded_md().child(Icon::Plus.svg().size(px(15.)).text_color(c.ui.muted)),
            TabStyle::Compact => new_tab.h_full().px(px(10.)).child("+"),
        };
        div()
            .id("tab-bar")
            .flex_none()
            .h(px(height))
            .w_full()
            .flex()
            .bg(c.bar)
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .when(style == TabStyle::Modern, |d| d.items_end().px(px(6.)).gap(px(2.)))
            .when(style == TabStyle::Compact, |d| d.items_stretch().text_size(px(12.)))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .flex_shrink(1.)
                    .h_full()
                    .when(style == TabStyle::Modern, |d| d.items_end().gap(px(2.)))
                    .children(items),
            )
            .child(new_tab)
            .child(
                // The empty rest of the strip: a drop target for "move to the
                // end", double-click for a new tab.
                div()
                    .id("tab-bar-rest")
                    .flex_1()
                    .h_full()
                    .on_click(|ev, window, cx| {
                        if ev.click_count() == 2 {
                            window.dispatch_action(Box::new(NewTab), cx);
                        }
                    })
                    .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| this.move_tab_to(d.id, last, window, cx))),
            )
            .into_any_element()
    }

    fn tab_view(&self, index: usize, tab: &Tab, cx: &App) -> TabView {
        let process = tab.focused_view().and_then(|v| v.read(cx).process().map(str::to_string));
        TabView { index, id: tab.id, label: self.tab_label(tab, cx).into(), active: index == self.active, process, panes: tab.panes.len() }
    }

    /// The program icon in `size` px, or `None` (no Nerd Font, unknown
    /// program, icons off). `on_accent`: drawn on the accent color.
    fn process_icon(&self, process: Option<&str>, size: f32, color: Option<Hsla>, c: Colors, cx: &App) -> Option<AnyElement> {
        if !cx.global::<AppSettings>().client.tabs.icons {
            return None;
        }
        let icon = self.nerd.icon(process?)?;
        let font = self.nerd.font.clone()?;
        Some(
            div()
                .flex_none()
                .w(px(size + 2.))
                .flex()
                .justify_center()
                .font(font)
                .text_size(px(size))
                .line_height(px(size + 2.))
                .text_color(color.unwrap_or_else(|| c.icon(icon.color, cx)))
                .child(SharedString::from(icon.glyph.to_string()))
                .into_any_element(),
        )
    }

    /// Shared behaviour of both styles: switch, close, rename, reorder.
    fn tab_shell(&self, t: &TabView, cx: &mut Context<'_, Self>) -> gpui::Stateful<gpui::Div> {
        let (index, id) = (t.index, t.id);
        let drag = DraggedTab { id, label: t.label.clone() };
        let accent = Ui::get(cx).accent;
        let renaming = self.rename.as_ref().is_some_and(|r| r.tab == id);
        div()
            .id(gpui::ElementId::NamedInteger("tab".into(), id))
            .group(SharedString::from(format!("tab-{id}")))
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .cursor_pointer()
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
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                if ev.click_count() == 2 {
                    this.start_rename(index, window, cx);
                }
            }))
            // Dragging in the rename field selects text instead.
            .when(!renaming, |d| d.on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone())))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.border_l_2().border_color(accent))
            .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| this.move_tab_to(d.id, index, window, cx)))
    }

    /// The tab's title, or the rename field while renaming it.
    fn tab_title(&self, t: &TabView, cx: &mut Context<'_, Self>) -> AnyElement {
        match self.rename.as_ref().filter(|r| r.tab == t.id) {
            Some(r) => {
                div().flex_1().min_w(px(80.)).on_key_down(cx.listener(Self::on_rename_key)).child(r.input.clone()).into_any_element()
            }
            None => div().flex_1().min_w_0().truncate().child(t.label.clone()).into_any_element(),
        }
    }

    fn modern_tab(&self, t: TabView, c: Colors, cx: &mut Context<'_, Self>) -> AnyElement {
        let index = t.index;
        let group = SharedString::from(format!("tab-{}", t.id));
        let icon = self
            .process_icon(t.process.as_deref(), 14.0, None, c, cx)
            .unwrap_or_else(|| Icon::Terminal.svg().size(px(14.)).text_color(c.ui.muted).into_any_element());
        let close = div()
            .id(gpui::ElementId::NamedInteger("tab-close".into(), t.id))
            .flex_none()
            .size(px(18.))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.bg(c.hover))
            .child(Icon::Close.svg().size(px(12.)).text_color(c.ui.muted))
            .when(!t.active, |d| d.invisible().group_hover(group, |s| s.visible()))
            .on_click(cx.listener(move |this, _, window, cx| this.close_tab_at(index, window, cx)));
        let title = self.tab_title(&t, cx);
        self.tab_shell(&t, cx)
            .flex_basis(px(220.))
            .flex_shrink(1.)
            .min_w(px(72.))
            .max_w(px(240.))
            .h(px(MODERN_TAB_HEIGHT))
            .pl(px(10.))
            .pr(px(6.))
            .gap(px(8.))
            .rounded_t(px(8.))
            .text_sm()
            .when(t.active, |d| d.bg(c.terminal).text_color(c.ui.text))
            .when(!t.active, |d| d.text_color(c.ui.muted).hover(|s| s.bg(c.hover).text_color(c.ui.text)))
            .child(icon)
            .child(title)
            .when(t.panes > 1, |d| {
                d.child(div().flex_none().text_xs().text_color(c.ui.muted).child(SharedString::from(format!("{}", t.panes))))
            })
            .child(close)
            .into_any_element()
    }

    fn compact_tab(&self, t: TabView, c: Colors, cx: &mut Context<'_, Self>) -> AnyElement {
        let mono = cx.global::<AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into());
        let fg = if t.active { c.ui.on_accent } else { c.ui.muted };
        let icon = self.process_icon(t.process.as_deref(), 12.0, t.active.then_some(fg), c, cx);
        let title = self.tab_title(&t, cx);
        let number = SharedString::from(format!("{}", t.index + 1));
        self.tab_shell(&t, cx)
            .flex_shrink(1.)
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
            .when(t.panes > 1, |d| d.child(div().flex_none().opacity(0.7).child(SharedString::from(format!("[{}]", t.panes)))))
            .into_any_element()
    }
}
