//! The PDF object walker, through lopdf, in the one mode that re-encodes
//! nothing: the info dictionary and the XMP packet go, the pages are
//! untouched. Fast and Quality run the same walk before encoding and are
//! covered by the `open` target's cost budget rather than here.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::{pdf, Mode};

fuzz_target!(|data: &[u8]| {
    if !pdf::is_pdf(data) {
        return;
    }
    let _ = pdf::page_count(data);
    let _ = pdf::rewrite(data, Mode::Strip, 0.0, 0.0, 0, None);
});
