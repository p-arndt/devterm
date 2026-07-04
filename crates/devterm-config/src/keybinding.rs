//! Key chords, modifiers, and the default/tmux keymap presets.
//!
//! A [`KeyChord`] is a set of [`Mods`] plus a [`KeyCode`]. Chords parse from and
//! render to `+`-separated strings like `ctrl+shift+h` or `alt+left`. The presets
//! map every [`Action`] to a chord.

use crate::action::Action;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Keyboard modifier state for a chord.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Mods {
    /// Control key.
    pub ctrl: bool,
    /// Alt / Option key.
    pub alt: bool,
    /// Shift key.
    pub shift: bool,
    /// Logo / Super / Command / Windows key.
    pub logo: bool,
}

/// A named (non-character) key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Named {
    Enter,
    Tab,
    Escape,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl Named {
    /// The canonical lowercase name used in chord strings.
    fn as_str(&self) -> &'static str {
        match self {
            Named::Enter => "enter",
            Named::Tab => "tab",
            Named::Escape => "escape",
            Named::Space => "space",
            Named::Backspace => "backspace",
            Named::Delete => "delete",
            Named::Insert => "insert",
            Named::Home => "home",
            Named::End => "end",
            Named::PageUp => "pageup",
            Named::PageDown => "pagedown",
            Named::ArrowUp => "up",
            Named::ArrowDown => "down",
            Named::ArrowLeft => "left",
            Named::ArrowRight => "right",
            Named::F1 => "f1",
            Named::F2 => "f2",
            Named::F3 => "f3",
            Named::F4 => "f4",
            Named::F5 => "f5",
            Named::F6 => "f6",
            Named::F7 => "f7",
            Named::F8 => "f8",
            Named::F9 => "f9",
            Named::F10 => "f10",
            Named::F11 => "f11",
            Named::F12 => "f12",
        }
    }

    /// Parse a name token (already lowercased). Accepts a few aliases.
    fn parse(token: &str) -> Option<Named> {
        let named = match token {
            "enter" | "return" => Named::Enter,
            "tab" => Named::Tab,
            "escape" | "esc" => Named::Escape,
            "space" => Named::Space,
            "backspace" => Named::Backspace,
            "delete" | "del" => Named::Delete,
            "insert" | "ins" => Named::Insert,
            "home" => Named::Home,
            "end" => Named::End,
            "pageup" | "pgup" => Named::PageUp,
            "pagedown" | "pgdn" | "pgdown" => Named::PageDown,
            "up" | "arrowup" => Named::ArrowUp,
            "down" | "arrowdown" => Named::ArrowDown,
            "left" | "arrowleft" => Named::ArrowLeft,
            "right" | "arrowright" => Named::ArrowRight,
            "f1" => Named::F1,
            "f2" => Named::F2,
            "f3" => Named::F3,
            "f4" => Named::F4,
            "f5" => Named::F5,
            "f6" => Named::F6,
            "f7" => Named::F7,
            "f8" => Named::F8,
            "f9" => Named::F9,
            "f10" => Named::F10,
            "f11" => Named::F11,
            "f12" => Named::F12,
            _ => return None,
        };
        Some(named)
    }
}

/// A key: either a (lowercase) character or a named key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyCode {
    /// A character key, stored lowercase.
    Char(char),
    /// A named key.
    Named(Named),
}

/// A full key chord: modifiers + a key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyChord {
    /// Active modifiers.
    pub mods: Mods,
    /// The non-modifier key.
    pub code: KeyCode,
}

impl KeyChord {
    /// Construct a chord.
    pub fn new(mods: Mods, code: KeyCode) -> KeyChord {
        KeyChord { mods, code }
    }
}

/// Error returned when a chord string cannot be parsed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseKeyChordError(String);

impl fmt::Display for ParseKeyChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid key chord {:?}", self.0)
    }
}

impl std::error::Error for ParseKeyChordError {}

impl FromStr for KeyChord {
    type Err = ParseKeyChordError;

    fn from_str(s: &str) -> Result<KeyChord, ParseKeyChordError> {
        let err = || ParseKeyChordError(s.to_owned());
        let mut mods = Mods::default();
        let mut code: Option<KeyCode> = None;

        for raw in s.split('+') {
            let token = raw.trim().to_ascii_lowercase();
            if token.is_empty() {
                return Err(err());
            }
            // A key was already set but more tokens follow: malformed.
            if code.is_some() {
                return Err(err());
            }
            match token.as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "option" => mods.alt = true,
                "shift" => mods.shift = true,
                "logo" | "super" | "cmd" | "win" | "windows" => mods.logo = true,
                _ => {
                    // Must be the final key token: a Named key or a single char.
                    if let Some(named) = Named::parse(&token) {
                        code = Some(KeyCode::Named(named));
                    } else {
                        let mut chars = token.chars();
                        match (chars.next(), chars.next()) {
                            (Some(c), None) => code = Some(KeyCode::Char(c)),
                            _ => return Err(err()),
                        }
                    }
                }
            }
        }

        match code {
            Some(code) => Ok(KeyChord { mods, code }),
            None => Err(err()),
        }
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.ctrl {
            f.write_str("ctrl+")?;
        }
        if self.mods.alt {
            f.write_str("alt+")?;
        }
        if self.mods.shift {
            f.write_str("shift+")?;
        }
        if self.mods.logo {
            f.write_str("logo+")?;
        }
        match self.code {
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Named(named) => f.write_str(named.as_str()),
        }
    }
}

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
        (chord("ctrl+alt+left"), Action::FocusLeft),
        (chord("ctrl+alt+right"), Action::FocusRight),
        (chord("ctrl+alt+up"), Action::FocusUp),
        (chord("ctrl+alt+down"), Action::FocusDown),
        (chord("ctrl+shift+left"), Action::ResizeLeft),
        (chord("ctrl+shift+right"), Action::ResizeRight),
        (chord("ctrl+shift+up"), Action::ResizeUp),
        (chord("ctrl+shift+down"), Action::ResizeDown),
        (chord("ctrl+shift+c"), Action::Copy),
        (chord("ctrl+shift+v"), Action::Paste),
        (chord("ctrl+shift+k"), Action::ScrollLineUp),
        (chord("ctrl+shift+j"), Action::ScrollLineDown),
        (chord("shift+pageup"), Action::ScrollPageUp),
        (chord("shift+pagedown"), Action::ScrollPageDown),
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
        // tmux pane navigation: prefix + arrows.
        (chord("ctrl+alt+left"), Action::FocusLeft),
        (chord("ctrl+alt+right"), Action::FocusRight),
        (chord("ctrl+alt+up"), Action::FocusUp),
        (chord("ctrl+alt+down"), Action::FocusDown),
        // tmux resize: prefix + Ctrl-arrow; approximate with ctrl+shift+arrow.
        (chord("ctrl+shift+left"), Action::ResizeLeft),
        (chord("ctrl+shift+right"), Action::ResizeRight),
        (chord("ctrl+shift+up"), Action::ResizeUp),
        (chord("ctrl+shift+down"), Action::ResizeDown),
        // Copy-mode-ish clipboard.
        (chord("ctrl+alt+c"), Action::Copy),
        (chord("ctrl+alt+v"), Action::Paste),
        // tmux copy-mode scrolling (k/j like vi, PageUp/PageDown).
        (chord("ctrl+alt+k"), Action::ScrollLineUp),
        (chord("ctrl+alt+j"), Action::ScrollLineDown),
        (chord("ctrl+alt+pageup"), Action::ScrollPageUp),
        (chord("ctrl+alt+pagedown"), Action::ScrollPageDown),
        (chord("ctrl+alt+q"), Action::Quit),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn parse(s: &str) -> KeyChord {
        s.parse().unwrap()
    }

    #[test]
    fn parses_modifier_combinations() {
        let c = parse("ctrl+shift+h");
        assert_eq!(
            c,
            KeyChord::new(
                Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                KeyCode::Char('h'),
            )
        );

        let c = parse("alt+left");
        assert_eq!(
            c,
            KeyChord::new(
                Mods {
                    alt: true,
                    ..Mods::default()
                },
                KeyCode::Named(Named::ArrowLeft),
            )
        );
    }

    #[test]
    fn parsing_is_case_insensitive_and_trims() {
        assert_eq!(parse("CTRL+Shift+PageUp"), parse("ctrl+shift+pageup"));
        assert_eq!(parse(" ctrl + shift + h "), parse("ctrl+shift+h"));
    }

    #[test]
    fn modifier_aliases() {
        assert!(parse("control+a").mods.ctrl);
        assert!(parse("option+a").mods.alt);
        assert!(parse("super+a").mods.logo);
        assert!(parse("cmd+a").mods.logo);
        assert!(parse("win+a").mods.logo);
    }

    #[test]
    fn display_round_trips() {
        for s in ["ctrl+shift+h", "alt+left", "ctrl+shift+pageup", "shift+f5"] {
            let chord = parse(s);
            let rendered = chord.to_string();
            assert_eq!(rendered.parse::<KeyChord>().unwrap(), chord);
        }
    }

    #[test]
    fn rejects_malformed() {
        assert!("".parse::<KeyChord>().is_err());
        assert!("ctrl+".parse::<KeyChord>().is_err());
        assert!("ctrl+shift".parse::<KeyChord>().is_err()); // shift alone, no key
        assert!("h+ctrl".parse::<KeyChord>().is_err()); // key before modifier
        assert!("ctrl+ab".parse::<KeyChord>().is_err()); // multi-char non-named
    }

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
