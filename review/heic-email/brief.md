# Review: squint 0.2.0 → HEIC input for Shrink for Email — round 1

You are reviewing a patch to `squint`, a macOS image optimizer (Rust engine in `core/`, SwiftUI app in `app/`, GPL-3, public). Answer as a senior systems engineer who has shipped Core Graphics FFI from Rust and has been burned by it. Be specific and concrete: file, line, mechanism, failing input. No compliments; do not restate the patch back to me. Rank findings by expected impact. Under 1500 words. Read the files listed at the end; run nothing; write nothing; answer in your final message.

## The target

An iPhone camera writes HEIC. The Finder entry **Squint: Shrink for Email** — a preset that encodes at JPEG quality 75, caps the long edge at 2048 px (Lanczos, never enlarges) and writes BESIDE the original as `name-email.jpg` — refused HEIC because the Rust engine had no HEIC decoder. After the patch the engine decodes HEIC through the system's Image I/O (hand-written `extern "C"` bindings, no new crates, `cfg(target_os = "macos")`), so both the app and the CLI can read it, and the Email preset writes `name-email.jpg` from a `.heic`.

Non-negotiable invariants of this project:

1. **Never write JPEG bytes over a `.heic`.** In-place modes (Shrink, Shrink to a Quality Target) must not accept HEIC; only the write-beside preset may.
2. **Never convert colour to sRGB.** iPhone photos are Display P3. Pixels stay in the source space; the source ICC profile is embedded in the output JPEG.
3. **Never grow a file** — an optimizer that returns a bigger file keeps the original (`Error::NoSmallerResult`).
4. **Losing HDR is reported, never silent.** A HEIC carries an ISO 21496 / Apple gain map; the JPEG output cannot carry it (a known mozjpeg incompatibility, out of scope), so the result must report `Hdr::Dropped`.
5. **Decompression bombs are refused before allocation** (`MAX_PIXELS = 250M`, checked from the header).
6. **The FFI never unwinds into C**; buffers returned to Swift are freed by `squint_result_free`.

## What it does now (the patch)

- `core/src/imageio.rs` (new): `decode(bytes, max_pixels) -> Decoded { rgb, width, height, icc, orientation }`. `CFDataCreate` → `CGImageSourceCreateWithData` → properties (pixel width/height/orientation via `CFDictionaryGetValue` + `CFNumberGetValue`) → bomb check → `CGImageSourceCreateImageAtIndex` → if the image's colour space model is RGB, draw into a `CGBitmapContextCreate` context using THAT colour space and copy `CGColorSpaceCopyICCData`; otherwise a device-RGB context with no ICC. Bitmap info `kCGImageAlphaNoneSkipLast` (RGBX), byte order default. Fourth byte dropped. A `Cf<T>` guard calls `CFRelease` on drop.
- `core/src/source.rs` (new): `Source::open(bytes, cap)` — HEIF → `imageio::decode`, cap via the existing Lanczos `capped()`, then `apply_orientation`; JPEG/PNG → the pre-existing `image`-crate path. Carries `icc`, `orientation`, `converted_from: Option<&'static str>`.
- `core/src/lib.rs`: `optimize()` no longer refuses HEIC for Fast/Quality; uses `Source::open`; `Optimized.converted_from`; HDR reported `Dropped` when `heif::signals_hdr` (needle scan for `hdrgainmap` / `urn:iso:std:iso:ts:21496`) fires on a converted source.
- `core/src/ffi.rs` + `core/include/squint.h`: `SquintResult` gains a trailing `int converted`.
- `core/src/main.rs`: CLI uses `Source::open`; the TIFF-only early refusal remains.
- Swift: `Engine.Result.converted`; `Preset.destination(for:converted:)` switches the extension to `jpg`; `JobQueue.process` refuses HEIC for in-place presets BEFORE the engine call (by extension) and AFTER it (if `converted` and the destination equals the source); `project.yml` adds `public.heic`/`public.heif` to the Email entry only.
- Tests: 38 pass on macOS including a `sips`-generated 1200x800 HEIC round trip (dimensions, corner pixels, orientation 1, 150 px JPEG output).

Measured on real iPhone 16 Pro HEICs (5712x4284, Display P3, ISO gain map, one with orientation 6): see `measurements.txt`.

Settled and NOT under review: the perceptual search, the strip/metadata code (`metadata.rs`, `heif.rs` strip path, `tiff.rs`, `gainmap.rs`, `png.rs`), the 2048 cap value, the decision to write JPEG beside rather than re-encode HEIC, and the mozjpeg-vs-gain-map incompatibility.

## Questions

1. **Memory and ownership in `imageio.rs`** — every Create/Copy released exactly once, nothing released that was obtained by Get, no use after release, no leak on an early `?` return. Name the line if one is wrong. Is the `CGImageGetColorSpace` pointer (Get rule) kept alive long enough for `CGBitmapContextCreate`?
2. **Pixel correctness** — with `kCGImageAlphaNoneSkipLast | kCGBitmapByteOrderDefault` on an 8-bpc RGB context, is the memory order R,G,B,X on Apple silicon? Does `CGContextDrawImage` into a same-sized context ever flip, letterbox, or resample? What happens for a 16-bit or float-HDR HEIC source, a greyscale HEIC, an image whose colour space is RGB but has no ICC data, a multi-image HEIC (burst / live photo — is index 0 the right item?), and an animated HEIF?
3. **Bomb and size guards** — can a HEIC whose declared properties disagree with the decoded `CGImageGetWidth/Height` get past `MAX_PIXELS`? Is there any path where Image I/O allocates the full picture before our check? Is `w*h*4` overflow handled?
4. **The in-place safety net in Swift** — find an input path (drop on window, Services entry, a `.HEIF` upper-case extension, a HEIC named `.jpg`, a JPEG named `.heic`) where JPEG bytes could be written over a non-JPEG file, or where a `.heic` named `.jpg` bypasses the pre-check and gets replaced in place. Is `converted` derived from bytes or from extension, and does that matter here?
5. **HDR reporting** — can `signals_hdr` return false on a HEIC that does carry a gain map (the needle is `hdrgainmap`; the ISO form's marker in a HEIF is a box, not text)? Can it return true on one that does not?
6. **Behavioural regressions** — anything in the JPEG/PNG path that changed behaviour? (`Source::open` was meant to be a pure lift of four lines.) The CLI no longer calls `Image::decode` directly — any measurement the harness printed before that it no longer prints?
7. **Portability** — does the crate still `cargo metadata --locked` and compile on Linux CI (the macOS module is `cfg`-gated; the test too)?

Read the files listed below. Do not edit any files.

## Files to read (absolute paths)

- /Users/bemeadows/Projects/.lanes/squint-heic-wt/review/heic-email/patch.diff — the whole change against origin/main; read this first
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/review/heic-email/measurements.txt
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/core/src/imageio.rs
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/core/src/source.rs
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/core/src/lib.rs
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/core/src/ffi.rs
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/core/include/squint.h
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/core/src/main.rs
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/app/Sources/JobQueue.swift
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/app/Sources/Preset.swift
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/app/Sources/Engine.swift
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/app/Sources/Job.swift
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/app/Sources/ServiceProvider.swift
- /Users/bemeadows/Projects/.lanes/squint-heic-wt/app/project.yml
