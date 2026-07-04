//! The recursive layout tree node: weighted N-ary splits over panes and the geometry that
//! turns a subtree into per-pane rectangles.

use super::SplitDirection;
use super::tree::{Gutter, GutterId};
use crate::geometry::Rect;
use crate::id::PaneId;

/// Minimum weight so a pane never collapses to zero when shrunk.
const MIN_WEIGHT: f32 = 0.05;

/// Half-thickness of a gutter's hit rectangle, as a fraction of the parent split's length
/// along its axis. The full strip is twice this, centred on the boundary.
const GUTTER_HALF_FRACTION: f32 = 0.01;

/// A weighted child inside a split.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Child {
    /// Relative weight against its siblings (> 0).
    pub weight: f32,
    pub node: LayoutNode,
}

/// A node of the layout tree.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LayoutNode {
    /// Leaf: exactly one pane.
    Leaf(PaneId),
    /// Split with >= 2 children.
    Split {
        direction: SplitDirection,
        children: Vec<Child>,
    },
}

impl LayoutNode {
    pub(crate) fn leaf(id: PaneId) -> Self {
        LayoutNode::Leaf(id)
    }

    /// Collects all pane IDs in arrangement order (depth-first, leftmost first).
    pub(crate) fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            LayoutNode::Leaf(id) => out.push(*id),
            LayoutNode::Split { children, .. } => {
                for c in children {
                    c.node.collect_leaves(out);
                }
            }
        }
    }

    pub(crate) fn contains(&self, target: PaneId) -> bool {
        match self {
            LayoutNode::Leaf(id) => *id == target,
            LayoutNode::Split { children, .. } => children.iter().any(|c| c.node.contains(target)),
        }
    }

    /// Computes the pixel/unit rectangles of every pane within `rect`.
    pub(crate) fn compute_into(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            LayoutNode::Leaf(id) => out.push((*id, rect)),
            LayoutNode::Split {
                direction,
                children,
            } => {
                let total: f32 = children.iter().map(|c| c.weight).sum();
                let mut offset = 0.0;
                for c in children {
                    let frac = c.weight / total;
                    let child_rect = match direction {
                        SplitDirection::Horizontal => {
                            Rect::new(rect.x + offset, rect.y, rect.w * frac, rect.h)
                        }
                        SplitDirection::Vertical => {
                            Rect::new(rect.x, rect.y + offset, rect.w, rect.h * frac)
                        }
                    };
                    offset += match direction {
                        SplitDirection::Horizontal => rect.w * frac,
                        SplitDirection::Vertical => rect.h * frac,
                    };
                    c.node.compute_into(child_rect, out);
                }
            }
        }
    }

    /// Splits the leaf `target`. Returns `true` if `target` was found.
    ///
    /// If `target` already sits directly inside a split of the same direction, the new pane
    /// is inserted as an equal-weight sibling *next to it* (tmux-style flat, even splits).
    /// Otherwise the leaf is replaced by a new binary split.
    pub(crate) fn split_leaf(
        &mut self,
        target: PaneId,
        direction: SplitDirection,
        new_pane: PaneId,
    ) -> bool {
        match self {
            LayoutNode::Leaf(id) => {
                if *id == target {
                    let old = *id;
                    *self = LayoutNode::Split {
                        direction,
                        children: vec![
                            Child {
                                weight: 1.0,
                                node: LayoutNode::leaf(old),
                            },
                            Child {
                                weight: 1.0,
                                node: LayoutNode::leaf(new_pane),
                            },
                        ],
                    };
                    true
                } else {
                    false
                }
            }
            LayoutNode::Split {
                direction: dir,
                children,
            } => {
                // Same direction + target is a direct leaf child -> append as a sibling.
                if *dir == direction
                    && let Some(pos) = children
                        .iter()
                        .position(|c| matches!(&c.node, LayoutNode::Leaf(id) if *id == target))
                {
                    let weight = children[pos].weight;
                    children.insert(
                        pos + 1,
                        Child {
                            weight,
                            node: LayoutNode::leaf(new_pane),
                        },
                    );
                    return true;
                }
                for c in children.iter_mut() {
                    if c.node.split_leaf(target, direction, new_pane) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Removes `target` from this subtree. Returns `true` if removed. Collapses splits that
    /// would be left with a single child.
    ///
    /// Assumes `self` is a split (the root-leaf case is handled in [`LayoutTree::close`]).
    ///
    /// [`LayoutTree::close`]: super::LayoutTree::close
    pub(crate) fn remove(&mut self, target: PaneId) -> bool {
        let LayoutNode::Split { children, .. } = self else {
            return false;
        };

        let removed = if let Some(pos) = children
            .iter()
            .position(|c| matches!(&c.node, LayoutNode::Leaf(id) if *id == target))
        {
            children.remove(pos);
            true
        } else {
            children.iter_mut().any(|c| c.node.remove(target))
        };

        if !removed {
            return false;
        }

        // A split left with a single child -> replace it by that child (the child's weight
        // in the parent is preserved, since only `node` is swapped).
        if children.len() == 1 {
            let only = children.pop().expect("len == 1");
            *self = only.node;
        }
        true
    }

    /// Along the path to `pane`, scales the weight at the **nearest** ancestor split of axis
    /// `axis`. `applied` prevents a second application further up.
    pub(crate) fn resize_path(
        &mut self,
        pane: PaneId,
        axis: SplitDirection,
        factor: f32,
        applied: &mut bool,
    ) -> bool {
        match self {
            LayoutNode::Leaf(id) => *id == pane,
            LayoutNode::Split {
                direction,
                children,
            } => {
                for c in children.iter_mut() {
                    if c.node.resize_path(pane, axis, factor, applied) {
                        if !*applied && *direction == axis {
                            c.weight = (c.weight * factor).max(MIN_WEIGHT);
                            *applied = true;
                        }
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Computes the rectangles the direct children of this split occupy within `rect`, in
    /// arrangement order. Empty for a leaf.
    fn child_rects(&self, rect: Rect) -> Vec<Rect> {
        let LayoutNode::Split {
            direction,
            children,
        } = self
        else {
            return Vec::new();
        };
        let total: f32 = children.iter().map(|c| c.weight).sum();
        let mut offset = 0.0;
        let mut out = Vec::with_capacity(children.len());
        for c in children {
            let frac = c.weight / total;
            let cr = match direction {
                SplitDirection::Horizontal => {
                    Rect::new(rect.x + offset, rect.y, rect.w * frac, rect.h)
                }
                SplitDirection::Vertical => {
                    Rect::new(rect.x, rect.y + offset, rect.w, rect.h * frac)
                }
            };
            offset += match direction {
                SplitDirection::Horizontal => rect.w * frac,
                SplitDirection::Vertical => rect.h * frac,
            };
            out.push(cr);
        }
        out
    }

    /// Emits a [`Gutter`] for every interior split boundary in this subtree within `rect`.
    ///
    /// `next` is the running boundary ordinal in a deterministic pre-order traversal: at each
    /// split the boundaries between its own adjacent children are numbered first, then the
    /// children are visited left-to-right. [`Self::drag_gutter`] walks the tree in exactly the
    /// same order, so a [`GutterId`] round-trips to the same boundary in an unchanged tree.
    pub(crate) fn gutters_into(&self, rect: Rect, next: &mut u32, out: &mut Vec<Gutter>) {
        let LayoutNode::Split {
            direction,
            children,
        } = self
        else {
            return;
        };
        let rects = self.child_rects(rect);
        // One boundary sits on the trailing edge of every child but the last.
        let interior = children.len().saturating_sub(1);
        for r in rects.iter().take(interior) {
            let id = GutterId(*next);
            *next += 1;
            let bounds = match direction {
                SplitDirection::Horizontal => {
                    let bx = r.x + r.w;
                    let half = rect.w * GUTTER_HALF_FRACTION;
                    Rect::new(bx - half, rect.y, half * 2.0, rect.h)
                }
                SplitDirection::Vertical => {
                    let by = r.y + r.h;
                    let half = rect.h * GUTTER_HALF_FRACTION;
                    Rect::new(rect.x, by - half, rect.w, half * 2.0)
                }
            };
            out.push(Gutter {
                bounds,
                axis: *direction,
                id,
            });
        }
        for (c, cr) in children.iter().zip(rects.iter()) {
            c.node.gutters_into(*cr, next, out);
        }
    }

    /// Shifts weight across the boundary identified by `target`, growing the lower-coordinate
    /// child by `delta` fraction of the parent split's length and shrinking its higher
    /// neighbour by the same, clamped so neither drops below [`MIN_WEIGHT`]. `next` numbers
    /// boundaries exactly as [`Self::gutters_into`] does. Returns `true` once the boundary is
    /// found and moved.
    pub(crate) fn drag_gutter(&mut self, target: GutterId, next: &mut u32, delta: f32) -> bool {
        let LayoutNode::Split { children, .. } = self else {
            return false;
        };
        let n = children.len();
        for i in 0..n.saturating_sub(1) {
            let id = GutterId(*next);
            *next += 1;
            if id == target {
                let a = children[i].weight;
                let b = children[i + 1].weight;
                let pair = a + b;
                // `delta` is a fraction of the whole split; weights map linearly to length,
                // so a fraction of the total weight moves the boundary that far.
                let total: f32 = children.iter().map(|c| c.weight).sum();
                let lo = MIN_WEIGHT;
                let hi = (pair - MIN_WEIGHT).max(MIN_WEIGHT);
                let na = (a + delta * total).clamp(lo, hi);
                children[i].weight = na;
                children[i + 1].weight = pair - na;
                return true;
            }
        }
        for c in children.iter_mut() {
            if c.node.drag_gutter(target, next, delta) {
                return true;
            }
        }
        false
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        match self {
            LayoutNode::Leaf(_) => Ok(()),
            LayoutNode::Split { children, .. } => {
                if children.len() < 2 {
                    return Err("split with fewer than 2 children");
                }
                for c in children {
                    if !c.weight.is_finite() || c.weight <= 0.0 {
                        return Err("non-positive weight");
                    }
                    c.node.validate()?;
                }
                Ok(())
            }
        }
    }
}
