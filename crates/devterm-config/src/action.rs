//! User-triggerable actions and their string names.
//!
//! An [`Action`] is what a key chord maps to. Names are kebab-case both in serde
//! and in [`Action::from_str`], so `config.toml` and code agree on spelling.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Something the user can bind a key chord to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Split the focused pane into a left/right pair.
    SplitHorizontal,
    /// Split the focused pane into a top/bottom pair.
    SplitVertical,
    /// Close the focused pane.
    ClosePane,
    /// Open a new tab (with a fresh shell pane) and switch to it.
    NewTab,
    /// Close the current tab, dropping all of its panes.
    CloseTab,
    /// Switch to the tab to the right (wrapping).
    NextTab,
    /// Switch to the tab to the left (wrapping).
    PrevTab,
    /// Move focus to the pane on the left.
    FocusLeft,
    /// Move focus to the pane on the right.
    FocusRight,
    /// Move focus to the pane above.
    FocusUp,
    /// Move focus to the pane below.
    FocusDown,
    /// Grow/shrink the focused pane toward the left.
    ResizeLeft,
    /// Grow/shrink the focused pane toward the right.
    ResizeRight,
    /// Grow/shrink the focused pane upward.
    ResizeUp,
    /// Grow/shrink the focused pane downward.
    ResizeDown,
    /// Copy the current selection to the clipboard.
    Copy,
    /// Paste the clipboard into the focused pane.
    Paste,
    /// Scroll the focused pane up one line.
    ScrollLineUp,
    /// Scroll the focused pane down one line.
    ScrollLineDown,
    /// Scroll the focused pane up one page.
    ScrollPageUp,
    /// Scroll the focused pane down one page.
    ScrollPageDown,
    /// Open `config.toml` in the user's editor.
    OpenConfig,
    /// Open the inline settings overlay (arrow-key navigable).
    OpenSettings,
    /// Show/hide the floating "scratch" terminal overlaid on the layout.
    ToggleFloatingTerminal,
    /// Quit the application.
    Quit,
}

impl Action {
    /// All actions, in declaration order. Used to prove presets are exhaustive.
    pub const ALL: [Action; 25] = [
        Action::SplitHorizontal,
        Action::SplitVertical,
        Action::ClosePane,
        Action::NewTab,
        Action::CloseTab,
        Action::NextTab,
        Action::PrevTab,
        Action::FocusLeft,
        Action::FocusRight,
        Action::FocusUp,
        Action::FocusDown,
        Action::ResizeLeft,
        Action::ResizeRight,
        Action::ResizeUp,
        Action::ResizeDown,
        Action::Copy,
        Action::Paste,
        Action::ScrollLineUp,
        Action::ScrollLineDown,
        Action::ScrollPageUp,
        Action::ScrollPageDown,
        Action::OpenConfig,
        Action::OpenSettings,
        Action::ToggleFloatingTerminal,
        Action::Quit,
    ];

    /// The kebab-case name of this action (matches serde and [`FromStr`]).
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::SplitHorizontal => "split-horizontal",
            Action::SplitVertical => "split-vertical",
            Action::ClosePane => "close-pane",
            Action::NewTab => "new-tab",
            Action::CloseTab => "close-tab",
            Action::NextTab => "next-tab",
            Action::PrevTab => "prev-tab",
            Action::FocusLeft => "focus-left",
            Action::FocusRight => "focus-right",
            Action::FocusUp => "focus-up",
            Action::FocusDown => "focus-down",
            Action::ResizeLeft => "resize-left",
            Action::ResizeRight => "resize-right",
            Action::ResizeUp => "resize-up",
            Action::ResizeDown => "resize-down",
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::ScrollLineUp => "scroll-line-up",
            Action::ScrollLineDown => "scroll-line-down",
            Action::ScrollPageUp => "scroll-page-up",
            Action::ScrollPageDown => "scroll-page-down",
            Action::OpenConfig => "open-config",
            Action::OpenSettings => "open-settings",
            Action::ToggleFloatingTerminal => "toggle-floating-terminal",
            Action::Quit => "quit",
        }
    }
}

/// Error returned when an action name is not recognised.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseActionError(String);

impl std::fmt::Display for ParseActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown action {:?}", self.0)
    }
}

impl std::error::Error for ParseActionError {}

impl FromStr for Action {
    type Err = ParseActionError;

    fn from_str(s: &str) -> Result<Action, ParseActionError> {
        Action::ALL
            .into_iter()
            .find(|a| a.as_str() == s)
            .ok_or_else(|| ParseActionError(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_round_trips_for_all() {
        for action in Action::ALL {
            assert_eq!(action.as_str().parse::<Action>().unwrap(), action);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("not-an-action".parse::<Action>().is_err());
    }

    #[test]
    fn serde_uses_kebab_case() {
        let text = toml::to_string(&Wrap {
            a: Action::SplitHorizontal,
        })
        .unwrap();
        assert!(text.contains("split-horizontal"), "got: {text}");
        let back: Wrap = toml::from_str(&text).unwrap();
        assert_eq!(back.a, Action::SplitHorizontal);
    }

    #[derive(Serialize, Deserialize)]
    struct Wrap {
        a: Action,
    }
}
