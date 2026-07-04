//! Render-facing frame types.
//!
//! The [`Snapshot`] and its constituent [`RenderCell`] / [`Cursor`] / [`CursorShape`]
//! are everything a "dumb" renderer needs to paint one frame: every colour is already
//! resolved to concrete [`Rgb`], with inverse/dim/bold-brighten applied.

use crate::color::Rgb;

/// Rendered cursor shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    Hidden,
}

/// One rendered cell; fg/bg already have inverse/dim/bold-brighten applied.
#[derive(Clone, Copy, Debug)]
pub struct RenderCell {
    /// 0 = top visible row.
    pub line: u16,
    /// 0 = leftmost column.
    pub col: u16,
    pub c: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    /// Glyph occupies two columns.
    pub wide: bool,
}

/// Cursor position, shape and colour for one frame.
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub line: u16,
    pub col: u16,
    pub shape: CursorShape,
    pub color: Rgb,
}

/// Everything the renderer needs for one frame. Only non-blank cells are listed; the
/// renderer paints `default_bg` everywhere first.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<RenderCell>,
    pub cursor: Cursor,
    pub default_fg: Rgb,
    pub default_bg: Rgb,
    /// Rows scrolled up from the bottom (0 = live).
    pub scrollback_offset: usize,
    pub title: Option<String>,
}
