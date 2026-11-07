//! HDR gain maps, carried across the re-encode.
//!
//! A photograph from an iPhone is two images in one file. The primary is the
//! standard dynamic range picture; alongside it sits a smaller greyscale gain
//! map saying, per pixel, how far to lift the brightness on a display that can
//! show more than the primary encodes. The two are bound together by the Multi
//! Picture Format index in an APP2 segment.
//!
//! squint re-encodes from decoded pixels, and a decoder hands back only the
//! primary. Left alone, that silently converts every HDR photograph to standard
//! range: the file still opens, still looks correct on an ordinary display, and
//! looks flat on the display it was taken for. That is the same class of failure
//! as dropping the colour profile, so the map is carried across on purpose.
//!
//! Multi Picture Format also carries stereoscopic pairs and oversized
//! thumbnails. Only gain maps are handled here; a secondary image that does not
//! identify itself as one is left alone rather than guessed at.

/// Everything a source file carries alongside its primary image.
pub struct Secondaries {
    /// The secondary images, each a complete JPEG in its own right, in the order
    /// the index lists them.
    pub images: Vec<Vec<u8>>,
    /// The primary's ISO 21496-1 APP2 segment. It marks the file as carrying a
    /// gain map; without it a renderer has no reason to look for one.
    pub iso_segment: Option<Vec<u8>>,
    /// Whether any of them identifies itself as a gain map, as opposed to a
    /// stereo view or an oversized thumbnail.
    pub has_gain_map: bool,
    /// What the index said of every picture, the primary first, in the order
    /// it lists them: the type and flags, and which entries depend on which.
    pub entries: Vec<MpEntry>,
}

/// What a Multi Picture Format index entry says about its picture, apart
/// from where the picture is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MpEntry {
    /// Flags in the top five bits (dependent parent, dependent child,
    /// representative image), the format in the next three, and the type in
    /// the low twenty-four: baseline primary, a large thumbnail, a panorama,
    /// a disparity or multi-angle view, or undefined, which a gain map is.
    pub attribute: u32,
    /// The one-based entry numbers of the pictures this one depends on, or 0.
    pub dependents: [u16; 2],
}

const MPF_TAG: &[u8] = b"MPF\0";
pub(crate) const ISO_TAG: &[u8] = b"urn:iso:std:iso:ts:21496";
pub(crate) const XMP_TAG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Markers identifying a secondary image as a gain map rather than some other
/// Multi Picture Format payload.
///
/// Two vendors, two vocabularies, and matching only one of them was worse than
/// matching neither: a Google photograph's map was removed and then reported as
/// having never been there.
const GAIN_MAP_NEEDLES: [&[u8]; 5] = [
    b"hdrgainmap",   // Apple's auxiliary image type
    b"iso:ts:21496", // ISO 21496-1, which Apple also writes
    b"hdr-gain-map", // Google Ultra HDR, as an XMP namespace
    b"hdrgm:",       // Google Ultra HDR, as property names
    b"GContainer",   // Google's container markup, which carries the map
];

/// Walk the marker segments of a JPEG, yielding `(marker, payload_start, payload)`.
///
/// Unlike the parser in `metadata`, this reports absolute file positions,
/// because Multi Picture Format offsets are measured against the position of its
/// own header within the file.
fn segments(jpeg: &[u8]) -> Vec<(u8, usize, &[u8])> {
    let mut out = Vec::new();
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return out;
    }
    let mut i = 2;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            break;
        }
        let marker = jpeg[i + 1];
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        if len < 2 || i + 2 + len > jpeg.len() {
            break;
        }
        out.push((marker, i + 4, &jpeg[i + 4..i + 2 + len]));
        i += 2 + len;
    }
    out
}

/// Every image the Multi Picture Format index lists: its byte range, and what
/// the entry says about it.
///
/// Entry offsets are measured from the start of the format's own TIFF header,
/// not from the start of the file, and the first image is always recorded as
/// offset zero. Both quirks are in the specification.
fn mpf_entries(jpeg: &[u8]) -> Option<Vec<(usize, usize, MpEntry)>> {
    let (payload_start, payload) = segments(jpeg)
        .into_iter()
        .find(|(m, _, p)| *m == 0xE2 && p.starts_with(MPF_TAG))
        .map(|(_, s, p)| (s, p))?;

    let tiff = &payload[MPF_TAG.len()..];
    let tiff_base = payload_start + MPF_TAG.len();
    if tiff.len() < 8 {
        return None;
    }
    let big = match &tiff[0..2] {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16at = |o: usize| -> Option<u16> {
        let s = [*tiff.get(o)?, *tiff.get(o + 1)?];
        Some(if big { u16::from_be_bytes(s) } else { u16::from_le_bytes(s) })
    };
    let u32at = |o: usize| -> Option<u32> {
        let s = [*tiff.get(o)?, *tiff.get(o + 1)?, *tiff.get(o + 2)?, *tiff.get(o + 3)?];
        Some(if big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
    };

    let ifd = u32at(4)? as usize;
    let count = u16at(ifd)? as usize;
    let mut entries_at = None;
    let mut entries_len = 0usize;
    for e in 0..count {
        let off = ifd + 2 + e * 12;
        if u16at(off)? == 0xB002 {
            entries_len = u32at(off + 4)? as usize;
            entries_at = Some(u32at(off + 8)? as usize);
        }
    }
    let entries_at = entries_at?;

    let mut out = Vec::new();
    for k in 0..entries_len / 16 {
        let e = entries_at + k * 16;
        let attribute = u32at(e)?;
        let size = u32at(e + 4)? as usize;
        let rel = u32at(e + 8)? as usize;
        let dependents = [u16at(e + 12)?, u16at(e + 14)?];
        // The first image is recorded as offset zero and starts at the file
        // head. No other image can start there.
        let start = match (k, rel) {
            (0, 0) => 0,
            (_, 0) => return None,
            _ => tiff_base + rel,
        };
        if start + size > jpeg.len() {
            return None;
        }
        out.push((start, size, MpEntry { attribute, dependents }));
    }
    Some(out)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Lift out everything the file carries alongside its primary image.
///
/// Every secondary is taken, not only a recognised gain map. They are picture
/// data — a second view, a larger thumbnail, the brightness map — and the mode
/// that promises to leave pixels alone has no business choosing between them.
pub fn extract(jpeg: &[u8]) -> Option<Secondaries> {
    let listed = mpf_entries(jpeg)?;
    let images: Vec<Vec<u8>> = listed
        .iter()
        .skip(1)
        .map(|&(start, size, _)| jpeg[start..start + size].to_vec())
        .collect();
    let entries = listed.iter().map(|&(_, _, entry)| entry).collect();
    if images.is_empty() {
        return None;
    }

    let has_gain_map = images
        .iter()
        .any(|img| GAIN_MAP_NEEDLES.iter().any(|n| contains(img, n)));

    let iso_segment = segments(jpeg)
        .into_iter()
        .find(|(m, _, p)| *m == 0xE2 && p.starts_with(ISO_TAG))
        .map(|(_, _, p)| p.to_vec());

    Some(Secondaries { images, iso_segment, has_gain_map, entries })
}

/// Whether a file signals high dynamic range at all.
///
/// Checked on the primary as well as the secondaries, because the marker that
/// says a map exists lives in the primary and outlives an index this parser
/// cannot read.
pub fn signals_hdr(jpeg: &[u8]) -> bool {
    if let Some(found) = extract(jpeg) {
        if found.has_gain_map {
            return true;
        }
    }
    segments(jpeg)
        .into_iter()
        .any(|(m, _, p)| m == 0xE2 && p.starts_with(ISO_TAG))
        || contains(jpeg, b"hdrgm:")
}

/// The gain map's own descriptive segments, which must survive its re-encode.
///
/// The map is unusable without them. They carry the parameters a renderer needs
/// to apply it — the headroom it was authored against, and the transfer applied
/// to its values — so dropping them leaves a greyscale picture nothing can read.
/// EXIF is not among them and goes the way of the primary's.
///
/// XMP goes as `map_xmp` says, and `None` when it says to refuse.
pub fn descriptive_segments(map: &[u8]) -> Option<Vec<(u8, Vec<u8>)>> {
    let mut kept = Vec::new();
    for (m, _, p) in segments(map) {
        if m == 0xE1 && p.starts_with(XMP_TAG) {
            match map_xmp(&p[XMP_TAG.len()..]) {
                MapXmp::Keep => kept.push((m, p.to_vec())),
                MapXmp::Drop => {}
                MapXmp::Refuse => return None,
            }
        } else if m == 0xE2 && p.starts_with(ISO_TAG) {
            kept.push((m, p.to_vec()));
        }
    }
    Some(kept)
}

/// What becomes of a gain map's XMP packet.
pub(crate) enum MapXmp {
    /// It holds nothing but rendering parameters.
    Keep,
    /// It names no rendering parameters, so carries nothing a renderer needs.
    Drop,
    /// It holds rendering parameters and more: removing it would leave the
    /// map unreadable while it was reported as kept, and keeping it would
    /// keep the rest.
    Refuse,
}

pub(crate) fn map_xmp(packet: &[u8]) -> MapXmp {
    if crate::heif::xmp_is_rendering_only(packet) {
        MapXmp::Keep
    } else if crate::heif::RENDERING_NAMESPACES.iter().any(|ns| contains(packet, ns.as_bytes())) {
        MapXmp::Refuse
    } else {
        MapXmp::Drop
    }
}

/// Insert APP segments into a JPEG, after any JFIF and EXIF headers.
///
/// Apple orders these APP0, APP1, then the format index, then the colour
/// profile, and this follows suit, so that a file squint writes is ordered the
/// way the files it reads are.
pub fn insert_segments(jpeg: &[u8], segs: &[(u8, Vec<u8>)]) -> Option<Vec<u8>> {
    if segs.is_empty() {
        return Some(jpeg.to_vec());
    }
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut at = 2;
    while at + 4 <= jpeg.len()
        && jpeg[at] == 0xFF
        && (jpeg[at + 1] == 0xE0 || jpeg[at + 1] == 0xE1)
    {
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        if len < 2 || at + 2 + len > jpeg.len() {
            return None;
        }
        at += 2 + len;
    }

    let extra: usize = segs.iter().map(|(_, p)| p.len() + 4).sum();
    let mut out = Vec::with_capacity(jpeg.len() + extra);
    out.extend_from_slice(&jpeg[..at]);
    for (marker, payload) in segs {
        let len = payload.len() + 2;
        if len > u16::MAX as usize {
            return None;
        }
        out.push(0xFF);
        out.push(*marker);
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(&jpeg[at..]);
    Some(out)
}

/// Everything in the payload before the entry table: the tag, the TIFF header,
/// an index of three fields, and the end-of-index marker.
const MPF_HEADER_LEN: usize = 54;
/// Where the entry table sits, measured from the format's TIFF header.
const MPF_ENTRIES_REL: u32 = 50;
/// Multi Picture Format type code for the baseline primary image.
const MP_TYPE_PRIMARY: u32 = 0x0003_0000;

/// What an index written from nothing says: a baseline primary, then
/// pictures of undefined type, which is what a gain map is listed as.
fn default_entries(images: usize) -> Vec<MpEntry> {
    let entry = |attribute| MpEntry { attribute, dependents: [0, 0] };
    std::iter::once(entry(MP_TYPE_PRIMARY)).chain(std::iter::repeat_n(entry(0), images)).collect()
}

/// Payload size for a file holding `images` pictures in total.
fn mpf_payload_len(images: usize) -> usize {
    MPF_HEADER_LEN + 16 * images
}

/// Build the Multi Picture Format index for a primary and its secondaries.
///
/// `tiff_base` is where the format's TIFF header sits in the finished file,
/// because every offset in the index is measured from there rather than from
/// the start of the file, and the first image is always recorded as zero.
/// `entries` holds one entry for the primary and one for each secondary.
fn mpf_segment(primary_len: usize, secondaries: &[usize], tiff_base: usize, entries: &[MpEntry]) -> Vec<u8> {
    let total = 1 + secondaries.len();
    let mut p = Vec::with_capacity(mpf_payload_len(total));
    p.extend_from_slice(MPF_TAG);
    p.extend_from_slice(b"MM\x00\x2a\x00\x00\x00\x08"); // big endian, index at 8
    p.extend_from_slice(&3u16.to_be_bytes()); // three fields follow

    let mut field = |tag: u16, typ: u16, count: u32, value: [u8; 4]| {
        p.extend_from_slice(&tag.to_be_bytes());
        p.extend_from_slice(&typ.to_be_bytes());
        p.extend_from_slice(&count.to_be_bytes());
        p.extend_from_slice(&value);
    };
    field(0xB000, 7, 4, *b"0100"); // version
    field(0xB001, 4, 1, (total as u32).to_be_bytes()); // how many images
    field(0xB002, 7, 16 * total as u32, MPF_ENTRIES_REL.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes()); // no further index
    debug_assert_eq!(p.len(), MPF_HEADER_LEN);

    let mut entry = |listed: MpEntry, size: u32, offset: u32| {
        p.extend_from_slice(&listed.attribute.to_be_bytes());
        p.extend_from_slice(&size.to_be_bytes());
        p.extend_from_slice(&offset.to_be_bytes());
        p.extend_from_slice(&listed.dependents[0].to_be_bytes());
        p.extend_from_slice(&listed.dependents[1].to_be_bytes());
    };
    entry(entries[0], primary_len as u32, 0);
    let mut at = primary_len;
    for (&size, &listed) in secondaries.iter().zip(&entries[1..]) {
        entry(listed, size as u32, (at - tiff_base) as u32);
        at += size;
    }
    debug_assert_eq!(p.len(), mpf_payload_len(total));
    p
}

/// Join a primary image and a gain map into one Multi Picture Format file.
///
/// The index has to record the finished size of the primary and the position of
/// the map, and the index itself sits inside the primary and changes its size.
/// The knot is cut by knowing the index is a fixed 86 bytes: the primary's final
/// length can be computed before a single byte is written.
pub fn attach(primary: &[u8], images: &[Vec<u8>], iso_segment: Option<&[u8]>) -> Option<Vec<u8>> {
    attach_listed(primary, images, &default_entries(images.len()), iso_segment)
}

/// `attach`, writing each picture's index entry as `entries` gives it: the
/// primary's, then each secondary's. A source's stereo views and thumbnails
/// keep the types it listed them under, which a reader goes by. `entries` of
/// any other length than the pictures is a refusal.
pub fn attach_listed(primary: &[u8], images: &[Vec<u8>], entries: &[MpEntry], iso_segment: Option<&[u8]>) -> Option<Vec<u8>> {
    if entries.len() != 1 + images.len() {
        return None;
    }
    if images.is_empty() {
        return Some(primary.to_vec());
    }
    let payload_len = mpf_payload_len(1 + images.len());
    // A placeholder of the right length, filled in below once positions are known.
    let mut extra: Vec<(u8, Vec<u8>)> = vec![(0xE2, vec![0u8; payload_len])];
    if let Some(iso) = iso_segment {
        extra.push((0xE2, iso.to_vec()));
    }

    let mut out = insert_segments(primary, &extra)?;
    let mpf_payload_start = segments(&out)
        .into_iter()
        .find(|(m, _, p)| *m == 0xE2 && p.len() == payload_len && p.iter().all(|b| *b == 0))
        .map(|(_, s, _)| s)?;

    let tiff_base = mpf_payload_start + MPF_TAG.len();
    let sizes: Vec<usize> = images.iter().map(Vec::len).collect();
    let segment = mpf_segment(out.len(), &sizes, tiff_base, entries);
    out[mpf_payload_start..mpf_payload_start + payload_len].copy_from_slice(&segment);
    for image in images {
        out.extend_from_slice(image);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte ranges of the images listed in the Multi Picture Format index.
    fn mpf_images(jpeg: &[u8]) -> Option<Vec<(usize, usize)>> {
        Some(mpf_entries(jpeg)?.into_iter().map(|(start, size, _)| (start, size)).collect())
    }

    /// The smallest thing that parses as a JPEG for the purposes of these tests:
    /// a start marker, an APP0, and an end marker.
    fn stub(extra: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let base = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x08, b'J', b'F', b'I', b'F', 0x00, 0x00, // APP0
            0xFF, 0xD9, // EOI
        ];
        insert_segments(&base, extra).unwrap()
    }

    #[test]
    fn round_trips_a_gain_map_through_attach_and_extract() {
        let map = stub(&[(0xE1, [XMP_TAG, b"hdrgainmap"].concat())]);
        let primary = stub(&[]);
        let iso = [ISO_TAG, b":-1\0"].concat();

        let joined = attach(&primary, std::slice::from_ref(&map), Some(&iso)).unwrap();
        let back = extract(&joined).expect("a secondary should be found");

        assert_eq!(back.images, vec![map], "the map must come back byte for byte");
        assert_eq!(back.iso_segment.unwrap(), iso);
        assert!(back.has_gain_map);
    }

    #[test]
    fn records_the_map_where_it_actually_lands() {
        let map = stub(&[(0xE1, [XMP_TAG, b"hdrgainmap"].concat())]);
        let joined = attach(&stub(&[]), std::slice::from_ref(&map), None).unwrap();
        let images = mpf_images(&joined).unwrap();

        assert_eq!(images.len(), 2);
        let (start, size) = images[1];
        assert_eq!(size, map.len());
        assert_eq!(&joined[start..start + size], &map[..]);
        // The primary's recorded length must stop exactly where the map begins.
        assert_eq!(images[0].1, start);
    }

    /// Multi Picture Format also carries stereo pairs and large thumbnails.
    /// They are carried across like anything else, but they are not HDR.
    #[test]
    fn a_secondary_that_is_not_a_gain_map_is_carried_but_not_called_hdr() {
        let other = stub(&[(0xE1, b"something else entirely".to_vec())]);
        let joined = attach(&stub(&[]), std::slice::from_ref(&other), None).unwrap();
        let back = extract(&joined).expect("the secondary is still carried");

        assert_eq!(back.images, vec![other]);
        assert!(!back.has_gain_map, "a stereo view is not a gain map");
        assert!(!signals_hdr(&joined));
    }

    #[test]
    fn a_google_gain_map_is_recognised_too() {
        let map = stub(&[(0xE1, [XMP_TAG, b"hdrgm:Version=\"1.0\""].concat())]);
        let joined = attach(&stub(&[]), &[map], None).unwrap();

        assert!(extract(&joined).unwrap().has_gain_map, "Ultra HDR went unrecognised");
        assert!(signals_hdr(&joined));
    }

    /// A file listing more than two pictures used to come back with the extras
    /// deleted and the result reported as fully preserved.
    #[test]
    fn every_secondary_survives_the_round_trip() {
        let map = stub(&[(0xE1, [XMP_TAG, b"hdrgainmap"].concat())]);
        let thumb = stub(&[(0xE1, b"a larger thumbnail".to_vec())]);
        let carried = vec![map, thumb];

        let joined = attach(&stub(&[]), &carried, None).unwrap();
        let back = extract(&joined).expect("secondaries should be found");

        assert_eq!(back.images, carried, "one of the pictures was dropped");
        assert_eq!(mpf_images(&joined).unwrap().len(), 3);
        assert!(back.has_gain_map);
    }

    /// Only the first picture is recorded at offset zero. A later entry that
    /// claims it would hand the primary back as a secondary, with every
    /// segment the primary's own strip had just removed.
    #[test]
    fn a_later_entry_cannot_point_back_at_the_primary() {
        let map = stub(&[(0xE1, [XMP_TAG, b"hdrgainmap"].concat())]);
        let mut joined = attach(&stub(&[]), &[map], None).unwrap();
        let primary_len = mpf_images(&joined).unwrap()[0].1 as u32;
        let tiff_base = segments(&joined)
            .into_iter()
            .find(|(m, _, p)| *m == 0xE2 && p.starts_with(MPF_TAG))
            .map(|(_, s, _)| s + MPF_TAG.len())
            .unwrap();
        let second = tiff_base + MPF_ENTRIES_REL as usize + 16;
        joined[second + 4..second + 8].copy_from_slice(&primary_len.to_be_bytes());
        joined[second + 8..second + 12].copy_from_slice(&0u32.to_be_bytes());

        assert!(extract(&joined).is_none(), "the primary came back as a secondary");
    }

    #[test]
    fn keeps_the_maps_parameters_and_drops_its_exif() {
        let map = stub(&[
            (0xE1, [XMP_TAG, crate::heif::tests::IPHONE_JPEG_GAIN_MAP_XMP.as_bytes()].concat()),
            (0xE1, b"Exif\0\0camera identity".to_vec()),
            (0xE2, [ISO_TAG, b":-1\0"].concat()),
        ]);
        let kept = descriptive_segments(&map).expect("Apple's parameters are kept");

        assert_eq!(kept.len(), 2, "XMP and the ISO marker, and nothing else");
        assert!(kept.iter().all(|(_, p)| !contains(p, b"camera identity")));
    }

    #[test]
    fn finds_no_gain_map_in_an_ordinary_jpeg() {
        assert!(extract(&stub(&[])).is_none());
    }
}
