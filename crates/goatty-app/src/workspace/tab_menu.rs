//! A tab's right-click menu: rename, duplicate, close it, the others, or
//! the ones after it. Up/Down (or hovering) pick an item, Enter or a click
//! runs it, Esc or a click elsewhere closes the menu and gives focus back
//! to the terminal.

use goatty_config::TabStyle;
use gpui::{
    AnyElement, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Point, Role,
    StatefulInteractiveElement, Styled, Subscription, Window, anchored, deferred, div, prelude::*, px,
};

use super::Workspace;
use crate::settings::AppSettings;
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};

/// An open menu.
pub(super) struct TabMenu {
    /// The tab it's for, by id.
    pub tab: u64,
    /// Where it opened (window coordinates).
    at: Point<Pixels>,
    focus: FocusHandle,
    /// The highlighted item.
    selected: Option<Item>,
    _blur: Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Rename,
    Duplicate,
    Close,
    CloseOthers,
    CloseAfter,
}

const ITEMS: [Item; 5] = [Item::Rename, Item::Duplicate, Item::Close, Item::CloseOthers, Item::CloseAfter];

impl Item {
    fn icon(self) -> Option<Icon> {
        match self {
            Item::Rename => Some(Icon::Pencil),
            Item::Duplicate => Some(Icon::Copy),
            Item::Close => Some(Icon::Close),
            Item::CloseOthers | Item::CloseAfter => None,
        }
    }

    /// Whether a separator goes above it.
    fn starts_group(self) -> bool {
        self == Item::Close
    }
}

impl Workspace {
    pub(super) fn open_tab_menu(&mut self, tab: u64, at: Point<Pixels>, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.finish_rename(true, window, cx);
        let focus = cx.focus_handle();
        // Focus going anywhere else (a click outside, another window) closes it.
        let blur = cx.on_blur(&focus, window, |this, _, cx| {
            this.tab_menu = None;
            cx.notify();
        });
        focus.focus(window, cx);
        self.tab_menu = Some(TabMenu { tab, at, focus, selected: None, _blur: blur });
        cx.notify();
    }

    /// Close the menu and give focus back to the terminal.
    fn close_tab_menu(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.tab_menu.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn item_label(&self, item: Item, cx: &gpui::App) -> &'static str {
        let vertical = cx.global::<AppSettings>().client.tabs.style == TabStyle::Vertical;
        match item {
            Item::Rename => "Rename…",
            Item::Duplicate => "Duplicate",
            Item::Close => "Close tab",
            Item::CloseOthers => "Close other tabs",
            Item::CloseAfter if vertical => "Close tabs below",
            Item::CloseAfter => "Close tabs to the right",
        }
    }

    /// Whether `item` does anything for the tab at `index`.
    fn item_enabled(&self, item: Item, index: usize) -> bool {
        match item {
            Item::CloseOthers => self.tabs.len() > 1,
            Item::CloseAfter => index + 1 < self.tabs.len(),
            _ => true,
        }
    }

    fn run_tab_menu(&mut self, item: Item, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(index) = self.tab_menu.as_ref().and_then(|m| self.tab_index(m.tab)) else { return };
        if !self.item_enabled(item, index) {
            return;
        }
        self.close_tab_menu(window, cx);
        match item {
            Item::Rename => self.start_rename(index, window, cx),
            Item::Duplicate => self.duplicate_tab_at(index, window, cx),
            Item::Close => self.close_tab_at(index, window, cx),
            Item::CloseOthers => self.close_tabs(self.other_tabs(index), window, cx),
            Item::CloseAfter => {
                let after = self.tabs[index + 1..].iter().map(|t| t.id).collect();
                self.close_tabs(after, window, cx);
            }
        }
    }

    fn on_tab_menu_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(index) = self.tab_menu.as_ref().and_then(|m| self.tab_index(m.tab)) else { return };
        let enabled: Vec<Item> = ITEMS.into_iter().filter(|&i| self.item_enabled(i, index)).collect();
        let Some(menu) = self.tab_menu.as_mut() else { return };
        let pos = menu.selected.and_then(|s| enabled.iter().position(|&i| i == s));
        match ev.keystroke.key.as_str() {
            "escape" => self.close_tab_menu(window, cx),
            "down" | "tab" => menu.selected = Some(enabled[pos.map_or(0, |p| (p + 1) % enabled.len())]),
            "up" => menu.selected = Some(enabled[pos.map_or(enabled.len() - 1, |p| (p + enabled.len() - 1) % enabled.len())]),
            "home" => menu.selected = enabled.first().copied(),
            "end" => menu.selected = enabled.last().copied(),
            "enter" | "space" => {
                if let Some(item) = menu.selected {
                    self.run_tab_menu(item, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn render_tab_menu(&self, _window: &mut Window, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let menu = self.tab_menu.as_ref()?;
        let index = self.tab_index(menu.tab)?;
        let ui = Ui::get(cx);
        let mut list = div()
            .id("tab-menu")
            .role(Role::Menu)
            .track_focus(&menu.focus)
            .on_key_down(cx.listener(Self::on_tab_menu_key))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_tab_menu(window, cx)))
            .occlude()
            .w(px(220.))
            .p(px(4.))
            .flex()
            .flex_col()
            .bg(ui.menu)
            .border_1()
            .border_color(ui.border)
            .rounded(px(radius::CONTROL))
            .shadow_lg()
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .text_size(px(text::LABEL));
        for item in ITEMS {
            let enabled = self.item_enabled(item, index);
            if item.starts_group() {
                list = list.child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(ui.divider));
            }
            let selected = menu.selected == Some(item);
            let fg = if enabled { ui.text } else { ui.faint };
            let icon_color = match (enabled, selected) {
                (false, _) => ui.faint,
                (true, true) => ui.accent_text,
                (true, false) => ui.muted,
            };
            let icon = match item.icon() {
                Some(i) => i.svg().size(px(14.)).text_color(icon_color).into_any_element(),
                None => div().size(px(14.)).into_any_element(),
            };
            list = list.child(
                div()
                    .id(("tab-menu-item", item as usize))
                    .role(Role::MenuItem)
                    .aria_label(self.item_label(item, cx))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(30.))
                    .px(px(10.))
                    .rounded(px(radius::SMALL))
                    .text_color(fg)
                    .when(selected && enabled, |d| d.bg(ui.accent_soft).text_color(ui.accent_text))
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if let Some(m) = this.tab_menu.as_mut()
                                    && *hovered
                                    && m.selected != Some(item)
                                {
                                    m.selected = Some(item);
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| this.run_tab_menu(item, window, cx)))
                    })
                    .child(icon)
                    .child(self.item_label(item, cx)),
            );
        }
        Some(deferred(anchored().position(menu.at).snap_to_window_with_margin(px(8.)).child(list)).with_priority(2).into_any_element())
    }
}
