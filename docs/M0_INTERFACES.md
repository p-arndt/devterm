# DevTerm — M0 interface contract (authoritative)

This document defines the **exact public APIs** each crate must expose for the M0 walking
skeleton. Parallel implementers rely on it to stay wire-compatible. Implement these
signatures verbatim (names, field names, argument order). If reality forces a deviation,
prefer the smallest change and note it in your return value so the integrator can reconcile.

Locked dependency versions (read the real source under
`C:\Users\Patrick\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\` — do **not** trust
API memory):

- `alacritty_terminal 0.25.1` (uses `vte 0.15.0`)
- `portable-pty 0.9.0`
- `wgpu 24.0.5`
- `winit 0.30.13`
- `swash 0.2.9`
- `fontdb 0.23.0`
- `pollster 0.4.0`, `bytemuck 1.25.0`, `crossbeam-channel 0.5.15`

General conventions:
- All code and comments in **English**.
- `#![forbid(unsafe_code)]` stays in `devterm-pty`, `devterm-term`, `devterm-config`. In
  `devterm-render` unsafe is *allowed if unavoidable* (document each block); prefer the safe
  `Arc<Window>` wgpu surface path so it can stay forbidden if possible.
- Colors are fully resolved to RGB inside `devterm-term`; the renderer is "dumb" and never
  sees palette indices, inverse, or dim.
- Coordinates: `devterm-core::Rect` is the unit square `(0,0)..(1,1)`; only the renderer
  scales to physical pixels.

---

## devterm-pty  (`crates/devterm-pty/src/lib.rs`)

Wraps `portable-pty` (ConPTY on Windows). A reader thread streams child output on a channel
and calls a wake callback so the app can request a redraw.

```rust
use std::path::PathBuf;
use crossbeam_channel::Receiver;

/// Terminal size in cells (pixel_* may be 0; ConPTY ignores them).
#[derive(Clone, Copy, Debug)]
pub struct PtySize { pub cols: u16, pub rows: u16 }

/// What the child terminal produced or its exit.
#[derive(Clone, Debug)]
pub enum PtyEvent {
    /// Raw bytes from the child (feed straight into `devterm_term::Term::advance`).
    Output(Vec<u8>),
    /// The child process ended; exit code if known.
    Exited(Option<i32>),
}

/// How to launch the shell.
#[derive(Clone, Debug)]
pub struct PtyCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl PtyCommandSpec {
    /// PowerShell 7 (`pwsh.exe`) if on PATH, else Windows PowerShell
    /// (`powershell.exe`). Resolution happens at spawn time.
    pub fn default_shell() -> Self;
}

pub struct Pty { /* master, child, writer, reader thread handle */ }

impl Pty {
    /// Spawn `spec` on a fresh ConPTY of `size`. Starts a reader thread that forwards
    /// output on the returned `Receiver` and calls `wake` after every chunk (and on exit).
    pub fn spawn<F>(spec: &PtyCommandSpec, size: PtySize, wake: F) -> anyhow::Result<Pty>
    where F: Fn() + Send + 'static;

    /// Receiver of child output / exit events (single consumer).
    pub fn events(&self) -> Receiver<PtyEvent>;

    /// Write input bytes to the child.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()>;

    /// Resize the ConPTY.
    pub fn resize(&self, size: PtySize) -> anyhow::Result<()>;

    /// Kill the child (best effort).
    pub fn kill(&mut self) -> anyhow::Result<()>;
}
```

Notes: get the writer via `master.take_writer()` once and keep it behind a `Mutex` for
`write`. The reader thread owns the `Box<dyn Read>` from `master.try_clone_reader()`, loops
`read` into a buffer, sends `PtyEvent::Output`, calls `wake()`; on `Ok(0)`/error it waits on
the child, sends `PtyEvent::Exited(code)`, calls `wake()`, and stops.

---

## devterm-term  (`crates/devterm-term/src/`)

Wraps `alacritty_terminal`. Feeds PTY bytes through the VT parser and exposes an
RGB-resolved snapshot for the renderer. Single-threaded (lives on the app thread).

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape { Block, Underline, Beam, Hidden }

/// One rendered cell; fg/bg already have inverse/dim/bold-brighten applied.
#[derive(Clone, Copy, Debug)]
pub struct RenderCell {
    pub line: u16,   // 0 = top visible row
    pub col: u16,    // 0 = leftmost column
    pub c: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub wide: bool,  // glyph occupies two columns
}

#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub line: u16,
    pub col: u16,
    pub shape: CursorShape,
    pub color: Rgb,
}

/// Everything the renderer needs for one frame. Only non-blank cells are listed; the
/// renderer paints `default_bg` everywhere first.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<RenderCell>,
    pub cursor: Cursor,
    pub default_fg: Rgb,
    pub default_bg: Rgb,
    pub scrollback_offset: usize, // rows scrolled up from the bottom (0 = live)
    pub title: Option<String>,
}

pub struct Term { /* alacritty Term<Listener> + vte Processor + palette + shared state */ }

impl Term {
    /// New terminal of `cols` x `rows` with `scrollback_lines` of history.
    pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Self;

    /// Feed raw PTY output through the parser (updates the grid in place).
    pub fn advance(&mut self, bytes: &[u8]);

    /// Resize the grid (reflow handled by alacritty).
    pub fn resize(&mut self, cols: u16, rows: u16);

    /// Build the render snapshot from current grid state.
    pub fn snapshot(&self) -> Snapshot;

    /// Bytes the emulator wants written back to the child (DSR/DA replies, etc.).
    /// The app must forward these to `Pty::write`.
    pub fn drain_pty_writes(&mut self) -> Vec<u8>;

    /// Scroll the display: positive = up into history, negative = toward live. (M1)
    pub fn scroll_display(&mut self, delta_lines: i32);

    /// Whether the grid changed since the last `snapshot()` (redraw hint).
    pub fn dirty(&self) -> bool;
}
```

Implementation guidance (read the real source before writing):
- Construct with `alacritty_terminal::Term::new(config, &dimensions, listener)`. Implement a
  `Dimensions` type exposing `columns`, `screen_lines`, `total_lines` (= screen_lines +
  history). Config carries scrollback (`scrolling`) — verify field names in source.
- Parser: `alacritty_terminal::vte::ansi::Processor`; `processor.advance(&mut term, bytes)`.
- `Listener` implements `alacritty_terminal::event::EventListener`; on `Event::PtyWrite`
  push bytes into shared state, on `Event::Title(s)` store it. Keep shared state in
  `Arc<Mutex<..>>` (one clone in the listener, one in `Term`).
- Snapshot: use `term.renderable_content()` (`display_iter`, `cursor`, `display_offset`).
  Resolve each `Color` to `Rgb` via the palette: `Color::Spec(rgb)` direct;
  `Color::Named`/`Color::Indexed(i)` look up `renderable_content().colors[i]`, and when
  `None`, fall back to a **built-in xterm-256 default palette** you construct (16 ANSI +
  6×6×6 cube + 24 grays). Apply `Flags::INVERSE` by swapping fg/bg; `Flags::BOLD` may
  brighten ANSI 0–7 to 8–15; `Flags::DIM` darkens. Map wide/underline/strikeout flags.

---

## devterm-render  (`crates/devterm-render/src/`)

wgpu renderer. Owns instance/surface/device/queue, a swash glyph atlas, and the pipelines.
Draws the current snapshot every frame (vsync `PresentMode` = anti-flicker). Depends on
`winit` (for `Arc<Window>`), `devterm-core` (`Rect`), and `devterm-term` (`Snapshot`).

```rust
use std::sync::Arc;
use winit::window::Window;
use devterm_core::Rect;
use devterm_term::Snapshot;

#[derive(Clone, Copy, Debug)]
pub struct CellMetrics { pub width: f32, pub height: f32 } // physical px

pub struct PaneView<'a> {
    pub area: Rect,            // unit-square sub-rectangle for this pane
    pub snapshot: &'a Snapshot,
    pub focused: bool,
}

pub struct Renderer { /* wgpu state, atlas, pipelines */ }

impl Renderer {
    /// Bind a renderer to `window`. `font_size_px` is the cell font size in physical px at
    /// scale 1.0 (the app multiplies by scale factor). Blocks on device acquisition
    /// internally (pollster).
    pub fn new(window: Arc<Window>, font_size_px: f32) -> anyhow::Result<Renderer>;

    /// Surface + viewport resize (physical px).
    pub fn resize(&mut self, width_px: u32, height_px: u32);

    /// DPI scale change; rebuild glyph metrics/atlas as needed.
    pub fn set_scale_factor(&mut self, scale: f64);

    pub fn cell_metrics(&self) -> CellMetrics;

    /// Cols/rows that fit in the given physical pixel area at current metrics.
    pub fn grid_size_for(&self, width_px: u32, height_px: u32) -> (u16, u16);

    /// Render one frame of all panes. Returns wgpu surface errors so the app can
    /// reconfigure on `Lost`/`Outdated`.
    pub fn render(&mut self, panes: &[PaneView]) -> Result<(), wgpu::SurfaceError>;
}
```

Implementation guidance:
- Instance backends: prefer `Backends::DX12` on Windows (fall back to `PRIMARY`).
- Surface from `instance.create_surface(window.clone())` (Arc<Window> is 'static → safe).
- Pick an sRGB surface format from `surface.get_capabilities(&adapter).formats`; configure
  with `PresentMode::AutoVsync` (or `Fifo`), `CompositeAlphaMode::Opaque`.
- Font: use `fontdb` to load a system monospace (`Cascadia Mono` → `Consolas` →
  `JetBrains Mono` → any monospace). Get the face's raw font data + index for swash.
- Glyph atlas: an `R8Unorm` coverage texture. Rasterize glyphs on demand with swash
  (`ScaleContext` → `Render` with `Source::Outline`), keyed by `(char, bold, italic)`;
  store atlas uv + glyph metrics (left/top/advance) in a `HashMap`. Grow/repack simply
  (a shelf packer is fine).
- Two instanced pipelines sharing a viewport uniform:
  1. **background**: one colored quad per non-default-bg cell (+ selection later).
  2. **glyph**: one textured quad per cell glyph, sampling the atlas coverage as alpha,
     tinted by fg color.
  Plus a cursor quad (block, filled, XOR-ish or fg color under the char).
- Use `bytemuck` `Pod`/`Zeroable` for the instance structs. WGSL shaders inline as string
  literals. Keep the `wgpu` API calls verified against `wgpu-24.0.5` source (e.g.
  `request_device` arity, `Instance::new` signature, `RenderPassDescriptor` fields differ
  across versions).
- Map a pane's unit `Rect` × physical surface size → a pixel viewport; render that pane's
  grid inside it. For M0 a single full-window pane is enough, but keep it per-pane.

---

## devterm-config  (`crates/devterm-config/src/lib.rs`)

Minimal for M0; expands in M1. `serde` + `toml`.

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font_family: String,   // "" => renderer default
    pub font_size: f32,        // px at scale 1.0
    pub scrollback_lines: usize,
    pub shell_program: String, // "" => PtyCommandSpec::default_shell
    pub shell_args: Vec<String>,
}

impl Default for Config { /* font_size 15.0, scrollback 10_000, sensible defaults */ }

impl Config {
    /// Load from a TOML path; on missing file returns `Default`; on parse error returns
    /// the error.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Config>;

    /// The default config file path (`%APPDATA%\DevTerm\config.toml`).
    pub fn default_path() -> std::path::PathBuf;
}
```

---

## devterm-app  (`crates/devterm-app/src/`)

Binary `devterm`. Owns the winit event loop and wires pty ⇄ term ⇄ render. Split
`main.rs` into modules (`app.rs`, `keymap.rs`) as helpful.

M0 behaviour (single full-window pane):
- winit 0.30 `ApplicationHandler<UserEvent>`; create the `Window` in `resumed` as
  `Arc<Window>`.
- `UserEvent::Wake` is sent by the pty wake callback via `EventLoopProxy`; on it, call
  `window.request_redraw()`.
- On `resumed`: build `Renderer::new(window, font_px)`, compute `(cols, rows)` from
  `renderer.grid_size_for(inner_size)`, create `Term::new`, spawn `Pty` with the wake
  closure, size both to `(cols, rows)`.
- `WindowEvent::Resized` / `ScaleFactorChanged`: resize renderer, recompute cols/rows,
  `term.resize`, `pty.resize`, redraw.
- `WindowEvent::KeyboardInput` (Pressed only): translate to bytes via `keymap` and
  `pty.write`. Handle text via `event.text`, named keys (Enter→`\r`, Backspace→`\x7f`,
  Tab→`\t`, Esc→`\x1b`, arrows→`\x1b[A/B/C/D`, Home/End/PgUp/PgDn/Delete/Insert/F-keys→
  xterm sequences), and Ctrl+letter→control byte (`c & 0x1f`). Track modifiers via
  `ModifiersChanged`.
- `WindowEvent::RedrawRequested`: drain all pending `PtyEvent`s (`try_recv` loop) into
  `term.advance`; forward `term.drain_pty_writes()` to `pty.write`; take `term.snapshot()`;
  `renderer.render(&[PaneView{ area: Rect::UNIT, snapshot, focused: true }])`; on
  `SurfaceError::Lost/Outdated` reconfigure and retry.
- `MouseWheel`: `term.scroll_display` (M1-lite is fine to include).
- `main`: `env_logger::init()`, `EventLoop::<UserEvent>::with_user_event().build()?`,
  create proxy, `event_loop.run_app(&mut app)`.

M1 features are layered on top later (splits/tabs/focus, config load, keybindings,
selection/copy-paste); keep M0 code structured so a pane becomes `{ Pty, Term }` and the
window holds a `LayoutTree` from `devterm-core` (already implemented) — even if M0 uses a
single leaf.
