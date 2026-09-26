# Council brief — PDF and AVIF reach the Finder menu of a macOS image optimizer

You are reviewing a diff that spans a Rust C interface, Swift application code, an Info.plist services list, and a README. Answer in your final message. Read the files listed at the end if your tooling lets you; run nothing, write nothing, edit nothing. Be specific: file and line, the concrete input that triggers a fault, and whether you confirmed it by reading or suspect it. No compliments; do not restate the diff. Under 1500 words.

## The project

`squint` is a native macOS image optimizer (GPL-3, public, Rust core + SwiftUI app). It replaces ImageOptim. It encodes to a perceptual target (SSIMULACRA2) rather than a fixed quality, it never grows a file, and it keeps the ICC colour profile in every mode. It is driven from Finder's Services menu: right-click files, choose an entry, the application does the work.

Two rules govern where output goes, and this diff touches both:

- A result written **in place** replaces the original through `Writer.replaceInPlace` (a temp file in the same directory, then `FileManager.replaceItemAt`, which keeps Finder tags and the creation date). Only a result of the **same kind of file** may do this.
- A result that is a **different kind of file** (a JPEG made from a HEIC, and now an AVIF made from anything) is written **beside** the original and the original is never touched. The engine reports this through a `converted` flag, read from the bytes, not the filename.

## What existed before this diff

- The C interface had one entry point, `squint_optimize(input, len, mode, target, fixed_quality, png_min_quality, max_dimension)`. It always asked the engine for JPEG output (`OutputFormat::Jpeg`), which means: a JPEG or PNG re-encoded as itself, a PDF rewritten as a PDF with the pictures inside it re-encoded, everything else decoded and written as JPEG with `converted` set.
- The engine's `optimize_as(bytes, format, mode, …)` in `core/src/lib.rs` already accepted `OutputFormat::{Jpeg, Avif, WebpLossless}` and was reachable only from the command line. Its rules: a PDF asked for as anything but JPEG returns `Error::ReadOnlyFormat`; asking for AVIF sets `converted_from` (to the source's name, or "JPEG"/"PNG" for those inputs); a named format is never allowed to produce a larger file than its input.
- Swift `Preset` had `mode`, `maxDimension`, and `suffix`; `suffix == nil` meant "replace in place". Five Finder entries: Shrink (fast, in place), Shrink for Email and Shrink for Social (beside, `name-email.jpg` / `name-social.jpg`), Shrink to a Quality Target (in place), Remove Location Data (strip, in place).
- PDF was handled by the engine but no Finder entry declared `com.adobe.pdf`, so a PDF never reached the application.

## What this diff adds

1. **`core/src/ffi.rs` / `core/include/squint.h`.** A second entry point `squint_optimize_as(input, len, format, mode, …)` taking `SQUINT_FORMAT_JPEG|AVIF|WEBP_LOSSLESS`. `squint_optimize` now delegates to it with JPEG. An unrecognised format code is refused with a new `SQUINT_ERR_UNKNOWN_FORMAT` rather than read as JPEG. The header's description of `converted` is widened to "a different kind of file from the input". Four tests.
2. **`app/Sources/Engine.swift`.** `Engine.Format { jpeg, avif }`; `optimize` gains a `format:` argument and calls `squint_optimize_as`. `Result.outputExtension` now reads the magic for PNG, `%PDF`, and `ftyp` (AVIF is the only ISOBMFF the engine writes), else `jpg`.
3. **`app/Sources/Preset.swift`.** `format: Engine.Format` on every preset; `writesInPlace` is `suffix == nil && format == .jpeg`; new `Preset.avif` (mode quality, no cap, no suffix, format avif). `destination(for:outputExtension:)` returns the source URL when `writesInPlace`, else `stem + (suffix ?? "") + "." + outputExtension` beside it, so an AVIF conversion of `IMG_1.jpg` is `IMG_1.avif`.
4. **`app/Sources/JobQueue.swift`.** The pre-check that refuses an ISOBMFF input for an in-place preset now keys on `writesInPlace` rather than `suffix == nil`, so a HEIC may go to the AVIF preset. The format is passed to the engine. A new `landsOnSource` compares `destination.path.lowercased() == url.path.lowercased()`; a converted result landing on the source is refused (with a specific message when the preset is AVIF, meaning the input was already an AVIF), and `landsOnSource` decides between replace-in-place and a beside write.
5. **`app/Sources/Job.swift`.** The "wrote `name`" prefix keys on `!writesInPlace`.
6. **`app/Sources/ServiceProvider.swift`.** New `convertToAvif` handler → `Preset.avif`; `pdf` added to the extension filter behind Finder's own.
7. **`app/project.yml`.** `com.adobe.pdf` added to the two in-place entries' `NSSendFileTypes` (Shrink, Shrink to a Quality Target) and nowhere else. New entry "Squint: Convert to AVIF" accepting `public.jpeg`, `public.png`, `public.heic`, `public.heif`, `org.webmproject.webp`, `public.svg-image` (not AVIF, TIFF, GIF, PDF). Version 0.8.0, build 12.
8. **README** and version files.

## What has already been verified — do not re-report these as unknowns

- `cargo check --all-targets` and 93 tests pass on macOS, including the four new FFI tests (unknown format refused; plain entry point still means JPEG; PNG→AVIF reports `converted = 1` with `ftyp` bytes; a PDF asked for as AVIF is refused with `SQUINT_ERR_READ_ONLY`).
- `xcodegen` + `xcodebuild` Release succeed; the built bundle registers six services with the expected `NSMessage` names.
- Live on macOS 27 through the real Services mechanism (`NSPerformService` with file URLs on a pasteboard): a 665 KB PDF wrapping a JPEG went to 124 KB in place through Shrink and to 165 KB through Shrink to a Quality Target, still a valid `%PDF-1.5`; a JPEG through Shrink went 662 KB → 120 KB in place (regression); Convert to AVIF wrote `a-photo.avif` 262 KB from a 662 KB JPEG, `b-screen.avif` 506 KB from a 7.8 MB PNG, `c-phone.avif` 108 KB from a 340 KB HEIC, all 2401x1601 like their sources, sources byte-identical afterwards; Convert to AVIF on an existing `.avif` left it byte-identical.
- One test-rig artefact, not a defect: with two bundles of the same bundle identifier registered, the pasteboard server resolved service names against the older one, so the new entry could not be invoked until the old registration was removed. A Sparkle update replaces the bundle in place, so a user has one.

## Questions, in priority order

1. **Can a PDF ever be written over by something that is not a PDF, or a picture by a PDF?** Trace the in-place path for a PDF input through `JobQueue.process`: the ISOBMFF pre-check, `Engine.optimize(format: .jpeg)`, `outputExtension`, `destination`, `landsOnSource`, `Writer.replaceInPlace`. Then the same for a PDF that somehow reaches the AVIF preset (Finder should not offer it; assume it arrives anyway).
2. **Is `landsOnSource` right?** It lowercases both paths. Consider: a case-sensitive volume where `Photo.AVIF` and `Photo.avif` are different files; Unicode normalisation (NFD on APFS vs NFC in a URL); a symlinked source; `url` arriving with a trailing slash or a `file://` form that differs from what `destination` builds. What does each mistake cost — a wrong refusal, or a write over the original?
3. **The `converted` flag and the guard.** With the AVIF preset, `converted` is always set, so a conversion whose destination equals the source is refused. Is there any input for which `converted` is NOT set yet the destination is a different file, or IS set yet the write goes in place? Check the FFI mapping (`converted_from.is_some()`) against the engine's rules described above.
4. **`outputExtension` from magic.** `ftyp` at offset 4 is read as AVIF. The engine writes AVIF and nothing else ISOBMFF, but the *input* can be HEIC; is there any path where input bytes rather than output bytes reach `outputExtension`, or where a JPEG/PNG result could carry `ftyp` at offset 4?
5. **Name collisions.** `photo.jpg` and `photo.png` in one folder both convert to `photo.avif`; the second run replaces the first. The Email preset has always behaved this way (`photo-email.jpg` from either). Is that acceptable for a conversion, or should the AVIF entry number its output? State the trade-off; the owner will decide.
6. **The services list.** Is any declared type one the application will then refuse (a `public.heif` image sequence, an animated WebP)? Is any type missing that the engine could convert? Does adding `com.adobe.pdf` to the two in-place entries change how Finder filters a mixed selection?
7. **Concurrency and memory.** `JobQueue.concurrencyLimit` binds the batch by memory when any job's mode is `.quality`; the AVIF preset is `.quality`. A PDF in quality mode searches each picture inside it. Is there a path where a PDF or AVIF batch runs at core-count concurrency with quality-mode memory?
8. **Error paths that fail silently** — anything that returns success having written nothing, or reports a file changed when it was not.

## Files

- `review/finder-pdf-avif/diff.patch` — the whole diff against `main` (651 lines).
- `core/src/ffi.rs` — the C interface after the change (about 300 lines with tests).
- `app/Sources/Engine.swift`, `app/Sources/Preset.swift`, `app/Sources/JobQueue.swift`, `app/Sources/ServiceProvider.swift`, `app/Sources/Writer.swift` — the Swift side after the change, each under 200 lines.
- `core/src/lib.rs` — the engine; `optimize_as` begins near line 623 and is unchanged by this diff. Read it only for question 3.
