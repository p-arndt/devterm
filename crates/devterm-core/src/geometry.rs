//! Axis-aligned rectangles for layout computation.
//!
//! Coordinates are deliberately unitless (`f32`). The layout tree computes in a normalized
//! unit square ([`Rect::UNIT`]); the render layer scales the result into pixels only late.
//! That keeps the core logic independent of DPI and window size.

/// An axis-aligned rectangle with its origin at the top-left.
#[derive(Clone, Copy, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// The normalized unit square `(0,0)..(1,1)` — the layout's root area.
    pub const UNIT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Center point of the rectangle.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    /// Area of the rectangle.
    pub fn area(&self) -> f32 {
        self.w * self.h
    }

    /// Whether the rectangle contains the point `(px, py)` (edges inclusive).
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    /// Overlap area with `other` (0.0 if disjoint).
    pub fn intersection_area(&self, other: &Rect) -> f32 {
        let ox = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let oy = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        if ox > 0.0 && oy > 0.0 { ox * oy } else { 0.0 }
    }
}

/// Length of the 1D overlap of two intervals `[a0, a0+alen]` and `[b0, b0+blen]`.
/// Negative/zero when they do not overlap.
pub(crate) fn overlap_1d(a0: f32, alen: f32, b0: f32, blen: f32) -> f32 {
    (a0 + alen).min(b0 + blen) - a0.max(b0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_basics() {
        assert_eq!(Rect::UNIT.area(), 1.0);
        assert_eq!(Rect::UNIT.center(), (0.5, 0.5));
        assert!(Rect::UNIT.contains(0.5, 0.5));
        assert!(!Rect::UNIT.contains(1.5, 0.5));
    }

    #[test]
    fn intersection() {
        let a = Rect::new(0.0, 0.0, 0.5, 1.0);
        let b = Rect::new(0.5, 0.0, 0.5, 1.0);
        assert_eq!(a.intersection_area(&b), 0.0); // only share the edge
        let c = Rect::new(0.25, 0.0, 0.5, 1.0);
        assert!((a.intersection_area(&c) - 0.25).abs() < 1e-6);
    }
}
