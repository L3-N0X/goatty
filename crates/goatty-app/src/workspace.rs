//! The root view of a window. Owns the terminal panes and everything
//! window-level: the title, closing when the shell exits.
//!
//! Today it holds exactly one pane. Tabs and splits (#30) grow here: a tab
//! strip plus a split tree whose leaves are `TerminalView`s, with the window
//! title following the focused pane.

use std::sync::Arc;

use gpui::{AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window, div};

use crate::session::Session;
use crate::terminal::{TerminalEvent, TerminalView};

pub struct Workspace {
    pane: Entity<TerminalView>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(session: Arc<Session>, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let pane = cx.new(|cx| TerminalView::new(session, window, cx));
        let sub = cx.subscribe_in(&pane, window, Self::on_terminal_event);
        Self { pane, _subscriptions: vec![sub] }
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
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full().child(self.pane.clone())
    }
}
