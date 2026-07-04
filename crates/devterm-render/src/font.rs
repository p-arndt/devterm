//! Font discovery and the primary-plus-fallback chain, plus cell-metric computation.
//!
//! The primary face (index 0 of the chain) drives the monospace grid metrics; the rest
//! only supply glyphs the primary is missing (CJK, Nerd-Font symbols, box-drawing, emoji).

use swash::FontRef;

use crate::CellMetrics;

/// One font face in the fallback chain: raw file bytes plus the face index within
/// that file (a collection like a `.ttc` holds several faces). The primary face
/// (index 0 of the chain) drives the monospace grid metrics; the rest only supply
/// glyphs the primary is missing.
pub(crate) struct FontFace {
    pub(crate) data: Vec<u8>,
    pub(crate) index: u32,
}

/// Resolve the id of the primary monospace face from a loaded database, trying a
/// preferred chain first and finally any face the system flags as monospaced.
fn resolve_primary(db: &fontdb::Database) -> Option<fontdb::ID> {
    let candidates = [
        // Windows / bundled developer fonts.
        fontdb::Family::Name("Cascadia Mono"),
        fontdb::Family::Name("Consolas"),
        fontdb::Family::Name("JetBrains Mono"),
        // Common Linux monospace families.
        fontdb::Family::Name("DejaVu Sans Mono"),
        fontdb::Family::Name("Liberation Mono"),
        fontdb::Family::Name("Ubuntu Mono"),
        fontdb::Family::Name("Noto Sans Mono"),
        // macOS.
        fontdb::Family::Name("Menlo"),
        fontdb::Family::Name("SF Mono"),
        // Generic alias (fontdb maps this to its configured monospace family,
        // e.g. "Courier New", which may be absent on minimal Linux installs).
        fontdb::Family::Monospace,
    ];

    for family in candidates {
        let query = fontdb::Query {
            families: &[family],
            ..Default::default()
        };
        if let Some(id) = db.query(&query) {
            return Some(id);
        }
    }

    // Last resort: any installed face the system flags as monospaced. This keeps
    // startup working when none of the named families exist and the generic
    // `Monospace` alias points at an absent font.
    db.faces().find(|face| face.monospaced).map(|face| face.id)
}

/// Copy one face's raw bytes + index out of the database.
fn load_face(db: &fontdb::Database, id: fontdb::ID) -> Option<FontFace> {
    db.with_face_data(id, |data, index| FontFace {
        data: data.to_vec(),
        index,
    })
}

/// Load a monospace font's raw data + face index, trying a preferred chain first.
///
/// Retained as the single-face entry point (used by tests); [`load_font_faces`] builds
/// the full primary-plus-fallback chain the renderer actually uses.
#[cfg(test)]
fn load_monospace_font() -> Option<(Vec<u8>, u32)> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let id = resolve_primary(&db)?;
    db.with_face_data(id, |data, index| (data.to_vec(), index))
}

/// Build the font-fallback chain: the primary monospace face followed by whichever
/// symbol/Nerd, CJK and emoji families `fontdb` can resolve on this system.
///
/// Glyphs missing from the primary (CJK, Nerd-Font symbols, box-drawing, emoji) are
/// drawn from a later face instead of showing a tofu box. Metrics still come from the
/// primary only, so the monospace grid stays uniform. Uninstalled fallbacks are skipped;
/// with none present the chain is just the primary.
pub(crate) fn load_font_faces() -> Option<Vec<FontFace>> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let primary_id = resolve_primary(&db)?;
    let mut faces = vec![load_face(&db, primary_id)?];

    // One preferred family list per fallback category; the first that resolves wins.
    let fallback_groups: [&[&str]; 3] = [
        // Symbols / Nerd Font (powerline, box-drawing, dev icons).
        &[
            "Symbols Nerd Font Mono",
            "Symbols Nerd Font",
            "Noto Sans Symbols 2",
        ],
        // CJK.
        &[
            "Noto Sans CJK SC",
            "Noto Sans CJK JP",
            "Microsoft YaHei",
            "MS Gothic",
            "Source Han Sans SC",
            "Source Han Sans",
        ],
        // Emoji (renders monochrome here; see `Atlas::rasterize`).
        &["Noto Color Emoji", "Segoe UI Emoji", "Apple Color Emoji"],
    ];

    for group in fallback_groups {
        for name in group {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                ..Default::default()
            };
            if let Some(id) = db.query(&query)
                && id != primary_id
                && let Some(face) = load_face(&db, id)
            {
                faces.push(face);
                break;
            }
        }
    }

    Some(faces)
}

/// Index of the first face in `faces` whose charmap maps `c` to a non-zero glyph id.
/// Returns 0 (the primary) when no face maps `c`, so the primary renders `notdef`.
pub(crate) fn select_face(faces: &[FontFace], c: char) -> usize {
    let maps: Vec<bool> = faces
        .iter()
        .map(|face| {
            FontRef::from_index(&face.data, face.index as usize)
                .is_some_and(|font| font.charmap().map(c) != 0)
        })
        .collect();
    first_mapping_face(&maps)
}

/// Pure selection rule: the index of the first `true`, or 0 when all are `false`.
/// Split out from [`select_face`] so it can be unit-tested without any font files.
fn first_mapping_face(maps: &[bool]) -> usize {
    maps.iter().position(|&m| m).unwrap_or(0)
}

/// Compute cell metrics and the top-to-baseline distance for a given pixel font size.
pub(crate) fn compute_metrics(font_data: &[u8], font_index: u32, px: f32) -> (CellMetrics, f32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer aborts startup if no monospace font resolves, so font discovery
    /// must succeed on any system that has at least one monospaced face installed.
    /// This regressions the Linux case where only the generic/absent families matched.
    #[test]
    fn resolves_a_monospace_font() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let has_any_mono = db.faces().any(|face| face.monospaced);

        match load_monospace_font() {
            Some((data, _index)) => assert!(!data.is_empty(), "loaded font data was empty"),
            None => assert!(
                !has_any_mono,
                "a monospaced face is installed but load_monospace_font() returned None",
            ),
        }
    }

    /// The fallback chain always starts with a non-empty primary face when any
    /// monospaced font exists on the system.
    #[test]
    fn font_chain_has_primary_first() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let has_any_mono = db.faces().any(|face| face.monospaced);

        match load_font_faces() {
            Some(faces) => {
                assert!(!faces.is_empty(), "chain must contain at least the primary");
                assert!(!faces[0].data.is_empty(), "primary face data was empty");
            }
            None => assert!(
                !has_any_mono,
                "a monospaced face is installed but load_font_faces() returned None",
            ),
        }
    }

    /// The pure selection rule: first mapping face wins, primary (0) when none map.
    #[test]
    fn first_mapping_face_rules() {
        // Only the primary maps the char -> pick the primary.
        assert_eq!(first_mapping_face(&[true, false, false]), 0);
        // Primary lacks it but a later fallback maps it -> pick the fallback.
        assert_eq!(first_mapping_face(&[false, true, false]), 1);
        assert_eq!(first_mapping_face(&[false, false, true]), 2);
        // The earliest mapping face wins when several map it.
        assert_eq!(first_mapping_face(&[false, true, true]), 1);
        // No face maps it -> fall back to the primary (renders notdef).
        assert_eq!(first_mapping_face(&[false, false, false]), 0);
        // Degenerate empty chain -> primary index 0.
        assert_eq!(first_mapping_face(&[]), 0);
    }

    /// Against the real loaded chain, a plain ASCII letter must resolve to the primary
    /// (every monospace face maps it), exercising `select_face` end to end.
    #[test]
    fn select_face_prefers_primary_for_ascii() {
        let Some(faces) = load_font_faces() else {
            return; // no fonts installed; nothing to assert
        };
        assert_eq!(select_face(&faces, 'A'), 0);
        assert_eq!(select_face(&faces, 'x'), 0);
    }
}
