//! Pane pixel geometry, frame rendering, and pointer selection.
//!
//! Everything that maps the layout tree onto the window's pixels: deriving each pane's
//! grid, painting a frame, and translating pointer coordinates into a pane and cell.

use devterm_core::{PaneId, Rect};
use devterm_pty::{PtyEvent, PtySize};
use devterm_render::PaneView;
use devterm_term::SelectionMode;
use winit::event_loop::ActiveEventLoop;

use super::App;
use super::state::AppState;

impl App {
    // --- sizing ---------------------------------------------------------------

    /// Pixel rectangles of every pane laid out over the current window size.
    fn pixel_rects(state: &AppState) -> Vec<(PaneId, Rect)> {
        let size = state.window.inner_size();
        let rect = Rect::new(
            0.0,
            0.0,
            size.width.max(1) as f32,
            size.height.max(1) as f32,
        );
        state.layout.compute(rect)
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
    /// then draw one frame of every laid-out pane.
    pub(super) fn redraw(state: &mut AppState, event_loop: &ActiveEventLoop) {
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
        }

        let focused = state.layout.focused();

        // Anti-flicker: skip presenting mid DECSET-2026 synchronized update to avoid tearing.
        // The child's end-of-update sequence arrives as more output and wakes us to repaint.
        if let Some(pane) = state.panes.get(&focused)
            && pane.term.in_synchronized_update()
        {
            return;
        }

        // Lay out the panes and snapshot each one for the renderer.
        let areas = state.layout.compute(Rect::UNIT);
        let mut snaps = Vec::with_capacity(areas.len());
        for (id, area) in &areas {
            if let Some(pane) = state.panes.get(id) {
                snaps.push((*area, pane.term.snapshot(), *id == focused));
            }
        }
        let views: Vec<PaneView> = snaps
            .iter()
            .map(|(area, snapshot, focused)| PaneView {
                area: *area,
                snapshot,
                focused: *focused,
            })
            .collect();

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
            }
            Err(err) => {
                // `Timeout`, `Other`, and any future variants: skip this frame.
                log::warn!("wgpu surface error; skipping frame: {err:?}");
            }
        }
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
        state.window.request_redraw();
    }

    /// Extend the active selection in the focused pane to the pointer.
    pub(super) fn extend_selection(state: &mut AppState) {
        let focused = state.layout.focused();
        if let Some((col, line)) = Self::pane_local(state, focused, state.pointer)
            && let Some(pane) = state.panes.get_mut(&focused)
        {
            pane.term.update_selection(col, line);
            state.window.request_redraw();
        }
    }
}
