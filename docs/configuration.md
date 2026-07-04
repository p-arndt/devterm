# Configuration

DevTerm reads a single [TOML](https://toml.io) file. If the file is **missing**, DevTerm
uses built-in defaults — you don't need a config file to start. A **partial** file is
fine too: every setting has a default, so you only write the keys you want to change. On
a parse error, DevTerm keeps running with the last good values and logs the problem.

## Where the config lives

| Platform | Path |
|---|---|
| **Windows** | `%APPDATA%\DevTerm\config.toml` (e.g. `C:\Users\you\AppData\Roaming\DevTerm\config.toml`) |
| **Linux / macOS** | `$APPDATA/DevTerm/config.toml` if `APPDATA` is set, otherwise `./DevTerm/config.toml` relative to the launch directory |

> **Heads-up for Linux/macOS:** the path is currently derived from the `APPDATA`
> environment variable (a Windows-ism), so on a Unix box without `APPDATA` it resolves
> to a **relative** `DevTerm/config.toml`. Until a native `~/.config/devterm/` location
> is wired up, the clean way to get a stable config on Linux is to point `APPDATA` at
> your config home:
>
> ```sh
> # add to ~/.profile, ~/.bashrc, or ~/.zshrc
> export APPDATA="$HOME/.config"
> mkdir -p ~/.config/DevTerm
> $EDITOR ~/.config/DevTerm/config.toml
> ```
>
> DevTerm (launched from that shell) then reads and hot-reloads
> `~/.config/DevTerm/config.toml`.

## Full example

This is the default configuration written out in full. Copy it, change what you like,
delete the rest.

```toml
# --- general ---
font_family     = ""        # preferred font — see "Known limitations" below
font_size       = 15.0      # cell font size in px at 100% scale
scrollback_lines = 10000    # lines of history kept per pane

# --- shell ---
shell           = "auto"    # auto | pwsh | windows-power-shell | cmd | git-bash | wsl
shell_program   = ""        # explicit executable; overrides `shell` when non-empty
shell_args      = []        # extra args passed to the shell

# --- input ---
keymap_preset   = "default" # default | tmux  (see keybindings.md)

# NOTE: table-valued sections must come AFTER all the scalar keys above.
# TOML rule: once a [table] header appears, later bare keys belong to that table.

# --- colors (all values are "#rrggbb") ---
[theme]
ansi       = ["#000000", "#800000", "#008000", "#808000",
              "#000080", "#800080", "#008080", "#c0c0c0",
              "#808080", "#ff0000", "#00ff00", "#ffff00",
              "#0000ff", "#ff00ff", "#00ffff", "#ffffff"]
foreground = "#d0d0d0"
background = "#000000"
cursor     = "#d0d0d0"

# --- keybinding overrides (chord = action) ---
[keybindings]
# "ctrl+shift+d" = "split-horizontal"
```

## Settings reference

| Key | Type | Default | Meaning |
|---|---|---|---|
| `font_family` | string | `""` | Preferred font family. **Not yet wired** — see limitations. |
| `font_size` | float | `15.0` | Cell font size in px at 100% scale. Hot-reloadable. |
| `scrollback_lines` | int | `10000` | History lines kept per pane. Applies to newly created panes. |
| `shell` | enum | `"auto"` | Friendly shell preset (see below). |
| `shell_program` | string | `""` | Explicit shell executable; wins over `shell` when set. |
| `shell_args` | list | `[]` | Extra arguments passed to the shell. |
| `keymap_preset` | enum | `"default"` | Base keymap: `default` or `tmux`. |
| `[theme]` | table | xterm palette | 16 ANSI colors + foreground/background/cursor. |
| `[keybindings]` | table | `{}` | Chord → action overrides, applied on top of the preset. |

## Shells

`shell` picks a shell without you spelling out the full path:

| `shell` value | Launches |
|---|---|
| `"auto"` | DevTerm's own default: PowerShell 7 → Windows PowerShell on Windows; `$SHELL` → bash/zsh/sh on Unix. |
| `"pwsh"` | `pwsh.exe` (PowerShell 7 / Core) |
| `"windows-power-shell"` | `powershell.exe` (Windows PowerShell 5.x) |
| `"cmd"` | `cmd.exe` |
| `"git-bash"` | `bash.exe -i` (Git Bash) |
| `"wsl"` | `wsl.exe` |

**Resolution order:** a non-empty `shell_program` always wins (used with `shell_args`).
Otherwise `shell` is mapped to the program above, with any preset args (e.g. Git Bash's
`-i`) placed **before** your `shell_args`. `"auto"` leaves the choice to the app.

> **The named presets (`pwsh`, `cmd`, `git-bash`, `wsl`, …) are Windows-oriented** —
> they resolve to `.exe` names that only exist on Windows. **On Linux/macOS**, use
> `shell = "auto"` (which launches `$SHELL`, falling back to bash/zsh/sh) or set
> `shell_program` to an explicit path.

```toml
# Windows: use WSL with a login shell
shell      = "wsl"
shell_args = ["--", "bash", "-l"]

# Windows: a custom shell by full path
shell_program = "C:\\tools\\nu\\nu.exe"

# Linux/macOS: pick a specific shell
shell_program = "/usr/bin/fish"
shell_args    = ["-l"]

# Linux/macOS: or just follow $SHELL
shell = "auto"
```

## Themes

The `[theme]` table sets the 16 ANSI palette entries plus the default foreground,
background, and cursor colors. All values are `"#rrggbb"` hex strings. A partial table
merges onto the default palette, so you can override just a couple of slots:

```toml
[theme]
background = "#1d2021"   # only change the background
foreground = "#ebdbb2"
```

> **Named theme presets:** a `gruvbox-dark` palette exists internally, but selecting a
> theme *by name* from `config.toml` (e.g. `theme = "gruvbox-dark"`) isn't wired yet —
> for now, set colors via the `[theme]` table. Named-preset selection is a planned
> convenience.

## Fonts and the fallback chain

DevTerm auto-picks a system **monospace** font for the grid, then consults a **fallback
chain** for glyphs the primary font lacks — CJK, box-drawing, and Nerd Font symbols — so
you get real glyphs instead of tofu boxes (`□`).

**Windows** ships everything it needs (Cascadia Mono, plus CJK and symbol coverage), so
there's nothing to install.

**Linux** only has whatever fonts you've installed. For full coverage, add a CJK font,
an emoji font, and (optionally) a Nerd Font for powerline/dev-tool symbols:

```sh
# Debian / Ubuntu (apt)
sudo apt install fonts-noto-cjk fonts-noto-color-emoji

# Fedora (dnf)
sudo dnf install google-noto-sans-cjk-fonts google-noto-color-emoji-fonts

# Arch (pacman)
sudo pacman -S noto-fonts-cjk noto-fonts-emoji
```

Nerd Font symbols aren't packaged consistently across distros — grab a
[Nerd Font](https://www.nerdfonts.com/) (e.g. *Symbols Nerd Font Mono*) and drop it in
`~/.local/share/fonts/`, then run `fc-cache -f`. See the emoji caveat under
[Known limitations](#known-limitations).

## Hot-reload

DevTerm watches `config.toml` and applies changes **without a restart**. Save the file
and:

- **theme** — repainted immediately on every pane,
- **keybindings / keymap preset** — the keymap is rebuilt,
- **font size** — the grid re-rasterizes and panes reflow,
- **shell** — takes effect on the **next** pane you open (existing panes keep running).

## Known limitations

- **`font_family` is not honored yet.** The renderer currently auto-picks a system
  monospace font (and a CJK / Nerd-symbol fallback chain); it does not yet read
  `font_family`. Setting it has no effect. Wiring it through is a follow-up.
- **Color emoji render monochrome.** The glyph atlas is a coverage (alpha) texture, so
  emoji show as silhouettes. Full-color emoji need an RGBA atlas (planned for a later
  milestone). CJK and Nerd Font / box-drawing glyphs render correctly.
- **No native Linux/macOS config path yet.** The config location is derived from
  `APPDATA` (see [Where the config lives](#where-the-config-lives) for the
  `export APPDATA=...` workaround); a proper `~/.config/devterm/` path is planned. When
  the config directory doesn't exist, hot-reload disables itself gracefully.
