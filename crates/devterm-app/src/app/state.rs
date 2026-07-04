//! Window/GPU-surface-scoped application state and the external wake/reload event.

use std::collections::HashMap;
use std::sync::Arc;

use winit::window::Window;

use devterm_config::{Action, KeyChord};
use devterm_core::{IdGen, LayoutTree, PaneId};
use devterm_render::Renderer;
use devterm_term::Palette;

use super::pane::Pane;

/// Event delivered to the winit loop from outside `window_event`.
#[derive(Clone, Copy, Debug)]
pub enum UserEvent {
    /// A PTY reader produced output (or exited); request a redraw.
    Wake,
    /// `config.toml` changed on disk; reload it.
    ReloadConfig,
}

/// Everything that only exists once a window/GPU surface has been created (`resumed`).
pub(super) struct AppState {
    pub(super) window: Arc<Window>,
    pub(super) renderer: Renderer,
    pub(super) layout: LayoutTree,
    pub(super) panes: HashMap<PaneId, Pane>,
    pub(super) ids: IdGen,
    /// Resolved chord -> action lookup, rebuilt on config reload.
    pub(super) keymap: HashMap<KeyChord, Action>,
    /// Theme palette applied to every pane, refreshed on config reload.
    pub(super) palette: Palette,
    /// System clipboard, constructed lazily on first Copy/Paste.
    pub(super) clipboard: Option<arboard::Clipboard>,
    /// Last known pointer position (physical px).
    pub(super) pointer: (f64, f64),
    /// Whether the left mouse button is currently held (drag-selecting).
    pub(super) mouse_down: bool,
}
