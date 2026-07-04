//! Terminal emulation for DevTerm.
//!
//! Wraps `alacritty_terminal` (VT parser + grid + scrollback) — we do **not** write our
//! own emulator; that is the single biggest risk of the project (years of edge cases).
//! This crate owns:
//! - feeding PTY bytes into the emulator,
//! - resolving every cell colour to concrete RGB for a "dumb" renderer,
//! - exposing an RGB snapshot + cursor for the renderer,
//! - collecting bytes the emulator wants written back to the child (DSR/DA replies).
//!
//! The implementation is split by responsibility:
//! - `color` — the [`Rgb`] type and the built-in xterm-256 default palette.
//! - `palette` — the theme-override [`Palette`].
//! - `snapshot` — the render-facing frame types ([`Snapshot`], [`RenderCell`], …).
//! - `selection` — the [`SelectionMode`] enum.
//! - `resolve` — colour-resolution helpers (palette index → concrete RGB).
//! - `term` — the core [`Term`] wrapper.

#![forbid(unsafe_code)]

mod color;
mod palette;
mod resolve;
mod selection;
mod snapshot;
mod term;

pub use color::Rgb;
pub use palette::Palette;
pub use selection::SelectionMode;
pub use snapshot::{Cursor, CursorShape, RenderCell, Snapshot};
pub use term::Term;
