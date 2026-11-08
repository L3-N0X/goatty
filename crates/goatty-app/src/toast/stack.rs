//! One window's toasts, without any UI: order, replacing by key, eviction
//! when too many are open, and timers that pause. Time is passed in, so the
//! rules are testable without a window.
//!
//! An entry is *visible* until its time runs out or it is dismissed; then
//! it is *leaving* for [`EXIT`] (the layer animates it out) and removed.

use std::time::{Duration, Instant};

use super::{Level, Toast, ToastId};

/// At most this many toasts are visible; a new one pushes the oldest out.
pub const MAX_VISIBLE: usize = 4;
/// How long a dismissed toast stays around for its exit animation.
pub const EXIT: Duration = Duration::from_millis(320);

#[derive(Debug)]
pub struct Entry {
    pub id: ToastId,
    pub toast: Toast,
    /// How often this toast was shown in a row (same key, same content).
    pub count: u32,
    /// Time left while visible; `None` for sticky toasts.
    remaining: Option<Duration>,
    /// When the timer last (re)started; `None` while paused.
    running_since: Option<Instant>,
    leaving_since: Option<Instant>,
}

impl Entry {
    pub fn leaving(&self) -> bool {
        self.leaving_since.is_some()
    }

    fn deadline(&self) -> Option<Instant> {
        if self.leaving() {
            return None;
        }
        Some(self.running_since? + self.remaining?)
    }

    fn exit_done(&self) -> Option<Instant> {
        self.leaving_since.map(|t| t + EXIT)
    }

    fn start(&mut self, toast: Toast, paused: bool, now: Instant) {
        self.remaining = toast.visible_for();
        self.running_since = (!paused).then_some(now);
        self.leaving_since = None;
        self.toast = toast;
    }

    fn pause(&mut self, now: Instant) {
        if let (Some(since), Some(rem)) = (self.running_since.take(), self.remaining) {
            self.remaining = Some(rem.saturating_sub(now - since));
        }
    }
}

#[derive(Debug, Default)]
pub struct Stack {
    /// Oldest first.
    entries: Vec<Entry>,
    paused: bool,
}

impl Stack {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, id: ToastId) -> bool {
        self.entries.iter().any(|e| e.id == id && !e.leaving())
    }

    /// Show `toast`. One with the same key as a visible toast replaces it in
    /// place (taking the new id); otherwise it goes at the end.
    pub fn push(&mut self, id: ToastId, toast: Toast, now: Instant) {
        let paused = self.paused;
        if toast.key.is_some()
            && let Some(e) = self.entries.iter_mut().find(|e| !e.leaving() && e.toast.key == toast.key)
        {
            e.count = if e.toast.same_content(&toast) { e.count + 1 } else { 1 };
            e.id = id;
            e.start(toast, paused, now);
            return;
        }
        let mut e = Entry { id, toast: toast.clone(), count: 1, remaining: None, running_since: None, leaving_since: None };
        e.start(toast, paused, now);
        self.entries.push(e);
        self.evict(now);
    }

    /// Replace toast `id`'s contents, restarting its timer. Returns false if
    /// it isn't visible any more.
    pub fn update(&mut self, id: ToastId, toast: Toast, now: Instant) -> bool {
        let paused = self.paused;
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id && !e.leaving()) else { return false };
        e.count = 1;
        e.start(toast, paused, now);
        true
    }

    pub fn dismiss(&mut self, id: ToastId, now: Instant) {
        for e in self.entries.iter_mut().filter(|e| e.id == id && !e.leaving()) {
            e.leaving_since = Some(now);
        }
    }

    pub fn dismiss_all(&mut self, now: Instant) {
        for e in self.entries.iter_mut().filter(|e| !e.leaving()) {
            e.leaving_since = Some(now);
        }
    }

    /// Stop or restart every timer (pointer over a toast, window inactive).
    pub fn set_paused(&mut self, paused: bool, now: Instant) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        for e in self.entries.iter_mut().filter(|e| !e.leaving()) {
            if paused {
                e.pause(now);
            } else {
                e.running_since = Some(now);
            }
        }
    }

    /// Expire and remove entries whose time is up. Returns whether anything
    /// changed.
    pub fn advance(&mut self, now: Instant) -> bool {
        let before = self.entries.len();
        let mut changed = false;
        for e in &mut self.entries {
            if e.deadline().is_some_and(|d| d <= now) {
                e.leaving_since = Some(now);
                changed = true;
            }
        }
        self.entries.retain(|e| e.exit_done().is_none_or(|t| t > now));
        changed || self.entries.len() != before
    }

    /// When [`advance`](Self::advance) next has something to do.
    pub fn next_wake(&self) -> Option<Instant> {
        self.entries.iter().filter_map(|e| e.deadline().or(e.exit_done())).min()
    }

    /// Too many visible: the oldest goes, sparing work in progress if possible.
    fn evict(&mut self, now: Instant) {
        loop {
            let visible: Vec<usize> = (0..self.entries.len()).filter(|&i| !self.entries[i].leaving()).collect();
            if visible.len() <= MAX_VISIBLE {
                return;
            }
            let victim = visible.iter().copied().find(|&i| self.entries[i].toast.level != Level::Loading).unwrap_or(visible[0]);
            self.entries[victim].leaving_since = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn titles(s: &Stack) -> Vec<String> {
        s.entries().iter().filter(|e| !e.leaving()).map(|e| e.toast.title.to_string()).collect()
    }

    #[test]
    fn toasts_expire_then_leave() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(ToastId(1), Toast::info("a").duration(2 * S), t0);
        assert_eq!(s.next_wake(), Some(t0 + 2 * S));
        assert!(!s.advance(t0 + S));
        assert!(s.advance(t0 + 2 * S));
        assert!(s.entries()[0].leaving() && !s.contains(ToastId(1)));
        assert_eq!(s.next_wake(), Some(t0 + 2 * S + EXIT));
        assert!(s.advance(t0 + 2 * S + EXIT));
        assert!(s.is_empty());
        assert_eq!(s.next_wake(), None);
    }

    #[test]
    fn same_key_replaces_in_place_and_counts_repeats() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(ToastId(1), Toast::info("Text size 15").key("zoom").duration(2 * S), t0);
        s.push(ToastId(2), Toast::info("other"), t0);
        s.push(ToastId(3), Toast::info("Text size 16").key("zoom").duration(2 * S), t0 + S);
        assert_eq!(titles(&s), ["Text size 16", "other"]);
        assert_eq!(s.entries()[0].id, ToastId(3));
        assert_eq!(s.entries()[0].count, 1);
        // The timer restarted.
        assert!(!s.advance(t0 + 2 * S));
        s.push(ToastId(4), Toast::info("Text size 16").key("zoom"), t0 + 2 * S);
        assert_eq!(s.entries()[0].count, 2);
        // No key, no replacing.
        s.push(ToastId(5), Toast::info("other"), t0);
        assert_eq!(titles(&s).len(), 3);
    }

    #[test]
    fn update_restarts_and_misses_when_gone() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(ToastId(1), Toast::loading("Working…"), t0);
        assert_eq!(s.next_wake(), None, "loading is sticky");
        assert!(s.update(ToastId(1), Toast::success("Done").duration(S), t0 + 10 * S));
        assert_eq!(s.next_wake(), Some(t0 + 11 * S));
        s.dismiss(ToastId(1), t0 + 10 * S);
        assert!(!s.update(ToastId(1), Toast::success("Again"), t0 + 10 * S));
    }

    #[test]
    fn pausing_keeps_the_time_left() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(ToastId(1), Toast::info("a").duration(4 * S), t0);
        s.set_paused(true, t0 + S);
        assert_eq!(s.next_wake(), None);
        assert!(!s.advance(t0 + 100 * S));
        s.set_paused(false, t0 + 100 * S);
        assert_eq!(s.next_wake(), Some(t0 + 103 * S));
        // Shown while paused: the timer starts on resume.
        s.set_paused(true, t0 + 100 * S);
        s.push(ToastId(2), Toast::info("b").duration(S), t0 + 100 * S);
        s.set_paused(false, t0 + 200 * S);
        assert_eq!(s.next_wake(), Some(t0 + 201 * S));
    }

    #[test]
    fn too_many_evicts_the_oldest_but_spares_loading() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(ToastId(1), Toast::loading("busy"), t0);
        for i in 2..=MAX_VISIBLE as u64 + 2 {
            s.push(ToastId(i), Toast::info(format!("t{i}")), t0);
        }
        let visible = titles(&s);
        assert_eq!(visible.len(), MAX_VISIBLE);
        assert_eq!(visible[0], "busy");
        assert!(!visible.contains(&"t2".to_string()) && !visible.contains(&"t3".to_string()));
    }

    #[test]
    fn dismiss_all() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(ToastId(1), Toast::loading("busy"), t0);
        s.push(ToastId(2), Toast::info("x"), t0);
        s.dismiss_all(t0);
        assert!(titles(&s).is_empty());
        assert!(s.advance(t0 + EXIT));
        assert!(s.is_empty());
    }
}
