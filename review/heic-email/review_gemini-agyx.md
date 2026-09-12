### Findings (Ranked by Impact)

---

### 1. CRITICAL: Quartz Bitmap Context Draws Pixels Vertically Upside Down
* **File & Line:** `core/src/imageio.rs`, lines 498–516 (`CGBitmapContextCreate` and `CGContextDrawImage`).
* **Mechanism:** In Core Graphics (Quartz 2D), the default coordinate system of a `CGBitmapContext` places the origin `(0,0)` at the **bottom-left** corner with Y pointing upward. However, raw memory buffers returned by `CGBitmapContextGetData` layout pixel rows top-to-bottom starting at row 0. Passing a `CGImage` directly to `CGContextDrawImage` without flipping the Context Transformation Matrix (CTM) renders the image vertically inverted into the pixel array.
* **Failing Input:** Any valid HEIC image (e.g., `iphone-a.heic`). The resulting JPEG buffer is flipped upside down along the Y-axis.
* **Why the test passed:** In `core/src/source.rs:909-913`, the assertion `(p[c] - want[c]).abs() <= 16` allowed a tolerance of 16 counts for HEVC lossy compression. In the generated 1200x800 test pattern, row 10 vs row 789 produced a green channel value of 10 vs 21 (`|21 - 10| = 11 <= 16`), allowing vertical pixel inversion to pass the test silently.
* **Fix:** Apply a CTM flip prior to drawing:
  ```rust
  CGContextTranslateCTM(ctx.as_ptr(), 0.0, h as f64);
  CGContextScaleCTM(ctx.as_ptr(), 1.0, -1.0);
  ```

---

### 2. HIGH: Unlabeled Display P3 Pixels When `CGColorSpaceCopyICCData` Fails
* **File & Line:** `core/src/imageio.rs`, lines 475–489.
* **Mechanism:** When `CGColorSpaceGetModel(cs)` is `K_CG_COLOR_SPACE_MODEL_RGB`, `imageio.rs` passes `cs` directly to `CGBitmapContextCreate` so pixels remain in their native wide-gamut space (e.g., Display P3). If `CGColorSpaceCopyICCData(cs)` returns `NULL` (which happens for certain system-provided or programmatically constructed `CGColorSpace` instances lacking an uncompressed ICC profile block), `icc` evaluates to `None`. The Rust engine then writes raw Display P3 pixel coordinates into an un-profiled JPEG.
* **Failing Input:** Any HEIC image tagged with a system RGB color space where `CGColorSpaceCopyICCData` returns `NULL`.
* **Impact:** Standard JPEG renderers assume un-profiled JPEGs are sRGB, producing distorted, dull, and shifted colors (violating Invariant 2).

---

### 3. HIGH: Memory Allocation Bomb Check Bypassed on Missing Property Keys
* **File & Line:** `core/src/imageio.rs`, lines 417–454 vs 457–468.
* **Mechanism:** If `CGImageSourceCopyPropertiesAtIndex` returns `NULL` or lacks `kCGImagePropertyPixelWidth` / `Height` dictionary entries, `declared_width` and `declared_height` remain `0`. The first guard (`0 > max_pixels`) succeeds. Execution proceeds to line 457, invoking `CGImageSourceCreateImageAtIndex`. Image I/O allocates the uncompressed image buffer in RAM *before* `CGImageGetWidth` / `Height` are checked at lines 463–464.
* **Failing Input:** A crafted HEIC image missing EXIF/container header dimension dictionary keys but containing a 50,000 x 50,000 pixel frame payload.
* **Impact:** Bypasses `MAX_PIXELS` pre-allocation safety guarantees (violating Invariant 5).

---

### 4. MEDIUM: Hardcoded Index 0 Selects Auxiliary Images or Thumbnails
* **File & Line:** `core/src/imageio.rs`, lines 411 and 457 (`CGImageSourceCopyPropertiesAtIndex(..., 0, ...)` / `CGImageSourceCreateImageAtIndex(..., 0, ...)`).
* **Mechanism:** HEIF containers can store multiple image items (primary image, gain map image, depth map, micro-thumbnail, or Live Photo frames). Index `0` is not guaranteed to be the primary image item (which is defined by the ISOBMFF `pitm` box).
* **Failing Input:** Apple Live Photos or HEIC files with embedded depth/gain map items where index `0` is an auxiliary image or thumbnail rather than the full-resolution primary picture.
* **Fix:** Use `CGImageSourceGetPrimaryImageIndex(src)` to query the true primary item index.

---

### 5. MEDIUM: Inaccurate HDR Gain Map Detection via Naive String Matching
* **File & Line:** `core/src/heif.rs`, lines 258–261 (`signals_hdr`).
* **Mechanism:** `signals_hdr` performs an un-parsed byte window scan across the raw file for literal ASCII substrings `b"hdrgainmap"` and `b"urn:iso:std:iso:ts:21496"`.
  * **False Positive:** A standard SDR image containing user comments, XMP text, or software name strings (e.g., `"Edited with hdrgainmap v1.0"`) causes `signals_hdr` to return `true`, reporting `Hdr::Dropped` when no HDR gain map exists.
  * **False Negative:** ISO 21496-1 HEIF files using uppercase URN strings or fourcc box identifiers (e.g., `albm` / `gmap`) without the exact lowercase ASCII string will fail the match, failing to report `Hdr::Dropped` (violating Invariant 4).

---

### Answers to Specific Review Questions

1. **Memory & Ownership (`imageio.rs`):**
   * Releases match allocations via `Cf<T>` (`CFRelease` on drop). Borrowed pointers from `CFDictionaryGetValue` (`w_num`, `h_num`, `o_num`) are handled correctly without extra releases.
   * `CGImageGetColorSpace` returns a borrowed `CGColorSpaceRef` owned by `image` (Get Rule). `image` remains in scope for the duration of `decode()`, so `cs` remains valid when passed to `CGBitmapContextCreate`. However, if `CGColorSpaceCopyICCData(cs)` returns `NULL`, `icc` becomes `None` while `ctx_cs` still uses `cs`, outputting un-profiled wide-gamut JPEGs.

2. **Pixel Correctness:**
   * `kCGImageAlphaNoneSkipLast | kCGBitmapByteOrderDefault` yields R,G,B,X memory layout on Apple Silicon (little-endian 32-bit host byte order).
   * `CGContextDrawImage` into a standard `CGBitmapContext` draws the image **upside down** vertically because Quartz coordinates place `(0,0)` at the bottom-left.
   * 16-bit / float-HDR images are downsampled to 8-bpc by Core Graphics during context draw.
   * Monochrome/Greyscale HEICs trigger `CGColorSpaceCreateDeviceRGB()`, converting pixels to RGB but returning `icc: None`.
   * Hardcoded index 0 breaks on multi-image containers (Live Photos, depth maps).

3. **Bomb & Size Guards:**
   * Missing property keys allow decoding to reach `CGImageSourceCreateImageAtIndex` before checking dimensions, triggering large allocations inside Image I/O.
   * `total_bytes` calculation handles overflow via `checked_mul`.

4. **In-Place Swift Safety Net:**
   * Direct in-place safety checks in `JobQueue.swift:101-104` and post-engine checks at lines 141-143 prevent JPEG bytes from overwriting `.heic` files. `converted` is correctly derived from binary format inspection (`is_heif` magic bytes) rather than file extension.

5. **HDR Reporting:**
   * `signals_hdr` suffers from both false positives (text/metadata collisions) and false negatives (uppercase URNs or non-text ISO box types).

6. **Behavioural Regressions & CLI:**
   * PNG/JPEG code paths in `lib.rs` are unaffected (`Source::open` is a pure delegation).
   * `main.rs` preserves orientation and ICC output formatting.

7. **Portability:**
   * `cargo metadata --locked` and Linux CI compilation remain intact (`imageio` and macOS decoders are properly `cfg`-gated).
