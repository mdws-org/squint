//! The ISOBMFF walker: box sizes are declared by the file and every `iloc`
//! offset is absolute, which is where an overflow or an out-of-range slice
//! would live.
#![no_main]
use libfuzzer_sys::fuzz_target;
use squint_core::heif;

fuzz_target!(|data: &[u8]| {
    let _ = heif::is_heif(data);
    let _ = heif::is_avif(data);
    let _ = heif::is_image_sequence(data);
    let _ = heif::is_complete(data);
    if heif::is_isobmff_image(data) {
        let _ = heif::container_name(data);
        if let Some((out, _wiped)) = heif::strip_heif(data) {
            // Strip overwrites in place, so the length never changes.
            assert_eq!(out.len(), data.len());
        }
    }
});
