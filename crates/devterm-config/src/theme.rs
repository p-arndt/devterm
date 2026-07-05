//! Colour themes.
//!
//! A [`Theme`] carries the 16 ANSI colours plus foreground/background/cursor. The
//! [`Default`] reproduces the built-in xterm system palette used by `devterm-term`
//! today, so themes are additive rather than a behaviour change.

use crate::color::Color;
use serde::{Deserialize, Serialize};

/// A complete colour theme: 16 ANSI colours + foreground/background/cursor.
///
/// `#[serde(default)]` on every field lets a partial `[theme]` table merge onto
/// the [`Default`] palette.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    /// The 16 standard ANSI colours (0-7 normal, 8-15 bright).
    pub ansi: [Color; 16],
    /// Default foreground colour.
    pub foreground: Color,
    /// Default background colour.
    pub background: Color,
    /// Cursor colour.
    pub cursor: Color,
}

/// The standard xterm system palette for the first 16 ANSI colours.
const ANSI_DEFAULT: [Color; 16] = [
    Color::new(0x00, 0x00, 0x00), // 0  black
    Color::new(0x80, 0x00, 0x00), // 1  red
    Color::new(0x00, 0x80, 0x00), // 2  green
    Color::new(0x80, 0x80, 0x00), // 3  yellow
    Color::new(0x00, 0x00, 0x80), // 4  blue
    Color::new(0x80, 0x00, 0x80), // 5  magenta
    Color::new(0x00, 0x80, 0x80), // 6  cyan
    Color::new(0xc0, 0xc0, 0xc0), // 7  white
    Color::new(0x80, 0x80, 0x80), // 8  bright black
    Color::new(0xff, 0x00, 0x00), // 9  bright red
    Color::new(0x00, 0xff, 0x00), // 10 bright green
    Color::new(0xff, 0xff, 0x00), // 11 bright yellow
    Color::new(0x00, 0x00, 0xff), // 12 bright blue
    Color::new(0xff, 0x00, 0xff), // 13 bright magenta
    Color::new(0x00, 0xff, 0xff), // 14 bright cyan
    Color::new(0xff, 0xff, 0xff), // 15 bright white
];

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            ansi: ANSI_DEFAULT,
            foreground: Color::new(0xd0, 0xd0, 0xd0),
            background: Color::new(0x00, 0x00, 0x00),
            cursor: Color::new(0xd0, 0xd0, 0xd0),
        }
    }
}

impl Theme {
    /// Names of every built-in theme, in a stable order (for pickers / cycling).
    pub const BUILTIN_NAMES: &'static [&'static str] = &["default", "gruvbox-dark"];

    /// Look up a built-in theme by name. Returns `None` for unknown names.
    ///
    /// Known names: `"default"` (the xterm system palette) and `"gruvbox-dark"`.
    pub fn builtin(name: &str) -> Option<Theme> {
        match name {
            "default" => Some(Theme::default()),
            "gruvbox-dark" => Some(Theme::gruvbox_dark()),
            _ => None,
        }
    }

    /// A gruvbox-flavoured dark theme.
    fn gruvbox_dark() -> Theme {
        Theme {
            ansi: [
                Color::new(0x28, 0x28, 0x28), // 0  black  (bg0)
                Color::new(0xcc, 0x24, 0x1d), // 1  red
                Color::new(0x98, 0x97, 0x1a), // 2  green
                Color::new(0xd7, 0x99, 0x21), // 3  yellow
                Color::new(0x45, 0x85, 0x88), // 4  blue
                Color::new(0xb1, 0x62, 0x86), // 5  magenta
                Color::new(0x68, 0x9d, 0x6a), // 6  cyan
                Color::new(0xa8, 0x99, 0x84), // 7  white  (fg4)
                Color::new(0x92, 0x83, 0x74), // 8  bright black  (gray)
                Color::new(0xfb, 0x49, 0x34), // 9  bright red
                Color::new(0xb8, 0xbb, 0x26), // 10 bright green
                Color::new(0xfa, 0xbd, 0x2f), // 11 bright yellow
                Color::new(0x83, 0xa5, 0x98), // 12 bright blue
                Color::new(0xd3, 0x86, 0x9b), // 13 bright magenta
                Color::new(0x8e, 0xc0, 0x7c), // 14 bright cyan
                Color::new(0xeb, 0xdb, 0xb2), // 15 bright white  (fg1)
            ],
            foreground: Color::new(0xeb, 0xdb, 0xb2),
            background: Color::new(0x28, 0x28, 0x28),
            cursor: Color::new(0xeb, 0xdb, 0xb2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_documented_palette() {
        let theme = Theme::default();
        assert_eq!(theme.ansi[0], Color::new(0x00, 0x00, 0x00));
        assert_eq!(theme.ansi[1], Color::new(0x80, 0x00, 0x00));
        assert_eq!(theme.ansi[7], Color::new(0xc0, 0xc0, 0xc0));
        assert_eq!(theme.ansi[8], Color::new(0x80, 0x80, 0x80));
        assert_eq!(theme.ansi[9], Color::new(0xff, 0x00, 0x00));
        assert_eq!(theme.ansi[15], Color::new(0xff, 0xff, 0xff));
        assert_eq!(theme.foreground, Color::new(0xd0, 0xd0, 0xd0));
        assert_eq!(theme.background, Color::new(0x00, 0x00, 0x00));
        assert_eq!(theme.cursor, Color::new(0xd0, 0xd0, 0xd0));
    }

    #[test]
    fn builtin_lookup() {
        assert_eq!(Theme::builtin("default"), Some(Theme::default()));
        assert!(Theme::builtin("gruvbox-dark").is_some());
        assert_ne!(Theme::builtin("gruvbox-dark").unwrap(), Theme::default());
        assert_eq!(Theme::builtin("nope"), None);
    }

    #[test]
    fn partial_theme_table_merges_onto_default() {
        let text = r##"cursor = "#ff0000""##;
        let theme: Theme = toml::from_str(text).expect("parse partial theme");
        assert_eq!(theme.cursor, Color::new(0xff, 0x00, 0x00));
        // Untouched fields keep their defaults.
        assert_eq!(theme.foreground, Color::new(0xd0, 0xd0, 0xd0));
        assert_eq!(theme.ansi, ANSI_DEFAULT);
    }

    #[test]
    fn theme_round_trips_through_toml() {
        let theme = Theme::gruvbox_dark();
        let text = toml::to_string(&theme).expect("serialize theme");
        let back: Theme = toml::from_str(&text).expect("deserialize theme");
        assert_eq!(back, theme);
    }
}
