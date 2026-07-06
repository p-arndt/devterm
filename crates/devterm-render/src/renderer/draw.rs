//! Per-frame drawing: turning pane snapshots into background and glyph instances, drawing
//! pane separators / focus highlights, and submitting the frame to the surface.

use devterm_term::CursorShape;

use crate::color::{BORDER_ACCENT, BORDER_DIM, linear_rgba};
use crate::gpu::{BgInstance, GlyphInstance};
use crate::{PaneView, Renderer, push_frame};

impl Renderer {
    /// Render one frame: the tiled layout `panes`, then an optional floating `overlay`
    /// drawn on top. `chrome` is an optional UI strip (the tab bar) drawn in the base
    /// layer like a pane but without separators or a focus frame.
    ///
    /// The renderer draws all backgrounds before all glyphs, so a naive single-layer pass
    /// would let the base panes' *text* bleed over a pane stacked on top of them. To make
    /// the overlay opaque it is drawn as a **second layer**: the base layer's background and
    /// glyph instances are drawn first, then the overlay's own background quad (which
    /// occludes the base text beneath it) and finally the overlay's glyphs. The two layers
    /// share the instance buffers; each layer's draw selects its slice via a vertex-buffer
    /// byte offset (so `first_instance` stays 0 and the path is portable across backends).
    pub fn render(
        &mut self,
        panes: &[PaneView],
        overlay: Option<&PaneView>,
        chrome: Option<&PaneView>,
    ) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Build the CPU-side instance lists for every pane.
        let surface_w = self.config.width as f32;
        let surface_h = self.config.height as f32;
        let mut bg: Vec<BgInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();

        // --- base layer: window chrome (borderless), then the tiled layout panes ---
        if let Some(chrome) = chrome {
            self.build_pane(chrome, surface_w, surface_h, &mut bg, &mut glyphs);
        }
        for pane in panes {
            self.build_pane(pane, surface_w, surface_h, &mut bg, &mut glyphs);
        }
        // Pane separators + focus highlight, appended last so they blend over the
        // per-pane content. The glyph pass runs after this, so text at the very edge
        // still draws on top of the thin border and stays legible.
        self.build_borders(panes, surface_w, surface_h, &mut bg);
        let base_bg = bg.len();
        let base_glyphs = glyphs.len();

        // --- overlay layer: the floating terminal, drawn opaquely over the base ---
        if let Some(overlay) = overlay {
            self.build_pane(overlay, surface_w, surface_h, &mut bg, &mut glyphs);
            self.build_borders(std::slice::from_ref(overlay), surface_w, surface_h, &mut bg);
        }

        self.bg_instances
            .upload(&self.device, &self.queue, bytemuck::cast_slice(&bg));
        self.glyph_instances
            .upload(&self.device, &self.queue, bytemuck::cast_slice(&glyphs));

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

            // Layer draw order: base bg, base glyphs, overlay bg, overlay glyphs. Drawing the
            // overlay's glyphs only after its own opaque background quad is what occludes the
            // base text underneath it.
            self.draw_bg(&mut pass, 0, base_bg);
            self.draw_glyphs(&mut pass, 0, base_glyphs);
            self.draw_bg(&mut pass, base_bg, bg.len());
            self.draw_glyphs(&mut pass, base_glyphs, glyphs.len());
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// Draw background instances `[start, end)` from the shared buffer. The instance slice is
    /// selected by a vertex-buffer byte offset so `first_instance` stays 0.
    fn draw_bg(&self, pass: &mut wgpu::RenderPass<'_>, start: usize, end: usize) {
        if end <= start {
            return;
        }
        let stride = std::mem::size_of::<BgInstance>() as u64;
        pass.set_pipeline(&self.bg_pipeline);
        pass.set_bind_group(0, &self.viewport_bind_group, &[]);
        pass.set_vertex_buffer(0, self.bg_instances.buffer.slice(start as u64 * stride..));
        pass.draw(0..4, 0..(end - start) as u32);
    }

    /// Draw glyph instances `[start, end)` from the shared buffer (see [`draw_bg`]).
    fn draw_glyphs(&self, pass: &mut wgpu::RenderPass<'_>, start: usize, end: usize) {
        if end <= start {
            return;
        }
        let stride = std::mem::size_of::<GlyphInstance>() as u64;
        pass.set_pipeline(&self.glyph_pipeline);
        pass.set_bind_group(0, &self.viewport_bind_group, &[]);
        pass.set_bind_group(1, &self.atlas_bind_group, &[]);
        pass.set_vertex_buffer(
            0,
            self.glyph_instances.buffer.slice(start as u64 * stride..),
        );
        pass.draw(0..4, 0..(end - start) as u32);
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

        // Cursor: block/underline/beam. A focused block cursor is filled and inverts its
        // glyph below; an unfocused block cursor is drawn as a hollow outline (the classic
        // "unfocused terminal" look). Underline/beam are unchanged in both states.
        let cursor = snap.cursor;
        let block_cursor = pane.focused && cursor.shape == CursorShape::Block;
        if cursor.shape != CursorShape::Hidden {
            let cx = origin_x + cursor.col as f32 * cw;
            let cy = origin_y + cursor.line as f32 * ch;
            let color = linear_rgba(cursor.color, 1.0);
            match cursor.shape {
                CursorShape::Block if pane.focused => {
                    bg.push(BgInstance {
                        pos: [cx, cy],
                        size: [cw, ch],
                        color,
                    });
                }
                // Unfocused block: a thin outline instead of a filled quad.
                CursorShape::Block => {
                    push_frame(bg, cx, cy, cw, ch, self.border_thickness(), cursor.color);
                }
                CursorShape::Underline => bg.push(BgInstance {
                    pos: [cx, cy + ch - 2.0],
                    size: [cw, 2.0],
                    color,
                }),
                CursorShape::Beam => bg.push(BgInstance {
                    pos: [cx, cy],
                    size: [2.0, ch],
                    color,
                }),
                CursorShape::Hidden => {}
            }
        }

        // Glyphs.
        let px = self.font_px();
        for cell in &snap.cells {
            if cell.c == ' ' || cell.c == '\0' {
                continue;
            }
            let info =
                self.atlas
                    .glyph(&self.queue, &self.faces, px, cell.c, cell.bold, cell.italic);
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

    /// Physical-pixel border thickness, derived from the DPI scale (1px at scale 1,
    /// 2px at scale 2, …). Focused panes double this for a clearer highlight.
    fn border_thickness(&self) -> f32 {
        (self.scale_factor.round() as f32).max(1.0)
    }

    /// Draw thin pane separators and a focus highlight into the background instances.
    ///
    /// A focused pane gets an accent frame; unfocused panes get a dim separator. A lone
    /// full-window pane is only outlined (subtly) when focused, so a single-pane layout
    /// stays clean.
    fn build_borders(
        &self,
        panes: &[PaneView],
        surface_w: f32,
        surface_h: f32,
        bg: &mut Vec<BgInstance>,
    ) {
        let t = self.border_thickness();
        let single = panes.len() == 1;
        for pane in panes {
            let x = pane.area.x * surface_w;
            let y = pane.area.y * surface_h;
            let w = pane.area.w * surface_w;
            let h = pane.area.h * surface_h;
            let (color, thickness) = if pane.focused {
                // Subtle 1px accent for a lone pane; a heavier 2px accent when split.
                (BORDER_ACCENT, if single { t } else { t * 2.0 })
            } else if single {
                continue; // no border on an unfocused single pane
            } else {
                (BORDER_DIM, t)
            };
            push_frame(bg, x, y, w, h, thickness, color);
        }
    }
}
