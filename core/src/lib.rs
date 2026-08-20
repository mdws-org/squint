//! Perceptual image compression engine.
//!
//! The engine encodes to a perceptual target rather than a fixed quality number.
//! For each image it searches for the smallest file that still scores at or above
//! a SSIMULACRA2 threshold.

mod metadata;
pub mod ffi;
pub mod gainmap;
pub mod png;
pub use metadata::{extract_icc, extract_orientation};

use imgref::ImgVec;

/// SSIMULACRA2 misindexes its internal weight table below this size and returns
/// scores that are not meaningful. Perceptual targeting must be refused here.
pub const MIN_PERCEPTUAL_DIM: usize = 113;

/// JPEG cannot reach an arbitrary score. Above roughly this value the search
/// cannot converge, so it must fail rather than exhaust its probe budget.
pub const JPEG_SCORE_CEILING: f64 = 91.0;

/// The largest picture the engine will decode.
///
/// A small file can declare an enormous one: a forty kilobyte PNG claiming
/// 60000 by 60000 asks the decoder for fourteen gigabytes, and the answer is
/// either an abort that takes every other job with it or a machine in swap.
/// Refusing on the declared size, before anything is allocated, turns that into
/// an ordinary typed error. The cap sits far above any camera — a 48 megapixel
/// phone photograph is a fifth of it, and a large stitched panorama still fits.
pub const MAX_PIXELS: usize = 250_000_000;

/// Ceiling on what a decoder may allocate, as a backstop behind the size check.
///
/// Large enough for any picture that passes `MAX_PIXELS`, including the
/// decoder's own working buffers.
const MAX_DECODE_ALLOC: u64 = 1_500_000_000;

/// Decode with the declared size checked first and an allocation ceiling behind it.
///
/// Both halves matter. The size check gives a typed error naming the dimensions,
/// which is what a person needs to see; the allocation ceiling catches whatever
/// a malformed header can do that the size check cannot anticipate.
fn decode_limited(bytes: &[u8]) -> Result<image::DynamicImage, Error> {
    let sized = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let (width, height) = sized.into_dimensions().map_err(|e| Error::Decode(e.to_string()))?;
    let pixels = (width as usize).saturating_mul(height as usize);
    if pixels > MAX_PIXELS {
        return Err(Error::TooLarge { width: width as usize, height: height as usize });
    }

    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    reader.decode().map_err(|e| Error::Decode(e.to_string()))
}

#[derive(Debug)]
pub enum Error {
    Decode(String),
    Encode(String),
    Metric(String),
    /// The image is too small for perceptual targeting to be valid.
    TooSmall { shorter_side: usize },
    /// No quality setting reached the target. Carries the best score seen.
    Unreachable { best_score: f64 },
    /// The target was met, but only by a file larger than the original. An
    /// optimizer must never grow a file; the caller keeps the original.
    NoSmallerResult { best_bytes: usize, original_bytes: usize },
    /// The picture declares more pixels than the engine will decode.
    TooLarge { width: usize, height: usize },
    /// Something below panicked. Reported rather than allowed to unwind into C,
    /// where it would abort the process and take every other job with it.
    Panicked,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Decode(m) => write!(f, "decode failed: {m}"),
            Error::Encode(m) => write!(f, "encode failed: {m}"),
            Error::Metric(m) => write!(f, "metric failed: {m}"),
            Error::TooSmall { shorter_side } => write!(
                f,
                "image is {shorter_side}px on its shorter side; perceptual targeting requires at least {MIN_PERCEPTUAL_DIM}px"
            ),
            Error::Unreachable { best_score } => write!(
                f,
                "target unreachable; best achievable score was {best_score:.2}"
            ),
            Error::NoSmallerResult { best_bytes, original_bytes } => write!(
                f,
                "target is only reachable at {best_bytes} bytes, larger than the original {original_bytes}; keeping the original"
            ),
            Error::TooLarge { width, height } => write!(
                f,
                "image declares {width}x{height} pixels, beyond the {MAX_PIXELS} the engine will decode"
            ),
            Error::Panicked => write!(f, "the engine failed unexpectedly; the file was not changed"),
        }
    }
}

/// Decoded pixels with dimensions. Always RGB8.
///
/// Both sides of a comparison must reach this struct through the same decode
/// path. Comparing a Display P3 reference against an untagged candidate shifts
/// the score by more than the difference between metric implementations.
pub struct Image {
    pub pixels: Vec<[u8; 3]>,
    pub width: usize,
    pub height: usize,
}

impl Image {
    pub fn from_rgb8(bytes: &[u8], width: usize, height: usize) -> Self {
        let pixels = bytes.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        Image { pixels, width, height }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let img = decode_limited(bytes)?.to_rgb8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        Ok(Image::from_rgb8(&img.into_raw(), w, h))
    }

    pub fn shorter_side(&self) -> usize {
        self.width.min(self.height)
    }

    pub fn megapixels(&self) -> f64 {
        (self.width * self.height) as f64 / 1_000_000.0
    }

    /// Bake an EXIF orientation into the pixels.
    ///
    /// squint drops EXIF on re-encode, so an orientation left as a tag would be
    /// lost and the image would display rotated. Applying it to the pixels makes
    /// the file correct without metadata.
    pub fn apply_orientation(&mut self, orientation: u16) {
        if !(2..=8).contains(&orientation) {
            return;
        }
        let (pixels, w, h) = oriented(&self.pixels, self.width, self.height, orientation);
        self.pixels = pixels;
        self.width = w;
        self.height = h;
    }

    fn as_img(&self) -> ImgVec<[u8; 3]> {
        ImgVec::new(self.pixels.clone(), self.width, self.height)
    }

    /// Flat RGB bytes, as the JPEG encoder wants them.
    fn flat(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 3);
        for p in &self.pixels {
            out.extend_from_slice(p);
        }
        out
    }
}

/// Apply an EXIF orientation to a grid of pixels, returning it and its new size.
///
/// Written once and used for both the picture and its gain map. The two are a
/// matched pair, so a map turned differently from the image it lifts would
/// brighten the wrong corner of it.
pub fn oriented<P: Copy + Default>(
    pixels: &[P],
    width: usize,
    height: usize,
    orientation: u16,
) -> (Vec<P>, usize, usize) {
    if !(2..=8).contains(&orientation) {
        return (pixels.to_vec(), width, height);
    }
    let (w, h) = (width, height);
    let transposed = matches!(orientation, 5 | 6 | 7 | 8);
    let (nw, nh) = if transposed { (h, w) } else { (w, h) };
    let mut out = vec![P::default(); w * h];

    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                8 => (y, w - 1 - x),
                _ => (x, y),
            };
            out[ny * nw + nx] = pixels[y * w + x];
        }
    }
    (out, nw, nh)
}

/// Score a candidate against its reference. 100 is identical, 90 is visually
/// lossless under a flicker test, 80 is the highest quality useful for web
/// delivery, 70 is a good general web target.
///
/// The metric is asymmetric. Always pass the reference first.
pub fn score(reference: &Image, candidate: &Image) -> Result<f64, Error> {
    fast_ssim2::compute_ssimulacra2(reference.as_img().as_ref(), candidate.as_img().as_ref())
        .map_err(|e| Error::Metric(format!("{e:?}")))
}

/// Encode to JPEG, carrying the colour profile across.
///
/// Passing `None` for `icc` produces an untagged file. That is the bug ImageOptim
/// ships: it drops the profile with the EXIF, so Display P3 photographs are then
/// read as sRGB and their colours shift. Callers should pass the source profile.
pub fn encode_jpeg(image: &Image, quality: f32, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        comp.set_size(image.width, image.height);
        comp.set_quality(quality);
        let mut started = comp.start_compress(Vec::new())?;
        if let Some(profile) = icc {
            started.write_icc_profile(profile);
        }
        started.write_scanlines(&image.flat())?;
        started.finish()
    }))
    .map_err(|_| Error::Encode("mozjpeg panicked".into()))?
    .map_err(|e| Error::Encode(e.to_string()))
}

/// Whether a file signals high dynamic range, without lifting anything out.
pub fn has_gain_map(bytes: &[u8]) -> bool {
    gainmap::signals_hdr(bytes)
}

/// Strip a secondary image's metadata while keeping what makes it usable.
///
/// The general stripper removes every container it is not told to keep, which
/// for a gain map would take its parameters with the camera identity. They go
/// back on afterwards.
fn strip_gain_map(map: &[u8]) -> Option<Vec<u8>> {
    let bare = metadata::strip_jpeg(map)?;
    gainmap::insert_segments(&bare, &gainmap::descriptive_segments(map))
}

/// One encode plus one measurement.
#[derive(Debug, Clone)]
pub struct Probe {
    pub quality: f32,
    pub score: f64,
    pub bytes: usize,
}

#[derive(Debug)]
pub struct SearchResult {
    pub chosen: Probe,
    pub data: Vec<u8>,
    pub probes: Vec<Probe>,
}

/// Predict a starting quality for a target score.
///
/// Fitted against measured mozjpeg curves. The exponent lands within 1% of the
/// model oavif uses for libaom, so the shape transfers across codecs even though
/// the constant does not.
fn predict_quality(target: f64) -> f32 {
    (8.30 * (0.0285 * target).exp()).clamp(30.0, 98.0) as f32
}

/// Find the smallest JPEG that still scores at or above `target`.
///
/// The search collapses its bracket rather than exiting as soon as a probe lands
/// within tolerance. Tolerance exit is inherited from video encoding, where the
/// goal is to land near a number; here the goal is the smallest file above a bar,
/// and exiting early was measured to cost 13-45% in file size.
///
/// Returns the best *satisfying probe* rather than the bracket endpoint, because
/// quality is not monotonic in the encoder setting for synthetic content: text
/// and interface screenshots have been measured to invert by up to 3.5 points.
pub fn search(
    reference: &Image,
    target: f64,
    max_probes: usize,
    original_bytes: usize,
    icc: Option<&[u8]>,
) -> Result<SearchResult, Error> {
    if reference.shorter_side() < MIN_PERCEPTUAL_DIM {
        return Err(Error::TooSmall { shorter_side: reference.shorter_side() });
    }

    let mut probes: Vec<Probe> = Vec::new();
    let mut best: Option<(Probe, Vec<u8>)> = None;
    // The bracket spans the whole legal range and the prediction decides only
    // where to probe first. An earlier version narrowed the bracket to a window
    // around the prediction and meant to widen it on demand, but the widening
    // could never fire: it asked whether the first probe had landed at an edge,
    // and the first probe is the prediction, which sits in the middle by
    // construction. A picture needing a quality outside that window was refused
    // as unreachable, quoting a best score measured only inside a window the
    // search had never left. Spending the prediction on the opener rather than
    // on the bounds keeps the speed and drops the false refusal.
    let mut lo = 20.0f32;
    let mut hi = 98.0f32;
    let mut next = predict_quality(target);

    for _ in 0..max_probes {
        let mut q = next.round().clamp(lo, hi);
        if probes.iter().any(|p| p.quality == q) {
            // Interpolation landed somewhere already measured. Bisect rather
            // than stop: the bracket can still be several points wide, and the
            // smallest satisfying encode may be inside it.
            let mid = ((lo + hi) / 2.0).round().clamp(lo, hi);
            if mid == q || probes.iter().any(|p| p.quality == mid) {
                break; // genuinely collapsed onto measured points
            }
            q = mid;
        }

        let data = encode_jpeg(reference, q, icc)?;
        let decoded = Image::decode(&data)?;
        let s = score(reference, &decoded)?;
        let probe = Probe { quality: q, score: s, bytes: data.len() };
        probes.push(probe.clone());

        if s >= target {
            // Satisfies. Keep it if it is the smallest satisfying result so far.
            let better = best.as_ref().map_or(true, |(b, _)| probe.bytes < b.bytes);
            if better {
                best = Some((probe, data));
            }
            hi = q;
        } else {
            lo = q;
        }

        if hi - lo <= 1.0 {
            break;
        }

        // Interpolate once the target is bracketed. Before that the two
        // directions are not symmetric, and treating them the same is what made
        // the old search refuse images it could have encoded.
        //
        // With nothing satisfying yet, go straight to the ceiling. Whether the
        // target is reachable at all is the only question outstanding, one probe
        // answers it, and creeping upward can exhaust the budget before arriving
        // — which turns a reachable target into a refusal just as surely as a
        // bracket that could not widen. With nothing failing yet there is already
        // an answer in hand and only its size is in question, so step down
        // gently and keep the interpolation accurate.
        const STEP: f32 = 8.0;
        next = match interpolate(&probes, target) {
            Some(between) => between,
            None if best.is_none() => hi,
            None => (q - STEP).max(lo),
        };
    }

    match best {
        Some((chosen, data)) => {
            // An optimizer must never grow a file. A source that is already
            // compressed can require more bytes to match than it originally took.
            if chosen.bytes >= original_bytes {
                return Err(Error::NoSmallerResult {
                    best_bytes: chosen.bytes,
                    original_bytes,
                });
            }
            Ok(SearchResult { chosen, data, probes })
        }
        None => {
            let best_score = probes.iter().map(|p| p.score).fold(f64::MIN, f64::max);
            Err(Error::Unreachable { best_score })
        }
    }
}

/// Inverse linear interpolation on the two probes closest to the target.
fn interpolate(probes: &[Probe], target: f64) -> Option<f32> {
    let mut below = None::<&Probe>;
    let mut above = None::<&Probe>;
    for p in probes {
        if p.score < target && below.map_or(true, |b| p.score > b.score) {
            below = Some(p);
        }
        if p.score >= target && above.map_or(true, |a| p.score < a.score) {
            above = Some(p);
        }
    }
    let (b, a) = (below?, above?);
    if (a.score - b.score).abs() < f64::EPSILON {
        return None;
    }
    let t = (target - b.score) / (a.score - b.score);
    Some((b.quality as f64 + t * (a.quality - b.quality) as f64) as f32)
}

/// How hard to work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Encode once at a fixed quality and evaluate no metric. ImageOptim's speed.
    Fast,
    /// Search for the smallest file meeting a perceptual target.
    Quality,
    /// Remove metadata without touching the pixels. The result is
    /// pixel-identical to the input and usually only slightly smaller.
    Strip,
}

/// What became of a high dynamic range photograph's gain map.
///
/// Reported on every result so that losing the extra range is never something a
/// file does quietly. A picture that comes back flat on the display it was taken
/// for should say so.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hdr {
    /// The source carried no gain map.
    Absent,
    /// The source carried one and the output carries it too.
    Preserved,
    /// The source carried one and the output does not.
    Dropped,
}

/// The outcome of optimizing one file.
pub struct Optimized {
    pub data: Vec<u8>,
    /// Absent in fast mode, and for images too small to score.
    pub score: Option<f64>,
    pub hdr: Hdr,
    /// True when the colour count was reduced. PNG's lossy mode is a palette
    /// reduction rather than a quality dial, and it is on by default, so a file
    /// can come back visibly changed with no score to show for it.
    pub quantized: bool,
    pub original_bytes: usize,
}

/// Optimize an encoded image, dispatching on its format.
///
/// This is the single entry point. The command line harness and the C API both
/// call it, so the application cannot drift away from what the CLI measures.
pub fn optimize(
    bytes: &[u8],
    mode: Mode,
    target: f64,
    fixed_quality: f32,
    png_min_quality: Option<u8>,
) -> Result<Optimized, Error> {
    if mode == Mode::Strip {
        let (stripped, hdr) = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            (png::strip_png(bytes), Hdr::Absent)
        } else {
            // The orientation tag goes back on before anything else, because it
            // lengthens the picture and the gain map's index records where the
            // picture ends.
            let bare = metadata::strip_jpeg(bytes).and_then(|stripped| {
                match extract_orientation(bytes) {
                    1 => Some(stripped),
                    turned => gainmap::insert_segments(
                        &stripped,
                        &[(0xE1, metadata::orientation_segment(turned))],
                    ),
                }
            });
            // A secondary image is picture data, not metadata. Stripping a
            // photograph of its location should not also take away half its
            // brightness, or one eye of a stereo pair.
            match (bare, gainmap::extract(bytes)) {
                (Some(bare), Some(found)) => {
                    let cleaned = found
                        .images
                        .iter()
                        .map(|img| strip_gain_map(img))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| Error::Encode("could not strip a secondary image".into()))?;
                    let hdr = if found.has_gain_map { Hdr::Preserved } else { Hdr::Absent };
                    match gainmap::attach(&bare, &cleaned, found.iso_segment.as_deref()) {
                        Some(joined) => (Some(joined), hdr),
                        // Reattaching failed, so whatever was carried is gone.
                        None if found.has_gain_map => (Some(bare), Hdr::Dropped),
                        None => (Some(bare), Hdr::Absent),
                    }
                }
                // No index to read. The marker in the primary can still say a map
                // was there, and stripping cuts at the end of the primary, so it
                // is gone either way and must not be reported as absent.
                (Some(bare), None) if gainmap::signals_hdr(bytes) => (Some(bare), Hdr::Dropped),
                (bare, _) => (bare, Hdr::Absent),
            }
        };
        let stripped = stripped.ok_or_else(|| Error::Decode("not a JPEG or PNG".into()))?;

        if stripped.len() >= bytes.len() {
            return Err(Error::NoSmallerResult {
                best_bytes: stripped.len(),
                original_bytes: bytes.len(),
            });
        }
        return Ok(Optimized {
            data: stripped,
            score: None,
            hdr,
            quantized: false,
            original_bytes: bytes.len(),
        });
    }

    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let effort = match mode {
            Mode::Quality => png::Effort::Thorough,
            // Strip returns before reaching here; Fast must not pay the
            // thorough preset, which costs an order of magnitude for a few
            // percent.
            Mode::Fast | Mode::Strip => png::Effort::Quick,
        };
        let r = png::optimize_png(bytes, png_min_quality, mode == Mode::Quality, effort)?;
        return Ok(Optimized {
            data: r.data,
            score: r.score,
            hdr: Hdr::Absent,
            quantized: r.quantized,
            original_bytes: bytes.len(),
        });
    }

    let mut image = Image::decode(bytes)?;
    let icc = extract_icc(bytes);
    let orientation = extract_orientation(bytes);
    image.apply_orientation(orientation);

    let (data, score) = match mode {
        // Strip is handled above and never reaches this match.
        Mode::Fast | Mode::Strip => (encode_jpeg(&image, fixed_quality, icc.as_deref())?, None),
        Mode::Quality => {
            if target > JPEG_SCORE_CEILING {
                return Err(Error::Unreachable { best_score: JPEG_SCORE_CEILING });
            }
            let r = search(&image, target, 6, bytes.len(), icc.as_deref())?;
            (r.data, Some(r.chosen.score))
        }
    };

    // A gain map cannot yet be carried onto a re-encoded picture, so say so
    // rather than losing the range quietly.
    //
    // The container squint builds is sound: the same index and the same map,
    // attached to a primary from libjpeg-turbo or to the untouched primary that
    // strip mode produces, are read by ImageIO without complaint. Attached to a
    // primary mozjpeg encoded, macOS will not open the file at all — no
    // properties, no decode, and `sips` reports nothing either. What triggers it
    // lives in mozjpeg's entropy-coded output: the colour profile, the
    // quantization tables, the scan mode and the segment order were each
    // substituted in turn and none of them made the difference. An unopenable
    // file is a worse outcome than one that has lost its extra range, so the
    // map is left off until the picture can be encoded some other way.
    let hdr = if has_gain_map(bytes) { Hdr::Dropped } else { Hdr::Absent };

    // Checked on the finished file rather than the picture alone: carrying the
    // gain map costs bytes, and an optimizer must never grow a file.
    if data.len() >= bytes.len() {
        return Err(Error::NoSmallerResult {
            best_bytes: data.len(),
            original_bytes: bytes.len(),
        });
    }
    Ok(Optimized { data, score, hdr, quantized: false, original_bytes: bytes.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 wide by 3 tall image whose pixels encode their own coordinates as
    /// `y * 10 + x`, so a transform can be checked by reading values back.
    fn coded() -> Image {
        let mut pixels = Vec::new();
        for y in 0..3u8 {
            for x in 0..2u8 {
                pixels.push([y * 10 + x, 0, 0]);
            }
        }
        Image { pixels, width: 2, height: 3 }
    }

    fn at(img: &Image, x: usize, y: usize) -> u8 {
        img.pixels[y * img.width + x][0]
    }

    #[test]
    fn orientation_1_is_a_no_op() {
        let mut img = coded();
        img.apply_orientation(1);
        assert_eq!((img.width, img.height), (2, 3));
        assert_eq!(at(&img, 0, 0), 0);
        assert_eq!(at(&img, 1, 2), 21);
    }

    #[test]
    fn orientation_3_rotates_180() {
        let mut img = coded();
        img.apply_orientation(3);
        assert_eq!((img.width, img.height), (2, 3));
        // The bottom right pixel becomes the top left.
        assert_eq!(at(&img, 0, 0), 21);
        assert_eq!(at(&img, 1, 2), 0);
    }

    #[test]
    fn orientation_6_rotates_90_clockwise() {
        let mut img = coded();
        img.apply_orientation(6);
        // Dimensions transpose.
        assert_eq!((img.width, img.height), (3, 2));
        // Rotating clockwise moves the bottom left corner to the top left.
        assert_eq!(at(&img, 0, 0), 20);
        assert_eq!(at(&img, 1, 0), 10);
        assert_eq!(at(&img, 2, 0), 0);
        assert_eq!(at(&img, 0, 1), 21);
    }

    #[test]
    fn orientation_8_rotates_90_counterclockwise() {
        let mut img = coded();
        img.apply_orientation(8);
        assert_eq!((img.width, img.height), (3, 2));
        // The top right corner moves to the top left.
        assert_eq!(at(&img, 0, 0), 1);
        assert_eq!(at(&img, 2, 1), 20);
    }

    #[test]
    fn out_of_range_orientation_is_ignored() {
        let mut img = coded();
        img.apply_orientation(99);
        assert_eq!((img.width, img.height), (2, 3));
        assert_eq!(at(&img, 0, 0), 0);
    }

    /// A minimal PNG carrying one text chunk, which must not survive stripping.
    fn png_with_text() -> Vec<u8> {
        fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut c = Vec::new();
            c.extend_from_slice(&(data.len() as u32).to_be_bytes());
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            c.extend_from_slice(&[0, 0, 0, 0]); // CRC is not checked by the stripper
            c
        }
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend(chunk(b"IHDR", &[0; 13]));
        png.extend(chunk(b"tEXt", b"Comment\0secret location"));
        png.extend(chunk(b"iCCP", b"profile"));
        png.extend(chunk(b"IDAT", b"pixels"));
        png.extend(chunk(b"IEND", b""));
        png
    }

    #[test]
    fn stripping_a_png_removes_text_and_keeps_pixels_and_profile() {
        let out = png::strip_png(&png_with_text()).expect("valid png");
        assert!(!out.windows(4).any(|w| w == b"tEXt"), "text chunk survived");
        assert!(!out.windows(6).any(|w| w == b"secret"), "text payload survived");
        assert!(out.windows(4).any(|w| w == b"IDAT"), "pixels were dropped");
        assert!(out.windows(4).any(|w| w == b"iCCP"), "colour profile was dropped");
        assert!(out.len() < png_with_text().len());
    }

    #[test]
    fn stripping_rejects_input_that_is_not_a_png() {
        assert!(png::strip_png(b"not a png at all").is_none());
    }

    #[test]
    fn stripping_rejects_input_that_is_not_a_jpeg() {
        assert!(metadata::strip_jpeg(b"not a jpeg").is_none());
    }

    /// The output of a failed walk used to be returned as a success. Since the
    /// caller writes that over the original, every one of these refusals is a
    /// photograph not destroyed.
    #[test]
    fn a_png_that_does_not_walk_cleanly_is_refused_rather_than_truncated() {
        let good = png_with_text();

        // A chunk claiming to be longer than the file.
        let mut lying = good.clone();
        lying[8..12].copy_from_slice(&0xFFFF_u32.to_be_bytes());
        assert!(png::strip_png(&lying).is_none(), "a bad length must refuse");

        // Cut short, so the end marker never arrives.
        assert!(png::strip_png(&good[..good.len() - 4]).is_none(), "truncation must refuse");

        // Structurally walkable, but carrying no pixels.
        let mut headless = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        headless.extend_from_slice(&0u32.to_be_bytes());
        headless.extend_from_slice(b"IEND");
        headless.extend_from_slice(&[0, 0, 0, 0]);
        assert!(png::strip_png(&headless).is_none(), "a file with no pixels must refuse");
    }

    #[test]
    fn a_jpeg_whose_segment_length_is_wrong_is_refused_rather_than_truncated() {
        // A marker, an APP1 that lies about its length by one byte, then a scan.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x08];
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xD9]);
        assert!(metadata::strip_jpeg(&jpeg).is_some(), "the control must strip");

        let mut off_by_one = jpeg.clone();
        off_by_one[5] = 0x09; // the walk now lands one byte past the next marker
        assert!(metadata::strip_jpeg(&off_by_one).is_none());

        let mut runs_past_the_end = jpeg.clone();
        runs_past_the_end[4..6].copy_from_slice(&0xFF00_u16.to_be_bytes());
        assert!(metadata::strip_jpeg(&runs_past_the_end).is_none());
    }

    #[test]
    fn a_jpeg_with_no_end_marker_is_refused() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0x33];
        assert!(metadata::strip_jpeg(&jpeg).is_none());
    }

    /// The profile is kept in APP2, where it is read from, and nowhere else.
    #[test]
    fn a_profile_tag_on_some_other_segment_does_not_survive() {
        // APP13 carrying the profile tag: length covers the twelve payload bytes.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xED, 0x00, 0x0E];
        jpeg.extend_from_slice(b"ICC_PROFILE\0");
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xD9]);
        let out = metadata::strip_jpeg(&jpeg).expect("walks cleanly");
        assert!(!out.windows(11).any(|w| w == b"ICC_PROFILE"));
    }

    #[test]
    fn a_png_re_encode_carries_the_colour_chunks_across() {
        let source = png_with_text();
        // Stand in for the encoder's output: same picture, no colour chunks.
        let mut bare = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        for (kind, data) in [(&b"IHDR"[..], &[0u8; 13][..]), (b"IDAT", b"pixels"), (b"IEND", b"")] {
            bare.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bare.extend_from_slice(kind);
            bare.extend_from_slice(data);
            bare.extend_from_slice(&[0, 0, 0, 0]);
        }
        assert!(!bare.windows(4).any(|w| w == b"iCCP"), "the stand-in starts untagged");

        let carried = png::colour_chunks(&source);
        let out = png::with_colour_chunks(&bare, &carried).expect("valid png");

        assert!(out.windows(4).any(|w| w == b"iCCP"), "the profile did not come across");
        // It has to precede the pixels, or a decoder will not apply it.
        let icc = out.windows(4).position(|w| w == b"iCCP").unwrap();
        let idat = out.windows(4).position(|w| w == b"IDAT").unwrap();
        assert!(icc < idat, "the profile must come before the pixels");

        // Applying it twice must not produce two of anything.
        let again = png::with_colour_chunks(&out, &carried).expect("valid png");
        assert_eq!(again.len(), out.len(), "a second pass duplicated a chunk");
    }

    #[test]
    fn the_orientation_squint_writes_is_the_orientation_it_reads() {
        for turned in [2u16, 3, 4, 5, 6, 7, 8] {
            let payload = metadata::orientation_segment(turned);
            let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
            jpeg.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            jpeg.extend_from_slice(&payload);
            jpeg.extend_from_slice(&[0xFF, 0xD9]);
            assert_eq!(extract_orientation(&jpeg), turned);
        }
    }

    #[test]
    fn perceptual_targeting_is_refused_below_the_valid_size() {
        let img = Image { pixels: vec![[0, 0, 0]; 100 * 100], width: 100, height: 100 };
        match search(&img, 80.0, 4, 999_999, None) {
            Err(Error::TooSmall { shorter_side }) => assert_eq!(shorter_side, 100),
            other => panic!("expected TooSmall, got {other:?}"),
        }
    }
}
