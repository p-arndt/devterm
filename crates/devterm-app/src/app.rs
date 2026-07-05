//! The winit application: wires PTY <-> terminal model <-> renderer.
//!
//! Each pane is a `{ Pty, Term }` pair keyed by a [`PaneId`]; the window owns a
//! [`LayoutTree`] from `devterm-core`. Splits, focus moves, resizes and closes are layout
//! operations that re-derive every pane's own cols/rows from its pixel rectangle. Config
//! drives the keymap, theme palette and shell, and a background file watcher hot-reloads
//! `config.toml`.
//!
//! This module owns the [`App`] handler and the winit event loop; the mechanics are split
//! across submodules: [`state`] (window-scoped state), [`pane`] (a pane and its child),
//! [`view`] (pixel geometry, rendering, selection), [`actions`] (bound-action dispatch and
//! config reload), [`input`] (winit->config mapping), and [`config_watch`] (the hot-reload
//! watcher).
//!
//! [`PaneId`]: devterm_core::PaneId
//! [`LayoutTree`]: devterm_core::LayoutTree

mod actions;
mod config_watch;
mod input;
mod pane;
mod present;
mod settings;
mod state;
mod view;
mod window;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, WindowId};

use devterm_config::Config;
use devterm_core::{IdGen, LayoutTree};
use devterm_render::Renderer;

use crate::keymap::keymap;

use input::{build_keymap, chord_from_event, palette_from_theme};
use pane::build_pane;
use present::{BLINK_INTERVAL, MAX_DEFER, SETTLE_WINDOW};
use state::AppState;

pub use config_watch::spawn_config_watcher;
pub use state::UserEvent;

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
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            // Already initialized; some platforms may emit `resumed` again.
            return;
        }

        let attributes = window::initial_attributes(event_loop, "DevTerm");
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                log::error!("failed to create window: {err}");
                event_loop.exit();
                return;
            }
        };

        let mut renderer = match Renderer::new(window.clone(), self.config.font_size) {
            Ok(renderer) => renderer,
            Err(err) => {
                log::error!("failed to create renderer: {err}");
                event_loop.exit();
                return;
            }
        };

        // Apply font family + line height before the first grid derivation so cols/rows are
        // computed against the final cell metrics.
        let family = if self.config.font_family.is_empty() {
            None
        } else {
            Some(self.config.font_family.clone())
        };
        renderer.set_font_family(family);
        renderer.set_line_height(self.config.line_height);

        let size = window.inner_size();
        let (cols, rows) = renderer.grid_size_for(size.width, size.height);
        // Resolve the effective theme (named base + inline overlay) rather than the raw table.
        let palette = palette_from_theme(&self.config.resolve_theme());

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

        let now = Instant::now();
        self.state = Some(AppState {
            window,
            renderer,
            layout,
            panes,
            overlay: None,
            overlay_visible: false,
            settings: None,
            ids,
            keymap: build_keymap(&self.config),
            palette,
            clipboard: None,
            pointer: (0.0, 0.0),
            mouse_down: false,
            drag: None,
            cursor_icon: CursorIcon::Default,
            last_output: now,
            pending_output: false,
            last_present: now,
            // Force the very first frame even though no terminal has produced output yet.
            force_present: true,
            blink_visible: true,
            last_blink_toggle: now,
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                let blink = self.config.cursor.blink;
                if let Some(state) = self.state.as_mut() {
                    // Record the burst instead of painting now; `about_to_wait` schedules a
                    // coalesced present once the byte burst settles.
                    let now = Instant::now();
                    state.last_output = now;
                    state.pending_output = true;
                    // Output counts as activity: keep the cursor solid and restart its phase.
                    if blink {
                        state.blink_visible = true;
                        state.last_blink_toggle = now;
                    }
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
                state.force_present = true;
                state.window.request_redraw();
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.renderer.set_scale_factor(scale_factor);
                let size = state.window.inner_size();
                Self::resize_surface(state, size.width, size.height);
                state.force_present = true;
                state.window.request_redraw();
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // Typing counts as activity: keep the cursor solid and restart its phase.
                if self.config.cursor.blink {
                    state.blink_visible = true;
                    state.last_blink_toggle = Instant::now();
                }
                // The inline settings overlay captures all keyboard input while open.
                if let Some(menu) = state.settings.as_mut() {
                    let response =
                        menu.handle_key(&event.logical_key, event.text.as_deref(), self.modifiers);
                    Self::apply_settings_response(state, &self.config, &self.proxy, response);
                    return;
                }
                // A bound chord takes precedence; otherwise fall through to the raw byte path.
                let action = chord_from_event(&event, self.modifiers)
                    .and_then(|chord| state.keymap.get(&chord).copied());
                if let Some(action) = action {
                    Self::dispatch(state, &self.config, &self.proxy, action, event_loop);
                } else if let Some(bytes) = keymap(&event, self.modifiers) {
                    Self::send_input(state, &bytes);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                state.pointer = (position.x, position.y);
                if state.drag.is_some() {
                    // A divider drag takes precedence over selection.
                    Self::update_gutter_drag(state);
                } else if state.mouse_down {
                    Self::extend_selection(state);
                } else {
                    // Idle hover: show a resize cursor over dividers.
                    Self::update_hover_cursor(state);
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
                            // A press on a divider starts a resize drag instead of a
                            // selection; a press inside a pane selects text as before.
                            if !Self::begin_gutter_drag(state) {
                                state.mouse_down = true;
                                Self::begin_selection(state);
                            }
                        }
                        ElementState::Released => {
                            state.mouse_down = false;
                            state.drag = None;
                            Self::update_hover_cursor(state);
                        }
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
                    if state.overlay_visible {
                        // The floating terminal captures scrolling while it is shown.
                        if let Some(overlay) = state.overlay.as_mut() {
                            overlay.term.scroll_display(lines);
                        }
                    } else {
                        // Target the pane under the pointer, falling back to the focused one.
                        let target = Self::pane_at(state, state.pointer)
                            .unwrap_or_else(|| state.layout.focused());
                        if let Some(pane) = state.panes.get_mut(&target) {
                            pane.term.scroll_display(lines);
                        }
                    }
                    state.window.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                Self::redraw(state, &self.config, event_loop);
            }

            _ => {}
        }
    }

    /// Scheduler: after each batch of events, decide when the loop should next wake.
    ///
    /// Two timers drive it — the coalesced PTY-output present (paint once a byte burst has
    /// settled, bounded by [`MAX_DEFER`]) and the cursor blink toggle. Whichever is due now
    /// triggers its action; otherwise we `WaitUntil` the earliest pending deadline (or plain
    /// `Wait` when nothing is scheduled, so an idle terminal never spins the CPU).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let blink = self.config.cursor.blink;
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let now = Instant::now();
        let mut wake_at: Option<Instant> = None;

        // Coalesced PTY-output present: fire once settled, but no later than the deferral cap.
        if state.pending_output {
            let deadline = (state.last_output + SETTLE_WINDOW).min(state.last_present + MAX_DEFER);
            if now >= deadline {
                state.window.request_redraw();
            } else {
                wake_at = Some(earliest(wake_at, deadline));
            }
        }

        // Cursor blink: toggle visibility on the interval and repaint the focused cursor.
        if blink {
            let deadline = state.last_blink_toggle + BLINK_INTERVAL;
            if now >= deadline {
                state.blink_visible = !state.blink_visible;
                state.last_blink_toggle = now;
                state.force_present = true;
                state.window.request_redraw();
            } else {
                wake_at = Some(earliest(wake_at, deadline));
            }
        }

        match wake_at {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// The earlier of an optional running deadline and a new one.
fn earliest(current: Option<Instant>, candidate: Instant) -> Instant {
    match current {
        Some(existing) => existing.min(candidate),
        None => candidate,
    }
}
