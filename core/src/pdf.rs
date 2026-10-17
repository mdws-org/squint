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

use std::collections::BTreeSet;

use lopdf::{Document, Object, ObjectId};

use crate::{encode_jpeg, metadata, search_as, Error, Image, Mode, OutputFormat};

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

/// A page's `/MediaBox`, its own or the one it inherits from the page tree.
///
/// The box is one of the entries a page may leave to an ancestor, and a writer
/// whose pages are all one size tends to declare it once on the root `/Pages`
/// node and nowhere else: Ghostscript, LibreOffice and Word's export all do.
/// Read off the leaf alone, such a document has no page size at all.
fn media_box<'a>(doc: &'a Document, page: ObjectId) -> Option<&'a Vec<Object>> {
    let mut node = page;
    // A page tree is a few levels deep; the bound is against a `/Parent` cycle.
    for _ in 0..64 {
        let dict = doc.get_dictionary(node).ok()?;
        if let Ok(boxed) = dict.get(b"MediaBox") {
            return doc.dereference(boxed).ok()?.1.as_array().ok();
        }
        node = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
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
        let Some(boxed) = media_box(doc, id) else { continue };
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

/// The image XObjects that carry a picture, in the order they sit in the file.
///
/// Two kinds of image are not pictures. A stencil mask (`/ImageMask true`)
/// paints a shape and is one bit per pixel. A soft mask is another image's
/// alpha channel: a reader takes it as single-channel `DeviceGray` whatever
/// its dictionary says, so handing it back as a three-channel JPEG paints the
/// wrong opacity over the picture it belongs to. Any stream another image
/// names as its `/SMask` or `/Mask` is therefore left with the bytes it
/// arrived with, however it happens to be encoded.
fn image_candidates(doc: &Document) -> Vec<ObjectId> {
    let mut masks: BTreeSet<ObjectId> = BTreeSet::new();
    let mut pictures = Vec::new();
    for (id, object) in doc.objects.iter() {
        let Ok(stream) = object.as_stream() else { continue };
        let is_image = stream
            .dict
            .get(b"Subtype")
            .and_then(|o| o.as_name())
            .map(|n| n == b"Image")
            .unwrap_or(false);
        if !is_image {
            continue;
        }
        for key in [&b"SMask"[..], b"Mask"] {
            if let Ok(Object::Reference(mask)) = stream.dict.get(key) {
                masks.insert(*mask);
            }
        }
        let is_stencil = stream
            .dict
            .get(b"ImageMask")
            .and_then(|o| o.as_bool())
            .unwrap_or(false);
        if !is_stencil {
            pictures.push(*id);
        }
    }
    pictures.retain(|id| !masks.contains(id));
    pictures
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

    let metadata_removed =
        strip_document_metadata(&mut doc) + strip_image_metadata(&mut doc, mode == Mode::Strip)?;

    // Strip means the pixels are not touched, in a PDF as anywhere else. The
    // document's own metadata has already gone, and so has the EXIF inside its
    // JPEG pictures; their scans keep every byte they arrived with.
    if mode == Mode::Strip {
        // An object nothing refers to any more is still written out by the
        // save, so a removed reference would leave its target (an annotation's
        // author, an editor's private data) in the file.
        doc.prune_objects();
        let mut out = Vec::new();
        doc.save_modern(&mut out)
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
    let candidates = image_candidates(&doc);

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
        // A space that names a profile over three components still describes
        // the samples after re-encoding, because nothing here converts colour:
        // the profile stays with the picture, which is the rule for every
        // other format. A single-component source comes back as three, and a
        // profile for one channel says nothing true about those.
        let keeps_space = stream
            .dict
            .get(b"ColorSpace")
            .ok()
            .and_then(|space| colour_components(&doc, space))
            == Some(3);
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
        if !keeps_space {
            stream.dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        }
        stream.dict.set("BitsPerComponent", Object::Integer(8));
        // Parameters describing how the old bytes were packed say nothing true
        // about the new ones.
        stream.dict.remove(b"DecodeParms");
        stream.dict.remove(b"DecodeParams");
        images_rewritten += 1;
    }

    // Written the way a current producer writes: the small objects packed
    // into object streams and the cross-reference table as a stream. Written
    // one object at a time instead, every real document tried came back
    // larger than it arrived even with its metadata gone, and the never-grow
    // rule then refused all of them.
    let mut out = Vec::new();
    doc.save_modern(&mut out)
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
/// repeats all of it and often more, and sits on the catalog, then again on
/// pages, pictures and fonts that were made by a tool that signs its work.
///
/// An XMP packet is a stream of its own that a `/Metadata` entry points at,
/// and `save_to` writes every stream the document holds whether anything
/// still points at it or not. Taking the entry off the dictionary leaves the
/// packet's plain text in the file for anyone with a text editor, so the
/// stream goes from the object table as well.
fn strip_document_metadata(doc: &mut Document) -> usize {
    let mut removed = 0;

    // The trailer usually points at the info dictionary; emptying it is enough,
    // and leaves a valid reference for any reader that expects one. A writer
    // may also put the dictionary in the trailer itself, and that copy is
    // emptied in place.
    match doc.trailer.get(b"Info").cloned() {
        Ok(Object::Reference(id)) => {
            if let Ok(info) = doc.get_dictionary_mut(id) {
                removed += info.len();
                *info = lopdf::Dictionary::new();
            }
        }
        Ok(Object::Dictionary(info)) => {
            removed += info.len();
            doc.trailer.set("Info", lopdf::Dictionary::new());
        }
        _ => {}
    }

    let mut packets: BTreeSet<ObjectId> = BTreeSet::new();
    for (id, object) in doc.objects.iter_mut() {
        let dict = match object {
            Object::Dictionary(dict) => dict,
            Object::Stream(stream) => &mut stream.dict,
            _ => continue,
        };
        // A packet nothing points at any more, left behind by an earlier
        // writer, is still in the file and still says what it says.
        let is_packet = dict
            .get(b"Type")
            .and_then(|t| t.as_name())
            .map(|n| n == b"Metadata")
            .unwrap_or(false);
        if is_packet {
            packets.insert(*id);
        }
        if let Some(entry) = dict.remove(b"Metadata") {
            removed += 1;
            if let Ok(target) = entry.as_reference() {
                packets.insert(target);
            }
        }
    }
    for id in packets {
        doc.delete_object(id);
    }

    // A PDF 1.5 cross-reference stream is an object whose dictionary is the
    // trailer, info dictionary included when the writer put it there directly.
    // lopdf loads it like any other object and writes it back out beside the
    // fresh cross-reference it builds on save, so the old trailer, author and
    // all, would ride along. It describes the file as it arrived and nothing
    // reads it once the document is rewritten.
    let stale_xref: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter(|(_, o)| {
            o.as_stream().ok().and_then(|s| s.dict.get(b"Type").ok()).and_then(|t| t.as_name().ok())
                == Some(&b"XRef"[..])
        })
        .map(|(id, _)| *id)
        .collect();
    for id in stale_xref {
        doc.objects.remove(&id);
    }

    removed += strip_annotations_and_piece_info(doc);
    refresh_file_id(doc);
    removed
}

/// Who wrote each comment and when, and an application's private editing
/// data. An annotation's /T names its author and /M and /CreationDate date it;
/// its /Contents is the comment itself and stays. /PieceInfo, on a page or the
/// catalog, is where Illustrator, Acrobat and others keep their own data, and
/// /LastModified dates the page. A page may hold its annotations in an array
/// of its own, through a reference to one, or as dictionaries in the array.
fn strip_annotations_and_piece_info(doc: &mut Document) -> usize {
    let mut removed = 0;
    let mut annotations: BTreeSet<ObjectId> = BTreeSet::new();
    let mut arrays: BTreeSet<ObjectId> = BTreeSet::new();
    for object in doc.objects.values_mut() {
        let dict = match object {
            Object::Dictionary(dict) => dict,
            Object::Stream(stream) => &mut stream.dict,
            _ => continue,
        };
        for key in [&b"PieceInfo"[..], b"LastModified"] {
            removed += usize::from(dict.remove(key).is_some());
        }
        match dict.get_mut(b"Annots") {
            Ok(Object::Array(items)) => removed += scrub_annotation_array(items, &mut annotations),
            Ok(Object::Reference(id)) => {
                arrays.insert(*id);
            }
            _ => {}
        }
    }
    for id in arrays {
        if let Ok(Object::Array(items)) = doc.get_object_mut(id) {
            removed += scrub_annotation_array(items, &mut annotations);
        }
    }
    for id in annotations {
        if let Ok(annot) = doc.get_dictionary_mut(id) {
            removed += scrub_annotation(annot);
        }
    }
    removed
}

/// Scrub the annotations held in an /Annots array itself, and collect the ones
/// it refers to.
fn scrub_annotation_array(items: &mut [Object], referred: &mut BTreeSet<ObjectId>) -> usize {
    let mut removed = 0;
    for item in items {
        match item {
            Object::Reference(id) => {
                referred.insert(*id);
            }
            Object::Dictionary(annot) => removed += scrub_annotation(annot),
            _ => {}
        }
    }
    removed
}

/// Remove an annotation's author and dates. A form field's widget keeps its
/// /T: there it is the field's name, which the form is filled, scripted and
/// submitted by, not a person.
fn scrub_annotation(annot: &mut lopdf::Dictionary) -> usize {
    let is_field = annot.has(b"FT")
        || annot.get(b"Subtype").and_then(|s| s.as_name()).map(|n| n == b"Widget").unwrap_or(false);
    let mut removed = 0;
    for key in [&b"M"[..], b"CreationDate"] {
        removed += usize::from(annot.remove(key).is_some());
    }
    if !is_field {
        removed += usize::from(annot.remove(b"T").is_some());
    }
    removed
}

/// Replace the trailer's /ID, which ties this file to every other copy and
/// version of the same document, with a fresh one. An encrypted document's
/// keys are derived from its /ID, so it is left alone there. Not counted as
/// metadata removed: nearly every PDF has one, and counting it would make a
/// second Strip report something removed every time.
fn refresh_file_id(doc: &mut Document) {
    if doc.trailer.has(b"Encrypt") || !doc.trailer.has(b"ID") {
        return;
    }
    use std::hash::{BuildHasher, Hasher};
    let random = std::collections::hash_map::RandomState::new();
    let mut id = Vec::with_capacity(16);
    for half in 0..2u8 {
        let mut h = random.build_hasher();
        h.write_u8(half);
        id.extend_from_slice(&h.finish().to_le_bytes());
    }
    let fresh = Object::String(id, lopdf::StringFormat::Hexadecimal);
    doc.trailer.set("ID", Object::Array(vec![fresh.clone(), fresh]));
}

/// Remove EXIF, XMP and the other application segments from the JPEG pictures
/// inside a document, the way a JPEG Strip does, leaving each scan byte for
/// byte. A scanner or img2pdf stores a photograph as its whole JPEG file, GPS
/// and camera serial included; ReportLab and qpdf wrap it in ASCII or Flate
/// first. Outer filters are undone and the stripped JPEG is stored bare.
///
/// With `strict`, which Strip asks for, a picture whose filters cannot be
/// undone or whose JPEG does not walk refuses the document, because what it
/// carries cannot be shown to be gone. Returns the number of pictures that
/// lost metadata.
fn strip_image_metadata(doc: &mut Document, strict: bool) -> Result<usize, Error> {
    let unreadable = || Error::Decode("a picture inside this PDF could not be read, so its metadata cannot be shown to be removed".into());
    let mut work = Vec::new();
    for (id, object) in &doc.objects {
        let Ok(stream) = object.as_stream() else { continue };
        let is_image = stream.dict.get(b"Subtype").and_then(|o| o.as_name()).map(|n| n == b"Image").unwrap_or(false);
        if !is_image {
            continue;
        }
        let Some(filters) = filter_names(doc, &stream.dict) else { continue };
        if filters.last().map(Vec::as_slice) != Some(&b"DCTDecode"[..]) {
            continue;
        }
        work.push((*id, filters));
    }

    let mut stripped = 0;
    for (id, filters) in work {
        let Ok(stream) = doc.get_object_mut(id).and_then(Object::as_stream_mut) else { continue };
        let outer = &filters[..filters.len() - 1];
        let jpeg = if outer.is_empty() {
            stream.content.clone()
        } else {
            let mut wrapper = lopdf::Dictionary::new();
            wrapper.set("Filter", Object::Array(outer.iter().map(|n| Object::Name(n.clone())).collect()));
            if let (1, Ok(Object::Array(params))) = (outer.len(), stream.dict.get(b"DecodeParms")) {
                if let Some(Object::Dictionary(first)) = params.first() {
                    wrapper.set("DecodeParms", first.clone());
                }
            }
            match lopdf::Stream::new(wrapper, stream.content.clone()).decompressed_content_with_limit(MAX_DECODED) {
                Ok(bytes) => bytes,
                Err(_) if strict => return Err(unreadable()),
                Err(_) => continue,
            }
        };
        let clean = match crate::strip_jpeg_file(&jpeg, 1) {
            Ok(Some((clean, _))) => clean,
            Ok(None) if strict => return Err(unreadable()),
            Err(e) if strict => return Err(e),
            _ => continue,
        };
        if clean == jpeg {
            continue;
        }
        let dct_params = match stream.dict.get(b"DecodeParms") {
            Ok(Object::Array(params)) => params.last().filter(|p| p.as_dict().is_ok()).cloned(),
            Ok(params @ Object::Dictionary(_)) if outer.is_empty() => Some(params.clone()),
            _ => None,
        };
        stream.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        match dct_params {
            Some(params) => stream.dict.set("DecodeParms", params),
            None => {
                stream.dict.remove(b"DecodeParms");
            }
        }
        stream.set_content(clean);
        stripped += 1;
    }
    Ok(stripped)
}

/// Decoded size allowed for one picture's outer filters, against a stream
/// built to expand without limit.
const MAX_DECODED: usize = 512 * 1024 * 1024;

/// A stream's filters in decoding order, following references, or `None` when
/// it has none or they are not names.
fn filter_names(doc: &Document, dict: &lopdf::Dictionary) -> Option<Vec<Vec<u8>>> {
    let resolve = |o: &Object| -> Option<Object> {
        match o {
            Object::Reference(id) => doc.get_object(*id).ok().cloned(),
            other => Some(other.clone()),
        }
    };
    match resolve(dict.get(b"Filter").ok()?)? {
        Object::Name(n) => Some(vec![n]),
        Object::Array(items) => items
            .iter()
            .map(|o| resolve(o).and_then(|o| o.as_name().ok().map(<[u8]>::to_vec)))
            .collect(),
        _ => None,
    }
}

/// Pages a document declares, for the harness to report.
pub fn page_count(bytes: &[u8]) -> Option<usize> {
    Document::load_mem(bytes).ok().map(|d| d.get_pages().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn name(n: &str) -> Object {
        Object::Name(n.as_bytes().to_vec())
    }

    fn rect(w: i64, h: i64) -> Object {
        Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(w), Object::Integer(h)])
    }

    fn image(dict: lopdf::Dictionary) -> Object {
        Object::Stream(lopdf::Stream::new(dict, vec![0u8]))
    }

    /// A catalog over one page, returning the ids of the tree root and the page.
    fn one_page(doc: &mut Document, page: lopdf::Dictionary, tree: lopdf::Dictionary) -> (ObjectId, ObjectId) {
        let pages = doc.new_object_id();
        let mut page = page;
        page.set("Type", name("Page"));
        page.set("Parent", Object::Reference(pages));
        let page = doc.add_object(page);
        let mut tree = tree;
        tree.set("Type", name("Pages"));
        tree.set("Kids", Object::Array(vec![Object::Reference(page)]));
        tree.set("Count", Object::Integer(1));
        doc.objects.insert(pages, Object::Dictionary(tree));
        let catalog = doc.add_object(dictionary! { "Type" => name("Catalog"), "Pages" => Object::Reference(pages) });
        doc.trailer.set("Root", Object::Reference(catalog));
        (pages, page)
    }

    /// The smallest JPEG `strip_jpeg` walks: an EXIF segment naming a place,
    /// then a scan.
    const SCAN_ONLY: [u8; 10] = [0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xD9];

    fn jpeg_with_exif() -> Vec<u8> {
        let exif: &[u8] = b"Exif\0\0GPS 33.4484N 112.0740W";
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(exif);
        jpeg.extend_from_slice(&SCAN_ONLY[2..]);
        jpeg
    }

    /// A one-page PDF drawing one image stream stored under `filter`.
    fn pdf_with_image(filter: Object, content: Vec<u8>) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let picture = doc.add_object(Object::Stream(lopdf::Stream::new(
            dictionary! {
                "Type" => name("XObject"), "Subtype" => name("Image"), "Width" => 1, "Height" => 1,
                "ColorSpace" => name("DeviceRGB"), "BitsPerComponent" => 8, "Filter" => filter,
            },
            content,
        )));
        one_page(
            &mut doc,
            dictionary! { "Resources" => dictionary! { "XObject" => dictionary! {
                "Im0" => Object::Reference(picture) } } },
            dictionary! { "MediaBox" => rect(612, 792) },
        );
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    fn first_image_content(bytes: &[u8]) -> Vec<u8> {
        let doc = Document::load_mem(bytes).unwrap();
        doc.objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .find(|s| s.dict.get(b"Subtype").and_then(|n| n.as_name()).map(|n| n == b"Image").unwrap_or(false))
            .expect("the image is still there")
            .content
            .clone()
    }

    /// An img2pdf or scanner PDF stores each photograph as its JPEG file, EXIF
    /// and GPS included. Strip removed the document's metadata and left every
    /// picture's, while the row said the location was gone.
    #[test]
    fn strip_removes_exif_from_the_pictures_inside() {
        let bytes = pdf_with_image(name("DCTDecode"), jpeg_with_exif());
        let out = rewrite(&bytes, Mode::Strip, 0.0, 0.0, 0, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(!out.data.windows(4).any(|w| w == b"Exif"), "the picture's EXIF survived");
        assert_eq!(first_image_content(&out.data), SCAN_ONLY, "the scan must be untouched");
        assert!(out.metadata_removed >= 1, "a picture's EXIF is metadata removed");
    }

    /// The trailer may hold the info dictionary itself rather than a reference
    /// to one. Only the reference was emptied, so a direct dictionary kept the
    /// author, and a file carrying nothing else reported nothing to remove. A
    /// 1.5 file also keeps a copy of the trailer in its old cross-reference
    /// stream, which lopdf wrote back out unchanged.
    #[test]
    fn an_info_dictionary_written_into_the_trailer_is_emptied() {
        let mut doc = Document::with_version("1.5");
        one_page(&mut doc, dictionary! {}, dictionary! { "MediaBox" => rect(612, 792) });
        doc.trailer.set("Info", dictionary! { "Author" => Object::string_literal("Ben Meadows") });
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let out = rewrite(&bytes, Mode::Strip, 0.0, 0.0, 0, None).unwrap_or_else(|e| panic!("{e}"));
        assert!(!out.data.windows(11).any(|w| w == b"Ben Meadows"), "the author survived");
        assert_eq!(out.metadata_removed, 1);
    }

    /// A reviewed PDF names each commenter in the annotation's /T and dates
    /// every comment; an Illustrator or Acrobat export keeps its private
    /// editing data in /PieceInfo; and /ID ties the file to every other copy
    /// of the same document. Strip kept all of it.
    #[test]
    fn comment_authors_editing_data_and_the_file_id_go() {
        let mut doc = Document::with_version("1.5");
        let note = doc.add_object(dictionary! {
            "Type" => name("Annot"), "Subtype" => name("Text"), "Rect" => rect(10, 10),
            "T" => Object::string_literal("Ben Meadows"),
            "M" => Object::string_literal("D:20261017120000-07'00'"),
            "CreationDate" => Object::string_literal("D:20261016090000-07'00'"),
            "Contents" => Object::string_literal("move this paragraph up"),
        });
        one_page(
            &mut doc,
            dictionary! {
                "Annots" => Object::Array(vec![Object::Reference(note)]),
                "PieceInfo" => dictionary! { "Illustrator" => dictionary! { "Private" => Object::string_literal("layer history") } },
            },
            dictionary! { "MediaBox" => rect(612, 792) },
        );
        let original_id = Object::string_literal("0123456789ABCDEF");
        doc.trailer.set("ID", Object::Array(vec![original_id.clone(), original_id.clone()]));
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let out = rewrite(&bytes, Mode::Strip, 0.0, 0.0, 0, None).unwrap_or_else(|e| panic!("{e}"));
        // Read back through the parser: the writer packs plain objects into
        // compressed object streams, so a string missing from the raw bytes
        // proves nothing.
        let after = Document::load_mem(&out.data).unwrap();
        let annot = after
            .objects
            .values()
            .filter_map(|o| o.as_dict().ok())
            .find(|d| d.get(b"Subtype").and_then(|s| s.as_name()).map(|n| n == b"Text").unwrap_or(false))
            .expect("the annotation is still there");
        for key in [&b"T"[..], b"M", b"CreationDate"] {
            assert!(!annot.has(key), "/{} survived", String::from_utf8_lossy(key));
        }
        assert_eq!(annot.get(b"Contents").unwrap().as_str().unwrap(), b"move this paragraph up");
        assert!(after.objects.values().filter_map(|o| o.as_dict().ok()).all(|d| !d.has(b"PieceInfo")));
        match after.trailer.get(b"ID") {
            Ok(Object::Array(id)) => assert!(id.len() == 2 && id[0] != original_id, "the /ID was not replaced"),
            other => panic!("expected a fresh /ID, found {other:?}"),
        }
    }

    /// Whether any object in a document, strings and decoded streams
    /// included, still holds `needle`.
    fn mentions(doc: &Document, needle: &[u8]) -> bool {
        fn holds(o: &Object, needle: &[u8]) -> bool {
            match o {
                Object::String(s, _) | Object::Name(s) => s.windows(needle.len()).any(|w| w == needle),
                Object::Array(items) => items.iter().any(|i| holds(i, needle)),
                Object::Dictionary(d) => d.iter().any(|(_, v)| holds(v, needle)),
                Object::Stream(s) => {
                    s.dict.iter().any(|(_, v)| holds(v, needle))
                        || s.decompressed_content().unwrap_or_else(|_| s.content.clone()).windows(needle.len()).any(|w| w == needle)
                }
                _ => false,
            }
        }
        doc.objects.values().any(|o| holds(o, needle))
    }

    fn strip_and_reload(doc: &mut Document) -> Document {
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let out = rewrite(&bytes, Mode::Strip, 0.0, 0.0, 0, None).unwrap_or_else(|e| panic!("{e}"));
        Document::load_mem(&out.data).unwrap()
    }

    fn note(author: Object) -> lopdf::Dictionary {
        dictionary! {
            "Type" => name("Annot"), "Subtype" => name("Text"), "Rect" => rect(10, 10),
            "T" => author, "Contents" => Object::string_literal("move this paragraph up"),
        }
    }

    /// A page may hold its annotations through a reference to an array, as
    /// qpdf and incremental Acrobat saves write it, or hold an annotation
    /// dictionary in the array itself. Neither was visited.
    #[test]
    fn authors_in_indirect_arrays_and_inline_annotations_go() {
        let mut doc = Document::with_version("1.5");
        let first = doc.add_object(note(Object::string_literal("Ben Meadows")));
        let array = doc.add_object(Object::Array(vec![Object::Reference(first)]));
        one_page(&mut doc, dictionary! { "Annots" => Object::Reference(array) }, dictionary! { "MediaBox" => rect(612, 792) });
        assert!(!mentions(&strip_and_reload(&mut doc), b"Ben Meadows"), "an indirect array kept its author");

        let mut doc = Document::with_version("1.5");
        let inline = Object::Dictionary(note(Object::string_literal("Ben Meadows")));
        one_page(&mut doc, dictionary! { "Annots" => Object::Array(vec![inline]) }, dictionary! { "MediaBox" => rect(612, 792) });
        assert!(!mentions(&strip_and_reload(&mut doc), b"Ben Meadows"), "an inline annotation kept its author");
    }

    /// A form field's widget is an annotation too, and there /T is the field's
    /// name, which the form is filled, scripted and submitted by. It stays.
    #[test]
    fn a_form_fields_name_is_kept() {
        let mut doc = Document::with_version("1.5");
        let field = doc.add_object(dictionary! {
            "Type" => name("Annot"), "Subtype" => name("Widget"), "FT" => name("Tx"), "Rect" => rect(10, 10),
            "T" => Object::string_literal("customer_name"), "V" => Object::string_literal("filled in"),
        });
        one_page(&mut doc, dictionary! { "Annots" => Object::Array(vec![Object::Reference(field)]) }, dictionary! { "MediaBox" => rect(612, 792) });
        let after = strip_and_reload(&mut doc);
        let widget = after
            .objects
            .values()
            .filter_map(|o| o.as_dict().ok())
            .find(|d| d.get(b"Subtype").and_then(|s| s.as_name()).map(|n| n == b"Widget").unwrap_or(false))
            .expect("the field is still there");
        assert_eq!(widget.get(b"T").unwrap().as_str().unwrap(), b"customer_name", "the field lost its name");
    }

    /// An author or editing data held in an object of its own stayed in the
    /// file after the reference to it was removed, because the save writes
    /// every object.
    #[test]
    fn what_a_removed_reference_pointed_at_goes_too() {
        let mut doc = Document::with_version("1.5");
        let author = doc.add_object(Object::string_literal("Ben Meadows"));
        let private = doc.add_object(dictionary! { "Illustrator" => dictionary! { "Private" => Object::string_literal("layer history") } });
        let first = doc.add_object(note(Object::Reference(author)));
        one_page(
            &mut doc,
            dictionary! { "Annots" => Object::Array(vec![Object::Reference(first)]), "PieceInfo" => Object::Reference(private) },
            dictionary! { "MediaBox" => rect(612, 792) },
        );
        let after = strip_and_reload(&mut doc);
        assert!(!mentions(&after, b"Ben Meadows"), "the author object survived");
        assert!(!mentions(&after, b"layer history"), "the editing data survived");
    }

    fn first_image_filter(bytes: &[u8]) -> Object {
        let doc = Document::load_mem(bytes).unwrap();
        doc.objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .find(|s| s.dict.get(b"Subtype").and_then(|n| n.as_name()).map(|n| n == b"Image").unwrap_or(false))
            .expect("the image is still there")
            .dict
            .get(b"Filter")
            .unwrap()
            .clone()
    }

    /// ReportLab and qpdf store a JPEG under an outer filter, ASCII hex or
    /// Flate before DCTDecode. The picture was skipped and kept its EXIF. The
    /// outer filters are undone and the JPEG is stored bare.
    #[test]
    fn a_picture_under_a_filter_chain_loses_its_exif_too() {
        // Long enough that Flate shrinks it, which lopdf requires before it
        // will compress at all.
        let exif = [&b"Exif\0\0"[..], &[b'x'; 400], b"GPS 33.4484N 112.0740W"].concat();
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&exif);
        jpeg.extend_from_slice(&SCAN_ONLY[2..]);
        let hex: Vec<u8> = jpeg.iter().map(|b| format!("{b:02X}")).collect::<String>().into_bytes();
        let mut flate = lopdf::Stream::new(dictionary! {}, jpeg.clone());
        flate.compress().unwrap();
        assert!(flate.dict.has(b"Filter"), "lopdf did not compress the fixture");
        for (outer, stored) in [("ASCIIHexDecode", [hex, b">".to_vec()].concat()), ("FlateDecode", flate.content)] {
            let chain = Object::Array(vec![name(outer), name("DCTDecode")]);
            let out = rewrite(&pdf_with_image(chain, stored), Mode::Strip, 0.0, 0.0, 0, None)
                .unwrap_or_else(|e| panic!("{outer}: {e}"));
            assert_eq!(first_image_content(&out.data), SCAN_ONLY, "{outer}: the EXIF survived");
            assert_eq!(first_image_filter(&out.data).as_name().unwrap(), b"DCTDecode");
        }
    }

    /// A filter may be named through a reference. It still names DCTDecode.
    #[test]
    fn a_filter_given_by_reference_is_followed() {
        let mut doc = Document::with_version("1.5");
        let filter = doc.add_object(name("DCTDecode"));
        let picture = doc.add_object(Object::Stream(lopdf::Stream::new(
            dictionary! {
                "Type" => name("XObject"), "Subtype" => name("Image"), "Width" => 1, "Height" => 1,
                "ColorSpace" => name("DeviceRGB"), "BitsPerComponent" => 8, "Filter" => Object::Reference(filter),
            },
            jpeg_with_exif(),
        )));
        one_page(
            &mut doc,
            dictionary! { "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => Object::Reference(picture) } } },
            dictionary! { "MediaBox" => rect(612, 792) },
        );
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let out = rewrite(&bytes, Mode::Strip, 0.0, 0.0, 0, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(first_image_content(&out.data), SCAN_ONLY, "the EXIF survived");
    }

    /// A picture Strip cannot walk keeps whatever it carries, so the document
    /// is refused rather than written with it and reported clean.
    #[test]
    fn a_picture_that_cannot_be_read_refuses_the_strip() {
        let mut truncated = jpeg_with_exif();
        truncated.truncate(truncated.len() - 2); // no end of image
        let bytes = pdf_with_image(name("DCTDecode"), truncated);
        assert!(rewrite(&bytes, Mode::Strip, 0.0, 0.0, 0, None).is_err(), "written with its EXIF");
    }

    /// An HDR photograph placed whole in a PDF keeps its gain map, as it does
    /// when stripped on its own. Cutting at the primary's end of image threw
    /// the map away.
    #[test]
    fn a_picture_inside_keeps_its_gain_map() {
        let iso: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0\0\0";
        let mut map = vec![0xFF, 0xD8, 0xFF, 0xE2];
        map.extend_from_slice(&((iso.len() + 2) as u16).to_be_bytes());
        map.extend_from_slice(iso);
        map.extend_from_slice(&SCAN_ONLY[2..]);
        let photo = crate::gainmap::attach(&jpeg_with_exif(), &[map], None).unwrap();

        let out = rewrite(&pdf_with_image(name("DCTDecode"), photo), Mode::Strip, 0.0, 0.0, 0, None)
            .unwrap_or_else(|e| panic!("{e}"));
        let stored = first_image_content(&out.data);
        assert!(!stored.windows(4).any(|w| w == b"Exif"), "the EXIF survived");
        let found = crate::gainmap::extract(&stored).expect("the gain map was cut off");
        assert_eq!(found.images.len(), 1);
    }

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
    fn a_picture_keeps_the_profile_its_colours_are_defined_by() {
        // Flat raw samples above the perceptual minimum, so the fast encode
        // is far smaller than the source and the rewrite goes through.
        let side = MIN_IMAGE_DIM + 15;
        let raw = |components: usize| vec![0x80u8; side * side * components];
        let mut doc = Document::with_version("1.5");
        let profile = doc.add_object(Object::Stream(lopdf::Stream::new(dictionary! { "N" => 3 }, vec![])));
        let icc = Object::Array(vec![name("ICCBased"), Object::Reference(profile)]);
        let tagged = doc.add_object(Object::Stream(lopdf::Stream::new(
            dictionary! {
                "Type" => name("XObject"), "Subtype" => name("Image"), "Width" => side as i64, "Height" => side as i64,
                "ColorSpace" => icc, "BitsPerComponent" => 8,
            },
            raw(3),
        )));
        let grey = doc.add_object(Object::Stream(lopdf::Stream::new(
            dictionary! {
                "Type" => name("XObject"), "Subtype" => name("Image"), "Width" => side as i64, "Height" => side as i64,
                "ColorSpace" => name("DeviceGray"), "BitsPerComponent" => 8,
            },
            raw(1),
        )));
        one_page(
            &mut doc,
            dictionary! { "Resources" => dictionary! { "XObject" => dictionary! {
                "Im0" => Object::Reference(tagged), "Im1" => Object::Reference(grey),
            } } },
            dictionary! { "MediaBox" => rect(612, 792) },
        );
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let rewritten = rewrite(&bytes, Mode::Fast, 0.0, 75.0, 0, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(rewritten.images_rewritten, 2, "both pictures should have been re-encoded");

        let after = Document::load_mem(&rewritten.data).unwrap();
        let space_of = |id: ObjectId| after.get_object(id).unwrap().as_stream().unwrap().dict.get(b"ColorSpace").unwrap().clone();
        // The profile is still the space the samples are read through.
        let kept = space_of(tagged);
        assert_eq!(kept.as_array().unwrap()[0].as_name().unwrap(), b"ICCBased", "the profile was dropped: {kept:?}");
        // One channel became three, and a one-channel profile would lie about them.
        assert_eq!(space_of(grey).as_name().unwrap(), b"DeviceRGB");
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
    fn a_mask_is_not_a_picture() {
        let mut doc = Document::with_version("1.5");
        // A JPEG-coded soft mask looks like any other grey picture from its
        // own dictionary; only the picture that uses it says what it is.
        let soft = doc.add_object(image(dictionary! {
            "Type" => name("XObject"), "Subtype" => name("Image"),
            "ColorSpace" => name("DeviceGray"), "Filter" => name("DCTDecode"),
        }));
        let stencil = doc.add_object(image(dictionary! {
            "Type" => name("XObject"), "Subtype" => name("Image"), "ImageMask" => Object::Boolean(true),
        }));
        let picture = doc.add_object(image(dictionary! {
            "Type" => name("XObject"), "Subtype" => name("Image"), "ColorSpace" => name("DeviceRGB"),
            "SMask" => Object::Reference(soft), "Mask" => Object::Reference(stencil),
        }));
        let plain = doc.add_object(image(dictionary! { "Type" => name("XObject"), "Subtype" => name("Image") }));
        doc.add_object(image(dictionary! { "Type" => name("XObject"), "Subtype" => name("Form") }));

        assert_eq!(image_candidates(&doc), vec![picture, plain]);
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
    fn a_page_size_declared_once_on_the_tree_is_still_read() {
        // What Ghostscript, LibreOffice and Word's export write: one box on
        // the root, and every page inherits it.
        let mut doc = Document::with_version("1.5");
        let (tree, page) = one_page(&mut doc, dictionary! {}, dictionary! { "MediaBox" => rect(842, 1191) });
        assert_eq!(longest_page_pt(&doc), 1191.0, "an A3 page inherited from the tree");

        // A page's own box wins over the tree's.
        doc.get_dictionary_mut(page).unwrap().set("MediaBox", rect(612, 792));
        assert_eq!(longest_page_pt(&doc), 792.0);

        // No box anywhere is US Letter, not zero.
        doc.get_dictionary_mut(page).unwrap().remove(b"MediaBox");
        doc.get_dictionary_mut(tree).unwrap().remove(b"MediaBox");
        assert_eq!(longest_page_pt(&doc), FALLBACK_PAGE_LONG_PT);
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

    #[test]
    fn an_xmp_packet_leaves_the_file_and_not_only_the_catalog() {
        let mut doc = Document::with_version("1.5");
        let mut packet = |tool: &str| {
            doc.add_object(Object::Stream(lopdf::Stream::new(
                dictionary! { "Type" => name("Metadata"), "Subtype" => name("XML") },
                format!("<x:xmpmeta><rdf:Description xmp:CreatorTool=\"{tool}\"/></x:xmpmeta>").into_bytes(),
            )))
        };
        let on_document = packet("Quartz PDFContext");
        let on_page = packet("Adobe Photoshop");
        let on_picture = packet("Adobe Illustrator");
        // Nothing points at this one: an earlier writer left it behind.
        let orphan = packet("Acrobat Pro");

        let picture = doc.add_object(image(dictionary! {
            "Type" => name("XObject"), "Subtype" => name("Image"), "Metadata" => Object::Reference(on_picture),
        }));
        let (_, page) = one_page(
            &mut doc,
            dictionary! {
                "Metadata" => Object::Reference(on_page),
                "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => Object::Reference(picture) } },
            },
            dictionary! {},
        );
        let catalog = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        doc.get_dictionary_mut(catalog).unwrap().set("Metadata", Object::Reference(on_document));

        assert_eq!(strip_document_metadata(&mut doc), 3, "one entry each on the catalog, the page and the picture");
        for id in [on_document, on_page, on_picture, orphan] {
            assert!(!doc.has_object(id), "packet {id:?} is still in the object table");
        }
        assert!(doc.get_object(picture).is_ok(), "the picture itself stays");
        assert!(!doc.get_dictionary(page).unwrap().has(b"Metadata"));

        // What matters is the file, not the object table: `save_to` writes
        // every object it holds, pointed at or not.
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        assert!(!out.windows(11).any(|w| w == b"CreatorTool"), "an XMP packet survived into the written file");

        assert_eq!(strip_document_metadata(&mut doc), 0);
    }
}
