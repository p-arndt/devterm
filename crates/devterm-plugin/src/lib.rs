//! Plugin host for DevTerm (WASM via `wasmtime`).
//!
//! Deliberately deferred to **M3**: the plugin ABI is frozen only after the internal
//! event/command API has been stable across two releases. Plugins declare permissions in
//! a manifest and hook into: status-bar modules, palette commands, pane-output listeners,
//! and pane decorations.

#![forbid(unsafe_code)]

// Scaffolding only — implementation lands in M3.
