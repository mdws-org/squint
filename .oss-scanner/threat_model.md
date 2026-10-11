# Threat model

## What this project does and where untrusted input enters

Squint is a macOS image optimizer. A user right-clicks files in Finder or drops them on the window, and the Rust engine in `core/` re-encodes them smaller or strips their metadata, usually writing the result over the original. Every input file is untrusted: photos and documents arrive from email, chat and the web.

The attack surface is the engine's parsers and decoders: the JPEG segment walker (including the MPF index and HDR gain map), the PNG chunk walker, the TIFF directory walker, the ISOBMFF box walker (HEIC and AVIF), the RIFF chunk walker (WebP), the GIF block walker, the PDF object walker (through `lopdf`), the SVG rasterizer (`resvg`), and the image decoders behind Fast and Quality mode.

## Components that matter most / least

- Most: `core/src/` — `metadata.rs`, `png.rs`, `tiff.rs`, `heif.rs`, `webp.rs`, `gif.rs`, `pdf.rs`, `svg.rs`, `gainmap.rs`, `source.rs`, `imageio.rs`, and the C boundary in `ffi.rs`.
- In scope but lower: `core/src/lib.rs` search and scoring logic.
- Out of scope: the Swift app in `app/` (it needs Xcode and does not build in this image), `tools/` (test corpus scripts), `docs/`, `design/`, `review/`. HEIC and AVIF decode through Apple's Image I/O, which exists only on macOS; on Linux the engine refuses them before any decoder runs.
- Third-party crates are in scope only where Squint passes them input it did not validate first.

## How to exercise it

- CLI: `core/target/debug/squint <file> [--mode fast|quality|strip] [--out <path>]`. `strip` removes metadata without re-encoding; `fast` and `quality` decode and re-encode.
- Fuzz targets: `core/fuzz/fuzz_targets/` (`strip`, `open`, `png`, `tiff`, `heif`, `webp`, `gif`, `pdf`), already built. Run one with `cd /src/core && cargo +nightly fuzz run <target> -- -max_total_time=60`.
- Tests: `cd /src/core && cargo test --locked`.

## How you rate severity

The engine's own rules (from `README.md` and `SECURITY.md`) are the bar:

- Critical: a malformed file makes Squint write a wrong or truncated output over the original while reporting success, because the original is then gone. A stripper that returns what it managed to copy is this defect: a truncated file is smaller than the original and passes every later check.
- High: memory unsafety reachable from a file (any `unsafe` misuse, out-of-bounds in a C dependency through the input Squint passes it); a panic reachable from a file, because a panic crossing the C boundary aborts the process and takes every other file in the batch with it; reading any file other than the one handed in (for example an SVG that names a local file).
- High: location, camera or timestamp metadata surviving a path that claims to remove it (the users strip job-site photos that geotag client addresses before posting them).
- Medium: unbounded allocation or a hang from a declared size or a crafted structure; any check of a declared image size that happens after the allocation.
- Low: a typed error where a valid file should have been accepted.

## Anything to leave alone

- Dropping the ICC colour profile is a bug but not a security finding; report it as low.
- Lossy output differing from the input is the product working; only a wrong file written in place of the original matters.
- The licence of linked crates (GPL-3.0-or-later with libimagequant) is deliberate.
