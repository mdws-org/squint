//! C interface, so the macOS application can call the engine directly.
//!
//! Ownership rule: every buffer returned here was allocated by Rust and must be
//! handed back to `squint_result_free`. Nothing else may free it.

use crate::{optimize_as, Error, Hdr, Mode, OutputFormat};
use std::os::raw::{c_char, c_int};

pub const SQUINT_MODE_FAST: c_int = 0;
pub const SQUINT_MODE_QUALITY: c_int = 1;
pub const SQUINT_MODE_STRIP: c_int = 2;

pub const SQUINT_FORMAT_JPEG: c_int = 0;
pub const SQUINT_FORMAT_AVIF: c_int = 1;
pub const SQUINT_FORMAT_WEBP_LOSSLESS: c_int = 2;

pub const SQUINT_HDR_ABSENT: c_int = 0;
pub const SQUINT_HDR_PRESERVED: c_int = 1;
pub const SQUINT_HDR_DROPPED: c_int = 2;

/// Encodes the search may spend on one image. The application does not choose
/// this; only the command line harness varies it, for measurement.
const DEFAULT_PROBES: usize = 6;

pub const SQUINT_OK: c_int = 0;
pub const SQUINT_ERR_DECODE: c_int = 1;
pub const SQUINT_ERR_ENCODE: c_int = 2;
pub const SQUINT_ERR_METRIC: c_int = 3;
pub const SQUINT_ERR_TOO_SMALL: c_int = 4;
pub const SQUINT_ERR_UNREACHABLE: c_int = 5;
pub const SQUINT_ERR_NO_SMALLER: c_int = 6;
pub const SQUINT_ERR_NULL_INPUT: c_int = 7;
pub const SQUINT_ERR_TOO_LARGE: c_int = 8;
pub const SQUINT_ERR_PANIC: c_int = 9;
pub const SQUINT_ERR_COLOURS: c_int = 10;
pub const SQUINT_ERR_READ_ONLY: c_int = 11;
pub const SQUINT_ERR_UNKNOWN_FORMAT: c_int = 12;
pub const SQUINT_ERR_UNKNOWN_MODE: c_int = 13;

/// The result of one optimization.
///
/// `score` is NaN when no metric was evaluated, which is the normal case in fast
/// mode. Callers must check `error` before reading `data`.
#[repr(C)]
pub struct SquintResult {
    pub data: *mut u8,
    pub len: usize,
    pub original_len: usize,
    pub score: f64,
    /// What became of a high dynamic range gain map. See the `SQUINT_HDR_` values.
    pub hdr: c_int,
    /// Non-zero when the colour count was reduced to shrink the file.
    pub quantized: c_int,
    pub error: c_int,
    /// Human-readable error details allocated by Rust, or null when no message is set.
    pub error_message: *mut c_char,
    pub converted: c_int,
}

impl SquintResult {
    fn failure(error: c_int, original_len: usize, message: Option<&str>) -> Self {
        let error_message = message
            .and_then(|s| std::ffi::CString::new(s).ok())
            .map_or(std::ptr::null_mut(), |c| c.into_raw());
        SquintResult {
            data: std::ptr::null_mut(),
            len: 0,
            original_len,
            score: f64::NAN,
            hdr: SQUINT_HDR_ABSENT,
            quantized: 0,
            error,
            error_message,
            converted: 0,
        }
    }
}

fn code_for(e: &Error) -> c_int {
    match e {
        Error::Decode(_) => SQUINT_ERR_DECODE,
        Error::Encode(_) => SQUINT_ERR_ENCODE,
        Error::Metric(_) => SQUINT_ERR_METRIC,
        Error::TooSmall { .. } => SQUINT_ERR_TOO_SMALL,
        Error::Unreachable { .. } => SQUINT_ERR_UNREACHABLE,
        Error::NoSmallerResult { .. } => SQUINT_ERR_NO_SMALLER,
        Error::TooLarge { .. } => SQUINT_ERR_TOO_LARGE,
        Error::ColoursUnreachable { .. } => SQUINT_ERR_COLOURS,
        Error::ReadOnlyFormat { .. } => SQUINT_ERR_READ_ONLY,
        Error::Panicked => SQUINT_ERR_PANIC,
    }
}

/// Optimize an encoded image held in memory, writing the format it arrived in.
///
/// `mode` is one of the `SQUINT_MODE_` values. `png_min_quality` below 0
/// disables quantization. The input format is detected from the bytes; the
/// caller does not say. A format the engine cannot write as itself comes back
/// as JPEG with `converted` set.
///
/// # Safety
/// `input` must point to `input_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn squint_optimize(
    input: *const u8,
    input_len: usize,
    mode: c_int,
    target: f64,
    fixed_quality: f32,
    png_min_quality: c_int,
    max_dimension: c_int,
) -> SquintResult {
    squint_optimize_as(
        input,
        input_len,
        SQUINT_FORMAT_JPEG,
        mode,
        target,
        fixed_quality,
        png_min_quality,
        max_dimension,
    )
}

/// Optimize, writing the format the caller names.
///
/// `format` is one of the `SQUINT_FORMAT_` values. JPEG is what `squint_optimize`
/// asks for: a JPEG or PNG is re-encoded as itself, and anything else becomes a
/// JPEG. Naming AVIF or lossless WebP is a conversion, and the result is a
/// different kind of file from the input, reported through `converted` so the
/// caller writes it beside the original rather than over it. A PDF is rewritten
/// as a PDF whatever format is named, or refused when the name is not JPEG.
///
/// A format code that is not one of the three is refused rather than read as
/// JPEG. Collapsing an unrecognised mode into fast is what once turned strip
/// into a re-encode, and a format read wrong would overwrite a file with
/// another kind of file.
///
/// # Safety
/// `input` must point to `input_len` readable bytes.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn squint_optimize_as(
    input: *const u8,
    input_len: usize,
    format: c_int,
    mode: c_int,
    target: f64,
    fixed_quality: f32,
    png_min_quality: c_int,
    max_dimension: c_int,
) -> SquintResult {
    if input.is_null() || input_len == 0 {
        return SquintResult::failure(SQUINT_ERR_NULL_INPUT, 0, None);
    }
    let format = match format {
        SQUINT_FORMAT_JPEG => OutputFormat::Jpeg,
        SQUINT_FORMAT_AVIF => OutputFormat::Avif,
        SQUINT_FORMAT_WEBP_LOSSLESS => OutputFormat::WebpLossless,
        _ => return SquintResult::failure(SQUINT_ERR_UNKNOWN_FORMAT, input_len, None),
    };
    let bytes = std::slice::from_raw_parts(input, input_len);
    // Every mode is named here and any other value is refused. Collapsing the
    // unrecognised case into fast is what once silently turned strip into a
    // re-encode: the caller asked for the pixels to be left alone and got
    // them rewritten.
    let mode = match mode {
        SQUINT_MODE_FAST => Mode::Fast,
        SQUINT_MODE_QUALITY => Mode::Quality,
        SQUINT_MODE_STRIP => Mode::Strip,
        _ => return SquintResult::failure(SQUINT_ERR_UNKNOWN_MODE, input_len, None),
    };
    let png_min = if png_min_quality < 0 { None } else { Some(png_min_quality.min(100) as u8) };
    // Zero or below means no cap, matching how the header describes it.
    let cap = (max_dimension > 0).then_some(max_dimension as u32);

    // Nothing may unwind past this frame. It is `extern "C"`, so a panic crossing
    // it aborts the process, which in a batch means every other file in flight
    // dies with no error reported to anyone. A panic is a defect either way; the
    // difference is whether one file fails or all of them do.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        optimize_as(bytes, format, mode, target, fixed_quality, png_min, DEFAULT_PROBES, cap)
    }))
    .unwrap_or(Err(Error::Panicked));

    match outcome {
        Ok(mut out) => {
            out.data.shrink_to_fit();
            let len = out.data.len();
            let ptr = out.data.as_mut_ptr();
            std::mem::forget(out.data);
            SquintResult {
                data: ptr,
                len,
                original_len: out.original_bytes,
                score: out.score.unwrap_or(f64::NAN),
                hdr: match out.hdr {
                    Hdr::Absent => SQUINT_HDR_ABSENT,
                    Hdr::Preserved => SQUINT_HDR_PRESERVED,
                    Hdr::Dropped => SQUINT_HDR_DROPPED,
                },
                quantized: c_int::from(out.quantized),
                error: SQUINT_OK,
                error_message: std::ptr::null_mut(),
                converted: if out.converted_from.is_some() { 1 } else { 0 },
            }
        }
        Err(e) => SquintResult::failure(code_for(&e), input_len, Some(&e.to_string())),
    }
}

/// Release a buffer returned by `squint_optimize` or `squint_optimize_as`.
///
/// # Safety
/// Must be called at most once per result, and only on results this library
/// produced.
#[no_mangle]
pub unsafe extern "C" fn squint_result_free(result: SquintResult) {
    if !result.data.is_null() && result.len > 0 {
        drop(Vec::from_raw_parts(result.data, result.len, result.len));
    }
    if !result.error_message.is_null() {
        drop(std::ffi::CString::from_raw(result.error_message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with no structure to exploit, so a PNG of it is close to raw
    /// size and any lossy encode of it is far smaller: the never-grow rule can
    /// never be what decides these tests.
    fn noisy_png(side: u32) -> Vec<u8> {
        let mut state: u32 = 0x9E37_79B9;
        let img = image::RgbImage::from_fn(side, side, |_, _| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let b = state.to_be_bytes();
            image::Rgb([b[0], b[1], b[2]])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    unsafe fn call(bytes: &[u8], format: c_int, mode: c_int) -> SquintResult {
        squint_optimize_as(bytes.as_ptr(), bytes.len(), format, mode, 80.0, 75.0, -1, 0)
    }

    #[test]
    fn a_format_code_the_engine_does_not_know_is_refused() {
        let png = noisy_png(16);
        // 99 is nobody's format. Reading it as JPEG would hand the caller a
        // JPEG it did not ask for and let it be written over the source.
        let r = unsafe { call(&png, 99, SQUINT_MODE_FAST) };
        assert_eq!(r.error, SQUINT_ERR_UNKNOWN_FORMAT);
        assert!(r.data.is_null());
        assert_eq!(r.original_len, png.len());
        unsafe { squint_result_free(r) };
    }

    #[test]
    fn a_mode_code_the_engine_does_not_know_is_refused() {
        let png = noisy_png(16);
        // Read as fast, this would re-encode a file whose caller may have
        // asked for its pixels to be left alone.
        let r = unsafe { call(&png, SQUINT_FORMAT_JPEG, 7) };
        assert_eq!(r.error, SQUINT_ERR_UNKNOWN_MODE);
        assert!(r.data.is_null());
        unsafe { squint_result_free(r) };
    }

    #[test]
    fn the_plain_entry_point_still_means_the_input_format() {
        let png = noisy_png(64);
        let r = unsafe {
            squint_optimize(png.as_ptr(), png.len(), SQUINT_MODE_FAST, 80.0, 75.0, -1, 0)
        };
        // Noise may already be as small as a PNG gets, which is an outcome and
        // not a routing fault. What must hold is that nothing was converted.
        if r.error == SQUINT_OK {
            let out = unsafe { std::slice::from_raw_parts(r.data, r.len) };
            assert!(out.starts_with(&[0x89, b'P', b'N', b'G']), "a PNG stays a PNG");
            assert_eq!(r.converted, 0);
        } else {
            assert_eq!(r.error, SQUINT_ERR_NO_SMALLER);
        }
        unsafe { squint_result_free(r) };
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn asking_for_avif_reports_a_conversion() {
        let png = noisy_png(128);
        let r = unsafe { call(&png, SQUINT_FORMAT_AVIF, SQUINT_MODE_FAST) };
        assert_eq!(r.error, SQUINT_OK, "{}", unsafe {
            if r.error_message.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(r.error_message).to_string_lossy().into_owned()
            }
        });
        let out = unsafe { std::slice::from_raw_parts(r.data, r.len) };
        assert_eq!(&out[4..8], b"ftyp", "the bytes are an ISOBMFF container");
        assert_eq!(r.converted, 1, "an AVIF made from a PNG is a different kind of file");
        assert!(r.len < png.len());
        unsafe { squint_result_free(r) };
    }

    #[test]
    fn a_pdf_asked_for_as_avif_is_refused_not_converted() {
        let pdf = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n";
        let r = unsafe { call(pdf, SQUINT_FORMAT_AVIF, SQUINT_MODE_FAST) };
        assert_eq!(r.error, SQUINT_ERR_READ_ONLY);
        assert!(r.data.is_null());
        unsafe { squint_result_free(r) };
    }
}

/// A static, human-readable description of an error code. Never null, never freed.
#[no_mangle]
pub extern "C" fn squint_error_message(code: c_int) -> *const c_char {
    let s: &'static [u8] = match code {
        SQUINT_OK => b"ok\0",
        SQUINT_ERR_DECODE => b"the image could not be decoded; the file was not changed\0",
        SQUINT_ERR_ENCODE => b"the image could not be encoded; the file was not changed\0",
        SQUINT_ERR_METRIC => b"the perceptual metric failed; the file was not changed\0",
        SQUINT_ERR_TOO_SMALL => b"too small to judge perceptually. Use fast mode; the file was not changed\0",
        SQUINT_ERR_UNREACHABLE => b"the quality target cannot be reached for this image; the file was not changed\0",
        SQUINT_ERR_NO_SMALLER => b"already optimal; a smaller file is not possible\0",
        SQUINT_ERR_NULL_INPUT => b"no input was provided\0",
        SQUINT_ERR_TOO_LARGE => b"this image is too large to open safely; the file was not changed\0",
        SQUINT_ERR_PANIC => b"the engine failed unexpectedly; the file was not changed\0",
        SQUINT_ERR_COLOURS => b"this image's colours cannot be reduced that far; the file was not changed\0",
        SQUINT_ERR_READ_ONLY => b"this format can have its location data removed but cannot be shrunk yet; the file was not changed\0",
        SQUINT_ERR_UNKNOWN_FORMAT => b"the output format asked for is not one squint writes; the file was not changed\0",
        SQUINT_ERR_UNKNOWN_MODE => b"the mode asked for is not one squint has; the file was not changed\0",
        _ => b"unknown error\0",
    };
    s.as_ptr() as *const c_char
}
