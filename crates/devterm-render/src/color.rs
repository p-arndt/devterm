//! Color helpers: sRGB→linear conversion for the sRGB surface, plus the pane-border
//! accent/dim color constants.
//!
//! Colors arrive from `devterm-term` already fully resolved to sRGB bytes; these helpers
//! only convert them to linear light so the sRGB surface re-encodes them correctly on
//! write.

use devterm_term::Rgb;

/// Convert an 8-bit sRGB channel to linear light (the sRGB surface re-encodes on write).
fn srgb_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert a resolved [`Rgb`] plus an alpha into a linear RGBA array for the GPU.
pub(crate) fn linear_rgba(rgb: Rgb, alpha: f32) -> [f32; 4] {
    [
        srgb_to_linear(rgb.r),
        srgb_to_linear(rgb.g),
        srgb_to_linear(rgb.b),
        alpha,
    ]
}

/// Accent color of the focused pane's border.
pub(crate) const BORDER_ACCENT: Rgb = Rgb {
    r: 0x4d,
    g: 0x9a,
    b: 0xff,
};

/// Dim color of separators between unfocused panes.
pub(crate) const BORDER_DIM: Rgb = Rgb {
    r: 0x3a,
    g: 0x3a,
    b: 0x3a,
};
