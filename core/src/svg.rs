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
/// size when `long_edge` is None. Returns RGB8 pixels and their dimensions.
///
/// The picture is drawn onto white. JPEG has no alpha, and a drawing on a
/// transparent background composited onto black comes out looking like a
/// mistake. Filling the pixmap first and letting the renderer blend into it is
/// also the only correct way to do it here: a `tiny_skia` pixmap holds
/// premultiplied colour, so applying the straight-alpha formula afterwards
/// multiplies by alpha a second time and greys every antialiased edge.
pub fn rasterize(
    bytes: &[u8],
    long_edge: Option<u32>,
    max_pixels: usize,
) -> Result<(Vec<u8>, usize, usize), crate::Error> {
    let mut opt = usvg::Options::default();
    opt.fontdb = get_fontdb();
    // An SVG can name a file and have the renderer fetch it: `<image
    // href="file:///…">` is resolved from disk by the default resolver. A
    // program whose purpose is removing what a picture discloses must not be
    // talked into reading the filesystem by the picture it was handed, so only
    // data URLs are honoured and a path resolves to nothing.
    opt.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
        resolve_string: Box::new(|_href, _opts| None),
    };

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
    pixmap.fill(tiny_skia::Color::WHITE);

    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // Every pixel is opaque now, so the fourth byte carries no information and
    // premultiplication is the identity. Dropping it is all that is left.
    let rgb = pixmap
        .take()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    Ok((rgb, target_w, target_h))
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
        let center = (25 * w + 50) * 3;
        let (r, g, b) = (rgba[center], rgba[center + 1], rgba[center + 2]);

        // #123456 -> R=0x12 (18), G=0x34 (52), B=0x56 (86)
        assert!((r as i32 - 0x12).abs() <= 2, "r: {r}");
        assert!((g as i32 - 0x34).abs() <= 2, "g: {g}");
        assert!((b as i32 - 0x56).abs() <= 2, "b: {b}");
        assert_eq!(rgba.len(), w * h * 3);
    }

    /// A half-transparent white square on nothing. Composited onto white it
    /// stays white; the straight-alpha formula applied to premultiplied pixels
    /// returns about 191, and every antialiased edge in every drawing carries
    /// the same error.
    #[test]
    fn a_half_transparent_white_fill_stays_white() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
            <rect width="40" height="40" fill="#ffffff" fill-opacity="0.5"/>
        </svg>"##;
        let (rgb, w, _h) = rasterize(svg, None, 10_000_000).expect("rasterize succeeds");
        let centre = (20 * w + 20) * 3;
        for c in 0..3 {
            assert_eq!(rgb[centre + c], 255, "channel {c} was {}", rgb[centre + c]);
        }
    }

    /// The renderer must not fetch a file because the drawing asked it to.
    #[test]
    fn a_document_cannot_make_the_renderer_read_a_file() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
            <rect width="20" height="20" fill="#ffffff"/>
            <image href="file:///etc/hosts" width="20" height="20"/>
        </svg>"##;
        let (rgb, w, _h) = rasterize(svg, None, 10_000_000).expect("the drawing still renders");
        // Nothing was fetched, so the white rectangle is all there is.
        let centre = (10 * w + 10) * 3;
        assert_eq!(&rgb[centre..centre + 3], &[255, 255, 255]);
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
