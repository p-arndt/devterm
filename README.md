# DevTerm

A modern, plugin-friendly, project-oriented dev terminal for Windows-first — a real
terminal application (own window, own GPU renderer, one ConPTY per pane), not a
multiplexer running inside another terminal. See [`PLAN.md`](./PLAN.md) for the full
architecture and roadmap.

## Status

Early scaffolding. `devterm-core` (the pure layout-tree domain) is implemented and tested;
the rest of the crates are stubs to be filled in during **M0** (the walking skeleton).

## Workspace layout

| Crate | Responsibility | Milestone |
|---|---|---|
| `devterm-core` | Domain model: panes, layout tree, focus. **No I/O.** | done (grows) |
| `devterm-pty` | PTY/ConPTY: spawn, reader/writer threads, lifecycle | M0 |
| `devterm-term` | Terminal emulation (wraps `alacritty_terminal`) | M0 |
| `devterm-render` | wgpu renderer: cell grid, glyph atlas, splits | M0 |
| `devterm-config` | Config, keymaps, themes, project layouts | M1 |
| `devterm-app` | Binary `devterm`: window, input, chrome, IPC | M0 |
| `devterm-cli` | Binary `dt`: control a running instance | M2 |
| `devterm-plugin` | WASM plugin host (`wasmtime`) | M3 |

Dependency direction is strict: `app -> {render, term, pty, config, plugin} -> core`.

## Build & test

```sh
cargo test --all --all-features   # runs unit + property tests
cargo run -p devterm-app          # placeholder until M0
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
```

The toolchain is pinned in `rust-toolchain.toml` (Rust 1.96.0). CI (Windows) runs fmt,
clippy, tests, and `cargo-deny`.

## License

Dual-licensed under MIT or Apache-2.0.
