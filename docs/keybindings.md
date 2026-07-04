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
| `Ctrl+Shift+W` | Close pane | Closing the **last** pane quits DevTerm. |
| `Ctrl+Alt+←/→/↑/↓` | Move focus | Focus the geometric neighbor in that direction. |
| `Ctrl+Shift+←/→` | Widen focused pane | Grows the pane horizontally (~10% per press). |
| `Ctrl+Shift+↑/↓` | Heighten focused pane | Grows the pane vertically (~10% per press). |
| `Ctrl+Shift+C` | Copy | Copies the current selection to the system clipboard. |
| `Ctrl+Shift+V` | Paste | Pastes clipboard text (bracketed paste when the app supports it). |
| `Ctrl+Shift+K` | Scroll line up | Into scrollback history. |
| `Ctrl+Shift+J` | Scroll line down | Toward the live prompt. |
| `Shift+PageUp` | Scroll page up | One screenful, minus a row. |
| `Shift+PageDown` | Scroll page down | |
| `Ctrl+Shift+Q` | Quit | Closes DevTerm. |

> **Resize is grow-only right now.** Both `Ctrl+Shift+←` and `Ctrl+Shift+→` widen the
> focused pane (the arrow selects the *axis*, not the side), and `↑`/`↓` both make it
> taller. A dedicated shrink binding isn't wired yet — closing/reopening a split resets
> proportions. This is tracked as an M1 follow-up.

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
| `Ctrl+Alt+←/→/↑/↓` | Move focus |
| `Ctrl+Shift+←/→/↑/↓` | Resize focused pane |
| `Ctrl+Alt+C` / `Ctrl+Alt+V` | Copy / Paste |
| `Ctrl+Alt+K` / `Ctrl+Alt+J` | Scroll line up / down |
| `Ctrl+Alt+PageUp` / `Ctrl+Alt+PageDown` | Scroll page up / down |
| `Ctrl+Alt+Q` | Quit |

## Mouse

| Gesture | Effect |
|---|---|
| **Left click** | Focus the pane under the pointer. |
| **Left click + drag** | Select text in that pane (highlighted as inverse video). |
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
"ctrl+shift+t"   = "split-vertical"
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
  `delete` (`del`), `insert` (`ins`), `home`, `end`, `pageup` (`pgup`),
  `pagedown` (`pgdn`), `up`, `down`, `left`, `right`, `f1`…`f12`.

Examples: `ctrl+shift+h`, `alt+left`, `ctrl+shift+pageup`, `shift+f5`.

### Action names

Use these exact strings as the value in `[keybindings]`:

`split-horizontal`, `split-vertical`, `close-pane`,
`focus-left`, `focus-right`, `focus-up`, `focus-down`,
`resize-left`, `resize-right`, `resize-up`, `resize-down`,
`copy`, `paste`,
`scroll-line-up`, `scroll-line-down`, `scroll-page-up`, `scroll-page-down`,
`quit`.

Bindings that fail to parse (unknown key or action) are silently ignored, so a typo
disables just that one line rather than breaking the whole config.
