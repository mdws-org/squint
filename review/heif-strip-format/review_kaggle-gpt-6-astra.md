## Recommendation: B, retaining and scrubbing the old storage

Remove the Exif item structurally; zero its former payload. Rebuild `meta` within its original footprint, using valid `free` boxes for slack. Do not move `mdat` or image extents. Scrub discarded declarations too. If that cannot be done safely, **refuse Strip without publishing output**.

This avoids both malformed metadata and dependence on readers accepting “empty EXIF.” Removing metadata does not require compacting the file.

Expected reader behaviour for **B**, assuming valid remaining structure:

| Reader | Expectation and confidence |
|---|---|
| libheif | Decode normally; removed item absent from metadata enumeration. High confidence: image decoding does not require an Exif item. |
| Apple Image I/O | Decode without that EXIF/GPS metadata. High confidence generally; orientation/HDR equivalence needs testing. Your observations establish this only for A on that file. |
| ExifTool | No tags from the removed item; no malformed-Exif warning attributable to it. High confidence. Existing zeroed XMP would still warn. |
| Pillow/pillow-heif | No removed TIFF payload reaches `getexif()`; expect an empty result unless other EXIF exists. High confidence in avoiding the reported exception; verify plugin versions. |
| Android | Expect normal decoding without optional EXIF. Exact framework/OEM metadata and orientation behaviour unverified. |
| Windows WIC/Photos | Expect normal decoding where the codec is installed; exact codec/version behaviour unverified. |
| Chrome/Safari | Expect normal rendering for supported formats; HEIC support is not universal. Exact metadata/HDR paths unverified. |

HEIF image properties—`irot`, `imir`, `colr` through `ipma`—are separate from TIFF Orientation and ICC-related metadata. Preserve them. **If orientation or HDR depends solely on discarded EXIF/MakerNotes, byte-identical coded pixels do not guarantee identical rendering.** Refuse unless that dependency is safely preserved elsewhere.

## XMP policy

Do **not** destroy every `mime` item. ISO 14496-12 `infe.content_type` identifies its format; `mime` does not mean XMP.

Keep the two described auxiliary packets if parsing confirms they contain only approved rendering properties. A `cdsc` association to an auxiliary image is **not** a privacy guarantee.

Use namespace-aware, entity-disabled XML parsing and a strict rendering-property allowlist, with validated value types. Reconstruct packets containing approved properties; discard everything else, including identifiers, comments and unknown extensions. A blacklist cannot promise complete removal. Malformed or unsupported metadata must cause refusal, not silent retention.

Deleting all XMP risks older Apple HDR and portrait processing. Conversely, retaining arbitrary auxiliary XMP risks privacy leakage. Unknown rendering dependencies require refusal rather than guessing. Audit other metadata carriers before promising file-wide GPS/camera/date removal.

## C: precise payloads, with caveats

Exif, **18 bytes**, followed by zeros:

```text
00 00 00 00  4D 4D 00 2A  00 00 00 08  00 00  00 00 00 00
```

The HEIF `exif_tiff_header_offset` is zero, measured from immediately after its four-byte field. TIFF offsets are relative to `MM`.

This is commonly parseable as an empty TIFF, but zero-entry IFDs violate TIFF 6.0’s at-least-one-entry requirement, and this is not a fully populated conforming EXIF structure. I would not advertise universal reader acceptance.

For XMP, this exact UTF-8 serialization is **226 bytes**, with `BOM` replaced by bytes `EF BB BF`, no newline:

```xml
<?xpacket begin="BOM" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about=""/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>
```

Insert ASCII spaces **before the closing processing instruction** to fill the item. This is the minimum for this serialization, not the shortest possible XMP. Respect `content_encoding`; these bytes assume unencoded XML.

Across multiple extents, write one logical payload, not one header per extent. Below the chosen minimum, use B or refuse.

## B/D structural hazards

For B, repair both incoming and outgoing `iref` references, `ipma` associations, and applicable `grpl` entity lists/counts. Check `pitm` and dependency graphs; do not delete an item needed for image construction.

Resolve every extent: method 0 uses `base_offset + extent_offset` in the selected data reference; method 1 addresses `idat`; method 2 involves item construction. Detect shared/overlapping storage before scrubbing. Unsupported references or methods require refusal.

D offers no standardized “ignore this metadata” FourCC. Unknown-item handling varies; dependencies remain, and type-specific `infe` syntax changes. It is not a compatibility guarantee.

## Code defects and regression gates

- `infe` version 3’s type starts at **18**, not 20.
- `item_types` bounds fields against the enclosing body, not each entry; short entries can panic or consume neighbouring bytes. It ignores `entry_count`.
- Nonzero `data_reference_index` is ignored; unsupported construction methods silently escape removal.
- Only overlaps with `iinf`/`iloc` are checked—not image data, other boxes or shared extents.
- Partial box walks and missing item locations can report success. First-`meta` discovery misses other scopes, including sequence metadata.
- The fixture is not a valid decodable HEIF/Exif conformance test.

Assert complete graph validity; unchanged coded/auxiliary payload hashes and rendering properties; no recoverable personal bytes; clean metadata parsing; stable orientation, colour and HDR on real readers; idempotence; and refusal on unsupported layouts. Validate the complete candidate before an atomic replacement—never overwrite the original incrementally.