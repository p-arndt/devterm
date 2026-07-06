# Keybindings

DevTerm ships two built-in keymaps — the **default** map and a **tmux-flavored**
preset — and lets you override any binding in `config.toml`. Keys that are *not* bound
to a DevTerm action are passed straight through to the shell in the focused pane
(so `Ctrl+C`, arrows, `Tab`, function keys, etc. reach your programs unchanged).

## Default keymap

| Shortcut | Action | Notes |
|---|---|---|
| `Ctrl+Shift+H` | Split horizontal | New pane appears **side by side** (to the right); focus moves to it. |
| `Ctrl+Shift+S` | Split vertical | New pane appears **stacked** (below); focus moves to it. |
| `Ctrl+Shift+W` | Close pane | Closing a tab's **last** pane closes the tab; closing the last tab quits DevTerm. |
| `Ctrl+Shift+N` | New tab | Opens a fresh shell in a new tab and switches to it. |
| `Ctrl+Shift+X` | Close tab | Drops all of the tab's panes; closing the **last** tab quits DevTerm. |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab | Cycles through tabs, wrapping at the ends. |
| `Ctrl+Shift+←/→/↑/↓` | Move focus | Focus the geometric neighbor in that direction. |
| `Alt+Shift+←/→/↑/↓` | Resize focused pane | Moves its border toward the arrow: grow into a neighbor, shrink at the window edge (~10% per press). |
| `Ctrl+Shift+C` | Copy | Copies the current selection to the system clipboard. |
| `Ctrl+Shift+V` | Paste | Pastes clipboard text (bracketed paste when the app supports it). |
| `Ctrl+Shift+K` | Scroll line up | Into scrollback history. |
| `Ctrl+Shift+J` | Scroll line down | Toward the live prompt. |
| `Shift+PageUp` | Scroll page up | One screenful, minus a row. |
| `Shift+PageDown` | Scroll page down | |
| `Ctrl+,` | Open settings | Opens the inline settings overlay (arrow-key navigable). |
| `Ctrl+Shift+,` | Open config | Opens `config.toml` in your editor in a new stacked pane. |
| `Ctrl+Shift+T` | Toggle floating terminal | Shows/hides a centered "scratch" terminal floating over the layout for quick one-off commands. |
| `Ctrl+Shift+Q` | Quit | Closes DevTerm. |

> **Resize follows the arrow.** `Alt+Shift+arrow` slides the focused pane's border in the
> pressed direction: it **grows** the pane when a neighbor sits on that side and **shrinks**
> it when the arrow points at the window edge. On the right pane of a split, `←` grows it
> leftward and `→` shrinks it back — the opposite arrows on an axis are exact inverses.
> Resize acts on the nearest split along that axis; a pane with no split on the axis (e.g. a
> lone pane, or a horizontal-only layout resized vertically) doesn't move. The middle pane
> of a flat three-way split borders panes on both sides, so both arrows grow it — shrink it
> by growing a sibling instead.
>
> **Note (GNOME/Ubuntu):** `Ctrl+Alt+←/→/↑/↓` is reserved by the desktop for switching
> workspaces, which is why focus uses `Ctrl+Shift+arrows` and resize uses
> `Alt+Shift+arrows` instead.

## tmux preset

A prefix-free approximation of tmux, using tmux's letters as **direct** chords. Real
tmux uses a `Ctrl-b` prefix mode (press prefix, release, then the command key); that
needs app-side state and is a later refinement, so this preset binds the commands
directly instead.

Enable it in `config.toml`:

```toml
keymap_preset = "tmux"
```

| Shortcut | Action |
|---|---|
| `Ctrl+Alt+%` | Split horizontal (side by side) |
| `Ctrl+Alt+"` | Split vertical (stacked) |
| `Ctrl+Alt+X` | Close pane |
| `Ctrl+Alt+N` | New tab (tmux `c` is taken by Copy here) |
| `Ctrl+Alt+&` | Close tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+Shift+←/→/↑/↓` | Move focus |
| `Alt+Shift+←/→/↑/↓` | Resize focused pane (border follows the arrow) |
| `Ctrl+Alt+C` / `Ctrl+Alt+V` | Copy / Paste |
| `Ctrl+Alt+K` / `Ctrl+Alt+J` | Scroll line up / down |
| `Ctrl+Alt+PageUp` / `Ctrl+Alt+PageDown` | Scroll page up / down |
| `Ctrl+Alt+,` | Open settings |
| `Ctrl+Alt+Shift+,` | Open config |
| `Ctrl+Alt+T` | Toggle floating terminal |
| `Ctrl+Alt+Q` | Quit |

## Mouse

| Gesture | Effect |
|---|---|
| **Left click on the tab bar** | Switch to the clicked tab; the `+` button opens a new one. |
| **Left click** | Focus the pane under the pointer. |
| **Left click + drag** (inside a pane) | Select text in that pane (highlighted as inverse video). |
| **Drag a split border** | Resize the panes on either side (the cursor turns into a resize arrow on hover). |
| **Wheel up / down** | Scroll the pane **under the pointer** through its scrollback. |

Selection stays put after you release, so you can then press `Ctrl+Shift+C` to copy it.
There is no copy-on-select or middle-click-paste yet.

## Customizing bindings

Every action can be rebound in the `[keybindings]` table of `config.toml`. The table
maps a **chord string** to an **action name**; entries are applied *on top of* the
active preset, so you only list what you want to change.

```toml
keymap_preset = "default"

[keybindings]
"ctrl+shift+d"   = "split-horizontal"   # add an extra split key
"ctrl+shift+g"   = "split-vertical"
"alt+w"          = "close-pane"          # rebind close
```

Changes take effect **live** — save the file and DevTerm reloads it (see
[configuration.md](configuration.md#hot-reload)).

### Chord syntax

`modifier+modifier+…+key`, case-insensitive, whitespace around `+` is ignored.

- **Modifiers:** `ctrl` (`control`), `alt` (`option`), `shift`, `logo`
  (`super` / `cmd` / `win` / `windows`).
- **Key:** a single character (`a`, `7`, `%`) **or** a named key:
  `enter` (`return`), `tab`, `escape` (`esc`), `space`, `backspace`,
  `delete` (`del`unsafe), `insert` (`ins`), `home`, `end`, `pageup` (`pgup`),
  `pagedown` (`pgdn`), `up`, `down`, `left`, `right`, `f1`…`f12`.

Examples: `ctrl+shift+h`, `alt+left`, `ctrl+shift+pageup`, `shift+f5`.

### Action names

Use these exact strings as the value in `[keybindings]`:

`split-horizontal`, `split-vertical`, `close-pane`,
`new-tab`, `close-tab`, `next-tab`, `prev-tab`,
`focus-left`, `focus-right`, `focus-up`, `focus-down`,
`resize-left`, `resize-right`, `resize-up`, `resize-down`,
`copy`, `paste`,
`scroll-line-up`, `scroll-line-down`, `scroll-page-up`, `scroll-page-down`,
`open-settings`, `open-config`, `toggle-floating-terminal`, `quit`.

Tabs (`new-tab`, `close-tab`, `next-tab`, `prev-tab`) each hold their own pane layout;
the always-visible bar across the top of the window shows each tab's number plus the
title its focused pane reports (else "Tab N"), clickable to switch, with a trailing `+`
button that opens a new tab. Background tabs keep their shells running. Tab actions are
ignored while the floating terminal is up.

The floating terminal (`toggle-floating-terminal`) is a centered "scratch" terminal that
floats over the layout for quick one-off commands. The first toggle spawns it (running the
configured shell); later toggles hide/show it while keeping the process and its scrollback
alive. While it is shown it captures typing, copy/paste and scrolling; `close-pane`
dismisses it (killing its child), and typing `exit` closes it too. Layout actions
(split/focus/resize) are ignored while it is up.

The settings overlay (`open-settings`, `Ctrl+,`) is a centered panel with two pages,
switched with `Tab`:

- **General** — the common options (font, theme, shell, cursor, …). Navigate rows with
  `Up`/`Down`, change the selected value with `Left`/`Right`, and press `Enter` to edit a
  text field (font family) or cycle other fields.
- **Keybindings** — one row per action showing its current chord. Select an action with
  `Up`/`Down`, press `Enter`, then press the new key combo to rebind it. A combo already
  used by another action is rejected (shown in the footer) rather than silently stolen.
  Rebinding *moves* the binding — the action's old chord stops working.

`Esc` closes the overlay, writing any changes back to `config.toml` (which then
hot-reloads). `Ctrl+E` closes it and opens the raw file in your editor instead.

The editor used by `open-config` is `$VISUAL`, then `$EDITOR`, falling back to `vi`
(`notepad` on Windows). It opens `config.toml` in a new pane, so a terminal editor
works out of the box.

Bindings that fail to parse (unknown key or action) are silently ignored, so a typo
disables just that one line rather than breaking the whole config.
