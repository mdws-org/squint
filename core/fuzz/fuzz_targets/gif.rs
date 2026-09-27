//! The GIF block walker: sub-block chains, extension blocks, and the
//! application blocks the strip wipes without naming.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::gif;

fuzz_target!(|data: &[u8]| {
    if !gif::is_gif(data) {
        return;
    }
    if let Some((out, wiped)) = gif::strip_gif(data) {
        assert!(out.len() <= data.len());
        assert!(wiped <= data.len());
    }
});
