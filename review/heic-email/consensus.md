# Consensus: HEIC input for Shrink for Email — round 1 (2026-09-12)

## Lanes

| seat | route | saw | result |
|---|---|---|---|
| Gemini | agyx, worktree dir | diff + all listed files + measurements | 7,035 B in 32 s; 5 findings |
| DeepSeek V4.1 Flash | ccx `deepseek-flash` | brief with sources appended | 14,025 B; 10 findings + a ruled-out list |
| SWE-2 max | Devin CLI | file list | 6,671 B in 9 min, 0 rejections; 10 findings — the round's best read of `imageio.rs` |
| Codex (GPT-5.6) | ccx | — | dropped: `usage_limit_reached` on the ChatGPT free plan at preflight |
| Coder Terminal | producer, not a seat | — | wrote the draft; not a reviewer |

## Verified defects (probed, then fixed in `d04cc67`)

1. **Primary item.** Both code lanes: `CGImageSourceCreateImageAtIndex(0)` is not necessarily the container's primary picture. PASS (by API semantics; `CGImageSourceGetPrimaryImageIndex` exists for exactly this). Fixed: every call uses the primary index.
2. **Bomb check bypass when properties are absent.** Gemini #3, DeepSeek #6: with no `PixelWidth`/`PixelHeight` the declared size read as 0 and the check passed. PASS by reading the code. Fixed: a file that declares no size is refused before the picture is created.
3. **Wide-gamut pixels with no profile.** Gemini #2: when `CGColorSpaceCopyICCData` returns null on an RGB space, P3 values were written untagged. PASS by reading the code. Fixed: the source space is kept only when its profile came back; otherwise the picture is drawn into sRGB and left untagged.
4. **HDR detection by text needle.** Gemini #5, DeepSeek #1/#8: the `hdrgainmap` substring happens to hit Apple's auxiliary type URN and would miss an ISO-only container (its marker is a box). PASS: `grep` on the two iPhone files finds exactly one hit each, the Apple URN; the ISO URN string is absent even though ImageIO reports an ISO gain map. Fixed: `heif::signals_hdr` deleted; `imageio::decode` asks `CGImageSourceCopyAuxiliaryDataInfoAtIndex` for the ISO and Apple types and `Source.has_gain_map` carries the answer. Measured: both iPhone HEICs report `HDR GAIN MAP DROPPED`, the `sips` HEIC reports nothing.
5. **Flip test too weak.** Gemini #1 (the test half): rows 10 and 790 differ by 11 counts in green after the u8 wrap, inside the 16-count tolerance. PASS by arithmetic. Fixed: rows 10 and 700 (10 vs 188).

6. **Row stride assumed, never queried.** SWE-2 #1: with no buffer supplied, Quartz picks its own `bytesPerRow`; reading at the requested stride stays in bounds and shears the picture with `SQUINT_OK`. Every width measured so far (1200, 2048, 5712) had `w*4` divisible by 64, which is why nothing showed. PASS by reading the API contract. Fixed: `CGBitmapContextGetBytesPerRow` is queried and rows are walked at that stride; the test image is now 1201 px wide (4,804-byte rows) so a stride mistake fails the corner checks; a 1001-px `sips` HEIC is in the measurements.
7. **Truncated stream draws black with no error.** SWE-2 #4: `CGContextDrawImage` returns nothing, so a HEVC stream that dies mid-decode ships as a black `-email.jpg`. PASS, and worse than claimed: MEASURED on the real HEIC cut to 3 MB of 6, Image I/O reported the container complete, drew what it had, and the engine wrote a 10 KB `-email.jpg` with `SQUINT_OK`. SWE-2's prescribed fix (`CGImageSourceGetStatus`) was applied first and DID NOT catch it — the status stays complete when the index parses and the `mdat` is short. The check that works is our own: `heif::is_complete` walks `iloc` and refuses any extent that ends past the file. Both checks are in; the test cuts the `sips` HEIC to 60%; the real file at 50% and at 99% are both refused in the measurements.
8. **Output extension came from the input's name.** SWE-2 #5: a JPEG named `.heic` would produce `x-email.heic` holding JPEG bytes. PASS, and it is not hypothetical — Dropbox Camera Uploads on this Mac holds 16,353 files named `.heic` whose bytes are JPEG. Fixed: `Engine.Result.outputExtension` reads the output magic (`jpg`/`png`); `Preset.destination(for:outputExtension:)`; the in-place pre-check in `JobQueue` now sniffs the input bytes for `ftyp` instead of trusting the extension.
9. Smaller: unused imports on non-macOS builds (SWE-2 #9) — gated; stale `unreachable!` text (SWE-2 #10) — reworded; empty `CFData` from `CopyICCData` (SWE-2 nit) — treated as no profile.

## Refuted

- **"CGContextDrawImage draws upside down"** (Gemini #1, the defect half). REFUTED three ways: the portrait output viewed at 900 px is upright (sky up, trail down); ImageIO reads every output at orientation 1 with the expected long-edge dimensions; and the strengthened test (rows 10 vs 700) passes on the unchanged drawing code. Drawing a CGImage into a same-sized bitmap context lands it upright; the CTM flip Gemini prescribes would have inverted it.
- **Ownership / leaks in `imageio.rs`** — DeepSeek walked every Create/Copy/Get and found releases exactly once, no Get released, no early return skipping a release. Gemini agreed. Exonerated.
- **JPEG/PNG regression** — both lanes: `Source::open` is a faithful lift; measured byte-identical output on the 24 MP JPEG.
- **Swift safety net** — both lanes traced drop-on-window, Services, `.heic`-named JPEG and JPEG-named `.heic` and found no path that writes JPEG bytes over a non-JPEG file. `converted` is byte-derived; the pre-check is extension-derived; they compose.
- **Byte order constant** (DeepSeek #3) — correct on every Mac (all little-endian); the module is macOS-only. Not changed.
- **Extended-range decode** (DeepSeek #2) — the primary item of an iPhone HEIC is SDR; the range lives in the gain map, which is reported dropped. Not changed.

## Convergence

2 of 2 code lanes on: primary index, bomb-check ordering, HDR needle weakness, no ownership defects, no JPEG/PNG regression, Swift guard holds. Convergence here is evidence of reading the same code correctly, not of a shared prior — each finding was confirmed by reading the line or by measurement.

## Tensions

None left open. Gemini's flip claim and its test-weakness claim came as one finding; the test half was right and the defect half wrong, and each was probed separately.

## Judgements not adopted

- **Exempt write-beside presets from never-grow** (SWE-2 #7): at a 2048 cap the JPEG is a tenth of the HEIC; the case where it grows is a tiny HEIC, and "keeping the original" is still the honest answer. Unchanged; revisit if it ever fires in use.
- **Single-image PQ/HLG HEIF reports no HDR loss** (SWE-2 #3): iPhones write gain maps; documented in the module doc rather than parsed.
- **Extended-range decode clamps** (DeepSeek #2): the primary item is SDR; the range is in the gain map, which is reported dropped.

## Side findings (one line each, not in this PR)

- CI compiles nothing (`cargo-deny` + `cargo metadata` only), so the `cfg(not(target_os = "macos"))` branch of `Source::open` is never built anywhere. A `cargo check` job on ubuntu would close that. (DeepSeek #10, SWE-2 #9.)
- `a.heic` and `a.jpg` selected together under the Email preset both write `a-email.jpg`; last finisher wins silently. Pre-existing class (`.JPG` vs `.jpg`), now easier to trigger. (SWE-2 #6.)

## What each lane got wrong

- Gemini: the upside-down defect (see Refuted), and prescribed a CTM flip that would have broken the output.
- DeepSeek: "index 0 is the primary by spec" (its #4) — the spec's `pitm` is what `GetPrimaryImageIndex` reads, and it need not be 0; DeepSeek's own failing case contradicts its heading.
