//! JPEG marker parsing, for the metadata squint must preserve or deliberately drop.
//!
//! squint re-encodes from decoded pixels, so every marker in the source is dropped
//! unless it is carried across on purpose. That strips GPS, camera identity, and
//! timestamps by construction, which is the behaviour we want. Two things must
//! survive it: the colour profile, and the orientation.

/// Segments of a JPEG file, as `(marker_byte, payload)`.
///
/// A JPEG is `FFD8` followed by segments of `FF <marker> <len:2> <payload>`.
/// Scan data follows `FFDA` and is not a segment, so parsing stops there.
fn segments(jpeg: &[u8]) -> Vec<(u8, &[u8])> {
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
            i += 2; // standalone, no length
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            break; // start of scan, or end of image
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        if len < 2 || i + 2 + len > jpeg.len() {
            break;
        }
        out.push((marker, &jpeg[i + 4..i + 2 + len]));
        i += 2 + len;
    }
    out
}

/// Reassemble an ICC profile from its APP2 chunks.
///
/// ICC profiles larger than a segment are split across numbered APP2 markers,
/// each carrying the tag `ICC_PROFILE\0` then a 1-based sequence number and a
/// total count. Chunks must be concatenated in sequence order, not file order.
pub fn extract_icc(jpeg: &[u8]) -> Option<Vec<u8>> {
    const TAG: &[u8] = b"ICC_PROFILE\0";
    let mut chunks: Vec<(u8, &[u8])> = Vec::new();
    for (marker, payload) in segments(jpeg) {
        if marker != 0xE2 || payload.len() < TAG.len() + 2 || !payload.starts_with(TAG) {
            continue;
        }
        let seq = payload[TAG.len()];
        chunks.push((seq, &payload[TAG.len() + 2..]));
    }
    if chunks.is_empty() {
        return None;
    }
    chunks.sort_by_key(|(seq, _)| *seq);
    let mut icc = Vec::new();
    for (_, data) in chunks {
        icc.extend_from_slice(data);
    }
    Some(icc)
}

/// Read the EXIF orientation tag from APP1. Returns 1 (normal) when absent.
pub fn extract_orientation(jpeg: &[u8]) -> u16 {
    const EXIF: &[u8] = b"Exif\0\0";
    for (marker, payload) in segments(jpeg) {
        if marker != 0xE1 || !payload.starts_with(EXIF) {
            continue;
        }
        let tiff = &payload[EXIF.len()..];
        if tiff.len() < 8 {
            continue;
        }
        let big = match &tiff[0..2] {
            b"MM" => true,
            b"II" => false,
            _ => continue,
        };
        let u16at = |b: &[u8], o: usize| -> u16 {
            let (x, y) = (b[o], b[o + 1]);
            if big { u16::from_be_bytes([x, y]) } else { u16::from_le_bytes([x, y]) }
        };
        let u32at = |b: &[u8], o: usize| -> u32 {
            let s = [b[o], b[o + 1], b[o + 2], b[o + 3]];
            if big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) }
        };

        let ifd = u32at(tiff, 4) as usize;
        if ifd + 2 > tiff.len() {
            continue;
        }
        let count = u16at(tiff, ifd) as usize;
        for e in 0..count {
            let off = ifd + 2 + e * 12;
            if off + 12 > tiff.len() {
                break;
            }
            if u16at(tiff, off) == 0x0112 {
                // Value is a SHORT stored in the first two bytes of the value field.
                return u16at(tiff, off + 8);
            }
        }
    }
    1
}
