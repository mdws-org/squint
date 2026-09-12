An iPhone camera writes HEIC, and the one preset that matters for a job photo, Shrink for Email, refused it: the engine had no HEIC decoder, so `optimize()` returned `ReadOnlyFormat` for every mode except Strip. Squint could remove the location from a camera-roll photo but could not make the email copy of it.

## What changed

The Rust core now decodes HEIF through the system's Image I/O (`core/src/imageio.rs`), with hand-written `extern "C"` bindings to the twelve C functions involved. No new crates; `Cargo.lock` is unchanged. The module is `cfg(target_os = "macos")`; on Linux a HEIC still returns `ReadOnlyFormat`, so CI's `cargo metadata --locked` and a Linux build are unaffected.

Pixels come back in the source colour space, not converted to sRGB: the bitmap context is created with the image's own colour space, and that space's ICC profile is what the JPEG carries. When the system has no profile to give for the space (or the model is not RGB), the picture is converted to sRGB and left untagged, so an untagged file never holds wide-gamut values. The picture decoded is the container's declared primary item (`CGImageSourceGetPrimaryImageIndex`), not item 0. The declared size is checked against `MAX_PIXELS` before the picture is created, and a file that declares no size is refused rather than decoded unchecked. Whether the source carries a gain map is asked of Image I/O (`CGImageSourceCopyAuxiliaryDataInfoAtIndex`, Apple and ISO 21496 types) rather than guessed from the bytes. A file cut short is refused before Image I/O sees it: `heif::is_complete` walks the `iloc` index and rejects any extent that ends past the file, because Image I/O reports such a file as complete and draws what it has — measured on a 6 MB photograph cut to 3 MB, which came out as a 10 KB mostly empty JPEG with no error until this check existed. Rows are read at the stride the bitmap context reports, not the one requested, since Quartz may pad.

`core/src/source.rs` is the single way in for anything the engine re-encodes. `Source::open` decodes JPEG and PNG as before, hands a HEIF to Image I/O, caps the long edge with the existing Lanczos path, bakes the orientation in, and reports `converted_from`. Both `optimize()` and the CLI use it, so the harness measures what the app does.

The C API gains one trailing field, `converted`, non-zero when the output is a JPEG made from another format. On the Swift side the output's extension comes from the output's own bytes (`Engine.Result.outputExtension`), not from the input's name: a JPEG that Dropbox's camera upload saved as `IMG_1234.heic` comes out as `IMG_1234-email.jpg`. `JobQueue` refuses to write when a converted result would land on the source path, and a HEIC (judged by its `ftyp` box, not its name) handed to an in-place preset is refused before the engine runs, with a message naming Shrink for Email. The Email entry's `NSSendFileTypes` gains `public.heic` and `public.heif`; the three in-place entries do not.

A JPEG made from a HEIC cannot carry the gain map (the mozjpeg incompatibility recorded in `lib.rs`), so the result reports `Hdr::Dropped` whenever the source declares one.

## Measured

Two iPhone 16 Pro HEICs, 5712x4284, Display P3, ISO gain map, GPS, one of them orientation 6, on an Apple M5:

| input | output (fast, cap 2048) | time |
|---|---|---|
| 6,162,084 B, orientation 1 | 667 KB, 11.1% | 0.52 s |
| 5,446,125 B, orientation 6 | 532 KB, 10.0%, upright | 0.49 s |

Both outputs read back through ImageIO as 2048 on the long edge, Display P3, orientation 1, no EXIF, no GPS, no MakerApple. Segments: JFIF, ICC_PROFILE, DQT, SOF2, DHT, SOS. Quality mode at target 80 on the first photo: q91, 1,175 KB, score 80.13, 4 probes, 2.6 s. The 24 MP JPEG through the old and new binaries produces byte-identical output, so the JPEG path did not move.

Tests: 37 pass. The new macOS test writes a 1201x800 noisy picture (a row width no Quartz alignment divides), converts it to HEIC with `sips`, decodes it through `Source::open`, checks pixels on two rows whose values differ by far more than the tolerance (so a flipped or sheared decode fails), checks orientation and the absence of a gain map, re-encodes it to a 150 px JPEG, and requires the same file cut to 60% to be refused.

Reviewed by three outside models before this PR; the record, with what was confirmed, what was refuted by measurement and what was left alone, is in `review/heic-email/`. A smooth gradient was tried first and codes to 1,609 B as HEIC; the 150 px JPEG of it was 4,191 B, and `NoSmallerResult` refused it, which is the engine being right and the test being wrong.

## Not in this change

No new Finder entry. The in-place entries still refuse HEIC. TIFF is still metadata-only. The gain map is still not carried through a re-encode.
