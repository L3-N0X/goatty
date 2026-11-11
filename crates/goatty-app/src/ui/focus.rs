//! Keyboard focus for the settings window's controls: tab stops, a focus
//! ring that only shows for keyboard focus, scrolling the focused control
//! into view, and focusing a control by its element id.
//!
//! The controls in [`super::controls`] are stateless: they get no `App` to
//! make a `FocusHandle` with. So the window refills a small pool of spare
//! handles before it builds a page ([`begin`]) and a control takes one by its
//! element id ([`tab`]). Ids are unique per window, which GPUI needs anyway.
//! Outside [`begin`]/[`end`] (every other window) controls stay mouse-only.
//!
//! Components that render while the window lays out (a `RenderOnce` row, a
//! child view) run after [`end`]: they open the pool again with [`enter`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpui::{
    App, Bounds, BoxShadow, ElementId, FocusHandle, InteractiveElement, ParentElement, Pixels, ScrollHandle, Styled, Window, WindowId,
    canvas, point, px,
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
    /// The window [`begin`] was last called for: the only one that gets tab stops.
    owner: Option<WindowId>,
    spare: Vec<FocusHandle>,
    /// How many spare handles [`begin`] keeps ready (at least [`SPARE`]);
    /// doubles when a frame ran out.
    want: usize,
    /// A control found no spare handle this frame, so it has no tab stop yet.
    starved: bool,
    used: HashMap<ElementId, FocusHandle>,
    page: Option<ScrollHandle>,
    menu: Option<ScrollHandle>,
    /// The control the view last scrolled to, so scrolling the page by hand
    /// afterwards isn't undone on the next repaint.
    revealed: Option<ElementId>,
    /// Focus this control when it is first painted.
    autofocus: Option<ElementId>,
    /// Focus is waiting for a control to exist (see [`hand_over`]).
    pending: Option<Pending>,
    /// The controls built since [`begin`]: a focused one that isn't here any
    /// more is gone, and so is the focus (see [`rescue`]).
    built: HashSet<ElementId>,
}

struct Pending {
    target: Handover,
    /// Where focus waits: while it is still here, the user hasn't moved.
    root: FocusHandle,
}

/// The control focus should end up on, once it exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handover {
    /// This control.
    Control(ElementId),
    /// The first control painted in the page.
    FirstOnPage,
}

thread_local! {
    static POOL: RefCell<Pool> = RefCell::new(Pool::default());
}

/// Before building a page of `window`: refill the pool and say what scrolls.
pub fn begin(cx: &mut App, window: &Window, page: &ScrollHandle, menu: &ScrollHandle) {
    POOL.with_borrow_mut(|p| {
        let want = p.want.max(SPARE);
        while p.spare.len() < want {
            p.spare.push(cx.focus_handle());
        }
        p.active = true;
        p.owner = Some(window.window_handle().window_id());
        p.starved = false;
        p.built.clear();
        p.page = Some(page.clone());
        p.menu = Some(menu.clone());
        // The user moved focus on their own: stop waiting.
        if p.pending.as_ref().is_some_and(|h| window.focused(cx).as_ref() != Some(&h.root)) {
            p.pending = None;
        }
    });
}

/// After building the page. Whether some control got no handle (the window
/// should draw again: [`begin`] has made more ready by then).
pub fn end() -> bool {
    POOL.with_borrow_mut(|p| {
        p.active = false;
        std::mem::take(&mut p.starved)
    })
}

/// A scope opened by [`enter`].
#[must_use]
pub struct Scope(bool);

/// For a component that renders while `window` lays out, after [`end`]:
/// its controls get tab stops if `window` is the one the pool serves.
/// Give the scope back to [`leave`] when the component has been built.
pub fn enter(window: &Window) -> Scope {
    POOL.with_borrow_mut(|p| {
        let was = p.active;
        p.active = p.owner == Some(window.window_handle().window_id());
        Scope(was)
    })
}

/// Close a scope of [`enter`]. Draws `window` again if a control found no
/// handle (it will have one then).
pub fn leave(scope: Scope, window: &mut Window) {
    let starved = POOL.with_borrow_mut(|p| {
        p.active = scope.0;
        // A scope inside the build phase leaves the flag for [`end`].
        !scope.0 && std::mem::take(&mut p.starved)
    });
    if starved {
        window.refresh();
    }
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
            let h = h.clone();
            p.built.insert(id.clone());
            return Some(h);
        }
        let Some(h) = p.spare.pop() else {
            p.want = p.want.max(SPARE) * 2;
            p.starved = true;
            return None;
        };
        p.used.insert(id.clone(), h.clone());
        p.built.insert(id.clone());
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

/// Move focus to `root`, which always exists, and on to `target` as soon as
/// that is painted (the page after the drawer closed, a row after one of its
/// buttons removed itself). It keeps waiting, over any number of frames
/// (a page still loading), until `target` shows up or the user moves focus.
pub fn hand_over(root: &FocusHandle, target: Handover, window: &mut Window, cx: &mut App) {
    window.focus(root, cx);
    POOL.with_borrow_mut(|p| p.pending = Some(Pending { target, root: root.clone() }));
}

/// Call once the window has been laid out (every control is built by then):
/// if the focused control is no longer there, say a button that removed
/// itself, focus would sit on nothing and drop every key, so it goes to
/// `root`.
pub fn rescue(root: &FocusHandle, window: &mut Window, cx: &mut App) {
    let Some(focused) = window.focused(cx) else { return };
    let gone = POOL.with_borrow(|p| {
        p.owner == Some(window.window_handle().window_id()) && p.used.iter().any(|(id, h)| *h == focused && !p.built.contains(id))
    });
    if gone {
        window.focus(root, cx);
    }
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
        canvas(|_, _, _| (), move |bounds, _, window, cx| on_paint(&watch, &id, bounds, scroller, stop, window, cx)).absolute().size_full(),
    )
}

/// `el` (an action that fades in on hover, with the element id `id`) shows
/// while focus is on it or inside it, so the controls in it stay in the tab
/// order and are seen when reached. Hide it with `opacity(0.)`, never
/// `invisible()`: a hidden element has no tab stops.
pub fn show_when_focused<E>(el: E, id: &ElementId) -> E
where
    E: InteractiveElement + Styled,
{
    match take(id) {
        Some(handle) => el.track_focus(&handle.tab_stop(false)).in_focus(|s| s.opacity(1.)),
        None => el,
    }
}

fn on_paint(
    handle: &FocusHandle,
    id: &ElementId,
    bounds: Bounds<Pixels>,
    scroller: Scroller,
    stop: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let first = POOL.with_borrow_mut(|p| {
        let waited_for = p.pending.as_ref().is_some_and(|h| match &h.target {
            Handover::Control(c) => c == id,
            Handover::FirstOnPage => {
                stop && scroller == Scroller::Page && p.page.as_ref().is_some_and(|s| s.bounds().contains(&bounds.center()))
            }
        });
        let here = waited_for || p.autofocus.as_ref() == Some(id);
        if here {
            p.autofocus = None;
        }
        if waited_for {
            p.pending = None;
        }
        here
    });
    if first {
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
