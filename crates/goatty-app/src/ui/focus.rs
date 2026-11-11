//! Keyboard focus for the settings window's controls: tab stops, a focus
//! ring that only shows for keyboard focus, scrolling the focused control
//! into view, and focusing a control by its element id.
//!
//! The controls in [`super::controls`] are stateless: they get no `App` to
//! make a `FocusHandle` with. So the window refills a small pool of spare
//! handles before it builds a page ([`begin`]) and a control takes one by its
//! element id ([`tab`]). Ids are unique per window, which GPUI needs anyway.
//! Outside [`begin`]/[`end`] (every other window) controls stay mouse-only.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{
    App, Bounds, BoxShadow, ElementId, FocusHandle, InteractiveElement, ParentElement, Pixels, ScrollHandle, Styled, Window, canvas, point,
    px,
};

use super::style::Ui;

/// Handles kept ready for controls built in one frame.
const SPARE: usize = 64;

/// What scrolls when a focused control is out of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroller {
    /// The settings page.
    Page,
    /// An open dropdown menu.
    Menu,
}

/// How the focus ring is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ring {
    /// Around the control (pills, switches, buttons).
    Outer,
    /// Inside the control (full-width rows, cards and menu items that are
    /// clipped or touch their neighbours).
    Inner,
}

#[derive(Default)]
struct Pool {
    active: bool,
    spare: Vec<FocusHandle>,
    used: HashMap<ElementId, FocusHandle>,
    page: Option<ScrollHandle>,
    menu: Option<ScrollHandle>,
    /// The control the view last scrolled to, so scrolling the page by hand
    /// afterwards isn't undone on the next repaint.
    revealed: Option<ElementId>,
    /// Focus this control when it is first painted.
    autofocus: Option<ElementId>,
}

thread_local! {
    static POOL: RefCell<Pool> = RefCell::new(Pool::default());
}

/// Before building a page: refill the pool and say what scrolls.
pub fn begin(cx: &mut App, page: &ScrollHandle, menu: &ScrollHandle) {
    POOL.with_borrow_mut(|p| {
        while p.spare.len() < SPARE {
            p.spare.push(cx.focus_handle());
        }
        p.active = true;
        p.page = Some(page.clone());
        p.menu = Some(menu.clone());
    });
}

/// After building the page.
pub fn end() {
    POOL.with_borrow_mut(|p| p.active = false);
}

/// The scroll handle of the open dropdown menu, for `menu()` to track.
pub fn menu_scroll() -> Option<ScrollHandle> {
    POOL.with_borrow(|p| p.active.then(|| p.menu.clone()).flatten())
}

fn take(id: &ElementId) -> Option<FocusHandle> {
    POOL.with_borrow_mut(|p| {
        if !p.active {
            return None;
        }
        if let Some(h) = p.used.get(id) {
            return Some(h.clone());
        }
        let h = p.spare.pop()?;
        p.used.insert(id.clone(), h.clone());
        Some(h)
    })
}

/// The handle of control `id`, made if it doesn't exist yet.
pub fn handle(id: &ElementId, cx: &mut App) -> FocusHandle {
    POOL.with_borrow_mut(|p| p.used.entry(id.clone()).or_insert_with(|| cx.focus_handle()).clone())
}

/// Move focus to control `id` (a sibling that was just built).
pub fn focus(id: &ElementId, window: &mut Window, cx: &mut App) {
    let h = handle(id, cx);
    window.focus(&h, cx);
}

/// Focus control `id` the first time it is painted (a menu that was just
/// opened from the keyboard and whose items don't exist yet).
pub fn autofocus(id: ElementId) {
    POOL.with_borrow_mut(|p| p.autofocus = Some(id));
}

/// Whether control `id` is a tab stop of this window.
pub fn is_focused(id: &ElementId, window: &Window) -> bool {
    POOL.with_borrow(|p| p.used.get(id).is_some_and(|h| h.is_focused(window)))
}

pub fn shadow(ui: Ui, ring: Ring) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: ui.accent,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(2.),
        inset: ring == Ring::Inner,
    }]
}

/// Make `el` (which has the element id `id`) a tab stop with a focus ring.
/// Enter and Space click it, through GPUI's own handling of focused
/// elements with `on_click`.
pub fn tab<E>(el: E, id: &ElementId, ui: Ui, ring: Ring, scroller: Scroller) -> E
where
    E: InteractiveElement + ParentElement + Styled,
{
    tab_stop(el, id, ui, ring, scroller, true)
}

/// [`tab`], reachable with Tab only if `stop` (a group's current member;
/// the others are reached with arrow keys).
pub fn tab_stop<E>(el: E, id: &ElementId, ui: Ui, ring: Ring, scroller: Scroller, stop: bool) -> E
where
    E: InteractiveElement + ParentElement + Styled,
{
    let Some(handle) = take(id) else { return el };
    let handle = handle.tab_stop(stop);
    let watch = handle.clone();
    let id = id.clone();
    el.track_focus(&handle).focus_visible(move |s| s.shadow(shadow(ui, ring))).child(
        canvas(|_, _, _| (), move |bounds, _, window, cx| on_paint(&watch, &id, bounds, scroller, window, cx)).absolute().size_full(),
    )
}

fn on_paint(handle: &FocusHandle, id: &ElementId, bounds: Bounds<Pixels>, scroller: Scroller, window: &mut Window, cx: &mut App) {
    if POOL.with_borrow(|p| p.autofocus.as_ref() == Some(id)) {
        POOL.with_borrow_mut(|p| p.autofocus = None);
        window.focus(handle, cx);
    }
    let reveal = POOL.with_borrow_mut(|p| {
        if !handle.is_focused(window) {
            if p.revealed.as_ref() == Some(id) {
                p.revealed = None;
            }
            return None;
        }
        if p.revealed.as_ref() == Some(id) {
            return None;
        }
        p.revealed = Some(id.clone());
        // A click on a control that's under the pointer needs no scrolling.
        if !window.last_input_was_keyboard() {
            return None;
        }
        match scroller {
            Scroller::Page => p.page.clone().map(|s| (s, px(110.))),
            Scroller::Menu => p.menu.clone().map(|s| (s, px(8.))),
        }
    });
    if let Some((scroll, bottom_margin)) = reveal {
        reveal_in(&scroll, bounds, bottom_margin, window);
    }
}

/// Scroll `scroll` so `bounds` (window coordinates) shows, with room below
/// for the floating bar.
fn reveal_in(scroll: &ScrollHandle, bounds: Bounds<Pixels>, bottom_margin: Pixels, window: &mut Window) {
    let view = scroll.bounds();
    let centre = bounds.center();
    // The sidebar and other controls outside the scrolling area.
    if centre.x < view.left() || centre.x > view.right() {
        return;
    }
    let top = view.top() + px(24.);
    let bottom = view.bottom() - bottom_margin;
    let dy = if bounds.top() < top {
        top - bounds.top()
    } else if bounds.bottom() > bottom {
        bottom - bounds.bottom()
    } else {
        return;
    };
    let offset = scroll.offset();
    let max = scroll.max_offset().y;
    // GPUI doesn't clamp a set offset to the end: do it here.
    scroll.set_offset(point(offset.x, (offset.y + dy).clamp(-max, px(0.))));
    window.refresh();
}
