//! Window layouts (#118): what each GUI window showed (tabs, splits, which
//! session sits where), so starting goatty again restores the whole window
//! instead of one session.
//!
//! The layout itself is the client's JSON; the daemon only knows which
//! sessions it shows and which connection holds it. A layout belongs to at
//! most one connection: the one that saved it last or claimed it. When that
//! connection ends, the layout is free and the next window that asks
//! ([`Layouts::claim`]) gets it. Layouts live as long as the daemon (their
//! sessions do too).

use std::collections::HashMap;

use goatty_proto::{LayoutId, LayoutInfo, SessionId};

/// Larger layouts are ignored (a window with hundreds of panes is still far
/// below this).
pub(crate) const MAX_JSON: usize = 256 << 10;
/// Free layouts beyond this many are forgotten, oldest first.
const MAX_FREE: usize = 32;

struct Record {
    json: String,
    sessions: Vec<SessionId>,
    owner: Option<u64>,
    saved_unix: u64,
    /// Save order (newest wins a claim).
    seq: u64,
}

#[derive(Default)]
pub(crate) struct Layouts {
    map: HashMap<LayoutId, Record>,
    seq: u64,
}

impl Layouts {
    /// Store `conn`'s layout `id` (and give it to `conn`). No sessions:
    /// forget it.
    pub(crate) fn save(&mut self, conn: u64, id: LayoutId, sessions: Vec<SessionId>, json: String, now_unix: u64) {
        if sessions.is_empty() {
            self.map.remove(&id);
            return;
        }
        if json.len() > MAX_JSON {
            log::warn!("connection {conn}: layout of {} bytes ignored", json.len());
            return;
        }
        self.seq += 1;
        self.map.insert(id, Record { json, sessions, owner: Some(conn), saved_unix: now_unix, seq: self.seq });
        self.trim();
    }

    /// Give `conn` the newest free layout that still shows a running
    /// session, with only those sessions listed. Free layouts without any
    /// are dropped.
    pub(crate) fn claim(&mut self, conn: u64, alive: impl Fn(SessionId) -> bool) -> Option<LayoutInfo> {
        self.map.retain(|_, r| r.owner.is_some() || r.sessions.iter().any(|&s| alive(s)));
        let (&id, record) = self.map.iter_mut().filter(|(_, r)| r.owner.is_none()).max_by_key(|(_, r)| r.seq)?;
        record.owner = Some(conn);
        record.sessions.retain(|&s| alive(s));
        Some(LayoutInfo { id, json: record.json.clone(), sessions: record.sessions.clone(), saved_unix: record.saved_unix })
    }

    /// `conn` ended: its layouts are free for the next window.
    pub(crate) fn release(&mut self, conn: u64) {
        for r in self.map.values_mut().filter(|r| r.owner == Some(conn)) {
            r.owner = None;
        }
        self.trim();
    }

    fn trim(&mut self) {
        let mut free: Vec<(u64, LayoutId)> = self.map.iter().filter(|(_, r)| r.owner.is_none()).map(|(&id, r)| (r.seq, id)).collect();
        if free.len() <= MAX_FREE {
            return;
        }
        free.sort_unstable();
        for (_, id) in &free[..free.len() - MAX_FREE] {
            self.map.remove(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save(l: &mut Layouts, conn: u64, id: LayoutId, sessions: &[SessionId]) {
        l.save(conn, id, sessions.to_vec(), format!("layout {id}"), 0);
    }

    #[test]
    fn a_layout_is_claimed_once_its_window_is_gone() {
        let mut l = Layouts::default();
        save(&mut l, 1, 10, &[1, 2]);
        // Still held by connection 1.
        assert!(l.claim(2, |_| true).is_none());
        l.release(1);
        let got = l.claim(2, |_| true).unwrap();
        assert_eq!((got.id, got.json.as_str(), got.sessions), (10, "layout 10", vec![1, 2]));
        // A second window doesn't get the same layout.
        assert!(l.claim(3, |_| true).is_none());
    }

    #[test]
    fn newest_free_layout_wins_and_dead_sessions_drop_out() {
        let mut l = Layouts::default();
        save(&mut l, 1, 10, &[1]);
        save(&mut l, 2, 20, &[2, 3]);
        save(&mut l, 3, 30, &[4]);
        for c in 1..=3 {
            l.release(c);
        }
        // 30 is newest but its only session ended: dropped.
        let got = l.claim(9, |s| s != 4 && s != 3).unwrap();
        assert_eq!((got.id, got.sessions), (20, vec![2]));
        assert_eq!(l.claim(9, |s| s != 4).unwrap().id, 10);
        assert!(l.claim(9, |_| true).is_none());
    }

    #[test]
    fn saving_without_sessions_forgets_and_saving_reclaims() {
        let mut l = Layouts::default();
        save(&mut l, 1, 10, &[1]);
        save(&mut l, 1, 10, &[]);
        l.release(1);
        assert!(l.claim(2, |_| true).is_none());
        // A reconnected window saving again holds its layout again.
        save(&mut l, 1, 11, &[1]);
        l.release(1);
        save(&mut l, 5, 11, &[1]);
        assert!(l.claim(2, |_| true).is_none());
    }

    #[test]
    fn oversized_and_surplus_layouts_are_not_kept() {
        let mut l = Layouts::default();
        l.save(1, 1, vec![1], "x".repeat(MAX_JSON + 1), 0);
        assert!(l.map.is_empty());
        for id in 0..(MAX_FREE as u64 + 5) {
            save(&mut l, id, id, &[1]);
            l.release(id);
        }
        assert_eq!(l.map.len(), MAX_FREE);
        assert_eq!(l.claim(99, |_| true).unwrap().id, MAX_FREE as u64 + 4);
    }
}
