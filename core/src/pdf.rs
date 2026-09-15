//! PDF, where the pictures are the file and everything else is small.
//!
//! A scanned document is a wrapper around a handful of JPEGs. Measured across
//! Ben's own files: a 14 MB scan is thirty 300 dpi JPEGs, an 11 MB product
//! sheet is one image stored with no compression at all, and a 12 MB contract
//! is a mix of scans, raw samples and fax-coded pages. The text, the vectors
//! and the structure together are a rounding error against any of that.
//!
//! So the work here is not document plumbing. Each image is lifted out, run
//! through the same perceptual search that a JPEG on its own would get, and
//! put back where it was. Nothing else in the file is touched: the page tree,
//! the content streams, the fonts and the vector art come through unchanged,
//! which is what keeps text selectable and searchable afterwards.
//!
//! An image is placed on a page by the transformation matrix in the content
//! stream, not by its own pixel count, so a smaller image drawn by the same
//! matrix covers the same area at a lower resolution. That is what makes a
//! resolution cap possible without rewriting a single content stream.

use lopdf::{Document, Object, ObjectId};

use crate::{encode_jpeg, search_as, Error, Image, Mode, OutputFormat};

/// Resolution the presets cap at, in dots per inch.
///
/// Measured on a 300 dpi scan of Ben's: at a perceptual target of 80, capping
/// here halves the file where re-encoding alone takes off an eighth. 200 dpi
/// gives back a third and 120 takes off nearly three quarters, but 120 is
/// visibly soft on scanned text and starts to cost OCR.
pub const DEFAULT_MAX_DPI: f32 = 150.0;

/// Below this many pixels on the shorter side an image is left alone.
///
/// The perceptual metric is not meaningful under `MIN_PERCEPTUAL_DIM`, and an
/// icon or a rule three pixels tall is not where a document's bytes are.
const MIN_IMAGE_DIM: usize = crate::MIN_PERCEPTUAL_DIM;

/// A ceiling on what one decompressed image may occupy.
///
/// A stream declares its own decompressed length and a file may declare
/// anything. `lopdf` will honour the declaration, so the limit is given here
/// rather than discovered when the machine starts swapping.
const MAX_IMAGE_BYTES: usize = 512 * 1024 * 1024;

/// The long side of US Letter in points, used when a page declares no box.
const FALLBACK_PAGE_LONG_PT: f32 = 792.0;

/// What a pass over a document did.
pub struct Rewritten {
    pub data: Vec<u8>,
    /// Images re-encoded, of those that were candidates.
    pub images_rewritten: usize,
    pub images_seen: usize,
    /// Entries removed from the document's own metadata.
    pub metadata_removed: usize,
}

/// Whether these bytes are a PDF.
///
/// The marker is allowed to sit a little way in: some writers put a byte order
/// mark or stray whitespace first, and a file that opens everywhere else should
/// not be refused here on a technicality.
pub fn is_pdf(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(1024)];
    window.windows(5).any(|w| w == b"%PDF-")
}

/// Filters whose contents this module will not attempt to re-encode.
///
/// `CCITTFaxDecode` is one bit per pixel, which is what a fax-mode scan of text
/// produces; a JPEG of it is both larger and worse. `JPXDecode` is JPEG 2000,
/// which nothing in this tree can read. Both are left exactly as they are.
fn is_untouchable_filter(name: &[u8]) -> bool {
    matches!(name, b"CCITTFaxDecode" | b"JPXDecode" | b"JBIG2Decode")
}

/// The last filter named on a stream, which is the one the bytes are in.
fn outermost_filter(stream: &lopdf::Stream) -> Option<Vec<u8>> {
    match stream.dict.get(b"Filter").ok()? {
        Object::Name(n) => Some(n.clone()),
        Object::Array(a) => a.last()?.as_name().ok().map(|n| n.to_vec()),
        _ => None,
    }
}

/// The longest side of any page in the document, in points.
///
/// The longest side rather than the width, because the cap is applied to a
/// picture's longest side too, and mixing the two measures a portrait page's
/// height against its width: a 300 dpi scan of a letter page came out at 113
/// dpi where 150 was asked for, a quarter more detail thrown away than intended.
///
/// Every image is measured against this one page rather than against the page
/// it sits on. Finding an image's own page means walking each page's resources,
/// and the answer only decides how much to downsample: judging a picture
/// against a page at least as large as its own estimates its resolution low, so
/// the error is always towards leaving detail alone.
fn longest_page_pt(doc: &Document) -> f32 {
    let mut longest = 0.0f32;
    for (_, id) in doc.get_pages() {
        let Ok(page) = doc.get_dictionary(id) else { continue };
        let Ok(boxed) = page.get(b"MediaBox").and_then(|o| o.as_array()) else { continue };
        if boxed.len() != 4 {
            continue;
        }
        let num = |o: &Object| o.as_float().or_else(|_| o.as_i64().map(|i| i as f32)).unwrap_or(0.0);
        let side = (num(&boxed[2]) - num(&boxed[0])).abs().max((num(&boxed[3]) - num(&boxed[1])).abs());
        if side > longest {
            longest = side;
        }
    }
    if longest > 1.0 { longest } else { FALLBACK_PAGE_LONG_PT }
}

/// How many bytes each sample carries, or `None` for a space this will not read.
///
/// A scanner names its colour space by pointing at an ICC profile rather than
/// by calling it `DeviceRGB`, and the component count then lives in `/N` on the
/// stream that profile sits in. Reading only the plain names skips exactly the
/// files most worth rewriting: an 11 MB product sheet, one uncompressed image,
/// was passed over for this reason.
///
/// CMYK is left alone deliberately. Converting it to RGB without honouring the
/// profile changes the colours, and this module has no colour management.
fn colour_components(doc: &Document, space: &Object) -> Option<usize> {
    match space {
        Object::Name(name) => match name.as_slice() {
            b"DeviceRGB" => Some(3),
            b"DeviceGray" => Some(1),
            _ => None,
        },
        Object::Array(parts) => {
            if parts.first()?.as_name().ok()? != b"ICCBased" {
                return None;
            }
            let dict = match parts.get(1)? {
                Object::Reference(id) => &doc.get_object(*id).ok()?.as_stream().ok()?.dict,
                Object::Stream(stream) => &stream.dict,
                _ => return None,
            };
            match dict.get(b"N").ok()?.as_i64().ok()? {
                1 => Some(1),
                3 => Some(3),
                _ => None,
            }
        }
        Object::Reference(id) => colour_components(doc, doc.get_object(*id).ok()?),
        _ => None,
    }
}

/// Decode one image XObject to pixels, or `None` when it is not one this reads.
fn decode_image(doc: &Document, stream: &lopdf::Stream, filter: Option<&[u8]>) -> Option<Image> {
    let width = stream.dict.get(b"Width").ok()?.as_i64().ok()? as usize;
    let height = stream.dict.get(b"Height").ok()?.as_i64().ok()? as usize;
    if width.min(height) < MIN_IMAGE_DIM || width.saturating_mul(height) > crate::MAX_PIXELS {
        return None;
    }

    match filter {
        // The stream is a JPEG exactly as it would be on disk, so it goes
        // through the same decoder every other JPEG here does.
        Some(b"DCTDecode") => Image::decode(&stream.content).ok(),
        // Raw samples, possibly deflated. Only the two colour spaces a scanner
        // produces are read; anything indexed, separated or in a colour space
        // this cannot name is left alone rather than guessed at.
        None | Some(b"FlateDecode") | Some(b"LZWDecode") | Some(b"RunLengthDecode") => {
            if stream.dict.get(b"BitsPerComponent").ok()?.as_i64().ok()? != 8 {
                return None;
            }
            let components = colour_components(doc, stream.dict.get(b"ColorSpace").ok()?)?;
            let raw = stream.decompressed_content_with_limit(MAX_IMAGE_BYTES).ok()?;
            if raw.len() < width * height * components {
                return None;
            }
            let mut rgb = Vec::with_capacity(width * height * 3);
            for px in raw.chunks_exact(components).take(width * height) {
                match components {
                    3 => rgb.extend_from_slice(px),
                    _ => rgb.extend_from_slice(&[px[0], px[0], px[0]]),
                }
            }
            Some(Image::from_rgb8(&rgb, width, height))
        }
        _ => None,
    }
}

/// Re-encode the pictures inside a PDF, and remove what the document discloses.
///
/// `max_dpi` caps resolution, judged against the widest page. `mode` decides
/// whether each image is searched to a perceptual target or encoded once at a
/// fixed quality, exactly as a standalone picture would be.
pub fn rewrite(
    bytes: &[u8],
    mode: Mode,
    target: f64,
    fixed_quality: f32,
    max_probes: usize,
    max_dpi: Option<f32>,
) -> Result<Rewritten, Error> {
    let mut doc = Document::load_mem(bytes)
        .map_err(|e| Error::Decode(format!("this PDF could not be read: {e}")))?;

    let metadata_removed = strip_document_metadata(&mut doc);

    // Strip means the pixels are not touched, in a PDF as anywhere else. The
    // document's own metadata has already gone; the pictures inside it keep
    // every byte they arrived with.
    if mode == Mode::Strip {
        let mut out = Vec::new();
        doc.save_to(&mut out)
            .map_err(|e| Error::Encode(format!("this PDF could not be written: {e}")))?;
        return Ok(Rewritten {
            data: out,
            images_rewritten: 0,
            images_seen: 0,
            metadata_removed,
        });
    }

    let page_long_in = (longest_page_pt(&doc) / 72.0).max(1.0);

    // Collected first because re-encoding borrows the document mutably, one
    // object at a time, and the walk that finds them borrows it immutably.
    let candidates: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter(|(_, object)| {
            let Ok(stream) = object.as_stream() else { return false };
            let is_image = stream
                .dict
                .get(b"Subtype")
                .and_then(|o| o.as_name())
                .map(|n| n == b"Image")
                .unwrap_or(false);
            // A stencil mask paints a shape rather than carrying a picture, and
            // is one bit per pixel.
            let is_mask = stream
                .dict
                .get(b"ImageMask")
                .and_then(|o| o.as_bool())
                .unwrap_or(false);
            is_image && !is_mask
        })
        .map(|(id, _)| *id)
        .collect();

    let mut images_rewritten = 0usize;
    let images_seen = candidates.len();

    for id in candidates {
        let Ok(object) = doc.get_object(id) else { continue };
        let Ok(stream) = object.as_stream() else { continue };
        let filter = outermost_filter(stream);
        if filter.as_deref().map(is_untouchable_filter).unwrap_or(false) {
            continue;
        }
        let original_len = stream.content.len();
        let Some(image) = decode_image(&doc, stream, filter.as_deref()) else { continue };

        // The cap is on resolution, and resolution is pixels across the page
        // rather than pixels alone.
        let long_edge = image.width.max(image.height) as f32;
        let capped = match max_dpi {
            Some(dpi) if long_edge / page_long_in > dpi => {
                Some((dpi * page_long_in).round() as u32)
            }
            _ => None,
        };
        let Some((encoded, w, h)) =
            encode_one(&image, mode, target, fixed_quality, max_probes, capped)
        else {
            continue;
        };

        // Never grow: a picture already smaller than anything this can produce
        // keeps the bytes it arrived with.
        if encoded.len() >= original_len {
            continue;
        }

        let Ok(stream) = doc.get_object_mut(id).and_then(|o| o.as_stream_mut()) else { continue };
        stream.set_content(encoded);
        stream.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        stream.dict.set("Width", Object::Integer(w as i64));
        stream.dict.set("Height", Object::Integer(h as i64));
        stream.dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        stream.dict.set("BitsPerComponent", Object::Integer(8));
        // Parameters describing how the old bytes were packed say nothing true
        // about the new ones.
        stream.dict.remove(b"DecodeParms");
        stream.dict.remove(b"DecodeParams");
        images_rewritten += 1;
    }

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| Error::Encode(format!("this PDF could not be written: {e}")))?;

    Ok(Rewritten { data: out, images_rewritten, images_seen, metadata_removed })
}

/// Draw a picture smaller, through the same filter every other cap here uses.
fn downscaled(picture: &Image, edge: u32) -> Option<Image> {
    let buffer =
        image::RgbImage::from_raw(picture.width as u32, picture.height as u32, picture.flat())?;
    let smaller = crate::capped(image::DynamicImage::ImageRgb8(buffer), Some(edge)).to_rgb8();
    let (w, h) = (smaller.width() as usize, smaller.height() as usize);
    Some(Image::from_rgb8(&smaller.into_raw(), w, h))
}

/// Encode one picture the way the mode asks for, with the size it came out as.
fn encode_one(
    image: &Image,
    mode: Mode,
    target: f64,
    fixed_quality: f32,
    max_probes: usize,
    cap: Option<u32>,
) -> Option<(Vec<u8>, usize, usize)> {
    let scaled;
    let image = match cap {
        Some(edge) => {
            scaled = downscaled(image, edge)?;
            &scaled
        }
        None => image,
    };
    let encoded = match mode {
        Mode::Quality if image.shorter_side() >= crate::MIN_PERCEPTUAL_DIM => {
            // No size to beat here: the caller compares against the stream it
            // is replacing, which is a different thing from this picture alone.
            search_as(OutputFormat::Jpeg, image, target, max_probes, None, None)
                .ok()
                .map(|r| r.data)?
        }
        _ => encode_jpeg(image, fixed_quality, None).ok()?,
    };
    Some((encoded, image.width, image.height))
}

/// Remove what the document says about itself.
///
/// The same rule the other formats follow: keep what is needed to render and
/// drop the rest by exclusion, so a producer field nobody has heard of yet
/// leaves without being named. `/Info` carries the author, the application and
/// the times the file was written; `/Metadata` is an XMP packet that usually
/// repeats all of it and often more.
fn strip_document_metadata(doc: &mut Document) -> usize {
    let mut removed = 0;

    // The trailer points at the info dictionary; emptying it is enough, and
    // leaves a valid reference for any reader that expects one.
    if let Ok(Object::Reference(id)) = doc.trailer.get(b"Info").cloned() {
        if let Ok(info) = doc.get_dictionary_mut(id) {
            removed += info.len();
            *info = lopdf::Dictionary::new();
        }
    }

    let catalogs: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter(|(_, o)| {
            o.as_dict()
                .and_then(|d| d.get(b"Type"))
                .and_then(|t| t.as_name())
                .map(|n| n == b"Catalog")
                .unwrap_or(false)
        })
        .map(|(id, _)| *id)
        .collect();
    for id in catalogs {
        if let Ok(catalog) = doc.get_dictionary_mut(id) {
            if catalog.remove(b"Metadata").is_some() {
                removed += 1;
            }
        }
    }

    removed
}

/// Pages a document declares, for the harness to report.
pub fn page_count(bytes: &[u8]) -> Option<usize> {
    Document::load_mem(bytes).ok().map(|d| d.get_pages().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    #[test]
    fn a_pdf_is_recognised_by_its_marker_wherever_it_starts() {
        assert!(is_pdf(b"%PDF-1.7\n..."));
        // Some writers put a byte order mark or stray bytes first, and the file
        // opens everywhere else, so it is not refused here on a technicality.
        assert!(is_pdf(b"\xef\xbb\xbf   %PDF-1.4 rest"));
        assert!(!is_pdf(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_pdf(b"not a pdf"));
        assert!(!is_pdf(b""));
        // The marker has to be near the front, not anywhere in a large file.
        let mut late = vec![b'x'; 4096];
        late.extend_from_slice(b"%PDF-1.7");
        assert!(!is_pdf(&late));
    }

    #[test]
    fn the_filters_that_would_come_out_worse_are_left_alone() {
        // One bit per pixel. A JPEG of fax-coded text is both larger and worse,
        // and JPEG 2000 is a format nothing in this tree can read.
        for filter in [&b"CCITTFaxDecode"[..], b"JPXDecode", b"JBIG2Decode"] {
            assert!(is_untouchable_filter(filter), "{filter:?} must be left alone");
        }
        for filter in [&b"DCTDecode"[..], b"FlateDecode", b"LZWDecode"] {
            assert!(!is_untouchable_filter(filter));
        }
    }

    #[test]
    fn the_outermost_filter_is_the_one_the_bytes_are_in() {
        let named = lopdf::Stream::new(dictionary! { "Filter" => Object::Name(b"DCTDecode".to_vec()) }, vec![]);
        assert_eq!(outermost_filter(&named).as_deref(), Some(&b"DCTDecode"[..]));

        // A chain is applied in order, so the last one named is the encoding
        // the stored bytes actually carry.
        let chained = lopdf::Stream::new(
            dictionary! { "Filter" => Object::Array(vec![
                Object::Name(b"FlateDecode".to_vec()),
                Object::Name(b"DCTDecode".to_vec()),
            ]) },
            vec![],
        );
        assert_eq!(outermost_filter(&chained).as_deref(), Some(&b"DCTDecode"[..]));

        let bare = lopdf::Stream::new(dictionary! {}, vec![]);
        assert_eq!(outermost_filter(&bare), None);
    }

    #[test]
    fn a_colour_space_named_through_a_profile_is_still_read() {
        let doc = Document::with_version("1.5");

        assert_eq!(colour_components(&doc, &Object::Name(b"DeviceRGB".to_vec())), Some(3));
        assert_eq!(colour_components(&doc, &Object::Name(b"DeviceGray".to_vec())), Some(1));
        assert_eq!(colour_components(&doc, &Object::Name(b"DeviceCMYK".to_vec())), None);

        // What a scanner actually writes: the space points at an ICC profile
        // and the component count lives on that stream. Reading only the plain
        // names skipped an 11 MB file that compressed to 183 KB once this
        // worked.
        let profile = Object::Stream(lopdf::Stream::new(dictionary! { "N" => 3 }, vec![]));
        let icc = Object::Array(vec![Object::Name(b"ICCBased".to_vec()), profile]);
        assert_eq!(colour_components(&doc, &icc), Some(3));

        let grey = Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Stream(lopdf::Stream::new(dictionary! { "N" => 1 }, vec![])),
        ]);
        assert_eq!(colour_components(&doc, &grey), Some(1));

        // Four components is CMYK. Converting it without honouring the profile
        // changes the colours, so it is left alone.
        let cmyk = Object::Array(vec![
            Object::Name(b"ICCBased".to_vec()),
            Object::Stream(lopdf::Stream::new(dictionary! { "N" => 4 }, vec![])),
        ]);
        assert_eq!(colour_components(&doc, &cmyk), None);

        // An indexed or separation space is not one this reads.
        let indexed = Object::Array(vec![Object::Name(b"Indexed".to_vec()), Object::Integer(255)]);
        assert_eq!(colour_components(&doc, &indexed), None);
    }

    #[test]
    fn a_document_stops_saying_who_made_it() {
        let mut doc = Document::with_version("1.5");
        let info = doc.add_object(dictionary! {
            "Producer" => Object::string_literal("iPhone OS 11.3 Quartz PDFContext"),
            "Author" => Object::string_literal("Ben Meadows"),
            "CreationDate" => Object::string_literal("D:20180210154820-07'00'"),
        });
        doc.trailer.set("Info", Object::Reference(info));

        let removed = strip_document_metadata(&mut doc);
        assert_eq!(removed, 3, "every entry should have gone");

        let left = doc.get_dictionary(info).expect("the dictionary survives as an empty one");
        assert!(left.is_empty(), "the info dictionary still names something");

        // Running it again finds nothing, which is how the caller tells an
        // already-clean document from one it changed.
        assert_eq!(strip_document_metadata(&mut doc), 0);
    }
}
