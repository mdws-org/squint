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
    if !has_ftyp(bytes) {
        return false;
    }
    matches!(
        &bytes[8..12],
        b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" | b"miaf"
    ) && !is_avif(bytes)
}

/// Whether these bytes are an AVIF container.
///
/// AVIF is the same ISOBMFF tree as a HEIF with AV1 in the tiles instead of
/// HEVC, so everything below reads one exactly as it reads the other. The two
/// are told apart only to name the format in a message and in `converted_from`.
///
/// The compatible-brand list is consulted as well as the major brand: an AVIF
/// written with `mif1` as its major brand is common, and it is matched by the
/// HEIF brand list above, so major brand alone would call it a HEIC.
pub fn is_avif(bytes: &[u8]) -> bool {
    if !has_ftyp(bytes) {
        return false;
    }
    let avif_brand = |b: &[u8]| b == b"avif" || b == b"avis";
    avif_brand(&bytes[8..12]) || compatible_brands(bytes).any(avif_brand)
}

/// Whether these bytes are one of the ISOBMFF pictures read here.
pub fn is_isobmff_image(bytes: &[u8]) -> bool {
    is_heif(bytes) || is_avif(bytes)
}

/// Brands whose files hold a sequence of pictures rather than one.
///
/// `avis` is the AVIF image sequence, the animated counterpart of `avif`.
/// `msf1` is its HEIF equivalent, and `hevc` and `hevx` are the HEVC-coded
/// sequences that sit beside the `heic` and `heix` single images. The naming
/// invites the mistake: `heic` is one picture and `hevc` is many, a letter
/// apart.
const SEQUENCE_BRANDS: [&[u8; 4]; 4] = [b"avis", b"msf1", b"hevc", b"hevx"];

/// Whether this container holds a sequence of pictures rather than one.
///
/// Image I/O will decode a sequence to its primary frame, which would quietly
/// turn it into a single JPEG — the loss an animated WebP is refused to avoid,
/// arriving by another door. A sequence is refused for re-encoding; its
/// metadata can still be stripped, which leaves the frames where they are.
pub fn is_image_sequence(bytes: &[u8]) -> bool {
    if !has_ftyp(bytes) {
        return false;
    }
    let sequence = |b: &[u8]| SEQUENCE_BRANDS.iter().any(|s| s[..] == *b);
    sequence(&bytes[8..12]) || compatible_brands(bytes).any(sequence)
}

/// The name for the container, for a message and for `converted_from`.
pub fn container_name(bytes: &[u8]) -> &'static str {
    if is_avif(bytes) {
        "AVIF"
    } else {
        "HEIC"
    }
}

fn has_ftyp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

/// The brands listed after the major brand and minor version in `ftyp`.
///
/// The `ftyp` box's declared size bounds the walk; a size that runs past the
/// bytes given yields nothing rather than reading whatever follows.
fn compatible_brands(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    // The list begins at sixteen, after the major brand and the minor version.
    // A file can be an `ftyp` and stop before that — twelve bytes is a whole
    // major brand — so the start is clamped as well as the end, and a range
    // that would begin past the last byte collapses to an empty one.
    let start = 16.min(bytes.len());
    let declared = if (16..=bytes.len()).contains(&size) {
        size
    } else {
        start
    };
    // `size` is the box's own declaration and a file may declare anything. A
    // declaration reaching past the header swallows whatever boxes follow and
    // scans them four bytes at a time for `avif` or `avis` — which is what
    // decides whether this picture is named an AVIF and whether it is refused
    // as a sequence. A real brand list is short, so the scan is bounded and a
    // false size cannot reach beyond the header it belongs to.
    const MAX_BRANDS: usize = 32;
    let end = declared.min(start.saturating_add(MAX_BRANDS * 4));
    bytes[start..end].chunks_exact(4)
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
        // `size` comes from a 64-bit field a file is free to fill with anything,
        // so the sum is checked rather than taken. Left to wrap, a size near the
        // top of the address space lands `i + size` *below* `range.end`, passes
        // for a box that fits, and then moves `i` backwards — a walk that never
        // reaches the end, growing `out` until the machine gives up.
        let Some(past) = i.checked_add(size) else {
            break;
        };
        if size < header || past > range.end {
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
    // Four bytes of version and flags, two of field widths, then the item
    // count: two bytes wide before version 2 and four from version 2 on.
    if version >= 2 && body.len() < 10 {
        return None;
    }
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

/// Whether every item the container declares lies inside the bytes given.
///
/// A file cut short keeps its declarations, which sit at the front, and loses
/// the tail of its picture data. Image I/O reports such a file as complete
/// and draws what it can — a black or half-filled picture with no error — so
/// completeness is judged here, from the index, before it is asked to decode.
pub fn is_complete(bytes: &[u8]) -> bool {
    if !is_isobmff_image(bytes) {
        return false;
    }
    match find(bytes, b"iloc") {
        Some(iloc) => item_extents(bytes, &iloc).is_some(),
        None => false,
    }
}

/// Overwrite a HEIF's metadata where it lies, leaving the picture untouched.
///
/// Returns the result and how many bytes were destroyed. Zero means the file
/// carried none of the items this removes, which is a different outcome from a
/// failure and is reported as one.
pub fn strip_heif(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    if !is_isobmff_image(bytes) {
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
    fn completeness_is_judged_from_the_index() {
        assert!(!is_complete(b"not a heif"));
        assert!(!is_complete(b"\0\0\0\x18ftypheic\0\0\0\0mif1heic"), "a brand alone declares nothing");
    }

    #[test]
    fn refuses_something_that_is_not_a_heif() {
        assert!(strip_heif(b"not a heif at all, not even close").is_none());
    }

    /// An `ftyp` box: major brand, minor version, then compatible brands.
    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let mut out = Vec::new();
        let size = 16 + 4 * compatible.len();
        out.extend_from_slice(&be32(size as u32));
        out.extend_from_slice(b"ftyp");
        out.extend_from_slice(major);
        out.extend_from_slice(&be32(0));
        for brand in compatible {
            out.extend_from_slice(*brand);
        }
        out
    }

    #[test]
    fn a_brand_declared_past_the_header_is_not_read_as_one() {
        // An `ftyp` claiming to be far longer than it is, with `avif` planted
        // where a following box would sit. Believing the claim reads that
        // FourCC as a compatible brand and renames the picture.
        let mut lying = ftyp(b"heic", &[b"mif1"]);
        let header = lying.len();
        lying.extend_from_slice(&vec![0u8; 400]);
        lying.extend_from_slice(b"avif");
        lying.extend_from_slice(&vec![0u8; 64]);
        let total = lying.len() as u32;
        lying[0..4].copy_from_slice(&total.to_be_bytes());

        assert!(
            !is_avif(&lying),
            "a FourCC beyond the brand list is not a brand"
        );
        assert!(!is_image_sequence(&lying));
        assert_eq!(container_name(&lying), "HEIC");
        assert!(is_heif(&lying));
    }

    #[test]
    fn a_file_too_short_to_hold_a_brand_list_is_not_read_for_one() {
        // Twelve bytes is enough to be an `ftyp` with a major brand and nothing
        // after it, and the compatible-brand list begins at sixteen. Reaching
        // for it has to yield nothing rather than index past the end: this
        // arrives from a Finder context menu on whatever the user right-clicked.
        for len in 12..=15 {
            let mut short = b"\x00\x00\x00\x0cftypmif1".to_vec();
            short.resize(len, 0);
            assert!(is_heif(&short), "the major brand still reads");
            assert!(!is_avif(&short), "and there are no compatible brands to find");
            assert_eq!(container_name(&short), "HEIC");
        }
    }

    #[test]
    fn an_avif_is_told_apart_from_a_heic() {
        let avif = ftyp(b"avif", &[b"mif1", b"miaf"]);
        assert!(is_avif(&avif));
        assert!(!is_heif(&avif), "an AVIF must not also answer to HEIC");
        assert_eq!(container_name(&avif), "AVIF");

        let heic = ftyp(b"heic", &[b"mif1"]);
        assert!(is_heif(&heic));
        assert!(!is_avif(&heic));
        assert_eq!(container_name(&heic), "HEIC");
    }

    /// The case major brand alone gets wrong: `mif1` is in the HEIF list, so an
    /// AVIF that leads with it would be called a HEIC and named as one in every
    /// message, even though the compatible brands say what it is.
    #[test]
    fn a_mif1_avif_is_recognised_by_its_compatible_brands() {
        let avif = ftyp(b"mif1", &[b"avif"]);
        assert!(is_avif(&avif));
        assert!(!is_heif(&avif));
        assert_eq!(container_name(&avif), "AVIF");

        let heif = ftyp(b"mif1", &[b"heic"]);
        assert!(is_heif(&heif), "mif1 without an AVIF brand stays a HEIF");
        assert!(!is_avif(&heif));
    }

    #[test]
    fn a_lying_ftyp_size_does_not_read_past_the_bytes() {
        // The declared size claims brands that are not there. Reading them
        // would walk into whatever follows in memory and could match `avif` by
        // accident, so an out-of-range size yields no compatible brands.
        let mut avif = ftyp(b"mif1", &[b"avif"]);
        avif[0..4].copy_from_slice(&be32(4096));
        assert!(!is_avif(&avif), "brands past the end are not read");
        assert!(is_heif(&avif), "the major brand still stands on its own");

        // A size smaller than the header cannot bound anything either.
        let mut truncated = ftyp(b"mif1", &[b"avif"]);
        truncated[0..4].copy_from_slice(&be32(8));
        assert!(!is_avif(&truncated));
    }

    #[test]
    fn every_sequence_brand_is_refused_not_only_the_avif_one() {
        // `heic` is one picture and `hevc` is many, one letter apart, and both
        // were in the brand list this reads as a still.
        for brand in [b"msf1", b"hevc", b"hevx", b"avis"] {
            let seq = ftyp(brand, &[b"mif1"]);
            assert!(is_image_sequence(&seq), "{} is a sequence", String::from_utf8_lossy(brand));
        }
        for brand in [b"heic", b"heix", b"heim", b"heis", b"mif1", b"miaf"] {
            let single = ftyp(brand, &[b"mif1"]);
            assert!(
                !is_image_sequence(&single),
                "{} holds one picture",
                String::from_utf8_lossy(brand)
            );
        }
        // Declared only among the compatible brands, which is how a real one
        // is often written.
        assert!(is_image_sequence(&ftyp(b"mif1", &[b"msf1"])));
    }

    #[test]
    fn an_image_sequence_is_not_mistaken_for_a_single_picture() {
        let sequence = ftyp(b"avis", &[b"avif", b"miaf"]);
        assert!(is_avif(&sequence), "a sequence is still an AVIF");
        assert!(is_image_sequence(&sequence));

        // Written the other way round, which is how a real one often is.
        let by_compatible = ftyp(b"avif", &[b"avis", b"miaf"]);
        assert!(is_image_sequence(&by_compatible));

        let single = ftyp(b"avif", &[b"mif1", b"miaf"]);
        assert!(!is_image_sequence(&single), "one picture is not a sequence");

        let heic = ftyp(b"heic", &[b"mif1"]);
        assert!(!is_image_sequence(&heic));

        // The same short-file case the brand walk has to survive.
        assert!(!is_image_sequence(b"\x00\x00\x00\x0cftypavif"));
    }

    #[test]
    fn a_box_size_near_the_top_of_the_address_space_stops_the_walk() {
        // A 64-bit box size the file is free to invent. Added to the offset it
        // wraps, landing below the end of the range, so an unchecked walk reads
        // it as a box that fits and then steps backwards and never finishes.
        let mut bytes = b"\x00\x00\x00\x10ftypheic\x00\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&1u32.to_be_bytes()); // size 1: the real size is 64-bit
        bytes.extend_from_slice(b"meta");
        bytes.extend_from_slice(&u64::MAX.to_be_bytes());

        // Reaching an answer at all is the assertion; a walk that wrapped would
        // not return from here.
        assert!(!is_complete(&bytes), "nothing in this file locates an item");
        assert!(strip_heif(&bytes).is_none());
    }

    #[test]
    fn an_avif_is_stripped_by_the_same_walk_as_a_heif() {
        // Same tree, AVIF brand: the ISOBMFF layout is what the strip reads,
        // and the codec in the tiles is not part of it.
        let (mut bytes, exif) = heif_with_exif();
        assert!(contains(&bytes[exif.clone()], b"GPS"));
        bytes[8..12].copy_from_slice(b"avif");
        assert!(is_avif(&bytes));

        let (out, wiped) = strip_heif(&bytes).expect("an AVIF strips");
        assert!(wiped > 0, "the EXIF item should have been destroyed");
        assert!(!contains(&out[exif], b"GPS"));
        assert!(is_complete(&bytes), "and the index still describes the file");
    }

    /// Found by the fuzzer: an `iloc` box of exactly eight bytes that declares
    /// version 2, whose item count is four bytes wide. The walk read those four
    /// bytes past the end of the box and panicked. It now refuses the box.
    #[test]
    fn an_iloc_too_short_for_its_version_is_refused() {
        let hostile: [u8; 62] = [
            0x00, 0x00, 0x00, 0x2e, 0x66, 0x74, 0x79, 0x70, 0x6d, 0x73, 0x66, 0x31, 0x68, 0x65,
            0x76, 0x63, 0x6d, 0xff, 0xff, 0xff, 0xff, 0x73, 0x00, 0x2d, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x70, 0x68, 0x65, 0x0c, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x69, 0x6c, 0x6f, 0x63, 0xa3, 0x00,
            0x00, 0x00, 0x2e, 0x66, 0x74, 0x79,
        ];
        assert!(!is_complete(&hostile));
        if let Some((out, _)) = strip_heif(&hostile) {
            assert_eq!(out.len(), hostile.len());
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}
