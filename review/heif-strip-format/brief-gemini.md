# Review: Squint HEIC metadata removal — what to leave where the metadata was

You are reviewing how an image optimizer removes metadata from HEIC/AVIF (ISOBMFF/HEIF) files without re-encoding. Answer as a senior engineer who knows ISO/IEC 23008-12 (HEIF), ISO 14496-12 (ISOBMFF), EXIF/TIFF and XMP, and how real readers parse them (libheif, Apple Image I/O, ExifTool, Pillow/pillow-heif, Android, Windows WIC, browsers). Be concrete; cite the box/field and the reader behaviour you rely on, and say when you are not sure how a reader behaves rather than guessing. No compliments, do not restate this brief. Under 900 words.

Read files if listed; run nothing; write nothing; answer in your final message. Do not edit any files.

## The program

Squint (open source, GPL-3, macOS app + Rust engine) has a "Remove Location Data" mode (Strip) that must remove location, camera and date metadata and leave the picture byte-for-byte untouched. For HEIC/AVIF it finds every item in `iinf` whose `infe` item_type is `Exif` or `mime`, finds that item's extents in `iloc` (construction_method 0 only), and overwrites those bytes with zeros in place. Nothing moves: `iloc` offsets are absolute, and the file keeps its length. The function is `strip_heif` in `core/src/heif.rs` (attached). The reason given in that file for overwriting rather than removing: cutting bytes out would move everything after them and every absolute offset would have to be rewritten.

## What was observed

1. Linux test run (2026-10-11): after Strip, Pillow 12 `getexif()` on the result raises `SyntaxError: not a TIFF file (header b'\x00\x00...')`. libheif decodes the picture normally.
2. Measured today on a real iPhone 17 Pro Max HEIC (5712x4284, Display P3, orientation 6, ISO + Apple HDR gain map), before and after Strip:
   - Apple Image I/O (`CGImageSourceCopyPropertiesAtIndex`): decodes at full size, orientation 6 kept, ICC kept, EXIF dictionary 0 keys (was 32), GPS 0 keys (was 15), Model gone. HDR headroom 3.50 before and after; ISO and Apple gain maps still found (2856x2142).
   - ExifTool 13: GPS, Model, Orientation tags gone, no complaint about the Exif item, but `Warning: Invalid XMP [x2]` (also under `-validate`).
3. The file's metadata items, from `iinf` and `iref`:
   - item 120 `Exif`, `cdsc` -> items 46 and 118 (the primary grid and its HDR derivation). The real location/camera/date data.
   - item 64 `mime` `application/rdf+xml`, `cdsc` -> item 63. Its XMP holds only `semanticSegmentationMatte:SemanticSegmentationMatteVersion`.
   - item 116 `mime` `application/rdf+xml`, `cdsc` -> item 62. Its XMP holds only `HDRGainMap:HDRGainMapVersion` and `HDRGainMap:HDRGainMapHeadroom`.
   So both XMP items describe auxiliary images (a segmentation matte and the gain map), carry no personal data, and are destroyed anyway. On the JPEG path the project already learned that Apple's gain map is applied by values outside the map itself (two maker-note tags; without them headroom fell 4.00 -> 3.48), so destroying the gain map's own XMP may cost HDR on older iPhones whose files carry no ISO 21496-1 map.
4. The owner's constraint: a wrong choice that makes Squint's output unreadable to other programs is the worst outcome. Removing GPS/camera/date must stay complete; leaving the picture untouched must stay true; the file must never be written with half-done structure.

## The options on the table

- **A. Zero-fill (current).** Simple, nothing moves. Leaves an `Exif` item whose payload is not a TIFF and `mime` items whose payload is not XML.
- **B. Drop the items.** Remove their `infe` entries (fix `iinf` entry_count), their `iloc` entries, their `iref` references, and either cut their bytes (rewrite every absolute offset in `iloc`, and any box sizes up the tree) or leave the bytes as unreferenced dead space and zero them.
- **C. Minimal valid payload in place.** Keep the item and its length; write a valid empty structure and pad: for `Exif`, the 4-byte exif_tiff_header_offset then a TIFF header with an empty IFD0 (e.g. `MM\0*` + offset 8 + entry count 0 + next-IFD 0), rest zero; for XMP, a valid empty `<?xpacket ...?>` packet padded with whitespace up to the item length, as XMP's own in-place padding allows.
- **D. Retype in place.** Overwrite the 4-byte `item_type` in each `infe` (same length) with a type readers ignore, so the entry no longer claims to be Exif/XMP, and zero the payload.
- Any option you think is better.

## Questions

1. **Recommend one option** for the `Exif` item, and say what each major reader (libheif, Image I/O, ExifTool, Pillow/pillow-heif, Android's HEIF decoder, Windows WIC/Photos, Chrome/Safari) does with the result. Where a reader would reject or warn, say so and how sure you are.
2. **XMP items.** Should a `mime` item be destroyed at all when its packet carries no personal data (the gain-map and segmentation-matte packets above)? Propose a rule: e.g. keep XMP attached via `cdsc` to auxiliary images, or parse each packet and remove only personal properties, or remove all XMP. Weigh completeness of removal against breaking HDR and portrait rendering.
3. **For C:** exact bytes you would write for the Exif item and an XMP item, the minimum item length each needs, and what to do when an item is shorter than that minimum.
4. **For B or D:** what breaks. `iref` references to a removed item, `ipma` property associations, `grpl` groups, `idat`-based items (construction_method 1), multiple extents, items shared by reference.
5. **Correctness invariants** a test should assert for your choice, so that a later change cannot silently regress readability.
6. Anything wrong in `strip_heif` itself that this question surfaced (e.g. `mime` items that are not XMP, like other content types).

## Attached from our side

- `core/src/heif.rs` — the HEIF/AVIF walker; `strip_heif`, `item_types`, `item_extents` are the unit under review. The rest of the engine is not under review.

Read this file with your file tool: /Users/bemeadows/Projects/.lanes/squint-heif-strip/core/src/heif.rs
