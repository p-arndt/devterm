//! Built-in keymap presets: the default DevTerm map and a tmux-flavoured map.
//!
//! [`KeymapPreset`] selects which table [`default_keymap`] or [`tmux_preset`] a
//! config starts from before user overrides are applied. Each preset maps every
//! [`Action`] to a chord.

use super::KeyChord;
use crate::action::Action;
use serde::{Deserialize, Serialize};

/// Which built-in keymap to start from before user overrides.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeymapPreset {
    /// The default DevTerm keymap.
    #[default]
    Default,
    /// A tmux-flavoured keymap (direct chords; see [`tmux_preset`]).
    Tmux,
}

impl KeymapPreset {
    /// The preset's binding list.
    pub fn bindings(&self) -> Vec<(KeyChord, Action)> {
        match self {
            KeymapPreset::Default => default_keymap(),
            KeymapPreset::Tmux => tmux_preset(),
        }
    }
}

/// Parse a chord literal known to be valid (preset tables only).
fn chord(s: &str) -> KeyChord {
    s.parse()
        .unwrap_or_else(|_| panic!("preset chord {s:?} must be valid"))
}

/// The default DevTerm keymap. Covers every [`Action`] with non-conflicting chords.
pub fn default_keymap() -> Vec<(KeyChord, Action)> {
    vec![
        (chord("ctrl+shift+h"), Action::SplitHorizontal),
        (chord("ctrl+shift+s"), Action::SplitVertical),
        (chord("ctrl+shift+w"), Action::ClosePane),
        // Tabs. ctrl+shift+t is taken by the floating terminal, so "new tab" uses
        // ctrl+shift+n; ctrl+tab / ctrl+shift+tab cycle like a browser.
        (chord("ctrl+shift+n"), Action::NewTab),
        (chord("ctrl+shift+x"), Action::CloseTab),
        (chord("ctrl+tab"), Action::NextTab),
        (chord("ctrl+shift+tab"), Action::PrevTab),
        // Focus uses ctrl+shift+arrows; ctrl+alt+arrows is reserved for workspace
        // switching on GNOME/Ubuntu, so it is deliberately avoided.
        (chord("ctrl+shift+left"), Action::FocusLeft),
        (chord("ctrl+shift+right"), Action::FocusRight),
        (chord("ctrl+shift+up"), Action::FocusUp),
        (chord("ctrl+shift+down"), Action::FocusDown),
        (chord("alt+shift+left"), Action::ResizeLeft),
        (chord("alt+shift+right"), Action::ResizeRight),
        (chord("alt+shift+up"), Action::ResizeUp),
        (chord("alt+shift+down"), Action::ResizeDown),
        (chord("ctrl+shift+c"), Action::Copy),
        (chord("ctrl+shift+v"), Action::Paste),
        (chord("ctrl+shift+k"), Action::ScrollLineUp),
        (chord("ctrl+shift+j"), Action::ScrollLineDown),
        (chord("shift+pageup"), Action::ScrollPageUp),
        (chord("shift+pagedown"), Action::ScrollPageDown),
        // Editor-style "open settings" chord (matches VS Code's Ctrl+,): opens the
        // inline overlay. Ctrl+Shift+, still opens the raw file in an editor.
        (chord("ctrl+,"), Action::OpenSettings),
        (chord("ctrl+shift+,"), Action::OpenConfig),
        // Quake-style drop-down scratch terminal.
        (chord("ctrl+shift+t"), Action::ToggleFloatingTerminal),
        (chord("ctrl+shift+q"), Action::Quit),
    ]
}

/// A tmux-flavoured keymap.
///
/// Real tmux uses a `Ctrl-b` *prefix* mode: press the prefix, release, then press
/// the command key. That requires app-side state to track "prefix armed", so this
/// preset instead approximates tmux with DIRECT (prefix-free) chords using the same
/// letters tmux uses. A true prefix mode is a later refinement in the app layer.
pub fn tmux_preset() -> Vec<(KeyChord, Action)> {
    vec![
        // tmux uses `"` and `%` after the prefix to split; approximate with alt chords.
        (chord("ctrl+alt+\""), Action::SplitVertical),
        (chord("ctrl+alt+%"), Action::SplitHorizontal),
        (chord("ctrl+alt+x"), Action::ClosePane),
        // tmux windows ≈ tabs: prefix+c/&/n/p. `c` is taken by Copy here, so "new tab"
        // falls back to ctrl+alt+n and cycling uses the universal ctrl+tab chords.
        (chord("ctrl+alt+n"), Action::NewTab),
        (chord("ctrl+alt+&"), Action::CloseTab),
        (chord("ctrl+tab"), Action::NextTab),
        (chord("ctrl+shift+tab"), Action::PrevTab),
        // tmux pane navigation: prefix + arrows. Approximated with ctrl+shift+arrow —
        // ctrl+alt+arrow is reserved for workspace switching on GNOME/Ubuntu.
        (chord("ctrl+shift+left"), Action::FocusLeft),
        (chord("ctrl+shift+right"), Action::FocusRight),
        (chord("ctrl+shift+up"), Action::FocusUp),
        (chord("ctrl+shift+down"), Action::FocusDown),
        // tmux resize: prefix + Ctrl-arrow; approximate with alt+shift+arrow.
        (chord("alt+shift+left"), Action::ResizeLeft),
        (chord("alt+shift+right"), Action::ResizeRight),
        (chord("alt+shift+up"), Action::ResizeUp),
        (chord("alt+shift+down"), Action::ResizeDown),
        // Copy-mode-ish clipboard.
        (chord("ctrl+alt+c"), Action::Copy),
        (chord("ctrl+alt+v"), Action::Paste),
        // tmux copy-mode scrolling (k/j like vi, PageUp/PageDown).
        (chord("ctrl+alt+k"), Action::ScrollLineUp),
        (chord("ctrl+alt+j"), Action::ScrollLineDown),
        (chord("ctrl+alt+pageup"), Action::ScrollPageUp),
        (chord("ctrl+alt+pagedown"), Action::ScrollPageDown),
        (chord("ctrl+alt+,"), Action::OpenSettings),
        (chord("ctrl+alt+shift+,"), Action::OpenConfig),
        (chord("ctrl+alt+t"), Action::ToggleFloatingTerminal),
        (chord("ctrl+alt+q"), Action::Quit),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn assert_covers_all_actions(bindings: &[(KeyChord, Action)]) {
        for action in Action::ALL {
            assert!(
                bindings.iter().any(|(_, a)| *a == action),
                "missing binding for {action:?}"
            );
        }
    }

    fn assert_no_conflicts(bindings: &[(KeyChord, Action)]) {
        let mut seen = HashSet::new();
        for (chord, _) in bindings {
            assert!(seen.insert(*chord), "duplicate chord {chord}");
        }
    }

    #[test]
    fn default_preset_covers_every_action_without_conflicts() {
        let bindings = default_keymap();
        assert_covers_all_actions(&bindings);
        assert_no_conflicts(&bindings);
    }

    #[test]
    fn tmux_preset_covers_every_action_without_conflicts() {
        let bindings = tmux_preset();
        assert_covers_all_actions(&bindings);
        assert_no_conflicts(&bindings);
    }

    #[test]
    fn keymap_preset_serde_kebab_and_default() {
        assert_eq!(KeymapPreset::default(), KeymapPreset::Default);
        let text = toml::to_string(&Wrap {
            p: KeymapPreset::Tmux,
        })
        .unwrap();
        assert!(text.contains("tmux"), "got: {text}");
    }

    #[derive(Serialize, Deserialize)]
    struct Wrap {
        p: KeymapPreset,
    }
}
