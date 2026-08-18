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
        let img = image::load_from_memory(bytes)
            .map_err(|e| Error::Decode(e.to_string()))?
            .to_rgba8();
        let (width, height) = (img.width() as usize, img.height() as usize);
        let pixels = img
            .into_raw()
            .chunks_exact(4)
            .map(|c| rgb::RGBA8::new(c[0], c[1], c[2], c[3]))
            .collect();
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
pub fn quantize(image: &RgbaImage, min_quality: u8, max_quality: u8) -> Result<RgbaImage, Error> {
    let mut liq = imagequant::new();
    liq.set_quality(min_quality, max_quality)
        .map_err(|e| Error::Encode(format!("quality range rejected: {e:?}")))?;

    let mut src = liq
        .new_image(&image.pixels[..], image.width, image.height, 0.0)
        .map_err(|e| Error::Encode(format!("{e:?}")))?;

    let mut res = liq
        .quantize(&mut src)
        .map_err(|_| Error::Unreachable { best_score: min_quality as f64 })?;
    res.set_dithering_level(1.0).ok();

    let (palette, indices) = res
        .remapped(&mut src)
        .map_err(|e| Error::Encode(format!("{e:?}")))?;

    let pixels = indices.iter().map(|&i| palette[i as usize]).collect();
    Ok(RgbaImage { pixels, width: image.width, height: image.height })
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
    measure: bool,
    effort: Effort,
) -> Result<PngResult, Error> {
    let source = RgbaImage::decode(bytes)?;

    let (candidate, quantized) = match min_quality {
        Some(min) => (quantize(&source, min, 100)?, true),
        None => (RgbaImage::decode(bytes)?, false),
    };

    let encoded = candidate.encode_png()?;
    let optimized = optimize_lossless(&encoded, effort)?;

    if optimized.len() >= bytes.len() {
        return Err(Error::NoSmallerResult {
            best_bytes: optimized.len(),
            original_bytes: bytes.len(),
        });
    }

    let score = if measure && source.shorter_side() >= crate::MIN_PERCEPTUAL_DIM {
        Some(score_rgba(&source, &candidate)?)
    } else {
        None
    };

    Ok(PngResult { data: optimized, score, quantized })
}


/// Chunks a PNG needs in order to render. Everything else is metadata.
const PNG_KEEP: [&[u8; 4]; 8] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", // structure and pixels
    b"tRNS", b"iCCP", b"sRGB", b"gAMA", // transparency and colour
];

/// Remove metadata chunks from a PNG without touching the pixels.
///
/// `IDAT` is copied unchanged, so the image is pixel-identical. Dropped chunks
/// include `tEXt`, `iTXt` and `zTXt` (where generators write prompts and seeds),
/// `eXIf` (location and camera data), `tIME`, and `caBX` (C2PA credentials).
///
/// The colour chunks are kept, for the same reason the JPEG path keeps ICC.
pub fn strip_png(png: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if !png.starts_with(SIGNATURE) {
        return None;
    }
    let mut out = Vec::with_capacity(png.len());
    out.extend_from_slice(SIGNATURE);

    let mut i = SIGNATURE.len();
    while i + 12 <= png.len() {
        let len = u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as usize;
        let kind = &png[i + 4..i + 8];
        let total = 12 + len;
        if i + total > png.len() {
            break;
        }
        if PNG_KEEP.iter().any(|k| k[..] == *kind) {
            out.extend_from_slice(&png[i..i + total]);
        }
        i += total;
        if kind == b"IEND" {
            break;
        }
    }
    Some(out)
}
