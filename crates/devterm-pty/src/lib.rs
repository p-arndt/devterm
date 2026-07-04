//! PTY layer for DevTerm (Windows: ConPTY, Unix: native PTY — both via `portable-pty`).
//!
//! Responsibilities:
//! - spawn a shell process on a pseudo-terminal,
//! - a reader thread that streams output bytes to the terminal model,
//! - a writer that forwards keyboard input,
//! - process lifecycle: resize, exit code, kill.
//!
//! Parser and renderer stay decoupled from this layer: the reader only *feeds* bytes;
//! it never decides when a frame is "done" (see PLAN.md, anti-flicker architecture).
//!
//! Module layout:
//! - `spec`: value types ([`PtySize`], [`PtyEvent`], [`PtyCommandSpec`]).
//! - `shell`: default shell resolution per platform.
//! - `pty`: the running-process type [`Pty`] and its lifecycle.

#![forbid(unsafe_code)]

mod pty;
mod shell;
mod spec;

pub use pty::Pty;
pub use spec::{PtyCommandSpec, PtyEvent, PtySize};
