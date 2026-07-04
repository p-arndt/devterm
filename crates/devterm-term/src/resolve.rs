//! Colour resolution helpers.
//!
//! Turn alacritty's palette-indexed cell colours into the concrete [`Rgb`] values the
//! renderer consumes, consulting the live emulator palette first, then the theme-override
//! [`Palette`], then the built-in xterm default. Also maps alacritty's cursor shape onto
//! our public [`CursorShape`].

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape as AnsiCursorShape};

use crate::color::{Rgb, builtin_palette};
use crate::palette::Palette;
use crate::snapshot::CursorShape;

/// Look up an index in the live palette. If the child never set it via OSC, fall back to
/// the theme `override_palette` for the slots it owns (ANSI 0-15, fg/bg/cursor), and to
/// the built-in xterm default for everything else (cube + gray ramp).
pub(crate) fn resolve_indexed(colors: &Colors, override_palette: &Palette, index: usize) -> Rgb {
    match colors[index] {
        Some(rgb) => rgb.into(),
        None => palette_fallback(override_palette, index),
    }
}

/// Theme-aware fallback for a palette index the live emulator has not set.
fn palette_fallback(palette: &Palette, index: usize) -> Rgb {
    match index {
        0..=15 => palette.ansi[index],
        256 => palette.foreground,
        257 => palette.background,
        258 => palette.cursor,
        other => builtin_palette(other),
    }
}

/// Resolve any `Color` to a concrete RGB value.
pub(crate) fn resolve_color(colors: &Colors, override_palette: &Palette, color: AnsiColor) -> Rgb {
    match color {
        AnsiColor::Spec(rgb) => rgb.into(),
        AnsiColor::Named(named) => resolve_indexed(colors, override_palette, named as usize),
        AnsiColor::Indexed(i) => resolve_indexed(colors, override_palette, i as usize),
    }
}

/// Brighten an ANSI 0-7 foreground colour to its 8-15 bright variant (for BOLD).
pub(crate) fn brighten(color: AnsiColor) -> AnsiColor {
    match color {
        AnsiColor::Named(named) if (named as usize) < 8 => AnsiColor::Named(named.to_bright()),
        AnsiColor::Indexed(i) if i < 8 => AnsiColor::Indexed(i + 8),
        other => other,
    }
}

/// Map alacritty's cursor shape to our public shape.
pub(crate) fn map_cursor_shape(shape: AnsiCursorShape) -> CursorShape {
    match shape {
        AnsiCursorShape::Block => CursorShape::Block,
        AnsiCursorShape::Underline => CursorShape::Underline,
        AnsiCursorShape::Beam => CursorShape::Beam,
        AnsiCursorShape::HollowBlock => CursorShape::Block,
        AnsiCursorShape::Hidden => CursorShape::Hidden,
    }
}
