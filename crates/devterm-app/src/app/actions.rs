//! Bound-action dispatch, the individual action handlers, and config hot-reload.

use std::time::Instant;

use devterm_config::{Action, Config};
use devterm_core::{Direction, LayoutError, SplitDirection};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};

use super::App;
use super::input::{build_keymap, palette_from_theme, term_cursor_shape};
use super::pane::build_pane;
use super::state::{AppState, UserEvent};

impl App {
    // --- actions --------------------------------------------------------------

    /// Dispatch a bound [`Action`]. Takes the pieces it needs as arguments so it can be
    /// called while `state` is borrowed out of `self`.
    pub(super) fn dispatch(
        state: &mut AppState,
        config: &Config,
        proxy: &EventLoopProxy<UserEvent>,
        action: Action,
        event_loop: &ActiveEventLoop,
    ) {
        match action {
            Action::SplitHorizontal => {
                Self::split_pane(state, config, proxy, SplitDirection::Horizontal)
            }
            Action::SplitVertical => {
                Self::split_pane(state, config, proxy, SplitDirection::Vertical)
            }
            Action::ClosePane => Self::close_focused(state, event_loop),
            Action::FocusLeft => Self::focus(state, Direction::Left),
            Action::FocusRight => Self::focus(state, Direction::Right),
            Action::FocusUp => Self::focus(state, Direction::Up),
            Action::FocusDown => Self::focus(state, Direction::Down),
            Action::ResizeLeft => Self::resize_focused(state, Direction::Left),
            Action::ResizeRight => Self::resize_focused(state, Direction::Right),
            Action::ResizeUp => Self::resize_focused(state, Direction::Up),
            Action::ResizeDown => Self::resize_focused(state, Direction::Down),
            Action::Copy => Self::copy_selection(state),
            Action::Paste => Self::paste_clipboard(state),
            Action::ScrollLineUp => Self::scroll(state, 1),
            Action::ScrollLineDown => Self::scroll(state, -1),
            Action::ScrollPageUp => Self::scroll_page(state, 1),
            Action::ScrollPageDown => Self::scroll_page(state, -1),
            Action::Quit => event_loop.exit(),
        }
    }

    /// Split the focused pane, spawn a fresh pane into the new leaf, and re-size everything.
    fn split_pane(
        state: &mut AppState,
        config: &Config,
        proxy: &EventLoopProxy<UserEvent>,
        direction: SplitDirection,
    ) {
        let id = state.ids.next_pane();
        // A provisional grid; `resize_panes` immediately fixes it to the leaf's real area.
        let size = state.window.inner_size();
        let (cols, rows) = state.renderer.grid_size_for(size.width, size.height);
        let pane = match build_pane(config, proxy, state.palette, cols, rows) {
            Ok(pane) => pane,
            Err(err) => {
                log::error!("failed to spawn pane: {err}");
                return;
            }
        };
        state.layout.split(direction, id);
        state.panes.insert(id, pane);
        Self::resize_panes(state);
        state.force_present = true;
        state.window.request_redraw();
    }

    /// Close the focused pane, dropping it (its `Pty` `Drop` kills the child).
    fn close_focused(state: &mut AppState, event_loop: &ActiveEventLoop) {
        let focused = state.layout.focused();
        match state.layout.close(focused) {
            Ok(()) => {
                state.panes.remove(&focused);
                Self::resize_panes(state);
                state.force_present = true;
                state.window.request_redraw();
            }
            // Closing the last pane quits DevTerm (parity with closing the window).
            Err(LayoutError::CannotCloseLastPane) => event_loop.exit(),
            Err(err) => log::warn!("close pane failed: {err}"),
        }
    }

    fn focus(state: &mut AppState, dir: Direction) {
        if state.layout.move_focus(dir) {
            // Focus changes no terminal, so force a present to move the highlight.
            state.force_present = true;
            state.window.request_redraw();
        }
    }

    fn resize_focused(state: &mut AppState, dir: Direction) {
        // The pressed arrow moves the focused pane's border in that direction: grow toward a
        // neighbor, shrink toward the window edge. So on the right pane of a split, Left grows
        // it leftward and Right shrinks it — the border follows the key. ~10% per press.
        const STEP: f32 = 1.1;
        state.layout.resize_directional(dir, STEP);
        Self::resize_panes(state);
        state.force_present = true;
        state.window.request_redraw();
    }

    fn scroll(state: &mut AppState, lines: i32) {
        let focused = state.layout.focused();
        if let Some(pane) = state.panes.get_mut(&focused) {
            pane.term.scroll_display(lines);
            state.force_present = true;
            state.window.request_redraw();
        }
    }

    fn scroll_page(state: &mut AppState, sign: i32) {
        let rows = Self::focused_rows(state).max(2) as i32;
        Self::scroll(state, sign * (rows - 1));
    }

    /// Copy the focused pane's selection to the system clipboard.
    fn copy_selection(state: &mut AppState) {
        let focused = state.layout.focused();
        let Some(text) = state
            .panes
            .get(&focused)
            .and_then(|pane| pane.term.selected_text())
        else {
            return;
        };
        if let Some(clipboard) = Self::clipboard(state)
            && let Err(err) = clipboard.set_text(text)
        {
            log::warn!("failed to set clipboard: {err}");
        }
    }

    /// Paste the system clipboard into the focused pane, honouring bracketed paste.
    fn paste_clipboard(state: &mut AppState) {
        let text = match Self::clipboard(state) {
            Some(clipboard) => match clipboard.get_text() {
                Ok(text) => text,
                Err(err) => {
                    log::warn!("failed to read clipboard: {err}");
                    return;
                }
            },
            None => return,
        };
        let focused = state.layout.focused();
        let Some(pane) = state.panes.get(&focused) else {
            return;
        };
        let bytes = if pane.term.bracketed_paste() {
            // Wrap in CSI 200~ / CSI 201~ so the child treats it as pasted, not typed.
            let mut wrapped = Vec::with_capacity(text.len() + 12);
            wrapped.extend_from_slice(b"\x1b[200~");
            wrapped.extend_from_slice(text.as_bytes());
            wrapped.extend_from_slice(b"\x1b[201~");
            wrapped
        } else {
            text.into_bytes()
        };
        if let Err(err) = pane.pty.write(&bytes) {
            log::warn!("failed to paste into pty: {err}");
        }
    }

    /// Lazily construct the system clipboard, caching it. `None` if unavailable.
    fn clipboard(state: &mut AppState) -> Option<&mut arboard::Clipboard> {
        if state.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(clipboard) => state.clipboard = Some(clipboard),
                Err(err) => {
                    log::warn!("clipboard unavailable: {err}");
                    return None;
                }
            }
        }
        state.clipboard.as_mut()
    }

    // --- config hot-reload ----------------------------------------------------

    /// Reload `config.toml` from disk and re-apply everything hot-swappable: keymap, resolved
    /// theme palette, cursor shape, and (when changed) font family, font size and line height
    /// — re-deriving every pane's grid when the cell metrics move. Shell changes take effect
    /// on the next spawned pane.
    pub(super) fn reload_config(&mut self) {
        let path = Config::default_path();
        let config = match Config::load(&path) {
            Ok(config) => config,
            Err(err) => {
                log::warn!("config reload failed: {err}");
                return;
            }
        };
        log::info!("reloaded config from {}", path.display());

        // Resolve the effective theme (named base + inline overlay) so `theme_name` applies.
        let palette = palette_from_theme(&config.resolve_theme());
        let cursor_shape = term_cursor_shape(config.cursor.shape);
        // Detect metric-affecting changes against the previous config before swapping.
        let font_size_changed = (config.font_size - self.config.font_size).abs() > f32::EPSILON;
        let font_family_changed = config.font_family != self.config.font_family;
        let line_height_changed =
            (config.line_height - self.config.line_height).abs() > f32::EPSILON;
        let font_size = config.font_size;
        let font_family = if config.font_family.is_empty() {
            None
        } else {
            Some(config.font_family.clone())
        };
        let line_height = config.line_height;
        self.config = config;

        if let Some(state) = self.state.as_mut() {
            state.keymap = build_keymap(&self.config);
            state.palette = palette;
            for pane in state.panes.values_mut() {
                pane.term.set_palette(palette);
                pane.term.set_default_cursor_shape(cursor_shape);
            }

            // Font family / size / line height all move the cell metrics; apply each that
            // changed, then re-derive every pane's cols/rows once.
            let mut metrics_changed = false;
            if font_family_changed {
                state.renderer.set_font_family(font_family);
                metrics_changed = true;
            }
            if font_size_changed {
                state.renderer.set_font_size(font_size);
                metrics_changed = true;
            }
            if line_height_changed {
                state.renderer.set_line_height(line_height);
                metrics_changed = true;
            }
            if metrics_changed {
                Self::resize_panes(state);
            }

            // Reset the blink phase so a toggled `cursor.blink` takes effect cleanly.
            state.blink_visible = true;
            state.last_blink_toggle = Instant::now();
            state.force_present = true;
            state.window.request_redraw();
        }
    }
}
