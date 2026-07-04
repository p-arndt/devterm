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
mod state;
mod view;

use std::collections::HashMap;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use devterm_config::Config;
use devterm_core::{IdGen, LayoutTree};
use devterm_render::Renderer;

use crate::keymap::keymap;

use input::{build_keymap, chord_from_event, palette_from_theme};
use pane::build_pane;
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
