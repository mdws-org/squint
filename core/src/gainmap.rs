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

/// A gain map lifted out of a source file, with everything needed to reattach it.
pub struct GainMap {
    /// The secondary image, a complete JPEG in its own right.
    pub jpeg: Vec<u8>,
    /// The primary's ISO 21496-1 APP2 segment. It marks the file as carrying a
    /// gain map; without it a renderer has no reason to look for one.
    pub iso_segment: Option<Vec<u8>>,
}

const MPF_TAG: &[u8] = b"MPF\0";
const ISO_TAG: &[u8] = b"urn:iso:std:iso:ts:21496";
const XMP_TAG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Markers identifying a secondary image as a gain map rather than some other
/// Multi Picture Format payload.
const GAIN_MAP_NEEDLES: [&[u8]; 2] = [b"hdrgainmap", b"iso:ts:21496"];

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

/// Byte ranges of the images listed in the Multi Picture Format index.
///
/// Entry offsets are measured from the start of the format's own TIFF header,
/// not from the start of the file, and the first image is always recorded as
/// offset zero. Both quirks are in the specification.
fn mpf_images(jpeg: &[u8]) -> Option<Vec<(usize, usize)>> {
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
        let size = u32at(e + 4)? as usize;
        let rel = u32at(e + 8)? as usize;
        // The first image is recorded as offset zero and starts at the file head.
        let start = if rel == 0 { 0 } else { tiff_base + rel };
        if start + size > jpeg.len() {
            return None;
        }
        out.push((start, size));
    }
    Some(out)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Lift the gain map out of a JPEG, if it has one.
pub fn extract(jpeg: &[u8]) -> Option<GainMap> {
    let images = mpf_images(jpeg)?;
    let map = images.iter().skip(1).find_map(|&(start, size)| {
        let candidate = &jpeg[start..start + size];
        let is_gain_map = GAIN_MAP_NEEDLES.iter().any(|n| contains(candidate, n));
        is_gain_map.then(|| candidate.to_vec())
    })?;

    let iso_segment = segments(jpeg)
        .into_iter()
        .find(|(m, _, p)| *m == 0xE2 && p.starts_with(ISO_TAG))
        .map(|(_, _, p)| p.to_vec());

    Some(GainMap { jpeg: map, iso_segment })
}

/// The gain map's own descriptive segments, which must survive its re-encode.
///
/// The map is unusable without them. They carry the parameters a renderer needs
/// to apply it — the headroom it was authored against, and the transfer applied
/// to its values — so dropping them leaves a greyscale picture nothing can read.
/// EXIF is not among them and goes the way of the primary's.
pub fn descriptive_segments(map: &[u8]) -> Vec<(u8, Vec<u8>)> {
    segments(map)
        .into_iter()
        .filter(|(m, _, p)| {
            (*m == 0xE1 && p.starts_with(XMP_TAG)) || (*m == 0xE2 && p.starts_with(ISO_TAG))
        })
        .map(|(m, _, p)| (m, p.to_vec()))
        .collect()
}

/// Insert APP segments into a JPEG, after the JFIF header where there is one.
///
/// Position matters only in that APP0 must stay first. Apple writes the format
/// index ahead of the colour profile and this follows suit, so that a file
/// squint writes is ordered the way the files it reads are.
pub fn insert_segments(jpeg: &[u8], segs: &[(u8, Vec<u8>)]) -> Option<Vec<u8>> {
    if segs.is_empty() {
        return Some(jpeg.to_vec());
    }
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    // Skip APP0 if present, so the inserted segments follow it.
    let mut at = 2;
    if jpeg[at] == 0xFF && jpeg[at + 1] == 0xE0 {
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        at += 2 + len;
    }
    if at > jpeg.len() {
        return None;
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

/// Size of the Multi Picture Format payload this writes: a header, an index of
/// three fields, and two sixteen-byte entries. Apple writes exactly this shape.
const MPF_PAYLOAD_LEN: usize = 86;
/// Where the entry table sits, measured from the format's TIFF header.
const MPF_ENTRIES_REL: u32 = 50;
/// Multi Picture Format type code for the baseline primary image.
const MP_TYPE_PRIMARY: u32 = 0x0003_0000;

/// Build the Multi Picture Format index for a primary and one secondary image.
///
/// `tiff_base` is where the format's TIFF header sits in the finished file,
/// because every offset in the index is measured from there.
fn mpf_segment(primary_len: usize, map_len: usize, tiff_base: usize) -> Vec<u8> {
    let mut p = Vec::with_capacity(MPF_PAYLOAD_LEN);
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
    field(0xB001, 4, 1, 2u32.to_be_bytes()); // two images
    field(0xB002, 7, 32, MPF_ENTRIES_REL.to_be_bytes()); // where the entries are
    p.extend_from_slice(&0u32.to_be_bytes()); // no further index

    let map_rel = (primary_len - tiff_base) as u32;
    for (attr, size, offset) in [
        (MP_TYPE_PRIMARY, primary_len as u32, 0u32),
        (0u32, map_len as u32, map_rel),
    ] {
        p.extend_from_slice(&attr.to_be_bytes());
        p.extend_from_slice(&size.to_be_bytes());
        p.extend_from_slice(&offset.to_be_bytes());
        p.extend_from_slice(&0u16.to_be_bytes()); // no dependent image
        p.extend_from_slice(&0u16.to_be_bytes());
    }
    debug_assert_eq!(p.len(), MPF_PAYLOAD_LEN);
    p
}

/// Join a primary image and a gain map into one Multi Picture Format file.
///
/// The index has to record the finished size of the primary and the position of
/// the map, and the index itself sits inside the primary and changes its size.
/// The knot is cut by knowing the index is a fixed 86 bytes: the primary's final
/// length can be computed before a single byte is written.
pub fn attach(primary: &[u8], map: &[u8], iso_segment: Option<&[u8]>) -> Option<Vec<u8>> {
    // A placeholder of the right length, filled in below once positions are known.
    let mut extra: Vec<(u8, Vec<u8>)> = vec![(0xE2, vec![0u8; MPF_PAYLOAD_LEN])];
    if let Some(iso) = iso_segment {
        extra.push((0xE2, iso.to_vec()));
    }

    let mut out = insert_segments(primary, &extra)?;
    let mpf_payload_start = segments(&out)
        .into_iter()
        .find(|(m, _, p)| *m == 0xE2 && p.len() == MPF_PAYLOAD_LEN && p.iter().all(|b| *b == 0))
        .map(|(_, s, _)| s)?;

    let tiff_base = mpf_payload_start + MPF_TAG.len();
    let segment = mpf_segment(out.len(), map.len(), tiff_base);
    out[mpf_payload_start..mpf_payload_start + MPF_PAYLOAD_LEN].copy_from_slice(&segment);
    out.extend_from_slice(map);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let joined = attach(&primary, &map, Some(&iso)).unwrap();
        let back = extract(&joined).expect("a gain map should be found");

        assert_eq!(back.jpeg, map, "the map must come back byte for byte");
        assert_eq!(back.iso_segment.unwrap(), iso);
    }

    #[test]
    fn records_the_map_where_it_actually_lands() {
        let map = stub(&[(0xE1, [XMP_TAG, b"hdrgainmap"].concat())]);
        let joined = attach(&stub(&[]), &map, None).unwrap();
        let images = mpf_images(&joined).unwrap();

        assert_eq!(images.len(), 2);
        let (start, size) = images[1];
        assert_eq!(size, map.len());
        assert_eq!(&joined[start..start + size], &map[..]);
        // The primary's recorded length must stop exactly where the map begins.
        assert_eq!(images[0].1, start);
    }

    #[test]
    fn leaves_alone_a_secondary_that_is_not_a_gain_map() {
        // Multi Picture Format also carries stereo pairs and large thumbnails.
        let other = stub(&[(0xE1, b"something else entirely".to_vec())]);
        let joined = attach(&stub(&[]), &other, None).unwrap();
        assert!(extract(&joined).is_none());
    }

    #[test]
    fn keeps_the_maps_parameters_and_drops_its_exif() {
        let map = stub(&[
            (0xE1, [XMP_TAG, b"gain map parameters"].concat()),
            (0xE1, b"Exif\0\0camera identity".to_vec()),
            (0xE2, [ISO_TAG, b":-1\0"].concat()),
        ]);
        let kept = descriptive_segments(&map);

        assert_eq!(kept.len(), 2, "XMP and the ISO marker, and nothing else");
        assert!(kept.iter().all(|(_, p)| !contains(p, b"camera identity")));
    }

    #[test]
    fn finds_no_gain_map_in_an_ordinary_jpeg() {
        assert!(extract(&stub(&[])).is_none());
    }
}
