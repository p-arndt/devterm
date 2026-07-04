//! Pane pixel geometry, frame rendering, and pointer selection.
//!
//! Everything that maps the layout tree onto the window's pixels: deriving each pane's
//! grid, painting a frame, and translating pointer coordinates into a pane and cell.

use std::time::Instant;

use devterm_config::Config;
use devterm_core::{Gutter, PaneId, Rect, SplitDirection};
use devterm_pty::{PtyEvent, PtySize};
use devterm_render::PaneView;
use devterm_term::{CursorShape, SelectionMode, Snapshot};
use winit::event_loop::ActiveEventLoop;
use winit::window::CursorIcon;

use super::App;
use super::present::should_present;
use super::state::{AppState, GutterDrag};

/// Extra hit-test padding (physical px) around a divider so it is easy to grab.
const GUTTER_HIT_PAD: f32 = 3.0;

impl App {
    // --- sizing ---------------------------------------------------------------

    /// The window's full pixel rectangle (origin at top-left).
    fn window_rect(state: &AppState) -> Rect {
        let size = state.window.inner_size();
        Rect::new(
            0.0,
            0.0,
            size.width.max(1) as f32,
            size.height.max(1) as f32,
        )
    }

    /// Pixel rectangles of every pane laid out over the current window size.
    fn pixel_rects(state: &AppState) -> Vec<(PaneId, Rect)> {
        state.layout.compute(Self::window_rect(state))
    }

    /// Re-derive every pane's own cols/rows from its pixel rectangle and resize its model +
    /// child. Call after any layout change (resize, split, close, scale/font change).
    pub(super) fn resize_panes(state: &mut AppState) {
        for (id, area) in Self::pixel_rects(state) {
            if let Some(pane) = state.panes.get_mut(&id) {
                let (cols, rows) = state.renderer.grid_size_for(
                    area.w.round().max(1.0) as u32,
                    area.h.round().max(1.0) as u32,
                );
                pane.term.resize(cols, rows);
                let _ = pane.pty.resize(PtySize { cols, rows });
            }
        }
    }

    /// The focused pane's current row count (for page scrolling).
    pub(super) fn focused_rows(state: &AppState) -> u16 {
        let focused = state.layout.focused();
        Self::pixel_rects(state)
            .into_iter()
            .find(|(id, _)| *id == focused)
            .map(|(_, area)| {
                state
                    .renderer
                    .grid_size_for(
                        area.w.round().max(1.0) as u32,
                        area.h.round().max(1.0) as u32,
                    )
                    .1
            })
            .unwrap_or(1)
    }

    /// Resize the surface (physical px), then re-derive every pane's grid.
    pub(super) fn resize_surface(state: &mut AppState, width: u32, height: u32) {
        state.renderer.resize(width, height);
        Self::resize_panes(state);
    }

    // --- rendering ------------------------------------------------------------

    /// Drain child output into each model, flush emulator replies, reap exited children,
    /// then present one frame — but only if something actually changed.
    ///
    /// Damage tracking: clean panes are never re-snapshotted; each pane reuses its cached
    /// [`Snapshot`] and only dirty panes are re-snapshotted. If no pane is dirty and no
    /// non-terminal change is pending (`force_present`), the present is skipped entirely.
    /// The decision itself is the pure [`should_present`], which also folds in the byte-burst
    /// settle window and the DECSET-2026 synchronized-update skip.
    pub(super) fn redraw(state: &mut AppState, config: &Config, event_loop: &ActiveEventLoop) {
        // Pump each pane's PTY. Iterate over a snapshot of the ids so we can `get_mut`.
        let ids: Vec<PaneId> = state.panes.keys().copied().collect();
        let mut exited: Vec<PaneId> = Vec::new();
        for id in ids {
            let Some(pane) = state.panes.get_mut(&id) else {
                continue;
            };
            while let Ok(event) = pane.events.try_recv() {
                match event {
                    PtyEvent::Output(bytes) => pane.term.advance(&bytes),
                    PtyEvent::Exited(_code) => exited.push(id),
                }
            }
            let writes = pane.term.drain_pty_writes();
            if !writes.is_empty() {
                let _ = pane.pty.write(&writes);
            }
        }

        // Reap panes whose child exited: close them in the layout and drop the pane. If that
        // empties the window, quit.
        let had_exits = !exited.is_empty();
        for id in exited {
            if !state.panes.contains_key(&id) {
                continue;
            }
            match state.layout.close(id) {
                Ok(()) => {
                    state.panes.remove(&id);
                }
                Err(_) => {
                    // The last pane's child exited: nothing left to show.
                    event_loop.exit();
                    return;
                }
            }
        }
        if had_exits {
            Self::resize_panes(state);
            state.force_present = true;
        }

        let focused = state.layout.focused();
        // Only the focused pane's synchronized-update state gates tearing (matching the
        // frozen contract): the end sequence arrives as more output and wakes us again.
        let in_sync = state
            .panes
            .get(&focused)
            .is_some_and(|pane| pane.term.in_synchronized_update());
        let any_dirty = state.panes.values().any(|pane| pane.term.dirty());

        let now = Instant::now();
        let since_byte = now.saturating_duration_since(state.last_output);
        let since_present = now.saturating_duration_since(state.last_present);
        let present = should_present(
            any_dirty,
            state.force_present,
            in_sync,
            since_byte,
            since_present,
        );

        if !present {
            // Keep the pending flag set only while we are still coalescing a burst, so the
            // scheduler reschedules us; otherwise (sync-blocked or nothing to draw) drop it
            // and wait for the next wake to avoid busy-looping.
            let coalescing = !in_sync
                && (any_dirty || state.force_present)
                && since_byte < super::present::SETTLE_WINDOW
                && since_present < super::present::MAX_DEFER;
            if !coalescing {
                state.pending_output = false;
            }
            return;
        }

        // Re-snapshot only the panes that changed; reuse the cache for the rest.
        let areas = state.layout.compute(Rect::UNIT);
        for (id, _) in &areas {
            if let Some(pane) = state.panes.get_mut(id)
                && (pane.last_snapshot.is_none() || pane.term.dirty())
            {
                pane.last_snapshot = Some(pane.term.snapshot());
            }
        }

        // Cursor blink: when the focused cursor is in its hidden phase, present a copy of
        // its snapshot with the cursor suppressed (the cache keeps the real shape).
        let mut hidden_snapshot: Option<Snapshot> = None;
        if config.cursor.blink
            && !state.blink_visible
            && let Some(pane) = state.panes.get(&focused)
            && let Some(snap) = pane.last_snapshot.as_ref()
        {
            let mut copy = snap.clone();
            copy.cursor.shape = CursorShape::Hidden;
            hidden_snapshot = Some(copy);
        }

        let mut views: Vec<PaneView> = Vec::with_capacity(areas.len());
        for (id, area) in &areas {
            let is_focused = *id == focused;
            let snapshot: &Snapshot = match (is_focused, hidden_snapshot.as_ref()) {
                // Focused pane in the cursor's hidden blink phase: use the suppressed copy.
                (true, Some(hidden)) => hidden,
                _ => match state
                    .panes
                    .get(id)
                    .and_then(|pane| pane.last_snapshot.as_ref())
                {
                    Some(snap) => snap,
                    None => continue,
                },
            };
            views.push(PaneView {
                area: *area,
                snapshot,
                focused: is_focused,
            });
        }

        state.window.pre_present_notify();
        match state.renderer.render(&views) {
            Ok(()) => {}
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                // Reconfigure the surface at the current size and retry once.
                let size = state.window.inner_size();
                state.renderer.resize(size.width, size.height);
                let _ = state.renderer.render(&views);
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("wgpu surface out of memory; exiting");
                event_loop.exit();
                return;
            }
            Err(err) => {
                // `Timeout`, `Other`, and any future variants: skip this frame.
                log::warn!("wgpu surface error; skipping frame: {err:?}");
            }
        }

        state.last_present = now;
        state.pending_output = false;
        state.force_present = false;
    }

    /// Route a translated key sequence to the focused pane's child.
    pub(super) fn send_input(state: &AppState, bytes: &[u8]) {
        let focused = state.layout.focused();
        if let Some(pane) = state.panes.get(&focused)
            && let Err(err) = pane.pty.write(bytes)
        {
            log::warn!("failed to write to pty: {err}");
        }
    }

    // --- selection ------------------------------------------------------------

    /// The id of the pane whose pixel rectangle contains `pointer`.
    pub(super) fn pane_at(state: &AppState, (px, py): (f64, f64)) -> Option<PaneId> {
        let (px, py) = (px as f32, py as f32);
        Self::pixel_rects(state)
            .into_iter()
            .find(|(_, r)| r.contains(px, py))
            .map(|(id, _)| id)
    }

    /// Convert `pointer` to a `(col, line)` inside pane `id`, clamped to its grid.
    fn pane_local(state: &AppState, id: PaneId, (px, py): (f64, f64)) -> Option<(u16, u16)> {
        let (_, area) = Self::pixel_rects(state)
            .into_iter()
            .find(|(pid, _)| *pid == id)?;
        let metrics = state.renderer.cell_metrics();
        let (cols, rows) = state.renderer.grid_size_for(
            area.w.round().max(1.0) as u32,
            area.h.round().max(1.0) as u32,
        );
        let col = (((px as f32 - area.x) / metrics.width.max(1.0)).floor())
            .clamp(0.0, cols.saturating_sub(1) as f32) as u16;
        let line = (((py as f32 - area.y) / metrics.height.max(1.0)).floor())
            .clamp(0.0, rows.saturating_sub(1) as f32) as u16;
        Some((col, line))
    }

    /// Begin a selection at the pointer, focusing the pane it lands in.
    pub(super) fn begin_selection(state: &mut AppState) {
        let Some(id) = Self::pane_at(state, state.pointer) else {
            return;
        };
        state.layout.focus(id);
        if let Some((col, line)) = Self::pane_local(state, id, state.pointer)
            && let Some(pane) = state.panes.get_mut(&id)
        {
            pane.term.start_selection(col, line, SelectionMode::Simple);
        }
        state.force_present = true;
        state.window.request_redraw();
    }

    /// Extend the active selection in the focused pane to the pointer.
    pub(super) fn extend_selection(state: &mut AppState) {
        let focused = state.layout.focused();
        if let Some((col, line)) = Self::pane_local(state, focused, state.pointer)
            && let Some(pane) = state.panes.get_mut(&focused)
        {
            pane.term.update_selection(col, line);
            state.force_present = true;
            state.window.request_redraw();
        }
    }

    // --- divider (gutter) drag resize -----------------------------------------

    /// The split divider under `pointer`, if any, with a few px of grab padding.
    pub(super) fn gutter_at(state: &AppState, (px, py): (f64, f64)) -> Option<Gutter> {
        let (px, py) = (px as f32, py as f32);
        let rect = Self::window_rect(state);
        state.layout.gutters(rect).into_iter().find(|g| {
            let b = g.bounds;
            px >= b.x - GUTTER_HIT_PAD
                && px <= b.x + b.w + GUTTER_HIT_PAD
                && py >= b.y - GUTTER_HIT_PAD
                && py <= b.y + b.h + GUTTER_HIT_PAD
        })
    }

    /// Pixel length of the split that owns `g`, measured along its drag axis. Used to turn
    /// a pointer pixel delta into the fraction-of-split-length that `drag_gutter` expects.
    ///
    /// The gutter spans its split's full cross-axis extent, so the split's children are the
    /// panes whose centre falls inside that span; the split's along-axis length is the span
    /// of those panes' edges.
    fn gutter_split_length(state: &AppState, g: &Gutter) -> f32 {
        let rects = Self::pixel_rects(state);
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        match g.axis {
            // Vertical divider: length is the split's width; children share its y-span.
            SplitDirection::Horizontal => {
                let (y0, y1) = (g.bounds.y, g.bounds.y + g.bounds.h);
                for (_, r) in &rects {
                    let cy = r.y + r.h * 0.5;
                    if cy >= y0 && cy <= y1 {
                        lo = lo.min(r.x);
                        hi = hi.max(r.x + r.w);
                    }
                }
            }
            // Horizontal divider: length is the split's height; children share its x-span.
            SplitDirection::Vertical => {
                let (x0, x1) = (g.bounds.x, g.bounds.x + g.bounds.w);
                for (_, r) in &rects {
                    let cx = r.x + r.w * 0.5;
                    if cx >= x0 && cx <= x1 {
                        lo = lo.min(r.y);
                        hi = hi.max(r.y + r.h);
                    }
                }
            }
        }
        (hi - lo).max(1.0)
    }

    /// Begin dragging the divider under the pointer, if one is there. Returns `true` if a
    /// drag started (so the caller does not also begin a text selection).
    pub(super) fn begin_gutter_drag(state: &mut AppState) -> bool {
        let Some(g) = Self::gutter_at(state, state.pointer) else {
            return false;
        };
        state.drag = Some(GutterDrag {
            id: g.id,
            axis: g.axis,
            last: state.pointer,
        });
        true
    }

    /// Advance the active divider drag to the current pointer position.
    pub(super) fn update_gutter_drag(state: &mut AppState) {
        let Some(drag) = state.drag else {
            return;
        };
        // Re-fetch the boundary so its (moved) bounds give the current split length.
        let rect = Self::window_rect(state);
        let Some(g) = state
            .layout
            .gutters(rect)
            .into_iter()
            .find(|g| g.id == drag.id)
        else {
            return;
        };
        let length = Self::gutter_split_length(state, &g);
        let cur = state.pointer;
        let delta_px = match drag.axis {
            SplitDirection::Horizontal => (cur.0 - drag.last.0) as f32,
            SplitDirection::Vertical => (cur.1 - drag.last.1) as f32,
        };
        if let Some(d) = state.drag.as_mut() {
            d.last = cur;
        }
        let frac = delta_px / length;
        if frac != 0.0 && state.layout.drag_gutter(drag.id, frac) {
            Self::resize_panes(state);
            state.force_present = true;
            state.window.request_redraw();
        }
    }

    /// Set the window cursor to a resize icon while hovering a divider, else the default.
    pub(super) fn update_hover_cursor(state: &mut AppState) {
        let icon = match Self::gutter_at(state, state.pointer) {
            // Horizontal split => vertical divider => left/right resize.
            Some(g) if g.axis == SplitDirection::Horizontal => CursorIcon::EwResize,
            // Vertical split => horizontal divider => up/down resize.
            Some(_) => CursorIcon::NsResize,
            None => CursorIcon::Default,
        };
        if icon != state.cursor_icon {
            state.cursor_icon = icon;
            state.window.set_cursor(icon);
        }
    }
}
