//! HEIF containers, which is what an iPhone camera writes.
//!
//! A HEIF is not a stream of segments like a JPEG or a chain of chunks like a
//! PNG. It is a tree of ISOBMFF boxes, and the picture is stored as *items*: a
//! grid of coded tiles, each declared in `iinf` and located by `iloc` as an
//! offset and a length into the `mdat` blob at the end. EXIF and XMP are items
//! too, sitting in that same blob alongside the tiles.
//!
//! That shape decides how metadata is removed here. Cutting the bytes out would
//! move everything after them, and every offset in `iloc` is absolute, so each
//! one would have to be found and rewritten — a great deal of arithmetic where a
//! single mistake produces a file that opens and shows the wrong thing.
//! Overwriting the payloads where they lie destroys them just as completely and
//! moves nothing. The file keeps a few kilobytes of dead space, which is a fair
//! price for an operation whose purpose is removal rather than size.
//!
//! ImageIO looks like it should do this and does not. Asking
//! `CGImageDestinationCopyImageSource` to drop metadata works on JPEG and, on
//! HEIF, returns success having changed nothing at all — measured on a
//! photograph whose GPS, EXIF and maker note all survived a copy that reported
//! itself as having excluded them.

/// Item types holding metadata rather than picture.
///
/// `Exif` is the EXIF block, carrying GPS, camera identity and timestamps.
/// `mime` is how XMP is stored in a HEIF.
const METADATA_ITEMS: [&[u8; 4]; 2] = [b"Exif", b"mime"];

/// Whether these bytes are a HEIF container.
///
/// The brand list is the set an Apple camera and its exports actually write.
pub fn is_heif(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    matches!(
        &bytes[8..12],
        b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" | b"miaf"
    )
}

/// One box: where it starts, how long it is, and how much of that is header.
struct Box {
    at: usize,
    size: usize,
    header: usize,
}

impl Box {
    fn body(&self) -> std::ops::Range<usize> {
        self.at + self.header..self.at + self.size
    }
}

/// Read the boxes laid out in `range`, without recursing.
fn boxes(bytes: &[u8], range: std::ops::Range<usize>) -> Vec<(&[u8], Box)> {
    let mut out = Vec::new();
    let mut i = range.start;
    while i + 8 <= range.end {
        let size = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let kind = &bytes[i + 4..i + 8];
        let (size, header) = match size {
            // A size of one means the real size is the following 64 bits.
            1 if i + 16 <= range.end => {
                let mut wide = [0u8; 8];
                wide.copy_from_slice(&bytes[i + 8..i + 16]);
                (u64::from_be_bytes(wide) as usize, 16)
            }
            // A size of zero means the box runs to the end of its parent.
            0 => (range.end - i, 8),
            n => (n, 8),
        };
        if size < header || i + size > range.end {
            break;
        }
        out.push((kind, Box { at: i, size, header }));
        i += size;
    }
    out
}

/// Find a box by kind at the top level, then inside `meta`.
///
/// `meta` is a full box, so its children start four bytes into its body, past
/// the version and flags.
fn find<'a>(bytes: &'a [u8], kind: &[u8]) -> Option<Box> {
    let top = boxes(bytes, 0..bytes.len());
    if let Some((_, b)) = top.iter().find(|(k, _)| *k == kind) {
        return Some(Box { at: b.at, size: b.size, header: b.header });
    }
    let (_, meta) = top.into_iter().find(|(k, _)| *k == b"meta")?;
    let children = meta.at + meta.header + 4..meta.at + meta.size;
    boxes(bytes, children)
        .into_iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, b)| b)
}

/// The type of every item declared in `iinf`, by item identifier.
fn item_types(bytes: &[u8], iinf: &Box) -> Vec<(u32, [u8; 4])> {
    let body = &bytes[iinf.body()];
    if body.is_empty() {
        return Vec::new();
    }
    // A FullBox: one version byte, three flag bytes, then a count whose width
    // depends on the version.
    let mut at = if body[0] == 0 { 6 } else { 8 };
    let mut out = Vec::new();
    while at + 12 <= body.len() {
        let size = u32::from_be_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]) as usize;
        if size < 12 || at + size > body.len() {
            break;
        }
        if &body[at + 4..at + 8] == b"infe" {
            let version = body[at + 8];
            // Version 2 numbers items in sixteen bits, version 3 in thirty-two.
            let (id, type_at) = match version {
                2 => (u16::from_be_bytes([body[at + 12], body[at + 13]]) as u32, at + 16),
                3 => (
                    u32::from_be_bytes([body[at + 12], body[at + 13], body[at + 14], body[at + 15]]),
                    at + 20,
                ),
                _ => {
                    at += size;
                    continue;
                }
            };
            if type_at + 4 <= body.len() {
                let mut kind = [0u8; 4];
                kind.copy_from_slice(&body[type_at..type_at + 4]);
                out.push((id, kind));
            }
        }
        at += size;
    }
    out
}

/// Where each item's bytes are, by item identifier.
///
/// Only items the file places at an absolute offset are reported. An item built
/// some other way — `construction_method` other than zero puts it inside `idat`
/// rather than in the file at large — is left out rather than guessed at, since
/// the cost of guessing is overwriting part of the picture.
fn item_extents(bytes: &[u8], iloc: &Box) -> Option<Vec<(u32, Vec<(usize, usize)>)>> {
    let body = &bytes[iloc.body()];
    if body.len() < 8 {
        return None;
    }
    let version = body[0];
    let mut at = 4;

    let offset_size = (body[at] >> 4) as usize;
    let length_size = (body[at] & 0xF) as usize;
    let base_size = (body[at + 1] >> 4) as usize;
    let index_size = (body[at + 1] & 0xF) as usize;
    at += 2;

    let count = if version < 2 {
        let n = u16::from_be_bytes([body[at], body[at + 1]]) as usize;
        at += 2;
        n
    } else {
        let n = u32::from_be_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]) as usize;
        at += 4;
        n
    };

    // Widths come from the header above and are at most eight bytes each.
    let read = |width: usize, at: &mut usize| -> Option<u64> {
        if width == 0 {
            return Some(0);
        }
        if *at + width > body.len() || width > 8 {
            return None;
        }
        let mut wide = [0u8; 8];
        wide[8 - width..].copy_from_slice(&body[*at..*at + width]);
        *at += width;
        Some(u64::from_be_bytes(wide))
    };

    let mut out = Vec::new();
    for _ in 0..count {
        let id = if version < 2 { read(2, &mut at)? as u32 } else { read(4, &mut at)? as u32 };
        let mut absolute = true;
        if version >= 1 {
            let method = read(2, &mut at)? & 0xF;
            absolute = method == 0;
        }
        read(2, &mut at)?; // data reference index
        let base = read(base_size, &mut at)?;
        let extent_count = read(2, &mut at)? as usize;

        let mut extents = Vec::new();
        for _ in 0..extent_count {
            if version >= 1 && index_size > 0 {
                read(index_size, &mut at)?;
            }
            let offset = read(offset_size, &mut at)?;
            let length = read(length_size, &mut at)?;
            let start = base.checked_add(offset)? as usize;
            let end = start.checked_add(length as usize)?;
            if end > bytes.len() {
                return None;
            }
            if absolute {
                extents.push((start, length as usize));
            }
        }
        out.push((id, extents));
    }
    Some(out)
}

/// Overwrite a HEIF's metadata where it lies, leaving the picture untouched.
///
/// Returns the result and how many bytes were destroyed. Zero means the file
/// carried none of the items this removes, which is a different outcome from a
/// failure and is reported as one.
pub fn strip_heif(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    if !is_heif(bytes) {
        return None;
    }
    let iinf = find(bytes, b"iinf")?;
    let iloc = find(bytes, b"iloc")?;
    let types = item_types(bytes, &iinf);
    let extents = item_extents(bytes, &iloc)?;

    let mut out = bytes.to_vec();
    let mut wiped = 0;
    for (id, kind) in &types {
        if !METADATA_ITEMS.iter().any(|m| m[..] == kind[..]) {
            continue;
        }
        let Some((_, ranges)) = extents.iter().find(|(other, _)| other == id) else {
            continue;
        };
        for &(start, length) in ranges {
            // An item whose bytes overlap the declarations that describe it
            // means this file is not laid out the way it is read here, and
            // zeroing would take out part of the structure.
            let end = start + length;
            let overlaps = |b: &Box| start < b.at + b.size && b.at < end;
            if overlaps(&iinf) || overlaps(&iloc) {
                return None;
            }
            out[start..end].fill(0);
            wiped += length;
        }
    }
    Some((out, wiped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be32(n: u32) -> [u8; 4] {
        n.to_be_bytes()
    }

    /// A HEIF with one picture item and one EXIF item, laid out the way a
    /// camera lays one out: declarations in `meta`, payloads in `mdat`, and
    /// absolute offsets tying them together.
    fn heif_with_exif() -> (Vec<u8>, std::ops::Range<usize>) {
        // Two items: 1 is the picture, 2 is the EXIF block.
        let mut infe1 = Vec::new();
        infe1.extend_from_slice(&be32(0)); // size, filled below
        infe1.extend_from_slice(b"infe");
        infe1.extend_from_slice(&[2, 0, 0, 0]); // version 2, no flags
        infe1.extend_from_slice(&1u16.to_be_bytes()); // item id
        infe1.extend_from_slice(&0u16.to_be_bytes()); // protection
        infe1.extend_from_slice(b"hvc1");
        let n = infe1.len() as u32;
        infe1[0..4].copy_from_slice(&be32(n));

        let mut infe2 = infe1.clone();
        infe2[12..14].copy_from_slice(&2u16.to_be_bytes());
        infe2[16..20].copy_from_slice(b"Exif");

        let mut iinf = Vec::new();
        iinf.extend_from_slice(&be32(0));
        iinf.extend_from_slice(b"iinf");
        iinf.extend_from_slice(&[0, 0, 0, 0]); // version 0
        iinf.extend_from_slice(&2u16.to_be_bytes()); // two entries
        iinf.extend_from_slice(&infe1);
        iinf.extend_from_slice(&infe2);
        let n = iinf.len() as u32;
        iinf[0..4].copy_from_slice(&be32(n));

        // iloc version 1, four-byte offsets and lengths, no base, no index.
        let mut iloc = Vec::new();
        iloc.extend_from_slice(&be32(0));
        iloc.extend_from_slice(b"iloc");
        iloc.extend_from_slice(&[1, 0, 0, 0]);
        iloc.push(0x44); // offset_size 4, length_size 4
        iloc.push(0x00); // base_size 0, index_size 0
        iloc.extend_from_slice(&2u16.to_be_bytes()); // two items
        // Filled in once the payload positions are known.
        for (id, _) in [(1u16, ()), (2u16, ())] {
            iloc.extend_from_slice(&id.to_be_bytes());
            iloc.extend_from_slice(&0u16.to_be_bytes()); // construction method 0
            iloc.extend_from_slice(&0u16.to_be_bytes()); // data reference
            iloc.extend_from_slice(&1u16.to_be_bytes()); // one extent
            iloc.extend_from_slice(&be32(0)); // offset
            iloc.extend_from_slice(&be32(0)); // length
        }
        let n = iloc.len() as u32;
        iloc[0..4].copy_from_slice(&be32(n));

        let mut meta = Vec::new();
        meta.extend_from_slice(&be32(0));
        meta.extend_from_slice(b"meta");
        meta.extend_from_slice(&[0, 0, 0, 0]);
        meta.extend_from_slice(&iinf);
        let iloc_in_meta = meta.len();
        meta.extend_from_slice(&iloc);
        let n = meta.len() as u32;
        meta[0..4].copy_from_slice(&be32(n));

        let mut out = Vec::new();
        out.extend_from_slice(&be32(20));
        out.extend_from_slice(b"ftypheic");
        out.extend_from_slice(&[0; 8]);
        let meta_at = out.len();
        out.extend_from_slice(&meta);

        let mdat_at = out.len();
        out.extend_from_slice(&be32(8 + 16 + 24));
        out.extend_from_slice(b"mdat");
        let picture_at = out.len();
        out.extend_from_slice(&[0xAA; 16]); // stands in for coded picture
        const EXIF: &[u8] = b"Exif\0\0GPS 51.5 N 0.1 W\0";
        let exif_at = out.len();
        out.extend_from_slice(EXIF);
        let exif_len = EXIF.len();
        let _ = mdat_at;

        // Patch the two extents now that the payloads have addresses.
        let base = meta_at + iloc_in_meta + 16;
        out[base + 8..base + 12].copy_from_slice(&be32(picture_at as u32));
        out[base + 12..base + 16].copy_from_slice(&be32(16));
        const ENTRY: usize = 16;
        out[base + ENTRY + 8..base + ENTRY + 12].copy_from_slice(&be32(exif_at as u32));
        out[base + ENTRY + 12..base + ENTRY + 16].copy_from_slice(&be32(exif_len as u32));

        (out, exif_at..exif_at + exif_len)
    }

    #[test]
    fn recognises_what_an_iphone_writes() {
        let (heif, _) = heif_with_exif();
        assert!(is_heif(&heif));
        assert!(!is_heif(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_heif(b"\x89PNG\r\n\x1a\n\0\0\0\0"));
        assert!(!is_heif(b"short"));
    }

    #[test]
    fn destroys_the_exif_payload_and_leaves_the_picture_alone() {
        let (heif, exif) = heif_with_exif();
        assert!(contains(&heif, b"GPS 51.5 N"), "the fixture should carry a location");

        let (out, wiped) = strip_heif(&heif).expect("a well formed heif");

        assert_eq!(wiped, exif.len(), "the whole exif payload should go");
        assert_eq!(out.len(), heif.len(), "nothing moves, so nothing changes length");
        assert!(!contains(&out, b"GPS 51.5 N"), "the location survived");
        assert!(out[exif].iter().all(|b| *b == 0), "the payload should be zeroed");
        // The coded picture is sixteen bytes of 0xAA and must be untouched.
        assert_eq!(out.iter().filter(|b| **b == 0xAA).count(), 16);
    }

    #[test]
    fn reports_nothing_wiped_when_there_is_no_metadata() {
        let (heif, exif) = heif_with_exif();
        // Rename the item type so nothing matches what this removes.
        let mut without = heif.clone();
        let at = find_bytes(&without, b"Exif").expect("the declaration");
        without[at..at + 4].copy_from_slice(b"hvc1");

        let (out, wiped) = strip_heif(&without).expect("still a heif");
        assert_eq!(wiped, 0, "nothing should have been removed");
        assert!(contains(&out[exif.clone()], b"GPS"), "and nothing should have been touched");
    }

    #[test]
    fn refuses_something_that_is_not_a_heif() {
        assert!(strip_heif(b"not a heif at all, not even close").is_none());
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}
