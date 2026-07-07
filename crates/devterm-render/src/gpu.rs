//! GPU plumbing shared by the two instanced pipelines: the POD instance structs uploaded
//! straight into vertex buffers, the viewport uniform, a vertex buffer that grows on
//! demand, and the inline WGSL shader sources for the background and glyph pipelines.

use bytemuck::{Pod, Zeroable};

// ---------------------------------------------------------------------------
// GPU instance structs (POD for direct upload into vertex buffers).
// ---------------------------------------------------------------------------

/// A solid colored quad: pixel position/size + linear RGBA. `radius` is the corner radius
/// in physical px; `0.0` draws a plain (unrounded) quad, the common case for cell
/// backgrounds, the cursor and pane borders.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct BgInstance {
    pub(crate) pos: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) color: [f32; 4],
    pub(crate) radius: f32,
}

/// A textured glyph quad: pixel position/size, atlas UV rect, linear RGBA tint.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GlyphInstance {
    pub(crate) pos: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) uv_min: [f32; 2],
    pub(crate) uv_max: [f32; 2],
    pub(crate) color: [f32; 4],
}

/// Viewport uniform: `xy` = surface size in physical px (`zw` padding for 16-byte align).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Viewport {
    pub(crate) size: [f32; 2],
    pub(crate) _pad: [f32; 2],
}

// ---------------------------------------------------------------------------
// A GPU vertex buffer that grows on demand.
// ---------------------------------------------------------------------------

pub(crate) struct InstanceBuffer {
    pub(crate) buffer: wgpu::Buffer,
    capacity: u64,
    label: &'static str,
}

impl InstanceBuffer {
    pub(crate) fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let capacity = 256 * std::mem::size_of::<GlyphInstance>() as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            capacity,
            label,
        }
    }

    /// Upload `bytes`, reallocating the buffer if it does not fit. Returns the number of
    /// bytes uploaded (0 for an empty slice, in which case the buffer is left untouched).
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
    ) -> u64 {
        if bytes.is_empty() {
            return 0;
        }
        let needed = bytes.len() as u64;
        if needed > self.capacity {
            let capacity = needed.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = capacity;
        }
        queue.write_buffer(&self.buffer, 0, bytes);
        needed
    }
}

// ---------------------------------------------------------------------------
// Inline WGSL shaders.
// ---------------------------------------------------------------------------

/// Background pipeline: solid colored instanced quads in pixel space. A non-zero
/// per-instance `radius` rounds the quad's corners via a signed-distance field with 1px
/// anti-aliasing (used by the tab bar); `radius == 0` returns the flat color unchanged, so
/// cell backgrounds/cursor/borders are byte-identical to the un-rounded path.
pub(crate) const BG_WGSL: &str = r#"
struct Viewport { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Pixel offset of this fragment from the quad centre, the quad half-extent, and the
    // corner radius — everything the fragment SDF needs.
    @location(1) local: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) radius: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) inst_pos: vec2<f32>,
    @location(1) inst_size: vec2<f32>,
    @location(2) inst_color: vec4<f32>,
    @location(3) inst_radius: f32,
) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = inst_pos + corner * inst_size;
    let ndc = vec2<f32>(p.x / viewport.size.x * 2.0 - 1.0, 1.0 - p.y / viewport.size.y * 2.0);
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = inst_color;
    out.local = (corner - vec2<f32>(0.5, 0.5)) * inst_size;
    out.half_size = inst_size * 0.5;
    out.radius = inst_radius;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Rounded-box signed distance (Inigo Quilez): distance from `local` to the box shrunk
    // by the radius, minus the radius. `fwidth` is evaluated unconditionally (it needs
    // uniform control flow); the branch is folded into `select` so radius == 0 stays flat.
    let r = min(in.radius, min(in.half_size.x, in.half_size.y));
    let q = abs(in.local) - (in.half_size - vec2<f32>(r, r));
    let dist = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    let aa = max(fwidth(dist), 0.0001);
    let rounded_alpha = 1.0 - smoothstep(-aa, aa, dist);
    let alpha = select(1.0, rounded_alpha, in.radius > 0.0);
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
"#;

/// Glyph pipeline: textured instanced quads tinted by fg, coverage sampled as alpha.
pub(crate) const GLYPH_WGSL: &str = r#"
struct Viewport { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) inst_pos: vec2<f32>,
    @location(1) inst_size: vec2<f32>,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) inst_color: vec4<f32>,
) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = inst_pos + corner * inst_size;
    let ndc = vec2<f32>(p.x / viewport.size.x * 2.0 - 1.0, 1.0 - p.y / viewport.size.y * 2.0);
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv_min + corner * (uv_max - uv_min);
    out.color = inst_color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas_tex, atlas_smp, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
"#;
