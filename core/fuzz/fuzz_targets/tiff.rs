//! The TIFF directory walker: a tag's declared size can overrun into the
//! pixels, and the strip must destroy bytes only where no strip of picture
//! lives.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::tiff;

fuzz_target!(|data: &[u8]| {
    if !tiff::is_tiff(data) {
        return;
    }
    if let Some((out, wiped)) = tiff::strip_tiff(data) {
        // The directory is rewritten shorter in place; nothing moves.
        assert_eq!(out.len(), data.len());
        assert!(wiped <= data.len());
    }
});
