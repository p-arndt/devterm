//! The core terminal wrapper.
//!
//! Owns an `alacritty_terminal::Term` plus the VT `Processor`, feeds it PTY bytes, and
//! resolves every cell colour to concrete [`Rgb`] when building a [`Snapshot`]. Also
//! houses the event listener, grid dimensions, and the colour-resolution helpers.

use std::cell::Cell as StdCell;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term as ATerm, TermMode};
use alacritty_terminal::vte::ansi::{NamedColor, Processor};

use crate::palette::Palette;
use crate::resolve::{brighten, map_cursor_shape, resolve_color, resolve_indexed};
use crate::selection::SelectionMode;
use crate::snapshot::{Cursor, CursorShape, RenderCell, Snapshot};

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
    /// Theme override for palette-backed default colours (see [`Palette`]).
    palette: Palette,
    /// Fallback cursor shape used when the running program has not explicitly chosen one
    /// (i.e. it still reports the default [`CursorShape::Block`]). See
    /// [`set_default_cursor_shape`](Term::set_default_cursor_shape).
    default_cursor_shape: CursorShape,
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
            palette: Palette::default(),
            default_cursor_shape: CursorShape::Block,
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
        let palette = &self.palette;
        let default_fg = resolve_indexed(colors, palette, NamedColor::Foreground as usize);
        let default_bg = resolve_indexed(colors, palette, NamedColor::Background as usize);
        let cursor_color = resolve_indexed(colors, palette, NamedColor::Cursor as usize);

        // Active selection range in grid coordinates (same space as `indexed.point`).
        let selection_range = self
            .term
            .selection
            .as_ref()
            .and_then(|s| s.to_range(&self.term));

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

            let mut fg = resolve_color(colors, palette, fg_color);
            let mut bg = resolve_color(colors, palette, cell.bg);

            if flags.contains(Flags::DIM) {
                fg = fg.dimmed();
            }

            // Membership in the active selection (grid coordinates).
            let selected = selection_range.is_some_and(|r| r.contains(indexed.point));

            // Inverse video swaps fg/bg. Selection also inverts, so the two compose as a
            // real XOR: a selected inverse cell renders non-inverse.
            let invert = flags.contains(Flags::INVERSE) ^ selected;
            if invert {
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
            // background, or a line decoration. Selected cells are always emitted so a
            // selection over empty space is visible.
            let is_blank = c == ' ' && bg == default_bg && !underline && !strikeout;
            if is_blank && !selected {
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
        // alacritty does not distinguish "program left the default" from "program
        // explicitly chose Block", so we treat a reported Block as unset and let our
        // configured default win. Any non-Block shape is an explicit program choice
        // (DECSCUSR) and is honoured verbatim.
        let reported_shape = map_cursor_shape(content.cursor.shape);
        let shape = if reported_shape == CursorShape::Block {
            self.default_cursor_shape
        } else {
            reported_shape
        };
        let cursor = Cursor {
            line: cursor_line as u16,
            col: cursor_col,
            shape,
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

    /// Snap the display back to the live view (bottom of the grid), discarding any
    /// scrollback offset. Marks the term dirty.
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
        self.dirty.set(true);
    }

    /// Set the fallback cursor shape used when the running program has not explicitly
    /// chosen one. Because alacritty reports the default as [`CursorShape::Block`] and
    /// does not expose "unset", the override only replaces a reported `Block`; any
    /// explicit non-Block shape from the program (DECSCUSR) still wins. Marks dirty.
    pub fn set_default_cursor_shape(&mut self, shape: CursorShape) {
        self.default_cursor_shape = shape;
        self.dirty.set(true);
    }

    /// Whether the grid changed since the last `snapshot()` (redraw hint).
    pub fn dirty(&self) -> bool {
        self.dirty.get()
    }

    /// Install a theme override for the palette-backed default colours. Takes effect on
    /// the next [`snapshot`](Term::snapshot); marks the term dirty.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
        self.dirty.set(true);
    }

    /// Convert a viewport coordinate (line 0 = top visible row) to an alacritty grid
    /// `Point`, accounting for the current scrollback display offset.
    fn viewport_point(&self, col: u16, line: u16) -> Point {
        let display_offset = self.term.grid().display_offset() as i32;
        Point::new(Line(line as i32 - display_offset), Column(col as usize))
    }

    /// Begin a selection anchored at viewport `(col, line)`. Replaces any existing
    /// selection; marks the term dirty.
    pub fn start_selection(&mut self, col: u16, line: u16, mode: SelectionMode) {
        let point = self.viewport_point(col, line);
        self.term.selection = Some(Selection::new(mode.to_alacritty(), point, Side::Left));
        self.dirty.set(true);
    }

    /// Extend the active selection to viewport `(col, line)`. No-op if no selection is
    /// active; marks the term dirty.
    pub fn update_selection(&mut self, col: u16, line: u16) {
        let point = self.viewport_point(col, line);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, Side::Right);
            self.dirty.set(true);
        }
    }

    /// Drop the active selection; marks the term dirty.
    pub fn clear_selection(&mut self) {
        self.term.selection = None;
        self.dirty.set(true);
    }

    /// The currently selected text, or `None` if there is no selection or it is empty.
    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// Whether the child enabled bracketed paste mode (DECSET 2004). The app wraps pasted
    /// text in the CSI 200~/201~ markers when this is true.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Whether a synchronized update (DECSET 2026) is currently in progress. The app may
    /// defer painting until it ends to avoid flicker.
    ///
    /// Delegates to the underlying vte `Processor`, which already buffers synchronized
    /// output and tracks the begin/end sequences internally — so no hand-rolled parser is
    /// needed.
    pub fn in_synchronized_update(&self) -> bool {
        self.parser.sync_timeout().sync_timeout().is_some()
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{DEFAULT_FG, Rgb};

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

    #[test]
    fn selection_inverts_cells_and_yields_text() {
        let mut term = Term::new(20, 5, 100);
        term.advance(b"hi");
        term.start_selection(0, 0, SelectionMode::Simple);
        term.update_selection(1, 0);
        assert!(term.dirty());

        let snap = term.snapshot();
        let h = cell_at(&snap, 0, 0).expect("selected 'h'");
        let i = cell_at(&snap, 0, 1).expect("selected 'i'");
        // Selected cells render inverse: fg/bg swapped vs the defaults.
        assert_eq!(h.fg, snap.default_bg);
        assert_eq!(h.bg, snap.default_fg);
        assert_eq!(i.fg, snap.default_bg);
        assert_eq!(i.bg, snap.default_fg);

        assert_eq!(term.selected_text().as_deref(), Some("hi"));
    }

    #[test]
    fn selection_and_inverse_cancel_out() {
        let mut term = Term::new(20, 5, 100);
        // SGR 7 = inverse, print 'Z'.
        term.advance(b"\x1b[7mZ");
        term.start_selection(0, 0, SelectionMode::Simple);
        term.update_selection(0, 0);
        let snap = term.snapshot();
        let z = cell_at(&snap, 0, 0).expect("cell 'Z'");
        // Inverse XOR selection => back to normal orientation.
        assert_eq!(z.fg, snap.default_fg);
        assert_eq!(z.bg, snap.default_bg);
    }

    #[test]
    fn clear_selection_removes_highlight() {
        let mut term = Term::new(20, 5, 100);
        term.advance(b"hi");
        term.start_selection(0, 0, SelectionMode::Simple);
        term.update_selection(1, 0);
        let _ = term.snapshot();

        term.clear_selection();
        assert!(term.selected_text().is_none());

        let snap = term.snapshot();
        let h = cell_at(&snap, 0, 0).expect("cell 'h'");
        // Back to normal foreground on default background.
        assert_eq!(h.fg, snap.default_fg);
        assert_eq!(h.bg, snap.default_bg);
    }

    #[test]
    fn set_palette_changes_default_foreground() {
        let mut term = Term::new(20, 5, 100);
        term.advance(b"h");

        let palette = Palette {
            foreground: Rgb::new(0x11, 0x22, 0x33),
            ..Palette::default()
        };
        term.set_palette(palette);
        assert!(term.dirty());

        let snap = term.snapshot();
        assert_eq!(snap.default_fg, Rgb::new(0x11, 0x22, 0x33));
        let h = cell_at(&snap, 0, 0).expect("cell 'h'");
        assert_eq!(h.fg, Rgb::new(0x11, 0x22, 0x33));
    }

    #[test]
    fn set_palette_overrides_ansi_color() {
        let mut term = Term::new(20, 5, 100);
        // SGR 31 = ANSI red (index 1).
        term.advance(b"\x1b[31mX");

        let mut palette = Palette::default();
        palette.ansi[1] = Rgb::new(0xab, 0xcd, 0xef);
        term.set_palette(palette);

        let snap = term.snapshot();
        let x = cell_at(&snap, 0, 0).expect("cell 'X'");
        assert_eq!(x.fg, Rgb::new(0xab, 0xcd, 0xef));
    }

    #[test]
    fn bracketed_paste_flips_after_enable() {
        let mut term = Term::new(20, 5, 100);
        assert!(!term.bracketed_paste());
        // DECSET 2004 = enable bracketed paste.
        term.advance(b"\x1b[?2004h");
        assert!(term.bracketed_paste());
        // DECRST 2004 = disable.
        term.advance(b"\x1b[?2004l");
        assert!(!term.bracketed_paste());
    }

    #[test]
    fn synchronized_update_tracks_2026() {
        let mut term = Term::new(20, 5, 100);
        assert!(!term.in_synchronized_update());
        // DECSET 2026 = begin synchronized update.
        term.advance(b"\x1b[?2026h");
        assert!(term.in_synchronized_update());
        // DECRST 2026 = end synchronized update.
        term.advance(b"\x1b[?2026l");
        assert!(!term.in_synchronized_update());
    }

    // ---------------------------------------------------------------------
    // Anti-flicker regression (PLAN.md section 4, the "PSmux bug").
    // ---------------------------------------------------------------------

    /// A representative byte stream exercising plain text on several rows, SGR colour /
    /// attribute changes, cursor moves, a line clear, and a DECSET-2026 synchronized
    /// update block. Deliberately fixed (no randomness) so the test is deterministic.
    fn representative_stream() -> Vec<u8> {
        let mut s: Vec<u8> = Vec::new();
        s.extend_from_slice(b"Hello world"); // row 0 plain text
        s.extend_from_slice(b"\x1b[2;1H"); // CUP row 2 col 1
        s.extend_from_slice(b"\x1b[31mred\x1b[0m"); // SGR red then reset
        s.extend_from_slice(b"\x1b[3;1H\x1b[1mbold\x1b[0m"); // row 3, bold
        s.extend_from_slice(b"\x1b[4;1H\x1b[3mital\x1b[0m"); // row 4, italic
        s.extend_from_slice(b"\x1b[5;1H\x1b[4mundr\x1b[0m"); // row 5, underline
        s.extend_from_slice(b"\x1b[6;1HZZZZZZ"); // row 6, junk to be cleared
        s.extend_from_slice(b"\x1b[6;1H\x1b[2K"); // clear line 6
        // Synchronized-update block: begin, draw, end.
        s.extend_from_slice(b"\x1b[?2026h");
        s.extend_from_slice(b"\x1b[7;1H\x1b[32msync\x1b[0m");
        s.extend_from_slice(b"\x1b[8;1Hmore");
        s.extend_from_slice(b"\x1b[?2026l");
        s.extend_from_slice(b"\x1b[9;1Hafter"); // draw after the sync block
        s
    }

    /// Deterministic, sortable projection of a snapshot: every visible cell plus the
    /// cursor line/col/shape. Two snapshots with equal projections are visually identical.
    #[allow(clippy::type_complexity)]
    fn project(
        snap: &Snapshot,
    ) -> (
        Vec<(u16, u16, char, Rgb, Rgb, bool, bool, bool)>,
        (u16, u16, CursorShape),
    ) {
        let mut cells: Vec<(u16, u16, char, Rgb, Rgb, bool, bool, bool)> = snap
            .cells
            .iter()
            .map(|c| {
                (
                    c.line,
                    c.col,
                    c.c,
                    c.fg,
                    c.bg,
                    c.bold,
                    c.italic,
                    c.underline,
                )
            })
            .collect();
        cells.sort_by_key(|t| (t.0, t.1));
        (
            cells,
            (snap.cursor.line, snap.cursor.col, snap.cursor.shape),
        )
    }

    #[test]
    fn chunk_invariance_final_state_identical() {
        let stream = representative_stream();

        // Reference: fed whole.
        let mut whole = Term::new(20, 12, 100);
        whole.advance(&stream);
        let reference = project(&whole.snapshot());

        // For every fixed chunk size, feeding the same bytes in that many pieces must
        // land on byte-for-byte the same final projection. This walks essentially every
        // split boundary of the stream.
        for chunk in 1..=stream.len() {
            let mut term = Term::new(20, 12, 100);
            for piece in stream.chunks(chunk) {
                term.advance(piece);
            }
            let got = project(&term.snapshot());
            assert_eq!(got, reference, "mismatch at chunk size {chunk}");
        }
    }

    #[test]
    fn no_settle_mid_sync_across_split() {
        let mut term = Term::new(20, 8, 100);

        // Feed the BSU one byte at a time; the flag must stay false until the final byte
        // of the sequence lands, then flip to true. This proves a split *inside* the BSU
        // never settles the frame early.
        let bsu = b"\x1b[?2026h";
        for (i, b) in bsu.iter().enumerate() {
            term.advance(&[*b]);
            let last = i + 1 == bsu.len();
            assert_eq!(
                term.in_synchronized_update(),
                last,
                "BSU flag wrong after {} byte(s)",
                i + 1
            );
        }

        // Drawing inside the block keeps the flag set.
        term.advance(b"\x1b[1;1Hdrawing");
        assert!(term.in_synchronized_update());

        // Feed the ESU one byte at a time; the flag must stay true until the final byte.
        let esu = b"\x1b[?2026l";
        for (i, b) in esu.iter().enumerate() {
            term.advance(&[*b]);
            let last = i + 1 == esu.len();
            assert_eq!(
                term.in_synchronized_update(),
                !last,
                "ESU flag wrong after {} byte(s)",
                i + 1
            );
        }
        assert!(!term.in_synchronized_update());
    }

    #[test]
    fn scroll_to_bottom_snaps_to_live() {
        let mut term = Term::new(10, 3, 100);
        // Produce enough lines to build scrollback beyond the 3 visible rows.
        for _ in 0..10 {
            term.advance(b"line\r\n");
        }
        term.scroll_display(4); // scroll up into history
        let snap = term.snapshot();
        assert!(snap.scrollback_offset > 0, "expected to be scrolled up");

        term.scroll_to_bottom();
        assert!(term.dirty());
        let snap = term.snapshot();
        assert_eq!(snap.scrollback_offset, 0);
    }

    #[test]
    fn default_cursor_shape_override_applies() {
        let mut term = Term::new(20, 5, 100);
        term.advance(b"x");

        // No program choice, no override yet: the hardcoded default is Block.
        assert_eq!(term.snapshot().cursor.shape, CursorShape::Block);

        // Overriding the default is reflected in the next snapshot.
        term.set_default_cursor_shape(CursorShape::Beam);
        assert!(term.dirty());
        assert_eq!(term.snapshot().cursor.shape, CursorShape::Beam);
    }

    #[test]
    fn explicit_cursor_shape_beats_default_override() {
        let mut term = Term::new(20, 5, 100);
        term.set_default_cursor_shape(CursorShape::Beam);
        // DECSCUSR 4 = steady underline: an explicit program choice.
        term.advance(b"\x1b[4 q");
        // The program's explicit non-Block choice wins over the default override.
        assert_eq!(term.snapshot().cursor.shape, CursorShape::Underline);
    }

    #[test]
    fn wide_char_occupies_two_columns_without_spacer_cell() {
        let mut term = Term::new(20, 5, 100);
        // A full-width CJK ideograph (width 2) followed by an ASCII 'X'.
        term.advance("世X".as_bytes());
        let snap = term.snapshot();

        // Exactly one emitted cell carries the wide glyph, at column 0, flagged wide.
        let wide_cells: Vec<&RenderCell> = snap.cells.iter().filter(|c| c.wide).collect();
        assert_eq!(wide_cells.len(), 1);
        let w = wide_cells[0];
        assert_eq!(w.c, '世');
        assert_eq!(w.col, 0);
        assert!(w.wide);

        // No stray cell at the spacer column (col 1).
        assert!(cell_at(&snap, 0, 1).is_none());

        // The trailing 'X' lands at column 2, not column 1.
        let x = cell_at(&snap, 0, 2).expect("'X' after the wide glyph");
        assert_eq!(x.c, 'X');
        assert!(!x.wide);

        // Cursor sits just past the 'X': columns 0-1 (wide) + 2 (X) => col 3.
        assert_eq!(snap.cursor.col, 3);
    }
}
