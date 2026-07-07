//! Per-frame drawing: turning pane snapshots into background and glyph instances, drawing
//! pane separators / focus highlights, and submitting the frame to the surface.

use devterm_term::CursorShape;

use crate::color::{BORDER_ACCENT, BORDER_DIM, linear_rgba};
use crate::gpu::{BgInstance, GlyphInstance};
use crate::{Chrome, PaneView, Renderer, push_frame};

impl Renderer {
    /// Render one frame: the tiled layout `panes`, then an optional floating `overlay`
    /// drawn on top. `chrome` is the optional window chrome (tab bar / caption strip),
    /// drawn in the base layer *before* the panes so a tab that overhangs the strip is
    /// overpainted by the terminal background beneath it.
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
        chrome: Option<&Chrome>,
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

        // --- base layer: window chrome (caption strip), then the tiled layout panes ---
        if let Some(chrome) = chrome {
            self.build_chrome(chrome, &mut bg, &mut glyphs);
        }
        for pane in panes {
            self.build_pane(pane, surface_w, surface_h, 0.0, &mut bg, &mut glyphs);
        }
        // Pane separators + focus highlight, appended last so they blend over the
        // per-pane content. The glyph pass runs after this, so text at the very edge
        // still draws on top of the thin border and stays legible.
        self.build_borders(panes, surface_w, surface_h, &mut bg);
        let base_bg = bg.len();
        let base_glyphs = glyphs.len();

        // --- overlay layer: the floating terminal, drawn opaquely over the base ---
        if let Some(overlay) = overlay {
            self.build_overlay_decor(overlay, surface_w, surface_h, &mut bg);
            let radius = self.overlay_radius();
            self.build_pane(overlay, surface_w, surface_h, radius, &mut bg, &mut glyphs);
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

    /// Turn one pane's snapshot into background and glyph instances. `corner_radius` rounds
    /// the pane's base fill (used by the floating overlay); tiled panes pass `0.0`.
    fn build_pane(
        &mut self,
        pane: &PaneView,
        surface_w: f32,
        surface_h: f32,
        corner_radius: f32,
        bg: &mut Vec<BgInstance>,
        glyphs: &mut Vec<GlyphInstance>,
    ) {
        let snap = pane.snapshot;
        let area = pane.area;
        let pane_x = area.x * surface_w;
        let pane_y = area.y * surface_h;
        let pane_w = area.w * surface_w;
        let pane_h = area.h * surface_h;
        // The cell grid sits inset by the content padding; the base fill covers the full
        // pane, so the padding shows as a quiet margin in the pane's own background.
        let pad = self.content_pad();
        let origin_x = pane_x + pad;
        let origin_y = pane_y + pad;
        let cw = self.metrics.width;
        let ch = self.metrics.height;

        // Fill the whole pane with the default background first.
        bg.push(BgInstance {
            pos: [pane_x, pane_y],
            size: [pane_w, pane_h],
            color: linear_rgba(snap.default_bg, 1.0),
            radius: corner_radius,
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
                radius: 0.0,
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
                        radius: 0.0,
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
                    radius: 0.0,
                }),
                CursorShape::Beam => bg.push(BgInstance {
                    pos: [cx, cy],
                    size: [2.0, ch],
                    color,
                    radius: 0.0,
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

    /// Turn the window chrome (tab bar / caption strip) into background + glyph instances.
    /// Rectangles carry their own corner radius (rounded tabs, square buttons); text runs
    /// are laid out one cell width per character, mirroring [`build_pane`]'s glyph path.
    fn build_chrome(
        &mut self,
        chrome: &Chrome,
        bg: &mut Vec<BgInstance>,
        glyphs: &mut Vec<GlyphInstance>,
    ) {
        for r in &chrome.rects {
            bg.push(BgInstance {
                pos: [r.x, r.y],
                size: [r.w, r.h],
                color: linear_rgba(r.color, 1.0),
                radius: r.radius,
            });
        }

        let px = self.font_px();
        let cw = self.metrics.width;
        for text in &chrome.texts {
            let scale = if text.scale > 0.0 { text.scale } else { 1.0 };
            // UI runs render in the proportional UI face at the chrome's own point size
            // and advance by real glyph widths; mono runs render in the grid font and
            // advance one (scaled) cell per character.
            let tpx = if text.ui { self.ui_px() } else { px } * scale;
            let mut pen_x = text.x;
            let color = linear_rgba(text.color, 1.0);
            for c in text.text.chars() {
                if c != ' ' && c != '\0' {
                    let info = if text.ui {
                        self.atlas.glyph_from(
                            &self.queue,
                            &self.faces,
                            self.ui_face,
                            tpx,
                            c,
                            text.bold,
                        )
                    } else {
                        self.atlas
                            .glyph(&self.queue, &self.faces, tpx, c, text.bold, false)
                    };
                    if info.width > 0.0 && info.height > 0.0 {
                        glyphs.push(GlyphInstance {
                            pos: [pen_x + info.left, text.baseline_y - info.top],
                            size: [info.width, info.height],
                            uv_min: info.uv_min,
                            uv_max: info.uv_max,
                            color,
                        });
                    }
                }
                pen_x += if text.ui {
                    self.ui_char_advance(c, scale)
                } else {
                    cw * scale
                };
            }
        }
    }

    /// Physical-pixel border thickness, derived from the DPI scale (1px at scale 1,
    /// 2px at scale 2, …). Focused panes double this for a clearer highlight.
    fn border_thickness(&self) -> f32 {
        (self.scale_factor.round() as f32).max(1.0)
    }

    /// Corner radius of the floating overlay's body (physical px).
    fn overlay_radius(&self) -> f32 {
        10.0 * self.scale_factor as f32
    }

    /// The floating overlay's depth treatment, drawn *under* its body: a translucent scrim
    /// dimming the whole base layer, a soft drop shadow (stacked translucent rounded
    /// quads), and a hairline border ring the rounded body then fills all but 1px of.
    fn build_overlay_decor(
        &self,
        overlay: &PaneView,
        surface_w: f32,
        surface_h: f32,
        bg: &mut Vec<BgInstance>,
    ) {
        let x = overlay.area.x * surface_w;
        let y = overlay.area.y * surface_h;
        let w = overlay.area.w * surface_w;
        let h = overlay.area.h * surface_h;
        let s = self.scale_factor as f32;
        let radius = self.overlay_radius();

        // Scrim: dim everything behind the overlay so it clearly owns the focus.
        bg.push(BgInstance {
            pos: [0.0, 0.0],
            size: [surface_w, surface_h],
            color: [0.0, 0.0, 0.0, 0.45],
            radius: 0.0,
        });

        // Drop shadow: three stacked translucent rounded quads, growing outward and
        // shifted slightly down, approximate a soft gaussian falloff.
        for (grow, drop, alpha) in [(18.0, 6.0, 0.05), (10.0, 4.0, 0.10), (4.0, 2.0, 0.16)] {
            let g = grow * s;
            bg.push(BgInstance {
                pos: [x - g, y - g + drop * s],
                size: [w + 2.0 * g, h + 2.0 * g],
                color: [0.0, 0.0, 0.0, alpha],
                radius: radius + g,
            });
        }

        // Hairline border: a rounded ring one physical px proud of the body on every side.
        let t = self.border_thickness();
        bg.push(BgInstance {
            pos: [x - t, y - t],
            size: [w + 2.0 * t, h + 2.0 * t],
            color: [1.0, 1.0, 1.0, 0.14],
            radius: radius + t,
        });
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
            // A lone pane gets no frame at all, so the content reads as one clean surface
            // flowing up into the tab bar (like Windows Terminal). Splits still show focus:
            // an accent frame on the focused pane, dim separators between the rest.
            if single {
                continue;
            }
            let (color, thickness) = if pane.focused {
                (BORDER_ACCENT, t * 2.0)
            } else {
                (BORDER_DIM, t)
            };
            push_frame(bg, x, y, w, h, thickness, color);
        }
    }
}
