//! Bound-action dispatch, the individual action handlers, and config hot-reload.

use std::collections::HashMap;
use std::time::Instant;

use devterm_config::{Action, Config};
use devterm_core::{Direction, LayoutError, LayoutTree, SplitDirection};
use devterm_pty::PtyCommandSpec;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};

use super::App;
use super::input::{build_keymap, palette_from_theme, term_cursor_shape};
use super::pane::{build_pane, build_pane_with_spec};
use super::settings::{SettingsMenu, SettingsResponse};
use super::state::{AppState, Tab, UserEvent};

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
        // While the floating terminal captures interaction, layout-structure actions
        // (split/focus/resize/tabs) would silently mutate the hidden layout underneath it,
        // so they are suppressed. Copy/paste/scroll and close are re-targeted at the overlay.
        if state.overlay_visible
            && matches!(
                action,
                Action::SplitHorizontal
                    | Action::SplitVertical
                    | Action::NewTab
                    | Action::CloseTab
                    | Action::NextTab
                    | Action::PrevTab
                    | Action::FocusLeft
                    | Action::FocusRight
                    | Action::FocusUp
                    | Action::FocusDown
                    | Action::ResizeLeft
                    | Action::ResizeRight
                    | Action::ResizeUp
                    | Action::ResizeDown
            )
        {
            return;
        }
        match action {
            Action::SplitHorizontal => {
                Self::split_pane(state, config, proxy, SplitDirection::Horizontal)
            }
            Action::SplitVertical => {
                Self::split_pane(state, config, proxy, SplitDirection::Vertical)
            }
            // With the overlay up, "close pane" dismisses the floating terminal (killing its
            // child); otherwise it closes the focused layout pane.
            Action::ClosePane if state.overlay_visible => Self::close_overlay(state),
            Action::ClosePane => Self::close_focused(state, event_loop),
            Action::NewTab => Self::new_tab(state, config, proxy),
            Action::CloseTab => Self::close_tab(state, event_loop),
            Action::NextTab => Self::switch_tab(state, 1),
            Action::PrevTab => Self::switch_tab(state, -1),
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
            Action::OpenConfig => Self::open_config(state, config, proxy),
            Action::OpenSettings => Self::open_settings(state, config),
            Action::ToggleFloatingTerminal => Self::toggle_floating(state, config, proxy),
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
        // A provisional grid; `resize_panes` immediately fixes it to the leaf's real area.
        let (cols, rows) = Self::provisional_grid(state);
        let pane = match build_pane(config, proxy, state.palette, cols, rows) {
            Ok(pane) => pane,
            Err(err) => {
                log::error!("failed to spawn pane: {err}");
                return;
            }
        };
        Self::insert_split_pane(state, direction, pane);
    }

    /// Open `config.toml` in the user's editor in a new pane stacked below the focused one.
    ///
    /// The editor is `$VISUAL`, else `$EDITOR`, else a platform default (`vi` on Unix,
    /// `notepad` on Windows). The config directory is created first so the editor can
    /// write the file on save even when it does not exist yet.
    fn open_config(state: &mut AppState, config: &Config, proxy: &EventLoopProxy<UserEvent>) {
        let path = Config::default_path();
        if let Some(dir) = path.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            log::error!("failed to create config dir {}: {err}", dir.display());
            return;
        }

        let (program, mut args) = resolve_editor();
        args.push(path.to_string_lossy().into_owned());
        let spec = PtyCommandSpec {
            program,
            args,
            cwd: None,
            env: Vec::new(),
        };

        let (cols, rows) = Self::provisional_grid(state);
        let pane = match build_pane_with_spec(config, proxy, state.palette, cols, rows, spec) {
            Ok(pane) => pane,
            Err(err) => {
                log::error!("failed to open config editor: {err}");
                return;
            }
        };
        Self::insert_split_pane(state, SplitDirection::Vertical, pane);
    }

    // --- inline settings overlay ----------------------------------------------

    /// Open the inline settings overlay, seeding it from the current config. Hides the
    /// floating terminal so only one overlay is shown at a time.
    fn open_settings(state: &mut AppState, config: &Config) {
        if state.settings.is_none() {
            state.settings = Some(SettingsMenu::new(config.clone()));
        }
        state.overlay_visible = false;
        state.force_present = true;
        state.window.request_redraw();
    }

    /// Apply the settings overlay's response to a key: repaint, close (persisting edits),
    /// or bail out to the raw-file editor.
    pub(super) fn apply_settings_response(
        state: &mut AppState,
        config: &Config,
        proxy: &EventLoopProxy<UserEvent>,
        response: SettingsResponse,
    ) {
        match response {
            SettingsResponse::Ignore => {}
            SettingsResponse::Redraw => {
                state.force_present = true;
                state.window.request_redraw();
            }
            SettingsResponse::Close => Self::close_settings(state),
            SettingsResponse::OpenEditor => {
                Self::close_settings(state);
                Self::open_config(state, config, proxy);
            }
        }
    }

    /// Close the settings overlay, persisting the edited config to disk if it changed. The
    /// file watcher then hot-reloads it, applying the changes live.
    fn close_settings(state: &mut AppState) {
        if let Some(menu) = state.settings.take()
            && menu.dirty
        {
            let path = Config::default_path();
            if let Err(err) = menu.config.save(&path) {
                log::error!("failed to save config from settings overlay: {err}");
            }
        }
        state.force_present = true;
        state.window.request_redraw();
    }

    /// A provisional cols/rows for a freshly spawned pane; `resize_panes` corrects it to the
    /// leaf's real area once the split lands.
    fn provisional_grid(state: &AppState) -> (u16, u16) {
        let size = state.window.inner_size();
        state.renderer.grid_size_for(size.width, size.height)
    }

    /// Split the focused leaf in `direction`, insert `pane` into the new leaf, and re-layout.
    fn insert_split_pane(state: &mut AppState, direction: SplitDirection, pane: super::pane::Pane) {
        let id = state.ids.next_pane();
        let tab = state.tab_mut();
        tab.layout.split(direction, id);
        tab.panes.insert(id, pane);
        Self::resize_panes(state);
        state.force_present = true;
        state.window.request_redraw();
    }

    /// Close the focused pane, dropping it (its `Pty` `Drop` kills the child).
    fn close_focused(state: &mut AppState, event_loop: &ActiveEventLoop) {
        let tab = state.tab_mut();
        let focused = tab.layout.focused();
        match tab.layout.close(focused) {
            Ok(()) => {
                tab.panes.remove(&focused);
                Self::resize_panes(state);
                state.force_present = true;
                state.window.request_redraw();
            }
            // Closing a tab's last pane closes the tab (and the last tab quits DevTerm,
            // parity with closing the window).
            Err(LayoutError::CannotCloseLastPane) => Self::close_tab(state, event_loop),
            Err(err) => log::warn!("close pane failed: {err}"),
        }
    }

    // --- tabs -------------------------------------------------------------------

    /// Open a new tab with a single fresh shell pane right of the current tab, and switch
    /// to it.
    pub(super) fn new_tab(
        state: &mut AppState,
        config: &Config,
        proxy: &EventLoopProxy<UserEvent>,
    ) {
        // A provisional grid; `resize_panes` immediately fixes it to the real layout area.
        let (cols, rows) = Self::provisional_grid(state);
        let pane = match build_pane(config, proxy, state.palette, cols, rows) {
            Ok(pane) => pane,
            Err(err) => {
                log::error!("failed to spawn pane for new tab: {err}");
                return;
            }
        };
        let pane_id = state.ids.next_pane();
        let mut panes = HashMap::new();
        panes.insert(pane_id, pane);
        let tab = Tab {
            id: state.ids.next_tab(),
            layout: LayoutTree::new(pane_id),
            panes,
        };
        state.tabs.insert(state.active_tab + 1, tab);
        Self::activate_tab(state, state.active_tab + 1);
    }

    /// Close the current tab, dropping all its panes (each `Pty` `Drop` kills its child).
    /// Closing the last tab quits DevTerm.
    fn close_tab(state: &mut AppState, event_loop: &ActiveEventLoop) {
        let active = state.active_tab;
        Self::close_tab_at(state, active, event_loop);
    }

    /// Close tab `index` (the bar's `×` button / middle click can target any tab, not
    /// just the active one). Closing the last tab quits DevTerm.
    pub(super) fn close_tab_at(state: &mut AppState, index: usize, event_loop: &ActiveEventLoop) {
        if state.tabs.len() == 1 {
            event_loop.exit();
            return;
        }
        state.tabs.remove(index);
        let next = if state.active_tab > index {
            state.active_tab - 1
        } else {
            state.active_tab.min(state.tabs.len() - 1)
        };
        Self::activate_tab(state, next);
    }

    /// Switch to the neighbouring tab in `delta` direction, wrapping at the ends.
    fn switch_tab(state: &mut AppState, delta: isize) {
        let n = state.tabs.len() as isize;
        if n <= 1 {
            return;
        }
        let next = (state.active_tab as isize + delta).rem_euclid(n) as usize;
        Self::activate_tab(state, next);
    }

    /// Make tab `index` the active one and bring its panes up to date with the current
    /// window size and cell metrics (both may have changed while it was in the background).
    pub(super) fn activate_tab(state: &mut AppState, index: usize) {
        state.active_tab = index;
        Self::resize_panes(state);
        state.force_present = true;
        state.window.request_redraw();
    }

    /// Toggle the floating "scratch" terminal. The first show spawns its child (the config
    /// shell); later toggles just hide/show it, keeping the process and its scrollback alive.
    fn toggle_floating(state: &mut AppState, config: &Config, proxy: &EventLoopProxy<UserEvent>) {
        if state.overlay_visible {
            state.overlay_visible = false;
        } else {
            if state.overlay.is_none() {
                let (cols, rows) = Self::overlay_grid(state);
                match build_pane(config, proxy, state.palette, cols, rows) {
                    Ok(pane) => state.overlay = Some(pane),
                    Err(err) => {
                        log::error!("failed to spawn floating terminal: {err}");
                        return;
                    }
                }
            }
            state.overlay_visible = true;
            // Re-derive the overlay grid from the current window size before it is shown.
            Self::resize_overlay(state);
        }
        state.force_present = true;
        state.window.request_redraw();
    }

    /// Dismiss the floating terminal, dropping its pane (its `Pty` `Drop` kills the child).
    pub(super) fn close_overlay(state: &mut AppState) {
        state.overlay = None;
        state.overlay_visible = false;
        state.force_present = true;
        state.window.request_redraw();
    }

    fn focus(state: &mut AppState, dir: Direction) {
        if state.tab_mut().layout.move_focus(dir) {
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
        state.tab_mut().layout.resize_directional(dir, STEP);
        Self::resize_panes(state);
        state.force_present = true;
        state.window.request_redraw();
    }

    fn scroll(state: &mut AppState, lines: i32) {
        {
            let Some(pane) = Self::active_pane_mut(state) else {
                return;
            };
            pane.term.scroll_display(lines);
        }
        state.force_present = true;
        state.window.request_redraw();
    }

    fn scroll_page(state: &mut AppState, sign: i32) {
        let rows = Self::focused_rows(state).max(2) as i32;
        Self::scroll(state, sign * (rows - 1));
    }

    /// Copy the focused pane's selection to the system clipboard.
    fn copy_selection(state: &mut AppState) {
        let Some(text) = Self::active_pane(state).and_then(|pane| pane.term.selected_text()) else {
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
        let Some(pane) = Self::active_pane(state) else {
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
            for pane in state.tabs.iter_mut().flat_map(|tab| tab.panes.values_mut()) {
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

/// Resolve the user's editor command into `(program, args)`.
///
/// Prefers `$VISUAL`, then `$EDITOR`; either may carry flags (e.g. `code --wait`), which are
/// split off on whitespace and kept as leading args. Falls back to `notepad` on Windows and
/// `vi` elsewhere when neither variable is set.
fn resolve_editor() -> (String, Vec<String>) {
    for var in ["VISUAL", "EDITOR"] {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        let mut parts = value.split_whitespace().map(str::to_owned);
        if let Some(program) = parts.next() {
            return (program, parts.collect());
        }
    }
    let program = if cfg!(windows) { "notepad" } else { "vi" };
    (program.to_owned(), Vec::new())
}
