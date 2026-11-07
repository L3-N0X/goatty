//! TerminalView: owns one pane's session, maps GPUI input to terminal input,
//! and hosts the grid element.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use goatty_term::input::{Key, Mods, MouseAction, MouseButton, encode_key, encode_mouse, encode_paste};
use goatty_term::{Frame, SelectKind, TermMode, UiEvent};
use gpui::{
    ClipboardItem, Context, FocusHandle, Font, InteractiveElement, IntoElement, KeyDownEvent, MouseButton as GMouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollDelta, ScrollEvent, Styled, Window, div, px,
};

use crate::element::{Geom, Stats, TerminalElement, hsla};
use crate::glyphs::GlyphCache;
use crate::session::{Session, Waker};

pub struct TerminalView {
    session: Arc<Session>,
    waker: Arc<Waker>,
    focus: FocusHandle,
    frame: Rc<RefCell<Frame>>,
    geom: Rc<StdCell<Geom>>,
    stats: Rc<RefCell<Stats>>,
    font: Font,
    font_size: Pixels,
    selecting: bool,
    scroll_accum: f32,
    show_stats: bool,
    last_bytes: u64,
    glyph_mode: bool,
    glyphs: Rc<RefCell<GlyphCache>>,
    cursor_visible: bool,
    last_activity: Instant,
    base_title: String,
    shown_title: String,
}

/// Cursor blink half-period (xterm/GTK default is ~530 ms).
const BLINK: std::time::Duration = std::time::Duration::from_millis(530);

impl TerminalView {
    pub fn new(
        session: Arc<Session>,
        waker: Arc<Waker>,
        wake_rx: flume::Receiver<()>,
        font: Font,
        font_size: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        // I/O threads wake us; one notify per frame at most.
        cx.spawn_in(window, async move |this, cx| {
            while wake_rx.recv_async().await.is_ok() {
                let alive = this.update_in(cx, |view, window, cx| {
                    view.waker.clear();
                    view.poke_cursor();
                    view.drain_ui_events(window, cx);
                    if view.session.exited.load(Ordering::Acquire) {
                        log::info!("session ended");
                        cx.quit();
                    }
                    cx.notify();
                });
                if alive.is_err() {
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
                    let next = if !focused || view.last_activity.elapsed() < BLINK { true } else { !view.cursor_visible };
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

        let show_stats = std::env::var_os("GOATTY_STATS").is_some();
        if show_stats {
            // 1 Hz stats line on stderr.
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                    if this.update(cx, |view, _| view.report_stats()).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        Self {
            session,
            waker,
            focus,
            frame: Rc::new(RefCell::new(Frame::default())),
            geom: Rc::new(StdCell::new(Geom::default())),
            stats: Rc::new(RefCell::new(Stats::default())),
            font,
            font_size,
            selecting: false,
            scroll_accum: 0.0,
            show_stats,
            last_bytes: 0,
            glyph_mode: std::env::var("GOATTY_RENDER").map(|v| v != "shaped").unwrap_or(true),
            glyphs: Rc::new(RefCell::new(GlyphCache::new())),
            cursor_visible: true,
            last_activity: Instant::now(),
            base_title: "goatty".into(),
            shown_title: String::new(),
        }
    }

    fn report_stats(&mut self) {
        let mut st = self.stats.borrow_mut();
        let now = Instant::now();
        let secs = st.window_start.map(|t| (now - t).as_secs_f64()).unwrap_or(1.0);
        let bytes = self.session.bytes_in.load(Ordering::Relaxed);
        let mbps = (bytes - self.last_bytes) as f64 / 1e6 / secs;
        self.last_bytes = bytes;
        if st.frames > 0 {
            let f = st.frames as u64;
            eprintln!(
                "[stats] {:5.1} fps | prepaint {:6.2} ms avg (lock+snapshot {:5.2}) | paint {:5.2} ms avg | worst {:6.2} ms | {:5} segs/frame | in {:7.2} MB/s",
                st.frames as f64 / secs,
                st.prepaint_us as f64 / f as f64 / 1e3,
                st.snapshot_us as f64 / f as f64 / 1e3,
                st.paint_us as f64 / f as f64 / 1e3,
                st.max_frame_us as f64 / 1e3,
                st.shaped_segments / f,
                mbps
            );
        } else if mbps > 0.0 {
            eprintln!("[stats] idle frames | in {mbps:7.2} MB/s");
        }
        *st = Stats { window_start: Some(now), ..Default::default() };
    }

    fn drain_ui_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Ok(ev) = self.session.ui_events.try_recv() {
            match ev {
                UiEvent::Title(t) => self.base_title = t,
                UiEvent::ResetTitle => self.base_title = "goatty".into(),
                UiEvent::ClipboardStore(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                UiEvent::Bell | UiEvent::CwdChanged | UiEvent::Exit => {}
            }
        }
        // Host and link state (reconnecting…) are part of the title.
        let title = self.session.decorate_title(&self.base_title);
        if title != self.shown_title {
            window.set_window_title(&title);
            self.shown_title = title;
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.session.terminal.scroll_to_bottom();
        self.session.write(bytes);
    }

    fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(text) = self.session.terminal.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            true
        } else {
            false
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            // TODO(stage1): multiline paste confirmation per plan §9.
            let bytes = encode_paste(&text, self.session.terminal.mode());
            self.send(&bytes);
        }
    }

    /// Show the cursor solid and restart the blink timer.
    fn poke_cursor(&mut self) {
        self.last_activity = Instant::now();
        self.cursor_visible = true;
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.poke_cursor();
        cx.notify();
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let mods = Mods { ctrl: m.control, alt: m.alt, shift: m.shift, logo: m.platform };

        // App-level shortcuts (plan §9). Ctrl+C copies only with a selection.
        match (m.control, m.shift, ks.key.as_str()) {
            (true, true, "c") => {
                self.copy(cx);
                return;
            }
            (true, true, "v") | (true, false, "v") => {
                self.paste(cx);
                return;
            }
            (true, false, "c") if self.session.terminal.has_selection() => {
                self.copy(cx);
                self.session.terminal.selection_clear();
                cx.notify();
                return;
            }
            (true, true, "+") | (true, true, "=") => {
                self.font_size = (self.font_size + px(1.0)).min(px(48.0));
                cx.notify();
                return;
            }
            (true, true, "-") | (true, true, "_") => {
                self.font_size = (self.font_size - px(1.0)).max(px(6.0));
                cx.notify();
                return;
            }
            (_, true, "pageup") => {
                let rows = self.geom.get().rows as i32;
                self.session.terminal.scroll(rows);
                cx.notify();
                return;
            }
            (_, true, "pagedown") => {
                let rows = self.geom.get().rows as i32;
                self.session.terminal.scroll(-rows);
                cx.notify();
                return;
            }
            _ => {}
        }
        if m.platform {
            return; // leave Super combos to the compositor
        }

        let key = match ks.key.as_str() {
            "enter" => Key::Enter,
            "backspace" => Key::Backspace,
            "tab" => Key::Tab,
            "escape" => Key::Escape,
            "space" => Key::Space,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "insert" => Key::Insert,
            "delete" => Key::Delete,
            k if k.len() >= 2 && k.starts_with('f') && k[1..].parse::<u8>().is_ok() => Key::F(k[1..].parse().unwrap()),
            k => match &ks.key_char {
                // Typed text from the layout (already shifted). With ctrl the
                // layout often gives nothing; fall back to the key name.
                Some(t) if !m.control => Key::Text(t.as_str()),
                _ if k.chars().count() == 1 => Key::Text(k),
                _ => return, // modifier-only or unknown keys
            },
        };
        // key_char already includes shift; don't let encode_key see it for text.
        let mods = if matches!(key, Key::Text(_)) { Mods { shift: false, ..mods } } else { mods };
        if let Some(bytes) = encode_key(&key, mods, self.session.terminal.mode()) {
            self.session.terminal.selection_clear();
            self.send(&bytes);
        }
    }

    fn mouse_mode_active(&self, shift: bool) -> bool {
        !shift && self.session.terminal.mode().intersects(TermMode::MOUSE_MODE)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let (col, row, right) = self.geom.get().cell_at(ev.position);
        let mods = mods_of(&ev.modifiers);
        if self.mouse_mode_active(ev.modifiers.shift) {
            let btn = match ev.button {
                GMouseButton::Left => MouseButton::Left,
                GMouseButton::Middle => MouseButton::Middle,
                GMouseButton::Right => MouseButton::Right,
                _ => return,
            };
            if let Some(b) = encode_mouse(btn, MouseAction::Press, col, row, mods, self.session.terminal.mode()) {
                self.session.write(&b);
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
            GMouseButton::Middle => self.paste(cx),
            _ => {}
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let (col, row, _) = self.geom.get().cell_at(ev.position);
        if self.mouse_mode_active(ev.modifiers.shift) {
            let btn = match ev.button {
                GMouseButton::Left => MouseButton::Left,
                GMouseButton::Middle => MouseButton::Middle,
                GMouseButton::Right => MouseButton::Right,
                _ => return,
            };
            if let Some(b) = encode_mouse(btn, MouseAction::Release, col, row, mods_of(&ev.modifiers), self.session.terminal.mode()) {
                self.session.write(&b);
            }
        }
        if self.selecting {
            self.selecting = false;
            if !self.session.terminal.has_selection() {
                self.session.terminal.selection_clear();
            }
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let (col, row, right) = self.geom.get().cell_at(ev.position);
        if self.selecting && ev.pressed_button == Some(GMouseButton::Left) {
            self.session.terminal.selection_update(col, row, right);
            cx.notify();
            return;
        }
        if self.mouse_mode_active(ev.modifiers.shift) {
            let btn = match ev.pressed_button {
                Some(GMouseButton::Left) => MouseButton::Left,
                Some(GMouseButton::Middle) => MouseButton::Middle,
                Some(GMouseButton::Right) => MouseButton::Right,
                _ => MouseButton::None,
            };
            if let Some(b) = encode_mouse(btn, MouseAction::Motion, col, row, mods_of(&ev.modifiers), self.session.terminal.mode()) {
                self.session.write(&b);
            }
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
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
                if let Some(b) = encode_mouse(btn, MouseAction::Press, col, row, mods_of(&ev.modifiers), mode) {
                    self.session.write(&b);
                }
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

fn mods_of(m: &gpui::Modifiers) -> Mods {
    Mods { ctrl: m.control, alt: m.alt, shift: m.shift, logo: m.platform }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = hsla(self.session.terminal.palette.background);
        let focused = self.focus.is_focused(window);
        let _ = self.show_stats;
        div()
            .size_full()
            .bg(bg)
            .track_focus(&self.focus)
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
                line_height_factor: 1.25,
                padding: px(6.0),
                focused,
                glyph_mode: self.glyph_mode,
                glyphs: self.glyphs.clone(),
                cursor_visible: self.cursor_visible,
            })
    }
}
