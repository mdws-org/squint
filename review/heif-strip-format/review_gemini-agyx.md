### 1. Recommendation for the `Exif` Item

**Recommend Option C (Minimal valid payload in place).**

Zero-filling (Option A) leaves an `Exif` item whose payload violates the format declared by `infe`. Pillow’s `SyntaxError` occurs because Pillow expects a TIFF header (`II*\0` or `MM\0*`) at the offset specified by `exif_tiff_header_offset`; encountering `\0\0` crashes `getexif()`. Option C preserves box boundaries, file length, and `iloc` offsets while guaranteeing syntactic validity.

Reader behaviors under Option C:
- **Pillow / pillow-heif**: `getexif()` reads the 4-byte offset, parses the 14-byte empty TIFF header/IFD0, and returns an empty `{}` cleanly without raising `SyntaxError`.
- **Apple Image I/O (`CGImageSourceCopyPropertiesAtIndex`)**: Parses the empty IFD0, finds 0 entries, and omits the `{Exif}` and `{GPS}` dictionaries. Image decoding, color space, and orientation are unaffected.
- **ExifTool**: Parses TIFF IFD0, finds 0 tags, and exits cleanly with 0 tags and zero warnings under `-validate`.
- **libheif**: Does not parse TIFF internally during decode; exposes raw blocks via `heif_image_handle_get_metadata_data()`. Callers parsing EXIF receive a valid, empty TIFF structure.
- **Android (`HeifDecoder` / `ExifInterface`)**: Decodes HEVC tiles via Stagefright/libheif. `ExifInterface` parses the valid TIFF header, finds 0 tags, and returns empty attributes without error.
- **Windows WIC / Photos**: WIC's TIFF handler parses IFD0 cleanly and returns `WINCODEC_ERR_PROPERTYNOTFOUND` on query. Image renders normally.
- **Chrome / Safari**: Render pipelines rely on item properties (`ispe`, `colr`, `irot`) rather than Exif. Both render without warning.

---

### 2. XMP Items & Auxiliary Image Protection

**Do not destroy auxiliary image `mime` items.**

Gain maps and portrait segmentations require their XMP packets (`HDRGainMap:*`, `SemanticSegmentationMatteVersion`) to define headroom, gamma, and versioning. These auxiliary packets contain mathematical rendering parameters and **zero PII**. Destroying them breaks HDR tone-mapping in Chrome, Android, and older iOS devices, causing blown-out or flat SDR fallbacks.

**Proposed Rule:**
1. Check `iref` for `cdsc` (content describes) references from the `mime` item.
2. If the `mime` item describes an auxiliary item (`to_item_id != pitm`, where the target item has an `auxl` reference or `auxC` property): **Preserve it untouched**.
3. If the `mime` item describes the primary image (`to_item_id == pitm`), has no `cdsc` reference, or `iref` is absent: **Sanitize using Option C** (in-place minimal valid empty XMP packet padded with whitespace). Never zero-fill, which triggers ExifTool `Warning: Invalid XMP`.

---

### 3. Exact Bytes for Option C

#### Exif Item
Per ISO/IEC 23008-12 §A.2.1, the payload begins with a 4-byte `exif_tiff_header_offset` followed by a TIFF header (TIFF 6.0):
- `00 00 00 00` (4-byte offset = 0; TIFF header begins immediately)
- `49 49 2a 00` (Little-endian "II", magic 42)
- `08 00 00 00` (Offset to IFD0 = 8 bytes relative to TIFF start)
- `00 00` (IFD0 entry count = 0)
- `00 00 00 00` (Next IFD offset = 0)

**Exact hex (18 bytes):**
`00 00 00 00 49 49 2a 00 08 00 00 00 00 00 00 00 00 00`
- **Minimum length**: 18 bytes.
- Pad any remaining bytes in the item extent with `0x00`. Parsers terminate at IFD0.

#### XMP Item
Per ISO 16684-1 / Adobe XMP Part 3:
- Header (89 bytes):
  ```xml
  <?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>\n<x:xmpmeta xmlns:x="adobe:ns:meta/"/>\n
  ```
- Trailer (19 bytes):
  ```xml
  <?xpacket end="w"?>
  ```
- **Minimum length**: 108 bytes.
- Pad the space between byte 89 and `extent_len - 19` with ASCII spaces (`0x20`, `' '`).

#### Fallback if Shorter than Minimum
If an extent is < 18 bytes (Exif) or < 108 bytes (XMP), it is already corrupt. Fall back to **Option D**: overwrite the 4-byte `item_type` in `infe` with `b"void"` and zero the payload.

---

### 4. What Breaks in Option B and Option D

#### Option B (Dropping Items)
- **Offset cascade**: Removing `infe` and `iloc` entries shrinks `iinf`, `iloc`, and `meta`. Every subsequent absolute offset in `iloc` across the entire file must be recomputed and rewritten.
- **`iref` dangling references**: Deleting an item referenced in `iref` (or whose `from_item_id` was deleted) creates orphan references. Libheif and strict validators reject unresolved item IDs.
- **`ipma` / `grpl`**: Leaves orphaned associations in `ipma` and broken groupings in `grpl`.
- **`idat` items**: Construction method 1 items cannot be removed without rebuilding `idat`.

#### Option D (Retyping in Place)
- **`iref` validation**: Retyping an item to `void` leaves an unknown item referencing image tiles via `cdsc`. ExifTool `-validate` flags this as invalid.
- **`ipma` pollution**: Properties (e.g. `colr`) remain bound to an unknown item type.

---

### 5. Correctness Invariants for Testing

1. **Byte-level isolation**: Total file length unchanged (`out.len() == orig.len()`). All bytes outside target metadata extents remain strictly bit-for-bit identical.
2. **Exif syntax**: Extent begins with `00 00 00 00 49 49 2a 00`; Pillow `getexif()` returns `{}` with no `SyntaxError`.
3. **ExifTool cleanliness**: ExifTool reports 0 GPS, Model, Make, or DateTimeOriginal tags, and `-validate` emits 0 warnings.
4. **XMP parser validity**: Retained auxiliary XMP parses as valid XML. Cleared primary XMP passes XML parsers without malformed entity errors.
5. **Decoder integrity**: Apple Image I/O and libheif decode full pixel dimensions, preserving Display P3 ICC profile, orientation, and HDR gain map headroom.

---

### 6. Flaws Identified in `strip_heif`

1. **Blind `mime` matching (`heif.rs:347`)**: Matches any `mime` item without checking `content_type` in `infe` (which may be SVG, application JSON depth maps, or neural overlays).
2. **Collateral damage to rendering**: Destroys auxiliary gain-map and portrait-matte XMP without inspecting `iref`.
3. **Silent skip on `idat` items (`heif.rs:287`)**: Items with `construction_method == 1` (`idat` box) are ignored (`absolute == false`). If Exif data resides in `idat`, `strip_heif` leaves it completely intact without warning.
4. **`infe` v0/v1 blindness (`heif.rs:210`)**: Skips version 0 and 1 `infe` boxes where MIME types are stored as strings rather than FourCCs.
5. **Orientation stripping**: If orientation is stored only in EXIF (common in non-Apple HEIFs) and not in an `irot` item property in `ipma`, clearing EXIF causes readers to lose image orientation.
