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
        // Pixels, the colour profile and kept data come out byte for byte.
        let keep = tiff::unchangeable(data).expect("a stripped tiff was walked");
        for (start, end) in keep {
            let end = end.min(data.len());
            if start < end {
                assert_eq!(&out[start..end], &data[start..end], "bytes {start}..{end} changed");
            }
        }
    }
});
