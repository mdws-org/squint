//! Strip mode on arbitrary bytes. This is the path a right-click reaches with
//! every file type the engine knows, dispatched on magic: JPEG, PNG, GIF,
//! TIFF, HEIF and AVIF, WebP, PDF. A panic here is a crash of the
//! application; the engine's own rule is that a malformed file leaves with a
//! typed error and the original untouched.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::{optimize, Mode};

fuzz_target!(|data: &[u8]| {
    let _ = optimize(data, Mode::Strip, 80.0, 75.0, Some(70), 1, None);
});
