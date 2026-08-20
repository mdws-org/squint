//! C interface, so the macOS application can call the engine directly.
//!
//! Ownership rule: every buffer returned here was allocated by Rust and must be
//! handed back to `squint_result_free`. Nothing else may free it.

use crate::{optimize, Error, Hdr, Mode};
use std::os::raw::{c_char, c_int};

pub const SQUINT_MODE_FAST: c_int = 0;
pub const SQUINT_MODE_QUALITY: c_int = 1;
pub const SQUINT_MODE_STRIP: c_int = 2;

pub const SQUINT_HDR_ABSENT: c_int = 0;
pub const SQUINT_HDR_PRESERVED: c_int = 1;
pub const SQUINT_HDR_DROPPED: c_int = 2;

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
}

impl SquintResult {
    fn failure(error: c_int, original_len: usize) -> Self {
        SquintResult {
            data: std::ptr::null_mut(),
            len: 0,
            original_len,
            score: f64::NAN,
            hdr: SQUINT_HDR_ABSENT,
            quantized: 0,
            error,
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
        Error::Panicked => SQUINT_ERR_PANIC,
    }
}

/// Optimize an encoded image held in memory.
///
/// `mode` is one of the `SQUINT_MODE_` values. `png_min_quality` below 0
/// disables quantization. The format is detected from the bytes; the caller
/// does not say.
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
) -> SquintResult {
    if input.is_null() || input_len == 0 {
        return SquintResult::failure(SQUINT_ERR_NULL_INPUT, 0);
    }
    let bytes = std::slice::from_raw_parts(input, input_len);
    // Every mode is named here. Collapsing the unrecognised case into fast is
    // what silently turned strip into a re-encode: the caller asked for the
    // pixels to be left alone and got them rewritten.
    let mode = match mode {
        SQUINT_MODE_QUALITY => Mode::Quality,
        SQUINT_MODE_STRIP => Mode::Strip,
        _ => Mode::Fast,
    };
    let png_min = if png_min_quality < 0 { None } else { Some(png_min_quality.min(100) as u8) };

    // Nothing may unwind past this frame. It is `extern "C"`, so a panic crossing
    // it aborts the process, which in a batch means every other file in flight
    // dies with no error reported to anyone. A panic is a defect either way; the
    // difference is whether one file fails or all of them do.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        optimize(bytes, mode, target, fixed_quality, png_min)
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
            }
        }
        Err(e) => SquintResult::failure(code_for(&e), input_len),
    }
}

/// Release a buffer returned by `squint_optimize`.
///
/// # Safety
/// Must be called at most once per result, and only on results this library
/// produced.
#[no_mangle]
pub unsafe extern "C" fn squint_result_free(result: SquintResult) {
    if !result.data.is_null() && result.len > 0 {
        drop(Vec::from_raw_parts(result.data, result.len, result.len));
    }
}

/// A static, human-readable description of an error code. Never null, never freed.
#[no_mangle]
pub extern "C" fn squint_error_message(code: c_int) -> *const c_char {
    let s: &'static [u8] = match code {
        SQUINT_OK => b"ok\0",
        SQUINT_ERR_DECODE => b"the image could not be decoded\0",
        SQUINT_ERR_ENCODE => b"the image could not be encoded\0",
        SQUINT_ERR_METRIC => b"the perceptual metric failed\0",
        SQUINT_ERR_TOO_SMALL => b"too small to judge perceptually; use fast mode\0",
        SQUINT_ERR_UNREACHABLE => b"the quality target cannot be reached for this image\0",
        SQUINT_ERR_NO_SMALLER => b"already optimal; a smaller file is not possible\0",
        SQUINT_ERR_NULL_INPUT => b"no input was provided\0",
        SQUINT_ERR_TOO_LARGE => b"this image is too large to open safely; the file was not changed\0",
        SQUINT_ERR_PANIC => b"the engine failed unexpectedly; the file was not changed\0",
        _ => b"unknown error\0",
    };
    s.as_ptr() as *const c_char
}
