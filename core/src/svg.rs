//! SVG parsing and rasterization.
//!
//! Squint does not encode SVG; an SVG source is rasterized to pixels and encoded
//! beside the original as JPEG.

use std::sync::{Arc, OnceLock};
use resvg::usvg::{self, fontdb};
use resvg::tiny_skia;

static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

fn get_fontdb() -> Arc<fontdb::Database> {
    FONTS.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    }).clone()
}

/// Check whether the input bytes look like an SVG document.
///
/// Returns true if `<svg` appears within the first 1 KiB, after skipping any
/// UTF-8 BOM, leading whitespace, XML declarations (`<?xml ... ?>`), and DOCTYPEs.
/// Note: gzip-compressed SVGs (`.svgz`) are not handled and return false.
pub fn is_svg(bytes: &[u8]) -> bool {
    let window = if bytes.len() > 1024 {
        &bytes[..1024]
    } else {
        bytes
    };

    let mut slice = window;

    // Skip UTF-8 BOM if present
    if slice.starts_with(&[0xEF, 0xBB, 0xBF]) {
        slice = &slice[3..];
    }

    loop {
        // Skip leading whitespace
        while let Some(&b) = slice.first() {
            if b.is_ascii_whitespace() {
                slice = &slice[1..];
            } else {
                break;
            }
        }

        if slice.starts_with(b"<?xml") {
            if let Some(pos) = slice.windows(2).position(|w| w == b"?>") {
                slice = &slice[pos + 2..];
                continue;
            } else {
                return false;
            }
        }

        if slice.starts_with(b"<!DOCTYPE") || slice.starts_with(b"<!doctype") {
            if let Some(pos) = slice.iter().position(|&b| b == b'>') {
                slice = &slice[pos + 1..];
                continue;
            } else {
                return false;
            }
        }

        // Also skip comments <!-- ... --> if any occur before <svg
        if slice.starts_with(b"<!--") {
            if let Some(pos) = slice.windows(3).position(|w| w == b"-->") {
                slice = &slice[pos + 3..];
                continue;
            } else {
                return false;
            }
        }

        break;
    }

    // Check for <svg tag start (either <svg> or <svg with whitespace or attributes)
    if slice.starts_with(b"<svg") {
        if slice.len() == 4 {
            return true;
        }
        let next = slice[4];
        if next.is_ascii_whitespace() || next == b'>' || next == b'/' {
            return true;
        }
    }

    false
}

/// Rasterize at `long_edge` pixels on the longer side, or the document's own
/// size when `long_edge` is None. Returns RGBA8 pixels and their dimensions.
pub fn rasterize(
    bytes: &[u8],
    long_edge: Option<u32>,
    max_pixels: usize,
) -> Result<(Vec<u8>, usize, usize), crate::Error> {
    let mut opt = usvg::Options::default();
    opt.fontdb = get_fontdb();

    let tree = usvg::Tree::from_data(bytes, &opt)
        .map_err(|e| crate::Error::Decode(e.to_string()))?;

    let doc_w = tree.size().width();
    let doc_h = tree.size().height();

    let longer_doc = doc_w.max(doc_h);

    let (scale, target_w, target_h) = match long_edge {
        Some(cap) if cap > 0 => {
            let cap_f = cap as f32;
            if longer_doc > cap_f {
                let s = cap_f / longer_doc;
                let w = (doc_w * s).round().max(1.0) as usize;
                let h = (doc_h * s).round().max(1.0) as usize;
                (s, w, h)
            } else {
                // Never enlarging past scale factor of 1 if document is smaller than cap
                let w = doc_w.round().max(1.0) as usize;
                let h = doc_h.round().max(1.0) as usize;
                (1.0, w, h)
            }
        }
        _ => {
            let w = doc_w.round().max(1.0) as usize;
            let h = doc_h.round().max(1.0) as usize;
            (1.0, w, h)
        }
    };

    // Check width * height > max_pixels BEFORE allocating pixmap
    if target_w.saturating_mul(target_h) > max_pixels {
        return Err(crate::Error::TooLarge {
            width: target_w,
            height: target_h,
        });
    }

    let mut pixmap = tiny_skia::Pixmap::new(target_w as u32, target_h as u32)
        .ok_or_else(|| crate::Error::Decode("failed to allocate pixmap".into()))?;

    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // Return RGBA8 as resvg produces it. Do not premultiply or convert.
    let data = pixmap.take();
    Ok((data, target_w, target_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rasterize_rectangle_and_color() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <rect width="200" height="100" fill="#123456"/>
        </svg>"##;

        // Long edge 100 -> should scale 200x100 down to 100x50
        let (rgba, w, h) = rasterize(svg, Some(100), 10_000_000).expect("rasterize succeeds");
        assert_eq!(w, 100);
        assert_eq!(h, 50);

        // Check pixel at center (50, 25)
        let center_idx = (25 * w + 50) * 4;
        let r = rgba[center_idx];
        let g = rgba[center_idx + 1];
        let b = rgba[center_idx + 2];
        let a = rgba[center_idx + 3];

        // #123456 -> R=0x12 (18), G=0x34 (52), B=0x56 (86), A=255
        assert!((r as i32 - 0x12).abs() <= 2, "r: {r}");
        assert!((g as i32 - 0x34).abs() <= 2, "g: {g}");
        assert!((b as i32 - 0x56).abs() <= 2, "b: {b}");
        assert_eq!(a, 255);
    }

    #[test]
    fn test_svg_too_large() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000">
            <rect width="100000" height="100000" fill="blue"/>
        </svg>"##;

        match rasterize(svg, None, crate::MAX_PIXELS) {
            Err(crate::Error::TooLarge { width, height }) => {
                assert_eq!(width, 100_000);
                assert_eq!(height, 100_000);
            }
            res => panic!("expected Error::TooLarge, got {res:?}"),
        }
    }
}
