//! GPU renderer for DevTerm (wgpu / D3D12 on Windows).
//!
//! Renders the terminal cell grid, cursor, selection and split borders. Owns the glyph
//! atlas built from `swash`-rasterized glyphs with a `fontdb` fallback chain (Nerd Font
//! symbols, emoji, CJK). Uses gamma-correct blending to match ClearType expectations.
//!
//! Draws the *current* model state every VSync — never a per-read snapshot — which is the
//! core of the anti-flicker design (see PLAN.md).

#![forbid(unsafe_code)]

// Scaffolding only — implementation lands in M0.
