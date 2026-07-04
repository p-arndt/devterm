# DevTerm Documentation

A GPU-rendered, multi-pane developer terminal for **Windows and Linux** (Windows is the
primary target; Linux is fully supported). DevTerm is a real terminal *application* — its
own window, its own renderer, one PTY per pane (ConPTY on Windows, a native PTY on
Linux) — not a multiplexer running inside another terminal.

> **Status:** Milestone **M1** ("daily-usable") is implemented — splits, focus
> navigation, keyboard resize, scrollback, mouse selection + clipboard, font fallback,
> configurable keybindings, themes, and `config.toml` hot-reload. See
> [`../PLAN.md`](../PLAN.md) for the full roadmap and what is still open.

## Documentation index

| Doc | What's in it |
|---|---|
| [**usage.md**](usage.md) | Day-to-day: panes, splits, focus, scrolling, selection, copy/paste — the practical guide. |
| [**keybindings.md**](keybindings.md) | Every default shortcut, the tmux preset, chord syntax, and how to remap. |
| [**configuration.md**](configuration.md) | The complete `config.toml` reference with an annotated example, shells, themes, and hot-reload. |

## Build & run

DevTerm is a Cargo workspace. Everywhere you need a recent Rust toolchain
(edition 2024, rustc ≥ 1.96) and a working GPU:

- **Windows** → Direct3D 12 (present on Windows 10/11 out of the box).
- **Linux** → Vulkan (install your GPU's Vulkan driver — see prerequisites below).

```sh
# from the repository root — same on every platform
cargo run -p devterm-app            # debug build, launches the terminal
cargo run --release -p devterm-app  # optimized build (noticeably smoother)
```

The compiled binary is named `devterm` (`target/release/devterm`, or
`devterm.exe` on Windows). On first launch, with no config file present, DevTerm starts
with sensible defaults (see [configuration.md](configuration.md)).

### Linux prerequisites

`winit` (windowing), `wgpu` (Vulkan), and `arboard` (clipboard) need a few system
libraries and a Vulkan driver. Pick your distro:

**Debian / Ubuntu / Mint / Pop!\_OS (apt)**

```sh
sudo apt install build-essential pkg-config \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libxcb1-dev \
  libxkbcommon-dev libwayland-dev \
  mesa-vulkan-drivers vulkan-tools
```

**Fedora / RHEL / Nobara (dnf)**

```sh
sudo dnf install gcc pkgconf-pkg-config \
  libX11-devel libXcursor-devel libXrandr-devel libXi-devel libxcb-devel \
  libxkbcommon-devel wayland-devel \
  vulkan-loader vulkan-tools mesa-vulkan-drivers
```

**Arch / Manjaro / EndeavourOS (pacman)**

```sh
sudo pacman -S --needed base-devel pkgconf \
  libx11 libxcursor libxrandr libxi libxcb \
  libxkbcommon wayland \
  vulkan-icd-loader vulkan-tools
# plus the Vulkan driver for your GPU:
#   Intel: vulkan-intel   AMD: vulkan-radeon   NVIDIA: nvidia-utils
```

Verify Vulkan can see your GPU with `vulkaninfo --summary` (from `vulkan-tools`). Both
X11 and Wayland sessions work. For the CJK / emoji / Nerd-symbol glyph fallback, also
install the fonts listed in
[configuration.md → Fonts](configuration.md#fonts-and-the-fallback-chain).

### Windows prerequisites

Just the Rust toolchain (MSVC) — Direct3D 12 and the default fonts (Cascadia Mono,
Segoe UI Emoji) ship with the OS. Nothing else to install.

### Checks

```sh
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## A 30-second tour

1. Launch it — you get one full-window pane running your default shell.
2. `Ctrl+Shift+H` splits it **side by side**; `Ctrl+Shift+S` splits it **stacked**.
   Focus moves to the new pane.
3. `Ctrl+Shift+<arrow>` moves focus between panes; the focused pane is outlined.
   `Alt+Shift+<arrow>` slides its border that way (grow into a neighbor, shrink at the edge).
4. Click a pane to focus it; drag to select text; `Ctrl+Shift+C` copies,
   `Ctrl+Shift+V` pastes.
5. Scroll the wheel (or `Shift+PageUp`/`PageDown`) to move through scrollback.
6. `Ctrl+Shift+W` closes the focused pane. Closing the last pane quits DevTerm.

Full shortcut list: [keybindings.md](keybindings.md).

## How it fits together

DevTerm is split into crates with a strict one-way dependency direction
(`app → {render, term, pty, config} → core`):

| Crate | Role |
|---|---|
| `devterm-core` | Pure layout tree (splits, focus, resize), geometry, IDs. No I/O. |
| `devterm-pty` | Spawns the shell on a pseudo-terminal (ConPTY on Windows); reader/writer threads. |
| `devterm-term` | Wraps `alacritty_terminal` (VT parsing, grid, scrollback); produces render snapshots. |
| `devterm-render` | `wgpu` renderer: cell grid, glyph atlas, cursor, pane borders, font fallback. |
| `devterm-config` | `config.toml` schema, keymaps, themes, shell presets. |
| `devterm-app` | The window/event loop that wires it all together. |

There is also a `devterm-cli` crate (the `dt` command) and a `devterm-plugin` crate —
both are scaffolding for later milestones (M2/M3) and not functional yet.
