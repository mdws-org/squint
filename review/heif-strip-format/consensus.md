# HEIC metadata removal: what to leave where the metadata was — round 1 synthesis (2026-10-12)

## Lanes

| lane | route | saw | result |
|---|---|---|---|
| SWE-2 max | Devin | brief + heif.rs | 6.5 KB |
| DeepSeek V4.1 Flash | ccx | brief + heif.rs appended | 5.5 KB |
| Gemini | agyx | brief + heif.rs by path | 6.5 KB |
| GPT-6 Luna | Kagi (Codex on ccx was at its free-plan limit) | brief + heif.rs attached | 7.6 KB |
| GPT-6 Astra | Devin | — | 0 bytes (paid Devin seats unavailable after the Max plan ended 10-11) |
| GPT-6 Astra | Kaggle proxy | brief + heif.rs inlined | 5.9 KB |

## The choice

Four of five lanes recommend **C, a valid empty payload in place**, for the Exif item. Astra recommends **B, removing the item structurally** inside the `meta` box's original footprint, and refusing when that cannot be done safely. B needs consistent edits to `iinf`, `iloc`, `iref`, `ipma` and `grpl`, which every lane, Astra included, lists as the risk; C touches payload bytes only. C is adopted.

On XMP, all five agree: do not destroy every `mime` item; keep a packet only when an allowlist of rendering properties covers everything in it, and fail closed otherwise.

## Probed before adoption (svk, Image I/O, ExifTool 13, Pillow + pillow-heif)

- Zero-fill (main), iPhone 17 Pro Max HEIC: Pillow `getexif()` raises "not a TIFF file"; ExifTool `Invalid XMP [x2]`. Reproduced.
- C for Exif and XMP on the same file: Pillow returns 0 tags without raising; ExifTool `-validate` OK; Image I/O 0 EXIF and 0 GPS keys, orientation 6, Display P3 and headroom 3.50 kept.
- **Not raised by any lane, found while probing:** on older iPhone HEICs (14 Pro Max, 15 Pro, 12) the Apple gain map is applied by maker-note tags 33 and 48 inside the Exif item, as on the JPEG path. Main's Strip drops headroom from 8.00, 6.91 and 4.00 to 3.48 on those three files. Zero-fill and an empty TIFF both lose it.
- The two maker-note tags restore it only when the Exif item is laid out as the iPhone lays it out (offset 6, `Exif\0\0`, then the TIFF). With offset 0 and the TIFF straight after it, Image I/O read the same maker note and reported 1.00. Adopted: offset 6.
- The XMP packets on these files hold only `HDRGainMap:*` and `semanticSegmentationMatte:*` properties. Destroying or emptying them did not change the headroom Image I/O reported on any file tested, so keeping them is about not damaging what the brief calls rendering data, not about headroom on these files.

## Defects the lanes found in strip_heif, checked against the code

| claim | lanes | verdict |
|---|---|---|
| `infe` version 3's type is at entry offset 18, not 20 | Astra | **confirmed** (32-bit id + 16-bit protection index after the 12-byte FullBox header); fixed, test added |
| `item_types` bounds fields against `iinf`'s body, not the entry | Astra | **confirmed**; fields now read from inside the entry |
| every `mime` item is treated as XMP regardless of content type | all five | **confirmed**; only `application/rdf+xml` is treated as XMP |
| `data_reference_index` read and discarded | SWE-2, DeepSeek, Astra | **confirmed**; non-zero is now "not in this file" |
| `construction_method` 1/2 metadata silently skipped while reporting success | all five | **confirmed**; now a refusal |
| overlap check covers only `iinf`/`iloc` | DeepSeek, Luna, Astra | **confirmed**; a metadata extent shared with any non-metadata item is now a refusal |
| `start + length` unchecked | Luna | **confirmed**; `checked_add` |
| zero-entry IFD0 violates TIFF 6.0 | Astra | **accepted**; the written block has IFD0 → Exif IFD with ExifVersion, no empty directory |

## Not adopted

- Retyping items (D) as the short-item fallback (SWE-2, Gemini): no reader-neutral fourcc exists, as Astra and DeepSeek say. An item too short for the plain block is zeroed as before; no real camera writes one.
- Rebuilding packets with only allowlisted properties (Astra): the packets seen are all-or-nothing, so keep-or-empty covers them without an XML writer.
