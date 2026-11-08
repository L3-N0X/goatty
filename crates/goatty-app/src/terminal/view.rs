//! TerminalView: one pane. Owns a [`Session`], maps GPUI input to terminal
//! input, hosts the grid element, and tells its parent (the workspace) about
//! title changes and the shell exiting via [`TerminalEvent`].

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use goatty_config::{ClientSettings, CursorShape, FontSettings, Renderer};
use goatty_term::input::{Key, Mods, MouseAction, MouseButton, encode_key, encode_mouse, encode_paste};
use goatty_term::{CursorKind, Frame, SelectKind, TermMode, UiEvent};
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Font, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton as GMouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollDelta, ScrollEvent,
    Styled, Window, div, px,
};

use super::element::{Geom, TerminalElement, hsla};
use super::glyphs::GlyphCache;
use super::keys::{mods, terminal_key};
use super::stats::Stats;
use crate::actions::{
    Copy, CopyOrInterrupt, DecreaseFontSize, IncreaseFontSize, Paste, ResetFontSize, ScrollPageDown, ScrollPageUp, TERMINAL_CONTEXT,
};
use crate::session::Session;
use crate::settings::{AppSettings, palette};
use crate::toast::{self, Toast};

/// Cursor blink half-period (xterm/GTK default is ~530 ms).
const BLINK: Duration = Duration::from_millis(530);
const DEFAULT_TITLE: &str = "goatty";
/// The zoom toast is brief: the text size itself is the feedback.
const ZOOM_TOAST: Duration = Duration::from_millis(1800);

/// What a pane tells its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// [`TerminalView::title`] changed.
    TitleChanged,
    /// The shell exited; the pane should close.
    Exited,
}

pub struct TerminalView {
    session: Arc<Session>,
    focus: FocusHandle,
    // Shared with the element, which fills them during prepaint.
    frame: Rc<RefCell<Frame>>,
    geom: Rc<StdCell<Geom>>,
    stats: Rc<RefCell<Stats>>,
    glyphs: Rc<RefCell<GlyphCache>>,
    // Appearance (from the settings; see `apply_settings`).
    font: Font,
    /// Settings size plus `zoom`.
    font_size: Pixels,
    /// Ctrl with +/- on top of the settings size, until reset.
    zoom: f32,
    line_height: f32,
    padding: Pixels,
    glyph_mode: bool,
    cursor_blink: bool,
    // Interaction state.
    selecting: bool,
    scroll_accum: f32,
    cursor_visible: bool,
    last_activity: Instant,
    last_bytes: u64,
    /// Title set by the application (OSC 0/2).
    app_title: String,
    /// Last decorated title reported via [`TerminalEvent::TitleChanged`].
    title: String,
    exited: bool,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    pub fn new(session: Arc<Session>, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let app = cx.global::<AppSettings>().clone();
        let settings = app.client.clone();
        let focus = cx.focus_handle();
        cx.observe_global::<AppSettings>(|view, cx| {
            let app = cx.global::<AppSettings>().clone();
            view.apply_settings(&app);
            cx.notify();
        })
        .detach();
        window.focus(&focus, cx);

        // I/O threads wake us; one notify per frame at most.
        let wake_rx = session.wake_rx.clone();
        cx.spawn_in(window, async move |this, cx| {
            while wake_rx.recv_async().await.is_ok() {
                if this.update(cx, |view, cx| view.on_wake(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Cursor blink. Stays solid while typing/output is happening and only
        // ticks (one cheap frame per 530 ms) while the window is focused.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let alive = this.update_in(cx, |view, window, cx| {
                    let focused = view.focus.is_focused(window);
                    let next =
                        if !focused || !view.cursor_blink || view.last_activity.elapsed() < BLINK { true } else { !view.cursor_visible };
                    if next != view.cursor_visible {
                        view.cursor_visible = next;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        if settings.debug.stats {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let alive = this.update(cx, |view, _| {
                        let bytes = view.session.bytes_in.load(Ordering::Relaxed);
                        view.stats.borrow_mut().report(bytes, &mut view.last_bytes);
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        let mut view = Self {
            session,
            focus,
            frame: Rc::new(RefCell::new(Frame::default())),
            geom: Rc::new(StdCell::new(Geom::default())),
            stats: Rc::new(RefCell::new(Stats::default())),
            glyphs: Rc::new(RefCell::new(GlyphCache::new())),
            font: gpui::font("monospace"),
            font_size: px(settings.font.size),
            zoom: 0.0,
            line_height: settings.font.line_height,
            padding: px(settings.terminal.padding),
            glyph_mode: true,
            cursor_blink: true,
            selecting: false,
            scroll_accum: 0.0,
            cursor_visible: true,
            last_activity: Instant::now(),
            last_bytes: 0,
            app_title: DEFAULT_TITLE.into(),
            title: String::new(),
            exited: false,
        };
        view.apply_settings(&app);
        view
    }

    /// Take font, layout, cursor and colors from the settings. Called at
    /// start and whenever the settings change (live, no restart).
    fn apply_settings(&mut self, app: &AppSettings) {
        let s: &ClientSettings = &app.client;
        let family = s.font.family.clone().unwrap_or_else(|| "monospace".into());
        if self.font.family.as_ref() != family.as_str() {
            self.font = gpui::font(family);
        }
        self.font_size = px((s.font.size + self.zoom).clamp(FontSettings::MIN_SIZE, FontSettings::MAX_SIZE));
        self.line_height = s.font.line_height;
        self.padding = px(s.terminal.padding);
        self.glyph_mode = s.terminal.renderer == Renderer::Glyphs;
        self.cursor_blink = s.terminal.cursor_blink;
        if !self.cursor_blink {
            self.cursor_visible = true;
        }
        self.session.terminal.set_default_cursor(match s.terminal.cursor_shape {
            CursorShape::Block => CursorKind::Block,
            CursorShape::Bar => CursorKind::Beam,
            CursorShape::Underline => CursorKind::Underline,
        });
        self.session.set_palette(palette(&app.colors, s.theme.bold_is_bright));
    }

    /// Title for a window or tab: the application's title plus host and
    /// link state when remote.
    pub fn title(&self) -> String {
        self.session.decorate_title(&self.app_title)
    }

    fn on_wake(&mut self, cx: &mut Context<'_, Self>) {
        self.session.clear_wake();
        self.poke_cursor();
        while let Ok(ev) = self.session.ui_events.try_recv() {
            match ev {
                UiEvent::Title(t) => self.app_title = t,
                UiEvent::ResetTitle => self.app_title = DEFAULT_TITLE.into(),
                UiEvent::ClipboardStore(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                UiEvent::Bell | UiEvent::CwdChanged | UiEvent::Exit => {}
            }
        }
        // Host and link state (reconnecting…) are part of the title, so
        // re-check even without a Title event.
        let title = self.title();
        if title != self.title {
            self.title = title;
            cx.emit(TerminalEvent::TitleChanged);
        }
        if !self.exited && self.session.exited.load(Ordering::Acquire) {
            self.exited = true;
            cx.emit(TerminalEvent::Exited);
        }
        cx.notify();
    }

    fn send(&mut self, bytes: &[u8]) {
        self.session.terminal.scroll_to_bottom();
        self.session.write(bytes);
    }

    /// Show the cursor solid and restart the blink timer.
    fn poke_cursor(&mut self) {
        self.last_activity = Instant::now();
        self.cursor_visible = true;
    }

    // ---- actions ------------------------------------------------------------

    fn copy_selection(&mut self, cx: &mut Context<'_, Self>) -> bool {
        match self.session.terminal.selection_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    /// The explicit copy action (menu, palette, Ctrl+Shift+C) says what it
    /// did; Ctrl+C with a selection (`copy_or_interrupt`) stays quiet.
    fn copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<'_, Self>) {
        let toast = match self.session.terminal.selection_text() {
            Some(text) => {
                let chars = text.chars().count();
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                Toast::success("Copied to clipboard").body(if chars == 1 { "1 character".into() } else { format!("{chars} characters") })
            }
            None => Toast::info("Nothing to copy").body("Select text with the mouse first."),
        };
        toast::show_in(window, cx, toast.key("terminal.copy"));
    }

    fn copy_or_interrupt(&mut self, _: &CopyOrInterrupt, _window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.copy_selection(cx) {
            self.session.terminal.selection_clear();
        } else if let Some(bytes) = encode_key(&Key::Text("c"), Mods { ctrl: true, ..Mods::default() }, self.session.terminal.mode()) {
            self.poke_cursor();
            self.send(&bytes);
        }
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.paste_clipboard(cx) {
            toast::show_in(window, cx, Toast::info("Nothing to paste").body("The clipboard has no text.").key("terminal.paste"));
        }
    }

    /// Returns false if the clipboard had no text.
    fn paste_clipboard(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return false };
        // Multiline/suspicious paste confirmation: #29.
        let bytes = encode_paste(&text, self.session.terminal.mode());
        self.send(&bytes);
        true
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _window: &mut Window, cx: &mut Context<'_, Self>) {
        self.session.terminal.scroll(self.geom.get().rows as i32);
        cx.notify();
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _window: &mut Window, cx: &mut Context<'_, Self>) {
        self.session.terminal.scroll(-(self.geom.get().rows as i32));
        cx.notify();
    }

    /// Zoom this pane (0 resets) and show the new size in a toast that
    /// replaces itself while zooming.
    fn zoom_by(&mut self, delta: f32, window: &mut Window, cx: &mut Context<'_, Self>) {
        let base = cx.global::<AppSettings>().client.font.size;
        let before = self.zoom;
        self.zoom =
            if delta == 0.0 { 0.0 } else { (base + self.zoom + delta).clamp(FontSettings::MIN_SIZE, FontSettings::MAX_SIZE) - base };
        self.font_size = px(base + self.zoom);
        cx.notify();
        let size = base + self.zoom;
        let toast = if delta != 0.0 && self.zoom == before {
            let limit = if delta > 0.0 { "largest" } else { "smallest" };
            Toast::info(format!("Text size {size} px")).body(format!("That's the {limit} size."))
        } else if self.zoom == 0.0 {
            Toast::info(format!("Text size {size} px")).body("The size from your settings.")
        } else {
            Toast::info(format!("Text size {size} px")).action("Reset", |window, cx| window.dispatch_action(Box::new(ResetFontSize), cx))
        };
        toast::show_in(window, cx, toast.key("view.zoom").duration(ZOOM_TOAST));
    }

    fn increase_font_size(&mut self, _: &IncreaseFontSize, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.zoom_by(1.0, window, cx);
    }

    fn decrease_font_size(&mut self, _: &DecreaseFontSize, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.zoom_by(-1.0, window, cx);
    }

    fn reset_font_size(&mut self, _: &ResetFontSize, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.zoom_by(0.0, window, cx);
    }

    // ---- raw input ----------------------------------------------------------

    /// Keys that no binding claimed go to the shell.
    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<'_, Self>) {
        self.poke_cursor();
        cx.notify();
        let Some((key, mods)) = terminal_key(&ev.keystroke) else { return };
        if let Some(bytes) = encode_key(&key, mods, self.session.terminal.mode()) {
            self.session.terminal.selection_clear();
            self.send(&bytes);
        }
    }

    fn mouse_mode_active(&self, shift: bool) -> bool {
        !shift && self.session.terminal.mode().intersects(TermMode::MOUSE_MODE)
    }

    fn report_mouse(&self, button: MouseButton, action: MouseAction, col: usize, row: usize, m: &gpui::Modifiers) {
        if let Some(b) = encode_mouse(button, action, col, row, mods(m), self.session.terminal.mode()) {
            self.session.write(&b);
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        window.focus(&self.focus, cx);
        let (col, row, right) = self.geom.get().cell_at(ev.position);
        if self.mouse_mode_active(ev.modifiers.shift) {
            if let Some(btn) = mouse_button(Some(ev.button)) {
                self.report_mouse(btn, MouseAction::Press, col, row, &ev.modifiers);
            }
            return;
        }
        match ev.button {
            GMouseButton::Left => {
                let kind = match ev.click_count {
                    1 => SelectKind::Simple,
                    2 => SelectKind::Word,
                    _ => SelectKind::Line,
                };
                self.session.terminal.selection_start(col, row, kind, right);
                if kind != SelectKind::Simple {
                    self.session.terminal.selection_update(col, row, right);
                }
                self.selecting = true;
                cx.notify();
            }
            GMouseButton::Middle => {
                self.paste_clipboard(cx);
            }
            _ => {}
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _window: &mut Window, cx: &mut Context<'_, Self>) {
        let (col, row, _) = self.geom.get().cell_at(ev.position);
        if self.mouse_mode_active(ev.modifiers.shift)
            && let Some(btn) = mouse_button(Some(ev.button))
        {
            self.report_mouse(btn, MouseAction::Release, col, row, &ev.modifiers);
        }
        if self.selecting {
            self.selecting = false;
            if !self.session.terminal.has_selection() {
                self.session.terminal.selection_clear();
            }
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<'_, Self>) {
        let (col, row, right) = self.geom.get().cell_at(ev.position);
        if self.selecting && ev.pressed_button == Some(GMouseButton::Left) {
            self.session.terminal.selection_update(col, row, right);
            cx.notify();
            return;
        }
        if self.mouse_mode_active(ev.modifiers.shift) {
            let btn = mouse_button(ev.pressed_button).unwrap_or(MouseButton::None);
            self.report_mouse(btn, MouseAction::Motion, col, row, &ev.modifiers);
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _window: &mut Window, cx: &mut Context<'_, Self>) {
        let geom = self.geom.get();
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y * 3.0,
            ScrollDelta::Pixels(p) => p.y / geom.cell.height.max(px(1.0)),
        };
        self.scroll_accum += lines;
        let whole = self.scroll_accum.trunc() as i32;
        if whole == 0 {
            return;
        }
        self.scroll_accum -= whole as f32;
        let mode = self.session.terminal.mode();
        let (col, row, _) = geom.cell_at(ev.position);
        if self.mouse_mode_active(ev.modifiers.shift) {
            let btn = if whole > 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
            for _ in 0..whole.unsigned_abs() {
                self.report_mouse(btn, MouseAction::Press, col, row, &ev.modifiers);
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // less/man/etc. in the alt screen: wheel → arrow keys.
            let key = if whole > 0 { Key::Up } else { Key::Down };
            if let Some(b) = encode_key(&key, Mods::default(), mode) {
                for _ in 0..whole.unsigned_abs() {
                    self.session.write(&b);
                }
            }
        } else {
            self.session.terminal.scroll(whole);
            cx.notify();
        }
    }
}

fn mouse_button(b: Option<GMouseButton>) -> Option<MouseButton> {
    match b? {
        GMouseButton::Left => Some(MouseButton::Left),
        GMouseButton::Middle => Some(MouseButton::Middle),
        GMouseButton::Right => Some(MouseButton::Right),
        _ => None,
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let bg = hsla(self.session.terminal.palette().background);
        let focused = self.focus.is_focused(window);
        div()
            .size_full()
            .bg(bg)
            .key_context(TERMINAL_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::copy_or_interrupt))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::increase_font_size))
            .on_action(cx.listener(Self::decrease_font_size))
            .on_action(cx.listener(Self::reset_font_size))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(GMouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(GMouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(GMouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(GMouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(GMouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(TerminalElement {
                session: self.session.clone(),
                frame: self.frame.clone(),
                geom: self.geom.clone(),
                stats: self.stats.clone(),
                font: self.font.clone(),
                font_size: self.font_size,
                line_height_factor: self.line_height,
                padding: self.padding,
                focused,
                glyph_mode: self.glyph_mode,
                glyphs: self.glyphs.clone(),
                cursor_visible: self.cursor_visible,
            })
    }
}
