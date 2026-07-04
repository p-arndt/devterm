//! Typed, collision-free IDs for panes and tabs.

/// Stable identifier of a pane within a session.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PaneId(pub u64);

/// Stable identifier of a tab/workspace within a session.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TabId(pub u64);

/// Monotonically increasing generator for [`PaneId`]/[`TabId`].
///
/// IDs are never reused — not even after a pane is closed. That avoids subtle bugs where
/// a late-arriving PTY event would be attributed to the wrong (new) pane.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IdGen {
    next_pane: u64,
    next_tab: u64,
}

impl IdGen {
    /// New generator; the first ID handed out for each kind is `1`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Hands out the next guaranteed-unused [`PaneId`].
    pub fn next_pane(&mut self) -> PaneId {
        self.next_pane += 1;
        PaneId(self.next_pane)
    }

    /// Hands out the next guaranteed-unused [`TabId`].
    pub fn next_tab(&mut self) -> TabId {
        self.next_tab += 1;
        TabId(self.next_tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_and_disjoint_per_kind() {
        let mut ids = IdGen::new();
        assert_eq!(ids.next_pane(), PaneId(1));
        assert_eq!(ids.next_pane(), PaneId(2));
        // The tab counter is independent of the pane counter.
        assert_eq!(ids.next_tab(), TabId(1));
        assert_eq!(ids.next_pane(), PaneId(3));
    }
}
