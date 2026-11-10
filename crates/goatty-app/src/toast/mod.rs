//! Toasts: small, non-blocking notifications in the corner of a window.
//!
//! Anything with an `&mut App` can show one, from the UI thread or (via
//! `cx.update`) from a finished background task:
//!
//! ```ignore
//! toast::show(cx, Toast::success("Copied to clipboard"));
//!
//! // Progress, then the result in the same place:
//! let id = toast::show(cx, Toast::loading("Reloading shells…"));
//! // … later …
//! toast::update(cx, id, Toast::error("Couldn't reload shells").body(err).action("Retry", |_, cx| retry(cx)));
//!
//! // Repeated feedback replaces itself instead of piling up:
//! toast::show(cx, Toast::info("Text size 15 px").key("view.zoom"));
//! ```
//!
//! Toasts never take focus: the terminal keeps the keyboard, and clicking a
//! toast's button doesn't move focus either. They pause while the pointer
//! is over one and while their window is in the background, so nothing
//! disappears unread. Every toast is also logged.
//!
//! Pieces:
//! - [`Toast`]: what to show (level, title, body, buttons, key, lifetime).
//! - [`stack`]: one window's toasts without UI (ordering, replacing by key,
//!   eviction, pausable timers), unit-tested with explicit clocks.
//! - [`ToastLayer`]: the view a window's root renders on top of its content;
//!   it registers itself here so [`show`] can find it.
//! - The functions below route a toast to a window: the one given, else the
//!   active window if it has a layer, else the most recently opened one.
//!   Toasts shown before any layer exists wait for the first one.

mod layer;
pub mod stack;

use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyWindowHandle, App, Global, SharedString, WeakEntity, Window};

pub use layer::ToastLayer;

/// How a toast looks and how long it stays by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
    /// Work in progress, with a spinner. Stays until updated or dismissed.
    Loading,
}

impl Level {
    /// How long a toast of this level stays without a fixed lifetime.
    fn base_duration(self) -> Option<Duration> {
        match self {
            Level::Success => Some(Duration::from_millis(3000)),
            Level::Info => Some(Duration::from_millis(4000)),
            Level::Warning => Some(Duration::from_millis(6000)),
            Level::Error => Some(Duration::from_millis(8000)),
            Level::Loading => None,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Level::Info => "info",
            Level::Success => "success",
            Level::Warning => "warning",
            Level::Error => "error",
            Level::Loading => "loading",
        })
    }
}

/// How long a toast stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// From its level, a little longer for long text or buttons.
    Auto,
    /// Exactly this long (while visible and not paused).
    For(Duration),
    /// Until dismissed or updated.
    Sticky,
}

/// A toast button's code; runs with the toast's window.
pub type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A button on a toast. Clicking it runs the handler and dismisses the toast.
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub run: Handler,
}

impl fmt::Debug for ToastAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToastAction").field("label", &self.label).finish_non_exhaustive()
    }
}

/// What a toast shows. Build one with [`Toast::info`] and friends.
#[derive(Debug, Clone)]
pub struct Toast {
    pub level: Level,
    /// One short line: what happened.
    pub title: SharedString,
    /// Optional details, wrapped, in a smaller muted font.
    pub body: Option<SharedString>,
    /// Toasts with the same key replace each other (in place) instead of
    /// stacking up: zooming shows one "Text size" toast, not ten.
    pub key: Option<SharedString>,
    pub lifetime: Lifetime,
    /// Buttons; the first is drawn as the primary one.
    pub actions: Vec<ToastAction>,
}

/// Extra time per character past this many (title + body).
const READING_CHARS: usize = 60;
const READING_PER_CHAR: Duration = Duration::from_millis(40);
/// Buttons need time to reach.
const ACTIONS_EXTRA: Duration = Duration::from_millis(3000);
const MAX_AUTO: Duration = Duration::from_secs(15);

impl Toast {
    pub fn new(level: Level, title: impl Into<SharedString>) -> Self {
        Self { level, title: title.into(), body: None, key: None, lifetime: Lifetime::Auto, actions: Vec::new() }
    }

    pub fn info(title: impl Into<SharedString>) -> Self {
        Self::new(Level::Info, title)
    }

    pub fn success(title: impl Into<SharedString>) -> Self {
        Self::new(Level::Success, title)
    }

    pub fn warning(title: impl Into<SharedString>) -> Self {
        Self::new(Level::Warning, title)
    }

    pub fn error(title: impl Into<SharedString>) -> Self {
        Self::new(Level::Error, title)
    }

    pub fn loading(title: impl Into<SharedString>) -> Self {
        Self::new(Level::Loading, title)
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        let body = body.into();
        self.body = (!body.is_empty()).then_some(body);
        self
    }

    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Stay exactly `d` (while visible and not paused).
    pub fn duration(mut self, d: Duration) -> Self {
        self.lifetime = Lifetime::For(d);
        self
    }

    /// Stay until dismissed or updated.
    pub fn sticky(mut self) -> Self {
        self.lifetime = Lifetime::Sticky;
        self
    }

    /// Add a button. Clicking it runs `f` (with the toast's window) and
    /// dismisses the toast.
    pub fn action(mut self, label: impl Into<SharedString>, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.actions.push(ToastAction { label: label.into(), run: Rc::new(f) });
        self
    }

    /// How long this toast stays visible, `None` for sticky.
    pub fn visible_for(&self) -> Option<Duration> {
        match self.lifetime {
            Lifetime::For(d) => Some(d),
            Lifetime::Sticky => None,
            Lifetime::Auto => {
                let mut d = self.level.base_duration()?;
                let chars = self.title.chars().count() + self.body.as_ref().map_or(0, |b| b.chars().count());
                d += READING_PER_CHAR * chars.saturating_sub(READING_CHARS) as u32;
                if !self.actions.is_empty() {
                    d += ACTIONS_EXTRA;
                }
                Some(d.min(MAX_AUTO))
            }
        }
    }

    /// Whether `other` says the same thing (a repeat gets a "×2" badge).
    pub fn same_content(&self, other: &Toast) -> bool {
        self.level == other.level && self.title == other.title && self.body == other.body
    }
}

/// Identifies a shown toast, for [`update`] and [`dismiss`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToastId(pub u64);

/// Where toasts go: every window's layer. A GPUI global.
#[derive(Default)]
struct ToastCenter {
    next_id: u64,
    /// In registration order (the last is the newest window).
    layers: Vec<(AnyWindowHandle, WeakEntity<ToastLayer>)>,
    /// Shown before any layer existed.
    pending: Vec<(ToastId, Toast)>,
}

impl Global for ToastCenter {}

enum Op {
    Show(ToastId, Toast),
    Update(ToastId, Toast),
    #[expect(dead_code, reason = "toast API for upcoming features")]
    Dismiss(ToastId),
    DismissAll,
}

/// Show `toast` in the active goatty window.
pub fn show(cx: &mut App, toast: Toast) -> ToastId {
    send(cx, None, toast)
}

/// Show `toast` in `window` (falls back like [`show`] if it has no layer).
pub fn show_in(window: &Window, cx: &mut App, toast: Toast) -> ToastId {
    send(cx, Some(window.window_handle()), toast)
}

/// Replace the toast `id` with `toast`, in place, restarting its timer. If
/// it is gone (dismissed or expired), `toast` is shown as a new one.
pub fn update(cx: &mut App, id: ToastId, toast: Toast) {
    log_toast(&toast);
    deliver(cx, None, Op::Update(id, toast));
}

/// Dismiss toast `id` (no-op if it's gone).
#[expect(dead_code, reason = "toast API for upcoming features")]
pub fn dismiss(cx: &mut App, id: ToastId) {
    deliver(cx, None, Op::Dismiss(id));
}

/// Dismiss every toast in every window.
pub fn dismiss_all(cx: &mut App) {
    deliver(cx, None, Op::DismissAll);
}

fn send(cx: &mut App, window: Option<AnyWindowHandle>, toast: Toast) -> ToastId {
    let center = cx.default_global::<ToastCenter>();
    center.next_id += 1;
    let id = ToastId(center.next_id);
    log_toast(&toast);
    deliver(cx, window, Op::Show(id, toast));
    id
}

fn log_toast(t: &Toast) {
    match &t.body {
        Some(b) => log::info!("toast ({}): {}: {b}", t.level, t.title),
        None => log::info!("toast ({}): {}", t.level, t.title),
    }
}

/// Apply `op` once the current update is done, so showing a toast works
/// from anywhere, including a toast's own button handler.
fn deliver(cx: &mut App, window: Option<AnyWindowHandle>, op: Op) {
    cx.defer(move |cx| apply(cx, window, op));
}

fn apply(cx: &mut App, window: Option<AnyWindowHandle>, op: Op) {
    let center = cx.default_global::<ToastCenter>();
    center.layers.retain(|(_, l)| l.upgrade().is_some());
    let layers: Vec<_> = center.layers.iter().filter_map(|(w, l)| Some((*w, l.upgrade()?))).collect();
    // Not `cx.active_window()`: on Wayland it borrows the platform client,
    // which may already be borrowed while deferred work runs. Each layer
    // tracks its own window's activation.
    let target = window
        .and_then(|w| layers.iter().find(|(lw, _)| *lw == w))
        .or_else(|| layers.iter().find(|(_, l)| l.read(cx).window_active()))
        .or(layers.last())
        .map(|(_, l)| l.clone());
    match op {
        Op::Show(id, toast) => match target {
            Some(layer) => layer.update(cx, |l, cx| l.show(id, toast, cx)),
            None => cx.default_global::<ToastCenter>().pending.push((id, toast)),
        },
        Op::Update(id, toast) => {
            if let Some((_, layer)) = layers.iter().find(|(_, l)| l.read(cx).contains(id)) {
                layer.update(cx, |l, cx| l.update(id, toast, cx));
            } else if let Some(p) = cx.default_global::<ToastCenter>().pending.iter_mut().find(|(pid, _)| *pid == id) {
                p.1 = toast;
            } else {
                apply(cx, window, Op::Show(id, toast));
            }
        }
        Op::Dismiss(id) => {
            cx.default_global::<ToastCenter>().pending.retain(|(pid, _)| *pid != id);
            for (_, layer) in &layers {
                layer.update(cx, |l, cx| l.dismiss(id, cx));
            }
        }
        Op::DismissAll => {
            cx.default_global::<ToastCenter>().pending.clear();
            for (_, layer) in &layers {
                layer.update(cx, |l, cx| l.dismiss_all(cx));
            }
        }
    }
}

/// Called by a new [`ToastLayer`]: register it and hand it what was waiting.
fn register(cx: &mut App, window: AnyWindowHandle, layer: WeakEntity<ToastLayer>) -> Vec<(ToastId, Toast)> {
    let center = cx.default_global::<ToastCenter>();
    center.layers.push((window, layer));
    std::mem::take(&mut center.pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_lifetime_grows_with_text_and_buttons() {
        let short = Toast::info("Copied");
        assert_eq!(short.visible_for(), Some(Duration::from_secs(4)));
        let long = Toast::info("x".repeat(30)).body("y".repeat(80));
        assert_eq!(long.visible_for(), Some(Duration::from_secs(4) + READING_PER_CHAR * 50));
        let button = Toast::error("Failed").action("Retry", |_, _| {});
        assert_eq!(button.visible_for(), Some(Duration::from_secs(8) + ACTIONS_EXTRA));
        assert_eq!(Toast::error("x".repeat(2000)).visible_for(), Some(MAX_AUTO));
        assert_eq!(Toast::loading("Working").visible_for(), None);
        assert_eq!(Toast::info("x").sticky().visible_for(), None);
        assert_eq!(Toast::loading("x").duration(Duration::from_secs(1)).visible_for(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn empty_body_is_none() {
        assert!(Toast::info("x").body("").body.is_none());
        assert!(Toast::info("x").same_content(&Toast::info("x").key("k")));
        assert!(!Toast::info("x").same_content(&Toast::success("x")));
    }
}
