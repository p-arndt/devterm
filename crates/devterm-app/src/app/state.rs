//! Window/GPU-surface-scoped application state and the external wake/reload event.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use winit::window::{CursorIcon, Window};

use devterm_config::{Action, KeyChord};
use devterm_core::{GutterId, IdGen, LayoutTree, PaneId, SplitDirection, TabId};
use devterm_render::Renderer;
use devterm_term::Palette;

use super::pane::Pane;

/// One tab/workspace: its own layout tree over its own set of panes. Exactly one tab is
/// active (visible, receiving input) at a time; the others keep their children running in
/// the background. Dropping a tab drops its panes, whose `Pty` `Drop` kills the children.
pub(super) struct Tab {
    /// Stable identity (unused for lookups today; reserved for session persistence).
    #[allow(dead_code)]
    pub(super) id: TabId,
    pub(super) layout: LayoutTree,
    pub(super) panes: HashMap<PaneId, Pane>,
}

/// Event delivered to the winit loop from outside `window_event`.
#[derive(Clone, Debug)]
pub enum UserEvent {
    /// A PTY reader produced output (or exited); request a redraw.
    Wake,
    /// `config.toml` changed on disk; reload it.
    ReloadConfig,
    /// A background check found a newer published release (carries the version,
    /// e.g. `"0.2.0"`). Triggers the "new version available" notice + prompt.
    UpdateAvailable(String),
    /// A self-update attempt finished: `Ok(version)` installed successfully (restart
    /// to run it), `Err(message)` failed with a user-facing reason. Only ever
    /// constructed on Windows (where self-update runs), but matched on all platforms.
    #[cfg_attr(not(windows), allow(dead_code))]
    UpdateResult(Result<String, String>),
}

/// An in-progress mouse drag of a split divider (gutter).
#[derive(Clone, Copy, Debug)]
pub(super) struct GutterDrag {
    /// The boundary being dragged. Stable while the tree's shape is unchanged.
    pub(super) id: GutterId,
    /// The split's axis, which fixes the drag axis (Horizontal split => drag along x).
    pub(super) axis: SplitDirection,
    /// Pointer position (physical px) at the last processed drag step.
    pub(super) last: (f64, f64),
}

/// Everything that only exists once a window/GPU surface has been created (`resumed`).
pub(super) struct AppState {
    pub(super) window: Arc<Window>,
    pub(super) renderer: Renderer,
    /// All tabs, in display order. Never empty (an empty window exits the app instead).
    pub(super) tabs: Vec<Tab>,
    /// Index into [`tabs`](Self::tabs) of the visible tab.
    pub(super) active_tab: usize,
    /// A floating "scratch" terminal drawn centered on top of the layout, spawned lazily on
    /// the first toggle. It lives outside the [`LayoutTree`]; when `overlay_visible` it
    /// captures keyboard input, copy/paste and scrolling. Dropped when its child exits or it
    /// is closed.
    pub(super) overlay: Option<Pane>,
    /// Whether the floating terminal is currently shown (and capturing interaction).
    pub(super) overlay_visible: bool,
    /// The inline settings overlay while it is open; `None` when closed. When present it
    /// captures all keyboard input and is drawn (like the floating terminal) as the top
    /// overlay layer, taking precedence over it.
    pub(super) settings: Option<super::settings::SettingsMenu>,
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
    /// Active split-divider drag, if the current press started on a gutter (takes
    /// precedence over text selection until released).
    pub(super) drag: Option<GutterDrag>,
    /// Current window cursor icon; tracked so we only call `set_cursor_icon` on change.
    pub(super) cursor_icon: CursorIcon,

    // --- frame-timing / anti-flicker state ------------------------------------
    /// Instant of the most recent PTY wake (byte burst); drives coalescing.
    pub(super) last_output: Instant,
    /// A wake arrived and its output has not yet been presented (present is pending).
    pub(super) pending_output: bool,
    /// Instant of the last presented frame; bounds how long a burst may defer.
    pub(super) last_present: Instant,
    /// A non-terminal change needs a present even if no terminal is dirty. Cleared on
    /// present.
    pub(super) force_present: bool,

    // --- cursor blink ---------------------------------------------------------
    /// Whether the focused cursor is currently in its visible phase.
    pub(super) blink_visible: bool,
    /// Instant of the last blink toggle (or last activity reset).
    pub(super) last_blink_toggle: Instant,
}

impl AppState {
    /// The active (visible) tab.
    ///
    /// These borrow all of `self`; where the renderer must be borrowed alongside the
    /// tab's panes, index `self.tabs[self.active_tab]` directly to split the borrow.
    pub(super) fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    /// Mutable counterpart to [`tab`](Self::tab).
    pub(super) fn tab_mut(&mut self) -> &mut Tab {
        let active = self.active_tab;
        &mut self.tabs[active]
    }
}
