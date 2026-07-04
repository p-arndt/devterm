//! Configuration for DevTerm.
//!
//! Owns `config.toml` (font, size, theme, shell, scrollback) with hot-reload, the
//! keybinding schema (default keymap + tmux preset), themes, and project layout files
//! (`devterm.yml`, M2). Pure schema + validation; the file watcher lives in the app.

#![forbid(unsafe_code)]

// Scaffolding only — implementation lands in M1.
