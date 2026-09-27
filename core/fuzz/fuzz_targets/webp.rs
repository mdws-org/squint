//! The RIFF walker: chunk lengths are declared by the file, odd lengths carry
//! a pad byte the length does not count, and the walk must land exactly on
//! the end of the file.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::webp;

fuzz_target!(|data: &[u8]| {
    if !webp::is_webp(data) {
        return;
    }
    let _ = webp::is_animated(data);
    let _ = webp::icc_profile(data);
    let _ = webp::orientation(data);
    if let Some((out, wiped)) = webp::strip_webp(data) {
        // Chunks are removed, never added.
        assert!(out.len() <= data.len());
        assert!(wiped <= data.len());
    }
});
