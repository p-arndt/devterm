//! Pane pixel geometry, frame rendering, and pointer selection.
//!
//! Everything that maps the layout tree onto the window's pixels: deriving each pane's
//! grid, painting a frame, and translating pointer coordinates into a pane and cell.

use std::time::{Duration, Instant};

use devterm_config::Config;
use devterm_core::{Gutter, PaneId, Rect, SplitDirection};
use devterm_pty::{PtyEvent, PtySize};
use devterm_render::PaneView;
use devterm_term::{CursorShape, SelectionMode, Snapshot};
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, ResizeDirection};

use super::App;
use super::present::should_present;
use super::state::{AppState, GutterDrag};
use super::tabbar::{self, Hit};

/// Extra hit-test padding (physical px) around a divider so it is easy to grab.
const GUTTER_HIT_PAD: f32 = 3.0;

/// The floating terminal's size as a fraction of the window (centered on both axes).
const OVERLAY_W_FRAC: f32 = 0.6;
const OVERLAY_H_FRAC: f32 = 0.5;

/// The settings overlay's size as a fraction of the window. Taller than the floating
/// terminal so the keybindings list has room to breathe.
const SETTINGS_W_FRAC: f32 = 0.7;
const SETTINGS_H_FRAC: f32 = 0.78;

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

    /// Height of the always-visible titlebar strip in physical pixels. Sized like Windows
    /// Terminal's tab row (~40 logical px), but never shorter than a tab label needs.
    pub(super) fn tab_bar_px(state: &AppState) -> f32 {
        let scale = state.window.scale_factor() as f32;
        let cell_h = state.renderer.cell_metrics().height;
        (40.0 * scale).max(cell_h + 14.0 * scale).round()
    }

    /// The pixel rectangle the layout tree tiles: the window minus the tab bar strip.
    fn layout_rect(state: &AppState) -> Rect {
        let win = Self::window_rect(state);
        let bar = Self::tab_bar_px(state);
        Rect::new(0.0, bar, win.w, (win.h - bar).max(1.0))
    }

    /// Pixel rectangles of every pane of the active tab laid out below the tab bar.
    fn pixel_rects(state: &AppState) -> Vec<(PaneId, Rect)> {
        state.tab().layout.compute(Self::layout_rect(state))
    }

    /// Re-derive every active-tab pane's own cols/rows from its pixel rectangle and resize
    /// its model + child. Call after any layout change (resize, split, close, scale/font
    /// change, tab switch). Background tabs are caught up when they become active.
    pub(super) fn resize_panes(state: &mut AppState) {
        let active = state.active_tab;
        for (id, area) in Self::pixel_rects(state) {
            if let Some(pane) = state.tabs[active].panes.get_mut(&id) {
                let (cols, rows) = state.renderer.grid_size_for(
                    area.w.round().max(1.0) as u32,
                    area.h.round().max(1.0) as u32,
                );
                pane.term.resize(cols, rows);
                let _ = pane.pty.resize(PtySize { cols, rows });
            }
        }
        // Keep the floating terminal's grid in step with the window on every layout change.
        Self::resize_overlay(state);
    }

    // --- floating terminal geometry -------------------------------------------

    /// The floating terminal's rectangle in unit (0..1) coordinates, centered on the window.
    /// Shares the same coordinate space as [`LayoutTree::compute`] so it renders identically.
    fn overlay_unit_rect() -> Rect {
        Rect::new(
            (1.0 - OVERLAY_W_FRAC) * 0.5,
            (1.0 - OVERLAY_H_FRAC) * 0.5,
            OVERLAY_W_FRAC,
            OVERLAY_H_FRAC,
        )
    }

    /// The floating terminal's rectangle in physical pixels.
    fn overlay_pixel_rect(state: &AppState) -> Rect {
        let win = Self::window_rect(state);
        let u = Self::overlay_unit_rect();
        Rect::new(u.x * win.w, u.y * win.h, u.w * win.w, u.h * win.h)
    }

    /// The floating terminal's cols/rows for the current window size.
    pub(super) fn overlay_grid(state: &AppState) -> (u16, u16) {
        let r = Self::overlay_pixel_rect(state);
        state
            .renderer
            .grid_size_for(r.w.round().max(1.0) as u32, r.h.round().max(1.0) as u32)
    }

    // --- settings overlay geometry --------------------------------------------

    /// The settings overlay's rectangle in unit (0..1) coordinates, centered on the window.
    fn settings_unit_rect() -> Rect {
        Rect::new(
            (1.0 - SETTINGS_W_FRAC) * 0.5,
            (1.0 - SETTINGS_H_FRAC) * 0.5,
            SETTINGS_W_FRAC,
            SETTINGS_H_FRAC,
        )
    }

    /// The settings overlay's cols/rows for the current window size.
    fn settings_grid(state: &AppState) -> (u16, u16) {
        let win = Self::window_rect(state);
        let u = Self::settings_unit_rect();
        state.renderer.grid_size_for(
            (u.w * win.w).round().max(1.0) as u32,
            (u.h * win.h).round().max(1.0) as u32,
        )
    }

    /// Resize the floating terminal's model + child to match its current pixel rectangle.
    pub(super) fn resize_overlay(state: &mut AppState) {
        let (cols, rows) = Self::overlay_grid(state);
        if let Some(overlay) = state.overlay.as_mut() {
            overlay.term.resize(cols, rows);
            let _ = overlay.pty.resize(PtySize { cols, rows });
        }
    }

    /// The pane that currently receives keyboard input, copy/paste and scrolling: the
    /// floating terminal while it is shown, otherwise the focused layout pane.
    pub(super) fn active_pane(state: &AppState) -> Option<&super::pane::Pane> {
        if state.overlay_visible {
            state.overlay.as_ref()
        } else {
            let tab = state.tab();
            tab.panes.get(&tab.layout.focused())
        }
    }

    /// Mutable counterpart to [`active_pane`](Self::active_pane).
    pub(super) fn active_pane_mut(state: &mut AppState) -> Option<&mut super::pane::Pane> {
        if state.overlay_visible {
            state.overlay.as_mut()
        } else {
            let tab = state.tab_mut();
            let focused = tab.layout.focused();
            tab.panes.get_mut(&focused)
        }
    }

    /// The active pane's current row count (for page scrolling).
    pub(super) fn focused_rows(state: &AppState) -> u16 {
        if state.overlay_visible {
            return Self::overlay_grid(state).1;
        }
        let focused = state.tab().layout.focused();
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
        // Pump every pane's PTY in every tab — background tabs keep their shells flowing
        // (and their event channels drained) even though only the active tab is drawn.
        let mut exited: Vec<(usize, PaneId)> = Vec::new();
        for (tab_index, tab) in state.tabs.iter_mut().enumerate() {
            for (&id, pane) in tab.panes.iter_mut() {
                while let Ok(event) = pane.events.try_recv() {
                    match event {
                        PtyEvent::Output(bytes) => pane.term.advance(&bytes),
                        PtyEvent::Exited(_code) => exited.push((tab_index, id)),
                    }
                }
                let writes = pane.term.drain_pty_writes();
                if !writes.is_empty() {
                    let _ = pane.pty.write(&writes);
                }
            }
        }

        // Reap panes whose child exited: close them in their tab's layout and drop the
        // pane. A tab whose last pane exited is dropped whole; when no tab remains, quit.
        let had_exits = !exited.is_empty();
        let mut dead_tabs: Vec<usize> = Vec::new();
        for (tab_index, id) in exited {
            let tab = &mut state.tabs[tab_index];
            if !tab.panes.contains_key(&id) {
                continue;
            }
            match tab.layout.close(id) {
                Ok(()) => {
                    tab.panes.remove(&id);
                }
                Err(_) => {
                    if !dead_tabs.contains(&tab_index) {
                        dead_tabs.push(tab_index);
                    }
                }
            }
        }
        dead_tabs.sort_unstable();
        for tab_index in dead_tabs.into_iter().rev() {
            state.tabs.remove(tab_index);
            if state.active_tab > tab_index {
                state.active_tab -= 1;
            }
        }
        if state.tabs.is_empty() {
            // The last tab's last child exited: nothing left to show.
            event_loop.exit();
            return;
        }
        state.active_tab = state.active_tab.min(state.tabs.len() - 1);
        if had_exits {
            Self::resize_panes(state);
            state.force_present = true;
        }

        // Pump the floating terminal's PTY (drained regardless of visibility so a hidden
        // scratch shell keeps running). If its child exits, drop and hide it.
        if let Some(overlay) = state.overlay.as_mut() {
            let mut overlay_exited = false;
            while let Ok(event) = overlay.events.try_recv() {
                match event {
                    PtyEvent::Output(bytes) => overlay.term.advance(&bytes),
                    PtyEvent::Exited(_code) => overlay_exited = true,
                }
            }
            let writes = overlay.term.drain_pty_writes();
            if !writes.is_empty() {
                let _ = overlay.pty.write(&writes);
            }
            if overlay_exited {
                state.overlay = None;
                state.overlay_visible = false;
                state.force_present = true;
            }
        }

        let focused = state.tab().layout.focused();
        // Only the active pane's synchronized-update state gates tearing (matching the
        // frozen contract): the end sequence arrives as more output and wakes us again. The
        // active pane is the floating terminal while it is shown, else the focused layout pane.
        let in_sync =
            Self::active_pane(state).is_some_and(|pane| pane.term.in_synchronized_update());
        // A hidden overlay is not drawn, so its dirtiness must not trigger a present.
        let overlay_dirty = state.overlay_visible
            && state
                .overlay
                .as_ref()
                .is_some_and(|overlay| overlay.term.dirty());
        // Background tabs are not drawn, so only the active tab's panes gate a present.
        let any_dirty = overlay_dirty || state.tab().panes.values().any(|pane| pane.term.dirty());

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

        // The active tab's panes tile the unit square minus the tab bar strip; this is the
        // unit-space twin of `layout_rect` (same fractions, so pixels line up exactly).
        let win = Self::window_rect(state);
        let bar_frac = Self::tab_bar_px(state) / win.h;
        let areas = state
            .tab()
            .layout
            .compute(Rect::new(0.0, bar_frac, 1.0, 1.0 - bar_frac));

        // Re-snapshot only the panes that changed; reuse the cache for the rest.
        let active = state.active_tab;
        for (id, _) in &areas {
            if let Some(pane) = state.tabs[active].panes.get_mut(id)
                && (pane.last_snapshot.is_none() || pane.term.dirty())
            {
                pane.last_snapshot = Some(pane.term.snapshot());
            }
        }
        if state.overlay_visible
            && let Some(overlay) = state.overlay.as_mut()
            && (overlay.last_snapshot.is_none() || overlay.term.dirty())
        {
            overlay.last_snapshot = Some(overlay.term.snapshot());
        }

        // While the floating terminal is shown it holds focus: the layout panes render
        // unfocused (dim borders, hollow cursor) and the overlay carries the accent + live
        // cursor. Otherwise the layout's focused pane is highlighted as usual.
        let base_focused = if state.overlay_visible || state.settings.is_some() {
            None
        } else {
            Some(focused)
        };

        // Cursor blink: when the active cursor is in its hidden phase, present a copy of its
        // snapshot with the cursor suppressed (the cache keeps the real shape). Only the
        // focused/active terminal blinks — the layout base while the overlay is up does not.
        let blink_off = config.cursor.blink && !state.blink_visible;
        let make_hidden = |snap: &Snapshot| {
            let mut copy = snap.clone();
            copy.cursor.shape = CursorShape::Hidden;
            copy
        };
        let base_hidden: Option<Snapshot> = base_focused
            .filter(|_| blink_off)
            .and_then(|id| state.tab().panes.get(&id))
            .and_then(|pane| pane.last_snapshot.as_ref())
            .map(&make_hidden);
        let overlay_hidden: Option<Snapshot> = state
            .overlay
            .as_ref()
            .filter(|_| state.overlay_visible && blink_off)
            .and_then(|overlay| overlay.last_snapshot.as_ref())
            .map(&make_hidden);

        // The titlebar chrome, synthesized fresh each present (cheap: a handful of rects +
        // glyphs). The layout is the shared source of truth for the paint and the hit-test.
        let scale = state.window.scale_factor() as f32;
        let bar_layout = super::tabbar::layout(
            &state.tabs,
            win.w,
            Self::tab_bar_px(state),
            &state.renderer,
            scale,
        );
        let chrome = super::tabbar::build_chrome(
            &bar_layout,
            state.active_tab,
            state.hovered,
            state.window.is_maximized(),
            &state.palette,
            &state.renderer,
        );

        let mut views: Vec<PaneView> = Vec::with_capacity(areas.len() + 1);
        for (id, area) in &areas {
            let is_focused = base_focused == Some(*id);
            let snapshot: &Snapshot = match (is_focused, base_hidden.as_ref()) {
                // Focused pane in the cursor's hidden blink phase: use the suppressed copy.
                (true, Some(hidden)) => hidden,
                // Index the tab directly so the snapshot borrow pins only `state.tabs`,
                // leaving `state.renderer` free for the render call below.
                _ => match state.tabs[active]
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

        // The inline settings overlay, when open, is synthesized into a snapshot and drawn
        // as the top overlay layer (taking precedence over the floating terminal). It is
        // owned here so the `PaneView` below can borrow it for the render call.
        let settings_snapshot: Option<Snapshot> = if state.settings.is_some() {
            let (cols, rows) = Self::settings_grid(state);
            state
                .settings
                .as_ref()
                .map(|menu| menu.snapshot(&state.palette, cols, rows))
        } else {
            None
        };

        // The floating terminal is drawn as a separate top layer by the renderer so its
        // opaque background occludes the layout text beneath it. The settings overlay, when
        // open, replaces it as that top layer.
        let overlay_view: Option<PaneView> = if let Some(snapshot) = settings_snapshot.as_ref() {
            Some(PaneView {
                area: Self::settings_unit_rect(),
                snapshot,
                focused: true,
            })
        } else if state.overlay_visible {
            state.overlay.as_ref().and_then(|overlay| {
                let snapshot = match (overlay_hidden.as_ref(), overlay.last_snapshot.as_ref()) {
                    (Some(hidden), _) => Some(hidden),
                    (None, snap) => snap,
                };
                snapshot.map(|snapshot| PaneView {
                    area: Self::overlay_unit_rect(),
                    snapshot,
                    focused: true,
                })
            })
        } else {
            None
        };

        state.window.pre_present_notify();
        match state
            .renderer
            .render(&views, overlay_view.as_ref(), Some(&chrome))
        {
            Ok(()) => {}
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                // Reconfigure the surface at the current size and retry once.
                let size = state.window.inner_size();
                state.renderer.resize(size.width, size.height);
                let _ = state
                    .renderer
                    .render(&views, overlay_view.as_ref(), Some(&chrome));
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

    /// Route a translated key sequence to the active pane's child (the floating terminal
    /// while it is shown, otherwise the focused layout pane).
    pub(super) fn send_input(state: &AppState, bytes: &[u8]) {
        if let Some(pane) = Self::active_pane(state)
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
        // The cell grid sits inset by the pane's content padding.
        let pad = state.renderer.content_pad();
        let col = (((px as f32 - area.x - pad) / metrics.width.max(1.0)).floor())
            .clamp(0.0, cols.saturating_sub(1) as f32) as u16;
        let line = (((py as f32 - area.y - pad) / metrics.height.max(1.0)).floor())
            .clamp(0.0, rows.saturating_sub(1) as f32) as u16;
        Some((col, line))
    }

    /// The current titlebar geometry (shared source of truth for paint + hit-test).
    fn titlebar_layout(state: &AppState) -> tabbar::Layout {
        let win = Self::window_rect(state);
        let scale = state.window.scale_factor() as f32;
        tabbar::layout(
            &state.tabs,
            win.w,
            Self::tab_bar_px(state),
            &state.renderer,
            scale,
        )
    }

    /// What the pointer is over on the titlebar, or `None` when it is below the strip. A
    /// `Some` consumes the press, so the caller does not also start a selection/gutter drag.
    pub(super) fn titlebar_hit(state: &AppState) -> Option<Hit> {
        if state.pointer.1 >= Self::tab_bar_px(state) as f64 {
            return None;
        }
        let layout = Self::titlebar_layout(state);
        Some(tabbar::hit(
            &layout,
            state.pointer.0 as f32,
            state.pointer.1 as f32,
        ))
    }

    /// Which window edge/corner the pointer is within grabbing distance of (for a manual
    /// resize on the borderless window), or `None`. Never resizes a maximized window.
    pub(super) fn resize_dir(state: &AppState) -> Option<ResizeDirection> {
        if state.window.is_maximized() {
            return None;
        }
        let win = Self::window_rect(state);
        let m = 6.0 * state.window.scale_factor() as f32;
        let (px, py) = (state.pointer.0 as f32, state.pointer.1 as f32);
        let left = px <= m;
        let right = px >= win.w - m;
        let top = py <= m;
        let bottom = py >= win.h - m;
        Some(match (top, bottom, left, right) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (true, _, _, true) => ResizeDirection::NorthEast,
            (_, true, true, _) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::North,
            (_, true, ..) => ResizeDirection::South,
            (_, _, true, _) => ResizeDirection::West,
            (_, _, _, true) => ResizeDirection::East,
            _ => return None,
        })
    }

    /// Set `state.hovered` for the titlebar element under the pointer, requesting a repaint
    /// when it changes (so hover highlights track the pointer). Clears it below the strip.
    pub(super) fn update_titlebar_hover(state: &mut AppState) {
        let hovered = if state.pointer.1 < Self::tab_bar_px(state) as f64 {
            Some(Self::titlebar_hit(state).unwrap_or(Hit::Drag))
        } else {
            None
        };
        if hovered != state.hovered {
            state.hovered = hovered;
            state.force_present = true;
            state.window.request_redraw();
        }
    }

    /// Handle a left-press on the empty caption area: a quick second press in the same spot
    /// toggles maximize (like a titlebar double-click); otherwise arm a pending window drag
    /// that begins once the pointer moves ([`maybe_begin_window_drag`]).
    pub(super) fn on_caption_press(state: &mut AppState) {
        let now = Instant::now();
        let double = state.last_caption_click.is_some_and(|(t, (x, y))| {
            now.duration_since(t) < Duration::from_millis(400)
                && (state.pointer.0 - x).abs() + (state.pointer.1 - y).abs() < 6.0
        });
        if double {
            let max = state.window.is_maximized();
            state.window.set_maximized(!max);
            state.last_caption_click = None;
            state.titlebar_press = None;
        } else {
            state.last_caption_click = Some((now, state.pointer));
            state.titlebar_press = Some(state.pointer);
        }
    }

    /// Once a caption press has moved beyond a small threshold, hand off to the OS window
    /// drag loop (aero-snap aware). Consumes the pending press.
    pub(super) fn maybe_begin_window_drag(state: &mut AppState) {
        let Some((sx, sy)) = state.titlebar_press else {
            return;
        };
        let moved = (state.pointer.0 - sx).abs() + (state.pointer.1 - sy).abs();
        if moved > 4.0 {
            state.titlebar_press = None;
            let _ = state.window.drag_window();
        }
    }

    /// Begin a selection at the pointer, focusing the pane it lands in.
    pub(super) fn begin_selection(state: &mut AppState) {
        let Some(id) = Self::pane_at(state, state.pointer) else {
            return;
        };
        state.tab_mut().layout.focus(id);
        if let Some((col, line)) = Self::pane_local(state, id, state.pointer)
            && let Some(pane) = state.tab_mut().panes.get_mut(&id)
        {
            pane.term.start_selection(col, line, SelectionMode::Simple);
        }
        state.force_present = true;
        state.window.request_redraw();
    }

    /// Extend the active selection in the focused pane to the pointer.
    pub(super) fn extend_selection(state: &mut AppState) {
        let focused = state.tab().layout.focused();
        if let Some((col, line)) = Self::pane_local(state, focused, state.pointer)
            && let Some(pane) = state.tab_mut().panes.get_mut(&focused)
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
        let rect = Self::layout_rect(state);
        state.tab().layout.gutters(rect).into_iter().find(|g| {
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
        let rect = Self::layout_rect(state);
        let Some(g) = state
            .tab()
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
        if frac != 0.0 && state.tab_mut().layout.drag_gutter(drag.id, frac) {
            Self::resize_panes(state);
            state.force_present = true;
            state.window.request_redraw();
        }
    }

    /// Update the window cursor for what the pointer is over: a window-edge resize arrow
    /// takes precedence, then the plain arrow anywhere on the titlebar, then a split-divider
    /// resize arrow, else the default.
    pub(super) fn update_hover_cursor(state: &mut AppState) {
        let icon = if let Some(dir) = Self::resize_dir(state) {
            CursorIcon::from(dir)
        } else if state.pointer.1 < Self::tab_bar_px(state) as f64 {
            CursorIcon::Default
        } else {
            match Self::gutter_at(state, state.pointer) {
                // Horizontal split => vertical divider => left/right resize.
                Some(g) if g.axis == SplitDirection::Horizontal => CursorIcon::EwResize,
                // Vertical split => horizontal divider => up/down resize.
                Some(_) => CursorIcon::NsResize,
                None => CursorIcon::Default,
            }
        };
        if icon != state.cursor_icon {
            state.cursor_icon = icon;
            state.window.set_cursor(icon);
        }
    }
}
