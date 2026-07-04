# DevTerm — M1 execution plan (v0.1 "daily-usable")

Prereq: M0 walking skeleton compiles and runs (single PowerShell pane rendered, typeable,
resizable, no flicker). M1 turns that into a terminal you can use as your daily driver.

M1 feature list (from PLAN.md §M1) mapped to crates and parallel workstreams. The `app`
crate is the big integrator and mostly follows the others, exactly like M0.

## Workstreams (A–D run in parallel; APP integrates after)

### Stream A — `devterm-term` (emulation depth)
- **Selection model**: wrap alacritty's `Selection`; expose
  `start_selection(point, mode)`, `update_selection(point)`, `selection_text() -> Option<String>`,
  and include selected ranges in `Snapshot` (so render can highlight). Modes: char / word / line.
- **Scrollback**: already have `scroll_display`; add `scroll_to_bottom`, clamp, and reflect
  `scrollback_offset` in the snapshot (done in M0 contract).
- **Damage + DECSET 2026 (synchronized output)**: expose real damage bounds; gate snapshot
  "settled" state between BSU/ESU (mode 2026). Verify whether alacritty_terminal 0.25 already
  tracks mode 2026 (`private_mode`/`Mode`); if not, intercept in the parser wrapper. This is
  the anti-flicker completion — pair with a golden-stream + random-chunk regression test.
- **Wide chars / graphemes hardening**: ensure `wide` + spacer cells map correctly.

### Stream B — `devterm-config` (config + keymaps)
- Expand `Config`: `theme` (named + explicit 16-color palette + fg/bg/cursor), `font` block
  (family, size, line-height), `scrollback_lines`, `shells` list, `cursor` (shape, blink).
- **Keymap schema**: parse bindings like `"ctrl+shift+d" -> SplitHorizontal`; an `Action`
  enum (Split{dir}, ClosePane, FocusMove{dir}, ResizePane{dir}, Copy, Paste, ScrollUp/Down,
  ScrollPageUp/Down, NewTab, NextTab, etc.). Ship a **default keymap** + a **tmux preset**
  (prefix `ctrl+b` then key) selectable via config.
- `load` + validation with helpful errors; keep `default_path()`.

### Stream C — `devterm-pty` (shells + lifecycle)
- **Shell presets**: `PtyCommandSpec::{powershell7, windows_powershell, cmd, git_bash, wsl}`
  with PATH/þinstall detection; keep `default_shell`.
- **Lifecycle**: expose exit code on `Exited`; support restart (spawn a fresh child reusing
  size); make `kill` robust.

### Stream D — `devterm-render` (multi-pane visuals + fonts)
- **Multi-pane**: draw all panes from `&[PaneView]`; add split gutters/borders and a focus
  highlight on the focused pane.
- **Selection highlight** + **cursor shapes** (block/underline/beam, hollow when unfocused).
- **Font fallback chain**: fontdb fallback for glyphs missing in the primary face — Nerd Font
  symbols, emoji (color? monochrome ok for M1), CJK. Per-glyph face selection in the atlas key.
- **DPI**: solidify `set_scale_factor` rebuild (atlas + metrics).

## APP integration (`devterm-app`) — after A–D
- **Pane manager**: replace the single pane with `LayoutTree` (devterm-core, done) + a
  `HashMap<PaneId, Pane { pty: Pty, term: Term }>`. Route pty events per pane; redraw only
  dirty panes (damage).
- **Keybinding dispatch**: map `KeyEvent` → `Action` via the config keymap; execute:
  split H/V (spawn new pane+pty, update tree), close pane (kill pty, update tree), focus
  move (tree `move_focus`), resize (tree `resize`).
- **Mouse**: click → focus pane (hit-test via `tree.compute`); drag on a gutter → resize;
  drag inside a pane → selection; wheel → scroll that pane.
- **Clipboard**: copy selection / paste with **bracketed paste**. Add a clipboard dep
  (`arboard`) — evaluate at M1 start.
- **Config hot-reload**: watch the config file (add `notify`) on a thread → reload → apply
  (font size, theme, keymap, scrollback) live.
- **Anti-flicker**: honor DECSET 2026 + damage — only present a frame when the model is
  "settled"; coalesce byte bursts (~1–2 ms) before marking settled.

## New dependencies to add at M1 start (declare centrally, activate per crate)
- `arboard` (clipboard) — app
- `notify` (config file watching) — app
- confirm `unicode-width` usage in term/render for wide cells (already added in M0)

## Suggested M1 workflow shape
1. Parallel: Stream A (term), B (config), C (pty), D (render) — each builds `-p` clean.
2. APP integration agent — wires pane manager, keymap dispatch, mouse, clipboard, hot-reload.
3. Integrate: `cargo build --all`, `test --all`, `clippy -D warnings`, `fmt`; add a
   golden-stream + random-chunk anti-flicker regression test (PLAN.md §4).
4. Acceptance (manual, user): use DevTerm a week as the main terminal.
