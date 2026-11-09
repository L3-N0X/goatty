//! A window's layout as goattyd keeps it (#118): tabs, their split trees
//! with session ids as leaves, names, focus and zoom. The workspace saves
//! it on every change (`Connection::save_layout`); starting goatty again
//! claims the newest layout no window holds and reattaches its sessions
//! ([`restore`]).
//!
//! The JSON is ours alone; the daemon stores it as is. Bump [`VERSION`]
//! when the format changes incompatibly (an unreadable layout is ignored,
//! its sessions stay detached).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use goatty_client::Connection;
use goatty_proto::{LayoutId, SessionId};
use goatty_term::TermSize;
use serde::{Deserialize, Serialize};

use super::layout::Node;
use crate::session::{AttachMode, Session};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub version: u32,
    pub tabs: Vec<SavedTab>,
    /// Index of the active tab.
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    /// Set by renaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Leaves are session ids.
    pub tree: Node,
    pub focused: SessionId,
    #[serde(default)]
    pub zoomed: bool,
}

/// A restored tab: its tree has session ids as leaves, all attached.
pub struct RestoredTab {
    pub name: Option<String>,
    pub tree: Node,
    pub focused: SessionId,
    pub zoomed: bool,
}

/// A window brought back from a saved layout.
pub struct Restored {
    pub id: LayoutId,
    pub tabs: Vec<RestoredTab>,
    pub active: usize,
    pub sessions: HashMap<SessionId, Arc<Session>>,
}

/// How a window starts.
pub enum Startup {
    /// One tab with this session.
    Session(Arc<Session>),
    Restore(Restored),
}

/// A fresh layout id: random, so windows on other machines attached to the
/// same goattyd don't collide.
pub fn new_id() -> LayoutId {
    use std::hash::{BuildHasher, Hasher};
    // `RandomState` is seeded randomly per process.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.write_u32(std::process::id());
    h.finish()
}

/// Keep what can be shown: tabs whose trees still have a session in
/// `alive` (each session once), focus on a pane that exists, the active
/// index in range. `None` when no tab is left.
pub fn prune(saved: SavedWorkspace, alive: &HashSet<SessionId>) -> Option<(Vec<RestoredTab>, usize)> {
    if saved.version != VERSION {
        log::info!("saved layout has format {} (this goatty reads {VERSION}); not restoring it", saved.version);
        return None;
    }
    let mut seen = HashSet::new();
    let mut tabs = Vec::new();
    let mut active = 0;
    for (i, tab) in saved.tabs.into_iter().enumerate() {
        let Some(tree) = tab.tree.map_leaves(&mut |id| (alive.contains(&id) && seen.insert(id)).then_some(id)) else { continue };
        // The active tab, else the one before it.
        if i <= saved.active {
            active = tabs.len();
        }
        let leaves = tree.leaves();
        let focused = if leaves.contains(&tab.focused) { tab.focused } else { leaves[0] };
        let zoomed = tab.zoomed && leaves.len() > 1;
        tabs.push(RestoredTab { name: tab.name.filter(|n| !n.trim().is_empty()), tree, focused, zoomed });
    }
    if tabs.is_empty() {
        return None;
    }
    let active = active.min(tabs.len() - 1);
    Some((tabs, active))
}

/// Claim the newest saved layout on `conn` and reattach its sessions.
/// `None` when there is none, or none of its sessions could be attached
/// (the caller then reattaches a single session as before). Blocks.
/// What a window attached to `conn` starts with: the last window's saved
/// layout when attaching automatically, else a single session.
pub fn startup(conn: &Arc<Connection>, size: TermSize, mode: crate::session::AttachMode) -> Result<Startup> {
    use crate::session::AttachMode;
    let restored = match mode {
        AttachMode::Auto => restore(conn, size).unwrap_or_else(|e| {
            log::warn!("restoring the last window: {e:#}");
            None
        }),
        _ => None,
    };
    Ok(match restored {
        Some(r) => Startup::Restore(r),
        None => Startup::Session(Session::daemon(conn.clone(), size, mode, None)?),
    })
}

pub fn restore(conn: &Arc<Connection>, size: TermSize) -> Result<Option<Restored>> {
    let Some(info) = conn.claim_layout()? else { return Ok(None) };
    let saved: SavedWorkspace = match serde_json::from_str(&info.json) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("saved layout {:x} is unreadable ({e}); not restoring it", info.id);
            return Ok(None);
        }
    };
    let alive: HashSet<SessionId> = info.sessions.iter().copied().collect();
    let Some((tabs, _)) = prune(saved.clone(), &alive) else { return Ok(None) };
    let mut sessions = HashMap::new();
    for id in tabs.iter().flat_map(|t| t.tree.leaves()) {
        match Session::daemon(conn.clone(), size, AttachMode::Id(id), None) {
            Ok(s) => {
                sessions.insert(id, s);
            }
            // Ended since the claim.
            Err(e) => log::info!("not restoring session {id}: {e:#}"),
        }
    }
    let attached: HashSet<SessionId> = sessions.keys().copied().collect();
    let Some((tabs, active)) = prune(saved, &attached) else { return Ok(None) };
    log::info!("restored a window with {} tabs and {} sessions", tabs.len(), sessions.len());
    Ok(Some(Restored { id: info.id, tabs, active, sessions }))
}

#[cfg(test)]
mod tests {
    use super::super::layout::Direction;
    use super::*;

    fn tab(tree: Node, focused: SessionId) -> SavedTab {
        SavedTab { name: None, tree, focused, zoomed: false }
    }

    fn split(a: SessionId, b: SessionId) -> Node {
        let mut t = Node::Leaf(a);
        t.split(a, b, Direction::Right);
        t
    }

    #[test]
    fn json_format() {
        let saved = SavedWorkspace {
            version: VERSION,
            tabs: vec![SavedTab { name: Some("build".into()), tree: split(3, 7), focused: 7, zoomed: true }, tab(Node::Leaf(9), 9)],
            active: 1,
        };
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<SavedWorkspace>(&json).unwrap(), saved);
        // Unset fields are optional.
        let short: SavedWorkspace = serde_json::from_str(r#"{"version":1,"tabs":[{"tree":{"leaf":4},"focused":4}],"active":0}"#).unwrap();
        assert_eq!(short.tabs[0], tab(Node::Leaf(4), 4));
    }

    #[test]
    fn pruning_drops_ended_sessions_and_fixes_focus() {
        let saved = SavedWorkspace {
            version: VERSION,
            tabs: vec![
                SavedTab { name: Some(" ".into()), tree: split(1, 2), focused: 2, zoomed: true },
                tab(Node::Leaf(3), 3),
                tab(Node::Leaf(4), 4),
            ],
            active: 1,
        };
        let alive: HashSet<SessionId> = [1, 4].into();
        let (tabs, active) = prune(saved.clone(), &alive).unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].tree, Node::Leaf(1));
        assert_eq!((tabs[0].focused, tabs[0].zoomed, tabs[0].name.as_deref()), (1, false, None));
        // The active tab (3) ended: the one before it is active.
        assert_eq!(active, 0);
        assert!(prune(saved.clone(), &HashSet::new()).is_none());
        assert!(prune(SavedWorkspace { version: VERSION + 1, ..saved }, &alive).is_none());
    }

    #[test]
    fn a_session_shows_once() {
        let saved = SavedWorkspace { version: VERSION, tabs: vec![tab(split(1, 2), 1), tab(split(2, 3), 3)], active: 1 };
        let (tabs, active) = prune(saved, &[1, 2, 3].into()).unwrap();
        assert_eq!(tabs[0].tree.leaves(), [1, 2]);
        assert_eq!(tabs[1].tree, Node::Leaf(3));
        assert_eq!(active, 1);
    }
}
