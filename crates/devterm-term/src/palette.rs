//! Theme override palette for the palette-backed default colours.

use crate::color::{ANSI_16, DEFAULT_BG, DEFAULT_FG, Rgb};

/// Theme override for the palette-backed default colours.
///
/// Consulted whenever the live emulator palette has no entry for a slot (i.e. the child
/// never set it via OSC). OSC-set colours always win over this. [`Palette::default`]
/// reproduces the built-in xterm palette, so an unset theme changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The 16 ANSI colours (indices 0..=15).
    pub ansi: [Rgb; 16],
    /// Default foreground (named `Foreground` slot, index 256).
    pub foreground: Rgb,
    /// Default background (named `Background` slot, index 257).
    pub background: Rgb,
    /// Cursor colour (named `Cursor` slot, index 258).
    pub cursor: Rgb,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            ansi: ANSI_16,
            foreground: DEFAULT_FG,
            background: DEFAULT_BG,
            cursor: DEFAULT_FG,
        }
    }
}
