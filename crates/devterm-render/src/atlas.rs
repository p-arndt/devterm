//! The glyph coverage atlas: an `R8Unorm` texture filled on demand by a simple shelf
//! packer, plus its cache keys and per-glyph placement records.
//!
//! Glyphs are rasterized from swash outlines against the resolved fallback face and packed
//! into free shelf slots; callers get back UV placement they can turn into textured quads.

use std::collections::HashMap;

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Angle, Transform, Vector};

use crate::font::{FontFace, select_face};

/// Atlas key: a character in one of the four bold/italic styling combinations,
/// tagged with the index (into the font-fallback chain) of the face that owns it.
/// The face is part of the key so the same codepoint drawn from two different
/// faces never collides in the cache.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    c: char,
    bold: bool,
    italic: bool,
    face: usize,
}

/// Placement + UV of a rasterized glyph inside the atlas (all in physical px / [0,1] uv).
#[derive(Clone, Copy)]
pub(crate) struct GlyphInfo {
    pub(crate) uv_min: [f32; 2],
    pub(crate) uv_max: [f32; 2],
    /// Bitmap left offset from the pen position.
    pub(crate) left: f32,
    /// Bitmap top offset above the baseline.
    pub(crate) top: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// Coverage atlas: an `R8Unorm` texture filled on demand by a simple shelf packer.
pub(crate) struct Atlas {
    texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    size: u32,
    shelf_x: u32,
    shelf_y: u32,
    shelf_height: u32,
    glyphs: HashMap<GlyphKey, GlyphInfo>,
    /// Resolved fallback-chain index for each character seen so far. Face selection
    /// depends only on the codepoint (bold/italic are synthetic), so this is keyed on
    /// `char` and keeps face lookup O(1) after the first sighting.
    face_of: HashMap<char, usize>,
    scale_ctx: ScaleContext,
}

impl Atlas {
    const SIZE: u32 = 1024;

    pub(crate) fn new(device: &wgpu::Device) -> Self {
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
            face_of: HashMap::new(),
            scale_ctx: ScaleContext::new(),
        }
    }

    /// Drop all cached glyphs and reset the packer (used when the pixel size changes).
    /// The texture is reused; stale texels are never sampled because their UVs are gone.
    pub(crate) fn reset(&mut self) {
        self.shelf_x = 0;
        self.shelf_y = 0;
        self.shelf_height = 0;
        self.glyphs.clear();
    }

    /// Resolve which face in `faces` should render `c`, caching the result.
    ///
    /// Picks the first face whose charmap maps `c` to a non-zero glyph id; if no face
    /// maps it, falls back to the primary (index 0), which renders `notdef` — the same
    /// tofu box as before the fallback chain existed.
    fn resolve_face(&mut self, faces: &[FontFace], c: char) -> usize {
        if let Some(&face) = self.face_of.get(&c) {
            return face;
        }
        let face = select_face(faces, c);
        self.face_of.insert(c, face);
        face
    }

    /// Return the atlas entry for `(c, bold, italic)`, resolving the fallback face,
    /// then rasterizing and packing on first use.
    pub(crate) fn glyph(
        &mut self,
        queue: &wgpu::Queue,
        faces: &[FontFace],
        px: f32,
        c: char,
        bold: bool,
        italic: bool,
    ) -> GlyphInfo {
        let face = self.resolve_face(faces, c);
        let key = GlyphKey {
            c,
            bold,
            italic,
            face,
        };
        if let Some(info) = self.glyphs.get(&key) {
            return *info;
        }
        let info = self.rasterize(queue, faces, px, key).unwrap_or(GlyphInfo {
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
    ///
    /// The glyph is taken from the resolved fallback face (`key.face`). Coverage is
    /// rasterized from the outline source, matching the primary face, so CJK, box-drawing
    /// and Nerd-Font symbols render sharp. Color-bitmap emoji faces (COLR/CBDT) only
    /// contribute their monochrome outline here — the atlas is `R8Unorm` coverage, so
    /// full color emoji needs an RGBA atlas and is deferred to M4.
    fn rasterize(
        &mut self,
        queue: &wgpu::Queue,
        faces: &[FontFace],
        px: f32,
        key: GlyphKey,
    ) -> Option<GlyphInfo> {
        let face = faces.get(key.face)?;
        let font = FontRef::from_index(&face.data, face.index as usize)?;
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
