//! 24-bit RGB colour and the built-in xterm-256 default palette.
//!
//! Defines [`Rgb`] — the fully-resolved colour type that leaves this crate — plus the
//! built-in default palette (16 ANSI + 6x6x6 cube + 24 grays) used to resolve palette
//! indices the child never overrode via OSC.

use alacritty_terminal::vte::ansi::Rgb as AnsiRgb;

/// 24-bit RGB colour, fully resolved (no palette indices leave this crate).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub(crate) const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Darken for the `DIM` attribute (~2/3 brightness).
    pub(crate) fn dimmed(self) -> Self {
        Self {
            r: ((self.r as u16 * 2) / 3) as u8,
            g: ((self.g as u16 * 2) / 3) as u8,
            b: ((self.b as u16 * 2) / 3) as u8,
        }
    }
}

impl From<AnsiRgb> for Rgb {
    fn from(c: AnsiRgb) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in default palette (xterm-256): 16 ANSI + 6x6x6 cube + 24 grays.
// ---------------------------------------------------------------------------

/// Standard system palette for the first 16 ANSI colours.
pub(crate) const ANSI_16: [Rgb; 16] = [
    Rgb::new(0x00, 0x00, 0x00), // 0  black
    Rgb::new(0x80, 0x00, 0x00), // 1  red
    Rgb::new(0x00, 0x80, 0x00), // 2  green
    Rgb::new(0x80, 0x80, 0x00), // 3  yellow
    Rgb::new(0x00, 0x00, 0x80), // 4  blue
    Rgb::new(0x80, 0x00, 0x80), // 5  magenta
    Rgb::new(0x00, 0x80, 0x80), // 6  cyan
    Rgb::new(0xc0, 0xc0, 0xc0), // 7  white
    Rgb::new(0x80, 0x80, 0x80), // 8  bright black
    Rgb::new(0xff, 0x00, 0x00), // 9  bright red
    Rgb::new(0x00, 0xff, 0x00), // 10 bright green
    Rgb::new(0xff, 0xff, 0x00), // 11 bright yellow
    Rgb::new(0x00, 0x00, 0xff), // 12 bright blue
    Rgb::new(0xff, 0x00, 0xff), // 13 bright magenta
    Rgb::new(0x00, 0xff, 0xff), // 14 bright cyan
    Rgb::new(0xff, 0xff, 0xff), // 15 bright white
];

/// Default foreground when the palette has no override.
pub(crate) const DEFAULT_FG: Rgb = Rgb::new(0xd0, 0xd0, 0xd0);
/// Default background when the palette has no override.
pub(crate) const DEFAULT_BG: Rgb = Rgb::new(0x00, 0x00, 0x00);

/// Resolve a palette index (0..=268) to an RGB value using the built-in xterm-256
/// default palette. Handles the ANSI 16, the 6x6x6 cube, the 24-step gray ramp, and
/// the named foreground/background/cursor slots.
pub(crate) fn builtin_palette(index: usize) -> Rgb {
    match index {
        0..=15 => ANSI_16[index],
        16..=231 => {
            // 6x6x6 colour cube.
            let i = index - 16;
            let r = i / 36;
            let g = (i / 6) % 6;
            let b = i % 6;
            let component = |v: usize| -> u8 { if v == 0 { 0 } else { (55 + 40 * v) as u8 } };
            Rgb::new(component(r), component(g), component(b))
        }
        232..=255 => {
            // 24-step grayscale ramp.
            let level = (8 + 10 * (index - 232)) as u8;
            Rgb::new(level, level, level)
        }
        // Named slots (Foreground = 256, Background = 257, Cursor = 258, ...).
        256 => DEFAULT_FG,
        257 => DEFAULT_BG,
        258 => DEFAULT_FG, // cursor colour falls back to foreground
        _ => DEFAULT_FG,
    }
}
