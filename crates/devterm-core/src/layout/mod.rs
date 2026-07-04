//! The layout tree: N-ary splits over panes, plus focus.
//!
//! # Model
//!
//! A [`LayoutNode`] is either a single pane ([`LayoutNode::Leaf`]) or a
//! [`LayoutNode::Split`] with >= 2 children. Each child carries a **relative weight**; a
//! child's visible share is `weight / sum_of_siblings`. Weights therefore need *not* sum to
//! 1 — which avoids floating-point drift under repeated splitting/closing.
//!
//! # Convention
//!
//! - [`SplitDirection::Horizontal`]: children are placed **side by side** (left -> right),
//!   the divider is vertical.
//! - [`SplitDirection::Vertical`]: children are **stacked** (top -> bottom), the divider is
//!   horizontal.
//!
//! Every mutating operation preserves the invariants in [`LayoutTree::validate`].
//!
//! # Structure
//!
//! The recursive [`LayoutNode`]/[`Child`] tree and its rect computation live in the `node`
//! submodule; the public [`LayoutTree`] handle (split/focus/resize/close) and [`LayoutError`]
//! live in the `tree` submodule. Both are re-exported here so the `devterm_core::layout::*`
//! paths stay flat.

mod node;
mod tree;

pub use node::{Child, LayoutNode};
pub use tree::{Gutter, GutterId, LayoutError, LayoutTree};

/// Arrangement direction of a split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SplitDirection {
    /// Children side by side (left -> right).
    Horizontal,
    /// Children stacked (top -> bottom).
    Vertical,
}

/// Movement direction for focus navigation and interactive resize.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// The axis this direction acts along.
    pub(crate) fn axis(self) -> SplitDirection {
        match self {
            Direction::Left | Direction::Right => SplitDirection::Horizontal,
            Direction::Up | Direction::Down => SplitDirection::Vertical,
        }
    }
}
