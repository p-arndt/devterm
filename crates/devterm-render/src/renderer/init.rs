//! Renderer construction and state transitions: surface/device/pipeline setup, DPI-scale
//! and font-size changes, viewport upkeep, and the metric queries callers use to lay out
//! their grids.

use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use winit::window::Window;

use crate::atlas::Atlas;
use crate::font::{compute_metrics, load_font_faces};
use crate::gpu::{BG_WGSL, BgInstance, GLYPH_WGSL, GlyphInstance, InstanceBuffer, Viewport};
use crate::{CellMetrics, Renderer};

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

        // --- font: primary monospace face plus whatever fallback faces resolve ---
        let faces = load_font_faces()
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
        let primary = &faces[0];
        let (metrics, baseline) = compute_metrics(
            &primary.data,
            primary.index,
            font_size_px * scale_factor as f32,
        );

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
            faces,
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
        let primary = &self.faces[0];
        let (metrics, baseline) = compute_metrics(&primary.data, primary.index, self.font_px());
        self.metrics = metrics;
        self.baseline = baseline;
        // Cached glyphs were rasterized at the old pixel size; drop them.
        self.atlas.reset();
    }

    /// Change the base font size (physical px at scale 1.0), recompute cell metrics and
    /// baseline, and drop cached glyphs so they re-rasterize at the new size. Callers must
    /// re-derive each pane's cols/rows afterwards (cell size changed). No-op if unchanged.
    pub fn set_font_size(&mut self, px: f32) {
        let px = px.max(1.0);
        if (px - self.base_font_px).abs() < f32::EPSILON {
            return;
        }
        self.base_font_px = px;
        let primary = &self.faces[0];
        let (metrics, baseline) = compute_metrics(&primary.data, primary.index, self.font_px());
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

    /// Current physical pixel font size (base size times DPI scale).
    pub(crate) fn font_px(&self) -> f32 {
        self.base_font_px * self.scale_factor as f32
    }

    /// Push the current surface size into the viewport uniform buffer.
    pub(crate) fn write_viewport(&self) {
        let vp = Viewport {
            size: [self.config.width as f32, self.config.height as f32],
            _pad: [0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.viewport_buffer, 0, bytemuck::bytes_of(&vp));
    }
}
