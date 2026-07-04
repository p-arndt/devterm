//! PTY layer for DevTerm (Windows: ConPTY via `portable-pty`).
//!
//! Responsibilities (filled in during **M0**):
//! - spawn a shell process on a pseudo-terminal,
//! - a reader thread that streams output bytes to the terminal model,
//! - a writer that forwards keyboard input,
//! - process lifecycle: resize, exit code, kill, restart.
//!
//! Parser and renderer stay decoupled from this layer: the reader only *feeds* bytes;
//! it never decides when a frame is "done" (see PLAN.md, anti-flicker architecture).

#![forbid(unsafe_code)]

// Scaffolding only — implementation lands in M0.
