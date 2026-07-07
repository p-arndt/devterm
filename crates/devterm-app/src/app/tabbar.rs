//! The titlebar: the integrated caption strip across the top of the (borderless) window.
//!
//! Modeled on Windows Terminal — the tabs live *inside* the titlebar alongside the window's
//! own minimize / maximize / close buttons, there is no separate OS title row. Tabs float
//! as rounded blocks fully inside the strip: the active tab is a clearly lighter raised
//! block with a bold label, inactive tabs are label-only (a hover block appears under the
//! pointer) with thin separators between them. Every tab carries a small terminal icon. A
//! trailing `+` opens a new tab and the empty area is a drag handle for moving the window.
//!
//! Unlike the panes, the strip is drawn with pixel-precise primitives (rounded rectangles +
//! text) rather than the cell grid, so tab shapes and button glyphs are not snapped to the
//! character grid. [`layout`] is the single source of truth for where everything sits: the
//! painter ([`build_chrome`]) and the click hit-test ([`hit`]) both derive from it so they
//! can never disagree. All coordinates here are physical pixels.

use devterm_core::Rect;
use devterm_render::{Chrome, ChromeRect, ChromeText, Renderer};
use devterm_term::{Palette, Rgb};

use super::state::Tab;

/// Longest tab label before width-based truncation (a hard cap so a runaway title cannot
/// blow up the natural tab width computation).
const MAX_TITLE: usize = 40;

// Layout constants, in *logical* pixels (multiplied by the DPI scale at layout time).
/// Width of each window-control button (minimize / maximize / close).
const CTRL_W: f32 = 46.0;
/// Maximum width of a single tab; longer labels are truncated with an ellipsis.
const TAB_MAX: f32 = 240.0;
/// Width of the trailing `+` new-tab button.
const PLUS_W: f32 = 34.0;
/// Gap between adjacent tab blocks (the separators live in this gap).
const TAB_GAP: f32 = 6.0;
/// Left inset before the first tab.
const LEFT_PAD: f32 = 8.0;
/// Inner left padding of a tab (before its icon) and right padding (after its close button).
const TAB_PAD_L: f32 = 10.0;
const TAB_PAD_R: f32 = 8.0;
/// Side length of the per-tab terminal icon plate, and the gap between it and the label.
const ICON_SIDE: f32 = 16.0;
const ICON_GAP: f32 = 8.0;
/// Gap between a tab's label and its close button.
const TAB_TEXT_GAP: f32 = 6.0;
/// Side length of a tab's `×` close button hit/hover square.
const CLOSE_SIDE: f32 = 18.0;
/// Vertical inset of a tab from the top of the strip (also the bottom margin of an inactive,
/// floating tab).
const TAB_TOP: f32 = 5.0;
/// Corner radius of tab blocks.
const TAB_RADIUS: f32 = 9.0;
/// Minimum label width (logical px) a tab must keep for its close button / icon to show.
const MIN_LABEL_W: f32 = 24.0;

/// What a point on the titlebar lands on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Hit {
    /// A tab body: switch to it.
    Tab(usize),
    /// A tab's `×`: close it.
    Close(usize),
    /// The trailing `+`: open a new tab.
    NewTab,
    /// The window minimize button.
    Minimize,
    /// The window maximize / restore button.
    Maximize,
    /// The window close button.
    WindowClose,
    /// Empty caption area: a drag handle (also double-click to maximize).
    Drag,
}

/// One tab's placement on the strip.
pub(super) struct TabRect {
    pub(super) index: usize,
    /// The tab's visual/hit rectangle (its x-extent is what selects the tab).
    pub(super) body: Rect,
    /// The `×` close square; zero-sized when the tab is too narrow to show one.
    pub(super) close: Rect,
    /// The little terminal icon plate; zero-sized when the tab is too narrow.
    pub(super) icon: Rect,
    /// The tab's (already length-capped) label, truncated to `body` width at paint time.
    pub(super) label: String,
}

/// The full titlebar geometry in physical pixels, shared by the painter and hit-test.
pub(super) struct Layout {
    /// Strip height (physical px).
    pub(super) height: f32,
    /// Full strip width (physical px).
    pub(super) width: f32,
    pub(super) scale: f32,
    pub(super) tabs: Vec<TabRect>,
    pub(super) plus: Rect,
    pub(super) min: Rect,
    pub(super) max: Rect,
    pub(super) close: Rect,
}

/// Linear blend `a -> b` by `t` (0 = `a`, 1 = `b`), used to derive the strip's tints from the
/// theme so it works on any palette without new config colours.
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
/// human ("C:\\Program Files\\PowerShell\\7\\pwsh.exe" -> "pwsh"), else "Tab N".
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
/// (or their CWD) show as `pwsh` / `7` instead of a truncated `C:\\Program Files\\…`.
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

/// Truncate `s` (with a trailing ellipsis) to at most `max_w` physical px of UI text.
fn truncate_to_width(renderer: &Renderer, s: &str, max_w: f32) -> String {
    if renderer.ui_text_width(s, 1.0) <= max_w {
        return s.to_owned();
    }
    let ellipsis_w = renderer.ui_char_advance('…', 1.0);
    let mut out = String::new();
    let mut w = 0.0;
    for c in s.chars() {
        let adv = renderer.ui_char_advance(c, 1.0);
        if w + adv + ellipsis_w > max_w {
            break;
        }
        out.push(c);
        w += adv;
    }
    if out.is_empty() && ellipsis_w > max_w {
        return String::new();
    }
    out + "…"
}

/// Compute the full titlebar geometry for a strip `width`×`height` (physical px). The
/// renderer measures the (proportional) label text; `scale` is the DPI factor (constants
/// above are logical px).
pub(super) fn layout(
    tabs: &[Tab],
    width: f32,
    height: f32,
    renderer: &Renderer,
    scale: f32,
) -> Layout {
    let s = scale;
    let ctrl_w = CTRL_W * s;

    // Window controls, right-to-left: close, maximize, minimize. Each is full strip height.
    let close = Rect::new(width - ctrl_w, 0.0, ctrl_w, height);
    let max = Rect::new(width - 2.0 * ctrl_w, 0.0, ctrl_w, height);
    let min = Rect::new(width - 3.0 * ctrl_w, 0.0, ctrl_w, height);
    let buttons_left = width - 3.0 * ctrl_w;

    let left = LEFT_PAD * s;
    let plus_w = PLUS_W * s;
    let tab_gap = TAB_GAP * s;
    let pad_l = TAB_PAD_L * s;
    let pad_r = TAB_PAD_R * s;
    let text_gap = TAB_TEXT_GAP * s;
    let close_side = CLOSE_SIDE * s;
    let tab_top = TAB_TOP * s;
    let tab_max = TAB_MAX * s;
    let icon_side = ICON_SIDE * s;
    let icon_gap = ICON_GAP * s;

    let labels: Vec<String> = tabs.iter().enumerate().map(|(i, t)| label(t, i)).collect();
    let n = labels.len();

    // Space available for all tabs (leaving room for the `+` after them).
    let avail = (buttons_left - left - plus_w - tab_gap).max(0.0);

    // Each tab's natural width from icon + label + close, capped; then shrink uniformly if
    // they overflow. Labels render proportionally, so measure them with the UI face.
    let natural: Vec<f32> = labels
        .iter()
        .map(|l| {
            let text = renderer.ui_text_width(l, 1.0);
            (pad_l + icon_side + icon_gap + text + text_gap + close_side + pad_r).min(tab_max)
        })
        .collect();
    let total: f32 = natural.iter().sum::<f32>() + tab_gap * (n.saturating_sub(1)) as f32;
    let widths: Vec<f32> = if n == 0 {
        Vec::new()
    } else if total <= avail {
        natural
    } else {
        let each = ((avail - tab_gap * (n - 1) as f32) / n as f32).max(0.0);
        vec![each; n]
    };

    let mut tab_rects = Vec::with_capacity(n);
    let mut x = left;
    for (i, w) in widths.iter().copied().enumerate() {
        // Tabs float fully inside the strip: inset from the top *and* the bottom.
        let body = Rect::new(x, tab_top, w, height - 2.0 * tab_top);
        // Only show a close button if the tab is wide enough to also keep a minimal label
        // — otherwise clicking anywhere on the (tiny) tab just selects it.
        let min_label = MIN_LABEL_W * s;
        let close_rect = if w >= pad_l + text_gap + close_side + pad_r + min_label {
            let cx = x + w - pad_r - close_side;
            let cy = (height - close_side) * 0.5;
            Rect::new(cx, cy, close_side, close_side)
        } else {
            Rect::new(0.0, 0.0, 0.0, 0.0)
        };
        // The icon needs the label to still have some room; drop it before the close button.
        let icon_rect =
            if w >= pad_l + icon_side + icon_gap + close_side + pad_r + min_label + text_gap {
                Rect::new(x + pad_l, (height - icon_side) * 0.5, icon_side, icon_side)
            } else {
                Rect::new(0.0, 0.0, 0.0, 0.0)
            };
        tab_rects.push(TabRect {
            index: i,
            body,
            close: close_rect,
            icon: icon_rect,
            label: labels[i].clone(),
        });
        x += w + tab_gap;
    }

    let plus = Rect::new(x, tab_top, plus_w, height - 2.0 * tab_top);

    Layout {
        height,
        width,
        scale: s,
        tabs: tab_rects,
        plus,
        min,
        max,
        close,
    }
}

/// Whether `px` falls in `r`'s horizontal extent (tabs/`+` are clickable over the full strip
/// height, so only their x-range matters).
fn in_x(r: &Rect, px: f32) -> bool {
    px >= r.x && px < r.x + r.w && r.w > 0.0
}

/// What titlebar element the point `(px, py)` lands on.
pub(super) fn hit(layout: &Layout, px: f32, py: f32) -> Hit {
    if py < 0.0 || py > layout.height {
        return Hit::Drag;
    }
    // Window controls take precedence (they sit at the far right, clear of the tabs).
    if in_x(&layout.close, px) {
        return Hit::WindowClose;
    }
    if in_x(&layout.max, px) {
        return Hit::Maximize;
    }
    if in_x(&layout.min, px) {
        return Hit::Minimize;
    }
    for tab in &layout.tabs {
        if in_x(&tab.body, px) {
            if tab.close.w > 0.0 && tab.close.contains(px, py) {
                return Hit::Close(tab.index);
            }
            return Hit::Tab(tab.index);
        }
    }
    if in_x(&layout.plus, px) {
        return Hit::NewTab;
    }
    Hit::Drag
}

/// Push a `t`-thick rectangular outline of the `w`×`h` box at `(x, y)` (four square quads).
fn frame(rects: &mut Vec<ChromeRect>, x: f32, y: f32, w: f32, h: f32, t: f32, color: Rgb) {
    let t = t.max(1.0).min(w).min(h);
    let mut push = |x: f32, y: f32, w: f32, h: f32| {
        rects.push(ChromeRect {
            x,
            y,
            w,
            h,
            color,
            radius: 0.0,
        })
    };
    push(x, y, w, t); // top
    push(x, y + h - t, w, t); // bottom
    push(x, y, t, h); // left
    push(x + w - t, y, t, h); // right
}

/// Build the titlebar chrome (rects + text) for the current tabs, active index, hovered
/// element and window-maximized state, tinted from `palette`. The renderer supplies the
/// UI-font metrics that place/centre the label + glyph text.
pub(super) fn build_chrome(
    layout: &Layout,
    active: usize,
    hovered: Option<Hit>,
    maximized: bool,
    palette: &Palette,
    renderer: &Renderer,
) -> Chrome {
    let fg = palette.foreground;
    let bg = palette.background;
    let s = layout.scale;
    let h = layout.height;

    // Theme-derived tints, modeled on Windows Terminal's dark chrome: a dark strip over
    // near-black content; the active tab is a clearly *lighter*, raised rounded block with
    // a bold label; inactive tabs are label-only (a subtler block appears under the
    // pointer) with thin separators floating between them.
    let strip_bg = mix(bg, fg, 0.06);
    let active_bg = mix(bg, fg, 0.17);
    let inactive_hover_bg = mix(bg, fg, 0.11);
    let inactive_fg = mix(bg, fg, 0.60);
    let separator = mix(bg, fg, 0.25);
    let glyph = mix(bg, fg, 0.60);
    let ctrl_hover_bg = mix(bg, fg, 0.14);
    let tab_close_hover_bg = mix(bg, fg, 0.28);
    let close_hover_bg = Rgb {
        r: 0xc4,
        g: 0x2b,
        b: 0x1c,
    };

    let radius = TAB_RADIUS * s;
    // Baseline that vertically centres a line of UI text in the strip.
    let (ui_h, ui_ascent) = renderer.ui_line(1.0);
    let text_baseline = (h - ui_h) * 0.5 + ui_ascent;

    let mut chrome = Chrome::default();

    // The strip background spans the full width (fills any gaps between elements).
    chrome.rects.push(ChromeRect {
        x: 0.0,
        y: 0.0,
        w: layout.width,
        h,
        color: strip_bg,
        radius: 0.0,
    });

    // --- tabs ---
    for tab in &layout.tabs {
        let is_active = tab.index == active;
        let hovered_tab = hovered == Some(Hit::Tab(tab.index));
        let close_hovered = hovered == Some(Hit::Close(tab.index));

        // The active tab is a raised, lighter rounded block floating in the strip (like
        // Windows Terminal's Win11 look). Inactive tabs draw no block; pointing at one
        // raises a subtler hover block of the same shape.
        if is_active || hovered_tab || close_hovered {
            chrome.rects.push(ChromeRect {
                x: tab.body.x,
                y: tab.body.y,
                w: tab.body.w,
                h: tab.body.h,
                color: if is_active {
                    active_bg
                } else {
                    inactive_hover_bg
                },
                radius,
            });
        }
        let text_fg = if is_active {
            fg
        } else if hovered_tab || close_hovered {
            mix(bg, fg, 0.80)
        } else {
            inactive_fg
        };

        // The little terminal icon: a dark rounded plate with a bright `>_` prompt mark.
        let has_icon = tab.icon.w > 0.0;
        if has_icon {
            push_icon(&mut chrome, &tab.icon, s, renderer);
        }

        // Label, truncated to the room left of the close button.
        let has_close = tab.close.w > 0.0;
        let reserved = TAB_PAD_L * s
            + TAB_PAD_R * s
            + if has_icon {
                ICON_SIDE * s + ICON_GAP * s
            } else {
                0.0
            }
            + if has_close {
                TAB_TEXT_GAP * s + CLOSE_SIDE * s
            } else {
                0.0
            };
        let text_room = (tab.body.w - reserved).max(0.0);
        let text_x = tab.body.x
            + TAB_PAD_L * s
            + if has_icon {
                ICON_SIDE * s + ICON_GAP * s
            } else {
                0.0
            };
        chrome.texts.push(ChromeText {
            x: text_x,
            baseline_y: text_baseline,
            text: truncate_to_width(renderer, &tab.label, text_room),
            color: text_fg,
            bold: false,
            scale: 1.0,
            ui: true,
        });

        // Close button: shown only on the active or hovered tab so inactive tabs stay clean
        // (Windows Terminal reveals the `×` on hover). A hover square highlights it.
        if has_close && (is_active || hovered_tab || close_hovered) {
            if close_hovered {
                chrome.rects.push(ChromeRect {
                    x: tab.close.x,
                    y: tab.close.y,
                    w: tab.close.w,
                    h: tab.close.h,
                    color: tab_close_hover_bg,
                    radius: 5.0 * s,
                });
            }
            push_center_glyph(&mut chrome, renderer, &tab.close, '✕', glyph);
        }
    }

    // Thin vertical separators floating in the gaps between adjacent quiet tabs (dropped
    // next to the active or hovered tab, whose block already provides the boundary).
    for pair in layout.tabs.windows(2) {
        let involved = |i: usize| {
            i == active || hovered == Some(Hit::Tab(i)) || hovered == Some(Hit::Close(i))
        };
        if involved(pair[0].index) || involved(pair[1].index) {
            continue;
        }
        let gap_mid = (pair[0].body.x + pair[0].body.w + pair[1].body.x) * 0.5;
        let sep_h = (h * 0.38).round();
        chrome.rects.push(ChromeRect {
            x: gap_mid - 0.5 * s,
            y: (h - sep_h) * 0.5,
            w: s.max(1.0),
            h: sep_h,
            color: separator,
            radius: 0.0,
        });
    }

    // --- new-tab (+) button ---
    if hovered == Some(Hit::NewTab) {
        chrome.rects.push(ChromeRect {
            x: layout.plus.x,
            y: layout.plus.y,
            w: layout.plus.w,
            h: layout.plus.h,
            color: inactive_hover_bg,
            radius: 6.0 * s,
        });
    }
    push_center_glyph(&mut chrome, renderer, &layout.plus, '+', glyph);

    // --- window controls ---
    // Minimize.
    if hovered == Some(Hit::Minimize) {
        push_fill(&mut chrome, &layout.min, ctrl_hover_bg);
    }
    let (mcx, mcy) = layout.min.center();
    let line_w = 10.0 * s;
    let th = (s).round().max(1.0);
    chrome.rects.push(ChromeRect {
        x: mcx - line_w * 0.5,
        y: mcy - th * 0.5,
        w: line_w,
        h: th,
        color: glyph,
        radius: 0.0,
    });

    // Maximize / restore.
    if hovered == Some(Hit::Maximize) {
        push_fill(&mut chrome, &layout.max, ctrl_hover_bg);
    }
    let (xcx, xcy) = layout.max.center();
    let side = 9.0 * s;
    if maximized {
        // Restore: a front square with a second square peeking out top-right.
        let off = 3.0 * s;
        frame(
            &mut chrome.rects,
            xcx - side * 0.5,
            xcy - side * 0.5 + off,
            side,
            side,
            th,
            glyph,
        );
        // Back square: top and right edges only (the front square covers the rest).
        chrome.rects.push(ChromeRect {
            x: xcx - side * 0.5 + off,
            y: xcy - side * 0.5 - off,
            w: side,
            h: th,
            color: glyph,
            radius: 0.0,
        });
        chrome.rects.push(ChromeRect {
            x: xcx - side * 0.5 + off + side - th,
            y: xcy - side * 0.5 - off,
            w: th,
            h: side,
            color: glyph,
            radius: 0.0,
        });
    } else {
        frame(
            &mut chrome.rects,
            xcx - side * 0.5,
            xcy - side * 0.5,
            side,
            side,
            th,
            glyph,
        );
    }

    // Close.
    let close_hovered = hovered == Some(Hit::WindowClose);
    if close_hovered {
        push_fill(&mut chrome, &layout.close, close_hover_bg);
    }
    let close_glyph = if close_hovered {
        Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        }
    } else {
        glyph
    };
    push_center_glyph(&mut chrome, renderer, &layout.close, '✕', close_glyph);

    chrome
}

/// Push a full-height fill of `r` (button hover background).
fn push_fill(chrome: &mut Chrome, r: &Rect, color: Rgb) {
    chrome.rects.push(ChromeRect {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color,
        radius: 0.0,
    });
}

/// Paint a tab's terminal icon: a dark rounded plate with a bright `>_` prompt mark,
/// evoking the Windows Terminal / PowerShell app icons without needing image assets. The
/// mark stays in the *monospace* grid font — it depicts a prompt, not UI text.
fn push_icon(chrome: &mut Chrome, r: &Rect, s: f32, renderer: &Renderer) {
    // Plate: a fixed dark slate blue, readable on both the strip and a raised tab block.
    chrome.rects.push(ChromeRect {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: Rgb {
            r: 0x1c,
            g: 0x2a,
            b: 0x44,
        },
        radius: 3.5 * s,
    });
    // `>_` mark, centred on the plate.
    let metrics = renderer.cell_metrics();
    let baseline = renderer.text_baseline();
    let scale = 0.52;
    let text_w = 2.0 * metrics.width * scale;
    let (cx, cy) = r.center();
    chrome.texts.push(ChromeText {
        x: cx - text_w * 0.5,
        baseline_y: cy - metrics.height * scale * 0.5 + baseline * scale,
        text: ">_".to_owned(),
        color: Rgb {
            r: 0xdd,
            g: 0xe6,
            b: 0xf5,
        },
        bold: true,
        scale,
        ui: false,
    });
}

/// Center a single UI-font glyph horizontally and vertically inside `r`, using its real
/// advance and the UI line metrics.
fn push_center_glyph(chrome: &mut Chrome, renderer: &Renderer, r: &Rect, c: char, color: Rgb) {
    if r.w <= 0.0 {
        return;
    }
    let adv = renderer.ui_char_advance(c, 1.0);
    let (ui_h, ui_ascent) = renderer.ui_line(1.0);
    let (cx, cy) = r.center();
    chrome.texts.push(ChromeText {
        x: cx - adv * 0.5,
        baseline_y: cy - ui_h * 0.5 + ui_ascent,
        text: c.to_string(),
        color,
        bold: false,
        scale: 1.0,
        ui: true,
    });
}
