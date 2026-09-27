//! The decode door. Every format Fast and Quality read arrives here as bytes
//! and leaves as RGB, so this is where a decoder allocation, a dimension
//! overflow, or a rasterizer given a hostile drawing would show. HEIC and
//! AVIF decode through Image I/O and are only reachable on macOS; on Linux
//! they are refused before any decoder runs, which is itself the behaviour
//! under test. The cap keeps a declared 60,000 pixel square from being
//! allocated.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::Source;

fuzz_target!(|data: &[u8]| {
    let _ = Source::open(data, Some(512));
});
