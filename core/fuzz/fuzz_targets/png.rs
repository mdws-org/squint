//! The PNG chunk walker: the strip keeps a named list and drops the rest, and
//! the colour chunks are carried across a re-encode by name.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::png;

fuzz_target!(|data: &[u8]| {
    if !data.starts_with(&[0x89, b'P', b'N', b'G']) {
        return;
    }
    let carried = png::colour_chunks(data);
    let _ = png::with_colour_chunks(data, &carried);
    if let Some(out) = png::strip_png(data) {
        assert!(out.len() <= data.len());
    }
});
