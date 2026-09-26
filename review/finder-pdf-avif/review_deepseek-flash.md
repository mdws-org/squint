## 1. The in-place path for a PDF — traces clean

**PDF via Shrink / Shrink to a Quality Target.** `isIsobmff` is `input[4..<8] == "ftyp"` (JobQueue.swift:135 in diff.patch); a `%PDF` header never matches, so the pre-check passes. `Engine.optimize(format: .jpeg)` reaches `optimize_as`, which takes the `pdf::is_pdf` branch (lib.rs, before the `Source::open` fall-through), returns `converted_from: None` → `converted = 0`, and the bytes start `%PDF` → `outputExtension == "pdf"`. With `writesInPlace` true, `destination(for:)` returns `url` unchanged, so `landsOnSource` is trivially true, the guard is skipped, and `replaceInPlace` writes PDF over PDF. Confirmed by reading. The engine refuses a PDF result that did not shrink (`NoSmallerResult`), so the write is never a no-op.

**PDF forced into the AVIF preset.** No new path exists. `Preset.avif.format == .avif`, so `Engine.optimize_as` hits the `format != OutputFormat::Jpeg` check inside the `pdf::is_pdf` branch and returns `Error::ReadOnlyFormat { format: "PDF" }` → `SQUINT_ERR_READ_ONLY` → thrown before `destination` is even computed → `.failed`, nothing written. The new FFI test asserts exactly this. The order matters and is correct: the PDF branch runs before the `png`/decode branches, so no route turns a PDF into a picture.

**Picture written over a PDF:** impossible in the other direction too — a picture result's `outputExtension` is `jpg`/`png`/`avif`/`pdf` and `destination` for an in-place preset is the source itself, but a PDF source can only carry `pdf` bytes back. Confirmed.

## 2. `landsOnSource` — the errors all go one way

The comparison can only ever turn a *false* into a *true*, never the reverse, and a `true` does only two things: fire the `converted` guard (refuse) or select `replaceInPlace` instead of `write(to:)`. For a non-converted result, `converted == 0` means the input was a JPEG, PNG or PDF asked for as JPEG, which means `writesInPlace` is true, which means `destination` is *literally* `url` — so the loosened write branch is unreachable with a divergent path. **There is no case where the case-folding causes a write over the original; only wrong refusals.** That is the important asymmetry, and it holds.

- **NFC/NFD:** no divergence. Both sides derive from the same `URL` string (`destination` only appends components to it), so the same normalisation survives; `deletingPathExtension().lastPathComponent` round-trips the characters.
- **Trailing slash / `file://` form:** no divergence, same reason. Comparing `.path` is *more* robust than the old whole-`URL` comparison.
- **Symlink:** both sides are the path as given, never resolved, so they cannot diverge. (`replaceItemAt` on a symlink is a separate, pre-existing question.)
- **Case-sensitive volume, `Photo.AVIF` + `Photo.avif`:** `destination` for `Photo.AVIF` is `Photo.avif`, `landsOnSource` true, `converted` true → refused with "this picture is already an AVIF". Wrong refusal, right outcome — the message is also false if `Photo.AVIF` is really a PNG. **Cost: a user who wants that conversion cannot have it.** No data risk.
- **Collision between two sources:** `photo.jpg` and `photo.JPG` both convert to one `photo.avif`; the second replaces the first's output. Cost: a few hundred KB of derived file.

## 3. `converted` and the guard — sound, with one non-obvious gap

Mapping `converted_from.is_some() → converted = 1` (ffi.rs), against `converted_from = src.converted_from.or(if format != Jpeg { Some("PNG"/"JPEG") } else { None })`:

- `converted == 0 && destination != url`: only reachable if a preset has `suffix != nil || format == .avif` **and** `converted == 0`. Both such presets are `.jpeg`-format; a `.jpeg` result over a JPEG/PNG input gives `converted == 0` and writes to `name-email.jpg` — a genuinely different path. This is pre-existing and correct (and can overwrite an earlier copy).
- `converted == 1 && landsOnSource`: always refused. Correct.
- The gap: `converted == 0 && destination == url` for a source whose **bytes are a JPEG but whose name is not** — a Dropbox camera-upload `IMG_1.heic` holding JPEG. Plain preset writes JPEG bytes into `photo.heic`, in place. That is deliberate per JobQueue's own comment, pre-existing, and unchanged by this diff. The AVIF entry inherits it correctly (`photo.avif` from a real JPEG named `.heic` gets AVIF bytes at `photo.avif`).
- SVG: `src.converted_from` is set for a rasterized SVG, so `converted == 1` and the guard fires — but the preset is email/social/avif, all of which write beside, so the guard never fires for SVG in practice. Sound.

## 4. `outputExtension` from magic — sound

`outputExtension` is a computed property on `Result.data`, the engine's own output; input bytes never reach it. Ordering is `png → %PDF → ftyp → jpg`. A JPEG result cannot carry `ftyp` at offset 4 (it starts `FF D8 FF`), and a PNG result is caught by the first test. The `%PDF` test precedes `ftyp` and a PDF is longer than 12 bytes, so no shadowing. Confirmed.

## 5. Name collisions — keep the overwrite, do not number

Numbering breaks the property that matters more: running the entry a second time on the same file must replace the previous result, not accrete `photo-2.avif`, `photo-3.avif`. The collision only bites when two *different* stems collapse to one, and for the common case (`photo.jpg` + `photo.png`) they are the same picture in two containers, which is the `-email` behaviour you already chose deliberately. The AVIF entry widens the exposure only for a same-stem pair where one is an `.svg` or a `.webp` — genuinely different pictures. Trade-off: numbering protects that rare pair at the cost of idempotence on every re-run. Keep the overwrite.

## 6. Services list

- **Declared then refused:** `public.heif` and animated `org.webmproject.webp` are both offered and then refused by name (`is_image_sequence`, `webp::is_animated` → `ReadOnlyFormat`). This is the "shows up and refuses" mode the comment calls out; the email/social entries already carried the same exposure, so this is not new, but it is declared-and-refused either way.
- **Missing:** nothing I can find. Excluding `public.avif`, `public.tiff`, `public.gif` and the PDF is correct in each case.
- **Mixed selections:** adding `com.adobe.pdf` to Shrink and Quality means a photo-plus-PDF selection now *shows* an entry where the PDF previously suppressed it — and the PDF is rewritten, in place, while the user was thinking about the photo. Coherent, but newly possible.
- **Filter/declaration drift:** `isSupported` now includes `pdf` while `stripMetadata`'s `NSSendFileTypes` does not, so the filter is willing to strip a PDF that Finder never offers the entry for. Harmless today because Finder's filter is the stricter one; it is exactly the drift the comment warns must not exist.

## 7. Concurrency — fast-mode PDF rewrites run at core count

`concurrencyLimit` returns `cores` unless some job has `mode == .quality` (JobQueue `concurrencyLimit(for:)`). A PDF through **Shrink** is `.fast`, so a batch of PDFs on a 16 GB / 8 core machine runs **eight concurrent document rewrites**, each holding every decoded picture in a document at once. The figures the comment rests on — "fast peaks at 167 MB per job", and the 2.84 GB comparison peak — were measured for single images, not for a multi-page document. With `MEMORY_SHARE` 0.75 and `bytesPerQualityJob` 3 GB, quality mode is bound correctly (that yields 2 on 8 GB, 4 on 16 GB, 8 on 32 GB), and PDF via **Shrink to a Quality Target** is `.quality`, so it is bound. The exposure is specifically PDF-via-Shrink, and it is a plausible memory regression, not a measured one. Worth a measurement before release.

## 8. Silent failure paths

The concrete one: **a conversion that writes nothing reports `alreadyOptimal`.** `optimize_as` sets `size_to_beat = Some(bytes.len())` whenever `format != OutputFormat::Jpeg`, so an AVIF that comes out larger than its source returns `NoSmallerResult`, which `Engine.Failure.isAlreadyOptimal` maps to the "already optimal" outcome. For a *conversion* that message is wrong — the question was never whether a smaller file was possible, it was whether an AVIF came out, and none did. This is not a rare edge: `public.svg-image` is declared on the entry, and a raster of a typical drawing is far larger than the drawing, so **every SVG through Convert to AVIF lands here** — entry offered, nothing written, "already optimal" reported. The same silently happens to an already-well-optimized PNG, which the live run's `b-screen.avif` (506 KB from 7.8 MB) shows is not universal but is plainly reachable. (Reasoned from `size_to_beat` and the declared types; not measured.) The conversion path needs its own message, distinct from `alreadyOptimal`, whenever `format != .jpeg` and nothing was written.

Minor, same class: the `.avif` refusal message says "already an AVIF" when the guard fires, but the guard fires on *destination == source*, not on content — a PNG named `photo.avif` gets that message.

*One line outside the reviewed surface: the README's new "there is no Intel build" is a public claim I cannot verify from this diff.*
