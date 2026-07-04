//! Key chords, modifiers, and the default/tmux keymap presets.
//!
//! A [`KeyChord`] is a set of [`Mods`] plus a [`KeyCode`]. Chords parse from and
//! render to `+`-separated strings like `ctrl+shift+h` or `alt+left`. The presets
//! map every [`Action`] to a chord.
//!
//! The module is split into three parts:
//! - `chord` — [`Mods`], [`KeyCode`], [`KeyChord`], and their parse/display impls.
//! - `named` — the [`Named`] non-character keys.
//! - `preset` — [`KeymapPreset`] and the [`default_keymap`]/[`tmux_preset`] tables.
//!
//! [`Action`]: crate::Action

mod chord;
mod named;
mod preset;

pub use chord::{KeyChord, KeyCode, Mods, ParseKeyChordError};
pub use named::Named;
pub use preset::{KeymapPreset, default_keymap, tmux_preset};
