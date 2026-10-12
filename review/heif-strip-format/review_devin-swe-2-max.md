**Recommendation: C for the `Exif` item; same minimal-packet approach for any XMP you strip; retype (D) only when an item is too short to hold a valid payload.**

### 1. Reader behavior under C (valid empty-TIFF `Exif` item)

- **libheif**: never parses the Exif payload; `heif_image_handle_get_metadata` hands the blob to callers. No change vs. today — decode already confirmed.
- **Pillow/pillow-heif**: parses the blob it is handed. Empty IFD0 → empty `Exif` object, no `SyntaxError`. Fixes the observed regression.
- **Image I/O**: EXIF/GPS dictionaries empty — identical to your measurement (it tolerated the zero blob; a valid TIFF is strictly less weird).
- **ExifTool**: reads `exif_tiff_header_offset`, jumps to TIFF, sees entry_count 0. Expect silence; moderately confident — ExifTool warns on *structurally invalid* TIFF, and a count-0 IFD is valid. Verify once on a real file.
- **Android**: AndroidX `ExifInterface` reads the `Exif` item in HEIC → all attributes null; `HeifDecoder`/ImageDecoder ignore metadata for pixels. Medium confidence on old platform builds.
- **Windows WIC/Photos**: decode path doesn't touch items; property queries return nothing. Medium confidence, untested — but a present-but-empty block is the same shape as a sparse camera file.
- **Chrome/Safari**: Chrome ignores HEIC entirely; for AVIF, libavif just exposes the blob and browsers don't surface EXIF. Safari routes through Image I/O.

### 2. XMP rule

Keep a `mime`/`application/rdf+xml` item only when **every `cdsc` target is a non-primary item** AND the packet survives a property whitelist (namespaces like `HDRGainMap:*`, `semanticSegmentationMatte:*`, aux-matte). If the packet fails to parse, or any property is outside the whitelist → overwrite in place with the minimal packet below. Ordering matters: unknown → strip, since removal completeness outranks HDR. The whitelist needs a real RDF read, not a substring scan — property names in values or comments can fool one. Accept that a weird-but-benign packet occasionally dies (cost: a matte or gain-map hint), never that a personal packet survives. Keeping aux XMP is doubly important: stripping the `Exif` item already destroys the maker-note tags that apply the gain map on pre-ISO-21496 files, so this packet may be the last headroom signal.

### 3. C bytes

Exif, 18 bytes minimum:
```
00 00 00 00          exif_tiff_header_offset = 0
4D 4D 00 2A          "MM", 42
00 00 00 08          IFD0 offset
00 00                entry_count = 0
00 00 00 00          next IFD = 0
```
Remainder zero. Little-endian `49 49 2A 00 08 00 00 00` is equivalent. Item < 18 bytes → it was already malformed; retype `item_type` to `free` (the `type_at` offset the walker already computes: +16 for infe v2, +20 for v3) and zero it — nothing then attempts a parse.

XMP (~180 bytes): `<?xpacket begin="\xEF\xBB\xBF" id="W5M0MpCehiHzreSzNTczkc9"?>\n<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="squint">\n<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/>\n</x:xmpmeta>\n` then **spaces** to fill, then `\n<?xpacket end="w"?>`. Pad with `0x20` inside the packet — the XMP in-place convention — never zero-pad the tail; trailing NULs are what produced your "Invalid XMP" warnings. Below ~60 bytes write `<x:xmpmeta xmlns:x="adobe:ns:meta/"/>` + space pad; below that, retype.

### 4. What breaks under B and D

**B done right** means repacking `iinf`/`iloc`/`iref`/`ipma` in place at constant box size (parsers read exactly `entry_count` items; pad tails) and retyping orphaned `cdsc` sub-boxes to `free`. The danger is enumeration: the same item_id can appear in `iref` (any ref type, either direction), `ipma` property entries, `grpl`/`EntityToGroupBox`, and `pitm` — each a different serialization, and a missed one leaves a dangling id that `-validate` flags. Cutting bytes is worse: every `iloc` extent and base_offset past the cut, plus `idat` contents for method-1 items. Highest implementation risk for a cosmetic gain. **D** leaves an item declaring `content_type=application/rdf+xml` while typed `free`, plus a live `cdsc` — consistent enough for readers (skipping unknown item types is spec-required), but it *relies* on that universal behavior, and ExifTool may list the oddity. Correct as fallback, not as plan.

### 5. Test invariants

1. `out.len()==in.len()`; byte-diff mask equals the union of stripped extents.
2. Every non-stripped item's extents hash-identical.
3. `iinf`/`iloc`/`iref`/`ipma` bytes identical (C touches payloads only).
4. Decode gate: libheif pixels == input pixels; Pillow `getexif()` returns empty **without raising**; ExifTool clean under `-validate`; Image I/O 0 EXIF/GPS keys.
5. Completeness: no personal substring survives *anywhere*, and every `Exif`/`mime` item was handled — an item retaining its original payload is a failure (catches the idat hole in §6.1).
6. **Idempotency**: `strip(strip(x))` must not corrupt the minimal payload — skip items already in canonical form, or the second run reintroduces the Pillow error. Automatic under A, not under C.
7. Hostile corpus: overlapping extents, dref≠0, truncated boxes → `None` or a fully readable file, never half-written.

### 6. Bugs this surfaced in `strip_heif`

1. **`construction_method ≠ 0` items get an empty extent list and are silently skipped** — an Exif item stored in `idat` survives while `wiped` reports success. Detect and report at minimum; ideally zero the idat-relative range.
2. **`data_reference_index` is read and discarded.** Nonzero means external storage, but `base+offset` can still land inside this file — you'd zero the wrong bytes, possibly the picture. Skip dref≠0.
3. **`mime` matched without `content_type`** — only `application/rdf+xml` is XMP; other MIME payloads get destroyed. Parse the second NUL string after `item_name`.
4. **Overlap guard covers only `iinf`/`iloc`** — an in-bounds extent can still cover `ftyp`, `iprp`/`hvcC` (zero that and the picture won't decode), `idat`, or the `mdat` header. Stronger invariant: permit extents only within `mdat` bodies; refuse otherwise.
5. **`iinf` v1 has a 16-bit `entry_count`**; `at = if v0 {6} else {8}` skips two bytes too far — v1 files misparse.
6. **`infe` v0/v1 carry no `item_type`** — legacy Exif items are invisible, and their `item_name` strings survive in all options; worth checking on real corpora.
7. `find` only descends into the **first top-level `meta`** — sequence files keep per-track metadata under `trak` that this never sees.
