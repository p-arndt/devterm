//! Terminal emulation for DevTerm.
//!
//! Wraps `alacritty_terminal` (VT parser + grid + scrollback) — we do **not** write our
//! own emulator; that is the single biggest risk of the project (years of edge cases).
//! This crate owns:
//! - feeding PTY bytes into the emulator,
//! - exposing the grid + damage regions to the renderer,
//! - synchronized-output (DECSET 2026) frame boundaries,
//! - selection and scrollback search.

#![forbid(unsafe_code)]

// Scaffolding only — implementation lands in M0.
