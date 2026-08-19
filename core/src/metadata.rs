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


/// Build an APP1 payload carrying nothing but the EXIF orientation.
///
/// Strip copies the pixels through untouched, which means the tag saying which
/// way up they go has to survive with them. Left out, a portrait photograph
/// comes back on its side: the pixels are identical and the picture is wrong.
/// The re-encoding modes have no such problem, because they turn the pixels
/// themselves and then need no tag.
///
/// Everything else EXIF carries is left behind. Which way up a picture goes
/// identifies nobody.
pub fn orientation_segment(orientation: u16) -> Vec<u8> {
    let mut p = Vec::with_capacity(32);
    p.extend_from_slice(b"Exif\0\0");
    p.extend_from_slice(b"MM\x00\x2a\x00\x00\x00\x08"); // big endian, first entry at 8
    p.extend_from_slice(&1u16.to_be_bytes()); // one field
    p.extend_from_slice(&0x0112u16.to_be_bytes()); // orientation
    p.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    p.extend_from_slice(&1u32.to_be_bytes());
    // A SHORT sits in the first two bytes of the four byte value field.
    p.extend_from_slice(&orientation.to_be_bytes());
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(&0u32.to_be_bytes()); // no further block
    p
}

/// Remove every metadata container from a JPEG without touching the pixels.
///
/// The entropy-coded scan is copied byte for byte, so the result is
/// pixel-identical to the input. This is the difference between shrinking a file
/// and sanitising one: a photograph being delivered as finished work should lose
/// its location data without being re-encoded.
///
/// Every `APPn` segment is dropped except the ICC colour profile. That covers
/// EXIF and its embedded thumbnail (APP1), XMP (APP1), IPTC (APP13), Multi
/// Picture Format and HDR gain maps (APP2), Apple's rotation block (APP10), and
/// C2PA content credentials (APP11), without needing to recognise any of them:
/// anything not deliberately kept is removed.
///
/// Refuses rather than returning what it managed to copy. A segment length that
/// is wrong by one byte desynchronises the walk, and returning the bytes
/// gathered up to that point produces a headers-only file that is smaller than
/// the original and therefore passes every check downstream — which the caller
/// then writes over the photograph. Apple's decoder resynchronises where this
/// one cannot, so the files that trigger it are files the user can still open.
pub fn strip_jpeg(jpeg: &[u8]) -> Option<Vec<u8>> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut out = Vec::with_capacity(jpeg.len());
    out.extend_from_slice(&jpeg[0..2]); // SOI

    let mut i = 2;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            return None; // the walk has lost the marker boundary
        }
        let marker = jpeg[i + 1];

        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            out.extend_from_slice(&jpeg[i..i + 2]);
            i += 2;
            continue;
        }
        if marker == 0xDA {
            // Start of scan. Copy the entropy-coded data, but stop at the end of
            // image rather than the end of file. Apple writes trailing data past
            // EOI — the Multi Picture Format secondary image and XMP live there —
            // and copying to EOF would carry that metadata straight through.
            //
            // Scanning for FFD9 is safe inside entropy-coded data: a literal FF is
            // stored as FF00, and the only other markers permitted are the restart
            // markers FFD0 through FFD7.
            let mut j = i + 2;
            while j + 1 < jpeg.len() {
                if jpeg[j] == 0xFF && jpeg[j + 1] == 0xD9 {
                    out.extend_from_slice(&jpeg[i..j + 2]);
                    return Some(out);
                }
                j += 1;
            }
            // No end marker anywhere in the scan. The pixels cannot be shown to
            // have survived, so the original is left as it is.
            return None;
        }
        if marker == 0xD9 {
            out.extend_from_slice(&jpeg[i..i + 2]);
            return Some(out);
        }

        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        if len < 2 || i + 2 + len > jpeg.len() {
            return None; // a segment claiming to end past the end of the file
        }
        let payload = &jpeg[i + 4..i + 2 + len];
        let is_app = (0xE0..=0xEF).contains(&marker);
        let is_comment = marker == 0xFE;
        // The profile lives in APP2 and is read from APP2, so it is kept only
        // there. Without the marker test, any segment at all that opened with
        // the tag would survive a strip.
        let is_profile = marker == 0xE2 && payload.starts_with(b"ICC_PROFILE\0");
        let keep = (!is_app && !is_comment) || is_profile;

        if keep {
            out.extend_from_slice(&jpeg[i..i + 2 + len]);
        }
        i += 2 + len;
    }
    // Ran out of file without reaching the scan or the end marker.
    None
}
