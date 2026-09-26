# Probes — finder-pdf-avif round

Each claim from a lane that describes a mechanism was reproduced or refuted before it reached the owner. Run on svk (Apple silicon, macOS 27) against the 0.8.0 build at `~/squint-080`.

## PASS — an SVG through Convert to AVIF writes nothing and is reported as "already optimal" (DeepSeek, question 8)

Mechanism claimed: `optimize_as` sets `size_to_beat = Some(bytes.len())` for any named format, so a raster of a drawing, which is always larger than the drawing, is refused with `NoSmallerResult`; the application maps that error to the "already optimal" outcome.

Reproduction, a 233-byte SVG (a rectangle, a circle, one word) through the CLI, which takes the same engine path:

```
squint probe.svg --format avif --mode quality --target 80 --out probe.avif
probe.svg  1200x800 = 0.96 MP  0 KB  converted from SVG
target is only reachable at 4631 bytes, larger than the original 233; keeping the original
probe.avif: not written
```

Reproduced. The entry declared `public.svg-image`, so every SVG offered would have ended here. Fixed in two places: `public.svg-image` leaves the Convert to AVIF entry (the rasterised drawing is never smaller than the drawing), and `JobQueue` reports a refused conversion as "an AVIF of this picture would be larger than the original, so none was written" instead of "already optimal", since a well-optimised PNG or HEIC can still land there.

## REFUTED — a PDF through Shrink holds every decoded picture at once, at core-count concurrency (DeepSeek, question 7)

Mechanism claimed: fast mode runs at core-count concurrency and a multi-image document decodes all its pictures, so a batch of PDFs could exceed the 167 MB-per-fast-job figure the concurrency limit rests on.

Reading `core/src/pdf.rs` `rewrite`: the candidates are iterated one at a time (line 292), each decoded (line 311), encoded, and its stream replaced before the next is decoded. Nothing holds two decoded pictures at once, so a document's peak is its largest single picture plus the document bytes, the same shape as a single JPEG in fast mode.

Measurement, the 665 KB test PDF (one 2401x1601 photograph) in fast mode:

```
/usr/bin/time -l squint p2.pdf --mode fast --out p2-out.pdf
p2.pdf  1 pages  650 KB -> 121 KB  18.6%  images re-encoded in place  0.341s
maximum resident set size  61259776
peak memory footprint      55722512
```

61 MB resident. Refuted; no change.

## REFUTED — Unicode normalisation could make `landsOnSource` miss a real alias (GPT-6, question 2, marked "suspect")

`destination` is built from the same `URL` by `deletingLastPathComponent().appendingPathComponent(stem + suffix).appendingPathExtension(ext)`; the stem is the source's own `lastPathComponent` minus its extension, so both sides carry identical bytes for every component but the extension. No normalisation is introduced on either side. DeepSeek reached the same conclusion by reading; recorded so the next round does not re-suspect it.

## REFUTED — Strip could write a byte-identical file and report it done (GPT-6, question 8, marked "unverified")

`optimize_as` returns `Error::NoSmallerResult` from every strip path when nothing was removed (`wiped == 0` for HEIF, TIFF, GIF, WebP; `metadata_removed == 0` for a PDF; `stripped.len() >= bytes.len()` for JPEG and PNG), and the application maps that to "nothing to remove". Pre-existing behaviour, unchanged by this diff.

## ACCEPTED as designed — a wrong refusal for `Photo.AVIF` on a case-sensitive volume (Gemini, GPT-6, DeepSeek, question 2)

All three lanes agree the case-folded comparison can only turn a miss into a refusal, never a refusal into a write over the source. On a case-sensitive volume `Photo.AVIF` and `Photo.avif` are distinct files and the conversion is refused where it could have been written. The cost is a conversion the user cannot have; the alternative cost is writing the AVIF over the source on the far more common case-insensitive volume. Kept. The refusal message no longer claims the source "is already an AVIF", since the guard fires on where the result would land, not on what the source is.

## REFUTED — `public.heif` re-admits AVIF files to the Convert to AVIF entry (SWE-2, question 6, marked "suspected")

Measured on macOS 27 with `UTType.conforms(to:)`: `public.avif` → `public.heif` is **false**, `public.heic` → `public.heif` is false, and both conform to `public.heif-standard`. This is the measurement already recorded in `project.yml`. The live run in which the entry ran on an existing `.avif` went through `NSPerformService`, which does not filter file URLs by declared type the way Finder does, so it says nothing about what Finder offers.

## REFUTED — `Job.mode` might be a stored field defaulting to fast (SWE-2, question 7; `Job.swift` was not in that lane's file list)

`Job.mode` is `preset.mode` (`Job.swift:24`), so an AVIF batch is bound by memory like any quality batch.

## REFUTED — a conversion of an animated WebP or a HEIF sequence silently keeps one frame (SWE-2, question 6)

`optimize_as` refuses both with `Error::ReadOnlyFormat` before any decoder runs (`lib.rs`, the `webp::is_animated` and `heif::is_image_sequence` checks). The entry is offered and then refuses, which the Email and Social entries have always done for the same files; nothing is written.

## FIXED — an unknown mode code still collapsed to fast while an unknown format is refused (SWE-2, question 3)

`squint_optimize_as` now refuses any mode outside the three named ones with `SQUINT_ERR_UNKNOWN_MODE`, with a test. Pre-existing, and the same defect class the format arm had just closed.

## FIXED — the shared extension filter admitted `pdf` to every entry (SWE-2 and DeepSeek, question 6)

`ServiceProvider.isSupported` takes the preset and admits a PDF only when the preset writes in place, so a PDF reaching the Email or Social preset by a route other than Finder is turned away rather than rewritten as `name-email.pdf`. The stale "No JPEG or PNG images were selected" message was replaced at the same time.

## FIXED — declaration drift: the extension filter accepted `pdf` for Remove Location Data while Finder never offered it (DeepSeek, question 6)

`ServiceProvider.isSupported` gained `pdf` for the two shrinking entries, which made the shared filter willing to strip a PDF the Strip entry did not declare. The engine already strips a PDF (info dictionary and XMP packet), so `com.adobe.pdf` was added to the Remove Location Data entry rather than the filter narrowed. This widens the change beyond the brief's two entries; recorded for the owner.
