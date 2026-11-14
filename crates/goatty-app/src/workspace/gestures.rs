//! The tabs' pointer gestures without UI (unit-tested): where a dragged tab
//! goes, switching tabs with the mouse wheel, and resizing or folding the
//! vertical tab sidebar by dragging its edge.

use std::time::{Duration, Instant};

use goatty_config::TabSettings;

/// Where tab `from` goes while the pointer, at `pos` along the strip, is
/// over tab `over`, which spans `start..end`. The tab moves once the pointer
/// crosses the middle of the tab it's over, coming from its own side. So
/// after a move the pointer is over the dragged tab itself (equal sizes), or
/// short of the other tab's middle (different sizes): it never flips back
/// and forth.
pub fn reorder_target(from: usize, over: usize, pos: f32, start: f32, end: f32) -> Option<usize> {
    let mid = (start + end) / 2.0;
    match over.cmp(&from) {
        std::cmp::Ordering::Greater if pos >= mid => Some(over),
        std::cmp::Ordering::Less if pos <= mid => Some(over),
        _ => None,
    }
}

/// How far a mouse wheel turns in one notch (GPUI reports 3 lines).
const LINES_PER_NOTCH: f32 = 3.0;
/// How far a touchpad scrolls for one tab.
const PIXELS_PER_TAB: f32 = 60.0;
/// A pause this long starts the count over, so a half-finished touchpad
/// swipe doesn't switch a tab much later.
const WHEEL_RESET: Duration = Duration::from_millis(400);

/// One scroll event's vertical amount, positive when scrolling up (GPUI's
/// sign): lines from a mouse wheel, pixels from a touchpad.
#[derive(Debug, Clone, Copy)]
pub enum Wheel {
    Lines(f32),
    Pixels(f32),
}

/// Turns scroll events over the tabs into tab switches: a wheel notch is
/// one tab, a touchpad has to scroll [`PIXELS_PER_TAB`] for one.
#[derive(Debug, Default)]
pub struct WheelSwitch {
    /// Notches scrolled and not used yet (positive: up).
    pending: f32,
    last: Option<Instant>,
}

impl WheelSwitch {
    /// Feed one scroll event at `now`. Returns how many tabs to move:
    /// positive is forward (scrolling down), negative back.
    pub fn feed(&mut self, wheel: Wheel, now: Instant) -> isize {
        let notches = match wheel {
            Wheel::Lines(l) => l / LINES_PER_NOTCH,
            Wheel::Pixels(p) => p / PIXELS_PER_TAB,
        };
        let stale = self.last.is_none_or(|t| now.duration_since(t) > WHEEL_RESET);
        // Turning the other way starts over at once.
        if stale || notches.signum() != self.pending.signum() {
            self.pending = 0.0;
        }
        self.last = Some(now);
        self.pending += notches;
        // A high-resolution wheel's partial notches add up; rounding the
        // last bit up keeps 0.999… from never arriving.
        let whole = (self.pending + self.pending.signum() * 1e-3).trunc();
        self.pending -= whole;
        -(whole as isize)
    }
}

/// Width of the folded sidebar: a rail of icons.
pub const RAIL_WIDTH: f32 = 52.0;
/// Dragging the edge further left than this folds the sidebar.
const FOLD_BELOW: f32 = 96.0;
/// Dragging a folded sidebar's edge past this unfolds it (the gap to
/// [`FOLD_BELOW`] keeps it from flickering between the two).
const UNFOLD_ABOVE: f32 = 120.0;
/// The sidebar never takes more than this share of the window.
const MAX_SHARE: f32 = 0.45;

/// The vertical tab sidebar's size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sidebar {
    /// The unfolded width (kept while folded, for unfolding).
    pub width: f32,
    pub folded: bool,
}

impl Sidebar {
    /// The widest the sidebar gets in a window `window` px wide.
    pub fn max_width(window: f32) -> f32 {
        (window * MAX_SHARE).clamp(TabSettings::MIN_SIDEBAR_WIDTH, TabSettings::MAX_SIDEBAR_WIDTH)
    }

    /// How wide it's drawn in a window `window` px wide.
    pub fn drawn_width(self, window: f32) -> f32 {
        if self.folded { RAIL_WIDTH } else { self.width.min(Self::max_width(window)) }
    }

    /// The sidebar (`self` so far) while its edge is dragged to `x` (from
    /// the window's left edge) in a window `window` px wide, by a drag that
    /// started at `start`. Folding keeps the width from before the drag,
    /// not the narrowest one the drag passed on its way.
    pub fn dragged_to(self, x: f32, window: f32, start: Sidebar) -> Self {
        let folded = if self.folded { x < UNFOLD_ABOVE } else { x < FOLD_BELOW };
        if folded {
            return Self { width: start.width, folded };
        }
        Self { width: x.round().clamp(TabSettings::MIN_SIDEBAR_WIDTH, Self::max_width(window)), folded }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorder_moves_past_the_middle_only() {
        // Tabs of 100 px: dragging tab 0 right over tab 1 (100..200).
        assert_eq!(reorder_target(0, 1, 120.0, 100.0, 200.0), None);
        assert_eq!(reorder_target(0, 1, 150.0, 100.0, 200.0), Some(1));
        // Dragging tab 2 left over tab 0 (0..100).
        assert_eq!(reorder_target(2, 0, 80.0, 0.0, 100.0), None);
        assert_eq!(reorder_target(2, 0, 40.0, 0.0, 100.0), Some(0));
        // Over itself: nothing.
        assert_eq!(reorder_target(1, 1, 150.0, 100.0, 200.0), None);
    }

    #[test]
    fn reorder_is_stable_with_different_sizes() {
        // A (0..50) dragged right over a wide B (50..200): moves at 125.
        assert_eq!(reorder_target(0, 1, 126.0, 50.0, 200.0), Some(1));
        // Now B is 0..150 and A 150..200; the pointer at 126 is over B,
        // which comes before A: only left of B's middle (75) moves back.
        assert_eq!(reorder_target(1, 0, 126.0, 0.0, 150.0), None);
    }

    #[test]
    fn a_wheel_notch_is_one_tab() {
        let t = Instant::now();
        let mut w = WheelSwitch::default();
        // Down is forward.
        assert_eq!(w.feed(Wheel::Lines(-3.0), t), 1);
        assert_eq!(w.feed(Wheel::Lines(-3.0), t), 1);
        assert_eq!(w.feed(Wheel::Lines(3.0), t), -1);
        // Two notches in one event (a fast spin).
        assert_eq!(w.feed(Wheel::Lines(6.0), t), -2);
        // A high-resolution wheel: thirds of a notch add up.
        let mut w = WheelSwitch::default();
        assert_eq!(w.feed(Wheel::Lines(-1.0), t), 0);
        assert_eq!(w.feed(Wheel::Lines(-1.0), t), 0);
        assert_eq!(w.feed(Wheel::Lines(-1.0), t), 1);
    }

    #[test]
    fn touchpads_scroll_a_distance_per_tab() {
        let t = Instant::now();
        let mut w = WheelSwitch::default();
        let steps: isize = (0..10).map(|_| w.feed(Wheel::Pixels(-15.0), t)).sum();
        assert_eq!(steps, 2, "150 px is two tabs");
        // Turning around starts over.
        assert_eq!(w.feed(Wheel::Pixels(40.0), t), 0);
        assert_eq!(w.feed(Wheel::Pixels(30.0), t), -1);
        // A pause forgets what was left.
        let mut w = WheelSwitch::default();
        assert_eq!(w.feed(Wheel::Pixels(-50.0), t), 0);
        assert_eq!(w.feed(Wheel::Pixels(-50.0), t + Duration::from_secs(1)), 0);
        assert_eq!(w.feed(Wheel::Pixels(-20.0), t + Duration::from_secs(1)), 1);
    }

    #[test]
    fn sidebar_resizes_within_limits() {
        let s = Sidebar { width: 220.0, folded: false };
        assert_eq!(s.dragged_to(300.4, 1600.0, s), Sidebar { width: 300.0, folded: false }, "whole pixels");
        assert_eq!(s.dragged_to(130.0, 1600.0, s).width, TabSettings::MIN_SIDEBAR_WIDTH);
        assert_eq!(s.dragged_to(900.0, 1600.0, s).width, TabSettings::MAX_SIDEBAR_WIDTH);
        // A narrow window caps it at 45 %.
        assert_eq!(s.dragged_to(400.0, 600.0, s).width, 270.0);
        assert_eq!(Sidebar { width: 400.0, folded: false }.drawn_width(600.0), 270.0);
    }

    #[test]
    fn sidebar_folds_and_unfolds_with_hysteresis() {
        let start = Sidebar { width: 260.0, folded: false };
        // On its way left the drag passes the narrowest width…
        let narrow = start.dragged_to(140.0, 1600.0, start);
        assert_eq!(narrow.width, TabSettings::MIN_SIDEBAR_WIDTH);
        // …but folding keeps the width from before the drag, for unfolding.
        let folded = narrow.dragged_to(90.0, 1600.0, start);
        assert_eq!(folded, Sidebar { width: 260.0, folded: true });
        assert_eq!(folded.drawn_width(1600.0), RAIL_WIDTH);
        // Between the thresholds it stays as it is.
        assert!(folded.dragged_to(110.0, 1600.0, start).folded);
        assert!(!narrow.dragged_to(110.0, 1600.0, start).folded);
        assert_eq!(folded.dragged_to(125.0, 1600.0, start), Sidebar { width: TabSettings::MIN_SIDEBAR_WIDTH, folded: false });
    }
}
