//! Cursor appearance configuration.
//!
//! [`CursorConfig`] carries the user's preferred cursor [`CursorShapePref`] and a
//! blink toggle, deserialized from a `[cursor]` table in `config.toml`. Pure
//! schema; the renderer decides how to draw each shape.

use serde::{Deserialize, Serialize};

/// Preferred cursor shape.
///
/// [`CursorShapePref::Default`] means "follow the program / terminal default"
/// rather than forcing a specific shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorShapePref {
    /// Follow the program / terminal default (no override).
    #[default]
    Default,
    /// A filled block covering the cell.
    Block,
    /// A horizontal bar under the cell.
    Underline,
    /// A vertical bar at the left of the cell.
    Beam,
}

/// Cursor appearance: shape preference plus blink toggle.
///
/// `#[serde(default)]` on every field lets a partial `[cursor]` table merge onto
/// the [`Default`] (`shape = Default`, `blink = false`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    /// Preferred cursor shape.
    pub shape: CursorShapePref,
    /// Whether the cursor should blink.
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> CursorConfig {
        CursorConfig {
            shape: CursorShapePref::Default,
            blink: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_shapeless_and_static() {
        let cursor = CursorConfig::default();
        assert_eq!(cursor.shape, CursorShapePref::Default);
        assert!(!cursor.blink);
    }

    #[test]
    fn shape_serializes_kebab_case() {
        let cursor = CursorConfig {
            shape: CursorShapePref::Underline,
            blink: true,
        };
        let text = toml::to_string(&cursor).expect("serialize cursor");
        assert!(text.contains("shape = \"underline\""), "got: {text}");
        assert!(text.contains("blink = true"), "got: {text}");
    }

    #[test]
    fn cursor_round_trips_through_toml() {
        for shape in [
            CursorShapePref::Default,
            CursorShapePref::Block,
            CursorShapePref::Underline,
            CursorShapePref::Beam,
        ] {
            let cursor = CursorConfig { shape, blink: true };
            let text = toml::to_string(&cursor).expect("serialize cursor");
            let back: CursorConfig = toml::from_str(&text).expect("deserialize cursor");
            assert_eq!(back, cursor);
        }
    }

    #[test]
    fn partial_table_merges_onto_default() {
        // Only `blink` set: `shape` keeps its default.
        let cursor: CursorConfig = toml::from_str("blink = true").expect("parse partial cursor");
        assert_eq!(cursor.shape, CursorShapePref::Default);
        assert!(cursor.blink);

        // Only `shape` set: `blink` keeps its default.
        let cursor: CursorConfig =
            toml::from_str("shape = \"beam\"").expect("parse partial cursor");
        assert_eq!(cursor.shape, CursorShapePref::Beam);
        assert!(!cursor.blink);
    }
}
