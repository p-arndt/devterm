//! Terminal emulation for DevTerm.
//!
//! Wraps `alacritty_terminal` (VT parser + grid + scrollback) — we do **not** write our
//! own emulator; that is the single biggest risk of the project (years of edge cases).
//! This crate owns:
//! - feeding PTY bytes into the emulator,
//! - resolving every cell colour to concrete RGB for a "dumb" renderer,
//! - exposing an RGB snapshot + cursor for the renderer,
//! - collecting bytes the emulator wants written back to the child (DSR/DA replies).

#![forbid(unsafe_code)]

use std::cell::Cell as StdCell;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Term as ATerm};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape as AnsiCursorShape, NamedColor, Processor, Rgb as AnsiRgb,
};

// ---------------------------------------------------------------------------
// Public value types (see docs/M0_INTERFACES.md — implemented verbatim).
// ---------------------------------------------------------------------------

/// 24-bit RGB colour, fully resolved (no palette indices leave this crate).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Darken for the `DIM` attribute (~2/3 brightness).
    fn dimmed(self) -> Self {
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

// ---------------------------------------------------------------------------
// Built-in default palette (xterm-256): 16 ANSI + 6x6x6 cube + 24 grays.
// ---------------------------------------------------------------------------

/// Standard system palette for the first 16 ANSI colours.
const ANSI_16: [Rgb; 16] = [
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
const DEFAULT_FG: Rgb = Rgb::new(0xd0, 0xd0, 0xd0);
/// Default background when the palette has no override.
const DEFAULT_BG: Rgb = Rgb::new(0x00, 0x00, 0x00);

/// Resolve a palette index (0..=268) to an RGB value using the built-in xterm-256
/// default palette. Handles the ANSI 16, the 6x6x6 cube, the 24-step gray ramp, and
/// the named foreground/background/cursor slots.
fn builtin_palette(index: usize) -> Rgb {
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

// ---------------------------------------------------------------------------
// Event listener: captures PtyWrite bytes and the window title.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SharedState {
    /// Bytes the emulator wants written back to the child.
    pty_writes: Vec<u8>,
    /// Current window title, if the child set one.
    title: Option<String>,
}

/// `EventListener` implementation stored inside the alacritty `Term`. It shares state
/// with the outer [`Term`] wrapper through an `Arc<Mutex<..>>`.
#[derive(Clone)]
struct Listener {
    state: Arc<Mutex<SharedState>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                if let Ok(mut state) = self.state.lock() {
                    state.pty_writes.extend_from_slice(text.as_bytes());
                }
            }
            Event::Title(title) => {
                if let Ok(mut state) = self.state.lock() {
                    state.title = Some(title);
                }
            }
            Event::ResetTitle => {
                if let Ok(mut state) = self.state.lock() {
                    state.title = None;
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Grid dimensions.
// ---------------------------------------------------------------------------

/// Our own `Dimensions` implementation. `total_lines` = visible rows + scrollback.
#[derive(Clone, Copy)]
struct Dims {
    columns: usize,
    screen_lines: usize,
    total_lines: usize,
}

impl Dims {
    fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Self {
        let screen_lines = rows as usize;
        Self {
            columns: cols as usize,
            screen_lines,
            total_lines: screen_lines + scrollback_lines,
        }
    }
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.total_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

// ---------------------------------------------------------------------------
// Term wrapper.
// ---------------------------------------------------------------------------

/// Wraps an `alacritty_terminal::Term` plus the VT `Processor`. Single-threaded: lives
/// on the app thread. All colours are resolved to RGB when building a [`Snapshot`].
pub struct Term {
    term: ATerm<Listener>,
    parser: Processor,
    state: Arc<Mutex<SharedState>>,
    scrollback_lines: usize,
    /// Set by `advance`/`resize`/`scroll_display`, cleared by `snapshot`.
    dirty: StdCell<bool>,
}

impl Term {
    /// New terminal of `cols` x `rows` with `scrollback_lines` of history.
    pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Self {
        let state = Arc::new(Mutex::new(SharedState::default()));
        let listener = Listener {
            state: state.clone(),
        };

        let config = Config {
            scrolling_history: scrollback_lines,
            ..Config::default()
        };
        let dims = Dims::new(cols, rows, scrollback_lines);
        let term = ATerm::new(config, &dims, listener);

        Self {
            term,
            parser: Processor::new(),
            state,
            scrollback_lines,
            dirty: StdCell::new(true),
        }
    }

    /// Feed raw PTY output through the parser (updates the grid in place).
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.dirty.set(true);
    }

    /// Resize the grid (reflow handled by alacritty).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let dims = Dims::new(cols, rows, self.scrollback_lines);
        self.term.resize(dims);
        self.dirty.set(true);
    }

    /// Build the render snapshot from current grid state.
    pub fn snapshot(&self) -> Snapshot {
        let content = self.term.renderable_content();

        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;
        let display_offset = content.display_offset;

        // Resolve the palette-backed default colours once.
        let colors = content.colors;
        let default_fg = resolve_indexed(colors, NamedColor::Foreground as usize);
        let default_bg = resolve_indexed(colors, NamedColor::Background as usize);
        let cursor_color = resolve_indexed(colors, NamedColor::Cursor as usize);

        let mut cells = Vec::new();

        for indexed in content.display_iter {
            let cell = indexed.cell;
            let flags = cell.flags;

            // Skip the trailing/leading spacer halves of wide characters; the wide
            // glyph itself carries the WIDE_CHAR flag and covers both columns.
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }

            // Viewport line: 0 = top visible row.
            let line = indexed.point.line.0 + display_offset as i32;
            if line < 0 || line >= rows as i32 {
                continue;
            }
            let col = indexed.point.column.0;
            if col >= cols as usize {
                continue;
            }

            // Bold may brighten ANSI 0-7 foregrounds to 8-15.
            let bold = flags.contains(Flags::BOLD);
            let fg_color = if bold { brighten(cell.fg) } else { cell.fg };

            let mut fg = resolve_color(colors, fg_color);
            let mut bg = resolve_color(colors, cell.bg);

            if flags.contains(Flags::DIM) {
                fg = fg.dimmed();
            }

            // Inverse swaps foreground and background.
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            // Hidden text renders as blank (but keeps its background).
            let c = if flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                cell.c
            };

            let underline = flags.contains(Flags::UNDERLINE);
            let strikeout = flags.contains(Flags::STRIKEOUT);

            // Only emit non-blank cells: anything with visible glyph, a non-default
            // background, or a line decoration.
            let is_blank = c == ' ' && bg == default_bg && !underline && !strikeout;
            if is_blank {
                continue;
            }

            cells.push(RenderCell {
                line: line as u16,
                col: col as u16,
                c,
                fg,
                bg,
                bold,
                italic: flags.contains(Flags::ITALIC),
                underline,
                strikeout,
                wide: flags.contains(Flags::WIDE_CHAR),
            });
        }

        // Cursor: convert from grid coordinates to viewport coordinates.
        let cursor_point = content.cursor.point;
        let cursor_line = (cursor_point.line.0 + display_offset as i32).clamp(0, rows as i32 - 1);
        let cursor_col = (cursor_point.column.0 as u16).min(cols.saturating_sub(1));
        let cursor = Cursor {
            line: cursor_line as u16,
            col: cursor_col,
            shape: map_cursor_shape(content.cursor.shape),
            color: cursor_color,
        };

        let title = self.state.lock().ok().and_then(|s| s.title.clone());

        self.dirty.set(false);

        Snapshot {
            cols,
            rows,
            cells,
            cursor,
            default_fg,
            default_bg,
            scrollback_offset: display_offset,
            title,
        }
    }

    /// Bytes the emulator wants written back to the child (DSR/DA replies, etc.).
    /// The app must forward these to `Pty::write`.
    pub fn drain_pty_writes(&mut self) -> Vec<u8> {
        match self.state.lock() {
            Ok(mut state) => std::mem::take(&mut state.pty_writes),
            Err(_) => Vec::new(),
        }
    }

    /// Scroll the display: positive = up into history, negative = toward live.
    pub fn scroll_display(&mut self, delta_lines: i32) {
        self.term.scroll_display(Scroll::Delta(delta_lines));
        self.dirty.set(true);
    }

    /// Whether the grid changed since the last `snapshot()` (redraw hint).
    pub fn dirty(&self) -> bool {
        self.dirty.get()
    }
}

// ---------------------------------------------------------------------------
// Colour resolution helpers.
// ---------------------------------------------------------------------------

/// Look up an index in the live palette, falling back to the built-in default palette.
fn resolve_indexed(colors: &Colors, index: usize) -> Rgb {
    match colors[index] {
        Some(rgb) => rgb.into(),
        None => builtin_palette(index),
    }
}

/// Resolve any `Color` to a concrete RGB value.
fn resolve_color(colors: &Colors, color: AnsiColor) -> Rgb {
    match color {
        AnsiColor::Spec(rgb) => rgb.into(),
        AnsiColor::Named(named) => resolve_indexed(colors, named as usize),
        AnsiColor::Indexed(i) => resolve_indexed(colors, i as usize),
    }
}

/// Brighten an ANSI 0-7 foreground colour to its 8-15 bright variant (for BOLD).
fn brighten(color: AnsiColor) -> AnsiColor {
    match color {
        AnsiColor::Named(named) if (named as usize) < 8 => AnsiColor::Named(named.to_bright()),
        AnsiColor::Indexed(i) if i < 8 => AnsiColor::Indexed(i + 8),
        other => other,
    }
}

/// Map alacritty's cursor shape to our public shape.
fn map_cursor_shape(shape: AnsiCursorShape) -> CursorShape {
    match shape {
        AnsiCursorShape::Block => CursorShape::Block,
        AnsiCursorShape::Underline => CursorShape::Underline,
        AnsiCursorShape::Beam => CursorShape::Beam,
        AnsiCursorShape::HollowBlock => CursorShape::Block,
        AnsiCursorShape::Hidden => CursorShape::Hidden,
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_at(snap: &Snapshot, line: u16, col: u16) -> Option<&RenderCell> {
        snap.cells.iter().find(|c| c.line == line && c.col == col)
    }

    #[test]
    fn writes_plain_text() {
        let mut term = Term::new(20, 5, 1000);
        term.advance(b"hi");
        let snap = term.snapshot();

        let h = cell_at(&snap, 0, 0).expect("cell 'h' present");
        let i = cell_at(&snap, 0, 1).expect("cell 'i' present");
        assert_eq!(h.c, 'h');
        assert_eq!(i.c, 'i');
        // Default foreground applied.
        assert_eq!(h.fg, DEFAULT_FG);
    }

    #[test]
    fn sgr_red_sets_foreground() {
        let mut term = Term::new(20, 5, 1000);
        // SGR 31 = ANSI red, then print 'X'.
        term.advance(b"\x1b[31mX");
        let snap = term.snapshot();

        let x = cell_at(&snap, 0, 0).expect("cell 'X' present");
        assert_eq!(x.c, 'X');
        // ANSI red from the built-in palette (index 1).
        assert_eq!(x.fg, Rgb::new(0x80, 0x00, 0x00));
        assert_eq!(x.fg.g, 0);
        assert_eq!(x.fg.b, 0);
        assert!(x.fg.r > 0);
    }

    #[test]
    fn cursor_move_updates_position() {
        let mut term = Term::new(20, 10, 1000);
        // CUP: move cursor to row 3, column 5 (1-based) -> line 2, col 4 (0-based).
        term.advance(b"\x1b[3;5H");
        let snap = term.snapshot();

        assert_eq!(snap.cursor.line, 2);
        assert_eq!(snap.cursor.col, 4);
    }

    #[test]
    fn dirty_flag_tracks_changes() {
        let mut term = Term::new(10, 3, 100);
        // Fresh terminal starts dirty.
        assert!(term.dirty());
        let _ = term.snapshot();
        assert!(!term.dirty());
        term.advance(b"a");
        assert!(term.dirty());
    }

    #[test]
    fn bold_brightens_ansi_foreground() {
        let mut term = Term::new(20, 5, 100);
        // SGR 1 (bold) + 31 (red) -> bright red.
        term.advance(b"\x1b[1;31mB");
        let snap = term.snapshot();
        let b = cell_at(&snap, 0, 0).expect("cell 'B' present");
        assert!(b.bold);
        assert_eq!(b.fg, Rgb::new(0xff, 0x00, 0x00));
    }

    #[test]
    fn inverse_swaps_colors() {
        let mut term = Term::new(20, 5, 100);
        // SGR 7 = inverse.
        term.advance(b"\x1b[7mZ");
        let snap = term.snapshot();
        let z = cell_at(&snap, 0, 0).expect("cell 'Z' present");
        // After inverse, fg becomes the default background and bg the default fg.
        assert_eq!(z.fg, snap.default_bg);
        assert_eq!(z.bg, snap.default_fg);
    }
}
