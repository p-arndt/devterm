//! The winit application: wires PTY <-> terminal model <-> renderer.
//!
//! For the M0 walking skeleton this drives a single full-window PowerShell pane, but the
//! structure is already multi-pane ready: each pane is a `{ Pty, Term }` pair keyed by a
//! [`PaneId`], and the window owns a [`LayoutTree`] from `devterm-core` (M0 uses a single
//! leaf). Rendering iterates the layout so adding splits later is a layout change, not an
//! app rewrite.

use std::collections::HashMap;
use std::sync::Arc;

use crossbeam_channel::Receiver;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use devterm_config::Config;
use devterm_core::{IdGen, LayoutTree, PaneId, Rect};
use devterm_pty::{Pty, PtyCommandSpec, PtyEvent, PtySize};
use devterm_render::{PaneView, Renderer};
use devterm_term::Term;

use crate::keymap::keymap;

/// Event delivered to the winit loop from outside the main thread.
#[derive(Clone, Copy, Debug)]
pub enum UserEvent {
    /// The PTY reader produced output (or exited); request a redraw.
    Wake,
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
    #[allow(dead_code)] // Reserved for spawning additional panes in M1.
    ids: IdGen,
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

    /// Resize the renderer, then re-derive cols/rows and resize the model + child for
    /// every pane. `width`/`height` are physical pixels.
    fn resize_surface(state: &mut AppState, width: u32, height: u32) {
        state.renderer.resize(width, height);
        let (cols, rows) = state.renderer.grid_size_for(width, height);
        for pane in state.panes.values_mut() {
            pane.term.resize(cols, rows);
            let _ = pane.pty.resize(PtySize { cols, rows });
        }
    }

    /// Drain child output into the model, flush emulator replies to the child, then draw
    /// one frame of every pane laid out over the window.
    fn redraw(state: &mut AppState, event_loop: &ActiveEventLoop) {
        // Pump each pane's PTY: feed output into the parser and forward emulator writes.
        for pane in state.panes.values_mut() {
            while let Ok(event) = pane.events.try_recv() {
                match event {
                    PtyEvent::Output(bytes) => pane.term.advance(&bytes),
                    PtyEvent::Exited(_code) => {
                        event_loop.exit();
                        return;
                    }
                }
            }
            let writes = pane.term.drain_pty_writes();
            if !writes.is_empty() {
                let _ = pane.pty.write(&writes);
            }
        }

        // Lay out the panes and snapshot each one for the renderer.
        let focused = state.layout.focused();
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

        let term = Term::new(cols, rows, self.config.scrollback_lines);

        // Build the shell spec from config, falling back to the default shell.
        let spec = if self.config.shell_program.is_empty() {
            let mut spec = PtyCommandSpec::default_shell();
            spec.args = self.config.shell_args.clone();
            spec
        } else {
            PtyCommandSpec {
                program: self.config.shell_program.clone(),
                args: self.config.shell_args.clone(),
                cwd: None,
                env: Vec::new(),
            }
        };

        let proxy = self.proxy.clone();
        let pty = match Pty::spawn(&spec, PtySize { cols, rows }, move || {
            let _ = proxy.send_event(UserEvent::Wake);
        }) {
            Ok(pty) => pty,
            Err(err) => {
                log::error!("failed to spawn pty: {err}");
                event_loop.exit();
                return;
            }
        };
        let events = pty.events();

        let mut ids = IdGen::new();
        let pane_id = ids.next_pane();
        let layout = LayoutTree::new(pane_id);

        let mut panes = HashMap::new();
        panes.insert(pane_id, Pane { pty, term, events });

        window.request_redraw();

        self.state = Some(AppState {
            window,
            renderer,
            layout,
            panes,
            ids,
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                if let Some(state) = &self.state {
                    state.window.request_redraw();
                }
            }
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
                if event.state == ElementState::Pressed
                    && let Some(bytes) = keymap(&event, self.modifiers)
                {
                    Self::send_input(state, &bytes);
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
                    let focused = state.layout.focused();
                    if let Some(pane) = state.panes.get_mut(&focused) {
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
