//! Per-frame drawing: turning pane snapshots into background and glyph instances, drawing
//! pane separators / focus highlights, and submitting the frame to the surface.

use devterm_term::CursorShape;

use crate::color::{BORDER_ACCENT, BORDER_DIM, linear_rgba};
use crate::gpu::{BgInstance, GlyphInstance};
use crate::{PaneView, Renderer, push_frame};

impl Renderer {
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

        // Pane separators + focus highlight, appended last so they blend over the
        // per-pane content. The glyph pass runs after this, so text at the very edge
        // still draws on top of the thin border and stays legible.
        self.build_borders(panes, surface_w, surface_h, &mut bg);

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
