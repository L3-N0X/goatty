//! The root view of a window. Owns the terminal panes and everything
//! window-level: the title, closing when the shell exits, the command
//! palette overlay, and the window's toasts (`crate::toast`).
//!
//! Today it holds exactly one pane. Tabs and splits (#30) grow here: a tab
//! strip plus a split tree whose leaves are `TerminalView`s, with the window
//! title following the focused pane.

use std::sync::Arc;

use gpui::{AppContext, Context, Entity, Focusable, IntoElement, ParentElement, Render, Styled, Subscription, Window, div, prelude::*, px};

use crate::actions::ToggleCommandPalette;
use crate::command_palette::{CommandPalette, PaletteEvent};
use crate::session::Session;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::toast::ToastLayer;

pub struct Workspace {
    pane: Entity<TerminalView>,
    palette: Option<(Entity<CommandPalette>, Subscription)>,
    toasts: Entity<ToastLayer>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(session: Arc<Session>, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let pane = cx.new(|cx| TerminalView::new(session, window, cx));
        let sub = cx.subscribe_in(&pane, window, Self::on_terminal_event);
        // The desktop's light/dark switch (themes can follow it).
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            crate::settings::AppSettings::set_system_dark(cx, crate::settings::is_dark(window.appearance()));
        });
        let toasts = cx.new(|cx| ToastLayer::new(window, cx));
        Self { pane, palette: None, toasts, _subscriptions: vec![sub, appearance] }
    }

    fn on_terminal_event(&mut self, pane: &Entity<TerminalView>, ev: &TerminalEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        match ev {
            TerminalEvent::TitleChanged => window.set_window_title(&pane.read(cx).title()),
            TerminalEvent::Exited => {
                log::info!("session ended");
                // The last window closing quits the app (see `app::run`).
                window.remove_window();
            }
        }
    }

    fn toggle_palette(&mut self, _: &ToggleCommandPalette, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        let palette = cx.new(|cx| CommandPalette::new(window, cx));
        let sub = cx.subscribe_in(&palette, window, |this, _, ev: &PaletteEvent, window, cx| match ev {
            PaletteEvent::Dismissed => this.close_palette(window, cx),
            PaletteEvent::Run(command) => {
                let command = command.clone();
                // Focus the terminal first: actions go to the focused element.
                this.close_palette(window, cx);
                command.run(window, cx);
            }
        });
        self.palette = Some((palette, sub));
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.palette.take().is_some() {
            self.pane.focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .on_action(cx.listener(Self::toggle_palette))
            .child(self.pane.clone())
            .child(self.toasts.clone())
            .when_some(self.palette.as_ref(), |d, (palette, _)| {
                d.child(div().absolute().top(px(48.)).left_0().right_0().px_4().flex().justify_center().child(palette.clone()))
            })
    }
}
