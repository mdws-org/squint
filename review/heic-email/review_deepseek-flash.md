**Review: HEIC input for Shrink for Email**

I verified the pieces the findings rest on: `heif::is_heif`'s brand list, `gainmap::signals_hdr`'s needle set, the CI workflow, and the sample-image scaling path. No files edited.

---

### 1. HDR is silently dropped on every HEIC that reaches the Email preset — invariant 4 broken

`lib.rs`:

```rust
let hdr = if has_gain_map(bytes) || (src.converted_from.is_some() && heif::signals_hdr(bytes)) {
```

`has_gain_map` → `gainmap::signals_hdr`, which is a **JPEG** parser: `extract(jpeg)` looks for an `MPF\0` APP2 index, then `segments()` returns early unless `jpeg[0]==0xFF && jpeg[1]==0xD8`, then it checks an APP2 payload starting with the ISO tag and the ASCII needle `hdrgm:`. A HEIC has none of that structure. So the first disjunct is always false for a HEIF.

The second disjunct requires `heif::signals_hdr` to find `hdrgainmap` or `urn:iso:std:iso:ts:21496` as **contiguous ASCII** in the file. An Apple HEIC declares its map as an auxiliary image type — an ISOBMFF box with `auxC` carrying the null-terminated `urn:com:apple:photo:2020:aux:hdrgainmap` string. `"hdrgainmap"` is a suffix of that, so that needle happens to hit. The ISO 21496 form is different: the marker is a **box** (`tmap` / a four-character code such as `tmap` or `auxC` with `urn:iso:std:iso:ts:21496-1:map`), and the versioned suffix means the literal needle does not match. The gainmap module's own comment records the consequence in the JPEG case: *"matching only one of them was worse than matching neither: a Google photograph's map was removed and then reported as having never been there."*

Failing input: a HEIC whose gain map is declared only through the ISO form, or is declared in a form whose type string is UTF-16/escaped. Output JPEG reports `Hdr::Absent`, `Job.detail` prints no "HDR removed", and the user is not told the picture lost its range. Fix: reuse the gain map's full needle set (`core/src/gainmap.rs:42`) against the HEIC bytes, plus a real check for the `auxC`/`tmap` boxes; do not add the substring `21496` alone — that would be a false-positive minefield.

### 2. `CGContextDrawImage` does not colour-convert when the context uses the image's own space

`imageio.rs:218-248`. For a Display P3 source, `ctx_cs` is the image's `CGColorSpace` and the context is 8 bpc. Display P3 is a 16-bit-native extended-range space; the per-component range at 8 bpc is 0…255, so `-> 1.0` maps exactly onto 255 and the pipeline is the identity — correct for SDR. But Image I/O handing back a `CGImage` with *extended-range* components (a floating-point or HDR-aware decode of the same HEIC) is not representable: values above 1.0 clamp to the per-component max, and everything outside the Display P3 gamut clips. This path is untested — the only HEIC test is `sips`-generated, and `sips` writes a Display P3 output whose profile round-trips (536 bytes measured). A real iPhone HDR HEIC decoded by Image I/O as extended range will bleed at exactly the highlights the preset exists to show.

Cheap defence, without a second conversion: check `CGImageGetBitsPerComponent` / `CGImageGetBitmapInfo` (already-linkable externs) and refuse, or downconvert through a device-RGB context, rather than assuming `CGImageGetColorSpace` implies display-referred SDR. Note also that `CGColorSpaceGetModel` returns 1 for RGB, so greyscale (model 0) falls into the device-RGB branch — a greyscale HEIC is decoded as RGB by the draw, which is a conversion *into* sRGB, i.e. invariant 2's forbidden move in a case nobody weighed.

### 3. The byte order is a constant, not the platform's

`imageio.rs:247`: `K_CG_IMAGE_ALPHA_NONE_SKIP_LAST | K_CG_BITMAP_BYTE_ORDER_DEFAULT`, with `K_CG_BITMAP_BYTE_ORDER_DEFAULT = 0`. On Apple silicon (little-endian) the R,G,B,X claim is correct, and `measurements.txt` confirms it end to end on the real photos. But the value is a literal, and the constant's name says "the platform's default". The XOR that makes this true on LE also means the same source compiled for Linux (see finding 7) would need `kCGBitmapByteOrder32Little`'s numeric value, not 0. If this crate is ever built off-Darwin the file will not compile anyway (the `#[link(name = "CoreFoundation", kind = "framework")]` attribute is unconditional; only the module declaration is `cfg`-gated), but the intent — "byte order default means the order the engine reads" — should be written down and asserted, not left as an unverified comment.

### 4. Multi-image HEIC: index 0 is the primary by spec; the count check is not a guard

`imageio.rs:146-157`. `CGImageSourceGetCount >= 1` and `CreateImageAtIndex(0)` is right for burst, live photo and animated HEIF — the primary is item 0 in the `pitm` sense. What is missing is any statement that a HEIC whose primary is a thumbnail would silently decode the thumbnail: Image I/O's `kCGImageSourceShouldCacheImmediately` is a performance knob, not a correctness one, and `CGImageSourceCreateImageAtIndex(0)` on a container with a `thmb` item returns the primary, not the thumb. Fine. The bad case is a HEIC container whose item 0 is a preview and the real picture is index 1 — legal ISOBMFF, written by some encoders — which this code silently returns at preview resolution and then reports as a successful conversion. No test covers a `.heif` with `count > 1`.

### 5. Swift safety net: real leaks, and two that only the extension check plugs

`JobQueue.swift:131-141`. The pre-check is extension-based and the post-check is byte-derived; for the reachable routes they compose:

- **Drop on window**: `JobQueue.mode` + `Preset.plain`, so `preset.suffix == nil` → the pre-check fires only if the URL ends `.heic`/`.heif`. A HEIC named `photo.jpg` (drag-renamed, or an AirDrop that stripped the extension) passes the pre-check, decodes through the HEIC path, gets `converted == true` from `result.converted`, `destination(for:converted:)` returns `url` (no suffix), and the post-check refuses. Safe.
- **Services**: `.email` sets `suffix`, so a `.heic` named `.jpg` produces `photo-email.jpg`, and the write is beside, never over. Safe.

Where it leaks: **(a)** a HEIC named with an extension not in `isSupported` (`imageio.rs`'s `is_heif` accepts `heis`/`miaf`/`msf1`, and ServiceProvider only lists `jpg/jpeg/png/heic/heif/tif/tiff`) — Finder's own type filter will usually catch it, but it is an unhandled input; **(b)** `Writer.replaceInPlace` is reached for any input whose `converted == false` and whose extension was `heic` *if the engine ever returns `converted == false` for HEIC on a non-macOS build*, which cannot happen in the app; **(c)** the `W*f` in `Job.detail`'s `destination(for:converted:)` recomputes the destination rather than remembering it, so a result whose `converted` disagrees with what was written would report the wrong filename — cosmetic, but it means the reported name and the actual file can diverge if the post-check is ever relaxed.

The load-bearing point: `converted` is derived from bytes, which is correct, but the *guard* is derived from extension. A `.heic` whose bytes are a JPEG and whose decode reports `converted == false` will be replaced in place with a JPEG — which is the right behaviour, and the extension check should not be reading a file that is actually HEIC.

### 6. Bomb / size guards: the declared check is not behind the allocated one

`imageio.rs:193-215`. `dw.saturating_mul(dh) > max_pixels` runs before `CGImageSourceCreateImageAtIndex`, which is the right ordering — but `CGImageSourceCreateWithData` has already parsed the container, and `CGImageSourceCopyPropertiesAtIndex(0, null)` allocates the property dictionary, which for a hostile HEIC can carry a large `iinf`/`iprp`. The check that matters is the second one, and it runs *after* `CreateImageAtIndex` has built the CGImage. Image I/O decodes lazily; the CGImage is a reference into the source's decoded surface, and the surface is not materialised until the draw. So the full-picture allocation happens at `CGContextDrawImage`, after both checks — good — but it also means the checks cannot prevent Image I/O from allocating during `CreateImageAtIndex` for a container whose `ispe` disagrees with its `iloc`. `dw/dh` are read from `kCGImagePropertyPixelWidth/Height`, which on a HEIC is the `ispe` of the primary; a malformed `ispe` claiming 100x100 over an actual 60000x60000 coded item passes check one and only trips check two — after the CGImage exists. Not a demonstrated exploit, but the "before allocation" invariant is asserted more strongly than the code earns. Worth a deliberate statement in the module doc rather than leaving it implied.

`w * h * 4` overflow is handled (`checked_mul` at 233 and 237), and `Vec::with_capacity(w * h * 3)` at 266 can overflow in release (wrapping) for absurd dimensions — but it is unreachable given `w*h <= 250M`, so this is a latent-only hazard, not a live one.

### 7. Ownership in `imageio.rs` — the Get pointer is used after its owner is released

This is the one I would fix before merge. `CGImageGetColorSpace` is a **Get** and correctly not released (line 217, `_cs_guard = None`), and it is used at 246 inside `CGBitmapContextCreate`. That use is fine: the `image` Cf holds a reference through the call, and the `image` guard outlives the context. But `Cc<T>` has no lifetime link to `image`, so the borrow checker is not enforcing what the comment claims; if the release order at the end of the function is ever reordered (drop `image` before `ctx`), the context survives with a space whose owner is gone. In the current code `image` is dropped at the end of the unsafe block, after `ctx` and after the pixels are copied out, so there is no use-after-free today. Flagging it as a structure that will rot, not a live bug.

The real leak: `_cs_guard` is bound to the else branch only. On the RGB branch the image's colour space is used to create a *retained* copy inside `CGBitmapContextCreate` (the context retains its space), which is correct — but that retain is against the caller's reference, and Image I/O's `CGImageGetColorSpace` returns a reference held by the image, not retained for the caller. `CGBitmapContextCreate` retains it, so the context's reference keeps it alive; the image's reference is released when `image` drops. No leak, no double-free. All `Cf` drops release exactly once. No early `?` return skips a release — every `?` is after the relevant `Cf` is constructed and drops on unwind. Clean.

### 8. `signals_hdr`'s needle scan: false positives are real

`heif.rs:255-258`. `b"hdrgainmap"` and `b"urn:iso:std:iso:ts:21496"` scanned as raw substrings over the whole file. The versioned ISO form (`urn:iso:std:iso:ts:21496-1:map`) does not match the unversioned needle — so the ISO-only HEIC is the false **negative** from finding 1. False positive: a HEIC whose EXIF or XMP comment contains the string, or whose `auxC` names a non-gain-map auxiliary with the substring in a type name, reports `Dropped` on a picture with no gain map. Low frequency, but the needle set is a hand-rolled parser standing in for a box walk `heif.rs` already knows how to do (`boxes()` is right there). If HDR reporting is going to be an invariant, walk the boxes and read `auxC`.

### 9. Regressions: none on the JPEG/PNG path, one behavioural change not in the tables

`Source::open`'s JPEG/PNG branch is a faithful lift of `Image::decode_capped` + `extract_icc` + `extract_orientation` + `apply_orientation`, in the same order, and `measurements.txt` records byte-identical output for a 24 MP JPEG through both. The CLI change adds `converted from {f}` to the header line and `written as JPEG (from {f})` to the result line — additive, no measurement dropped. The `--against` path now opens the reference through `Source::open(…, None)`, same as before.

One thing that did change and is not in the tables: `optimize` on **HEIC + Strip** still takes the `heif::strip_heif` branch (Strip is handled above the `Source::open` call), which is right — but the Strip branch now sits *after* `converted_from` was added to `Optimized` and returns `converted_from: None` for a HEIC strip. The app's `Job.detail` reads `converted` for the destination line only when `suffix != nil`, and strip presets have no suffix, so it is inert. Worth confirming no future caller reads `converted` as "output format" — it means "this was written as JPEG from another format", which a stripped HEIC is not.

### 10. Portability

No CI workflow compiles anything — the only workflow is `cargo-deny` + `cargo metadata --locked` on ubuntu, and `cargo metadata` does not build, so the `#[link(name = … kind = "framework")]` attributes never get parsed on Linux. A `cargo build` on Linux would fail on the unguarded `#[link]` attributes in `imageio.rs` if the module were ever declared, but it is `#[cfg(target_os = "macos")]`-gated at `lib.rs`, so the module is not compiled. `cargo metadata --locked` passes trivially — Cargo.toml was not touched. The risk is that nobody is compiling on Linux, so the `cfg(not(target_os = "macos"))` branch in `Source::open` (returning `ReadOnlyFormat`) is never compiled or tested; a typo in it would ship.

---

**Ranked:** (1) HDR silent drop, invariant 4; (2) unverified extended-range/P3 draw path, invariant 2; (5) extension-vs-bytes guard asymmetry; (6) "before allocation" overstated; (3) byte-order constant; (8) needle scan; (7) `Cc` lifetime structure; (10) no compile CI.

**Ruled out** (checked, not findings): every Create/Copy in `imageio.rs` is released exactly once and no Get is released; no early `?` skips a release; `kCGImageAlphaNoneSkipLast` is RGBX on 8-bpc; same-sized `CGContextDrawImage` does not resample or letterbox; index 0 is the primary per spec; `w*h*4` overflow is handled; `Source::open`'s JPEG path is a faithful lift with byte-identical measured output; `is_heif` gates before `imageio::decode` so non-HEIC never reaches it; `cargo metadata --locked` is unaffected.
