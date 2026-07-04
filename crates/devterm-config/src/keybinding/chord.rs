//! Key chords, modifiers, key codes, and their parse/display impls.
//!
//! A [`KeyChord`] is a set of [`Mods`] plus a [`KeyCode`]. Chords parse from and
//! render to `+`-separated strings like `ctrl+shift+h` or `alt+left`.

use super::named::Named;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
