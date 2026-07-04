//! Property tests for the layout tree.
//!
//! A random sequence of operations (split / close / focus / resize / move) is applied to a
//! tree that starts with one pane. After *every* operation, the structural and geometric
//! invariants must hold.

use devterm_core::{Direction, LayoutTree, PaneId, Rect, SplitDirection};
use proptest::prelude::*;

/// An abstract operation; concrete pane/child selection happens relative to the current
/// state so that every operation stays valid.
#[derive(Clone, Copy, Debug)]
enum Op {
    Split { horizontal: bool, target_idx: usize },
    Close { idx: usize },
    Focus { idx: usize },
    Move { dir: u8 },
    Resize { grow: bool },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (any::<bool>(), 0usize..8).prop_map(|(h, i)| Op::Split {
            horizontal: h,
            target_idx: i
        }),
        (0usize..8).prop_map(|i| Op::Close { idx: i }),
        (0usize..8).prop_map(|i| Op::Focus { idx: i }),
        (0u8..4).prop_map(|d| Op::Move { dir: d }),
        any::<bool>().prop_map(|g| Op::Resize { grow: g }),
    ]
}

fn dir_from(d: u8) -> Direction {
    match d % 4 {
        0 => Direction::Left,
        1 => Direction::Right,
        2 => Direction::Up,
        _ => Direction::Down,
    }
}

/// Checks all geometric invariants for the computed rectangles.
fn assert_geometry(tree: &LayoutTree) {
    let rects = tree.compute(Rect::UNIT);
    assert_eq!(rects.len(), tree.pane_count(), "one rectangle per pane");

    let mut area_sum = 0.0;
    for (_, r) in &rects {
        assert!(r.w > 0.0 && r.h > 0.0, "panes have positive area: {r:?}");
        assert!(
            r.x >= -1e-4 && r.y >= -1e-4 && r.x + r.w <= 1.0 + 1e-4 && r.y + r.h <= 1.0 + 1e-4,
            "pane lies within the unit square: {r:?}"
        );
        area_sum += r.area();
    }
    // Panes tile the area with no gaps and no overlaps.
    assert!((area_sum - 1.0).abs() < 1e-3, "areas sum to 1: {area_sum}");
    for (i, (_, a)) in rects.iter().enumerate() {
        for (_, b) in rects.iter().skip(i + 1) {
            assert!(
                a.intersection_area(b) < 1e-4,
                "panes do not overlap: {a:?} vs {b:?}"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// After every operation, all invariants hold.
    #[test]
    fn invariants_hold_under_random_ops(ops in prop::collection::vec(op_strategy(), 0..60)) {
        let mut tree = LayoutTree::new(PaneId(1));
        let mut next_id = 2u64;

        for op in ops {
            let panes = tree.panes();
            match op {
                Op::Split { horizontal, target_idx } => {
                    let target = panes[target_idx % panes.len()];
                    let dir = if horizontal {
                        SplitDirection::Horizontal
                    } else {
                        SplitDirection::Vertical
                    };
                    let id = PaneId(next_id);
                    next_id += 1;
                    prop_assert!(tree.split_pane(target, dir, id));
                }
                Op::Close { idx } => {
                    let target = panes[idx % panes.len()];
                    // Closing the last pane may be attempted, but returns an error.
                    let _ = tree.close(target);
                }
                Op::Focus { idx } => {
                    let target = panes[idx % panes.len()];
                    prop_assert!(tree.focus(target));
                }
                Op::Move { dir } => {
                    let _ = tree.move_focus(dir_from(dir));
                }
                Op::Resize { grow } => {
                    tree.resize(Direction::Right, if grow { 1.5 } else { 0.66 });
                    tree.resize(Direction::Down, if grow { 1.5 } else { 0.66 });
                }
            }

            prop_assert!(tree.validate().is_ok(), "invariant violated: {:?}", tree.validate());
            prop_assert!(tree.pane_count() >= 1);
            assert_geometry(&tree);
        }
    }

    /// The computed state depends only on the tree, not on the order/granularity in which
    /// operations are grouped — the model-level basis of the anti-flicker guarantee: the
    /// same logical state always yields the same grid.
    #[test]
    fn compute_is_deterministic(ops in prop::collection::vec(op_strategy(), 0..40)) {
        let build = || {
            let mut tree = LayoutTree::new(PaneId(1));
            let mut next_id = 2u64;
            for op in &ops {
                let panes = tree.panes();
                if let Op::Split { horizontal, target_idx } = op {
                    let target = panes[target_idx % panes.len()];
                    let dir = if *horizontal {
                        SplitDirection::Horizontal
                    } else {
                        SplitDirection::Vertical
                    };
                    let id = PaneId(next_id);
                    next_id += 1;
                    tree.split_pane(target, dir, id);
                } else if let Op::Close { idx } = op {
                    let target = panes[idx % panes.len()];
                    let _ = tree.close(target);
                }
            }
            tree
        };
        let a = build();
        let b = build();
        prop_assert_eq!(a.compute(Rect::UNIT), b.compute(Rect::UNIT));
    }
}
