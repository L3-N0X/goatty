//! The root view of a window: tabs, each holding a split layout of
//! terminal panes, plus everything window-level: the title (the focused
//! pane's), closing when the last shell exits, the command palette overlay,
//! paste and close confirmations (`crate::paste_confirm`,
//! `crate::close_confirm`) and the window's toasts (`crate::toast`).
//!
//! - `layout`: the split tree of a tab (pure, unit-tested).
//! - `tab_bar`: the tab strip in its two styles (`tabs.style`).
//! - `icons`: Nerd Font icons for the program running in a tab.
//! - `saved`: the layout goattyd keeps for the window, so the next
//!   `goatty` restores every tab and split (#118).
//!
//! Each tab is on one host, and a window can hold tabs on several (a local
//! shell next to a few servers). A tab remembers its [`Spawner`]: new
//! panes and new tabs from it are new sessions there (another `create` on
//! the same goattyd connection, `ssh host` again for plain SSH, another
//! local PTY), started in the focused pane's directory. Closing a pane
//! ends its shell (after asking, when a program runs in it); closing the
//! window only detaches (the daemons keep the sessions).
//!
//! A tab whose session isn't there yet shows the connect screen
//! (`crate::connect`, #37): the window opens at once, and the tab gets its
//! panes when the connection is made (`open_connect_tab`).
//!
//! Each goattyd the window has tabs on keeps the layout of those tabs, so
//! `goatty` (or `goatty --ssh host`) brings them back. While a window is
//! active, the settings window and "reload shells" act on its active tab's
//! host (`settings_window::set_host_link`).

mod icons;
mod layout;
pub mod saved;
mod tab_bar;

use std::path::Path;
use std::sync::Arc;

use goatty_client::Connection;
use goatty_config::{TabActivity, TabBarShow, TabSettings};
use goatty_proto::LayoutId;
use goatty_term::TermSize;
use gpui::{
    AppContext, Context, DragMoveEvent, Entity, Focusable, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, Styled,
    Subscription, Task, Window, div, prelude::*, px, relative,
};

use crate::actions::*;
use crate::app::Initial;
use crate::close_confirm::{CloseConfirm, CloseConfirmEvent, Closing};
use crate::command_palette::{CommandPalette, PaletteEvent, Scope};
use crate::connect::{ConnectEvent, ConnectPlan, ConnectView};
use crate::paste_confirm::{PasteConfirm, PasteConfirmEvent};
use crate::session::AttachMode;
use crate::session::{Session, Spawner};
use crate::settings::AppSettings;
use crate::terminal::{Activity, PasteRequest, TerminalEvent, TerminalView};
use crate::toast::{self, Toast, ToastLayer};
use crate::ui::style::Ui;
use crate::ui::text_input::{TextInput, TextInputEvent};
use icons::NerdFont;
use layout::{Axis, Direction, Node, PaneId, Rect};
use saved::{SavedTab, SavedWindow, SavedWorkspace, Startup};

/// Width of the line between panes.
const DIVIDER: f32 = 1.0;
/// The grab area around it.
const DIVIDER_GRAB: f32 = 6.0;
/// How far the resize actions move a divider (share of the split).
const RESIZE_STEP: f32 = 0.05;

struct Pane {
    id: PaneId,
    view: Entity<TerminalView>,
    _subscriptions: [Subscription; 2],
}

struct Tab {
    id: u64,
    tree: Node,
    panes: Vec<Pane>,
    focused: PaneId,
    /// The focused pane fills the tab.
    zoomed: bool,
    /// Set by renaming; otherwise the focused pane's title.
    name: Option<String>,
    /// Where its sessions live; `None` while connecting.
    host: Option<Spawner>,
    /// The connect screen, until the tab's session is there (its tree is
    /// then a single placeholder leaf without a pane).
    connecting: Option<(Entity<ConnectView>, Subscription)>,
    /// The most important thing that happened in its panes while it was in
    /// the background (#122); cleared when it's shown.
    activity: Option<Activity>,
}

/// Do the tab settings show this kind of activity?
fn marks(tabs: &TabSettings, activity: Activity) -> bool {
    match activity {
        Activity::Bell => tabs.mark_bell,
        Activity::Finished | Activity::Failed => tabs.mark_activity != TabActivity::Off,
        Activity::Output => tabs.mark_activity == TabActivity::Output,
    }
}

impl Tab {
    fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    fn focused_view(&self) -> Option<&Entity<TerminalView>> {
        self.pane(self.focused).map(|p| &p.view)
    }
}

/// Where a new pane goes.
#[derive(Debug, Clone, Copy)]
enum Placement {
    NewTab,
    /// Next to pane `of` in tab `tab`, on the `dir` side.
    Split {
        tab: u64,
        of: PaneId,
        dir: Direction,
    },
}

/// What a close confirmation is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseTarget {
    Pane(PaneId),
    /// By tab id.
    Tab(u64),
}

/// A tab being renamed in place.
struct Rename {
    tab: u64,
    input: Entity<TextInput>,
    _subscriptions: [Subscription; 2],
}

/// Dragging a divider (a GPUI drag; see `on_drag_move`).
#[derive(Debug, Clone)]
struct DividerDrag {
    tab: u64,
    path: Vec<usize>,
    index: usize,
    axis: Axis,
    split: Rect,
}

/// Dragging a tab to reorder it.
#[derive(Clone)]
pub(super) struct DraggedTab {
    id: u64,
    label: gpui::SharedString,
}

/// The layout one goattyd keeps for this window's tabs on it.
struct HostLayout {
    conn: Arc<Connection>,
    id: LayoutId,
    /// What was last sent.
    json: String,
}

/// This window's tabs on one goattyd, as `save_layout` collects them.
struct LayoutGroup {
    conn: Arc<Connection>,
    sessions: Vec<goatty_proto::SessionId>,
    tabs: Vec<SavedTab>,
    /// How many of them come before the active tab (the active one's index
    /// when it's among them).
    before_active: usize,
}

pub struct Workspace {
    /// Where new tabs go when the active tab has no host yet: the first
    /// host that connected.
    home: Option<Spawner>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    nerd: NerdFont,
    rename: Option<Rename>,
    palette: Option<(Entity<CommandPalette>, Subscription)>,
    paste: Option<(Entity<PasteConfirm>, PaneId, Subscription)>,
    close: Option<(Entity<CloseConfirm>, CloseTarget, Subscription)>,
    toasts: Entity<ToastLayer>,
    layouts: Vec<HostLayout>,
    /// The window's size and state, saved with the layouts.
    window: SavedWindow,
    /// A layout save waiting for the window to stop resizing.
    save_window: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// How long the window's size has to settle before it's saved (a resize
/// drag changes it every frame).
const SAVE_WINDOW_AFTER: std::time::Duration = std::time::Duration::from_millis(500);

impl Workspace {
    pub fn new(initial: Initial, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        // The desktop's light/dark switch (themes can follow it).
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            crate::settings::AppSettings::set_system_dark(cx, crate::settings::is_dark(window.appearance()));
        });
        // The terminal font decides which icons can show.
        // `terminal.opacity` / `terminal.blur`, live.
        let mut background = background_appearance(&cx.global::<AppSettings>().client.terminal);
        window.set_background_appearance(background);
        let settings = cx.observe_global_in::<AppSettings>(window, move |this, window, cx| {
            let wanted = background_appearance(&cx.global::<AppSettings>().client.terminal);
            if wanted != background {
                background = wanted;
                window.set_background_appearance(wanted);
            }
            this.refresh_icon_font(cx);
            cx.notify();
        });
        // Settings and "reload shells" act on the active window's host, and
        // hosts opened in a tab go to the active window.
        let activation = cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active() {
                this.window_activated(window, cx);
            }
        });
        let bounds = cx.observe_window_bounds(window, |this: &mut Self, window, cx| this.window_bounds_changed(window, cx));
        cx.set_global(crate::hosts::LastWorkspace(window.window_handle()));
        let toasts = cx.new(|cx| ToastLayer::new(window, cx));
        let mut this = Self {
            home: None,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            nerd: NerdFont::default(),
            rename: None,
            palette: None,
            paste: None,
            close: None,
            toasts,
            layouts: Vec::new(),
            window: SavedWindow::from_bounds(window.window_bounds()),
            save_window: None,
            _subscriptions: vec![appearance, settings, activation, bounds],
        };
        this.refresh_icon_font(cx);
        match initial {
            Initial::Ready { startup, spawner } => {
                this.home = Some(spawner.clone());
                crate::shell_setup::offer(spawner.host_link(), cx);
                match startup {
                    Startup::Session(session) => this.insert_pane(session, Placement::NewTab, spawner, window, cx),
                    Startup::Restore(r) => {
                        if let Some(w) = r.window {
                            w.apply(window);
                            this.window = w;
                        }
                        let active = this.insert_restored(r, &spawner, 0, window, cx);
                        this.activate(active, window, cx);
                    }
                }
            }
            Initial::Connect(plan) => this.open_connect_tab(plan, window, cx),
        }
        this
    }

    /// The window was resized, maximized or went fullscreen: save its size
    /// with the layout once it settles.
    fn window_bounds_changed(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let now = SavedWindow::capture(window, self.window);
        if now == self.window {
            return;
        }
        self.window = now;
        self.save_window = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_WINDOW_AFTER).await;
            this.update(cx, |this, cx| this.save_layout(cx)).ok();
        }));
    }

    fn window_activated(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        cx.set_global(crate::hosts::LastWorkspace(window.window_handle()));
        if let Some(host) = self.active_tab().and_then(|t| t.host.as_ref()) {
            crate::settings_window::set_host_link(cx, Some(host.host_link()));
        }
    }

    /// The layout entry for `conn` (a new one for a host the window had no tabs on).
    fn layout_for(&mut self, conn: &Arc<Connection>) -> &mut HostLayout {
        let i = match self.layouts.iter().position(|l| Arc::ptr_eq(&l.conn, conn)) {
            Some(i) => i,
            None => {
                self.layouts.push(HostLayout { conn: conn.clone(), id: saved::new_id(), json: String::new() });
                self.layouts.len() - 1
            }
        };
        &mut self.layouts[i]
    }

    /// Rebuild the tabs of a saved layout from its attached sessions,
    /// inserted at `at`. Returns the index of its active tab.
    fn insert_restored(
        &mut self,
        r: saved::Restored,
        spawner: &Spawner,
        at: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> usize {
        if let Some(conn) = spawner.connection() {
            self.layout_for(conn).id = r.id;
        }
        let mut at = at.min(self.tabs.len());
        let first = at;
        let mut sessions = r.sessions;
        for t in r.tabs {
            let mut panes = Vec::new();
            let mut pane_of = std::collections::HashMap::new();
            let tree = t.tree.map_leaves(&mut |session_id| {
                let session = sessions.remove(&session_id)?;
                let pane = self.make_pane(session, window, cx);
                pane_of.insert(session_id, pane.id);
                let id = pane.id;
                panes.push(pane);
                Some(id)
            });
            let Some(tree) = tree else { continue };
            let focused = pane_of.get(&t.focused).copied().unwrap_or(panes[0].id);
            let id = self.next_id();
            let tab = Tab {
                id,
                tree,
                panes,
                focused,
                zoomed: t.zoomed,
                name: t.name,
                host: Some(spawner.clone()),
                connecting: None,
                activity: None,
            };
            self.tabs.insert(at, tab);
            at += 1;
        }
        (first + r.active).min(at.saturating_sub(1))
    }

    /// Tell each goattyd what this window's tabs on it look like now (sent
    /// only when it changed; local PTYs have nothing to save). A goattyd
    /// the window has no tabs on any more hears that the layout is gone.
    fn save_layout(&mut self, cx: &mut Context<'_, Self>) {
        let mut groups: Vec<LayoutGroup> = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            let Some(conn) = t.host.as_ref().and_then(Spawner::connection) else { continue };
            let session_of = |id: PaneId| t.pane(id).and_then(|p| p.view.read(cx).session().daemon_id());
            let Some(tree) = t.tree.map_leaves(&mut |id| session_of(id)) else { continue };
            let g = match groups.iter().position(|g| Arc::ptr_eq(&g.conn, conn)) {
                Some(g) => g,
                None => {
                    groups.push(LayoutGroup { conn: conn.clone(), sessions: Vec::new(), tabs: Vec::new(), before_active: 0 });
                    groups.len() - 1
                }
            };
            let g = &mut groups[g];
            g.sessions.extend(tree.leaves());
            let focused = session_of(t.focused).unwrap_or_else(|| tree.leaves()[0]);
            g.tabs.push(SavedTab { name: t.name.clone(), tree, focused, zoomed: t.zoomed });
            if i < self.active {
                g.before_active += 1;
            }
        }
        for g in &groups {
            self.layout_for(&g.conn);
        }
        self.layouts.retain_mut(|layout| {
            let (sessions, json) = match groups.iter_mut().find(|g| Arc::ptr_eq(&g.conn, &layout.conn)) {
                Some(g) => {
                    let active = g.before_active.min(g.tabs.len() - 1);
                    let saved =
                        SavedWorkspace { version: saved::VERSION, tabs: std::mem::take(&mut g.tabs), active, window: Some(self.window) };
                    match serde_json::to_string(&saved) {
                        Ok(j) => (std::mem::take(&mut g.sessions), j),
                        Err(e) => {
                            log::warn!("saving the layout: {e}");
                            return true;
                        }
                    }
                }
                None => (Vec::new(), String::new()),
            };
            if json != layout.json {
                layout.conn.save_layout(layout.id, sessions, json.clone());
                layout.json = json.clone();
            }
            // Done with a host once it heard the layout is gone.
            !json.is_empty()
        });
    }

    /// The window is going away with its last tab: every goattyd forgets
    /// its layout (sent before the kills, whose replies prove both arrived).
    fn forget_layouts(&mut self) {
        for layout in self.layouts.drain(..) {
            layout.conn.save_layout(layout.id, Vec::new(), String::new());
        }
    }

    fn refresh_icon_font(&mut self, cx: &mut Context<'_, Self>) {
        let app = cx.global::<AppSettings>();
        let family = app.client.font.family.clone().unwrap_or_default();
        let fonts = app.fonts.clone();
        self.nerd.update(&family, &fonts, cx);
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---- lookups ------------------------------------------------------------

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn focused_view(&self) -> Option<&Entity<TerminalView>> {
        self.active_tab()?.focused_view()
    }

    /// (tab index, pane) holding pane `id`.
    fn find_pane(&self, id: PaneId) -> Option<(usize, &Pane)> {
        self.tabs.iter().enumerate().find_map(|(i, t)| t.pane(id).map(|p| (i, p)))
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// What a tab is called: its name, else what its focused pane shows.
    fn tab_label(&self, tab: &Tab, cx: &gpui::App) -> String {
        if let Some(name) = &tab.name {
            return name.clone();
        }
        if let Some((view, _)) = &tab.connecting {
            return view.read(cx).host_label().to_string();
        }
        tab.focused_view().map(|v| pane_label(v.read(cx))).unwrap_or_else(|| "goatty".into())
    }

    // ---- focus and titles ---------------------------------------------------

    fn focus_active(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some((view, _)) = self.active_tab().and_then(|t| t.connecting.as_ref()) {
            view.focus_handle(cx).focus(window, cx);
        } else if let Some(view) = self.focused_view() {
            view.focus_handle(cx).focus(window, cx);
        }
    }

    fn update_window_title(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.active_tab() else { return };
        if let Some((view, _)) = &tab.connecting {
            window.set_window_title(&view.read(cx).title());
            return;
        }
        let Some(view) = tab.focused_view() else { return };
        let label = self.tab_label(tab, cx);
        window.set_window_title(&view.read(cx).session().decorate_title(&label));
    }

    /// A pane got focus (click, keyboard navigation, new pane).
    fn pane_focused(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((index, _)) = self.find_pane(id) else { return };
        let tab = &mut self.tabs[index];
        if tab.focused != id {
            tab.focused = id;
            tab.zoomed = false;
        }
        self.active = index;
        self.update_window_title(window, cx);
        self.save_layout(cx);
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.finish_rename(true, window, cx);
        self.active = index;
        self.focus_active(window, cx);
        self.update_window_title(window, cx);
        if window.is_window_active()
            && let Some(host) = &self.tabs[index].host
        {
            crate::settings_window::set_host_link(cx, Some(host.host_link()));
        }
        self.save_layout(cx);
        cx.notify();
    }

    // ---- connecting -----------------------------------------------------------

    /// A new tab (after the active one) that connects for `plan` and shows
    /// the connect screen until its session is there.
    pub fn open_connect_tab(&mut self, mut plan: ConnectPlan, window: &mut Window, cx: &mut Context<'_, Self>) {
        // A host this window has tabs on already: its last layout is this
        // window's, so start a session instead of claiming another one.
        if matches!(plan.mode, AttachMode::Auto)
            && let Some(live) = crate::hosts::pool::live(&plan.endpoint)
            && self.layouts.iter().any(|l| Arc::ptr_eq(&l.conn, &live))
        {
            plan.mode = AttachMode::New;
        }
        let id = self.next_id();
        let placeholder = self.next_id();
        let view = cx.new(|cx| ConnectView::new(plan, window, cx));
        let sub = cx.subscribe_in(&view, window, move |this, _, ev: &ConnectEvent, window, cx| this.on_connect_event(id, ev, window, cx));
        let tab = Tab {
            id,
            tree: Node::Leaf(placeholder),
            panes: Vec::new(),
            focused: placeholder,
            zoomed: false,
            name: None,
            host: None,
            connecting: Some((view, sub)),
            activity: None,
        };
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(at, tab);
        self.activate(at, window, cx);
    }

    fn on_connect_event(&mut self, tab: u64, ev: &ConnectEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(index) = self.tab_index(tab) else { return };
        match ev {
            ConnectEvent::Changed => {
                if index == self.active {
                    self.update_window_title(window, cx);
                }
                cx.notify();
            }
            ConnectEvent::Cancelled => self.remove_tab(index, window, cx),
            ConnectEvent::Ready => {
                let Some((view, _)) = &self.tabs[index].connecting else { return };
                let (ready, settings) = view.update(cx, |v, _| (v.take_ready(), v.take_open_settings()));
                let Some((startup, spawner)) = ready else { return };
                self.connected(index, startup, spawner, window, cx);
                if let Some(page) = settings {
                    crate::settings_window::open_at(cx, crate::settings_window::Page::from_name(&page));
                }
            }
        }
    }

    /// The connecting tab at `index` got its session: show it there (a
    /// restored layout replaces the tab with its tabs).
    fn connected(&mut self, index: usize, startup: Startup, spawner: Spawner, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.home.is_none() {
            self.home = Some(spawner.clone());
        }
        // Missing shell tools there: offer them (once per host and run).
        crate::shell_setup::offer(spawner.host_link(), cx);
        let was_active = index == self.active;
        let show = match startup {
            Startup::Session(session) => {
                let pane = self.make_pane(session, window, cx);
                let tab = &mut self.tabs[index];
                (tab.tree, tab.focused) = (Node::Leaf(pane.id), pane.id);
                tab.panes = vec![pane];
                tab.connecting = None;
                tab.host = Some(spawner);
                index
            }
            Startup::Restore(r) => {
                let n = r.tabs.len();
                // The window's only tab: the window is the restored one, so
                // it takes the restored size (not when restoring into a
                // window that has other tabs).
                if self.tabs.len() == 1
                    && let Some(w) = r.window
                {
                    w.apply(window);
                    self.window = w;
                }
                self.tabs.remove(index);
                let active = self.insert_restored(r, &spawner, index, window, cx);
                if self.active > index {
                    self.active += n.saturating_sub(1);
                }
                active
            }
        };
        if was_active {
            self.activate(show, window, cx);
        } else {
            self.save_layout(cx);
            cx.notify();
        }
    }

    // ---- creating and closing panes -----------------------------------------

    /// Start a session in the background and put it at `placement`: on the
    /// active tab's host (the window's first host while it connects).
    fn spawn(&mut self, placement: Placement, window: &mut Window, cx: &mut Context<'_, Self>) {
        let host = self.active_tab().and_then(|t| t.host.clone());
        let host = match placement {
            Placement::NewTab => host.or_else(|| self.home.clone()),
            Placement::Split { .. } => host,
        };
        let Some(spawner) = host else {
            let toast =
                Toast::info("Still connecting").body("New tabs and splits can open once this tab is connected.").key("workspace.spawn");
            toast::show_in(window, cx, toast);
            return;
        };
        // Same size and directory as the focused pane; the new pane resizes
        // itself on its first layout.
        let (size, cwd) = match self.focused_view() {
            Some(v) => {
                let v = v.read(cx);
                let (cols, rows) = v.session().terminal.size();
                (TermSize::new(cols, rows), v.cwd().map(|p| p.to_string_lossy().into_owned()))
            }
            None => (TermSize::new(100, 30), None),
        };
        let task = {
            let spawner = spawner.clone();
            cx.background_executor().spawn(async move { spawner.spawn(size, cwd) })
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(session) => this.insert_pane(session, placement, spawner, window, cx),
                Err(e) => {
                    let toast = Toast::error("Couldn't start a shell").body(format!("{e:#}")).key("workspace.spawn");
                    toast::show_in(window, cx, toast);
                }
            });
        })
        .detach();
    }

    fn make_pane(&mut self, session: Arc<Session>, window: &mut Window, cx: &mut Context<'_, Self>) -> Pane {
        let id = self.next_id();
        let view = cx.new(|cx| TerminalView::new(session, window, cx));
        let events =
            cx.subscribe_in(&view, window, move |this, _, ev: &TerminalEvent, window, cx| this.on_terminal_event(id, ev, window, cx));
        let focus = view.focus_handle(cx);
        let focused = cx.on_focus(&focus, window, move |this, window, cx| this.pane_focused(id, window, cx));
        Pane { id, view, _subscriptions: [events, focused] }
    }

    /// Put `session` (from `spawner`) at `placement`.
    fn insert_pane(
        &mut self,
        session: Arc<Session>,
        placement: Placement,
        spawner: Spawner,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let pane = self.make_pane(session, window, cx);
        let id = pane.id;

        // A split whose pane went away while the shell started opens a tab.
        let target = match placement {
            Placement::Split { tab, of, dir } => self.tab_index(tab).filter(|&i| self.tabs[i].tree.contains(of)).map(|i| (i, of, dir)),
            Placement::NewTab => None,
        };
        let index = match target {
            Some((i, of, dir)) => {
                let tab = &mut self.tabs[i];
                tab.tree.split(of, id, dir);
                tab.panes.push(pane);
                tab.focused = id;
                tab.zoomed = false;
                i
            }
            None => {
                let tab_id = self.next_id();
                let tab = Tab {
                    id: tab_id,
                    tree: Node::Leaf(id),
                    panes: vec![pane],
                    focused: id,
                    zoomed: false,
                    name: None,
                    host: Some(spawner),
                    connecting: None,
                    activity: None,
                };
                // New tabs open after the current one, like browsers.
                let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
                self.tabs.insert(at, tab);
                at
            }
        };
        self.activate(index, window, cx);
    }

    /// Take pane `id` out (its shell ended or is being ended). The tab goes
    /// with its last pane, the window with its last tab.
    fn remove_pane(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((index, _)) = self.find_pane(id) else { return };
        let tab = &mut self.tabs[index];
        if tab.tree == Node::Leaf(id) {
            self.remove_tab(index, window, cx);
            return;
        }
        let order = tab.tree.leaves();
        let pos = order.iter().position(|&p| p == id).unwrap_or(0);
        tab.tree.remove(id);
        tab.panes.retain(|p| p.id != id);
        if self.paste.as_ref().is_some_and(|(_, pane, _)| *pane == id) {
            self.paste = None;
        }
        if self.close.as_ref().is_some_and(|(_, target, _)| *target == CloseTarget::Pane(id)) {
            self.close = None;
        }
        if tab.focused == id {
            let left = tab.tree.leaves();
            tab.focused = left[pos.saturating_sub(1).min(left.len() - 1)];
            tab.zoomed = false;
        }
        if index == self.active {
            self.focus_active(window, cx);
            self.update_window_title(window, cx);
        }
        self.save_layout(cx);
        cx.notify();
    }

    fn remove_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let tab = self.tabs.remove(index);
        if self.rename.as_ref().is_some_and(|r| r.tab == tab.id) {
            self.rename = None;
        }
        if self.paste.as_ref().is_some_and(|(_, pane, _)| tab.pane(*pane).is_some()) {
            self.paste = None;
        }
        if self.close.as_ref().is_some_and(|(_, target, _)| match *target {
            CloseTarget::Tab(t) => t == tab.id,
            CloseTarget::Pane(p) => tab.pane(p).is_some(),
        }) {
            self.close = None;
        }
        if self.tabs.is_empty() {
            log::info!("last session ended");
            self.forget_layouts();
            // The last window closing quits the app (see `app::run`).
            window.remove_window();
            return;
        }
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.activate(self.active, window, cx);
    }

    /// End the shells of `panes` (off the UI thread: the daemon answers).
    fn kill_sessions(&self, panes: Vec<Arc<Session>>, cx: &mut Context<'_, Self>) -> gpui::Task<()> {
        cx.background_executor().spawn(async move {
            for s in panes {
                s.kill();
            }
        })
    }

    /// Close pane `id`, asking first when a program runs in it.
    fn close_pane_by_id(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((_, pane)) = self.find_pane(id) else { return };
        let busy = busy_programs(std::iter::once(pane), cx);
        if self.should_confirm(&busy, cx) {
            self.confirm_close(CloseTarget::Pane(id), Closing::Pane, busy, window, cx);
        } else {
            self.force_close(CloseTarget::Pane(id), window, cx);
        }
    }

    /// Close the tab at `index`, asking first when a program runs in it.
    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(index) else { return };
        let (id, panes) = (tab.id, tab.panes.len());
        let busy = busy_programs(tab.panes.iter(), cx);
        if self.should_confirm(&busy, cx) {
            self.confirm_close(CloseTarget::Tab(id), Closing::Tab { panes }, busy, window, cx);
        } else {
            self.force_close(CloseTarget::Tab(id), window, cx);
        }
    }

    fn should_confirm(&self, busy: &[String], cx: &gpui::App) -> bool {
        !busy.is_empty() && cx.global::<AppSettings>().client.panes.confirm_close
    }

    /// End the shells of `target` and take it out.
    fn force_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<'_, Self>) {
        let session_of = |p: &Pane| p.view.read(cx).session().clone();
        let (sessions, last): (Vec<_>, bool) = match target {
            CloseTarget::Pane(id) => {
                let Some((index, pane)) = self.find_pane(id) else { return };
                (vec![session_of(pane)], self.tabs.len() == 1 && self.tabs[index].panes.len() == 1)
            }
            CloseTarget::Tab(id) => {
                let Some(index) = self.tab_index(id) else { return };
                (self.tabs[index].panes.iter().map(session_of).collect(), self.tabs.len() == 1)
            }
        };
        let kill = self.kill_sessions(sessions, cx);
        if !last {
            kill.detach();
            self.remove_target(target, window, cx);
            return;
        }
        // Closing the window quits the app: let goattyd hear about it first
        // (the layout goes before the kills, whose replies prove both arrived).
        self.forget_layouts();
        cx.spawn_in(window, async move |this, cx| {
            kill.await;
            let _ = this.update_in(cx, |this, window, cx| this.remove_target(target, window, cx));
        })
        .detach();
    }

    fn remove_target(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<'_, Self>) {
        match target {
            CloseTarget::Pane(id) => self.remove_pane(id, window, cx),
            CloseTarget::Tab(id) => {
                if let Some(index) = self.tab_index(id) {
                    self.remove_tab(index, window, cx);
                }
            }
        }
    }

    fn on_terminal_event(&mut self, id: PaneId, ev: &TerminalEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        match ev {
            TerminalEvent::TitleChanged => {
                if self.focused_view().is_some_and(|v| self.find_pane(id).is_some_and(|(_, p)| p.view == *v)) {
                    self.update_window_title(window, cx);
                }
                cx.notify();
            }
            TerminalEvent::Exited => self.remove_pane(id, window, cx),
            TerminalEvent::ConfirmPaste(request) => self.confirm_paste(id, request.clone(), window, cx),
            TerminalEvent::Activity(activity) => self.mark_activity(id, *activity, cx),
        }
    }

    /// Mark the pane's tab if it's in the background and the settings
    /// want this kind of activity shown. A stronger mark (a bell over a
    /// finished command over output) replaces a weaker one.
    fn mark_activity(&mut self, pane: PaneId, activity: Activity, cx: &mut Context<'_, Self>) {
        let Some((index, _)) = self.find_pane(pane) else { return };
        if index == self.active || !marks(&cx.global::<AppSettings>().client.tabs, activity) {
            return;
        }
        let tab = &mut self.tabs[index];
        if tab.activity < Some(activity) {
            tab.activity = Some(activity);
            cx.notify();
        }
    }

    // ---- moving tabs ----------------------------------------------------------

    /// Put tab `id` at position `to`.
    fn move_tab_to(&mut self, id: u64, to: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(from) = self.tab_index(id) else { return };
        let to = to.min(self.tabs.len() - 1);
        if from == to {
            return;
        }
        let active_id = self.tabs[self.active].id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self.tab_index(active_id).unwrap_or(0);
        self.activate(self.active, window, cx);
    }

    fn move_active_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.active_tab() else { return };
        let to = (self.active as isize + delta).clamp(0, self.tabs.len() as isize - 1) as usize;
        self.move_tab_to(tab.id, to, window, cx);
    }

    // ---- renaming ---------------------------------------------------------------

    fn start_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.tabs.get(index) else { return };
        // A double-click inside the field selects a word; it doesn't start over.
        if self.rename.as_ref().is_some_and(|r| r.tab == tab.id) {
            return;
        }
        let (tab_id, label) = (tab.id, self.tab_label(tab, cx));
        let input = cx.new(|cx| {
            let mut input = TextInput::new("Tab name (empty: automatic)", label, cx);
            input.select_all_text(cx);
            input
        });
        let submitted = cx.subscribe_in(&input, window, |this, _, ev: &TextInputEvent, window, cx| {
            if let TextInputEvent::Submitted(_) = ev {
                this.finish_rename(true, window, cx);
            }
        });
        let focus = input.focus_handle(cx);
        // Clicking elsewhere keeps what was typed.
        let blurred = cx.on_blur(&focus, window, |this, window, cx| this.finish_rename(true, window, cx));
        focus.focus(window, cx);
        self.rename = Some(Rename { tab: tab_id, input, _subscriptions: [submitted, blurred] });
        cx.notify();
    }

    fn finish_rename(&mut self, keep: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(rename) = self.rename.take() else { return };
        if keep && let Some(i) = self.tab_index(rename.tab) {
            let text = rename.input.read(cx).text().trim().to_string();
            self.tabs[i].name = (!text.is_empty()).then_some(text);
        }
        self.focus_active(window, cx);
        self.update_window_title(window, cx);
        self.save_layout(cx);
        cx.notify();
    }

    fn on_rename_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if ev.keystroke.key == "escape" {
            self.finish_rename(false, window, cx);
            cx.stop_propagation();
        }
    }

    // ---- actions: tabs --------------------------------------------------------

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.spawn(Placement::NewTab, window, cx);
    }

    fn close_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(tab) = self.active_tab() {
            self.close_pane_by_id(tab.focused, window, cx);
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.close_tab_at(self.active, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + 1) % self.tabs.len(), window, cx);
        }
    }

    fn previous_tab(&mut self, _: &PreviousTab, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + self.tabs.len() - 1) % self.tabs.len(), window, cx);
        }
    }

    fn move_tab_left(&mut self, _: &MoveTabLeft, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.move_active_tab(-1, window, cx);
    }

    fn move_tab_right(&mut self, _: &MoveTabRight, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.move_active_tab(1, window, cx);
    }

    /// Alt+digit: tab `index` if it exists; otherwise the key goes to the
    /// shell (Alt+digit is a zsh digit argument), as it does with one tab.
    fn select_tab(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<'_, Self>) {
        match index.filter(|&i| i < self.tabs.len() && self.tabs.len() > 1) {
            Some(i) => self.activate(i, window, cx),
            None => cx.propagate(),
        }
    }

    fn rename_tab(&mut self, _: &RenameTab, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.start_rename(self.active, window, cx);
    }

    // ---- actions: panes ---------------------------------------------------------

    fn split(&mut self, dir: Direction, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(tab) = self.active_tab() {
            self.spawn(Placement::Split { tab: tab.id, of: tab.focused, dir }, window, cx);
        }
    }

    /// Alt+arrow: the neighbouring pane, or the key goes to the shell when
    /// there is none (so a single pane loses no shortcut).
    fn focus_pane(&mut self, dir: Direction, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.active_tab() else { return };
        match (!tab.zoomed).then(|| tab.tree.neighbor(tab.focused, dir)).flatten() {
            Some(id) => {
                if let Some(p) = tab.pane(id) {
                    p.view.focus_handle(cx).focus(window, cx);
                }
            }
            None => cx.propagate(),
        }
    }

    fn resize_pane(&mut self, dir: Direction, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.active_tab_mut() else { return };
        let focused = tab.focused;
        if tab.zoomed || !tab.tree.resize(focused, dir, RESIZE_STEP) {
            cx.propagate();
            return;
        }
        self.save_layout(cx);
        cx.notify();
    }

    fn toggle_pane_zoom(&mut self, _: &TogglePaneZoom, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.active_tab_mut() else { return };
        if matches!(tab.tree, Node::Leaf(_)) {
            toast::show_in(window, cx, Toast::info("Nothing to zoom").body("Zoom lets one pane of a split fill the tab.").key("pane.zoom"));
            return;
        }
        tab.zoomed = !tab.zoomed;
        self.save_layout(cx);
        cx.notify();
    }

    fn equalize_panes(&mut self, _: &EqualizePanes, _window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(tab) = self.active_tab_mut() {
            tab.tree.equalize();
            self.save_layout(cx);
            cx.notify();
        }
    }

    fn on_divider_drag(&mut self, ev: &DragMoveEvent<DividerDrag>, _window: &mut Window, cx: &mut Context<'_, Self>) {
        let d = ev.drag(cx).clone();
        let Some(i) = self.tab_index(d.tab) else { return };
        let b = ev.bounds;
        let (pos, start, len) = match d.axis {
            Axis::Horizontal => (ev.event.position.x - b.origin.x, d.split.x, d.split.w),
            Axis::Vertical => (ev.event.position.y - b.origin.y, d.split.y, d.split.h),
        };
        let total = match d.axis {
            Axis::Horizontal => b.size.width,
            Axis::Vertical => b.size.height,
        };
        if total <= px(0.) || len <= 0.0 {
            return;
        }
        let fraction = (pos / total - start) / len;
        self.tabs[i].tree.set_divider(&d.path, d.index, fraction);
        self.save_layout(cx);
        cx.notify();
    }

    // ---- overlays -----------------------------------------------------------------

    fn confirm_close(&mut self, target: CloseTarget, closing: Closing, busy: Vec<String>, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.close_palette(window, cx);
        let dialog = cx.new(|cx| CloseConfirm::new(closing, busy, window, cx));
        let sub = cx.subscribe_in(&dialog, window, |this, _, ev: &CloseConfirmEvent, window, cx| {
            let CloseConfirmEvent::Done(close) = *ev;
            let Some((_, target, _)) = this.close.take() else { return };
            if close {
                this.force_close(target, window, cx);
            }
            this.focus_active(window, cx);
            cx.notify();
        });
        self.close = Some((dialog, target, sub));
        cx.notify();
    }

    fn confirm_paste(&mut self, pane: PaneId, request: PasteRequest, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.close_palette(window, cx);
        let dialog = cx.new(|cx| PasteConfirm::new(request, window, cx));
        let sub = cx.subscribe_in(&dialog, window, |this, _, ev: &PasteConfirmEvent, window, cx| {
            let PasteConfirmEvent::Done(text) = ev;
            let Some((_, pane, _)) = this.paste.take() else { return };
            if let Some((_, p)) = this.find_pane(pane) {
                let view = p.view.clone();
                view.focus_handle(cx).focus(window, cx);
                if let Some(text) = text {
                    view.update(cx, |pane, cx| pane.paste_text(text, cx));
                }
            } else {
                this.focus_active(window, cx);
            }
            cx.notify();
        });
        // A newer paste replaces one still waiting.
        self.paste = Some((dialog, pane, sub));
        cx.notify();
    }

    fn toggle_palette(&mut self, _: &ToggleCommandPalette, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        self.open_palette(Scope::All, window, cx);
    }

    fn connect_to_host(&mut self, _: &ConnectToHost, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.open_palette(Scope::Hosts, window, cx);
    }

    fn open_palette(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<'_, Self>) {
        let palette = cx.new(|cx| CommandPalette::new(scope, window, cx));
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
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    // ---- rendering ------------------------------------------------------------------

    fn render_panes(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let app = cx.global::<AppSettings>();
        let dim = app.client.panes.dim_inactive;
        let bg = gpui::Hsla::from(gpui::rgb(app.colors.background.0));
        let mut area = div()
            .id("panes")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(ui.border)
            .on_drag_move(cx.listener(Self::on_divider_drag));
        let Some(tab) = self.active_tab() else { return area };
        if let Some((view, _)) = &tab.connecting {
            return area.child(view.clone());
        }
        let (leaves, dividers) = if tab.zoomed { (vec![(tab.focused, Rect::FULL)], Vec::new()) } else { tab.tree.layout(Rect::FULL) };
        let split = leaves.len() > 1;
        for (id, r) in leaves {
            let Some(pane) = tab.pane(id) else { continue };
            let faded = split && dim > 0.0 && id != tab.focused;
            area = area.child(
                div()
                    .id(gpui::ElementId::NamedInteger("pane".into(), id))
                    .absolute()
                    .left(relative(r.x))
                    .top(relative(r.y))
                    .w(relative(r.w))
                    .h(relative(r.h))
                    // The area's background shows through as the divider.
                    .when(r.x > 1e-4, |d| d.pl(px(DIVIDER)))
                    .when(r.y > 1e-4, |d| d.pt(px(DIVIDER)))
                    .child(
                        div()
                            .size_full()
                            .relative()
                            .child(pane.view.clone())
                            .when(faded, |d| d.child(div().absolute().inset_0().bg(bg.opacity(dim)))),
                    ),
            );
        }
        for (n, d) in dividers.into_iter().enumerate() {
            let drag = DividerDrag { tab: tab.id, path: d.path.clone(), index: d.index, axis: d.axis, split: d.split };
            let handle = div().id(gpui::ElementId::NamedInteger("divider".into(), n as u64)).absolute().occlude();
            let handle = match d.axis {
                Axis::Horizontal => handle
                    .left(relative(d.at))
                    .ml(px(-DIVIDER_GRAB / 2.0))
                    .w(px(DIVIDER_GRAB))
                    .top(relative(d.split.y))
                    .h(relative(d.split.h))
                    .cursor_col_resize(),
                Axis::Vertical => handle
                    .top(relative(d.at))
                    .mt(px(-DIVIDER_GRAB / 2.0))
                    .h(px(DIVIDER_GRAB))
                    .left(relative(d.split.x))
                    .w(relative(d.split.w))
                    .cursor_row_resize(),
            };
            area = area.child(handle.on_drag(drag, |_, _, _, cx| cx.new(|_| gpui::EmptyView)));
        }
        area
    }
}

/// A dialog centered over the window, which it dims.
fn modal(dialog: impl IntoElement) -> impl IntoElement {
    div().absolute().inset_0().bg(gpui::black().opacity(0.35)).flex().items_center().justify_center().px_4().child(dialog)
}

/// The programs running in `panes`, other than shells (one entry per busy
/// pane). Panes whose program isn't known yet count as idle.
fn busy_programs<'a>(panes: impl Iterator<Item = &'a Pane>, cx: &gpui::App) -> Vec<String> {
    panes.filter_map(|p| p.view.read(cx).process().filter(|name| !icons::is_shell(name)).map(str::to_string)).collect()
}

/// What a pane is called when its tab has no name: the application's
/// title, else the running program, else the directory's name.
fn pane_label(view: &TerminalView) -> String {
    if let Some(t) = view.app_title() {
        return t.to_string();
    }
    if let Some(p) = view.process().filter(|p| !icons::is_shell(p)) {
        return p.to_string();
    }
    if let Some(cwd) = view.cwd() {
        return dir_label(&cwd, std::env::var_os("HOME").as_deref().map(Path::new));
    }
    view.process().unwrap_or("goatty").to_string()
}

/// `~` for the home directory, else the last path component.
fn dir_label(dir: &Path, home: Option<&Path>) -> String {
    if Some(dir) == home {
        return "~".into();
    }
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.to_string_lossy().into_owned())
}

/// Opaque unless the terminal background is see-through.
pub fn background_appearance(t: &goatty_config::TerminalSettings) -> gpui::WindowBackgroundAppearance {
    use gpui::WindowBackgroundAppearance as W;
    match (t.is_transparent(), t.blur) {
        (false, _) => W::Opaque,
        (true, false) => W::Transparent,
        (true, true) => W::Blurred,
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        // However a tab became the active one, it's being looked at now.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.activity = None;
        }
        let tabs = &cx.global::<AppSettings>().client.tabs;
        let show_bar = tabs.show == TabBarShow::Always || self.tabs.len() > 1;
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::connect_to_host))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_pane))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::move_tab_left))
            .on_action(cx.listener(Self::move_tab_right))
            .on_action(cx.listener(|this, _: &SelectTab1, w, cx| this.select_tab(Some(0), w, cx)))
            .on_action(cx.listener(|this, _: &SelectTab2, w, cx| this.select_tab(Some(1), w, cx)))
            .on_action(cx.listener(|this, _: &SelectTab3, w, cx| this.select_tab(Some(2), w, cx)))
            .on_action(cx.listener(|this, _: &SelectTab4, w, cx| this.select_tab(Some(3), w, cx)))
            .on_action(cx.listener(|this, _: &SelectTab5, w, cx| this.select_tab(Some(4), w, cx)))
            .on_action(cx.listener(|this, _: &SelectTab6, w, cx| this.select_tab(Some(5), w, cx)))
            .on_action(cx.listener(|this, _: &SelectTab7, w, cx| this.select_tab(Some(6), w, cx)))
            .on_action(cx.listener(|this, _: &SelectTab8, w, cx| this.select_tab(Some(7), w, cx)))
            .on_action(cx.listener(|this, _: &SelectLastTab, w, cx| this.select_tab(this.tabs.len().checked_sub(1), w, cx)))
            .on_action(cx.listener(Self::rename_tab))
            .on_action(cx.listener(|this, _: &SplitRight, w, cx| this.split(Direction::Right, w, cx)))
            .on_action(cx.listener(|this, _: &SplitDown, w, cx| this.split(Direction::Down, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneLeft, w, cx| this.focus_pane(Direction::Left, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneRight, w, cx| this.focus_pane(Direction::Right, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneUp, w, cx| this.focus_pane(Direction::Up, w, cx)))
            .on_action(cx.listener(|this, _: &FocusPaneDown, w, cx| this.focus_pane(Direction::Down, w, cx)))
            .on_action(cx.listener(|this, _: &ResizePaneLeft, _, cx| this.resize_pane(Direction::Left, cx)))
            .on_action(cx.listener(|this, _: &ResizePaneRight, _, cx| this.resize_pane(Direction::Right, cx)))
            .on_action(cx.listener(|this, _: &ResizePaneUp, _, cx| this.resize_pane(Direction::Up, cx)))
            .on_action(cx.listener(|this, _: &ResizePaneDown, _, cx| this.resize_pane(Direction::Down, cx)))
            .on_action(cx.listener(Self::toggle_pane_zoom))
            .on_action(cx.listener(Self::equalize_panes))
            .when(show_bar, |d| d.child(self.render_tab_bar(window, cx)))
            .child(self.render_panes(cx))
            .child(self.toasts.clone())
            .when_some(self.palette.as_ref(), |d, (palette, _)| {
                d.child(div().absolute().top(px(48.)).left_0().right_0().px_4().flex().justify_center().child(palette.clone()))
            })
            .when_some(self.paste.as_ref(), |d, (dialog, _, _)| d.child(modal(dialog.clone())))
            .when_some(self.close.as_ref(), |d, (dialog, _, _)| d.child(modal(dialog.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_labels() {
        let home = Path::new("/home/me");
        assert_eq!(dir_label(Path::new("/home/me"), Some(home)), "~");
        assert_eq!(dir_label(Path::new("/home/me/coding/goatty"), Some(home)), "goatty");
        assert_eq!(dir_label(Path::new("/"), Some(home)), "/");
        assert_eq!(dir_label(Path::new("/etc"), None), "etc");
    }
}
