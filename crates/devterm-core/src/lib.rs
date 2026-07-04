//! DevTerm domain core.
//!
//! This crate is **pure logic with no I/O**: the layout tree, focus navigation, and
//! geometry. It knows nothing about `wgpu`/`winit` (rendering/windowing) or ConPTY
//! (processes). That keeps the heart of the app fully unit- and property-testable — see
//! `tests/layout_props.rs`.
//!
//! The central data structure is the [`LayoutTree`]: an N-ary tree of splits and panes.
//! Every operation (split, close, resize, move focus) is a pure transformation of that
//! tree with checkable invariants (see [`LayoutTree::validate`]).

#![forbid(unsafe_code)]

pub mod geometry;
pub mod id;
pub mod layout;

pub use geometry::Rect;
pub use id::{IdGen, PaneId, TabId};
pub use layout::{Child, Direction, LayoutError, LayoutNode, LayoutTree, SplitDirection};
