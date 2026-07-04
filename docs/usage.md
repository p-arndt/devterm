# Using DevTerm

The practical, day-to-day guide. For the exact key list see
[keybindings.md](keybindings.md); for settings see [configuration.md](configuration.md).

## Panes and splits

DevTerm starts as one full-window pane running your shell. Split it to run several
programs side by side, each in its own real pseudo-terminal.

- **Split side by side** — `Ctrl+Shift+H`. The window divides left/right and the new
  pane opens on the right with focus.
- **Split stacked** — `Ctrl+Shift+S`. The window divides top/bottom, new pane below.
- Splits nest: split a pane again to build grids. Repeated splits in the *same*
  direction stay flat and evenly sized (like tmux), rather than nesting ever deeper.
- **Close** the focused pane with `Ctrl+Shift+W`. Its shell is terminated and the
  remaining panes expand to reclaim the space. Closing the **last** pane quits DevTerm.
- When a pane's shell exits on its own (e.g. you type `exit`), that pane closes
  automatically; the app only quits when the final pane is gone.

## Moving focus

The **focused** pane is outlined and receives your keystrokes.

- **Keyboard:** `Ctrl+Shift+←/→/↑/↓` moves focus to the neighboring pane in that
  direction (chosen by geometry, so it does the intuitive thing in a grid).
- **Mouse:** click any pane to focus it.

> `Ctrl+Alt+arrows` would be the obvious focus chord, but GNOME/Ubuntu reserves it for
> switching workspaces — hence `Ctrl+Shift+arrows`.

## Resizing

`Alt+Shift+arrow` moves the focused pane's **border in the arrow's direction** (about 10%
per press). If a pane borders it on that side, the focused pane **grows** into it; if the
arrow points at the window edge, it **shrinks**. So on the right pane of a split, `←` grows
it leftward and `→` shrinks it back — the border tracks the key, and the two arrows on an
axis are exact inverses.

Resize adjusts the nearest split along the pressed axis; a pane with no split on that axis
doesn't move. (The middle pane of a flat three-way split has a neighbor on both sides, so
both arrows grow it — shrink it by growing one of its siblings instead.)

You can also **drag a split border with the mouse**: hover a divider (the cursor turns
into a resize arrow) and drag to move it.

## Scrollback

Each pane keeps history (10,000 lines by default; set `scrollback_lines` in the config).

- **Wheel** up/down scrolls the pane **under the pointer**.
- **Keyboard:** `Ctrl+Shift+K` / `Ctrl+Shift+J` scroll a line at a time;
  `Shift+PageUp` / `Shift+PageDown` scroll by pages.
- New output from the shell snaps you back to the live view.

## Selecting, copying, pasting

- **Select:** press and drag with the left mouse button. The selection highlights as
  inverse video and stays selected after you release.
- **Copy:** `Ctrl+Shift+C` puts the selection on the system clipboard.
- **Paste:** `Ctrl+Shift+V` inserts clipboard text. When the running program has
  enabled *bracketed paste* (most modern shells and editors do), DevTerm wraps the text
  so the program treats it as pasted input rather than typed commands — this prevents a
  multi-line paste from auto-executing line by line.

Clipboard access uses the OS clipboard on every platform (Windows clipboard; X11 or
Wayland on Linux). On Linux this needs a running display server — under a bare TTY or
headless session there's no clipboard and copy/paste is a no-op (logged as a warning).

## Choosing your shell

By default DevTerm launches PowerShell 7 (falling back to Windows PowerShell) on
Windows, or your `$SHELL` on Linux/macOS.

- **Windows:** set `shell` to a named preset — `pwsh`, `windows-power-shell`, `cmd`,
  `git-bash`, or `wsl`.
- **Linux/macOS:** the named presets are Windows-only; use `shell = "auto"` to follow
  `$SHELL`, or point `shell_program` at any executable (e.g. `/usr/bin/fish`).

See [configuration.md → Shells](configuration.md#shells).

## Making it yours

Everything below is live-reloaded — edit your `config.toml`
([where is it?](configuration.md#where-the-config-lives)), save, and the running window
updates:

- **Colors:** `theme_name` (`default` / `gruvbox-dark`) and/or the `[theme]` table
  (16 ANSI colors + fg/bg/cursor as `#rrggbb`).
- **Font:** `font_family`, `font_size`, and `line_height`.
- **Cursor:** the `[cursor]` table — `shape` and `blink`.
- **Keys:** switch `keymap_preset` to `"tmux"`, or remap individual actions in
  `[keybindings]`.

See [configuration.md](configuration.md) for the full reference and
[keybindings.md](keybindings.md) for chord syntax and action names.

## Anti-flicker

DevTerm decouples reading shell output from drawing, and honors the *synchronized
output* protocol (DECSET 2026) that modern TUIs (neovim, recent Ink-based CLIs like
Claude Code) use to mark frame boundaries — so full-screen apps repaint cleanly instead
of tearing mid-frame. You don't need to configure anything; it just behaves.
