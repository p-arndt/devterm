//! The public layout handle: [`LayoutTree`] plus its split / focus / resize / close
//! operations and the [`LayoutError`] they can return.

use super::{Direction, LayoutNode, SplitDirection};
use crate::geometry::{Rect, overlap_1d};
use crate::id::PaneId;

/// Smallest floating-point slack for geometric comparisons in the unit square.
const EPS: f32 = 1e-4;

/// Errors from layout operations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutError {
    /// The last remaining pane cannot be closed.
    CannotCloseLastPane,
    /// The given pane does not exist in the tree.
    PaneNotFound,
}

impl core::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let msg = match self {
            LayoutError::CannotCloseLastPane => "cannot close the last pane",
            LayoutError::PaneNotFound => "pane not found",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for LayoutError {}

/// A tab's layout tree together with the currently focused pane.
///
/// The tree is never empty: it starts with exactly one pane and always keeps >= 1 pane.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LayoutTree {
    root: LayoutNode,
    focused: PaneId,
}

impl LayoutTree {
    /// New tree with a single pane that immediately holds focus.
    pub fn new(first: PaneId) -> Self {
        Self {
            root: LayoutNode::leaf(first),
            focused: first,
        }
    }

    /// Root node (for rendering/serialization).
    pub fn root(&self) -> &LayoutNode {
        &self.root
    }

    /// Currently focused pane.
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// All panes in arrangement order.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.root.collect_leaves(&mut out);
        out
    }

    /// Number of panes (always >= 1).
    pub fn pane_count(&self) -> usize {
        let mut out = Vec::new();
        self.root.collect_leaves(&mut out);
        out.len()
    }

    /// Does the tree contain this pane?
    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.contains(pane)
    }

    /// Rectangles of all panes within `area` (e.g. [`Rect::UNIT`] or the window's pixel
    /// rectangle).
    pub fn compute(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.root.compute_into(area, &mut out);
        out
    }

    /// Splits the **focused** pane in `direction` and inserts `new_pane`. Focus moves to the
    /// new pane.
    ///
    /// # Panics
    /// If `new_pane` already exists in the tree (IDs must be fresh).
    pub fn split(&mut self, direction: SplitDirection, new_pane: PaneId) {
        assert!(!self.contains(new_pane), "new_pane must be a fresh ID");
        let target = self.focused;
        let ok = self.root.split_leaf(target, direction, new_pane);
        debug_assert!(ok, "the focused pane must exist in the tree");
        self.focused = new_pane;
    }

    /// Splits an arbitrary `target` pane. Returns `false` if it does not exist.
    ///
    /// # Panics
    /// If `new_pane` already exists in the tree.
    pub fn split_pane(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        new_pane: PaneId,
    ) -> bool {
        assert!(!self.contains(new_pane), "new_pane must be a fresh ID");
        let ok = self.root.split_leaf(target, direction, new_pane);
        if ok {
            self.focused = new_pane;
        }
        ok
    }

    /// Closes a pane. The tree collapses redundant splits automatically.
    ///
    /// If `pane` was focused, focus moves to the first remaining pane in arrangement order.
    pub fn close(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        if !self.contains(pane) {
            return Err(LayoutError::PaneNotFound);
        }
        // Root is a single leaf -> last pane.
        if let LayoutNode::Leaf(_) = self.root {
            return Err(LayoutError::CannotCloseLastPane);
        }
        let removed = self.root.remove(pane);
        debug_assert!(removed, "contains() was true, remove() must succeed");

        if self.focused == pane {
            self.focused = self.panes().first().copied().expect("tree is never empty");
        }
        Ok(())
    }

    /// Sets focus explicitly. Returns `false` if `pane` does not exist.
    pub fn focus(&mut self, pane: PaneId) -> bool {
        if self.contains(pane) {
            self.focused = pane;
            true
        } else {
            false
        }
    }

    /// The geometric neighbor of the focused pane in direction `dir`.
    ///
    /// Candidates are panes lying on the given side that overlap the focused pane on the
    /// cross axis. The largest overlap wins; ties are broken by the smallest gap.
    pub fn neighbor(&self, dir: Direction) -> Option<PaneId> {
        let rects = self.compute(Rect::UNIT);
        let from = rects.iter().find(|(id, _)| *id == self.focused)?.1;

        let mut best: Option<(PaneId, f32, f32)> = None; // (id, overlap, gap)
        for (id, r) in &rects {
            if *id == self.focused {
                continue;
            }
            let (on_side, gap, overlap) = match dir {
                Direction::Left => (
                    r.x + r.w <= from.x + EPS,
                    from.x - (r.x + r.w),
                    overlap_1d(from.y, from.h, r.y, r.h),
                ),
                Direction::Right => (
                    r.x >= from.x + from.w - EPS,
                    r.x - (from.x + from.w),
                    overlap_1d(from.y, from.h, r.y, r.h),
                ),
                Direction::Up => (
                    r.y + r.h <= from.y + EPS,
                    from.y - (r.y + r.h),
                    overlap_1d(from.x, from.w, r.x, r.w),
                ),
                Direction::Down => (
                    r.y >= from.y + from.h - EPS,
                    r.y - (from.y + from.h),
                    overlap_1d(from.x, from.w, r.x, r.w),
                ),
            };
            if on_side && overlap > EPS {
                let cand = (*id, overlap, gap.max(0.0));
                best = Some(match best {
                    None => cand,
                    Some(b) => {
                        if cand.1 > b.1 + EPS || (cand.1 >= b.1 - EPS && cand.2 < b.2) {
                            cand
                        } else {
                            b
                        }
                    }
                });
            }
        }
        best.map(|(id, _, _)| id)
    }

    /// Moves focus to the geometric neighbor in `dir`, if any. Returns `true` if focus
    /// changed.
    pub fn move_focus(&mut self, dir: Direction) -> bool {
        if let Some(next) = self.neighbor(dir) {
            self.focused = next;
            true
        } else {
            false
        }
    }

    /// Grows (`factor > 1`) or shrinks (`factor < 1`) the focused pane along the axis of
    /// `dir` by adjusting the weight at the nearest matching ancestor split. Siblings
    /// shrink/grow proportionally.
    pub fn resize(&mut self, dir: Direction, factor: f32) {
        let mut applied = false;
        self.root
            .resize_path(self.focused, dir.axis(), factor, &mut applied);
    }

    /// Interactive "border follows the arrow" resize. Moves the focused pane's border in
    /// the pressed `dir`: if a pane borders it on that side, the focused pane **grows**
    /// into it (the shared border slides toward `dir`); if `dir` points at the outer edge
    /// (no neighbor there), the pane **shrinks**. `step` is the grow factor (> 1); shrinking
    /// uses its reciprocal, so the two arrows on an axis are exact inverses.
    ///
    /// Example: with `[A | B]` and `B` focused, `Left` grows `B` leftward (toward `A`) and
    /// `Right` shrinks it back — the border tracks the key either way.
    pub fn resize_directional(&mut self, dir: Direction, step: f32) {
        let factor = if self.neighbor(dir).is_some() {
            step
        } else {
            1.0 / step
        };
        self.resize(dir, factor);
    }

    /// Checks the tree's structural invariants. Useful in tests and `debug_assert!`
    /// contexts.
    ///
    /// Guarantees: every split has >= 2 children with positive weights, all pane IDs are
    /// unique, and focus is on an existing pane.
    pub fn validate(&self) -> Result<(), &'static str> {
        self.root.validate()?;
        let panes = self.panes();
        let mut seen = std::collections::HashSet::new();
        for p in &panes {
            if !seen.insert(*p) {
                return Err("duplicate pane ID");
            }
        }
        if !panes.contains(&self.focused) {
            return Err("focus points at a non-existent pane");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u64) -> PaneId {
        PaneId(n)
    }

    #[test]
    fn single_pane_fills_area() {
        let t = LayoutTree::new(p(1));
        assert_eq!(t.pane_count(), 1);
        let rects = t.compute(Rect::UNIT);
        assert_eq!(rects, vec![(p(1), Rect::UNIT)]);
        t.validate().unwrap();
    }

    #[test]
    fn horizontal_split_halves_width() {
        let mut t = LayoutTree::new(p(1));
        t.split(SplitDirection::Horizontal, p(2));
        assert_eq!(t.focused(), p(2));
        let rects = t.compute(Rect::UNIT);
        let r1 = rects.iter().find(|(id, _)| *id == p(1)).unwrap().1;
        let r2 = rects.iter().find(|(id, _)| *id == p(2)).unwrap().1;
        assert!((r1.w - 0.5).abs() < 1e-6 && (r2.w - 0.5).abs() < 1e-6);
        assert!((r1.h - 1.0).abs() < 1e-6);
        t.validate().unwrap();
    }

    #[test]
    fn same_direction_split_stays_flat_and_even() {
        // Three consecutive horizontal splits -> one flat split of three thirds.
        let mut t = LayoutTree::new(p(1));
        t.split(SplitDirection::Horizontal, p(2));
        // Focus is on p(2); split horizontally again after refocusing p(1).
        t.focus(p(1));
        t.split(SplitDirection::Horizontal, p(3));
        match t.root() {
            LayoutNode::Split {
                direction,
                children,
            } => {
                assert_eq!(*direction, SplitDirection::Horizontal);
                assert_eq!(children.len(), 3, "flat, not nested");
            }
            _ => panic!("root should be a split"),
        }
        let rects = t.compute(Rect::UNIT);
        for (_, r) in rects {
            assert!((r.w - 1.0 / 3.0).abs() < 1e-6);
        }
        t.validate().unwrap();
    }

    #[test]
    fn close_collapses_and_moves_focus() {
        let mut t = LayoutTree::new(p(1));
        t.split(SplitDirection::Vertical, p(2)); // focus -> p(2)
        t.close(p(2)).unwrap();
        assert_eq!(t.pane_count(), 1);
        assert_eq!(t.focused(), p(1), "focus moves to the remaining pane");
        // Split collapses: the root is a leaf again.
        assert!(matches!(t.root(), LayoutNode::Leaf(_)));
        t.validate().unwrap();
    }

    #[test]
    fn cannot_close_last_pane() {
        let mut t = LayoutTree::new(p(1));
        assert_eq!(t.close(p(1)), Err(LayoutError::CannotCloseLastPane));
    }

    #[test]
    fn close_unknown_pane_errors() {
        let mut t = LayoutTree::new(p(1));
        assert_eq!(t.close(p(99)), Err(LayoutError::PaneNotFound));
    }

    #[test]
    fn neighbor_navigation_left_right() {
        // p1 | p2  (horizontal)
        let mut t = LayoutTree::new(p(1));
        t.split(SplitDirection::Horizontal, p(2));
        t.focus(p(1));
        assert_eq!(t.neighbor(Direction::Right), Some(p(2)));
        assert_eq!(t.neighbor(Direction::Left), None);
        t.focus(p(2));
        assert_eq!(t.neighbor(Direction::Left), Some(p(1)));
        assert_eq!(t.neighbor(Direction::Up), None);
    }

    #[test]
    fn resize_changes_share() {
        let mut t = LayoutTree::new(p(1));
        t.split(SplitDirection::Horizontal, p(2)); // focus -> p(2), 0.5 each
        t.resize(Direction::Right, 2.0); // double p(2)'s weight -> 2:1
        let rects = t.compute(Rect::UNIT);
        let r2 = rects.iter().find(|(id, _)| *id == p(2)).unwrap().1;
        assert!((r2.w - 2.0 / 3.0).abs() < 1e-6);
        t.validate().unwrap();
    }

    #[test]
    fn resize_shrink_is_inverse_of_grow() {
        // The app maps Right/Down -> grow and Left/Up -> shrink with reciprocal factors,
        // so growing then shrinking by 1/factor must restore the original share.
        let mut t = LayoutTree::new(p(1));
        t.split(SplitDirection::Horizontal, p(2)); // 0.5 each
        let width = |t: &LayoutTree| t.compute(Rect::UNIT).iter().find(|(id, _)| *id == p(2)).unwrap().1.w;
        let before = width(&t);
        t.resize(Direction::Right, 1.1); // grow
        assert!(width(&t) > before + 1e-4, "grow should widen the focused pane");
        t.resize(Direction::Left, 1.0 / 1.1); // shrink (opposite arrow, same axis)
        assert!((width(&t) - before).abs() < 1e-6, "shrink undoes the grow");
        t.validate().unwrap();
    }

    #[test]
    fn resize_directional_border_follows_the_arrow() {
        // [1 | 2] with the RIGHT pane (2) focused.
        let mut t = LayoutTree::new(p(1));
        t.split(SplitDirection::Horizontal, p(2)); // focus -> p(2)
        let width = |t: &LayoutTree| {
            t.compute(Rect::UNIT).iter().find(|(id, _)| *id == p(2)).unwrap().1.w
        };
        let before = width(&t);

        // Left points at the neighbor (pane 1) -> the right pane grows leftward.
        t.resize_directional(Direction::Left, 1.1);
        assert!(width(&t) > before + 1e-4, "Left grows the right pane toward its neighbor");

        // Right points at the window edge (no neighbor) -> shrink, undoing the grow.
        t.resize_directional(Direction::Right, 1.1);
        assert!((width(&t) - before).abs() < 1e-6, "Right shrinks back — border tracks the key");
        t.validate().unwrap();
    }
}
