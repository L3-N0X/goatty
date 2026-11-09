//! A tab's split layout, without UI: a tree whose leaves are pane ids and
//! whose inner nodes split their area along one axis into weighted parts.
//!
//! Splits are n-ary: splitting a pane in the direction its parent already
//! splits adds a sibling instead of nesting, so three panes side by side are
//! one split with three parts. Geometry is normalized (the tab's area is
//! 0..1 in both directions); the workspace scales it to pixels.
//!
//! The tree is plain data: saved window layouts (`super::saved`) store it
//! with session ids as leaves, and [`Node::map_leaves`] turns those back
//! into panes.

use serde::{Deserialize, Serialize};

/// Id of a pane within a workspace.
pub type PaneId = u64;

/// The smallest share of its split a part can be resized to.
const MIN_SHARE: f32 = 0.06;
const EPS: f32 = 1e-4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Parts side by side (dividers are vertical lines).
    Horizontal,
    /// Parts stacked (dividers are horizontal lines).
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn axis(self) -> Axis {
        match self {
            Direction::Left | Direction::Right => Axis::Horizontal,
            Direction::Up | Direction::Down => Axis::Vertical,
        }
    }

    /// Right/Down: towards the end of the axis.
    fn forward(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }
}

/// A normalized rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const FULL: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    fn start(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.x,
            Axis::Vertical => self.y,
        }
    }

    fn len(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.w,
            Axis::Vertical => self.h,
        }
    }

    fn end(&self, axis: Axis) -> f32 {
        self.start(axis) + self.len(axis)
    }

    fn other(axis: Axis) -> Axis {
        match axis {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }

    /// Length of the overlap with `o` across `axis` (the perpendicular axis).
    fn overlap_across(&self, o: &Rect, axis: Axis) -> f32 {
        let a = Self::other(axis);
        (self.end(a).min(o.end(a)) - self.start(a).max(o.start(a))).max(0.0)
    }

    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// A draggable boundary between part `index` and `index + 1` of the split at
/// `path` (child indices from the root).
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    pub path: Vec<usize>,
    pub index: usize,
    pub axis: Axis,
    /// The whole split's area.
    pub split: Rect,
    /// Position of the boundary along `axis` (normalized, absolute).
    pub at: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Node {
    Leaf(PaneId),
    Split { axis: Axis, children: Vec<Node>, sizes: Vec<f32> },
}

impl Node {
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { children, .. } => children.iter().for_each(|c| c.collect(out)),
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(l) => *l == id,
            Node::Split { children, .. } => children.iter().any(|c| c.contains(id)),
        }
    }

    /// Put `new` next to `target`, on the `dir` side, sharing `target`'s
    /// space equally. Returns false if `target` isn't in the tree.
    pub fn split(&mut self, target: PaneId, new: PaneId, dir: Direction) -> bool {
        let axis = dir.axis();
        match self {
            Node::Leaf(id) if *id == target => {
                let children =
                    if dir.forward() { vec![Node::Leaf(target), Node::Leaf(new)] } else { vec![Node::Leaf(new), Node::Leaf(target)] };
                *self = Node::Split { axis, children, sizes: vec![0.5, 0.5] };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { axis: a, children, sizes } => {
                // Same axis and `target` is a direct child: add a sibling.
                if *a == axis
                    && let Some(i) = children.iter().position(|c| *c == Node::Leaf(target))
                {
                    let half = sizes[i] / 2.0;
                    sizes[i] = half;
                    let at = if dir.forward() { i + 1 } else { i };
                    children.insert(at, Node::Leaf(new));
                    sizes.insert(at, half);
                    return true;
                }
                children.iter_mut().any(|c| c.split(target, new, dir))
            }
        }
    }

    /// Take `target` out; its space goes to the part before it (or after,
    /// for the first part). The root leaf can't be removed (close the tab
    /// instead): returns false then, and when `target` isn't in the tree.
    pub fn remove(&mut self, target: PaneId) -> bool {
        let Node::Split { children, sizes, .. } = self else { return false };
        if let Some(i) = children.iter().position(|c| *c == Node::Leaf(target)) {
            children.remove(i);
            let freed = sizes.remove(i);
            let heir = i.saturating_sub(1).min(sizes.len() - 1);
            sizes[heir] += freed;
        } else if !children.iter_mut().any(|c| c.remove(target)) {
            return false;
        }
        self.normalize();
        true
    }

    /// Collapse one-part splits and merge a part that splits along its
    /// parent's axis into the parent.
    fn normalize(&mut self) {
        let Node::Split { axis, children, sizes } = self else { return };
        for c in children.iter_mut() {
            c.normalize();
        }
        let mut i = 0;
        while i < children.len() {
            let same_axis = matches!(&children[i], Node::Split { axis: a, .. } if a == axis);
            if same_axis {
                let Node::Split { children: inner, sizes: inner_sizes, .. } = children.remove(i) else { unreachable!() };
                let share = sizes.remove(i);
                let n = inner.len();
                for (k, (c, s)) in inner.into_iter().zip(inner_sizes).enumerate() {
                    children.insert(i + k, c);
                    sizes.insert(i + k, s * share);
                }
                i += n;
            } else {
                i += 1;
            }
        }
        if children.len() == 1 {
            *self = children.pop().expect("one child");
        }
    }

    /// The same tree with every leaf renamed by `f`; leaves it returns
    /// `None` for are left out (their space goes to their siblings). `None`
    /// when no leaf is left. Also repairs what a hand-edited or damaged
    /// saved layout may hold: missing, negative or surplus sizes become
    /// equal shares, empty splits disappear.
    pub fn map_leaves(&self, f: &mut impl FnMut(PaneId) -> Option<PaneId>) -> Option<Node> {
        match self {
            Node::Leaf(id) => f(*id).map(Node::Leaf),
            Node::Split { axis, children, sizes } => {
                let valid = sizes.len() == children.len() && sizes.iter().all(|s| s.is_finite() && *s > 0.0);
                let (mut kept, mut shares) = (Vec::new(), Vec::new());
                for (i, child) in children.iter().enumerate() {
                    if let Some(c) = child.map_leaves(f) {
                        kept.push(c);
                        shares.push(if valid { sizes[i] } else { 1.0 });
                    }
                }
                if kept.is_empty() {
                    return None;
                }
                let total: f32 = shares.iter().sum();
                let mut node = Node::Split { axis: *axis, children: kept, sizes: shares.into_iter().map(|s| s / total).collect() };
                node.normalize();
                Some(node)
            }
        }
    }

    /// Every leaf's rectangle and every divider, inside `rect`.
    pub fn layout(&self, rect: Rect) -> (Vec<(PaneId, Rect)>, Vec<Divider>) {
        let mut leaves = Vec::new();
        let mut dividers = Vec::new();
        self.layout_into(rect, &mut Vec::new(), &mut leaves, &mut dividers);
        (leaves, dividers)
    }

    fn layout_into(&self, rect: Rect, path: &mut Vec<usize>, leaves: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<Divider>) {
        match self {
            Node::Leaf(id) => leaves.push((*id, rect)),
            Node::Split { axis, children, sizes } => {
                let total: f32 = sizes.iter().sum::<f32>().max(EPS);
                let mut at = rect.start(*axis);
                for (i, (child, size)) in children.iter().zip(sizes).enumerate() {
                    let len = rect.len(*axis) * size / total;
                    let r = match axis {
                        Axis::Horizontal => Rect { x: at, w: len, ..rect },
                        Axis::Vertical => Rect { y: at, h: len, ..rect },
                    };
                    path.push(i);
                    child.layout_into(r, path, leaves, dividers);
                    path.pop();
                    at += len;
                    if i + 1 < children.len() {
                        dividers.push(Divider { path: path.clone(), index: i, axis: *axis, split: rect, at });
                    }
                }
            }
        }
    }

    /// The pane next to `from` in direction `dir`: the one sharing the most
    /// of `from`'s edge, the nearest on a tie.
    pub fn neighbor(&self, from: PaneId, dir: Direction) -> Option<PaneId> {
        let (leaves, _) = self.layout(Rect::FULL);
        let (_, f) = *leaves.iter().find(|(id, _)| *id == from)?;
        let axis = dir.axis();
        let edge = if dir.forward() { f.end(axis) } else { f.start(axis) };
        let (fx, fy) = f.center();
        leaves
            .iter()
            .filter(|(id, _)| *id != from)
            .filter(|(_, r)| {
                let near = if dir.forward() { r.start(axis) } else { r.end(axis) };
                (near - edge).abs() < EPS && r.overlap_across(&f, axis) > EPS
            })
            .map(|(id, r)| {
                let (cx, cy) = r.center();
                (*id, r.overlap_across(&f, axis), (cx - fx).powi(2) + (cy - fy).powi(2))
            })
            // `min_by` keeps the first of equals: the earlier pane wins a full tie.
            .min_by(|a, b| b.1.total_cmp(&a.1).then(a.2.total_cmp(&b.2)))
            .map(|(id, _, _)| id)
    }

    /// Move a divider of `target`'s area by `amount` (a share of the split)
    /// in direction `dir`: the one on that side of the pane, else the one on
    /// the other side. Returns false when no split along that axis contains
    /// `target`.
    pub fn resize(&mut self, target: PaneId, dir: Direction, amount: f32) -> bool {
        let Some((path, i)) = self.resize_target(target, dir.axis()) else { return false };
        let Some(Node::Split { sizes, .. }) = self.at_mut(&path) else { return false };
        // Divider between parts i and i+1, or i-1 and i.
        let index = if i + 1 < sizes.len() { i } else { i - 1 };
        let delta = if dir.forward() { amount } else { -amount };
        shift(sizes, index, delta);
        true
    }

    /// The nearest split along `axis` above `target`, and the index of the
    /// part holding it.
    fn resize_target(&self, target: PaneId, axis: Axis) -> Option<(Vec<usize>, usize)> {
        fn walk(node: &Node, target: PaneId, axis: Axis, path: &mut Vec<usize>, best: &mut Option<(Vec<usize>, usize)>) -> bool {
            match node {
                Node::Leaf(id) => *id == target,
                Node::Split { axis: a, children, .. } => {
                    for (i, c) in children.iter().enumerate() {
                        path.push(i);
                        let found = walk(c, target, axis, path, best);
                        path.pop();
                        if found {
                            if *a == axis && best.is_none() {
                                *best = Some((path.clone(), i));
                            }
                            return true;
                        }
                    }
                    false
                }
            }
        }
        let mut best = None;
        walk(self, target, axis, &mut Vec::new(), &mut best);
        best
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        let mut node = self;
        for &i in path {
            let Node::Split { children, .. } = node else { return None };
            node = children.get_mut(i)?;
        }
        Some(node)
    }

    /// Drag a divider: put the boundary between parts `index` and
    /// `index + 1` of the split at `path` at `fraction` (0..1) of the split's
    /// length, within limits.
    pub fn set_divider(&mut self, path: &[usize], index: usize, fraction: f32) {
        let Some(Node::Split { sizes, .. }) = self.at_mut(path) else { return };
        if index + 1 >= sizes.len() {
            return;
        }
        let total: f32 = sizes.iter().sum();
        let before: f32 = sizes[..index].iter().sum::<f32>() / total;
        let current = before + sizes[index] / total;
        shift(sizes, index, (fraction - current) * total);
    }

    /// Give every part of every split the same share.
    pub fn equalize(&mut self) {
        if let Node::Split { children, sizes, .. } = self {
            let n = sizes.len() as f32;
            sizes.iter_mut().for_each(|s| *s = 1.0 / n);
            children.iter_mut().for_each(Node::equalize);
        }
    }
}

/// Move `delta` of size from part `index + 1` to part `index` (negative:
/// the other way), keeping both above the minimum.
fn shift(sizes: &mut [f32], index: usize, delta: f32) {
    let total: f32 = sizes.iter().sum();
    let min = MIN_SHARE * total;
    let (lo, hi) = (min - sizes[index], sizes[index + 1] - min);
    if lo > hi {
        return; // both parts already at the minimum
    }
    let delta = delta.clamp(lo, hi);
    sizes[index] += delta;
    sizes[index + 1] -= delta;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn splitting_the_same_way_adds_siblings() {
        let mut t = Node::Leaf(1);
        assert!(t.split(1, 2, Direction::Right));
        assert!(t.split(2, 3, Direction::Right));
        let Node::Split { axis, children, sizes } = &t else { panic!() };
        assert_eq!(*axis, Axis::Horizontal);
        assert_eq!(children.len(), 3);
        assert_eq!(sizes, &[0.5, 0.25, 0.25]);
        assert_eq!(t.leaves(), [1, 2, 3]);
        // Left inserts before.
        assert!(t.split(1, 4, Direction::Left));
        assert_eq!(t.leaves(), [4, 1, 2, 3]);
        assert!(!t.split(99, 5, Direction::Left));
    }

    #[test]
    fn splitting_across_nests() {
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        t.split(2, 3, Direction::Down);
        let (leaves, dividers) = t.layout(Rect::FULL);
        assert_eq!(leaves[0], (1, Rect { x: 0.0, y: 0.0, w: 0.5, h: 1.0 }));
        assert_eq!(leaves[1], (2, Rect { x: 0.5, y: 0.0, w: 0.5, h: 0.5 }));
        assert_eq!(leaves[2], (3, Rect { x: 0.5, y: 0.5, w: 0.5, h: 0.5 }));
        assert_eq!(dividers.len(), 2);
        // The root's divider comes before the dividers inside its later parts.
        assert_eq!((dividers[0].path.as_slice(), dividers[0].axis, dividers[0].at), (&[][..], Axis::Horizontal, 0.5));
        assert_eq!((dividers[1].path.as_slice(), dividers[1].index, dividers[1].axis), (&[1usize][..], 0, Axis::Vertical));
    }

    #[test]
    fn removing_collapses_and_merges() {
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        t.split(2, 3, Direction::Down);
        t.split(3, 4, Direction::Right);
        // 1 | (2 / (3 | 4)): removing 2 leaves 1 | 3 | 4 in one split.
        assert!(t.remove(2));
        let Node::Split { axis, children, sizes } = &t else { panic!() };
        assert_eq!(*axis, Axis::Horizontal);
        assert_eq!(children.len(), 3);
        assert!(close(sizes.iter().sum(), 1.0));
        assert!(close(sizes[0], 0.5) && close(sizes[1], 0.25));
        assert_eq!(t.leaves(), [1, 3, 4]);
        assert!(t.remove(1));
        assert!(t.remove(3));
        assert_eq!(t, Node::Leaf(4));
        assert!(!t.remove(4), "the last pane closes the tab instead");
        assert!(!t.remove(7));
    }

    #[test]
    fn removed_space_goes_to_the_previous_part() {
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        t.split(2, 3, Direction::Right);
        t.remove(2);
        let Node::Split { sizes, .. } = &t else { panic!() };
        assert_eq!(sizes, &[0.75, 0.25]);
        t.remove(1);
        assert_eq!(t, Node::Leaf(3));
    }

    #[test]
    fn neighbors() {
        // 1 | (2 / 3)
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        t.split(2, 3, Direction::Down);
        assert_eq!(t.neighbor(1, Direction::Right), Some(2), "a full tie: the first one");
        assert_eq!(t.neighbor(2, Direction::Left), Some(1));
        assert_eq!(t.neighbor(3, Direction::Left), Some(1));
        assert_eq!(t.neighbor(2, Direction::Down), Some(3));
        assert_eq!(t.neighbor(3, Direction::Up), Some(2));
        assert_eq!(t.neighbor(1, Direction::Left), None);
        assert_eq!(t.neighbor(1, Direction::Up), None);
        // A bigger shared edge wins.
        t.resize(2, Direction::Down, 0.3);
        assert_eq!(t.neighbor(1, Direction::Right), Some(2));
    }

    #[test]
    fn resizing() {
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        assert!(t.resize(1, Direction::Right, 0.1));
        let Node::Split { sizes, .. } = &t else { panic!() };
        assert!(close(sizes[0], 0.6));
        // The last part moves its left divider.
        assert!(t.resize(2, Direction::Right, 0.1));
        let Node::Split { sizes, .. } = &t else { panic!() };
        assert!(close(sizes[0], 0.7));
        assert!(!t.resize(1, Direction::Up, 0.1), "no vertical split");
        // Clamped to the minimum share.
        t.resize(1, Direction::Right, 5.0);
        let Node::Split { sizes, .. } = &t else { panic!() };
        assert!(close(sizes[1], MIN_SHARE));
    }

    #[test]
    fn mapping_leaves_renames_drops_and_repairs() {
        // 1 | (2 / 3)
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        t.split(2, 3, Direction::Down);
        assert_eq!(t.map_leaves(&mut |id| Some(id * 10)).unwrap().leaves(), [10, 20, 30]);
        // Without 2, 3 takes the whole right half: 1 | 3.
        let m = t.map_leaves(&mut |id| (id != 2).then_some(id)).unwrap();
        assert_eq!(m, Node::Split { axis: Axis::Horizontal, children: vec![Node::Leaf(1), Node::Leaf(3)], sizes: vec![0.5, 0.5] });
        // Without 1, the vertical split is all that's left.
        let m = t.map_leaves(&mut |id| (id != 1).then_some(id)).unwrap();
        assert!(matches!(&m, Node::Split { axis: Axis::Vertical, children, .. } if children.len() == 2));
        assert_eq!(t.map_leaves(&mut |id| (id == 3).then_some(id)), Some(Node::Leaf(3)));
        assert_eq!(t.map_leaves(&mut |_| None), None);
        // Bad sizes become equal shares.
        let bad = Node::Split { axis: Axis::Horizontal, children: vec![Node::Leaf(1), Node::Leaf(2)], sizes: vec![f32::NAN] };
        let Some(Node::Split { sizes, .. }) = bad.map_leaves(&mut Some) else { panic!() };
        assert_eq!(sizes, [0.5, 0.5]);
        let empty = Node::Split { axis: Axis::Vertical, children: vec![], sizes: vec![] };
        assert_eq!(empty.map_leaves(&mut Some), None);
    }

    #[test]
    fn trees_round_trip_through_json() {
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        t.split(2, 3, Direction::Down);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Node>(&json).unwrap(), t);
        assert!(json.contains("\"horizontal\""), "{json}");
    }

    #[test]
    fn dragging_a_divider() {
        let mut t = Node::Leaf(1);
        t.split(1, 2, Direction::Right);
        t.split(2, 3, Direction::Right);
        t.set_divider(&[], 1, 0.9);
        let Node::Split { sizes, .. } = &t else { panic!() };
        assert!(close(sizes[0], 0.5) && close(sizes[1], 0.4) && close(sizes[2], 0.1));
        t.set_divider(&[], 0, 0.0);
        let Node::Split { sizes, .. } = &t else { panic!() };
        assert!(close(sizes[0], MIN_SHARE));
        t.equalize();
        let Node::Split { sizes, .. } = &t else { panic!() };
        assert!(sizes.iter().all(|s| close(*s, 1.0 / 3.0)));
    }
}
