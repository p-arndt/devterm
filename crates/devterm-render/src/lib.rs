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

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use bytemuck::{Pod, Zeroable};
use devterm_core::Rect;
use devterm_term::{CursorShape, Rgb, Snapshot};
use winit::window::Window;

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Angle, Transform, Vector};

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
// GPU instance structs (POD for direct upload into vertex buffers).
// ---------------------------------------------------------------------------

/// A solid colored quad: pixel position/size + linear RGBA.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BgInstance {
    pos: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

/// A textured glyph quad: pixel position/size, atlas UV rect, linear RGBA tint.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphInstance {
    pos: [f32; 2],
    size: [f32; 2],
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    color: [f32; 4],
}

/// Viewport uniform: `xy` = surface size in physical px (`zw` padding for 16-byte align).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Viewport {
    size: [f32; 2],
    _pad: [f32; 2],
}

// ---------------------------------------------------------------------------
// Color helpers.
// ---------------------------------------------------------------------------

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
fn linear_rgba(rgb: Rgb, alpha: f32) -> [f32; 4] {
    [
        srgb_to_linear(rgb.r),
        srgb_to_linear(rgb.g),
        srgb_to_linear(rgb.b),
        alpha,
    ]
}

// ---------------------------------------------------------------------------
// Glyph atlas.
// ---------------------------------------------------------------------------

/// Atlas key: a character in one of the four bold/italic styling combinations.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    c: char,
    bold: bool,
    italic: bool,
}

/// Placement + UV of a rasterized glyph inside the atlas (all in physical px / [0,1] uv).
#[derive(Clone, Copy)]
struct GlyphInfo {
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    /// Bitmap left offset from the pen position.
    left: f32,
    /// Bitmap top offset above the baseline.
    top: f32,
    width: f32,
    height: f32,
}

/// Coverage atlas: an `R8Unorm` texture filled on demand by a simple shelf packer.
struct Atlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: u32,
    shelf_x: u32,
    shelf_y: u32,
    shelf_height: u32,
    glyphs: HashMap<GlyphKey, GlyphInfo>,
    scale_ctx: ScaleContext,
}

impl Atlas {
    const SIZE: u32 = 1024;

    fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("devterm glyph atlas"),
            size: wgpu::Extent3d {
                width: Self::SIZE,
                height: Self::SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size: Self::SIZE,
            shelf_x: 0,
            shelf_y: 0,
            shelf_height: 0,
            glyphs: HashMap::new(),
            scale_ctx: ScaleContext::new(),
        }
    }

    /// Drop all cached glyphs and reset the packer (used when the pixel size changes).
    /// The texture is reused; stale texels are never sampled because their UVs are gone.
    fn reset(&mut self) {
        self.shelf_x = 0;
        self.shelf_y = 0;
        self.shelf_height = 0;
        self.glyphs.clear();
    }

    /// Return the atlas entry for `key`, rasterizing and packing it on first use.
    fn glyph(
        &mut self,
        queue: &wgpu::Queue,
        font_data: &[u8],
        font_index: u32,
        px: f32,
        key: GlyphKey,
    ) -> GlyphInfo {
        if let Some(info) = self.glyphs.get(&key) {
            return *info;
        }
        let info = self
            .rasterize(queue, font_data, font_index, px, key)
            .unwrap_or(GlyphInfo {
                uv_min: [0.0, 0.0],
                uv_max: [0.0, 0.0],
                left: 0.0,
                top: 0.0,
                width: 0.0,
                height: 0.0,
            });
        self.glyphs.insert(key, info);
        info
    }

    /// Rasterize a glyph with swash and upload it into a free atlas shelf slot.
    fn rasterize(
        &mut self,
        queue: &wgpu::Queue,
        font_data: &[u8],
        font_index: u32,
        px: f32,
        key: GlyphKey,
    ) -> Option<GlyphInfo> {
        let font = FontRef::from_index(font_data, font_index as usize)?;
        let glyph_id = font.charmap().map(key.c);
        if glyph_id == 0 {
            return None; // no glyph for this codepoint in the font
        }

        let mut scaler = self.scale_ctx.builder(font).size(px).hint(true).build();

        let mut render = Render::new(&[Source::Outline]);
        if key.bold {
            // Faux-bold: emboldening strength scales with the font size.
            render.embolden(px * 0.03);
        }
        if key.italic {
            // Faux-italic: shear the outline ~12 degrees.
            render.transform(Some(Transform::skew(
                Angle::from_degrees(-12.0),
                Angle::from_degrees(0.0),
            )));
            render.offset(Vector::new(0.0, 0.0));
        }

        let image = render.render(&mut scaler, glyph_id)?;
        let w = image.placement.width;
        let h = image.placement.height;

        if w == 0 || h == 0 {
            // Whitespace or empty outline: a valid glyph with no coverage.
            return Some(GlyphInfo {
                uv_min: [0.0, 0.0],
                uv_max: [0.0, 0.0],
                left: image.placement.left as f32,
                top: image.placement.top as f32,
                width: 0.0,
                height: 0.0,
            });
        }

        let (x, y) = self.pack(w, h)?;

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &image.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        let inv = 1.0 / self.size as f32;
        Some(GlyphInfo {
            uv_min: [x as f32 * inv, y as f32 * inv],
            uv_max: [(x + w) as f32 * inv, (y + h) as f32 * inv],
            left: image.placement.left as f32,
            top: image.placement.top as f32,
            width: w as f32,
            height: h as f32,
        })
    }

    /// Reserve a `w x h` slot with a 1px gutter using a shelf packer. `None` if full.
    fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.size || h > self.size {
            return None;
        }
        if self.shelf_x + w > self.size {
            // Advance to the next shelf.
            self.shelf_x = 0;
            self.shelf_y += self.shelf_height + 1;
            self.shelf_height = 0;
        }
        if self.shelf_y + h > self.size {
            return None; // atlas exhausted
        }
        let pos = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_height = self.shelf_height.max(h);
        Some(pos)
    }
}

// ---------------------------------------------------------------------------
// A GPU vertex buffer that grows on demand.
// ---------------------------------------------------------------------------

struct InstanceBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
    label: &'static str,
}

impl InstanceBuffer {
    fn new(device: &wgpu::Device, label: &'static str) -> Self {
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
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> u64 {
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

    // Font state.
    font_data: Vec<u8>,
    font_index: u32,
    base_font_px: f32,
    scale_factor: f64,

    // Derived metrics at the current scale.
    metrics: CellMetrics,
    /// Distance from the top of a cell down to the glyph baseline (physical px).
    baseline: f32,
}

impl Renderer {
    /// Bind a renderer to `window`. `font_size_px` is the cell font size in physical px at
    /// scale 1.0. Blocks on device acquisition internally (pollster).
    pub fn new(window: Arc<Window>, font_size_px: f32) -> anyhow::Result<Renderer> {
        // Native DX12 on Windows: it is the expected backend on an NVIDIA/Windows box,
        // avoids the extra Vulkan compositor hop, and silences wgpu-hal's Vulkan
        // present-mode warning spam. `DX12 | PRIMARY` would be a no-op (PRIMARY already
        // contains DX12) and let wgpu pick Vulkan, which is exactly what happened.
        let backends = if cfg!(target_os = "windows") {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::PRIMARY
        };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });

        // Safe surface: Arc<Window> is 'static, so no `unsafe` handle juggling is needed.
        let surface = instance
            .create_surface(window.clone())
            .context("create wgpu surface from window")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .ok_or_else(|| anyhow!("no compatible wgpu adapter found"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("devterm device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .context("request wgpu device")?;

        // Choose an sRGB surface format so colors composite in linear light.
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or_else(|| caps.formats[0]);

        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::AutoVsync) {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::Fifo
        };
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes[0]
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        // --- font ---
        let (font_data, font_index) = load_monospace_font()
            .context("no monospace font found via fontdb (Cascadia/Consolas/JetBrains/any)")?;

        // --- viewport uniform + bind group (group 0) ---
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("devterm viewport uniform"),
            size: std::mem::size_of::<Viewport>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("devterm viewport layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("devterm viewport bind group"),
            layout: &viewport_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        // --- atlas + sampler bind group (group 1) ---
        let atlas = Atlas::new(&device);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("devterm atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("devterm atlas layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("devterm atlas bind group"),
            layout: &atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // --- pipelines ---
        let bg_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("devterm bg shader"),
            source: wgpu::ShaderSource::Wgsl(BG_WGSL.into()),
        });
        let glyph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("devterm glyph shader"),
            source: wgpu::ShaderSource::Wgsl(GLYPH_WGSL.into()),
        });

        let bg_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("devterm bg pipeline layout"),
            bind_group_layouts: &[&viewport_layout],
            push_constant_ranges: &[],
        });
        let glyph_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("devterm glyph pipeline layout"),
                bind_group_layouts: &[&viewport_layout, &atlas_layout],
                push_constant_ranges: &[],
            });

        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let color_target = wgpu::ColorTargetState {
            format,
            blend: Some(blend),
            write_mask: wgpu::ColorWrites::ALL,
        };

        const BG_ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let bg_buffer_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BgInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &BG_ATTRS,
        };

        const GLYPH_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4
        ];
        let glyph_buffer_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &GLYPH_ATTRS,
        };

        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        };

        let bg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("devterm bg pipeline"),
            layout: Some(&bg_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &bg_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[bg_buffer_layout],
            },
            primitive,
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &bg_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(color_target.clone())],
            }),
            multiview: None,
            cache: None,
        });

        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("devterm glyph pipeline"),
            layout: Some(&glyph_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &glyph_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[glyph_buffer_layout],
            },
            primitive,
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &glyph_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(color_target)],
            }),
            multiview: None,
            cache: None,
        });

        let bg_instances = InstanceBuffer::new(&device, "devterm bg instances");
        let glyph_instances = InstanceBuffer::new(&device, "devterm glyph instances");

        let scale_factor = window.scale_factor();
        let (metrics, baseline) =
            compute_metrics(&font_data, font_index, font_size_px * scale_factor as f32);

        let renderer = Renderer {
            _window: window,
            surface,
            device,
            queue,
            config,
            viewport_buffer,
            viewport_bind_group,
            atlas_bind_group,
            bg_pipeline,
            glyph_pipeline,
            bg_instances,
            glyph_instances,
            atlas,
            font_data,
            font_index,
            base_font_px: font_size_px,
            scale_factor,
            metrics,
            baseline,
        };
        renderer.write_viewport();
        Ok(renderer)
    }

    /// Surface + viewport resize (physical px).
    pub fn resize(&mut self, width_px: u32, height_px: u32) {
        let width = width_px.max(1);
        let height = height_px.max(1);
        if width == self.config.width && height == self.config.height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.write_viewport();
    }

    /// DPI scale change; rebuild glyph metrics/atlas as needed.
    pub fn set_scale_factor(&mut self, scale: f64) {
        if (scale - self.scale_factor).abs() < f64::EPSILON {
            return;
        }
        self.scale_factor = scale;
        let (metrics, baseline) = compute_metrics(&self.font_data, self.font_index, self.font_px());
        self.metrics = metrics;
        self.baseline = baseline;
        // Cached glyphs were rasterized at the old pixel size; drop them.
        self.atlas.reset();
    }

    pub fn cell_metrics(&self) -> CellMetrics {
        self.metrics
    }

    /// Cols/rows that fit in the given physical pixel area at current metrics.
    pub fn grid_size_for(&self, width_px: u32, height_px: u32) -> (u16, u16) {
        let cols = (width_px as f32 / self.metrics.width).floor().max(1.0);
        let rows = (height_px as f32 / self.metrics.height).floor().max(1.0);
        (
            cols.min(u16::MAX as f32) as u16,
            rows.min(u16::MAX as f32) as u16,
        )
    }

    /// Render one frame of all panes.
    pub fn render(&mut self, panes: &[PaneView]) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Build the CPU-side instance lists for every pane.
        let surface_w = self.config.width as f32;
        let surface_h = self.config.height as f32;
        let mut bg: Vec<BgInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();

        for pane in panes {
            self.build_pane(pane, surface_w, surface_h, &mut bg, &mut glyphs);
        }

        let bg_bytes = bytemuck::cast_slice(&bg);
        let glyph_bytes = bytemuck::cast_slice(&glyphs);
        self.bg_instances
            .upload(&self.device, &self.queue, bg_bytes);
        self.glyph_instances
            .upload(&self.device, &self.queue, glyph_bytes);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("devterm frame encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("devterm frame pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if !bg.is_empty() {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_vertex_buffer(0, self.bg_instances.buffer.slice(..));
                pass.draw(0..4, 0..bg.len() as u32);
            }

            if !glyphs.is_empty() {
                pass.set_pipeline(&self.glyph_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, self.glyph_instances.buffer.slice(..));
                pass.draw(0..4, 0..glyphs.len() as u32);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    // --- internals ---

    /// Current physical pixel font size (base size times DPI scale).
    fn font_px(&self) -> f32 {
        self.base_font_px * self.scale_factor as f32
    }

    /// Push the current surface size into the viewport uniform buffer.
    fn write_viewport(&self) {
        let vp = Viewport {
            size: [self.config.width as f32, self.config.height as f32],
            _pad: [0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.viewport_buffer, 0, bytemuck::bytes_of(&vp));
    }

    /// Turn one pane's snapshot into background and glyph instances.
    fn build_pane(
        &mut self,
        pane: &PaneView,
        surface_w: f32,
        surface_h: f32,
        bg: &mut Vec<BgInstance>,
        glyphs: &mut Vec<GlyphInstance>,
    ) {
        let snap = pane.snapshot;
        let area = pane.area;
        let origin_x = area.x * surface_w;
        let origin_y = area.y * surface_h;
        let pane_w = area.w * surface_w;
        let pane_h = area.h * surface_h;
        let cw = self.metrics.width;
        let ch = self.metrics.height;

        // Fill the whole pane with the default background first.
        bg.push(BgInstance {
            pos: [origin_x, origin_y],
            size: [pane_w, pane_h],
            color: linear_rgba(snap.default_bg, 1.0),
        });

        // Per-cell backgrounds (only where they differ from the default).
        for cell in &snap.cells {
            if cell.bg == snap.default_bg {
                continue;
            }
            let span = if cell.wide { 2.0 } else { 1.0 };
            bg.push(BgInstance {
                pos: [
                    origin_x + cell.col as f32 * cw,
                    origin_y + cell.line as f32 * ch,
                ],
                size: [cw * span, ch],
                color: linear_rgba(cell.bg, 1.0),
            });
        }

        // Cursor: block/underline/beam. A focused block cursor inverts its glyph below.
        let cursor = snap.cursor;
        let block_cursor = pane.focused && cursor.shape == CursorShape::Block;
        if cursor.shape != CursorShape::Hidden {
            let cx = origin_x + cursor.col as f32 * cw;
            let cy = origin_y + cursor.line as f32 * ch;
            let (pos, size) = match cursor.shape {
                CursorShape::Block => ([cx, cy], [cw, ch]),
                CursorShape::Underline => ([cx, cy + ch - 2.0], [cw, 2.0]),
                CursorShape::Beam => ([cx, cy], [2.0, ch]),
                CursorShape::Hidden => ([cx, cy], [0.0, 0.0]),
            };
            bg.push(BgInstance {
                pos,
                size,
                color: linear_rgba(cursor.color, 1.0),
            });
        }

        // Glyphs.
        let px = self.font_px();
        for cell in &snap.cells {
            if cell.c == ' ' || cell.c == '\0' {
                continue;
            }
            let key = GlyphKey {
                c: cell.c,
                bold: cell.bold,
                italic: cell.italic,
            };
            let info = self
                .atlas
                .glyph(&self.queue, &self.font_data, self.font_index, px, key);
            if info.width <= 0.0 || info.height <= 0.0 {
                continue;
            }

            // Invert the glyph color under a focused block cursor for legibility.
            let fg = if block_cursor && cell.line == cursor.line && cell.col == cursor.col {
                snap.default_bg
            } else {
                cell.fg
            };

            let pen_x = origin_x + cell.col as f32 * cw;
            let baseline_y = origin_y + cell.line as f32 * ch + self.baseline;
            glyphs.push(GlyphInstance {
                pos: [pen_x + info.left, baseline_y - info.top],
                size: [info.width, info.height],
                uv_min: info.uv_min,
                uv_max: info.uv_max,
                color: linear_rgba(fg, 1.0),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Font loading + metrics.
// ---------------------------------------------------------------------------

/// Load a monospace font's raw data + face index, trying a preferred chain first.
fn load_monospace_font() -> Option<(Vec<u8>, u32)> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let candidates = [
        fontdb::Family::Name("Cascadia Mono"),
        fontdb::Family::Name("Consolas"),
        fontdb::Family::Name("JetBrains Mono"),
        fontdb::Family::Monospace,
    ];

    for family in candidates {
        let query = fontdb::Query {
            families: &[family],
            ..Default::default()
        };
        if let Some(id) = db.query(&query)
            && let Some(data) = db.with_face_data(id, |data, index| (data.to_vec(), index))
        {
            return Some(data);
        }
    }
    None
}

/// Compute cell metrics and the top-to-baseline distance for a given pixel font size.
fn compute_metrics(font_data: &[u8], font_index: u32, px: f32) -> (CellMetrics, f32) {
    let fallback = CellMetrics {
        width: (px * 0.6).ceil().max(1.0),
        height: (px * 1.2).ceil().max(1.0),
    };
    let Some(font) = FontRef::from_index(font_data, font_index as usize) else {
        return (fallback, (px).ceil());
    };

    let m = font.metrics(&[]).scale(px);
    let height = (m.ascent + m.descent + m.leading).ceil().max(1.0);
    let baseline = m.ascent + m.leading * 0.5;

    // Advance width of a representative monospace glyph.
    let glyph_id = font.charmap().map('M');
    let advance = font.glyph_metrics(&[]).scale(px).advance_width(glyph_id);
    let width = if advance > 0.0 {
        advance.ceil().max(1.0)
    } else {
        fallback.width
    };

    (CellMetrics { width, height }, baseline)
}

// ---------------------------------------------------------------------------
// Inline WGSL shaders.
// ---------------------------------------------------------------------------

/// Background pipeline: solid colored instanced quads in pixel space.
const BG_WGSL: &str = r#"
struct Viewport { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) inst_pos: vec2<f32>,
    @location(1) inst_size: vec2<f32>,
    @location(2) inst_color: vec4<f32>,
) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = inst_pos + corner * inst_size;
    let ndc = vec2<f32>(p.x / viewport.size.x * 2.0 - 1.0, 1.0 - p.y / viewport.size.y * 2.0);
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = inst_color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// Glyph pipeline: textured instanced quads tinted by fg, coverage sampled as alpha.
const GLYPH_WGSL: &str = r#"
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
