//! The tab bar: a one-cell-high strip across the top of the window.
//!
//! Always visible, styled after modern terminal tab strips: the strip itself is a
//! lightened tint of the theme background, inactive tabs sit as slightly darker blocks
//! on it, and the active tab shares the terminal's own background so it reads as
//! connected to the pane below. Every tab carries a trailing `×` close button and the
//! strip ends with a `+` new-tab button. Like the settings overlay, the bar is
//! synthesized into a [`Snapshot`] and painted through the ordinary cell renderer — no
//! separate draw path. [`segments`] is the single source of truth for where each label
//! sits, shared by the painter and the click hit-test so they can never disagree.

use devterm_term::{Cursor, CursorShape, Palette, RenderCell, Rgb, Snapshot};

use super::state::Tab;

/// Longest tab label (longer titles are truncated with an ellipsis).
const MAX_TITLE: usize = 20;
/// Blank columns between adjacent tab blocks.
const GAP: u16 = 1;

/// What a click on the bar landed on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BarClick {
    /// A tab's label: switch to it.
    Tab(usize),
    /// A tab's `×` button: close it.
    Close(usize),
    /// The trailing `+` button: open a new tab.
    NewTab,
    /// The bar itself, between labels: consume the click but do nothing.
    Empty,
}

/// Linear blend `a -> b` by `t` (0 = `a`, 1 = `b`), used to derive the bar's tints from
/// the theme so it works on any palette without new config colours.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let ch = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb {
        r: ch(a.r, b.r),
        g: ch(a.g, b.g),
        b: ch(a.b, b.b),
    }
}

/// A tab's display label: the focused pane's reported (OSC) title reduced to something
/// human ("C:\Program Files\PowerShell\7\pwsh.exe" -> "pwsh"), else "Tab N".
fn label(tab: &Tab, index: usize) -> String {
    let title = tab
        .panes
        .get(&tab.layout.focused())
        .and_then(|pane| pane.last_snapshot.as_ref())
        .and_then(|snap| snap.title.clone())
        .map(|title| humanize(&title))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| format!("Tab {}", index + 1));
    if title.chars().count() > MAX_TITLE {
        title.chars().take(MAX_TITLE - 1).collect::<String>() + "…"
    } else {
        title
    }
}

/// Reduce a reported window title to a short display name: paths collapse to their last
/// component and a trailing `.exe` is dropped, so shells that report their binary path
/// (or their CWD) show as `pwsh` / `7` instead of a truncated `C:\Program Files\…`.
fn humanize(title: &str) -> String {
    let trimmed = title.trim();
    let last = trimmed
        .rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed);
    let last = last
        .strip_suffix(".exe")
        .or_else(|| last.strip_suffix(".EXE"))
        .unwrap_or(last);
    last.to_owned()
}

/// One entry on the bar with its column extent and text.
struct Segment {
    start: u16,
    len: u16,
    text: String,
}

/// Each tab's block, in tab order, followed by one extra entry for the `+` button. A tab
/// block renders as ` <text> × ` (the `×` is its close button); entries past the right
/// edge get zero-length segments (invisible and unclickable).
fn segments(tabs: &[Tab], cols: u16) -> Vec<Segment> {
    let mut out = Vec::with_capacity(tabs.len() + 1);
    let mut col: u16 = 0;
    for (i, tab) in tabs.iter().enumerate() {
        let text = label(tab, i);
        // " text × " = text + 2 padding + 2 for the close button.
        let len = (text.chars().count() as u16 + 4).min(cols.saturating_sub(col));
        out.push(Segment {
            start: col,
            len,
            text,
        });
        col = col.saturating_add(len).saturating_add(GAP);
    }
    // The `+` button.
    let len = 3.min(cols.saturating_sub(col));
    out.push(Segment {
        start: col,
        len,
        text: "+".to_owned(),
    });
    out
}

/// The column of the `×` inside a tab block (one cell before the trailing pad).
fn close_col(seg: &Segment) -> u16 {
    seg.start + seg.len.saturating_sub(2)
}

/// Synthesize the bar as a one-row [`Snapshot`] sized to `cols`.
pub(super) fn snapshot(tabs: &[Tab], active: usize, palette: &Palette, cols: u16) -> Snapshot {
    let fg = palette.foreground;
    let bg = palette.background;
    // Three depths derived from the theme: the strip (lightest), inactive tab blocks
    // (between), and the active tab, which uses the terminal background itself so it
    // visually connects to the pane below.
    let bar_bg = mix(bg, fg, 0.15);
    let inactive_bg = mix(bg, fg, 0.07);
    let inactive_fg = mix(bg, fg, 0.55);
    let close_fg = mix(bg, fg, 0.40);

    let mut cells: Vec<RenderCell> = Vec::new();
    let mut put = |col: u16, ch: char, cell_fg: Rgb, cell_bg: Rgb, bold: bool| {
        if col < cols {
            cells.push(RenderCell {
                line: 0,
                col,
                c: ch,
                fg: cell_fg,
                bg: cell_bg,
                bold,
                italic: false,
                underline: false,
                strikeout: false,
                wide: false,
            });
        }
    };

    let segs = segments(tabs, cols);
    let plus = segs.len() - 1;
    for (i, seg) in segs.iter().enumerate() {
        if i == plus {
            // The `+` button sits flat on the strip.
            for (j, ch) in " + ".chars().take(seg.len as usize).enumerate() {
                put(seg.start + j as u16, ch, inactive_fg, bar_bg, false);
            }
            continue;
        }
        let is_active = i == active;
        let (text_fg, block_bg) = if is_active {
            (fg, bg)
        } else {
            (inactive_fg, inactive_bg)
        };
        let x_fg = if is_active {
            mix(bg, fg, 0.70)
        } else {
            close_fg
        };
        let x_col = close_col(seg);
        let padded = format!(" {} ", seg.text);
        for j in 0..seg.len {
            let col = seg.start + j;
            if col == x_col {
                put(col, '×', x_fg, block_bg, false);
            } else if col == x_col + 1 {
                put(col, ' ', text_fg, block_bg, false);
            } else {
                let ch = padded.chars().nth(j as usize).unwrap_or(' ');
                put(col, ch, text_fg, block_bg, is_active);
            }
        }
    }

    Snapshot {
        cols,
        rows: 1,
        cells,
        cursor: Cursor {
            line: 0,
            col: 0,
            shape: CursorShape::Hidden,
            color: palette.cursor,
        },
        default_fg: fg,
        // The renderer fills the strip with the snapshot's default background, so the
        // tint covers the full width, not just the labelled cells.
        default_bg: bar_bg,
        scrollback_offset: 0,
        title: None,
    }
}

/// What a click on bar column `col` lands on.
pub(super) fn hit(tabs: &[Tab], cols: u16, col: u16) -> BarClick {
    let segs = segments(tabs, cols);
    let plus = segs.len() - 1;
    match segs
        .iter()
        .position(|seg| col >= seg.start && col < seg.start + seg.len)
    {
        Some(i) if i == plus => BarClick::NewTab,
        Some(i) if col == close_col(&segs[i]) => BarClick::Close(i),
        Some(i) => BarClick::Tab(i),
        None => BarClick::Empty,
    }
}
