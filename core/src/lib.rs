//! Perceptual image compression engine.
//!
//! The engine encodes to a perceptual target rather than a fixed quality number.
//! For each image it searches for the smallest file that still scores at or above
//! a SSIMULACRA2 threshold.

mod metadata;
pub mod png;
pub use metadata::{extract_icc, extract_orientation};

use imgref::ImgVec;

/// SSIMULACRA2 misindexes its internal weight table below this size and returns
/// scores that are not meaningful. Perceptual targeting must be refused here.
pub const MIN_PERCEPTUAL_DIM: usize = 113;

/// JPEG cannot reach an arbitrary score. Above roughly this value the search
/// cannot converge, so it must fail rather than exhaust its probe budget.
pub const JPEG_SCORE_CEILING: f64 = 91.0;

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
        let img = image::load_from_memory(bytes)
            .map_err(|e| Error::Decode(e.to_string()))?
            .to_rgb8();
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
        let (w, h) = (self.width, self.height);
        let transposed = matches!(orientation, 5 | 6 | 7 | 8);
        let (nw, nh) = if transposed { (h, w) } else { (w, h) };
        let mut out = vec![[0u8; 3]; w * h];

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
                out[ny * nw + nx] = self.pixels[y * w + x];
            }
        }
        self.pixels = out;
        self.width = nw;
        self.height = nh;
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
    // Bracket around the prediction rather than the whole legal range. Opening at
    // the extremes wastes a probe bisecting into territory nothing is chosen from.
    let predicted = predict_quality(target);
    let mut lo = (predicted - 12.0).max(20.0);
    let mut hi = (predicted + 8.0).min(98.0);
    let mut next = predicted;
    let mut widened = false;

    for _ in 0..max_probes {
        let q = next.round().clamp(lo, hi);
        if probes.iter().any(|p| p.quality == q) {
            break; // bracket has collapsed onto an already-measured point
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

        // If the prediction bracketed wrongly, widen once before giving up on it.
        if !widened && probes.len() == 1 {
            if s < target && q >= hi - 0.5 {
                hi = 98.0;
                widened = true;
            } else if s >= target && q <= lo + 0.5 {
                lo = 20.0;
                widened = true;
            }
        }

        if hi - lo <= 1.0 {
            break;
        }

        // Interpolate against the two probes bracketing the target where we can,
        // and fall back to bisection when we cannot.
        next = interpolate(&probes, target).unwrap_or((lo + hi) / 2.0);
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

    #[test]
    fn perceptual_targeting_is_refused_below_the_valid_size() {
        let img = Image { pixels: vec![[0, 0, 0]; 100 * 100], width: 100, height: 100 };
        match search(&img, 80.0, 4, 999_999, None) {
            Err(Error::TooSmall { shorter_side }) => assert_eq!(shorter_side, 100),
            other => panic!("expected TooSmall, got {other:?}"),
        }
    }
}
