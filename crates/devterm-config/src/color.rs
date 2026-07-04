//! Config-local RGB colour type with hex-string serde.
//!
//! Kept independent of `devterm-term` so the config crate honours the strict
//! dependency direction. Serialises to/from a `#rrggbb` hex string.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A 24-bit RGB colour.
///
/// Serialises as a lowercase hex string of the form `#rrggbb` (e.g. `#ff0000`
/// for pure red) and parses the same form (case-insensitive, `#` optional).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Color {
    /// Construct a colour from its channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Error returned when a hex colour string cannot be parsed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseColorError(String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid hex colour {:?}: expected #rrggbb", self.0)
    }
}

impl std::error::Error for ParseColorError {}

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Color, ParseColorError> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseColorError(s.to_owned()));
        }
        let parse = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16);
        match (parse(0), parse(2), parse(4)) {
            (Ok(r), Ok(g), Ok(b)) => Ok(Color { r, g, b }),
            _ => Err(ParseColorError(s.to_owned())),
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Color, D::Error> {
        struct ColorVisitor;

        impl Visitor<'_> for ColorVisitor {
            type Value = Color;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a hex colour string of the form #rrggbb")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Color, E> {
                value.parse().map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(ColorVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_hash() {
        assert_eq!("#ff0000".parse::<Color>().unwrap(), Color::new(255, 0, 0));
        assert_eq!("00ff00".parse::<Color>().unwrap(), Color::new(0, 255, 0));
        assert_eq!("#0000FF".parse::<Color>().unwrap(), Color::new(0, 0, 255));
    }

    #[test]
    fn display_is_lowercase_hash_form() {
        assert_eq!(Color::new(0xd0, 0xd0, 0xd0).to_string(), "#d0d0d0");
        assert_eq!(Color::new(255, 0, 0).to_string(), "#ff0000");
    }

    #[test]
    fn rejects_bad_input() {
        assert!("#12345".parse::<Color>().is_err());
        assert!("#gggggg".parse::<Color>().is_err());
        assert!("red".parse::<Color>().is_err());
    }

    #[test]
    fn hex_round_trips_through_string() {
        for c in [
            Color::new(0, 0, 0),
            Color::new(255, 255, 255),
            Color::new(0x80, 0x00, 0x80),
            Color::new(0x12, 0x34, 0x56),
        ] {
            assert_eq!(c.to_string().parse::<Color>().unwrap(), c);
        }
    }

    #[test]
    fn serde_round_trips_via_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrap {
            c: Color,
        }
        let w = Wrap {
            c: Color::new(0xab, 0xcd, 0xef),
        };
        let text = toml::to_string(&w).unwrap();
        assert!(text.contains("#abcdef"), "got: {text}");
        let back: Wrap = toml::from_str(&text).unwrap();
        assert_eq!(back, w);
    }
}
