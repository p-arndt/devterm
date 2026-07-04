//! GPU renderer for DevTerm (wgpu / D3D12 on Windows).
//!
//! Renders the terminal cell grid, cursor and split panes. Owns the wgpu
//! instance/surface/device/queue, a `swash`-rasterized glyph atlas (an `R8Unorm`
//! coverage texture), and two instanced pipelines that share a viewport uniform:
//!
//! 1. a **background** pipeline drawing one solid colored quad per non-default-bg cell
//!    (plus a pane-filling quad and the cursor block), and
//! 2. a **glyph** pipeline drawing one textured quad per printable cell, sampling the
//!    atlas coverage as alpha and tinting it with the cell foreground.
//!
//! Colors arrive from `devterm-term` already fully resolved to sRGB bytes; the renderer
//! is "dumb" and only converts them to linear light for the sRGB surface.
//!
//! The renderer prefers the safe `Arc<Window>` surface path, so `#![forbid(unsafe_code)]`
//! stays enabled: no `unsafe` is required against wgpu 24's safe API.
//!
//! # Module layout
//!
//! - [`color`] — sRGB→linear conversion and the border accent/dim constants.
//! - [`gpu`] — POD instance structs, the growable instance buffer, and the WGSL shaders.
//! - [`atlas`] — the swash-rasterized glyph coverage atlas and its shelf packer.
//! - [`font`] — font discovery/fallback chain and cell-metric computation.
//! - [`renderer`] — the [`Renderer`] method implementations (`init` + `draw`).

#![forbid(unsafe_code)]

use std::sync::Arc;

use devterm_core::Rect;
use devterm_term::{Rgb, Snapshot};
use winit::window::Window;

use crate::atlas::Atlas;
use crate::color::linear_rgba;
use crate::font::FontFace;
use crate::gpu::{BgInstance, InstanceBuffer};

mod atlas;
mod color;
mod font;
mod gpu;
mod renderer;

// ---------------------------------------------------------------------------
// Public interface (see docs/M0_INTERFACES.md — implemented verbatim).
// ---------------------------------------------------------------------------

/// Physical-pixel size of one terminal cell at the current scale factor.
#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
}

/// One pane to render: a unit-square sub-rectangle plus the snapshot to draw in it.
pub struct PaneView<'a> {
    pub area: Rect,
    pub snapshot: &'a Snapshot,
    pub focused: bool,
}

// ---------------------------------------------------------------------------
// Renderer.
// ---------------------------------------------------------------------------

/// Owns all GPU state and draws terminal snapshots into a window surface.
pub struct Renderer {
    // Keep the window alive as long as the surface references it.
    _window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    atlas_bind_group: wgpu::BindGroup,

    bg_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,

    bg_instances: InstanceBuffer,
    glyph_instances: InstanceBuffer,

    atlas: Atlas,

    // Font state: `faces[0]` is the primary monospace face (drives metrics); the rest
    // are the fallback chain used only when the primary lacks a codepoint.
    faces: Vec<FontFace>,
    base_font_px: f32,
    scale_factor: f64,

    // Derived metrics at the current scale.
    metrics: CellMetrics,
    /// Distance from the top of a cell down to the glyph baseline (physical px).
    baseline: f32,
}

/// Push four thin quads outlining the `w x h` rect at `(x, y)` with the given thickness.
fn push_frame(bg: &mut Vec<BgInstance>, x: f32, y: f32, w: f32, h: f32, t: f32, color: Rgb) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let t = t.min(w).min(h);
    let c = linear_rgba(color, 1.0);
    // top, bottom, left, right
    bg.push(BgInstance {
        pos: [x, y],
        size: [w, t],
        color: c,
    });
    bg.push(BgInstance {
        pos: [x, y + h - t],
        size: [w, t],
        color: c,
    });
    bg.push(BgInstance {
        pos: [x, y],
        size: [t, h],
        color: c,
    });
    bg.push(BgInstance {
        pos: [x + w - t, y],
        size: [t, h],
        color: c,
    });
}
