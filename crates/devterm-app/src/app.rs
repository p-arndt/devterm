//! The winit application: wires PTY <-> terminal model <-> renderer.
//!
//! Each pane is a `{ Pty, Term }` pair keyed by a [`PaneId`]; the window owns a
//! [`LayoutTree`] from `devterm-core`. Splits, focus moves, resizes and closes are layout
//! operations that re-derive every pane's own cols/rows from its pixel rectangle. Config
//! drives the keymap, theme palette and shell, and a background file watcher hot-reloads
//! `config.toml`.

use std::collections::HashMap;
use std::sync::Arc;

use crossbeam_channel::Receiver;
use notify::{EventKind, RecursiveMode, Watcher};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use devterm_config::{Action, Color, Config, KeyChord, KeyCode, Mods, Named, Theme};
use devterm_core::{Direction, IdGen, LayoutError, LayoutTree, PaneId, Rect, SplitDirection};
use devterm_pty::{Pty, PtyCommandSpec, PtyEvent, PtySize};
use devterm_render::{PaneView, Renderer};
use devterm_term::{Palette, Rgb, SelectionMode, Term};

use crate::keymap::keymap;

/// Event delivered to the winit loop from outside `window_event`.
#[derive(Clone, Copy, Debug)]
pub enum UserEvent {
    /// A PTY reader produced output (or exited); request a redraw.
    Wake,
    /// `config.toml` changed on disk; reload it.
    ReloadConfig,
}

/// One terminal pane: its child process and the emulator model driving it.
struct Pane {
    pty: Pty,
    term: Term,
    events: Receiver<PtyEvent>,
}

/// Everything that only exists once a window/GPU surface has been created (`resumed`).
struct AppState {
    window: Arc<Window>,
    renderer: Renderer,
    layout: LayoutTree,
    panes: HashMap<PaneId, Pane>,
    ids: IdGen,
    /// Resolved chord -> action lookup, rebuilt on config reload.
    keymap: HashMap<KeyChord, Action>,
    /// Theme palette applied to every pane, refreshed on config reload.
    palette: Palette,
    /// System clipboard, constructed lazily on first Copy/Paste.
    clipboard: Option<arboard::Clipboard>,
    /// Last known pointer position (physical px).
    pointer: (f64, f64),
    /// Whether the left mouse button is currently held (drag-selecting).
    mouse_down: bool,
}

/// The top-level application handler passed to `EventLoop::run_app`.
pub struct App {
    config: Config,
    proxy: EventLoopProxy<UserEvent>,
    modifiers: ModifiersState,
    state: Option<AppState>,
}

impl App {
    /// Create the handler. The window and renderer are built later, in `resumed`.
    pub fn new(config: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        App {
            config,
            proxy,
            modifiers: ModifiersState::empty(),
            state: None,
        }
    }

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
    fn resize_panes(state: &mut AppState) {
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
    fn focused_rows(state: &AppState) -> u16 {
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
    fn resize_surface(state: &mut AppState, width: u32, height: u32) {
        state.renderer.resize(width, height);
        Self::resize_panes(state);
    }

    // --- rendering ------------------------------------------------------------

    /// Drain child output into each model, flush emulator replies, reap exited children,
    /// then draw one frame of every laid-out pane.
    fn redraw(state: &mut AppState, event_loop: &ActiveEventLoop) {
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
    fn send_input(state: &AppState, bytes: &[u8]) {
        let focused = state.layout.focused();
        if let Some(pane) = state.panes.get(&focused)
            && let Err(err) = pane.pty.write(bytes)
        {
            log::warn!("failed to write to pty: {err}");
        }
    }

    // --- actions --------------------------------------------------------------

    /// Dispatch a bound [`Action`]. Takes the pieces it needs as arguments so it can be
    /// called while `state` is borrowed out of `self`.
    fn dispatch(
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
        state.window.request_redraw();
    }

    /// Close the focused pane, dropping it (its `Pty` `Drop` kills the child).
    fn close_focused(state: &mut AppState, event_loop: &ActiveEventLoop) {
        let focused = state.layout.focused();
        match state.layout.close(focused) {
            Ok(()) => {
                state.panes.remove(&focused);
                Self::resize_panes(state);
                state.window.request_redraw();
            }
            // Closing the last pane quits DevTerm (parity with closing the window).
            Err(LayoutError::CannotCloseLastPane) => event_loop.exit(),
            Err(err) => log::warn!("close pane failed: {err}"),
        }
    }

    fn focus(state: &mut AppState, dir: Direction) {
        if state.layout.move_focus(dir) {
            state.window.request_redraw();
        }
    }

    fn resize_focused(state: &mut AppState, dir: Direction) {
        // devterm-core grows the focused pane along the axis of `dir` (the sign selects the
        // axis, not the side); factor > 1 grows, < 1 shrinks. We grow toward the pressed key.
        const GROW: f32 = 1.1;
        state.layout.resize(dir, GROW);
        Self::resize_panes(state);
        state.window.request_redraw();
    }

    fn scroll(state: &mut AppState, lines: i32) {
        let focused = state.layout.focused();
        if let Some(pane) = state.panes.get_mut(&focused) {
            pane.term.scroll_display(lines);
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

    // --- input ----------------------------------------------------------------

    /// Translate a pressed key into a [`KeyChord`], or `None` for pure modifiers / dead keys.
    fn chord_from_event(event: &KeyEvent, mods: ModifiersState) -> Option<KeyChord> {
        let code = match &event.logical_key {
            Key::Character(s) => KeyCode::Char(s.chars().next()?.to_ascii_lowercase()),
            Key::Named(named) => KeyCode::Named(map_named(*named)?),
            _ => return None,
        };
        Some(KeyChord::new(mods_from_winit(mods), code))
    }

    // --- selection ------------------------------------------------------------

    /// The id of the pane whose pixel rectangle contains `pointer`.
    fn pane_at(state: &AppState, (px, py): (f64, f64)) -> Option<PaneId> {
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
    fn begin_selection(state: &mut AppState) {
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
    fn extend_selection(state: &mut AppState) {
        let focused = state.layout.focused();
        if let Some((col, line)) = Self::pane_local(state, focused, state.pointer)
            && let Some(pane) = state.panes.get_mut(&focused)
        {
            pane.term.update_selection(col, line);
            state.window.request_redraw();
        }
    }

    // --- config hot-reload ----------------------------------------------------

    /// Reload `config.toml` from disk and re-apply the keymap, palette and (if changed)
    /// font size. Shell changes take effect on the next spawned pane.
    fn reload_config(&mut self) {
        let path = Config::default_path();
        let config = match Config::load(&path) {
            Ok(config) => config,
            Err(err) => {
                log::warn!("config reload failed: {err}");
                return;
            }
        };
        log::info!("reloaded config from {}", path.display());

        let palette = palette_from_theme(&config.theme);
        let font_changed = (config.font_size - self.config.font_size).abs() > f32::EPSILON;
        let font_size = config.font_size;
        self.config = config;

        if let Some(state) = self.state.as_mut() {
            state.keymap = build_keymap(&self.config);
            state.palette = palette;
            for pane in state.panes.values_mut() {
                pane.term.set_palette(palette);
            }
            if font_changed {
                state.renderer.set_font_size(font_size);
                Self::resize_panes(state);
            }
            state.window.request_redraw();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            // Already initialized; some platforms may emit `resumed` again.
            return;
        }

        let attributes = Window::default_attributes().with_title("DevTerm");
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                log::error!("failed to create window: {err}");
                event_loop.exit();
                return;
            }
        };

        let renderer = match Renderer::new(window.clone(), self.config.font_size) {
            Ok(renderer) => renderer,
            Err(err) => {
                log::error!("failed to create renderer: {err}");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        let (cols, rows) = renderer.grid_size_for(size.width, size.height);
        let palette = palette_from_theme(&self.config.theme);

        let pane = match build_pane(&self.config, &self.proxy, palette, cols, rows) {
            Ok(pane) => pane,
            Err(err) => {
                log::error!("failed to spawn pty: {err}");
                event_loop.exit();
                return;
            }
        };

        let mut ids = IdGen::new();
        let pane_id = ids.next_pane();
        let layout = LayoutTree::new(pane_id);

        let mut panes = HashMap::new();
        panes.insert(pane_id, pane);

        window.request_redraw();

        self.state = Some(AppState {
            window,
            renderer,
            layout,
            panes,
            ids,
            keymap: build_keymap(&self.config),
            palette,
            clipboard: None,
            pointer: (0.0, 0.0),
            mouse_down: false,
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                if let Some(state) = &self.state {
                    state.window.request_redraw();
                }
            }
            UserEvent::ReloadConfig => self.reload_config(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if window_id != state.window.id() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                Self::resize_surface(state, size.width, size.height);
                state.window.request_redraw();
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.renderer.set_scale_factor(scale_factor);
                let size = state.window.inner_size();
                Self::resize_surface(state, size.width, size.height);
                state.window.request_redraw();
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // A bound chord takes precedence; otherwise fall through to the raw byte path.
                let action = Self::chord_from_event(&event, self.modifiers)
                    .and_then(|chord| state.keymap.get(&chord).copied());
                if let Some(action) = action {
                    Self::dispatch(state, &self.config, &self.proxy, action, event_loop);
                } else if let Some(bytes) = keymap(&event, self.modifiers) {
                    Self::send_input(state, &bytes);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                state.pointer = (position.x, position.y);
                if state.mouse_down {
                    Self::extend_selection(state);
                }
            }

            WindowEvent::MouseInput {
                state: button_state,
                button,
                ..
            } => {
                if button == MouseButton::Left {
                    match button_state {
                        ElementState::Pressed => {
                            state.mouse_down = true;
                            Self::begin_selection(state);
                        }
                        ElementState::Released => state.mouse_down = false,
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let cell_height = state.renderer.cell_metrics().height.max(1.0);
                // Positive wheel motion scrolls up into history.
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                    MouseScrollDelta::PixelDelta(pos) => {
                        (pos.y as f32 / cell_height).round() as i32
                    }
                };
                if lines != 0 {
                    // Target the pane under the pointer, falling back to the focused one.
                    let target = Self::pane_at(state, state.pointer)
                        .unwrap_or_else(|| state.layout.focused());
                    if let Some(pane) = state.panes.get_mut(&target) {
                        pane.term.scroll_display(lines);
                    }
                    state.window.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                Self::redraw(state, event_loop);
            }

            _ => {}
        }
    }
}

/// Build a `{ pty, term }` pane at `cols` x `rows` using the resolved shell and palette.
fn build_pane(
    config: &Config,
    proxy: &EventLoopProxy<UserEvent>,
    palette: Palette,
    cols: u16,
    rows: u16,
) -> anyhow::Result<Pane> {
    let mut term = Term::new(cols, rows, config.scrollback_lines);
    term.set_palette(palette);

    // Resolve the shell from config; an empty program means "app default".
    let resolved = config.resolve_shell();
    let spec = if resolved.program.is_empty() {
        let mut spec = PtyCommandSpec::default_shell();
        spec.args = resolved.args;
        spec
    } else {
        PtyCommandSpec {
            program: resolved.program,
            args: resolved.args,
            cwd: None,
            env: Vec::new(),
        }
    };

    let proxy = proxy.clone();
    let pty = Pty::spawn(&spec, PtySize { cols, rows }, move || {
        let _ = proxy.send_event(UserEvent::Wake);
    })?;
    let events = pty.events();
    Ok(Pane { pty, term, events })
}

/// Resolve config keybindings into a chord -> action lookup (later bindings win).
fn build_keymap(config: &Config) -> HashMap<KeyChord, Action> {
    config.resolve_keybindings().into_iter().collect()
}

/// Map a config [`Theme`] onto a `devterm-term` [`Palette`].
fn palette_from_theme(theme: &Theme) -> Palette {
    let convert = |c: Color| Rgb {
        r: c.r,
        g: c.g,
        b: c.b,
    };
    let mut ansi = [Rgb::default(); 16];
    for (dst, src) in ansi.iter_mut().zip(theme.ansi.iter()) {
        *dst = convert(*src);
    }
    Palette {
        ansi,
        foreground: convert(theme.foreground),
        background: convert(theme.background),
        cursor: convert(theme.cursor),
    }
}

/// Map winit modifier state onto config [`Mods`].
fn mods_from_winit(mods: ModifiersState) -> Mods {
    Mods {
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        shift: mods.shift_key(),
        logo: mods.super_key(),
    }
}

/// Map a winit [`NamedKey`] onto a config [`Named`], or `None` for keys we do not bind.
fn map_named(named: NamedKey) -> Option<Named> {
    Some(match named {
        NamedKey::Enter => Named::Enter,
        NamedKey::Tab => Named::Tab,
        NamedKey::Escape => Named::Escape,
        NamedKey::Space => Named::Space,
        NamedKey::Backspace => Named::Backspace,
        NamedKey::Delete => Named::Delete,
        NamedKey::Insert => Named::Insert,
        NamedKey::Home => Named::Home,
        NamedKey::End => Named::End,
        NamedKey::PageUp => Named::PageUp,
        NamedKey::PageDown => Named::PageDown,
        NamedKey::ArrowUp => Named::ArrowUp,
        NamedKey::ArrowDown => Named::ArrowDown,
        NamedKey::ArrowLeft => Named::ArrowLeft,
        NamedKey::ArrowRight => Named::ArrowRight,
        NamedKey::F1 => Named::F1,
        NamedKey::F2 => Named::F2,
        NamedKey::F3 => Named::F3,
        NamedKey::F4 => Named::F4,
        NamedKey::F5 => Named::F5,
        NamedKey::F6 => Named::F6,
        NamedKey::F7 => Named::F7,
        NamedKey::F8 => Named::F8,
        NamedKey::F9 => Named::F9,
        NamedKey::F10 => Named::F10,
        NamedKey::F11 => Named::F11,
        NamedKey::F12 => Named::F12,
        _ => return None,
    })
}

/// Watch the `config.toml` directory and post [`UserEvent::ReloadConfig`] on change.
///
/// Returns the watcher, which the caller must keep alive (dropping it stops watching);
/// `None` disables hot-reload (directory absent, or the watcher could not be created).
pub fn spawn_config_watcher(
    proxy: EventLoopProxy<UserEvent>,
) -> Option<notify::RecommendedWatcher> {
    let path = Config::default_path();
    let dir = path.parent()?.to_path_buf();
    if dir.as_os_str().is_empty() || !dir.exists() {
        log::info!("config directory {dir:?} absent; hot-reload disabled");
        return None;
    }

    let target = path;
    // Debounce: editors emit a burst of events per save. Start in the past so the first
    // change always passes.
    let last = std::sync::Mutex::new(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or_else(std::time::Instant::now),
    );

    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else {
                return;
            };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                return;
            }
            if !event.paths.contains(&target) {
                return;
            }
            if let Ok(mut guard) = last.lock() {
                let now = std::time::Instant::now();
                if now.duration_since(*guard) < std::time::Duration::from_millis(150) {
                    return;
                }
                *guard = now;
            }
            let _ = proxy.send_event(UserEvent::ReloadConfig);
        }) {
            Ok(watcher) => watcher,
            Err(err) => {
                log::warn!("failed to create config watcher: {err}");
                return None;
            }
        };

    if let Err(err) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
        log::warn!("failed to watch config dir {dir:?}: {err}");
        return None;
    }
    log::info!("watching {dir:?} for config changes");
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_keys() {
        assert_eq!(map_named(NamedKey::Enter), Some(Named::Enter));
        assert_eq!(map_named(NamedKey::ArrowLeft), Some(Named::ArrowLeft));
        assert_eq!(map_named(NamedKey::PageUp), Some(Named::PageUp));
        assert_eq!(map_named(NamedKey::F5), Some(Named::F5));
        // A key we deliberately do not bind.
        assert_eq!(map_named(NamedKey::CapsLock), None);
    }

    #[test]
    fn winit_mods_map_to_config_mods() {
        let mods = mods_from_winit(ModifiersState::CONTROL | ModifiersState::SHIFT);
        assert!(mods.ctrl && mods.shift);
        assert!(!mods.alt && !mods.logo);
    }

    #[test]
    fn theme_maps_to_palette() {
        let palette = palette_from_theme(&Theme::default());
        assert_eq!(
            palette.foreground,
            Rgb {
                r: 0xd0,
                g: 0xd0,
                b: 0xd0
            }
        );
        assert_eq!(palette.background, Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            palette.ansi[1],
            Rgb {
                r: 0x80,
                g: 0,
                b: 0
            }
        );

        // A different theme yields a different background.
        let gruvbox = palette_from_theme(&Theme::builtin("gruvbox-dark").unwrap());
        assert_ne!(gruvbox.background, palette.background);
    }

    #[test]
    fn default_keymap_binds_split_chord() {
        // The resolved lookup contains the documented default Copy chord.
        let map = build_keymap(&Config::default());
        let chord: KeyChord = "ctrl+shift+c".parse().unwrap();
        assert_eq!(map.get(&chord), Some(&Action::Copy));
    }
}
