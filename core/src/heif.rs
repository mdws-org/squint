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

/// One entry of `iinf`: the item's identifier, its type, and for a `mime`
/// item the content type that says what the bytes are.
struct ItemInfo {
    id: u32,
    kind: [u8; 4],
    content_type: Vec<u8>,
}

/// Every item declared in `iinf`.
///
/// Each field is read from inside its own `infe` entry. An entry too short to
/// hold the fields its version declares is skipped rather than read into the
/// next one.
fn item_types(bytes: &[u8], iinf: &Box) -> Vec<ItemInfo> {
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
        let entry = &body[at..at + size];
        if &entry[4..8] == b"infe" {
            // After the 12-byte FullBox header: version 2 numbers items in
            // sixteen bits and version 3 in thirty-two, then two bytes of
            // protection index, then the four-byte type.
            let (id, type_at) = match entry[8] {
                2 if size >= 20 => (u16::from_be_bytes([entry[12], entry[13]]) as u32, 16),
                3 if size >= 22 => (u32::from_be_bytes([entry[12], entry[13], entry[14], entry[15]]), 18),
                _ => {
                    at += size;
                    continue;
                }
            };
            let mut kind = [0u8; 4];
            kind.copy_from_slice(&entry[type_at..type_at + 4]);
            // A `mime` item's name and content type follow as two
            // null-terminated strings.
            let mut content_type = Vec::new();
            if &kind == b"mime" {
                let rest = &entry[type_at + 4..];
                if let Some(name_end) = rest.iter().position(|b| *b == 0) {
                    let after = &rest[name_end + 1..];
                    let len = after.iter().position(|b| *b == 0).unwrap_or(after.len());
                    content_type = after[..len].to_vec();
                }
            }
            out.push(ItemInfo { id, kind, content_type });
        }
        at += size;
    }
    out
}

/// Where one item's bytes are.
struct ItemPlace {
    id: u32,
    /// Absolute ranges in this file, as (start, length). Empty for an item
    /// stored any other way.
    extents: Vec<(usize, usize)>,
    /// True only when the item lies in this file at absolute offsets:
    /// `construction_method` 0 and data reference 0. Anything else (inside
    /// `idat`, built from other items, or in another file) is not located here,
    /// and overwriting by guess risks part of the picture.
    in_place: bool,
}

/// Where each item's bytes are, by item identifier.
fn item_extents(bytes: &[u8], iloc: &Box) -> Option<Vec<ItemPlace>> {
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
        // Zero means this file; anything else names an external one, whose
        // offsets mean nothing here.
        let data_reference = read(2, &mut at)?;
        absolute &= data_reference == 0;
        let base = read(base_size, &mut at)?;
        let extent_count = read(2, &mut at)? as usize;
        // Each extent normally takes bytes of the box to describe, which bounds
        // how many a file of a given size can declare. With every width zero
        // it takes none, so the count is bounded only by its sixteen bits, and
        // a few kilobytes of `iloc` could declare tens of millions of empty
        // extents to walk and keep. More than one such extent says nothing
        // about where the item is, so the box is refused.
        let extent_width = offset_size + length_size + if version >= 1 { index_size } else { 0 };
        if extent_width == 0 && extent_count > 1 {
            return None;
        }

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
        out.push(ItemPlace { id, extents, in_place: absolute });
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

/// Remove a HEIF's metadata where it lies, leaving the picture untouched.
///
/// Returns the result and how many bytes were replaced. Zero means the file
/// carried none of the items this removes, which is a different outcome from a
/// failure and is reported as one.
///
/// Nothing moves: each payload is overwritten in place, at its own length, so
/// every absolute offset in `iloc` stays true. What is written in place of a
/// payload is a valid, empty instance of what the item declares itself to be,
/// because readers that trust the declaration parse the bytes. A zero-filled
/// `Exif` item made Pillow's `getexif()` raise "not a TIFF file", and
/// zero-filled XMP drew "Invalid XMP" from ExifTool.
///
/// - An `Exif` item becomes a minimal EXIF block: no camera, place or time,
///   and Apple's two headroom values (maker note tags 33 and 48) when the
///   original carried them. An older iPhone's gain map is applied by those two
///   values alone; measured on an iPhone 14 Pro Max HEIC, Image I/O reported
///   8.00 of headroom before and 3.48 after they were removed.
/// - A `mime` item holding XMP is kept unchanged when every property in it is
///   one an Apple auxiliary image renders by (the HDR gain map's version and
///   headroom, a segmentation matte's version) and is replaced with an empty
///   packet otherwise.
/// - Any other `mime` item is zeroed, as before.
///
/// A metadata item this cannot locate in place, or whose bytes overlap the
/// declarations or another item, is a refusal: reporting success while the
/// metadata survives is worse than not stripping.
pub fn strip_heif(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    if !is_isobmff_image(bytes) {
        return None;
    }
    let iinf = find(bytes, b"iinf")?;
    let iloc = find(bytes, b"iloc")?;
    let items = item_types(bytes, &iinf);
    let places = item_extents(bytes, &iloc)?;
    let is_metadata = |id: u32| {
        items
            .iter()
            .any(|i| i.id == id && METADATA_ITEMS.iter().any(|m| m[..] == i.kind[..]))
    };

    let mut out = bytes.to_vec();
    let mut wiped = 0;
    for item in &items {
        if !METADATA_ITEMS.iter().any(|m| m[..] == item.kind[..]) {
            continue;
        }
        let Some(place) = places.iter().find(|p| p.id == item.id) else {
            continue;
        };
        if !place.in_place {
            return None;
        }
        for &(start, length) in &place.extents {
            let end = start.checked_add(length)?;
            let overlaps = |s: usize, e: usize| start < e && s < end;
            if overlaps(iinf.at, iinf.at + iinf.size) || overlaps(iloc.at, iloc.at + iloc.size) {
                return None;
            }
            let shared = places
                .iter()
                .filter(|p| p.id != item.id && !is_metadata(p.id))
                .flat_map(|p| p.extents.iter())
                .any(|&(s, l)| overlaps(s, s.saturating_add(l)));
            if shared {
                return None;
            }
        }

        let old: Vec<u8> =
            place.extents.iter().flat_map(|&(s, l)| bytes[s..s + l].iter().copied()).collect();
        let replacement = match &item.kind {
            b"Exif" => exif_replacement(&old),
            _ if is_xmp(&item.content_type) => {
                if xmp_is_rendering_only(&old) {
                    continue;
                }
                empty_xmp(old.len())
            }
            _ => vec![0; old.len()],
        };
        let mut written = 0;
        for &(start, length) in &place.extents {
            out[start..start + length].copy_from_slice(&replacement[written..written + length]);
            written += length;
        }
        wiped += old.len();
    }
    Some((out, wiped))
}

fn is_xmp(content_type: &[u8]) -> bool {
    content_type.eq_ignore_ascii_case(b"application/rdf+xml")
}

/// An EXIF block with nothing identifying in it, the same length as `old`.
///
/// Laid out as an iPhone lays one out: a four-byte offset of 6, `Exif\0\0`,
/// then a big-endian TIFF. The layout matters beyond tidiness: with an offset
/// of 0 and the TIFF straight after it, Image I/O read the maker note below
/// and reported 1.00 of headroom instead of 8.00. No directory is left empty,
/// since TIFF 6.0 asks for at least one entry: IFD0 points to an Exif
/// directory holding ExifVersion, and the maker note when there is one.
///
/// An item too short for even the plain block is zeroed, as it was before;
/// no real camera writes one.
fn exif_replacement(old: &[u8]) -> Vec<u8> {
    let headroom = apple_headroom(old);
    let mut block = exif_block(headroom);
    if block.len() > old.len() {
        block = exif_block(None);
    }
    if block.len() > old.len() {
        return vec![0; old.len()];
    }
    block.resize(old.len(), 0);
    block
}

/// Maker note tags 33 and 48 from an Apple EXIF block, each as the signed
/// numerator and denominator of its rational.
type Headroom = [(i32, i32); 2];

fn exif_block(headroom: Option<Headroom>) -> Vec<u8> {
    let be16 = |n: u16| n.to_be_bytes();
    let be32 = |n: u32| n.to_be_bytes();
    let entry = |tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        let mut e = Vec::with_capacity(12);
        e.extend_from_slice(&be16(tag));
        e.extend_from_slice(&be16(kind));
        e.extend_from_slice(&be32(count));
        e.extend_from_slice(&value);
        e
    };
    const ASCII_UNDEFINED: u16 = 7;
    const LONG: u16 = 4;
    const SRATIONAL: u16 = 10;

    // The maker note's own offsets count from its first byte.
    let maker: Vec<u8> = match headroom {
        Some(values) => {
            let mut m = b"Apple iOS\0".to_vec();
            m.extend_from_slice(&be16(1));
            m.extend_from_slice(b"MM");
            m.extend_from_slice(&be16(2));
            let values_at = 16 + 2 * 12 + 4;
            for (k, tag) in [33u16, 48].into_iter().enumerate() {
                m.extend(entry(tag, SRATIONAL, 1, be32((values_at + 8 * k) as u32)));
            }
            m.extend_from_slice(&be32(0));
            for (n, d) in values {
                m.extend_from_slice(&n.to_be_bytes());
                m.extend_from_slice(&d.to_be_bytes());
            }
            m
        }
        None => Vec::new(),
    };

    // TIFF offsets count from the byte order mark.
    let ifd0_at = 8u32;
    let exif_at = ifd0_at + 2 + 12 + 4;
    let exif_entries = if maker.is_empty() { 1 } else { 2 };
    let maker_at = exif_at + 2 + 12 * exif_entries + 4;

    let mut tiff = b"MM\0*".to_vec();
    tiff.extend_from_slice(&be32(ifd0_at));
    tiff.extend_from_slice(&be16(1));
    tiff.extend(entry(0x8769, LONG, 1, be32(exif_at)));
    tiff.extend_from_slice(&be32(0));
    tiff.extend_from_slice(&be16(exif_entries as u16));
    tiff.extend(entry(0x9000, ASCII_UNDEFINED, 4, *b"0232"));
    if !maker.is_empty() {
        tiff.extend(entry(0x927C, ASCII_UNDEFINED, maker.len() as u32, be32(maker_at)));
    }
    tiff.extend_from_slice(&be32(0));
    tiff.extend_from_slice(&maker);

    let mut block = be32(6).to_vec();
    block.extend_from_slice(b"Exif\0\0");
    block.extend_from_slice(&tiff);
    block
}

/// Apple maker note tags 33 and 48 from an EXIF item, if it has both.
///
/// Every read is bounds-checked against the item: this runs on bytes a file
/// supplied, and a value it cannot find is simply not carried.
fn apple_headroom(item: &[u8]) -> Option<Headroom> {
    let u16_at = |b: &[u8], at: usize, big: bool| -> Option<u16> {
        let s: [u8; 2] = b.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if big { u16::from_be_bytes(s) } else { u16::from_le_bytes(s) })
    };
    let u32_at = |b: &[u8], at: usize, big: bool| -> Option<u32> {
        let s: [u8; 4] = b.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
    };
    // The value of `tag` in the directory at `dir`, as an offset or inline value.
    let find_tag = |b: &[u8], dir: usize, big: bool, tag: u16| -> Option<(u16, u32, usize)> {
        let count = u16_at(b, dir, big)? as usize;
        (0..count.min(512)).find_map(|k| {
            let e = dir + 2 + 12 * k;
            (u16_at(b, e, big)? == tag).then_some(())?;
            Some((u16_at(b, e + 2, big)?, u32_at(b, e + 4, big)?, e + 8))
        })
    };

    let header = u32_at(item, 0, true)? as usize;
    let tiff = item.get(4usize.checked_add(header)?..)?;
    let big = match tiff.get(0..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let ifd0 = u32_at(tiff, 4, big)? as usize;
    let (_, _, exif_ptr) = find_tag(tiff, ifd0, big, 0x8769)?;
    let exif = u32_at(tiff, exif_ptr, big)? as usize;
    let (_, length, maker_ptr) = find_tag(tiff, exif, big, 0x927C)?;
    let maker_at = u32_at(tiff, maker_ptr, big)? as usize;
    let maker = tiff.get(maker_at..maker_at.checked_add(length as usize)?)?;
    if !maker.starts_with(b"Apple iOS\0") {
        return None;
    }
    let mbig = match maker.get(12..14)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let rational = |tag: u16| -> Option<(i32, i32)> {
        let (kind, count, at) = find_tag(maker, 14, mbig, tag)?;
        if kind != 10 || count != 1 {
            return None;
        }
        let value_at = u32_at(maker, at, mbig)? as usize;
        let n = u32_at(maker, value_at, mbig)? as i32;
        let d = u32_at(maker, value_at.checked_add(4)?, mbig)? as i32;
        Some((n, d))
    };
    Some([rational(33)?, rational(48)?])
}

/// The namespaces whose properties a gain map or an Apple auxiliary image is
/// rendered by. An XMP packet holding nothing else is left in place.
const RENDERING_NAMESPACES: [&str; 4] = [
    "http://ns.apple.com/HDRGainMap/1.0/",
    "http://ns.apple.com/semanticSegmentationMatte/1.0/",
    // Apple's description of an auxiliary image: what kind it is and how its
    // pixels are stored.
    "http://ns.apple.com/pixeldatainfo/1.0/",
    // Google's Ultra HDR gain map parameters.
    "http://ns.adobe.com/hdr-gain-map/1.0/",
];

/// The only values besides numbers that a rendering property may hold.
const RENDERING_WORDS: [&str; 3] = ["True", "False", "urn:com:apple:photo:2020:aux:hdrgainmap"];

/// Whether a property value, or the text between two tags, is a number, one
/// of `RENDERING_WORDS`, or nothing at all.
fn is_rendering_value(value: &str) -> bool {
    let value = value.trim();
    value.chars().all(|c| c.is_ascii_digit() || ".+-eE".contains(c)) || RENDERING_WORDS.contains(&value)
}

/// Whether an XMP packet holds nothing but rendering properties.
///
/// Fails closed: anything not recognised, including a namespace not in
/// `RENDERING_NAMESPACES`, a DOCTYPE or comment, a non-empty `rdf:about`, or a
/// property value that is neither a number nor one of `RENDERING_WORDS`, makes
/// the packet a candidate for removal. A packet kept by mistake can leak.
pub(crate) fn xmp_is_rendering_only(packet: &[u8]) -> bool {
    let end = packet.iter().rposition(|b| *b != 0 && !b.is_ascii_whitespace()).map_or(0, |e| e + 1);
    let Ok(text) = std::str::from_utf8(&packet[..end]) else {
        return false;
    };
    if text.contains("<!") {
        return false;
    }
    let mut allowed_prefixes: Vec<&str> = vec!["x", "rdf"];
    let mut found_property = false;
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        if !is_rendering_value(&rest[..open]) {
            return false;
        }
        let Some(close) = rest[open..].find('>') else {
            return false;
        };
        let tag = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        if let Some(pi) = tag.strip_prefix('?') {
            if !pi.starts_with("xpacket") {
                return false;
            }
            continue;
        }
        let tag = tag.trim_start_matches('/').trim_end_matches('/');
        let Some(name) = tag.split_whitespace().next() else {
            return false;
        };
        let attributes: Vec<(&str, &str)> = tag[name.len()..]
            .split('"')
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|pair| (pair[0].trim().trim_end_matches('=').trim(), pair.get(1).copied().unwrap_or("")))
            .filter(|(key, _)| !key.is_empty())
            .collect();
        // Declarations first, because a property can be written before the
        // namespace it belongs to is declared on the same tag.
        for (key, value) in &attributes {
            if let Some(prefix) = key.strip_prefix("xmlns:") {
                match *value {
                    "adobe:ns:meta/" | "http://www.w3.org/1999/02/22-rdf-syntax-ns#" => {}
                    v if RENDERING_NAMESPACES.contains(&v) => allowed_prefixes.push(prefix),
                    _ => return false,
                }
            }
        }
        for (key, value) in &attributes {
            match key.split_once(':') {
                Some(("xmlns", _)) | Some(("x", "xmptk")) => {}
                Some(("rdf", "about")) if value.is_empty() => {}
                // Ultra HDR writes its parameters as attributes of the description.
                Some((prefix, _))
                    if name == "rdf:Description"
                        && allowed_prefixes[2..].contains(&prefix)
                        && is_rendering_value(value) =>
                {
                    found_property = true
                }
                _ => return false,
            }
        }
        match name.split_once(':') {
            Some(("x", "xmpmeta"))
            | Some(("rdf", "RDF" | "Description" | "Seq" | "li")) => {}
            Some((prefix, _)) if allowed_prefixes[2..].contains(&prefix) => found_property = true,
            _ => return false,
        }
    }
    found_property && rest.trim().is_empty()
}

/// A valid XMP packet with nothing in it, padded with spaces to `length`, as
/// the XMP specification provides for rewriting a packet in place. Shorter
/// than the packet itself, the item is zeroed instead.
fn empty_xmp(length: usize) -> Vec<u8> {
    const HEAD: &[u8] =
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>"
            .as_bytes();
    const TAIL: &[u8] = b"<?xpacket end=\"w\"?>";
    if length < HEAD.len() + TAIL.len() {
        return vec![0; length];
    }
    let mut out = HEAD.to_vec();
    out.resize(length - TAIL.len(), b' ');
    out.extend_from_slice(TAIL);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn be32(n: u32) -> [u8; 4] {
        n.to_be_bytes()
    }

    /// A HEIF with one picture item and one EXIF item, laid out the way a
    /// camera lays one out: declarations in `meta`, payloads in `mdat`, and
    /// absolute offsets tying them together.
    pub(crate) fn heif_with_exif() -> (Vec<u8>, std::ops::Range<usize>) {
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

    #[test]
    fn empty_extents_without_width_are_refused_rather_than_walked() {
        // One item declaring two extents with every field width zero.
        let mut iloc = Vec::new();
        iloc.extend_from_slice(&be32(0));
        iloc.extend_from_slice(b"iloc");
        iloc.extend_from_slice(&[1, 0, 0, 0]);
        iloc.extend_from_slice(&[0, 0]);
        iloc.extend_from_slice(&1u16.to_be_bytes());
        iloc.extend_from_slice(&1u16.to_be_bytes());
        iloc.extend_from_slice(&0u16.to_be_bytes());
        iloc.extend_from_slice(&0u16.to_be_bytes());
        iloc.extend_from_slice(&2u16.to_be_bytes());
        let n = iloc.len() as u32;
        iloc[0..4].copy_from_slice(&be32(n));
        let b = Box { at: 0, size: iloc.len(), header: 8 };
        assert!(item_extents(&iloc, &b).is_none());

        // A single zero-width extent is still read.
        let last = iloc.len() - 1;
        iloc[last] = 1;
        assert_eq!(item_extents(&iloc, &b).map(|items| items.len()), Some(1));
    }

    /// The two XMP packets an iPhone 17 Pro Max writes beside its gain map
    /// and its segmentation matte, verbatim from a camera file.
    const IPHONE_GAIN_MAP_XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 6.0.0">
   <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
            xmlns:HDRGainMap="http://ns.apple.com/HDRGainMap/1.0/">
         <HDRGainMap:HDRGainMapVersion>131072</HDRGainMap:HDRGainMapVersion>
         <HDRGainMap:HDRGainMapHeadroom>3.500636</HDRGainMap:HDRGainMapHeadroom>
      </rdf:Description>
   </rdf:RDF>
</x:xmpmeta>"#;
    const IPHONE_MATTE_XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 6.0.0">
   <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
            xmlns:semanticSegmentationMatte="http://ns.apple.com/semanticSegmentationMatte/1.0/">
         <semanticSegmentationMatte:SemanticSegmentationMatteVersion>65536</semanticSegmentationMatte:SemanticSegmentationMatteVersion>
      </rdf:Description>
   </rdf:RDF>
</x:xmpmeta>"#;
    /// The XMP an iPhone writes into the gain map of a JPEG, verbatim from a
    /// camera file. Beside the HEIF packet it names the auxiliary image's kind
    /// and pixel format, and the kind is a word rather than a number.
    pub(crate) const IPHONE_JPEG_GAIN_MAP_XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 6.0.0">
   <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about=""
            xmlns:apdi="http://ns.apple.com/pixeldatainfo/1.0/"
            xmlns:HDRGainMap="http://ns.apple.com/HDRGainMap/1.0/">
         <apdi:AuxiliaryImageType>urn:com:apple:photo:2020:aux:hdrgainmap</apdi:AuxiliaryImageType>
         <apdi:NativeFormat>1278226488</apdi:NativeFormat>
         <apdi:StoredFormat>1278226488</apdi:StoredFormat>
         <HDRGainMap:HDRGainMapVersion>131072</HDRGainMap:HDRGainMapVersion>
         <HDRGainMap:HDRGainMapHeadroom>2.839412</HDRGainMap:HDRGainMapHeadroom>
      </rdf:Description>
   </rdf:RDF>
</x:xmpmeta>"#;
    /// The XMP of an Ultra HDR gain map, verbatim from a file. Google writes
    /// its parameters as attributes of the description, not as elements.
    pub(crate) const ULTRA_HDR_GAIN_MAP_XMP: &str = r#"<x:xmpmeta
  xmlns:x="adobe:ns:meta/"
  x:xmptk="Adobe XMP Core 5.1.2">
  <rdf:RDF
    xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description
      xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/"
      hdrgm:Version="1.0"
      hdrgm:GainMapMin="0"
      hdrgm:GainMapMax="15.9993"
      hdrgm:Gamma="1"
      hdrgm:OffsetSDR="0.015625"
      hdrgm:OffsetHDR="0.015625"
      hdrgm:HDRCapacityMin="0"
      hdrgm:HDRCapacityMax="15.9993"
      hdrgm:BaseRenditionIsHDR="False"/>
  </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn rendering_xmp_is_kept_and_anything_else_is_not() {
        assert!(xmp_is_rendering_only(IPHONE_GAIN_MAP_XMP.as_bytes()));
        assert!(xmp_is_rendering_only(IPHONE_MATTE_XMP.as_bytes()));
        // Trailing NULs and padding, as an item can carry, do not change that.
        let mut padded = IPHONE_GAIN_MAP_XMP.as_bytes().to_vec();
        padded.extend_from_slice(b"   \n\0\0\0");
        assert!(xmp_is_rendering_only(&padded));

        let with_location = IPHONE_GAIN_MAP_XMP.replace(
            "<HDRGainMap:HDRGainMapVersion>",
            "<exif:GPSLatitude>51,30.4N</exif:GPSLatitude><HDRGainMap:HDRGainMapVersion>",
        );
        let other_namespace = IPHONE_GAIN_MAP_XMP.replace(
            "http://ns.apple.com/HDRGainMap/1.0/",
            "http://ns.adobe.com/exif/1.0/",
        );
        let text_value = IPHONE_GAIN_MAP_XMP.replace("3.500636", "Ben's iPhone");
        let commented = IPHONE_GAIN_MAP_XMP.replace("</rdf:RDF>", "<!-- 51.5 N --></rdf:RDF>");
        let named = IPHONE_GAIN_MAP_XMP.replace("rdf:about=\"\"", "rdf:about=\"uuid:faf5bdd5\"");
        for refused in [&with_location, &other_namespace, &text_value, &commented, &named] {
            assert!(!xmp_is_rendering_only(refused.as_bytes()), "kept: {refused}");
        }
        assert!(!xmp_is_rendering_only(b""));
        assert!(!xmp_is_rendering_only(&[0xFF, 0xFE, 0x3C]));
    }

    /// The gain maps inside JPEGs, from both vendors, are rendering parameters
    /// too, and the same packets with anything personal added are not.
    #[test]
    fn jpeg_gain_map_xmp_is_kept_and_anything_added_to_it_is_not() {
        assert!(xmp_is_rendering_only(IPHONE_JPEG_GAIN_MAP_XMP.as_bytes()));
        assert!(xmp_is_rendering_only(ULTRA_HDR_GAIN_MAP_XMP.as_bytes()));
        // Ultra HDR writes one value per colour channel as a sequence.
        let per_channel = ULTRA_HDR_GAIN_MAP_XMP.replace(
            "/>\n  </rdf:RDF>",
            "><hdrgm:GainMapMax><rdf:Seq><rdf:li>15.9</rdf:li><rdf:li>14.2</rdf:li>\
             <rdf:li>13.0</rdf:li></rdf:Seq></hdrgm:GainMapMax></rdf:Description>\n  </rdf:RDF>",
        );
        assert!(xmp_is_rendering_only(per_channel.as_bytes()), "{per_channel}");

        let apple = IPHONE_JPEG_GAIN_MAP_XMP;
        let google = ULTRA_HDR_GAIN_MAP_XMP;
        let refused = [
            // A different word where the auxiliary image's kind goes.
            apple.replace("aux:hdrgainmap<", "aux:hdrgainmap Ben's iPhone<"),
            // Location written as an attribute, in its own namespace.
            google.replace(
                "hdrgm:Version=\"1.0\"",
                "xmlns:exif=\"http://ns.adobe.com/exif/1.0/\" exif:GPSLatitude=\"51.5\" hdrgm:Version=\"1.0\"",
            ),
            // A prefix nobody declared.
            google.replace("hdrgm:Gamma=", "exif:Gamma="),
            // Words where numbers go, as an attribute and inside a sequence.
            google.replace("hdrgm:Gamma=\"1\"", "hdrgm:Gamma=\"Ben\""),
            per_channel.replace("<rdf:li>14.2</rdf:li>", "<rdf:li>Ben</rdf:li>"),
            // An attribute without a namespace at all.
            google.replace("hdrgm:Gamma=", "Gamma="),
            // A rendering attribute somewhere other than the description.
            per_channel.replace("<rdf:li>15.9", "<rdf:li hdrgm:Gamma=\"1\">15.9"),
        ];
        for packet in &refused {
            assert!(!xmp_is_rendering_only(packet.as_bytes()), "kept: {packet}");
        }
    }

    #[test]
    fn an_empty_xmp_packet_fills_its_item_exactly() {
        for length in [127, 445, 4096] {
            let packet = empty_xmp(length);
            assert_eq!(packet.len(), length);
            let text = std::str::from_utf8(&packet).expect("UTF-8");
            assert!(text.starts_with("<?xpacket begin="));
            assert!(text.ends_with("<?xpacket end=\"w\"?>"));
        }
        assert_eq!(empty_xmp(40), vec![0; 40], "too short for a packet");
    }

    #[test]
    fn the_exif_written_back_keeps_apples_headroom_and_nothing_else() {
        let values = [(41767, 25207), (0, 1)];
        // An Apple-shaped EXIF item to read from: the block this module writes,
        // carrying the two maker note values, is one.
        let source = exif_block(Some(values));
        assert_eq!(apple_headroom(&source), Some(values));

        let mut original = source.clone();
        original.resize(2596, 0x55);
        let written = exif_replacement(&original);
        assert_eq!(written.len(), original.len(), "the item keeps its length");
        assert_eq!(&written[..10], b"\0\0\0\x06Exif\0\0", "laid out as an iPhone lays it out");
        assert_eq!(&written[10..14], b"MM\0*");
        assert_eq!(apple_headroom(&written), Some(values));

        // Without Apple's values the block is the plain one, and an item too
        // short even for that is zeroed.
        let plain = exif_replacement(&[0x55; 200]);
        assert_eq!(apple_headroom(&plain), None);
        assert_eq!(&plain[4..10], b"Exif\0\0");
        assert_eq!(exif_replacement(&[0x55; 20]), vec![0; 20]);
    }

    #[test]
    fn a_version_3_item_entry_is_read_at_its_own_offsets() {
        // infe version 3: four-byte item id, two-byte protection index, type.
        let mut infe = Vec::new();
        infe.extend_from_slice(&be32(22));
        infe.extend_from_slice(b"infe");
        infe.extend_from_slice(&[3, 0, 0, 0]);
        infe.extend_from_slice(&be32(70_000));
        infe.extend_from_slice(&0u16.to_be_bytes());
        infe.extend_from_slice(b"Exif");
        let mut iinf = Vec::new();
        iinf.extend_from_slice(&be32(0));
        iinf.extend_from_slice(b"iinf");
        iinf.extend_from_slice(&[1, 0, 0, 0]);
        iinf.extend_from_slice(&be32(1));
        iinf.extend_from_slice(&infe);
        let n = iinf.len() as u32;
        iinf[0..4].copy_from_slice(&be32(n));
        let items = item_types(&iinf, &Box { at: 0, size: iinf.len(), header: 8 });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 70_000);
        assert_eq!(&items[0].kind, b"Exif");
    }

    #[test]
    fn metadata_stored_where_it_cannot_be_overwritten_is_a_refusal() {
        let (mut heif, exif) = heif_with_exif();
        // Point the EXIF item at a data reference other than this file.
        let iloc = find(&heif, b"iloc").expect("iloc");
        let second_entry = iloc.at + iloc.header + 4 + 2 + 2 + 16;
        heif[second_entry + 4..second_entry + 6].copy_from_slice(&1u16.to_be_bytes());
        assert!(strip_heif(&heif).is_none(), "an EXIF item not in this file was reported as removed");
        assert!(contains(&heif[exif], b"GPS"));
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}
