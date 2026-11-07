//! The PNG path.
//!
//! PNG differs from JPEG in two ways that matter. It carries an alpha channel,
//! which the perceptual metric has no concept of, and its lossy mode is palette
//! quantization rather than a quality dial.
//!
//! Quantization reduces the colour count; oxipng then performs the palette
//! encoding and the lossless compression. Splitting it that way avoids
//! hand-rolling an indexed-PNG encoder.

use crate::{Error, Image};

/// Decoded pixels with alpha.
pub struct RgbaImage {
    pub pixels: Vec<rgb::RGBA8>,
    pub width: usize,
    pub height: usize,
}

impl RgbaImage {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::decode_capped(bytes, None)
    }

    /// Decoded with the orientation an `eXIf` chunk records applied to the
    /// pixels, because the re-encoded file carries no `eXIf` to say it.
    pub fn decode_capped(bytes: &[u8], max_dimension: Option<u32>) -> Result<Self, Error> {
        let img = crate::capped(crate::decode_limited(bytes)?, max_dimension).to_rgba8();
        let (width, height) = (img.width() as usize, img.height() as usize);
        let pixels: Vec<rgb::RGBA8> = img
            .into_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| rgb::RGBA8::new(c[0], c[1], c[2], c[3]))
            .collect();
        let (pixels, width, height) = crate::oriented(&pixels, width, height, orientation(bytes));
        Ok(RgbaImage { pixels, width, height })
    }

    pub fn shorter_side(&self) -> usize {
        self.width.min(self.height)
    }

    pub fn has_alpha(&self) -> bool {
        self.pixels.iter().any(|p| p.a != 255)
    }

    /// Flatten onto a uniform background so the metric can see it.
    ///
    /// SSIMULACRA2 has no alpha concept. The reference tool scores twice against
    /// light and dark backgrounds and keeps the worse result, because a difference
    /// hidden against one background can be obvious against the other.
    fn composite(&self, background: u8) -> Image {
        let bg = background as u32;
        let pixels = self
            .pixels
            .iter()
            .map(|p| {
                let a = p.a as u32;
                let mix = |c: u8| (((c as u32 * a) + bg * (255 - a)) / 255) as u8;
                [mix(p.r), mix(p.g), mix(p.b)]
            })
            .collect();
        Image { pixels, width: self.width, height: self.height }
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, Error> {
        let mut raw = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            raw.extend_from_slice(&[p.r, p.g, p.b, p.a]);
        }
        let buf = image::RgbaImage::from_raw(self.width as u32, self.height as u32, raw)
            .ok_or_else(|| Error::Encode("pixel buffer did not match dimensions".into()))?;
        let mut out = std::io::Cursor::new(Vec::new());
        buf.write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| Error::Encode(e.to_string()))?;
        Ok(out.into_inner())
    }
}

/// Score a PNG candidate against its reference, honouring alpha.
///
/// Returns the worse of two scores, taken against a dark and a light background.
pub fn score_rgba(reference: &RgbaImage, candidate: &RgbaImage) -> Result<f64, Error> {
    if !reference.has_alpha() && !candidate.has_alpha() {
        return crate::score(&reference.composite(255), &candidate.composite(255));
    }
    let dark = crate::score(&reference.composite(26), &candidate.composite(26))?;
    let light = crate::score(&reference.composite(229), &candidate.composite(229))?;
    Ok(dark.min(light))
}

/// Reduce the colour count with imagequant, keeping alpha.
///
/// `min_quality` is a floor, not a target: imagequant refuses to emit a result it
/// cannot bring up to that quality, which is the behaviour ImageOptim exposes as
/// `PngMinQuality` and the reason libimagequant is worth its GPL obligation.
#[cfg(feature = "lossy-png")]
pub fn quantize(image: &RgbaImage, min_quality: u8, max_quality: u8) -> Result<RgbaImage, Error> {
    let mut liq = imagequant::new();
    liq.set_quality(min_quality, max_quality)
        .map_err(|e| Error::Encode(format!("quality range rejected: {e:?}")))?;

    let mut src = liq
        .new_image(&image.pixels[..], image.width, image.height, 0.0)
        .map_err(|e| Error::Encode(format!("{e:?}")))?;

    let mut res = liq
        .quantize(&mut src)
        .map_err(|_| Error::ColoursUnreachable { floor: min_quality })?;
    res.set_dithering_level(1.0).ok();

    let (palette, indices) = res
        .remapped(&mut src)
        .map_err(|e| Error::Encode(format!("{e:?}")))?;

    let pixels = indices.iter().map(|&i| palette[i as usize]).collect();
    Ok(RgbaImage { pixels, width: image.width, height: image.height })
}

/// The signature every PNG opens with.
const SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Walk a PNG's chunks, yielding `(kind, whole_chunk_bytes)`.
///
/// Returns `None` rather than a partial list if the file does not walk cleanly,
/// because a chunk length that runs past the end of the file means the rest of
/// the walk is reading whatever happens to be there.
fn chunks(png: &[u8]) -> Option<Vec<(&[u8], &[u8])>> {
    if !png.starts_with(SIGNATURE) {
        return None;
    }
    let mut out = Vec::new();
    let mut i = SIGNATURE.len();
    while i + 12 <= png.len() {
        let len = u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as usize;
        let total = len.checked_add(12)?;
        if i + total > png.len() {
            return None;
        }
        let kind = &png[i + 4..i + 8];
        out.push((kind, &png[i..i + total]));
        i += total;
        if kind == b"IEND" {
            return Some(out);
        }
    }
    None // ran out of file without reaching the end marker
}

/// Chunks that decide how a PNG's colours are read.
///
/// The re-encoder writes none of these, so without carrying them across, a
/// Display P3 screenshot comes back untagged and is read as sRGB. That is the
/// ImageOptim behaviour this project exists to correct, and it was reaching the
/// PNG path unnoticed because only the JPEG path was ever checked for it.
const PNG_COLOUR: [&[u8; 4]; 5] = [b"iCCP", b"sRGB", b"gAMA", b"cHRM", b"cICP"];

/// Lift the colour chunks out of a PNG, whole.
///
/// Copied verbatim, with their length and checksum, so nothing has to be
/// recomputed: the bytes that described the colour space in the source describe
/// it in the output.
pub fn colour_chunks(png: &[u8]) -> Vec<Vec<u8>> {
    let Some(found) = chunks(png) else { return Vec::new() };
    found
        .into_iter()
        .filter(|(kind, _)| PNG_COLOUR.iter().any(|k| k[..] == **kind))
        .map(|(_, whole)| whole.to_vec())
        .collect()
}

/// Put colour chunks back, directly after `IHDR` where the format requires them.
///
/// A chunk whose kind is already present is skipped, so this cannot produce a
/// file with two of anything.
pub fn with_colour_chunks(png: &[u8], carried: &[Vec<u8>]) -> Option<Vec<u8>> {
    if carried.is_empty() {
        return Some(png.to_vec());
    }
    let present = chunks(png)?;
    let header = present.first().filter(|(kind, _)| *kind == b"IHDR")?.1.len();
    let at = SIGNATURE.len() + header;

    let mut out = Vec::with_capacity(png.len() + carried.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(&png[..at]);
    for chunk in carried {
        let kind = chunk.get(4..8)?;
        if present.iter().any(|(k, _)| *k == kind) {
            continue;
        }
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&png[at..]);
    Some(out)
}

/// How hard to work at lossless recompression.
///
/// The cost is not symmetric with the gain. On a 12 megapixel image the thorough
/// preset takes an order of magnitude longer than the quick one for a few percent
/// of size, which is the wrong trade for the mode that has to feel instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effort {
    Quick,
    Thorough,
}

/// Lossless recompression. oxipng also reduces the colour type, which is what
/// turns a quantized RGBA buffer into an indexed PNG.
pub fn optimize_lossless(png: &[u8], effort: Effort) -> Result<Vec<u8>, Error> {
    let opts = match effort {
        Effort::Quick => oxipng::Options::from_preset(1),
        Effort::Thorough => oxipng::Options::from_preset(4),
    };
    oxipng::optimize_from_memory(png, &opts).map_err(|e| Error::Encode(e.to_string()))
}

#[derive(Debug)]
pub struct PngResult {
    pub data: Vec<u8>,
    pub score: Option<f64>,
    pub quantized: bool,
}

/// Optimize a PNG. With `min_quality` set, colours are quantized first.
///
/// Never returns a file larger than the input.
pub fn optimize_png(
    bytes: &[u8],
    min_quality: Option<u8>,
    target: Option<f64>,
    effort: Effort,
    max_dimension: Option<u32>,
) -> Result<PngResult, Error> {
    let source = RgbaImage::decode_capped(bytes, max_dimension)?;

    // Quality mode promises a score, and a picture this small cannot be scored.
    if target.is_some() && source.shorter_side() < crate::MIN_PERCEPTUAL_DIM {
        return Err(Error::TooSmall { shorter_side: source.shorter_side() });
    }

    let (candidate, quantized) = match reduction(&source, min_quality, target)? {
        #[cfg(feature = "lossy-png")]
        Some(floor) => (quantize(&source, floor, 100)?, true),
        _ => (RgbaImage::decode_capped(bytes, max_dimension)?, false),
    };

    let encoded = candidate.encode_png()?;
    let optimized = optimize_lossless(&encoded, effort)?;

    // The encoder writes no colour chunks at all, so they are put back from the
    // source. After the lossless pass rather than before it, since oxipng is free
    // to rewrite what it is given but not what it has already returned.
    let optimized = with_colour_chunks(&optimized, &colour_chunks(bytes))
        .ok_or_else(|| Error::Encode("could not carry the colour profile across".into()))?;

    if optimized.len() >= bytes.len() {
        return Err(Error::NoSmallerResult {
            best_bytes: optimized.len(),
            original_bytes: bytes.len(),
        });
    }

    // Quality mode reports what this result actually scored. Fast mode measures
    // nothing, which is what makes it fast.
    let score = match target {
        Some(_) => Some(score_rgba(&source, &candidate)?),
        None => None,
    };

    Ok(PngResult { data: optimized, score, quantized })
}

/// The colour floor to quantize at, or `None` to leave the colours alone.
#[cfg(feature = "lossy-png")]
fn reduction(source: &RgbaImage, min_quality: Option<u8>, target: Option<f64>) -> Result<Option<u8>, Error> {
    match target {
        // Quality mode. The colour floor is searched rather than taken on trust,
        // because it is imagequant's own scale and says nothing about how the
        // result will score.
        Some(t) => search_colours(source, t),
        None => Ok(min_quality),
    }
}

/// Built without a quantizer, the colours are always left alone: Fast mode is
/// lossless, and Quality mode meets any target with the untouched pixels.
#[cfg(not(feature = "lossy-png"))]
fn reduction(_: &RgbaImage, _: Option<u8>, _: Option<f64>) -> Result<Option<u8>, Error> {
    Ok(None)
}

/// Probes the search may spend. Each one quantizes, encodes and scores.
#[cfg(feature = "lossy-png")]
const MAX_COLOUR_PROBES: usize = 6;

/// Find the fewest colours that still meet the perceptual target.
///
/// The lever is imagequant's quality floor, which is its own scale and predicts
/// nothing about SSIMULACRA2 — so it is searched rather than assumed, the way
/// the JPEG path searches its quality setting. Fewer colours means a smaller
/// file, so the search wants the lowest floor that still satisfies.
///
/// The floor is not reliably monotonic in the score, so the best satisfying
/// probe is kept rather than the endpoint of the bracket, for the same reason
/// the JPEG search keeps its own.
///
/// PNG has an option JPEG does not: leaving the pixels alone. That is
/// pixel-identical to the source and so meets any target by construction, which
/// makes it the answer when no reduction does. A perceptual target on a PNG can
/// therefore never be unreachable, only expensive.
#[cfg(feature = "lossy-png")]
fn search_colours(source: &RgbaImage, target: f64) -> Result<Option<u8>, Error> {
    let (mut lo, mut hi) = (0u8, 100u8);
    let mut best: Option<u8> = None;

    for _ in 0..MAX_COLOUR_PROBES {
        if lo > hi {
            break;
        }
        let floor = lo + (hi - lo) / 2;
        // imagequant refuses outright when it cannot reach the floor it was
        // given. That is a failure of this probe, not of the search: a lower
        // floor may still satisfy, so it is treated as a floor that did not.
        match quantize(source, floor, 100) {
            Ok(reduced) if score_rgba(source, &reduced)? >= target => {
                best = Some(floor);
                match floor.checked_sub(1) {
                    Some(next) => hi = next,
                    None => break, // nothing below zero left to try
                }
            }
            _ => lo = floor.saturating_add(1),
        }
    }

    // `None` means nothing reduced far enough, and the pixels are left alone.
    Ok(best)
}


/// Which way up this PNG says it goes, 1 when it does not say.
///
/// A PNG records a turn only in the TIFF block of an `eXIf` chunk, the same
/// block a JPEG keeps in APP1, so the tag is read by the same walk.
pub fn orientation(png: &[u8]) -> u16 {
    chunks(png)
        .and_then(|found| {
            found
                .into_iter()
                .find(|(kind, _)| *kind == b"eXIf")
                .and_then(|(_, whole)| crate::metadata::orientation_in_tiff(&whole[8..whole.len() - 4]))
        })
        .unwrap_or(1)
}

/// A whole chunk: length, kind, data and the checksum over kind and data.
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 12);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&out[4..]).to_be_bytes());
    out
}

/// The CRC-32 a PNG chunk carries (ISO 3309, reflected, polynomial 0xEDB88320).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Chunks a PNG needs in order to render. Everything else is metadata.
///
/// The animation chunks are here because an animated PNG's later frames are
/// picture, not provenance: dropping them leaves a still image and calls it
/// unchanged. `cHRM` and `cICP` are here for the same reason `iCCP` is — a file
/// that keeps its gamma and loses its primaries has had its colours altered.
const PNG_KEEP: [&[u8; 4]; 13] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", // structure and pixels
    b"tRNS", b"iCCP", b"sRGB", b"gAMA", // transparency and colour
    b"cHRM", b"cICP", // colour primaries, and HDR signalling
    b"acTL", b"fcTL", b"fdAT", // animation frames
];

/// Remove metadata chunks from a PNG without touching the pixels.
///
/// `IDAT` is copied unchanged, so the image is pixel-identical. Dropped chunks
/// include `tEXt`, `iTXt` and `zTXt` (where generators write prompts and seeds),
/// `eXIf` (location and camera data), `tIME`, and `caBX` (C2PA credentials).
///
/// The colour chunks are kept, for the same reason the JPEG path keeps ICC.
/// An orientation the `eXIf` chunk recorded is written back in an `eXIf` of
/// its own holding nothing else, before the first `IDAT` as the format asks:
/// the pixels are copied untouched, so the tag is all that says which way up
/// they go.
///
/// Refuses rather than returning what it managed to copy. A file whose chunk
/// lengths do not walk cleanly, or that is missing a header, pixels, or an end
/// marker, is left alone: the caller writes the result over the original, and a
/// partial copy of a photograph is worse than no copy at all.
pub fn strip_png(png: &[u8]) -> Option<Vec<u8>> {
    let walked = chunks(png)?;
    let turned = orientation(png);
    let mut out = Vec::with_capacity(png.len());
    out.extend_from_slice(SIGNATURE);

    let (mut header, mut pixels, mut end) = (false, false, false);
    for (kind, whole) in walked {
        match kind {
            b"IHDR" => header = true,
            b"IDAT" if !pixels => {
                pixels = true;
                if turned != 1 {
                    out.extend_from_slice(&chunk(b"eXIf", &crate::metadata::orientation_tiff(turned)));
                }
            }
            b"IEND" => end = true,
            _ => {}
        }
        if PNG_KEEP.iter().any(|k| k[..] == *kind) {
            out.extend_from_slice(whole);
        }
    }
    (header && pixels && end).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written by ExifTool 13.55 (`-n -Orientation=6`) into a PNG: an `eXIf`
    /// chunk holding the orientation and YCbCrPositioning. Made by
    /// `tools/audit-2026-11-06.py make`.
    const EXIFTOOL_ORIENTATION_6: &[u8] = include_bytes!("../tests/fixtures/orientation-6.png");

    fn exif_chunks(png: &[u8]) -> Vec<Vec<u8>> {
        chunks(png)
            .expect("walks as a PNG")
            .into_iter()
            .filter(|(kind, _)| *kind == b"eXIf")
            .map(|(_, whole)| whole[8..whole.len() - 4].to_vec())
            .collect()
    }

    #[test]
    fn a_strip_keeps_which_way_up_a_png_goes_and_nothing_else_of_its_exif() {
        assert_eq!(orientation(EXIFTOOL_ORIENTATION_6), 6, "the fixture is not turned");
        let out = strip_png(EXIFTOOL_ORIENTATION_6).expect("a well formed file");
        assert_eq!(orientation(&out), 6, "the picture would come back on its side");
        assert_eq!(exif_chunks(&out), vec![crate::metadata::orientation_tiff(6)], "more than the turn survived");
        assert!(out.len() < EXIFTOOL_ORIENTATION_6.len());
        // What a strip writes, a second strip leaves as it is.
        assert_eq!(strip_png(&out).expect("the result strips"), out);
    }

    #[test]
    fn a_re_encoded_png_has_its_turn_in_the_pixels() {
        let source = RgbaImage::decode(EXIFTOOL_ORIENTATION_6).expect("decodes");
        let plain = image::load_from_memory(EXIFTOOL_ORIENTATION_6).expect("decodes");
        assert_eq!(
            (source.width, source.height),
            (plain.height() as usize, plain.width() as usize),
            "a quarter turn was not applied"
        );
        let out = optimize_png(EXIFTOOL_ORIENTATION_6, None, None, Effort::Quick, None).expect("re-encodes");
        let back = image::load_from_memory(&out.data).expect("the result decodes");
        assert_eq!((back.width() as usize, back.height() as usize), (source.width, source.height));
        assert!(exif_chunks(&out.data).is_empty(), "the re-encode carried EXIF across");
    }

    #[test]
    fn a_chunk_checksum_is_the_one_png_readers_check() {
        // The IEND chunk every PNG ends with has this checksum.
        assert_eq!(chunk(b"IEND", &[]), [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
    }
}
