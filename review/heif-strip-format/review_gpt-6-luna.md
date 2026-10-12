## 1. Recommendation

Use **C for the Exif item**, subject to validating that its extents form a writable logical item without aliasing image data. Keep the item declared as `Exif`; replace its payload with a valid empty Exif item, not zero bytes. This preserves the picture and avoids leaving readers a TIFF parser error. It is the best fit for the requirement that output remain readable, but no single empty-payload form is guaranteed across every implementation.

Expected behavior, with confidence limits:

| Reader | Expected result for the empty Exif item |
|---|---|
| libheif | Image decoding should be unaffected; it normally treats Exif as item metadata, not image data. High confidence for decoding; lower for every metadata-extraction API/version. |
| Apple Image I/O | Should decode and report no Exif/GPS properties. The observed zero-payload file already decodes, but that does not prove identical behavior for the replacement TIFF. Moderate confidence. |
| ExifTool | Should parse the empty IFD and report no Exif tags, without the current malformed-XMP warning if XMP is also handled correctly. Test the exact output with `-validate`; do not infer this from the observed zero-fill case. |
| Pillow / pillow-heif | The reported `getexif()` exception is consistent with parsing zero bytes as TIFF. A valid empty TIFF should avoid that particular error, but plugin behavior depends on how it unwraps the HEIF Exif offset. Verify the exact output. |
| Android HEIF decoder | Image decoding should not depend on the Exif payload. I cannot confirm how its metadata APIs handle an empty Exif item across Android versions. |
| Windows WIC / Photos | I cannot confirm whether each version ignores, warns on, or exposes malformed/empty Exif items. Validate on supported Windows versions. |
| Chrome / Safari | Safari’s HEIF path is expected to use Apple decoding and preserve image display. Chrome HEIF support is platform/build-dependent; do not promise support based on the container edit alone. |

D is not safer: inventing an unknown `item_type` relies on readers ignoring it, and leaves a declared item with semantics Squint does not control. A is observably bad for Pillow and produces invalid metadata payloads. B is structurally expensive and offers no readability advantage over C.

## 2. XMP policy

Do **not** destroy an XMP item merely because it is XMP. The described packets contain auxiliary-image semantics, not personal data; removing the gain-map or segmentation-matte XMP can impair rendering or auxiliary-image interpretation. The JPEG gain-map observation is a separate warning that metadata outside the map may affect HDR behavior.

Use a content-aware rule:

- Identify XMP by both `item_type == mime` and its `content_type` field, such as `application/rdf+xml`. Do not treat every `mime` item as XMP.
- Parse XMP and remove personal properties; preserve allowlisted technical properties needed by auxiliary images.
- Preserve the two reported auxiliary packets unchanged if inspection confirms they contain only those version/headroom properties.
- If parsing fails or an unrecognized packet cannot be classified, do not silently claim complete removal. Fail the Strip operation, or apply an explicit conservative policy that removes the packet and accepts the rendering risk.

Removing all XMP is simple but unnecessarily risks HDR and portrait rendering. Preserving all XMP is not a complete privacy policy.

## 3. Exact in-place payloads for C

For Exif, use this **18-byte** payload:

```text
00 00 00 00  4D 4D 00 2A 00 00 00 08  00 00  00 00 00 00
```

The first four bytes give a TIFF-header offset of zero from the byte after the offset field. The TIFF is big-endian: header, IFD0 at offset 8, zero entries, and zero next-IFD offset. The minimum length for this payload is 18 bytes. Fill any remaining item bytes with zero; they are outside the referenced TIFF structure. If the item is shorter, do not write a truncated structure: fail the operation or use a separately validated fallback. Do not return a successful Strip result that leaves personal data behind.

For an XMP item that policy determines must be cleared, one concrete empty packet is the following UTF-8 byte sequence, with no line breaks:

```text
<?xpacket begin="\uFEFF" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about=""/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>
```

Encode `\uFEFF` as the actual UTF-8 bytes `EF BB BF`, not six literal characters. This exact packet is 207 bytes; pad to the item’s existing length with ASCII spaces **before** the closing `<?xpacket end="w"?>` processing instruction. This is a minimum for this chosen packet, not a universal XMP minimum. If shorter, do not truncate it; fail or use another tested valid packet. Keep the `mime` item’s content type intact.

## 4. Structural hazards in B and D

Dropping an item requires consistent edits to `iinf`’s entry count and `infe` entries, `iloc`, and relevant `iref` edges. `cdsc` references from metadata to described images must be removed or updated. Check `ipma` associations and `grpl` memberships by item ID rather than assuming they cannot mention the metadata item. Cutting bytes also requires updating every affected `iloc` offset and box size; leaving dead space avoids those shifts but still requires zeroing sensitive bytes.

`iloc` construction method 1 locates data in `idat`, not at an absolute file offset. Multiple extents must be treated as one logical item when creating a replacement payload. Shared or overlapping extents must not be overwritten if another item—including a coded image item—uses the same bytes. Retyping does not resolve those relationships.

## 5. Test invariants

For every successful Strip:

- The file length, all non-target item payloads, and coded image data are byte-identical.
- Every targeted Exif item parses as empty; no GPS, camera, or date values remain in Exif or XMP.
- Every retained XMP packet is well-formed and retains required auxiliary-image properties.
- `iinf`, `iloc`, `iref`, item associations, and extents remain structurally valid; no target extent overlaps protected image data or box structures.
- Independent decoders can decode the full image, and metadata APIs do not raise parse errors. Run this against the supported libheif, Apple, ExifTool, Pillow/pillow-heif, Android, and Windows paths; unsupported browser/container combinations should be recorded as unsupported, not assumed.
- Failure produces no output file. The in-memory function’s `None` is not enough to guarantee that: the caller must write to a temporary file and atomically replace the destination only after all validation and writes succeed.

## 6. Issues surfaced in `strip_heif`

The code currently wipes **every** `mime` item, without checking its `content_type`; that can destroy non-XMP data or rendering-critical XMP. Restrict targeting to recognized Exif and XMP formats.

Also, `item_extents` returns only absolute-offset extents, so a matching item located by `construction_method == 1` can be silently skipped while Strip reports success. Before mutation, validate every targeted item’s complete extent set, including method, length, and bounds. Check overlap against all protected item extents, not only `iinf` and `iloc`; otherwise a malicious or unusual shared extent can corrupt the picture. Finally, `end = start + length` in `strip_heif` is unchecked, and the `u64`-to-`usize` conversions in `item_extents` need checked range conversion on 32-bit targets.
