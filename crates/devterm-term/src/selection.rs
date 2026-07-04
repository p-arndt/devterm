//! Selection mode, mapping onto alacritty's `SelectionType`.

use alacritty_terminal::selection::SelectionType;

/// How a selection extends from its anchor. Maps onto alacritty `SelectionType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    /// Character-by-character selection.
    Simple,
    /// Word-granularity selection.
    Semantic,
    /// Whole-line selection.
    Lines,
}

impl SelectionMode {
    pub(crate) fn to_alacritty(self) -> SelectionType {
        match self {
            SelectionMode::Simple => SelectionType::Simple,
            SelectionMode::Semantic => SelectionType::Semantic,
            SelectionMode::Lines => SelectionType::Lines,
        }
    }
}
