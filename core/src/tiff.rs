//! TIFF, whose directories are the same structure EXIF uses.
//!
//! A TIFF is a header pointing at a chain of image file directories. Each entry
//! is twelve bytes — tag, type, count, and either the value or an offset to it —
//! and the pixels sit elsewhere, addressed by `StripOffsets` or `TileOffsets`.
//!
//! Removing metadata therefore has two halves, and both are needed. The entry
//! must go from the directory, because a reader given a tag that points at
//! nothing rejects the whole file: nulling the Exif pointer alone makes a TIFF
//! macOS will not open, measured before this was written. And the bytes the
//! entry pointed at must be destroyed, because an unreferenced location is still
//! a location to anyone reading the file rather than parsing it.
//!
//! The directory is rewritten shorter, in place, leaving unread bytes after it.
//! Nothing moves, so every offset elsewhere in the file stays correct.

/// How many bytes one value of each TIFF type occupies.
fn type_size(kind: u16) -> usize {
    match kind {
        1 | 2 | 6 | 7 | 129 => 1, // 129 is Exif 3.0's UTF-8 text
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4, // 13 is IFD, an offset written as a LONG
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

/// Tags naming a person, a place, a device or a moment.
///
/// Orientation and the colour profile are deliberately absent: one decides which
/// way up the picture goes, the other how to read its colour. So is a DNG's
/// UniqueCameraModel, which the format requires for choosing the colour profile
/// a raw file is developed with.
///
/// A GeoTIFF's georeferencing goes too. Its tie points, pixel scale or
/// transformation put the picture at coordinates on the Earth, and its keys
/// name the projection and often the site, so a survey or a property map
/// stripped of the rest would still say where it was made. So do the other
/// carriers of the same facts: RPC coefficients, whose offsets are the scene
/// centre in degrees, the older Intergraph matrix, and GDAL's metadata XML.
const REMOVE: [u16; 43] = [
    0x010D, // DocumentName
    0x010E, // ImageDescription
    0x010F, // Make
    0x0110, // Model
    0x0131, // Software
    0x0132, // DateTime
    0x013B, // Artist
    0x013C, // HostComputer
    0x8298, // Copyright
    0x02BC, // XMP
    0x83BB, // IPTC
    0x8649, // Photoshop image resources, which carry IPTC
    0x9003, // DateTimeOriginal, when a writer puts it here
    0x9286, // UserComment, likewise
    0x927C, // MakerNote, likewise
    0x9C9B, // XPTitle
    0x9C9C, // XPComment
    0x9C9D, // XPAuthor
    0x9C9E, // XPKeywords
    0x9C9F, // XPSubject
    0xA420, // ImageUniqueID, when a writer puts it here
    0xA430, // CameraOwnerName, likewise
    0xA431, // BodySerialNumber, likewise
    0xA433, // LensMake, likewise
    0xA434, // LensModel, likewise
    0xA435, // LensSerialNumber, likewise
    0xC615, // LocalizedCameraModel (DNG)
    0xC62F, // CameraSerialNumber (DNG)
    0xC634, // DNGPrivateData, which holds the camera's maker note (DNG)
    0xC65D, // RawDataUniqueID (DNG)
    0xC68B, // OriginalRawFileName (DNG)
    0xC68C, // OriginalRawFileData, the whole original raw file (DNG)
    0x830E, // ModelPixelScale (GeoTIFF)
    0x8482, // ModelTiepoint (GeoTIFF)
    0x85D8, // ModelTransformation (GeoTIFF)
    0x87AF, // GeoKeyDirectory (GeoTIFF)
    0x87B0, // GeoDoubleParams (GeoTIFF)
    0x87B1, // GeoAsciiParams (GeoTIFF)
    0x8480, // IntergraphMatrix, the older form of ModelTransformation
    0xA480, // GDAL_METADATA, which can name the projection and the site
    0xC69C, // RPCCoefficient, a geolocation model whose offsets are the scene centre
    0xC71B, // PreviewDateTime (DNG)
    0xC763, // TimeCodes (DNG), the time of day each frame was recorded
];

/// Directories walked, pages and their Exif, GPS and interoperability
/// directories together, before a file is refused rather than half stripped.
/// A long scan runs to a few hundred pages; the bound is against a chain built
/// to waste time.
const MAX_DIRECTORIES: usize = 4096;

/// Tags whose value is the offset of another directory.
const SUB_IFDS: [u16; 3] = [
    0x8769, // Exif
    0x8825, // GPS
    0xA005, // Interoperability
];

/// SubIFDs, the offsets of further image directories: a DNG's raw image and
/// its previews live there rather than in the page chain. Each is a page in
/// its own right, with pixels to keep and tags to remove.
const SUBIFDS: u16 = 0x014A;

/// Pairs of tags giving where the picture is and how long each piece runs.
const PIXEL_RANGES: [(u16, u16); 3] = [
    (0x0111, 0x0117), // strips
    (0x0144, 0x0145), // tiles
    (0x0201, 0x0202), // an embedded JPEG
];

/// The colour profile, which is data this must protect rather than remove.
const ICC: u16 = 0x8773;

/// XResolution and YResolution: printing density, which no pixel depends on.
/// Samsung's NX cameras from 2014 on point their previews' resolution into
/// IFD0's text or the maker note, so a removed value is wiped through them
/// rather than left in place or the file refused.
const RESOLUTION: [u16; 2] = [0x011A, 0x011B];

/// Compression values whose strips or tiles are JPEG streams: old-style JPEG,
/// JPEG, and the lossy JPEG a DNG may use.
const JPEG_COMPRESSION: [u64; 3] = [6, 7, 34892];

/// The tables a JPEG-compressed page's strips share, written as a JPEG
/// stream of their own.
const JPEG_TABLES: u16 = 0x015B;

/// What becomes of one segment of a JPEG among a TIFF's pixels.
enum Segment {
    Keep,
    /// Replacement bytes, as long as the payload they replace.
    Replace(Vec<u8>),
    /// A gain map whose XMP holds its rendering parameters and more.
    Refuse,
}

/// One segment cleaned as Squint's JPEG Strip cleans a photograph and its
/// gain map, but in place: a JPEG among a TIFF's pixels is addressed by
/// offset and cannot shrink, so what it carries is rewritten at the same
/// length rather than cut out.
///
/// The first image's EXIF becomes a block holding only the orientation the
/// picture is shown by, padded with zeros; left out, a portrait preview would
/// come back on its side. A later image's XMP, which in a gain map holds its
/// rendering parameters, goes as `gainmap::map_xmp` says. JFIF loses the
/// thumbnail it can carry after its header. The colour profile, the ISO gain
/// map segment and Adobe's colour transform stay, as do an APP0 of zeros and
/// a comment of one byte repeated, which some cameras write as filler; every
/// other application segment and comment is blanked, a later image's
/// Multi-Picture index among them. The walk rewrites the first image's index
/// itself (`clean_mpf`). `later` is whether the segment belongs to an image
/// after the stream's first.
fn clean_segment(marker: u8, payload: &[u8], later: bool) -> Segment {
    let blank = || Segment::Replace(vec![0; payload.len()]);
    match marker {
        0xE0 if payload.starts_with(b"JFIF\0") => {
            // The thumbnail's width, height and pixels follow the densities.
            let mut header = payload.to_vec();
            if let Some(thumbnail) = header.get_mut(12..) {
                thumbnail.fill(0);
            }
            Segment::Replace(header)
        }
        0xE0 if payload.iter().all(|&b| b == 0) => Segment::Keep,
        0xE2 if payload.starts_with(b"ICC_PROFILE\0") || payload.starts_with(crate::gainmap::ISO_TAG) => Segment::Keep,
        0xEE if payload.len() == 12 && payload.starts_with(b"Adobe") => Segment::Keep,
        0xE1 if !later && payload.starts_with(b"Exif\0") => {
            let orientation = payload.get(6..).and_then(crate::metadata::orientation_in_tiff).unwrap_or(1);
            let mut block = crate::metadata::orientation_segment(orientation);
            if block.len() > payload.len() {
                return blank();
            }
            block.resize(payload.len(), 0);
            Segment::Replace(block)
        }
        0xE1 if later && payload.starts_with(crate::gainmap::XMP_TAG) => match crate::gainmap::map_xmp(&payload[crate::gainmap::XMP_TAG.len()..]) {
            crate::gainmap::MapXmp::Keep => Segment::Keep,
            crate::gainmap::MapXmp::Drop => blank(),
            crate::gainmap::MapXmp::Refuse => Segment::Refuse,
        },
        0xE0..=0xEF => blank(),
        0xFE if payload.windows(2).any(|w| w[0] != w[1]) => blank(),
        _ => Segment::Keep, // tables, frame and scan headers, filler in a comment
    }
}

/// The first image's Multi-Picture index rewritten in place as Strip writes
/// one for a photograph, holding its version, image count and image entries
/// alone, with the rest of the segment zeroed: unique image IDs, attribute
/// directories, anything after the index. Also where each later image it
/// lists starts, counted from the index's TIFF header; an entry at offset
/// zero names the first image. `None` when the index cannot be read that
/// far, and it is blanked whole: no reader finds an image through it either.
/// So is one with more than one entry table, which no writer makes and which
/// could be repeated to cost far more than the bytes the walk is charged for.
/// Everything rewritten lies inside the segment, so an index whose parts
/// overlap comes out garbled at worst.
fn clean_mpf(payload: &[u8]) -> Option<(Vec<u8>, Vec<usize>)> {
    const TIFF: usize = 4; // after "MPF\0"
    let tiff = &payload[TIFF..];
    let big = match tiff.get(0..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let read = |at: usize, n: usize| tiff.get(at..at.checked_add(n)?);
    let u16at = |at| read(at, 2).map(|s| if big { u16::from_be_bytes([s[0], s[1]]) } else { u16::from_le_bytes([s[0], s[1]]) });
    let u32at = |at| {
        read(at, 4).map(|s| {
            let s = [s[0], s[1], s[2], s[3]];
            if big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) }
        })
    };
    let directory = u32at(4)? as usize;
    let count = u16at(directory)? as usize;
    read(directory, 2 + count * 12 + 4)?;
    let mut index = vec![0; payload.len()];
    index[..TIFF + 8].copy_from_slice(&payload[..TIFF + 8]);
    let mut kept = Vec::new();
    let mut images = Vec::new();
    let mut tabled = false;
    for entry in (0..count).map(|e| directory + 2 + e * 12) {
        match u16at(entry)? {
            0xB000 | 0xB001 => {}
            0xB002 if std::mem::replace(&mut tabled, true) => return None,
            0xB002 => {
                let (size, at) = (u32at(entry + 4)? as usize, u32at(entry + 8)? as usize);
                let table = read(at, size)?;
                index[TIFF + at..TIFF + at + size].copy_from_slice(table);
                for k in 0..size / 16 {
                    match u32at(at + k * 16 + 8)? {
                        0 => {}
                        offset => images.push(offset as usize),
                    }
                }
            }
            _ => continue,
        }
        kept.extend_from_slice(read(entry, 12)?);
    }
    let n = (kept.len() / 12) as u16;
    index[TIFF + directory..TIFF + directory + 2].copy_from_slice(&if big { n.to_be_bytes() } else { n.to_le_bytes() });
    index[TIFF + directory + 2..TIFF + directory + 2 + kept.len()].copy_from_slice(&kept);
    Some((index, images))
}

/// Read the JPEG stream at `start..end` and work out how to clean it: its own
/// segments before the first scan, between scans, and in a second image a
/// Multi-Picture file puts after the first. Such a stream is pixels to this
/// format, so nothing in it may move; each segment is cleaned where it lies
/// (`clean_segment`), and the result lists the rewrites as `(offset, bytes)`.
///
/// A stream that breaks before its first scan is `Scan::Broken`, refused
/// like a file laid out wrongly: a broken segment there could hide the ones
/// after it. One that breaks inside its scans is read as far as it goes:
/// some cameras write their raw sensor data as JPEG scans laid out in their
/// own way. Past the first scan, though, segments are found by reading the
/// scan data, and in such a stream a run of sensor data can read as one, so
/// a rewrite past the first scan must be followed by another scan, as a
/// segment between scans or in a later image's header is, and the stream must
/// reach an end of image, or it too is `Scan::Broken`. Bytes that do not open
/// as a JPEG are not one.
///
/// A later image the first image's Multi-Picture index lists but the walk
/// never reached, past the stream's end or behind bytes that are not padding,
/// would keep its metadata where a reader following the index finds it, so
/// such a stream is `Scan::Broken` too.
///
/// `budget` is what one walk of a file may still read. Strips can name the
/// same bytes many times, and a stream of fill bytes is read one at a time,
/// so a file built to be read for hours runs it out and `None` refuses it.
fn clean_jpeg(bytes: &[u8], start: usize, end: usize, budget: &mut usize) -> Option<Scan> {
    let (mut reached, mut listed) = (Vec::new(), Vec::new());
    let scan = walk_jpeg(bytes, start, end, budget, &mut reached, &mut listed)?;
    if matches!(scan, Scan::Clean(_)) && listed.iter().any(|at| reached.binary_search(at).is_err()) {
        return Some(Scan::Broken);
    }
    Some(scan)
}

/// `clean_jpeg`'s walk, noting where each later image starts in `reached`,
/// in order, and where the first image's index says they start in `listed`.
fn walk_jpeg(
    bytes: &[u8],
    start: usize,
    end: usize,
    budget: &mut usize,
    reached: &mut Vec<usize>,
    listed: &mut Vec<usize>,
) -> Option<Scan> {
    let end = end.min(bytes.len());
    let mut edits = Vec::new();
    if start.saturating_add(2) > end || bytes[start..start + 2] != [0xFF, 0xD8] {
        return Some(Scan::Clean(edits));
    }
    let mut at = start + 2;
    let mut scanning = false;
    let mut later = false;
    // Whether a scan has been read, whether a segment after one is to be
    // rewritten, and whether such a rewrite still waits for the scan that a
    // segment between scans or in a later image's header comes before.
    let mut past_scan = false;
    let mut rewrite_past_scan = false;
    let mut awaiting_scan = false;
    loop {
        if at + 1 >= end {
            return Some(cut_short(scanning, rewrite_past_scan, edits));
        }
        let before = at;
        if bytes[at] != 0xFF {
            return Some(cut_short(scanning, rewrite_past_scan, edits));
        }
        let marker = bytes[at + 1];
        match marker {
            0xFF => at += 1, // a fill byte before the marker
            0x01 | 0xD0..=0xD7 => at += 2,
            0xD9 => {
                if awaiting_scan {
                    return Some(Scan::Broken);
                }
                // A second image, such as a gain map, can follow, after
                // padding.
                let mut next = at + 2;
                while next < end && (bytes[next] == 0x00 || (bytes[next] == 0xFF && bytes.get(next + 1) == Some(&0xFF))) {
                    next += 1;
                }
                if next + 2 <= end && bytes[next..next + 2] == [0xFF, 0xD8] {
                    reached.push(next);
                    at = next + 2;
                    scanning = false;
                    later = true;
                } else {
                    *budget = budget.checked_sub(next - before)?;
                    return Some(Scan::Clean(edits));
                }
            }
            0xD8 => return Some(cut_short(scanning, rewrite_past_scan, edits)), // a start of image inside one
            _ => {
                if at + 4 > end {
                    return Some(cut_short(scanning, rewrite_past_scan, edits));
                }
                let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
                if length < 2 || at + 2 + length > end {
                    return Some(cut_short(scanning, rewrite_past_scan, edits));
                }
                if marker == 0xDA {
                    // The scan header, then entropy-coded data up to the next
                    // marker: a literal FF is stored as FF00, and restarts,
                    // TEM and fill bytes may appear inside it.
                    scanning = true;
                    past_scan = true;
                    awaiting_scan = false;
                    let mut next = at + 2 + length;
                    while next + 1 < end
                        && (bytes[next] != 0xFF || matches!(bytes[next + 1], 0x00 | 0x01 | 0xFF | 0xD0..=0xD7))
                    {
                        next += 1;
                    }
                    at = next;
                } else {
                    let payload = &bytes[at + 4..at + 2 + length];
                    let segment = if marker == 0xE2 && !later && payload.starts_with(b"MPF\0") {
                        match clean_mpf(payload) {
                            Some((index, images)) => {
                                // Offsets count from the index's TIFF header.
                                listed.extend(images.into_iter().map(|offset| at + 8 + offset));
                                Segment::Replace(index)
                            }
                            None => Segment::Replace(vec![0; payload.len()]),
                        }
                    } else {
                        clean_segment(marker, payload, later)
                    };
                    match segment {
                        Segment::Keep => {}
                        Segment::Replace(new) => {
                            if new != payload {
                                edits.push((at + 4, new));
                                rewrite_past_scan |= past_scan;
                                awaiting_scan |= past_scan;
                            }
                        }
                        Segment::Refuse => return Some(Scan::Carries),
                    }
                    at += 2 + length;
                }
            }
        }
        *budget = budget.checked_sub(at - before)?;
    }
}

/// What a walk that stops short of an end of image found: what it read, if it
/// stopped inside a scan with no rewrite past the first scan resting on that
/// reading, and otherwise a break.
fn cut_short(scanning: bool, rewrite_past_scan: bool, edits: Vec<(usize, Vec<u8>)>) -> Scan {
    if scanning && !rewrite_past_scan {
        Scan::Clean(edits)
    } else {
        Scan::Broken
    }
}

/// What a JPEG walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scan {
    /// Read to its end, or broken inside its scans, past which a raw file's
    /// own layout is not followed: the rewrites that clean it, if any.
    Clean(Vec<(usize, Vec<u8>)>),
    /// A gain map whose XMP holds more than its rendering parameters.
    Carries,
    /// A break before the first scan, which could hide segments behind it.
    Broken,
}

/// Whether these bytes are a TIFF this can read.
///
/// BigTIFF announces itself with 43 rather than 42 and lays its directories out
/// differently, so it is not claimed here.
pub fn is_tiff(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && (&bytes[0..4] == b"II\x2A\x00" || &bytes[0..4] == b"MM\x00\x2A")
}

struct Reader<'a> {
    bytes: &'a [u8],
    big_endian: bool,
}

impl<'a> Reader<'a> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.bytes.get(at..at + 2)?;
        Some(if self.big_endian {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.bytes.get(at..at + 4)?;
        Some(if self.big_endian {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        })
    }

    /// The entries of the directory at `at`, as `(position, tag, type, count)`.
    fn entries(&self, at: usize) -> Vec<(usize, u16, u16, u32)> {
        let Some(count) = self.u16(at) else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..count as usize {
            let entry = at + 2 + i * 12;
            let (Some(tag), Some(kind), Some(n)) =
                (self.u16(entry), self.u16(entry + 2), self.u32(entry + 4))
            else {
                break;
            };
            out.push((entry, tag, kind, n));
        }
        out
    }

    /// Where an entry's data lives, when it does not fit in the entry itself.
    fn data_at(&self, entry: usize, kind: u16, count: u32) -> Option<(usize, usize)> {
        let size = type_size(kind).checked_mul(count as usize)?;
        if size <= 4 {
            return None;
        }
        let at = self.u32(entry + 8)? as usize;
        (at.checked_add(size)? <= self.bytes.len()).then_some((at, size))
    }

    /// The numbers an entry holds, whether inline or elsewhere.
    ///
    /// The count is the file's own claim and is bounded here by what the file
    /// can actually hold: an inline value has four bytes, and data elsewhere
    /// has whatever lies between its offset and the end of the file. A count
    /// of four billion on a forty-byte file otherwise turns into four billion
    /// failed reads, which is a hang from a hostile file rather than a refusal.
    fn numbers(&self, entry: usize, kind: u16, count: u32) -> Vec<u64> {
        let width = type_size(kind);
        if width == 0 || count == 0 {
            return Vec::new();
        }
        let (at, room) = match self.data_at(entry, kind, count) {
            Some((at, size)) => (at, size),
            None => (entry + 8, 4),
        };
        let readable = self.bytes.len().saturating_sub(at).min(room) / width;
        (0..(count as usize).min(readable))
            .filter_map(|i| match width {
                2 => self.u16(at + i * 2).map(u64::from),
                4 => self.u32(at + i * 4).map(u64::from),
                _ => None,
            })
            .collect()
    }
}

/// The directories a SubIFDs entry lists. `None` when they cannot be walked
/// safely: an entry of a type that is not an offset (LONG or IFD), or a list
/// that runs past the end of the file, cannot be read, so the directories it
/// names would keep their tags while the file was reported clean. `budget` is
/// what one walk may still read across all its SubIFDs entries, so a
/// directory repeating the tag cannot make the walk do unbounded work.
fn sub_ifds(reader: &Reader, entry: usize, kind: u16, count: u32, budget: &mut usize) -> Option<Vec<usize>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if !matches!(kind, 4 | 13) || count as usize > *budget {
        return None;
    }
    *budget -= count as usize;
    let offsets = reader.numbers(entry, kind, count);
    if offsets.len() != count as usize {
        return None;
    }
    Some(offsets.into_iter().map(|v| v as usize).filter(|&v| v != 0).collect())
}

/// Whether `start..end` overlaps a range in `ranges`, sorted and disjoint as
/// `merged` leaves them.
fn overlaps(ranges: &[(usize, usize)], start: usize, end: usize) -> bool {
    let i = ranges.partition_point(|&(_, e)| e <= start);
    i < ranges.len() && ranges[i].0 < end
}

/// What a page keeps, as two sets of sorted, disjoint ranges: everything
/// that must not be zeroed by an overrun, and the part of it a removed value
/// may never be wiped through, which is all of it but the resolutions.
struct Kept {
    all: Vec<(usize, usize)>,
    strict: Vec<(usize, usize)>,
    /// Segments of JPEGs among the pixels to rewrite in place, by offset,
    /// each the length of the segment it replaces: the only bytes inside
    /// `strict` that a strip changes.
    edits: std::collections::BTreeMap<usize, Vec<u8>>,
    /// Pages whose next link names no page and is written back as zero.
    cut: std::collections::HashSet<usize>,
}

/// Where a removed entry's data lies, as `(start, value end, declared end)`;
/// `Some(None)` when the value sits in the entry itself.
///
/// `None` when the value cannot be wiped, so the file is refused rather than
/// reported clean with the value still in it: its type is not one TIFF
/// defines, so where it lies is unknown, or part of it lies in the pixels,
/// the colour profile or data a kept entry other than a resolution points
/// at, none of which may change. A value running past the end of the file is
/// wiped to the end.
///
/// Text ends at its first terminator. Writers' counts often claim bytes past
/// it, and on real files those run into the pixels; they are not the value,
/// so they are left alone and the rest is wiped. Any other value is its whole
/// declared length.
fn removable(reader: &Reader, kept: &Kept, entry: usize, kind: u16, count: u32) -> Option<Option<(usize, usize, usize)>> {
    if type_size(kind) == 0 && count > 0 {
        return None;
    }
    let size = type_size(kind).checked_mul(count as usize)?;
    if size <= 4 {
        return Some(None);
    }
    let at = reader.u32(entry + 8)? as usize;
    let end = at.saturating_add(size).min(reader.bytes.len());
    let Some(data) = reader.bytes.get(at..end) else {
        return Some(None); // starts past the end: nothing of it is in the file
    };
    let value_end = match kind {
        2 | 129 => data.iter().position(|&b| b == 0).map_or(end, |n| at + n),
        _ => end,
    };
    if value_end > at && overlaps(&kept.strict, at, value_end) {
        return None;
    }
    Some(Some((at, value_end, end)))
}

/// Read the JPEG stream at `start..end` once and add its rewrites to
/// `edits`, noting it in `rewritten` when it has any. Refused as a preview
/// when its gain map's XMP holds more than its rendering parameters; as a
/// layout when `clean_jpeg` finds it broken or the walk's budget runs out.
fn check_jpeg(
    bytes: &[u8],
    (start, end): (usize, usize),
    scanned: &mut std::collections::HashSet<(usize, usize)>,
    budget: &mut usize,
    edits: &mut std::collections::BTreeMap<usize, Vec<u8>>,
    rewritten: &mut Vec<(usize, usize)>,
) -> Result<(), Refused> {
    if !scanned.insert((start, end)) {
        return Ok(());
    }
    match clean_jpeg(bytes, start, end, budget) {
        Some(Scan::Clean(found)) => {
            if !found.is_empty() {
                rewritten.push((start, end));
            }
            edits.extend(found);
            Ok(())
        }
        Some(Scan::Carries) => Err(Refused::Preview),
        Some(Scan::Broken) | None => Err(Refused::Layout),
    }
}

/// `ranges`, sorted and disjoint, with every edit's bytes taken out. An edit
/// lies inside the stream it was read from, so inside one of the ranges.
fn without_edits(ranges: &[(usize, usize)], edits: &std::collections::BTreeMap<usize, Vec<u8>>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &(start, end) in ranges {
        let mut cursor = start;
        for (&at, new) in edits.range(start..end) {
            if cursor < at {
                out.push((cursor, at));
            }
            cursor = cursor.max(at + new.len());
        }
        if cursor < end {
            out.push((cursor, end));
        }
    }
    out
}

/// Every region the picture occupies, gathered before anything is written.
///
/// A tag's declared size cannot be trusted to stay inside the metadata. On a
/// real photograph the Exif directory sits at offset 8 and one of its entries
/// reaches past 2606, which is where the first image strip begins; zeroing on
/// the strength of the declaration alone destroyed the picture.
///
/// `Refused::Layout` when the directories run past `MAX_DIRECTORIES`: a page
/// left out of this walk has no protection, so stripping it could zero its
/// pixels. `Refused::Preview` when a gain map among the pixels carries XMP
/// beyond its rendering parameters. The metadata segments of the JPEGs among
/// the pixels are collected as rewrites (`Kept::edits`).
fn protected(reader: &Reader, first: usize) -> Result<Kept, Refused> {
    // The header, which names the first directory and the byte order.
    let mut out = vec![(0, 8)];
    // The resolutions, kept apart: an overrun must not zero them, but a
    // removed value lying under one is wiped through it.
    let mut resolutions = Vec::new();
    // Each directory with whether it is a page. A page's kept entries point
    // at data the file still needs; an Exif, GPS or interoperability directory
    // is removed whole, so nothing in it is kept. One directory reached in
    // both roles would be stripped as a page and then wiped as metadata, so
    // such a file is refused, unless the page role comes only from a next
    // link evaluated after the directory was walked as metadata, which is
    // then cut.
    let mut seen: std::collections::HashMap<usize, bool> = std::collections::HashMap::new();
    let mut queued = std::collections::HashSet::new();
    let mut queue = vec![(first, true)];
    let mut budget = MAX_DIRECTORIES;
    // Where each directory lies. Directories never overlap in a real file,
    // and one that does would be rewritten from bytes another rewrite moved.
    let mut extents = Vec::new();
    // JPEG streams already read, and what reading the rest may still cost:
    // twice the file, which streams that do not overlap never come near.
    let mut scanned = std::collections::HashSet::new();
    let mut jpeg_budget = reader.bytes.len().saturating_mul(2);
    // Rewrites of the metadata segments those streams carry, the streams that
    // carry them, and every stream read, which `out` leaves out.
    let mut edits = std::collections::BTreeMap::new();
    let mut rewritten = Vec::new();
    let mut streams = Vec::new();
    // Every entry and every strip is at least two bytes of the file unless
    // directories share them, so a walk reading far more than the file holds
    // is a file built to exhaust time or memory.
    let mut entry_budget = reader.bytes.len() / 12 + 1;
    let mut pieces = reader.bytes.len() / 2 + 3 * MAX_DIRECTORIES;
    // Next links, `(page, link)`, followed only once every directory reached
    // otherwise has been walked. A link to an Exif, GPS or interoperability
    // directory names metadata, not a page, as the Blackmagic Pocket Cinema
    // Camera 6K Pro links its first page to its Exif directory: it is cut,
    // and the directory is wiped as the metadata it is.
    let mut chained = Vec::new();
    let mut cut = std::collections::HashSet::new();
    // The metadata directories that cut links pointed at.
    let mut cut_to = std::collections::HashSet::new();

    loop {
        let (at, is_page) = match queue.pop() {
            Some(directory) => directory,
            None => match chained.pop() {
                Some((page, link)) => {
                    if seen.get(&link) == Some(&false) {
                        cut.insert(page);
                        cut_to.insert(link);
                    } else if queued.insert((link, true)) {
                        queue.push((link, true));
                    }
                    continue;
                }
                None => break,
            },
        };
        if at == 0 {
            continue;
        }
        match seen.get(&at) {
            Some(&role) if role == is_page => continue,
            Some(_) => return Err(Refused::Layout),
            None => {}
        }
        if seen.len() >= MAX_DIRECTORIES {
            return Err(Refused::Layout);
        }
        seen.insert(at, is_page);
        let entries = reader.entries(at);
        entry_budget = entry_budget.checked_sub(entries.len()).ok_or(Refused::Layout)?;
        extents.push((at, at + 2 + entries.len() * 12 + 4));
        let find = |want: u16| entries.iter().find(|(_, tag, _, _)| *tag == want).copied();
        // A tag naming pixels or tables twice would have its second entry go
        // unread. TIFF allows each tag once.
        let twice = |want: u16| entries.iter().filter(|(_, tag, _, _)| *tag == want).nth(1).is_some();
        if PIXEL_RANGES.iter().flat_map(|&(offsets, lengths)| [offsets, lengths]).chain([JPEG_TABLES]).any(twice) {
            return Err(Refused::Layout);
        }
        let compression = find(0x0103).and_then(|(e, _, k, n)| reader.numbers(e, k, n).first().copied());

        for (offsets_tag, lengths_tag) in PIXEL_RANGES {
            let (offsets_entry, lengths_entry) = (find(offsets_tag), find(lengths_tag));
            let (Some((oe, _, ok, on)), Some((le, _, lk, ln))) = (offsets_entry, lengths_entry) else {
                // Offsets without lengths name pieces that can be neither
                // protected nor read. Lengths without offsets name nothing:
                // Sony's SRF writes them for raw data it locates elsewhere.
                if offsets_entry.is_some() {
                    return Err(Refused::Layout);
                }
                continue;
            };
            let offsets = reader.numbers(oe, ok, on);
            let lengths = reader.numbers(le, lk, ln);
            // As many of each, and every one readable (`numbers` reads SHORT
            // and LONG widths only): a piece left out would have no
            // protection.
            if on != ln || offsets.len() != on as usize || lengths.len() != ln as usize {
                return Err(Refused::Layout);
            }
            pieces = pieces.checked_sub(offsets.len()).ok_or(Refused::Layout)?;
            // An embedded JPEG, or strips and tiles that are each one.
            let jpeg = offsets_tag == 0x0201 || compression.is_some_and(|c| JPEG_COMPRESSION.contains(&c));
            for (o, l) in offsets.iter().zip(lengths.iter()) {
                let (start, end) = (*o as usize, (*o + *l) as usize);
                if jpeg {
                    check_jpeg(reader.bytes, (start, end), &mut scanned, &mut jpeg_budget, &mut edits, &mut rewritten)?;
                    streams.push((start, end));
                } else {
                    out.push((start, end));
                }
            }
        }
        if let Some((e, _, k, n)) = find(JPEG_TABLES) {
            if let Some((start, size)) = reader.data_at(e, k, n) {
                check_jpeg(reader.bytes, (start, start + size), &mut scanned, &mut jpeg_budget, &mut edits, &mut rewritten)?;
                streams.push((start, start + size));
            }
        }

        for (entry, tag, kind, count) in &entries {
            // On a page, every entry that survives the strip: strip offsets
            // and counts, the colour profile, a colour map, tile tables. An
            // overrun from a removed entry must not zero what a kept one
            // points at.
            let kept = is_page && !REMOVE.contains(tag) && !SUB_IFDS.contains(tag);
            if kept {
                if let Some((at, size)) = reader.data_at(*entry, *kind, *count) {
                    if RESOLUTION.contains(tag) {
                        resolutions.push((at, at + size));
                    } else if *tag != JPEG_TABLES {
                        // The tables are kept as a stream, above.
                        out.push((at, at + size));
                    }
                }
            }
            if SUB_IFDS.contains(tag) {
                if let Some(sub) = reader.u32(entry + 8) {
                    if queued.insert((sub as usize, false)) {
                        queue.push((sub as usize, false));
                    }
                }
            }
            if *tag == SUBIFDS {
                // Sub-images hang off pages. One listed by a directory Strip
                // removes would never be walked, so its tags would stay.
                if !is_page {
                    return Err(Refused::Layout);
                }
                for sub in sub_ifds(reader, *entry, *kind, *count, &mut budget).ok_or(Refused::Layout)? {
                    if queued.insert((sub, true)) {
                        queue.push((sub, true));
                    }
                }
            }
        }

        // Only pages form a chain. An Exif directory's next link, which some
        // cameras point at the thumbnail page, is not followed.
        if is_page {
            if let Some(next) = reader.u32(at + 2 + entries.len() * 12).filter(|&next| next != 0) {
                chained.push((at, next as usize));
            }
        }
    }
    // A metadata directory a cut link pointed at may itself hold pixels, the
    // page the file also calls metadata, or link on to a page nothing else
    // reaches, which would then never be stripped. Either way the file is
    // refused.
    for &directory in &cut_to {
        let entries = reader.entries(directory);
        let holds_pixels = entries.iter().any(|(_, tag, _, _)| PIXEL_RANGES.iter().any(|&(offsets, _)| offsets == *tag));
        let next = reader.u32(directory + 2 + entries.len() * 12).map_or(0, |next| next as usize);
        if holds_pixels || (next != 0 && seen.get(&next) != Some(&true)) {
            return Err(Refused::Layout);
        }
    }
    extents.sort_unstable();
    if extents.windows(2).any(|w| w[1].0 < w[0].1) {
        return Err(Refused::Layout);
    }
    // A rewritten segment is nothing else the file keeps, which would read it
    // as its own data, and streams with rewrites share no bytes, so no read of
    // one sees another's rewrite as pixels. A stream named twice is one
    // stream. Real files never do this. (A directory lying in a stream is
    // refused where pages are stripped.)
    rewritten.sort_unstable();
    let named: std::collections::HashSet<_> = rewritten.iter().copied().collect();
    let rest = merged(out.iter().chain(&resolutions).copied().chain(streams.iter().copied().filter(|r| !named.contains(r))).collect());
    if rewritten.windows(2).any(|w| w[1].0 < w[0].1) || edits.iter().any(|(&at, new)| overlaps(&rest, at, at + new.len())) {
        return Err(Refused::Layout);
    }
    out.extend(streams);
    let all = merged(out.iter().chain(&resolutions).copied().collect());
    Ok(Kept { all, strict: merged(out), edits, cut })
}

/// The bytes Strip promises never to change in a file it strips: the header,
/// the pixels, the colour profile, and what kept entries other than a
/// resolution point at, less the metadata segments it rewrites inside the
/// JPEGs among the pixels.
/// `None` where `strip_tiff` would refuse before writing. The fuzz target
/// checks a successful strip against it.
#[doc(hidden)]
pub fn unchangeable(bytes: &[u8]) -> Option<Vec<(usize, usize)>> {
    if !is_tiff(bytes) {
        return None;
    }
    let reader = Reader { bytes, big_endian: bytes[0] == b'M' };
    let keep = protected(&reader, reader.u32(4)? as usize).ok()?;
    Some(without_edits(&keep.strict, &keep.edits))
}

/// Ranges sorted by start with overlapping and touching ones joined, so that
/// a lookup is a binary search rather than a scan of every range.
fn merged(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.retain(|(s, e)| s < e);
    ranges.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (s, e) in ranges {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Remove a TIFF's identifying tags, leaving its pixels exactly where they are.
///
/// Returns the result and how many bytes were destroyed. Zero means the file
/// carried none of these tags, which is an outcome rather than a failure.
pub fn strip_tiff(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    strip_tiff_checked(bytes).ok()
}

/// Why a TIFF was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Its structure cannot be stripped safely: directories overlapping the
    /// pixels or each other, values that cannot be wiped, a walk past bound.
    Layout,
    /// A gain map among its pixels, usually in the embedded preview, carries
    /// XMP beyond its rendering parameters, which Strip refuses in a
    /// photograph's gain map too.
    Preview,
}

/// `strip_tiff`, saying why a file was refused.
pub fn strip_tiff_checked(bytes: &[u8]) -> Result<(Vec<u8>, usize), Refused> {
    if !is_tiff(bytes) {
        return Err(Refused::Layout);
    }
    let reader = Reader { bytes, big_endian: bytes[0] == b'M' };
    let first = reader.u32(4).ok_or(Refused::Layout)? as usize;
    let keep = protected(&reader, first)?;
    strip_pages(&reader, first, &keep).ok_or(Refused::Layout)
}

/// Strip every page, given what the pages keep.
fn strip_pages(reader: &Reader, first: usize, keep: &Kept) -> Option<(Vec<u8>, usize)> {
    let bytes = reader.bytes;
    let keep_clear = &keep.all;

    let mut out = bytes.to_vec();
    let mut destroyed = Destroyed::default();
    let mut visited = std::collections::HashSet::new();
    // Exif, GPS and interoperability directories already wiped.
    let mut wiped = std::collections::HashSet::new();
    // Pages still to strip: the chain from the header, less the links
    // `protected` cut, and every directory a page lists in SubIFDs. Each
    // offset is queued once.
    let mut queue = vec![first];
    let mut queued = std::collections::HashSet::from([first]);
    let mut budget = MAX_DIRECTORIES;
    // Each page's directory as it is to be written back: where it lies, the
    // entries it keeps, its link to the next page, and where it used to end.
    // They are written after every wipe, so that a removed value lying over a
    // directory already stripped cannot zero it.
    let mut rewrites = Vec::new();

    // Every page is walked. Stopping early left the later pages' tags in place
    // while the file was reported clean, so running out of bound refuses.
    while let Some(directory) = queue.pop() {
        if directory == 0 || visited.contains(&directory) {
            continue;
        }
        if visited.len() >= MAX_DIRECTORIES {
            return None;
        }
        visited.insert(directory);
        // A directory with no readable entries is malformed. Stopping at it
        // would leave every later page unwalked, so the file is refused.
        let entries = reader.entries(directory);
        if entries.is_empty() {
            return None;
        }
        let next = reader.u32(directory + 2 + entries.len() * 12)?;
        let next = if keep.cut.contains(&directory) { 0 } else { next };
        // A directory lying over the pixels or kept data would have them
        // rewritten as if they were entries. Such a file is malformed.
        if overlaps(keep_clear, directory, directory + 2 + entries.len() * 12 + 4) {
            return None;
        }

        let mut kept: Vec<u8> = Vec::with_capacity(entries.len() * 12);
        for (entry, tag, kind, count) in &entries {
            if *tag == SUBIFDS {
                for sub in sub_ifds(reader, *entry, *kind, *count, &mut budget)? {
                    if queued.insert(sub) {
                        queue.push(sub);
                    }
                }
            }
            let unwanted = REMOVE.contains(tag) || SUB_IFDS.contains(tag);
            if !unwanted {
                kept.extend_from_slice(&bytes[*entry..*entry + 12]);
                continue;
            }

            // Destroy what the entry pointed at, so that dropping the entry
            // does not merely hide the value from a parser.
            if SUB_IFDS.contains(tag) {
                let sub = reader.u32(entry + 8)? as usize;
                wipe_removed_directory(reader, keep, &mut out, &mut destroyed, sub, &mut wiped)?;
            } else if let Some((at, value_end, end)) = removable(reader, keep, *entry, *kind, *count)? {
                // The value goes even through a resolution; what a count
                // claims past it is only wiped where nothing is kept.
                destroyed.wipe_outside(&mut out, &keep.strict, at, value_end);
                destroyed.wipe_outside(&mut out, keep_clear, value_end, end);
            }
        }

        rewrites.push((directory, kept, next, directory + 2 + entries.len() * 12 + 4));
        if queued.insert(next as usize) {
            queue.push(next as usize);
        }
    }

    // Clean the metadata segments of the JPEGs among the pixels where they
    // lie. Nothing else inside the pixels is ever written.
    for (&at, new) in &keep.edits {
        destroyed.wipe(&mut out, at, at + new.len());
        out[at..at + new.len()].copy_from_slice(new);
    }

    // Write each directory back shorter. What follows it is no longer read,
    // because the count says how many entries there are and the pointer to
    // the next directory now sits earlier.
    let order = if reader.big_endian { u16::to_be_bytes } else { u16::to_le_bytes };
    let order32 = if reader.big_endian { u32::to_be_bytes } else { u32::to_le_bytes };
    for (directory, kept, next, old_end) in rewrites {
        out[directory..directory + 2].copy_from_slice(&order((kept.len() / 12) as u16));
        out[directory + 2..directory + 2 + kept.len()].copy_from_slice(&kept);
        let tail = directory + 2 + kept.len();
        out[tail..tail + 4].copy_from_slice(&order32(next));
        // The slots the dropped entries leave after the new end still hold
        // their bytes, and a value of four bytes or fewer lives in its slot.
        destroyed.wipe(&mut out, tail + 4, old_end);
    }

    Some((out, destroyed.bytes))
}

/// Wipe a removed Exif, GPS or interoperability directory, its values, and
/// every such directory it names in turn: a GPS directory can hang off the
/// Exif one rather than the page. `None` when one cannot be wiped: it starts
/// inside the header, is cut off by the end of the file, lies in kept data,
/// or holds a value `removable` refuses. A pointer of zero names nothing, and
/// a directory starting past the end of the file holds nothing to wipe.
fn wipe_removed_directory(
    reader: &Reader,
    keep: &Kept,
    out: &mut [u8],
    destroyed: &mut Destroyed,
    first: usize,
    wiped: &mut std::collections::HashSet<usize>,
) -> Option<()> {
    let mut queue = vec![first];
    while let Some(at) = queue.pop() {
        if at == 0 || at >= reader.bytes.len() || !wiped.insert(at) {
            continue;
        }
        if at < 8 || wiped.len() > MAX_DIRECTORIES {
            return None;
        }
        let entries = reader.entries(at);
        let end = at + 2 + entries.len() * 12 + 4;
        // The directory holds short values in its own entries, so one lying
        // in kept data, or missing its end, would keep them.
        if end > reader.bytes.len() || overlaps(&keep.strict, at, end) {
            return None;
        }
        for (entry, tag, kind, count) in entries {
            if SUB_IFDS.contains(&tag) {
                queue.push(reader.u32(entry + 8)? as usize);
            }
            // A colour profile names no one and may be the page's own; Strip
            // has always left it.
            if tag == ICC {
                continue;
            }
            if let Some((start, value_end, declared_end)) = removable(reader, keep, entry, kind, count)? {
                destroyed.wipe_outside(out, &keep.strict, start, value_end);
                destroyed.wipe_outside(out, &keep.all, value_end, declared_end);
            }
        }
        destroyed.wipe_outside(out, &keep.strict, at, end);
    }
    Some(())
}

/// The regions zeroed so far, kept disjoint so that the byte count is the
/// number of distinct bytes destroyed. Two entries can name the same bytes,
/// and a sub-directory's own entries lie inside the region its parent entry
/// points at, so summing declared sizes reports more bytes than the file has.
#[derive(Default)]
struct Destroyed {
    /// Start to end, disjoint.
    ranges: std::collections::BTreeMap<usize, usize>,
    bytes: usize,
}

impl Destroyed {
    /// Zero `start..end` except where it overlaps a protected range. A tag's
    /// declared size can reach into the pixels or a kept array; the part that
    /// is metadata goes, the rest stays.
    /// `keep` is sorted and disjoint, as `merged` leaves it.
    fn wipe_outside(&mut self, out: &mut [u8], keep: &[(usize, usize)], start: usize, end: usize) {
        let mut cursor = start;
        let first = keep.partition_point(|&(_, ke)| ke <= start);
        for &(ks, ke) in keep[first..].iter().take_while(|&&(ks, _)| ks < end) {
            if cursor < ks {
                self.wipe(out, cursor, ks);
            }
            cursor = cursor.max(ke);
        }
        if cursor < end {
            self.wipe(out, cursor, end);
        }
    }

    fn wipe(&mut self, out: &mut [u8], start: usize, end: usize) {
        if start >= end || end > out.len() {
            return;
        }
        out[start..end].fill(0);
        let (mut s, mut e) = (start, end);
        let mut already = 0;
        // Ranges that overlap this one: the last starting before it, and every
        // one starting inside it. Each is folded in and removed.
        let overlapping: Vec<(usize, usize)> = self
            .ranges
            .range(..end)
            .rev()
            .take_while(|&(_, &re)| re > start)
            .map(|(&rs, &re)| (rs, re))
            .collect();
        for (rs, re) in overlapping {
            already += re.min(end) - rs.max(start);
            s = s.min(rs);
            e = e.max(re);
            self.ranges.remove(&rs);
        }
        self.ranges.insert(s, e);
        self.bytes += (end - start) - already;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian TIFF with pixels, a colour profile, a Make tag, and an
    /// Exif directory carrying a location. Laid out the way a camera lays one
    /// out: header, pixels, then the directory at the end.
    fn tiff_with_metadata() -> (Vec<u8>, std::ops::Range<usize>) {
        const PIXELS: &[u8] = &[0xAA; 32];
        const MAKE: &[u8] = b"A Camera Company\0";
        const GPS: &[u8] = b"51.5074 N 0.1278 W\0";
        const PROFILE: &[u8] = &[0xCC; 20];

        let mut out = Vec::new();
        out.extend_from_slice(b"II\x2A\x00");
        out.extend_from_slice(&0u32.to_le_bytes()); // directory offset, patched below

        let pixels_at = out.len();
        out.extend_from_slice(PIXELS);
        let make_at = out.len();
        out.extend_from_slice(MAKE);
        let gps_at = out.len();
        out.extend_from_slice(GPS);
        let profile_at = out.len();
        out.extend_from_slice(PROFILE);

        // The Exif directory: one entry, pointing at the location.
        let exif_at = out.len();
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0x9003u16.to_le_bytes()); // DateTimeOriginal
        out.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        out.extend_from_slice(&(GPS.len() as u32).to_le_bytes());
        out.extend_from_slice(&(gps_at as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // no directory after it

        let dir_at = out.len();
        let entries: [(u16, u16, u32, u32); 6] = [
            (0x0111, 4, 1, pixels_at as u32),          // StripOffsets
            (0x0117, 4, 1, PIXELS.len() as u32),       // StripByteCounts
            (0x0112, 3, 1, 1),                         // Orientation, inline
            (0x010F, 2, MAKE.len() as u32, make_at as u32),
            (0x8769, 4, 1, exif_at as u32),            // Exif directory
            (0x8773, 7, PROFILE.len() as u32, profile_at as u32), // ICC
        ];
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // no second directory

        let at = dir_at as u32;
        out[4..8].copy_from_slice(&at.to_le_bytes());
        (out, pixels_at..pixels_at + PIXELS.len())
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// A little-endian TIFF of chained pages. Each page has its own 16 bytes of
    /// pixels and the given extra entries, whose data (longer than four bytes)
    /// is written out of line before the page's directory.
    fn tiff_pages(pages: &[Vec<(u16, u16, Vec<u8>)>]) -> (Vec<u8>, Vec<std::ops::Range<usize>>) {
        let mut out = b"II\x2A\x00\0\0\0\0".to_vec();
        let mut pixels = Vec::new();
        let mut link = 4; // where the offset of the next directory goes
        for (n, extra) in pages.iter().enumerate() {
            let px = out.len();
            out.extend_from_slice(&[0xA0 | (n % 16) as u8; 16]);
            pixels.push(px..px + 16);
            let mut entries = vec![(0x0111u16, 4u16, 1u32, px as u32), (0x0117, 4, 1, 16)];
            for (tag, kind, data) in extra {
                let at = out.len();
                out.extend_from_slice(data);
                entries.push((*tag, *kind, data.len() as u32, at as u32));
            }
            entries.sort_by_key(|e| e.0);
            let dir = out.len();
            out[link..link + 4].copy_from_slice(&(dir as u32).to_le_bytes());
            out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for (tag, kind, count, value) in entries {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&kind.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
                out.extend_from_slice(&value.to_le_bytes());
            }
            link = out.len();
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        (out, pixels)
    }

    /// A cinema camera's DNG records the time of day each frame was shot in
    /// TimeCodes (SMPTE: hours, minutes, seconds, frames), as the Blackmagic
    /// Pocket Cinema Camera 6K Pro writes 13:55:50.03. It names a moment and
    /// goes like the capture date.
    #[test]
    fn dng_time_codes_are_removed() {
        let code = vec![0x03, 0x50, 0x55, 0x13, 0, 0, 0, 0]; // 13:55:50.03, BCD frames first
        let (tiff, pixels) = tiff_pages(&[vec![(0xC763, 1, code.clone())]]);
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, &code[..4]), "the time code stayed");
        let dir = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes([out[dir], out[dir + 1]]) as usize;
        assert!((0..count).all(|e| out[dir + 2 + e * 12..dir + 4 + e * 12] != 0xC763u16.to_le_bytes()), "the TimeCodes entry stayed");
        assert_eq!(&out[pixels[0].clone()], &tiff[pixels[0].clone()]);
    }

    /// The next link of the directory at `at` in a stripped file, wherever the
    /// rewrite left it.
    fn next_link(out: &[u8], at: usize) -> u32 {
        let count = u16::from_le_bytes([out[at], out[at + 1]]) as usize;
        let link = at + 2 + count * 12;
        u32::from_le_bytes(out[link..link + 4].try_into().unwrap())
    }

    /// A page whose next link points at its own Exif directory, as the
    /// Blackmagic Pocket Cinema Camera 6K Pro writes it. The directory is
    /// metadata, so the link is cut rather than the directory stripped as a
    /// page, and the file strips.
    #[test]
    fn a_next_link_to_the_pages_exif_directory_is_cut() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        tiff.extend_from_slice(&[0xAA; 16]); // the pixels, at 8
        let exif = tiff.len();
        tiff.extend_from_slice(&1u16.to_le_bytes());
        for field in [&0x9003u16.to_le_bytes()[..], &2u16.to_le_bytes(), &20u32.to_le_bytes(), &((exif + 18) as u32).to_le_bytes(), &[0; 4]] {
            tiff.extend_from_slice(field);
        }
        tiff.extend_from_slice(b"2026:10:19 12:00:00\0");
        let dir = tiff.len();
        tiff[4..8].copy_from_slice(&(dir as u32).to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        for (tag, kind, count, value) in [(0x0111u16, 4u16, 1u32, 8u32), (0x0117, 4, 1, 16), (0x8769, 4, 1, exif as u32)] {
            for field in [&tag.to_le_bytes()[..], &kind.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()] {
                tiff.extend_from_slice(field);
            }
        }
        tiff.extend_from_slice(&(exif as u32).to_le_bytes()); // the next link
        let (out, _) = strip_tiff(&tiff).expect("the link is cut and the file strips");
        assert!(!contains(&out, b"2026:10:19"), "the capture date stayed");
        assert_eq!(next_link(&out, dir), 0, "the link to the Exif directory stayed");
        assert_eq!(&out[8..24], &tiff[8..24], "the pixels moved");
        // That Exif directory linking on to a page nothing else reaches: cut,
        // the page would never be stripped, so the file is refused.
        let mut onward = tiff.clone();
        let page = onward.len();
        onward.extend_from_slice(&3u16.to_le_bytes());
        for (tag, kind, count, value) in [(0x0111u16, 4u16, 1u32, 8u32), (0x0117, 4, 1, 16), (0x013B, 2, 4, 0x6E6542)] {
            for field in [&tag.to_le_bytes()[..], &kind.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()] {
                onward.extend_from_slice(field);
            }
        }
        onward.extend_from_slice(&[0; 4]);
        onward[exif + 2 + 12..exif + 2 + 16].copy_from_slice(&(page as u32).to_le_bytes());
        assert_eq!(strip_tiff_checked(&onward), Err(Refused::Layout), "a page behind the Exif directory went unstripped");
    }

    /// An Exif directory that a page's next link points at and that names
    /// pixels of its own is a page the file also calls metadata, so the file
    /// is refused rather than the link cut.
    #[test]
    fn a_next_link_to_an_exif_directory_holding_pixels_is_refused() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        tiff.extend_from_slice(&[0xAA; 16]);
        let exif = tiff.len();
        tiff.extend_from_slice(&2u16.to_le_bytes());
        for (tag, kind, count, value) in [(0x0111u16, 4u16, 1u32, 8u32), (0x0117, 4, 1, 16)] {
            for field in [&tag.to_le_bytes()[..], &kind.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()] {
                tiff.extend_from_slice(field);
            }
        }
        tiff.extend_from_slice(&[0; 4]);
        let dir = tiff.len();
        tiff[4..8].copy_from_slice(&(dir as u32).to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        for (tag, kind, count, value) in [(0x0111u16, 4u16, 1u32, 8u32), (0x0117, 4, 1, 16), (0x8769, 4, 1, exif as u32)] {
            for field in [&tag.to_le_bytes()[..], &kind.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()] {
                tiff.extend_from_slice(field);
            }
        }
        tiff.extend_from_slice(&(exif as u32).to_le_bytes());
        assert_eq!(strip_tiff_checked(&tiff), Err(Refused::Layout));
    }

    /// Photoshop keeps IPTC (city, byline) in its image resources, Windows
    /// keeps title, comment, author, keywords and subject in its XP tags, and a
    /// DNG names the camera's serial and the raw file it came from. All of it
    /// sits in the first directory, and all of it survived a Strip.
    #[test]
    fn photoshop_windows_and_dng_identity_tags_are_removed() {
        let page = vec![
            (0x8649, 1, b"8BIM IPTC city Springfield".to_vec()), // Photoshop
            (0x9C9D, 1, b"B\0e\0n\0 \0M\0\0\0".to_vec()),       // XPAuthor, UCS-2
            (0x9C9C, 1, b"a\0t\0 \0h\0o\0m\0e\0\0\0".to_vec()), // XPComment
            (0xC62F, 2, b"SN-48151623\0".to_vec()),            // CameraSerialNumber
            (0xC68B, 2, b"IMG_4242.CR3\0".to_vec()),           // OriginalRawFileName
        ];
        let (tiff, pixels) = tiff_pages(&[page]);
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        for leak in [&b"Springfield"[..], b"B\0e\0n\0", b"h\0o\0m\0e\0", b"SN-48151623", b"IMG_4242"] {
            assert!(!contains(&out, leak), "survived: {:?}", String::from_utf8_lossy(leak));
        }
        assert_eq!(&out[pixels[0].clone()], &tiff[pixels[0].clone()], "the pixels moved");
    }

    /// The Exif identity tags a writer may put in the first directory, and a
    /// DNG's private data and embedded original raw, which carry the camera's
    /// own metadata wholesale.
    #[test]
    fn camera_owner_serials_lens_and_embedded_originals_are_removed() {
        let page = vec![
            (0xA420, 2, b"UID-0011223344\0".to_vec()),          // ImageUniqueID
            (0xA430, 2, b"Owner Ben M\0".to_vec()),             // CameraOwnerName
            (0xA431, 2, b"BODY-48151623\0".to_vec()),           // BodySerialNumber
            (0xA433, 2, b"LensMakerCo\0".to_vec()),             // LensMake
            (0xA434, 2, b"LensModelX 50mm\0".to_vec()),         // LensModel
            (0xA435, 2, b"LENS-99887766\0".to_vec()),           // LensSerialNumber
            (0xC634, 1, b"Adobe\0MakN serial 4242".to_vec()),   // DNGPrivateData
            (0xC68C, 7, b"original CR3 with GPS 33.44N".to_vec()), // OriginalRawFileData
        ];
        let (tiff, pixels) = tiff_pages(&[page]);
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        for leak in [&b"UID-00112"[..], b"Owner Ben", b"BODY-4815", b"LensMakerCo", b"LensModelX",
                     b"LENS-9988", b"serial 4242", b"GPS 33.44"] {
            assert!(!contains(&out, leak), "survived: {:?}", String::from_utf8_lossy(leak));
        }
        assert_eq!(&out[pixels[0].clone()], &tiff[pixels[0].clone()], "the pixels moved");
    }

    /// A directory with no entries ended the walk without reading where the
    /// next one was, so every page after it kept its tags and the file was
    /// reported clean. Such a directory is malformed, and the file is refused.
    #[test]
    fn an_empty_directory_in_the_chain_is_refused_rather_than_ending_the_walk() {
        let p1 = vec![(0x010F, 2, b"Camera One\0".to_vec())];
        let p3 = vec![(0x013B, 2, b"Ben Meadows\0".to_vec())];
        let (mut tiff, _) = tiff_pages(&[p1, p3]);
        let dir1 = u32::from_le_bytes(tiff[4..8].try_into().unwrap()) as usize;
        let count1 = u16::from_le_bytes(tiff[dir1..dir1 + 2].try_into().unwrap()) as usize;
        let link1 = dir1 + 2 + count1 * 12;
        let dir3 = u32::from_le_bytes(tiff[link1..link1 + 4].try_into().unwrap());
        let empty = tiff.len() as u32;
        tiff.extend_from_slice(&0u16.to_le_bytes());
        tiff.extend_from_slice(&dir3.to_le_bytes());
        tiff[link1..link1 + 4].copy_from_slice(&empty.to_le_bytes());

        match strip_tiff(&tiff) {
            None => {}
            Some((out, _)) => assert!(!contains(&out, b"Ben Meadows"), "the page after it kept its artist"),
        }
    }

    /// A GeoTIFF places its picture on the Earth: tie points with a pixel
    /// scale, or a transformation matrix, give coordinates, and the GeoKey
    /// directory and its parameters name the projection, the datum and often
    /// the site. A drone survey or a property map is a location, and Strip
    /// left every one of these in place.
    #[test]
    fn geotiff_georeferencing_is_removed() {
        let doubles = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        // Written as undefined bytes so that each count is a byte count, as
        // the builder declares it; the strip goes by tag, whatever the type.
        let tiepoint = doubles(&[0.0, 0.0, 0.0, -111.987654, 33.456789, 0.0]);
        let transform = doubles(&[0.5, 0.0, 0.0, -111.987654, 0.0, -0.5, 0.0, 33.456789,
                                  0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        let page = vec![
            (0x830E, 7, doubles(&[0.25, 0.25, 0.0])),            // ModelPixelScale
            (0x8482, 7, tiepoint.clone()),                       // ModelTiepoint
            (0x85D8, 7, transform.clone()),                      // ModelTransformation
            (0x87AF, 7, [1u16, 1, 0, 2, 1024, 0, 1, 1, 3072, 0, 1, 32612]
                .iter().flat_map(|k| k.to_le_bytes()).collect()), // GeoKeyDirectory
            (0x87B0, 7, doubles(&[6378137.0, 298.257223563])),    // GeoDoubleParams
            (0x87B1, 2, b"WGS 84 / UTM zone 12N|Heber parcel 2411|\0".to_vec()), // GeoAsciiParams
        ];
        let (tiff, pixels) = tiff_pages(&[page]);
        let (out, destroyed) = strip_tiff(&tiff).expect("a well formed tiff");
        let place = (-111.987654f64).to_le_bytes();
        for leak in [&place[..], &tiepoint[24..40], b"Heber parcel", b"UTM zone 12N",
                     &6378137.0f64.to_le_bytes()[..], &32612u16.to_le_bytes()[..]] {
            assert!(!contains(&out, leak), "survived: {leak:?}");
        }
        assert!(destroyed > 0);
        assert_eq!(&out[pixels[0].clone()], &tiff[pixels[0].clone()], "the pixels moved");
        let reader = Reader { bytes: &out, big_endian: false };
        let first = reader.u32(4).unwrap() as usize;
        let tags: Vec<u16> = reader.entries(first).iter().map(|e| e.1).collect();
        assert_eq!(tags, vec![0x0111, 0x0117], "a georeferencing entry stayed in the directory");
    }

    /// Georeferencing also travels outside the six GeoTIFF tags: a satellite
    /// or drone scene can be placed only by its RPC coefficients, whose offsets
    /// are the scene centre in degrees; old Intergraph files carry the same
    /// matrix as ModelTransformation; and GDAL writes the projection into its
    /// metadata XML. A DNG dates its preview separately from the photograph.
    #[test]
    fn other_georeferencing_and_the_preview_date_are_removed() {
        let doubles = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let page = vec![
            (0x8480, 7, doubles(&[0.5, 0.0, 0.0, -111.123456])),            // IntergraphMatrix
            (0xA480, 2, b"<GDALMetadata><Item name=\"SRS\">UTM zone 12N, site Heber</Item></GDALMetadata>\0".to_vec()),
            (0xC69C, 7, doubles(&[1.0, 2.0, 34.432109, -110.598765])),       // RPCCoefficient
            (0xC71B, 2, b"2026:10:18 21:04:00\0".to_vec()),                  // PreviewDateTime (DNG)
        ];
        let (tiff, pixels) = tiff_pages(&[page]);
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        for leak in [&(-111.123456f64).to_le_bytes()[..], b"site Heber", &34.432109f64.to_le_bytes()[..],
                     b"2026:10:18 21:04"] {
            assert!(!contains(&out, leak), "survived: {leak:?}");
        }
        assert_eq!(&out[pixels[0].clone()], &tiff[pixels[0].clone()], "the pixels moved");
    }

    /// A directory written out: entry count, entries, then no next directory.
    fn directory(entries: &[(u16, u16, u32, u32)]) -> Vec<u8> {
        let mut out = (entries.len() as u16).to_le_bytes().to_vec();
        for (tag, kind, count, value) in entries {
            for field in [&tag.to_le_bytes()[..], &kind.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()] {
                out.extend_from_slice(field);
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out
    }

    fn tags_at(tiff: &[u8], at: usize) -> Vec<u16> {
        Reader { bytes: tiff, big_endian: false }.entries(at).iter().map(|e| e.1).collect()
    }

    /// A DNG keeps its raw image, and often its previews, in sub-directories
    /// listed by SubIFDs (0x014A) rather than in the page chain. They were
    /// never walked, so their own tags stayed: here a camera, a date and the
    /// original file name, in a raw directory whose pixels must not move.
    #[test]
    fn a_dng_raw_sub_directory_is_stripped_and_its_pixels_kept() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let raw = tiff.len();
        tiff.extend_from_slice(&[0xB2; 16]);
        let make = tiff.len();
        tiff.extend_from_slice(b"RawCam Body 7\0");
        let date = tiff.len();
        tiff.extend_from_slice(b"2026:10:18 09:41:00\0");
        let name = tiff.len();
        tiff.extend_from_slice(b"BEN_0042.NEF\0");
        let sub = tiff.len();
        tiff.extend_from_slice(&directory(&[
            (0x0111, 4, 1, raw as u32), (0x0117, 4, 1, 16),
            (0x010F, 2, 14, make as u32), (0x0132, 2, 20, date as u32), (0xC68B, 2, 13, name as u32),
        ]));
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[
            (0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16), (0x014A, 13, 1, sub as u32),
        ]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());

        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        for leak in [&b"RawCam Body"[..], b"2026:10:18", b"BEN_0042"] {
            assert!(!contains(&out, leak), "survived: {:?}", String::from_utf8_lossy(leak));
        }
        assert_eq!(&out[px..px + 16], &tiff[px..px + 16], "the main picture moved");
        assert_eq!(&out[raw..raw + 16], &tiff[raw..raw + 16], "the raw pixels moved");
        assert_eq!(tags_at(&out, first), vec![0x0111, 0x0117, 0x014A], "the parent lost its SubIFDs entry");
        assert_eq!(tags_at(&out, sub), vec![0x0111, 0x0117], "the sub-directory kept a tag");
    }

    /// SubIFDs may list several directories, as an array of offsets stored
    /// out of line. Every one of them is walked.
    #[test]
    fn every_sub_directory_in_a_subifds_array_is_stripped() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let mut subs = Vec::new();
        let mut pixels = Vec::new();
        for n in 0..2u8 {
            let px = tiff.len();
            tiff.extend_from_slice(&[0xC0 | n; 16]);
            pixels.push(px);
            let artist = tiff.len();
            tiff.extend_from_slice(format!("Preview Author {n}\0").as_bytes());
            let at = tiff.len();
            tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                                (0x013B, 2, 17, artist as u32)]));
            subs.push(at as u32);
        }
        let list = tiff.len();
        for at in &subs {
            tiff.extend_from_slice(&at.to_le_bytes());
        }
        let make = tiff.len() - 8 - 7; // seven bytes before the list ends, so the
        let px = tiff.len();           // Make below runs over the whole list
        tiff.extend_from_slice(&[0xA1; 16]);
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x010F, 2, 7 + 8, make as u32),
                                            (0x014A, 4, 2, list as u32)]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());

        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"Preview Author"), "a sub-directory kept its artist");
        for at in pixels {
            assert_eq!(&out[at..at + 16], &tiff[at..at + 16], "a preview's pixels moved");
        }
        assert_eq!(&out[list..list + 8], &tiff[list..list + 8], "the SubIFDs offsets were destroyed");
    }

    /// A removed tag whose declared size runs on into a sub-directory's raw
    /// pixels must not zero them. Only the page chain's pixels were protected.
    #[test]
    fn an_overrun_does_not_zero_a_sub_directorys_pixels() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let make = tiff.len();
        tiff.extend_from_slice(b"Overrun Co\0");
        let raw = tiff.len();
        tiff.extend_from_slice(&[0xB2; 16]);
        let sub = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, raw as u32), (0x0117, 4, 1, 16)]));
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[
            (0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
            (0x010F, 2, 11 + 16, make as u32), // claims to run through the raw pixels
            (0x014A, 13, 1, sub as u32),
        ]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());

        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"Overrun Co"));
        assert_eq!(&out[raw..raw + 16], &tiff[raw..raw + 16], "the raw pixels were zeroed");
    }

    /// A sub-directory that lists its own parent must not walk forever, and
    /// is still stripped.
    #[test]
    fn a_sub_directory_listing_its_parent_is_stripped_without_looping() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let artist = tiff.len();
        tiff.extend_from_slice(b"Loop Artist\0");
        let sub = tiff.len();
        let first = sub + 2 + 4 * 12 + 4;
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x013B, 2, 12, artist as u32), (0x014A, 13, 1, first as u32)]));
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x014A, 13, 1, sub as u32)]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(strip_tiff(&tiff));
        });
        let out = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the walk looped");
        let (out, _) = out.expect("a well formed tiff");
        assert!(!contains(&out, b"Loop Artist"), "the sub-directory kept its artist");
    }

    /// One directory reached both as a page (through SubIFDs) and as an Exif
    /// directory would be stripped as one and wiped as the other, losing a
    /// page. Such a file is malformed and is refused.
    #[test]
    fn a_directory_that_is_both_a_page_and_an_exif_directory_is_refused() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let shared = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16)]));
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x014A, 13, 1, shared as u32), (0x8769, 4, 1, shared as u32)]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        assert!(strip_tiff(&tiff).is_none(), "a directory with two roles was stripped");
    }

    /// A SubIFDs offset into the pixels would have the strip rewrite picture
    /// bytes as if they were a directory. Refused instead.
    #[test]
    fn a_sub_directory_inside_the_pixels_is_refused() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        // Pixel bytes that read as a directory: one Make entry, inline.
        tiff.extend_from_slice(&directory(&[(0x010F, 2, 4, u32::from_le_bytes(*b"Cam\0"))]));
        tiff.extend_from_slice(&[0xA1; 8]);
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 26),
                                            (0x014A, 13, 1, px as u32)]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        match strip_tiff(&tiff) {
            None => {}
            Some((out, _)) => assert_eq!(&out[px..px + 26], &tiff[px..px + 26], "the pixels were rewritten"),
        }
    }

    /// A SubIFDs list longer than the directory bound is refused at once,
    /// rather than queued offset by offset.
    #[test]
    fn a_subifds_list_longer_than_the_bound_is_refused_quickly() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let sub = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16)]));
        let list = tiff.len();
        let n = MAX_DIRECTORIES as u32 + 1;
        for _ in 0..n {
            tiff.extend_from_slice(&(sub as u32).to_le_bytes());
        }
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x014A, 4, n, list as u32)]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(strip_tiff(&tiff).is_none());
        });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the walk did not finish"),
                "an over-long SubIFDs list was accepted");
    }

    /// SubIFDs are offsets, LONG or IFD. Declared as anything else they cannot
    /// be read, so the directories they name would keep their tags while the
    /// file was reported clean. Refused instead.
    #[test]
    fn a_subifds_entry_of_the_wrong_type_is_refused() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let artist = tiff.len();
        tiff.extend_from_slice(b"Hidden Artist\0");
        let sub = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x013B, 2, 14, artist as u32)]));
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x014A, 7, 4, sub as u32)]));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        match strip_tiff(&tiff) {
            None => {}
            Some((out, _)) => assert!(!contains(&out, b"Hidden Artist"), "reported clean with an unwalked directory"),
        }
    }

    /// A SubIFDs offset into another directory would have the strip rewrite
    /// it from its original bytes, writing a removed value back. Directories
    /// never overlap in a real file, so such a file is refused.
    #[test]
    fn a_sub_directory_overlapping_another_directory_is_refused() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let first = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x0131, 2, 4, u32::from_le_bytes(*b"Ben\0")),
                                            (0x014A, 13, 1, (first + 26) as u32)]));
        tiff.extend_from_slice(&[0u8; 4096]);
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        match strip_tiff(&tiff) {
            None => {}
            Some((out, _)) => assert!(!contains(&out, b"Ben\0"), "a removed value was written back"),
        }
    }

    /// An out-of-line SubIFDs list that runs past the end of the file cannot
    /// be read; misread as a single offset, the directories it names were
    /// never walked. Refused.
    #[test]
    fn a_subifds_list_running_past_the_end_is_refused() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let artist = tiff.len();
        tiff.extend_from_slice(b"Unwalked Artist\0");
        let sub = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x013B, 2, 16, artist as u32)]));
        let first = tiff.len();
        let list = first + 2 + 3 * 12 + 4;
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16),
                                            (0x014A, 4, 4096, list as u32)]));
        for v in [1u32, sub as u32, 0, 0, 0] {
            tiff.extend_from_slice(&v.to_le_bytes());
        }
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        match strip_tiff(&tiff) {
            None => {}
            Some((out, _)) => assert!(!contains(&out, b"Unwalked Artist"), "reported clean with an unwalked directory"),
        }
    }

    /// An Exif directory's next link is not a page chain. A camera that
    /// writes one pointing at the thumbnail directory must not get its file
    /// refused.
    #[test]
    fn an_exif_directory_whose_next_link_names_a_page_is_still_stripped() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let (px, thumb_px) = (tiff.len(), tiff.len() + 16);
        tiff.extend_from_slice(&[0xA1; 32]);
        let date = tiff.len();
        tiff.extend_from_slice(b"2026:10:18 23:00:00\0");
        let ifd1 = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, thumb_px as u32), (0x0117, 4, 1, 16)]));
        let exif = tiff.len();
        let mut exif_dir = directory(&[(0x9003, 2, 20, date as u32)]);
        let n = exif_dir.len();
        exif_dir[n - 4..].copy_from_slice(&(ifd1 as u32).to_le_bytes());
        tiff.extend_from_slice(&exif_dir);
        let first = tiff.len();
        let mut ifd0 = directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16), (0x8769, 4, 1, exif as u32)]);
        let n = ifd0.len();
        ifd0[n - 4..].copy_from_slice(&(ifd1 as u32).to_le_bytes());
        tiff.extend_from_slice(&ifd0);
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        let (out, _) = strip_tiff(&tiff).expect("an ordinary camera layout was refused");
        assert!(!contains(&out, b"2026:10:18 23:00"));
    }

    /// The SubIFDs values one walk reads are bounded in total, not only per
    /// entry: a directory repeating the tag must not cost minutes.
    #[test]
    fn repeated_subifds_entries_are_bounded_in_total() {
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        let px = tiff.len();
        tiff.extend_from_slice(&[0xA1; 16]);
        let sub = tiff.len();
        tiff.extend_from_slice(&directory(&[(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16)]));
        let list = tiff.len();
        for _ in 0..3000 {
            tiff.extend_from_slice(&(sub as u32).to_le_bytes());
        }
        let first = tiff.len();
        let mut entries = vec![(0x0111, 4, 1, px as u32), (0x0117, 4, 1, 16)];
        for _ in 0..200 {
            entries.push((0x014A, 4, 3000, list as u32));
        }
        tiff.extend_from_slice(&directory(&entries));
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(strip_tiff(&tiff).is_none());
        });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the walk did not finish"),
                "SubIFDs past the total bound were accepted");
    }

    /// A scanned document has dozens of pages. The walk stopped after eight
    /// and the rest kept their dates while the file was reported clean.
    #[test]
    fn every_page_of_a_long_document_is_stripped() {
        let pages: Vec<_> = (0..10)
            .map(|n| vec![(0x0132u16, 2u16, format!("PAGE{n:02}-2026:10:17\0").into_bytes())])
            .collect();
        let (tiff, pixels) = tiff_pages(&pages);
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"-2026:10:17"), "a page kept its date");
        for range in pixels {
            assert_eq!(&out[range.clone()], &tiff[range], "a page's pixels moved");
        }
    }

    /// A little-endian, one-directory TIFF from raw blocks placed in order
    /// after the header, then entries `(tag, type, count, value)`.
    fn tiff_from(blocks: &[&[u8]], entries: &[(u16, u16, u32, u32)]) -> Vec<u8> {
        let mut out = b"II\x2A\x00\0\0\0\0".to_vec();
        for block in blocks {
            out.extend_from_slice(block);
        }
        let dir = out.len() as u32;
        out[4..8].copy_from_slice(&dir.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            for field in [&tag.to_le_bytes()[..], &kind.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()] {
                out.extend_from_slice(field);
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out
    }

    /// A camera wrote Make with a count that runs into the first strip. The
    /// entry was dropped but nothing was wiped, because the range touched the
    /// pixels, so the maker's name stayed in the file while it was reported
    /// clean. The part before the pixels is now wiped.
    #[test]
    fn metadata_whose_count_runs_into_the_pixels_is_still_wiped() {
        const MAKE: &[u8] = b"Acme Camera Co\0";
        let pixels_at = 8 + MAKE.len();
        let tiff = tiff_from(
            &[MAKE, &[0xAA; 32]],
            &[(0x010F, 2, MAKE.len() as u32 + 8, 8), (0x0111, 4, 1, pixels_at as u32), (0x0117, 4, 1, 32)],
        );
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"Acme Camera"), "the maker survived");
        assert_eq!(&out[pixels_at..pixels_at + 32], &[0xAA; 32], "the pixels were touched");
    }

    /// A removed tag whose value fits in four bytes is held in the entry
    /// itself. Rewriting the directory shorter left the old entries' bytes
    /// after the new end, value included.
    #[test]
    fn a_removed_inline_value_does_not_survive_in_the_vacated_directory_slots() {
        // Software, four bytes, held inline in the entry itself.
        let inline = u32::from_le_bytes(*b"Ben\0");
        let tiff = tiff_from(
            &[&[0xAA; 16]],
            &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 16), (0x0131, 2, 4, inline)],
        );
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"Ben\0"), "the value stayed in the old directory bytes");
        assert_eq!(&out[8..24], &[0xAA; 16], "the pixels were touched");
    }

    /// An overrun that ends inside the strip-offsets array zeroed the array,
    /// which leaves a file that points its pixels at offset zero. The arrays a
    /// kept entry points at are protected like the pixels.
    #[test]
    fn an_overrun_into_a_kept_array_does_not_zero_it() {
        const MAKE: &[u8] = b"Acme\0\0\0\0";
        let offsets_at = 8 + MAKE.len();
        let counts_at = offsets_at + 8;
        let first = counts_at + 8;
        let second = first + 16;
        let offsets = [(first as u32).to_le_bytes(), (second as u32).to_le_bytes()].concat();
        let counts = [16u32.to_le_bytes(), 16u32.to_le_bytes()].concat();
        let tiff = tiff_from(
            &[MAKE, &offsets, &counts, &[0xAA; 16], &[0xBB; 16]],
            &[
                (0x010F, 2, MAKE.len() as u32 + 8, 8), // runs over the offsets array
                (0x0111, 4, 2, offsets_at as u32),
                (0x0117, 4, 2, counts_at as u32),
            ],
        );
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"Acme"), "the maker survived");
        assert_eq!(&out[offsets_at..offsets_at + 8], &offsets[..], "the strip offsets were zeroed");
        assert_eq!(&out[first..second + 16], &tiff[first..second + 16], "the pixels were touched");
    }

    /// A removed tag whose value lies inside a kept entry's data cannot be
    /// wiped without changing the kept data. A colour map laid over the
    /// maker's name kept the name while the entry was dropped and the file
    /// was reported clean.
    #[test]
    fn a_removed_value_inside_a_kept_entrys_data_is_refused() {
        const MAP: &[u8] = b"\x01\x02Acme Camera Co\0\x03\x04\x05\x06\x07\x08\x09";
        let map_at = 8;
        let pixels_at = map_at + MAP.len();
        let tiff = tiff_from(
            &[MAP, &[0xAA; 16]],
            &[
                (0x010F, 2, 15, map_at as u32 + 2), // Make, inside the colour map
                (0x0111, 4, 1, pixels_at as u32),
                (0x0117, 4, 1, 16),
                (0x0140, 3, (MAP.len() / 2) as u32, map_at as u32), // ColorMap
            ],
        );
        assert!(strip_tiff(&tiff).is_none(), "the maker's name stayed inside the colour map");
    }

    /// The same inside the pixels, which are never changed.
    #[test]
    fn a_removed_value_inside_the_pixels_is_refused() {
        let mut pixels = [0xAA; 32];
        pixels[4..19].copy_from_slice(b"Acme Camera Co\0");
        let tiff = tiff_from(&[&pixels], &[(0x010F, 2, 15, 12), (0x0111, 4, 1, 8), (0x0117, 4, 1, 32)]);
        assert!(strip_tiff(&tiff).is_none(), "the maker's name stayed inside the pixels");
    }

    /// And inside the colour profile, which is never removed.
    #[test]
    fn a_removed_value_inside_the_colour_profile_is_refused() {
        let mut profile = [0xCC; 40];
        profile[8..23].copy_from_slice(b"Acme Camera Co\0");
        let pixels_at = 8 + profile.len();
        let tiff = tiff_from(
            &[&profile, &[0xAA; 16]],
            &[
                (0x010F, 2, 15, 16),
                (0x0111, 4, 1, pixels_at as u32),
                (0x0117, 4, 1, 16),
                (0x8773, 7, profile.len() as u32, 8),
            ],
        );
        assert!(strip_tiff(&tiff).is_none(), "the maker's name stayed inside the profile");
    }

    /// An Exif directory goes whole, and each of its values is wiped; one
    /// lying inside kept data would stay, so the file is refused.
    #[test]
    fn an_exif_value_inside_kept_data_is_refused() {
        let mut pixels = [0xAA; 32];
        pixels[4..24].copy_from_slice(b"2026:10:19 09:41:00\0");
        let exif_at = 8 + pixels.len();
        let exif = directory(&[(0x9003, 2, 20, 12)]); // DateTimeOriginal, inside the pixels
        let tiff = tiff_from(
            &[&pixels, &exif],
            &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 32), (0x8769, 4, 1, exif_at as u32)],
        );
        assert!(strip_tiff(&tiff).is_none(), "the date stayed inside the pixels");
    }

    /// Text ends at its terminator, and the bytes a writer's count claims past
    /// it are not the value: those are the overruns above, which strip. A
    /// value with no other end is its whole declared length, so a binary
    /// value reaching into the pixels would leave its tail there.
    #[test]
    fn a_binary_value_reaching_into_the_pixels_is_refused() {
        const XMP: &[u8] = b"<x:xmpmeta>51.5074 N</x:xmpmeta>\0";
        let pixels_at = 8 + XMP.len();
        let tiff = tiff_from(
            &[XMP, &[0xAA; 32]],
            &[(0x02BC, 1, XMP.len() as u32 + 8, 8), (0x0111, 4, 1, pixels_at as u32), (0x0117, 4, 1, 32)],
        );
        assert!(strip_tiff(&tiff).is_none(), "the XMP's tail stayed in the pixels");
    }

    /// Text with no terminator inside its declared length has no end but that
    /// length, so it is treated like a binary value.
    #[test]
    fn unterminated_text_reaching_into_the_pixels_is_refused() {
        const MAKE: &[u8] = b"Acme Camera Co";
        let pixels_at = 8 + MAKE.len();
        let mut pixels = [0xAA; 32];
        pixels[..4].copy_from_slice(b"rp.\0");
        let tiff = tiff_from(
            &[MAKE, &pixels],
            &[(0x010F, 2, MAKE.len() as u32 + 3, 8), (0x0111, 4, 1, pixels_at as u32), (0x0117, 4, 1, 32)],
        );
        assert!(strip_tiff(&tiff).is_none(), "the end of the maker's name stayed in the pixels");
    }

    /// An Exif directory holds short values in its own entries, so a
    /// directory lying inside kept data would keep them.
    #[test]
    fn an_exif_directory_inside_kept_data_is_refused() {
        let mut pixels = vec![0xAA; 48];
        let exif = directory(&[(0x9003, 2, 4, u32::from_le_bytes(*b"2026"))]);
        pixels[4..4 + exif.len()].copy_from_slice(&exif);
        let tiff = tiff_from(&[&pixels], &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 48), (0x8769, 4, 1, 12)]);
        assert!(strip_tiff(&tiff).is_none(), "the Exif directory inside the pixels was kept");
    }

    /// A value whose declared length runs past the end of the file was not
    /// wiped at all: the entry was dropped and the maker's name stayed. What
    /// of it the file holds is now wiped.
    #[test]
    fn a_removed_value_running_past_the_end_is_wiped_to_the_end() {
        let mut tiff = tiff_from(&[&[0xAA; 16]], &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 16), (0x010F, 2, 64, 0)]);
        let make_at = tiff.len() as u32;
        tiff.extend_from_slice(b"Acme Camera Co\0");
        let entry = 24 + 2 + 2 * 12; // the third entry of the directory after the pixels
        tiff[entry + 8..entry + 12].copy_from_slice(&make_at.to_le_bytes());
        let (out, _) = strip_tiff(&tiff).expect("the value is wiped to the end");
        assert!(!contains(&out, b"Acme"), "the maker's name past the declared end stayed");
        assert_eq!(&out[8..24], &[0xAA; 16], "the pixels were touched");
    }

    /// A removed tag of a type TIFF does not define has no known length, so
    /// its value cannot be found to wipe.
    #[test]
    fn a_removed_value_of_an_unknown_type_is_refused() {
        let tiff = tiff_from(&[b"Acme Camera Co\0", &[0xAA; 16]], &[(0x010F, 0, 15, 8), (0x0111, 4, 1, 23), (0x0117, 4, 1, 16)]);
        assert!(strip_tiff(&tiff).is_none(), "the maker's name of type 0 stayed");
    }

    /// More strip offsets than lengths leaves a strip neither protected nor read.
    #[test]
    fn strip_offsets_and_lengths_that_disagree_are_refused() {
        let offsets = [8u32.to_le_bytes(), 16u32.to_le_bytes()].concat();
        let pixels_at = 8 + 16;
        let tiff = tiff_from(&[&[0xAA; 16], &offsets], &[(0x0111, 4, 2, pixels_at as u32), (0x0117, 4, 1, 8)]);
        assert!(strip_tiff(&tiff).is_none());
    }

    /// A GPS directory can hang off the Exif directory rather than the page.
    /// It was never wiped: the Exif directory went and the coordinates stayed.
    #[test]
    fn a_gps_directory_inside_the_exif_directory_is_wiped() {
        const LATITUDE: &[u8] = b"51.5074 N 0.1278 W\0";
        let lat_at = 8 + 16;
        let gps_at = lat_at + LATITUDE.len();
        let gps = directory(&[(0x0002, 2, LATITUDE.len() as u32, lat_at as u32)]);
        let exif_at = gps_at + gps.len();
        let exif = directory(&[(0x8825, 4, 1, gps_at as u32)]);
        let tiff = tiff_from(&[&[0xAA; 16], LATITUDE, &gps, &exif], &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 16), (0x8769, 4, 1, exif_at as u32)]);
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"51.5074"), "the coordinates stayed");
        assert_eq!(&out[8..24], &[0xAA; 16], "the pixels were touched");
    }

    /// An Exif directory cut off by the end of the file holds its short values
    /// inline, and the part past the end could not be wiped as a directory.
    #[test]
    fn an_exif_directory_cut_off_by_the_end_is_refused() {
        let mut tiff = tiff_from(&[&[0xAA; 16]], &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 16), (0x8769, 4, 1, 0)]);
        let exif_at = tiff.len() as u32;
        let mut exif = directory(&[(0xA430, 2, 4, u32::from_le_bytes(*b"Ben\0"))]);
        exif.truncate(exif.len() - 4); // no link to a next directory
        tiff.extend_from_slice(&exif);
        let entry = 24 + 2 + 2 * 12;
        tiff[entry + 8..entry + 12].copy_from_slice(&exif_at.to_le_bytes());
        assert!(strip_tiff(&tiff).is_none(), "the owner's name stayed");
    }

    /// The header names the byte order and the first directory. A removed
    /// value pointing into it was wiped there, and the stripped file no longer
    /// opened as a TIFF while it was reported clean.
    #[test]
    fn a_removed_value_in_the_header_is_refused() {
        let tiff = tiff_from(&[&[0xAA; 16]], &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 16), (0x010F, 2, 8, 0)]);
        assert!(strip_tiff(&tiff).is_none(), "the header was wiped");
    }

    /// A later page's removed value running over a directory already
    /// stripped zeroed it, cutting the chain; directories are now written
    /// after every wipe.
    #[test]
    fn a_removed_value_over_a_stripped_directory_leaves_it() {
        let mut tiff = b"II\x2A\x00\x08\0\0\0".to_vec();
        let (ifd0, ifd1, px0, px1) = (8u32, 38u32, 80u32, 96u32);
        let mut first = directory(&[(0x0111, 4, 1, px0), (0x0117, 4, 1, 16)]);
        let link = first.len() - 4;
        first[link..].copy_from_slice(&ifd1.to_le_bytes());
        tiff.extend_from_slice(&first);
        // Make's value starts in IFD0 and its count runs over the rest of it.
        tiff.extend_from_slice(&directory(&[(0x010F, 2, 40, ifd0), (0x0111, 4, 1, px1), (0x0117, 4, 1, 16)]));
        tiff.extend_from_slice(&[0xA0; 16]);
        tiff.extend_from_slice(&[0xB0; 16]);
        assert_eq!(tiff.len(), 112);
        let (out, _) = strip_tiff(&tiff).expect("two well formed pages");
        assert_eq!(tags_at(&out, 8), vec![0x0111, 0x0117], "the first page's directory was zeroed");
        assert_eq!(&out[80..112], &tiff[80..112], "the pixels were touched");
    }

    /// Exif 3.0 adds UTF-8 text (type 129), which is removed like ASCII.
    #[test]
    fn utf8_text_is_removed_like_ascii() {
        const ARTIST: &[u8] = "Zoë Ångström\0".as_bytes();
        let tiff = tiff_from(&[ARTIST, &[0xAA; 16]], &[(0x013B, 129, ARTIST.len() as u32, 8), (0x0111, 4, 1, 8 + ARTIST.len() as u32), (0x0117, 4, 1, 16)]);
        let (out, _) = strip_tiff(&tiff).expect("UTF-8 text strips");
        assert!(!contains(&out, "Zoë".as_bytes()), "the artist stayed");
    }

    /// Strips with no lengths, an embedded JPEG with no length, arrays of a
    /// type that cannot be read as offsets, or arrays cut short by the end of
    /// the file name pixels that cannot be protected, so a removed value
    /// could zero them. Lengths with no strips name nothing, as in Sony's SRF.
    #[test]
    fn pieces_that_cannot_be_protected_are_refused() {
        let make: (u16, u16, u32, u32) = (0x010F, 2, 40, 8);
        let cases = [
            ("strips without lengths", vec![make, (0x0111, 4, 1, 8)]),
            ("a JPEG without a length", vec![make, (0x0201, 4, 1, 8)]),
            ("BYTE offsets", vec![make, (0x0111, 1, 1, 8), (0x0117, 4, 1, 16)]),
            ("an array past the end", vec![make, (0x0111, 4, 2, 0x10_0000), (0x0117, 4, 2, 8)]),
            ("both arrays past the end", vec![make, (0x0111, 4, 2, 0x10_0000), (0x0117, 4, 2, 0x10_0000)]),
        ];
        for (name, entries) in cases {
            let tiff = tiff_from(&[&[0xAA; 48]], &entries);
            assert!(strip_tiff(&tiff).is_none(), "{name} passed");
        }
        let srf = tiff_from(&[b"Acme Camera Co\0", &[0xAA; 16]], &[(0x010F, 2, 15, 8), (0x0117, 4, 1, 16)]);
        assert!(strip_tiff(&srf).is_some(), "lengths with no strips were refused");
    }

    /// Strip arrays shared by many directories add up to far more pieces than
    /// the file could hold, which only a file built to exhaust memory has.
    #[test]
    fn strips_far_beyond_what_the_file_holds_are_refused() {
        let n = 20_000u32;
        let offsets: Vec<u8> = (0..n).flat_map(|_| 8u32.to_le_bytes()).collect();
        let lengths: Vec<u8> = (0..n).flat_map(|_| 16u32.to_le_bytes()).collect();
        let mut tiff = b"II\x2A\x00\0\0\0\0".to_vec();
        tiff.extend_from_slice(&[0xAA; 16]);
        let offsets_at = tiff.len() as u32;
        tiff.extend_from_slice(&offsets);
        let lengths_at = tiff.len() as u32;
        tiff.extend_from_slice(&lengths);
        let pages = 200;
        let first = tiff.len();
        tiff[4..8].copy_from_slice(&(first as u32).to_le_bytes());
        for page in 0..pages {
            let mut dir = directory(&[(0x0111, 4, n, offsets_at), (0x0117, 4, n, lengths_at)]);
            if page + 1 < pages {
                let next = (tiff.len() + dir.len()) as u32;
                let link = dir.len() - 4;
                dir[link..].copy_from_slice(&next.to_le_bytes());
            }
            tiff.extend_from_slice(&dir);
        }
        assert!(strip_tiff(&tiff).is_none(), "four million pieces from a {} byte file", tiff.len());
    }

    /// A colour profile inside an Exif directory is left as Strip always
    /// left it, rather than refusing a file whose profile sits there.
    #[test]
    fn a_profile_inside_the_exif_directory_strips() {
        let profile = [0xCC; 40];
        let date = b"2026:10:19 09:41:00\0";
        let date_at = 8 + profile.len() + 16;
        let exif_at = date_at + date.len();
        let exif = directory(&[(0x8773, 7, profile.len() as u32, 8), (0x9003, 2, date.len() as u32, date_at as u32)]);
        let tiff = tiff_from(
            &[&profile, &[0xAA; 16], date, &exif],
            &[(0x0111, 4, 1, 8 + profile.len() as u32), (0x0117, 4, 1, 16), (0x8769, 4, 1, exif_at as u32)],
        );
        let (out, _) = strip_tiff(&tiff).expect("a profile in the Exif directory");
        assert!(!contains(&out, b"2026:10:19"), "the date stayed");
        assert_eq!(&out[8..48], &profile[..], "the profile was touched");
        // That profile is unreachable once its directory goes, so a value
        // lying inside it is wiped rather than the file refused.
        let mut profile = [0xCC; 40];
        profile[10..30].copy_from_slice(date);
        let exif_at = 8 + profile.len() + 16;
        let exif = directory(&[(0x8773, 7, profile.len() as u32, 8), (0x9003, 2, date.len() as u32, 18)]);
        let tiff = tiff_from(
            &[&profile, &[0xAA; 16], &exif],
            &[(0x0111, 4, 1, 8 + profile.len() as u32), (0x0117, 4, 1, 16), (0x8769, 4, 1, exif_at as u32)],
        );
        let (out, _) = strip_tiff(&tiff).expect("a date inside an Exif profile");
        assert!(!contains(&out, b"2026:10:19"), "the date inside the profile stayed");
    }

    /// Sub-images hang off pages; one listed by an Exif directory would never
    /// be walked.
    #[test]
    fn sub_images_listed_by_an_exif_directory_are_refused() {
        const MAKE: &[u8] = b"Acme Camera Co\0";
        let (px, sub_px, make_at) = (8u32, 24u32, 40u32);
        let sub_at = make_at + MAKE.len() as u32;
        let sub = directory(&[(0x010F, 2, MAKE.len() as u32, make_at), (0x0111, 4, 1, sub_px), (0x0117, 4, 1, 16)]);
        let exif_at = sub_at + sub.len() as u32;
        let exif = directory(&[(0x014A, 4, 1, sub_at)]);
        let tiff = tiff_from(&[&[0xAA; 16], &[0xBB; 16], MAKE, &sub, &exif], &[(0x0111, 4, 1, px), (0x0117, 4, 1, 16), (0x8769, 4, 1, exif_at)]);
        assert!(strip_tiff(&tiff).is_none(), "a sub-image only an Exif directory names kept its maker");
    }

    /// An empty text value names nothing, so one starting inside kept data
    /// is no reason to refuse; nor is an Exif pointer past the end of the file.
    #[test]
    fn nothing_to_wipe_is_no_reason_to_refuse() {
        let map = [0u8; 16];
        let tiff = tiff_from(
            &[&map, &[0xAA; 16]],
            &[(0x010F, 2, 8, 12), (0x0111, 4, 1, 24), (0x0117, 4, 1, 16), (0x0140, 3, 8, 8)],
        );
        assert!(strip_tiff(&tiff).is_some(), "an empty maker's name inside the colour map was refused");
        let past = tiff_from(&[&[0xAA; 16]], &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 16), (0x8769, 4, 1, 1 << 20), (0x010F, 2, 4, 0x6D6341)]);
        assert!(strip_tiff(&past).is_some(), "an Exif pointer past the end was refused");
    }

    /// An Exif pointer of zero names no directory. It was read as one at the
    /// start of the file, and its wipe could zero the header.
    #[test]
    fn an_exif_pointer_of_zero_leaves_the_header() {
        let tiff = tiff_from(&[&[0xAA; 16]], &[(0x0111, 4, 1, 8), (0x0117, 4, 1, 16), (0x8769, 4, 1, 0)]);
        let (out, _) = strip_tiff(&tiff).expect("an absent Exif directory");
        assert_eq!(&out[..8], &tiff[..8], "the header was touched");
        // A pointer into the header reads its magic number as an entry count;
        // with the page far enough away nothing else stops the wipe.
        let into_header = tiff_from(&[&[0; 600], &[0xAA; 16]], &[(0x0111, 4, 1, 608), (0x0117, 4, 1, 16), (0x8769, 4, 1, 2)]);
        assert!(strip_tiff(&into_header).is_none(), "an Exif directory inside the header was wiped");
    }

    /// Samsung's NX cameras from 2014 on point their previews' XResolution
    /// and YResolution into IFD0's text: here a resolution starts inside the
    /// maker's name. The resolution is not a picture, and the name has to
    /// go, so it is wiped through the resolution rather than refused or left.
    #[test]
    fn a_removed_value_under_a_resolution_is_wiped_through() {
        const MAKE: &[u8] = b"SAMSUNG\0\0\0\0\0\0\0\0\0";
        let pixels_at = 8 + MAKE.len();
        let tiff = tiff_from(
            &[MAKE, &[0xAA; 16]],
            &[
                (0x010F, 2, MAKE.len() as u32, 8),
                (0x0111, 4, 1, pixels_at as u32),
                (0x0117, 4, 1, 16),
                (0x011A, 5, 1, 12), // XResolution, from inside the maker's name
            ],
        );
        let (out, _) = strip_tiff(&tiff).expect("a Samsung layout strips");
        assert!(!contains(&out, b"SAM") && !contains(&out, b"UNG"), "part of the maker's name stayed");
        assert_eq!(&out[pixels_at..pixels_at + 16], &[0xAA; 16], "the pixels were touched");
    }

    /// The same inside a maker note, where the NX3000 and NX mini point them.
    #[test]
    fn a_maker_note_under_a_resolution_is_wiped_through() {
        let mut note = b"Samsung SN 0123456789 lens 16-50".to_vec();
        note.extend_from_slice(&[0x5A; 16]);
        let note_at = 8;
        let pixels_at = note_at + note.len();
        let exif_at = pixels_at + 16;
        let exif = directory(&[(0x927C, 7, note.len() as u32, note_at as u32)]);
        let tiff = tiff_from(
            &[&note, &[0xAA; 16], &exif],
            &[
                (0x0111, 4, 1, pixels_at as u32),
                (0x0117, 4, 1, 16),
                (0x011A, 5, 1, note_at as u32 + 8), // XResolution, inside the maker note
                (0x8769, 4, 1, exif_at as u32),
            ],
        );
        let (out, _) = strip_tiff(&tiff).expect("a Samsung layout strips");
        assert_eq!(&out[note_at..note_at + note.len()], &vec![0; note.len()][..], "part of the maker note stayed");
        assert_eq!(&out[pixels_at..pixels_at + 16], &[0xAA; 16], "the pixels were touched");
    }

    /// Only the value's own bytes go through a resolution. A count running on
    /// past the text's end into a real resolution leaves the resolution alone.
    #[test]
    fn an_overrun_into_a_resolution_does_not_zero_it() {
        const MAKE: &[u8] = b"Acme\0\0\0\0";
        let resolution = [72u32.to_le_bytes(), 1u32.to_le_bytes()].concat();
        let resolution_at = 8 + MAKE.len();
        let pixels_at = resolution_at + 8;
        let tiff = tiff_from(
            &[MAKE, &resolution, &[0xAA; 16]],
            &[
                (0x010F, 2, MAKE.len() as u32 + 8, 8), // runs over the resolution
                (0x0111, 4, 1, pixels_at as u32),
                (0x0117, 4, 1, 16),
                (0x011A, 5, 1, resolution_at as u32),
            ],
        );
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");
        assert!(!contains(&out, b"Acme"), "the maker survived");
        assert_eq!(&out[resolution_at..resolution_at + 8], &resolution[..], "the resolution was zeroed");
    }

    /// A JPEG from its segments: SOI, each `(marker, payload)`, then a scan
    /// and EOI.
    fn jpeg(segments: &[(u8, &[u8])]) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        for (marker, payload) in segments {
            out.extend_from_slice(&[0xFF, *marker]);
            out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            out.extend_from_slice(payload);
        }
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0x33, 0xFF, 0xD9]);
        out
    }

    /// A one-page TIFF whose only picture is `preview`, an embedded JPEG.
    fn tiff_with_preview(preview: &[u8]) -> Vec<u8> {
        tiff_from(&[preview], &[(0x0201, 4, 1, 8), (0x0202, 4, 1, preview.len() as u32)])
    }

    /// An EXIF APP1 payload: a big-endian TIFF block with a description and
    /// an orientation, the way a camera's preview carries one.
    fn exif_payload(orientation: u16, description: &[u8]) -> Vec<u8> {
        let mut p = b"Exif\0\0MM\0\x2A\0\0\0\x08".to_vec();
        p.extend_from_slice(&2u16.to_be_bytes());
        p.extend_from_slice(&[0x01, 0x0E, 0x00, 0x02]); // ImageDescription, ASCII
        p.extend_from_slice(&(description.len() as u32).to_be_bytes());
        p.extend_from_slice(&(8 + 2 + 24 + 4u32).to_be_bytes()); // just after the directory
        p.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1]); // Orientation, SHORT, one
        p.extend_from_slice(&orientation.to_be_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(description);
        p
    }

    /// The payload of the first `marker` segment of the first image in `jpeg`.
    fn payload_of(jpeg: &[u8], marker: u8) -> &[u8] {
        let mut at = 2;
        loop {
            let length = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
            if jpeg[at + 1] == marker {
                return &jpeg[at + 4..at + 2 + length];
            }
            at += 2 + length;
        }
    }

    /// The preview's bytes in a stripped `tiff_with_preview` file.
    fn stripped_preview(preview: &[u8]) -> (Vec<u8>, usize) {
        let (out, removed) = strip_tiff(&tiff_with_preview(preview)).expect("a preview's metadata is cleaned, not refused");
        (out[8..8 + preview.len()].to_vec(), removed)
    }

    /// The orientation-only EXIF a cleaned preview carries, padded to `len`.
    fn orientation_only(orientation: u16, len: usize) -> Vec<u8> {
        let mut p = crate::metadata::orientation_segment(orientation);
        p.resize(len, 0);
        p
    }

    /// An embedded preview is a JPEG in its own right and can carry its own
    /// EXIF: an iPhone ProRAW preview carries the GPS position. The segment
    /// is rewritten in place to hold nothing but the orientation the preview
    /// is shown by, keeping its length, so no byte of the preview moves.
    #[test]
    fn a_preview_keeps_only_the_orientation_of_its_exif() {
        let exif = exif_payload(6, b"51.5074 N 0.1278 W\0");
        let preview = jpeg(&[(0xE1, &exif)]);
        let (cleaned, removed) = stripped_preview(&preview);
        assert!(removed > 0, "nothing was reported removed");
        assert!(!contains(&cleaned, b"51.5074"), "the location stayed in the preview");
        assert_eq!(payload_of(&cleaned, 0xE1), &orientation_only(6, exif.len())[..]);
        assert_eq!(&cleaned[..6], &preview[..6], "the segment's marker or length moved");
        assert_eq!(&cleaned[6 + exif.len()..], &preview[6 + exif.len()..], "the rest of the preview changed");
    }

    /// An EXIF segment too small for the orientation block is blanked.
    #[test]
    fn an_exif_segment_too_small_to_rewrite_is_blanked() {
        let preview = jpeg(&[(0xE1, b"Exif\0\0MM\0\x2A GPS!")]);
        let (cleaned, _) = stripped_preview(&preview);
        assert!(payload_of(&cleaned, 0xE1).iter().all(|&b| b == 0));
    }

    /// The preview's own XMP and IPTC say nothing a renderer needs, so their
    /// segments are blanked where they lie.
    #[test]
    fn a_previews_xmp_and_iptc_are_blanked() {
        for (marker, payload) in [
            (0xE1u8, &b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta><exif:GPSLatitude>51.5</exif:GPSLatitude></x:xmpmeta>"[..]),
            (0xED, b"Photoshop 3.0\08BIM\x04\x04 By-line"),
        ] {
            let preview = jpeg(&[(marker, payload)]);
            let (cleaned, removed) = stripped_preview(&preview);
            assert!(removed > 0);
            assert!(payload_of(&cleaned, marker).iter().all(|&b| b == 0), "segment {marker:02X} was not blanked");
            assert_eq!(&cleaned[..6], &preview[..6]);
        }
    }

    /// Whatever Squint's JPEG Strip removes is blanked: a comment, the older
    /// Photoshop identifier, content credentials (APP11), Ducky (APP12), and
    /// XMP's extension segment.
    #[test]
    fn every_segment_jpeg_strip_removes_is_blanked_in_a_preview() {
        let segments: [(u8, &[u8]); 5] = [
            (0xFE, b"taken at home"),
            (0xED, b"Adobe_Photoshop2.5:\x1c\x02\x50 By-line"),
            (0xEB, b"JP\0\x01 c2pa"),
            (0xEC, b"Ducky\0\x01"),
            (0xE1, b"http://ns.adobe.com/xmp/extension/\0 GPS"),
        ];
        for (marker, payload) in segments {
            let (cleaned, _) = stripped_preview(&jpeg(&[(marker, payload)]));
            assert!(payload_of(&cleaned, marker).iter().all(|&b| b == 0), "segment {marker:02X} was not blanked");
        }
    }

    /// A preview already cleaned has nothing left to remove, so stripping
    /// the stripped file again finds nothing.
    #[test]
    fn a_cleaned_preview_has_nothing_left_to_remove() {
        let preview = jpeg(&[(0xE1, &exif_payload(3, b"Acme Camera Co\0")), (0xFE, b"taken at home")]);
        let (once, _) = strip_tiff(&tiff_with_preview(&preview)).expect("cleaned");
        let (twice, removed) = strip_tiff(&once).expect("cleaned again");
        assert_eq!(removed, 0, "a second strip found more to remove");
        assert_eq!(once, twice);
    }

    /// The same EXIF in a page whose strips are JPEG-compressed, and in the
    /// tables such a page shares between its strips.
    #[test]
    fn jpeg_compressed_strips_and_tables_are_cleaned() {
        let exif = exif_payload(1, b"Acme Camera Co\0");
        let strip = jpeg(&[(0xE1, &exif)]);
        let tiff = tiff_from(&[&strip], &[(0x0103, 3, 1, 7), (0x0111, 4, 1, 8), (0x0117, 4, 1, strip.len() as u32)]);
        let (out, _) = strip_tiff(&tiff).expect("a JPEG strip's EXIF is cleaned");
        assert!(!contains(&out, b"Acme Camera"));
        let pixels_at = 8 + strip.len();
        let tiff = tiff_from(
            &[&strip, &[0xAA; 16]],
            &[(0x0103, 3, 1, 7), (0x0111, 4, 1, pixels_at as u32), (0x0117, 4, 1, 16), (0x015B, 7, strip.len() as u32, 8)],
        );
        let (out, _) = strip_tiff(&tiff).expect("the tables' EXIF is cleaned");
        assert!(!contains(&out, b"Acme Camera"));
        let keep = unchangeable(&tiff).expect("a well formed tiff");
        assert!(overlaps(&keep, 8, 14), "the tables lost their protection");
    }

    /// A preview with only the segments a decoder needs, JFIF, a colour
    /// profile, Adobe's colour transform and its tables, carries nothing to
    /// remove; nor does a progressive one with restarts and fill bytes in its
    /// scans.
    #[test]
    fn previews_with_nothing_to_remove_are_left_as_they_are() {
        let plain = jpeg(&[
            (0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"),
            (0xE2, b"ICC_PROFILE\0\x01\x01 profile"),
            (0xEE, b"Adobe\0\x64\0\0\0\0\x01"),
            (0xDB, &[0; 65]),
        ]);
        assert_eq!(stripped_preview(&plain).0, plain);
        // Pentax's empty APP0, the Multi-Picture index, Blackmagic's filler.
        let quirks = jpeg(&[(0xE0, b"\0\0"), (0xE2, &mpf_index(&[], &[], &[])), (0xFE, &[0x99; 4])]);
        assert_eq!(stripped_preview(&quirks).0, quirks);
        let mut progressive = jpeg(&[(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0")]);
        progressive.truncate(progressive.len() - 2);
        progressive.extend_from_slice(&[0x44, 0xFF, 0x00, 0x55, 0xFF, 0xD0, 0x66, 0xFF, 0xFF, 0xDA, 0x00, 0x02, 0x77, 0xFF, 0xD9]);
        assert_eq!(stripped_preview(&progressive).0, progressive);
    }

    /// A progressive preview can carry XMP between its scans, past where a
    /// walk stopping at the first scan would look; it is blanked there.
    #[test]
    fn metadata_between_a_previews_scans_is_blanked() {
        let mut preview = jpeg(&[]);
        preview.truncate(preview.len() - 2); // before EOI
        preview.extend_from_slice(&[0x44, 0xFF, 0x00, 0xFF, 0xD3, 0x55, 0xFF, 0x01, 0x66, 0xFF, 0xFF]);
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<exif:GPSLatitude>51.5</exif:GPSLatitude>";
        preview.extend_from_slice(&[0xFF, 0xE1]);
        preview.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
        preview.extend_from_slice(xmp);
        preview.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x66, 0xFF, 0xD9]);
        let (cleaned, _) = stripped_preview(&preview);
        assert!(!contains(&cleaned, b"GPSLatitude"), "the XMP between scans stayed");
        assert_eq!(cleaned.len(), preview.len());
    }

    /// A broken segment would hide the ones after it, so a preview that opens
    /// as a JPEG but cannot be followed to its end is refused.
    #[test]
    fn a_preview_that_cannot_be_followed_is_refused() {
        let mut hidden = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x00];
        hidden.extend_from_slice(&jpeg(&[(0xE1, b"Exif\0\0 GPS")])[2..]);
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&hidden)), Err(Refused::Layout), "a zero-length segment hid the EXIF");
        let stray = [&[0xFF, 0xD8, 0x00][..], &jpeg(&[(0xE1, b"Exif\0\0 GPS")])[2..]].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&stray)), Err(Refused::Layout), "a stray byte hid the EXIF");
        let truncated = [0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x00];
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&truncated)), Err(Refused::Layout), "a table cut off before the scan passed");
        // A second start of image read as a segment would swallow the EXIF.
        let mut restart = vec![0xFF, 0xD8, 0xFF, 0xD8, 0x00, 0x10, 0xFF, 0xE1, 0x00, 0x0C];
        restart.extend_from_slice(b"Exif\0\0 GPS"); // ten bytes, as both lengths say
        restart.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x11, 0xFF, 0xD9]);
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&restart)), Err(Refused::Layout), "a second start of image hid the EXIF");
    }

    /// A stream that breaks inside its scans is read as far as it goes:
    /// Hasselblad writes its raw data as JPEG scans laid out its own way.
    #[test]
    fn a_stream_broken_inside_its_scans_still_strips() {
        let broken = [0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0x55, 0xFF];
        assert!(strip_tiff(&tiff_with_preview(&broken)).is_some());
    }

    /// The gain map an iPhone ProRAW preview carries as a second image, with
    /// the XMP the iPhone 12 Pro writes: its rendering parameters only.
    const IPHONE_GAIN_MAP_XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"XMP Core 6.0.0\">\n   <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n      <rdf:Description rdf:about=\"\"\n            xmlns:HDRGainMap=\"http://ns.apple.com/HDRGainMap/1.0/\"\n            xmlns:apdi=\"http://ns.apple.com/pixeldatainfo/1.0/\">\n         <HDRGainMap:HDRGainMapVersion>65536</HDRGainMap:HDRGainMapVersion>\n         <apdi:NativeFormat>1278226488</apdi:NativeFormat>\n         <apdi:AuxiliaryImageType>urn:com:apple:photo:2020:aux:hdrgainmap</apdi:AuxiliaryImageType>\n         <apdi:StoredFormat>1278226488</apdi:StoredFormat>\n      </rdf:Description>\n   </rdf:RDF>\n</x:xmpmeta>\n";

    /// The gain map keeps its rendering XMP byte for byte, so the preview keeps
    /// its HDR, while the first image's EXIF is cleaned as before.
    #[test]
    fn a_gain_maps_rendering_xmp_is_kept() {
        let exif = exif_payload(6, b"51.5074 N 0.1278 W\0");
        let first = jpeg(&[(0xE1, &exif), (0xE2, b"MPF\0MM\0\x2A")]);
        let map = jpeg(&[(0xE1, IPHONE_GAIN_MAP_XMP)]);
        let pair = [first.clone(), map.clone()].concat();
        let (cleaned, _) = stripped_preview(&pair);
        assert!(!contains(&cleaned, b"51.5074"));
        assert_eq!(&cleaned[first.len()..], &map[..], "the gain map changed");
    }

    /// A gain map whose XMP holds its rendering parameters and anything more
    /// is refused, as Strip refuses one in a photograph: removing the XMP
    /// would leave the map unreadable, and keeping it would keep the rest.
    #[test]
    fn a_gain_map_whose_xmp_says_more_is_refused() {
        let xmp = String::from_utf8_lossy(IPHONE_GAIN_MAP_XMP)
            .replace("<apdi:StoredFormat>", "<xmp:CreatorTool>iPhone 12 Pro</xmp:CreatorTool>\n         <apdi:StoredFormat>")
            .into_bytes();
        let pair = [jpeg(&[(0xE2, b"MPF\0MM\0\x2A")]), jpeg(&[(0xE1, &xmp)])].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&pair)), Err(Refused::Preview));
    }

    /// A second image's EXIF is blanked, as Strip drops a photograph's gain
    /// map's EXIF, wherever the second image starts: right after the first,
    /// after padding, or past the end of a strip that names only the first.
    #[test]
    fn a_second_images_exif_is_cleaned_wherever_it_starts() {
        let exif = exif_payload(1, b"Acme Camera Co\0");
        let first = jpeg(&[(0xE2, b"MPF\0MM\0\x2A")]);
        let second = jpeg(&[(0xE1, &exif)]);
        for pair in [[first.clone(), second.clone()].concat(), [first.clone(), vec![0, 0, 0xFF, 0xFF], second.clone()].concat()] {
            let (cleaned, _) = stripped_preview(&pair);
            let second_at = pair.len() - second.len();
            assert!(payload_of(&cleaned[second_at..], 0xE1).iter().all(|&b| b == 0), "the second image's EXIF was not blanked");
        }
        let stream = [jpeg(&[]), second.clone()].concat();
        let tiff = tiff_from(
            &[&stream],
            &[(0x0103, 3, 1, 7), (0x0111, 4, 1, 8), (0x0117, 4, 1, 7), (0x0201, 4, 1, 8), (0x0202, 4, 1, stream.len() as u32)],
        );
        let (out, _) = strip_tiff(&tiff).expect("cleaned");
        assert!(!contains(&out, b"Acme Camera"), "the read past the strip's end was skipped");
    }

    /// A later image's XMP that names no rendering parameters, and any other
    /// APP1 it carries, are blanked, as Strip drops them from a photograph's
    /// gain map; the ISO gain map segment stays in both images, since a
    /// renderer looks for the map by it.
    #[test]
    fn a_gain_maps_other_metadata_is_blanked_and_its_iso_segment_kept() {
        let iso_primary: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0\0\0";
        let iso_map: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0\0\0\0\x01\0\0\0\0";
        let with = |offset| jpeg(&[(0xE2, &mpf_index(&[offset], &[], &[])), (0xE2, iso_primary)]);
        let first = with(with(0).len() as u32 - 10);
        let other_xmp: &[u8] = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta><xmp:CreatorTool>iPhone 12 Pro</xmp:CreatorTool></x:xmpmeta>";
        let map = jpeg(&[(0xE1, other_xmp), (0xE1, b"Acme maker data"), (0xE2, iso_map)]);
        let pair = [first.clone(), map.clone()].concat();
        let (cleaned, _) = stripped_preview(&pair);
        assert!(!contains(&cleaned, b"iPhone 12 Pro"), "the gain map kept its other XMP");
        assert!(!contains(&cleaned, b"Acme maker"), "the gain map kept its other APP1");
        assert_eq!(&cleaned[..first.len()], &first[..], "the first image's ISO segment or index changed");
        assert!(contains(&cleaned[first.len()..], iso_map), "the gain map lost its ISO segment");
    }

    /// Old-style JPEG (6) and DNG's lossy JPEG (34892) strips, and JPEG tiles.
    #[test]
    fn every_jpeg_compression_and_tiles_are_cleaned() {
        let piece = jpeg(&[(0xE1, &exif_payload(1, b"Acme Camera Co\0"))]);
        for compression in [6, 34892] {
            let tiff = tiff_from(&[&piece], &[(0x0103, 3, 1, compression), (0x0111, 4, 1, 8), (0x0117, 4, 1, piece.len() as u32)]);
            let (out, _) = strip_tiff(&tiff).expect("cleaned");
            assert!(!contains(&out, b"Acme Camera"), "compression {compression} kept its EXIF");
        }
        let tiles = tiff_from(&[&piece], &[(0x0103, 3, 1, 7), (0x0144, 4, 1, 8), (0x0145, 4, 1, piece.len() as u32)]);
        let (out, _) = strip_tiff(&tiles).expect("cleaned");
        assert!(!contains(&out, b"Acme Camera"), "a JPEG tile kept its EXIF");
    }

    /// The bytes Strip promises not to change leave out exactly the segments
    /// it rewrote, and include the rest of the preview.
    #[test]
    fn unchangeable_leaves_out_only_the_rewritten_segments() {
        let exif = exif_payload(6, b"51.5074 N 0.1278 W\0");
        let preview = jpeg(&[(0xE1, &exif)]);
        let tiff = tiff_with_preview(&preview);
        let keep = unchangeable(&tiff).expect("a well formed tiff");
        let (start, end) = (8 + 6, 8 + 6 + exif.len());
        assert!(!overlaps(&keep, start, end), "the rewritten EXIF is still promised unchanged");
        assert!(overlaps(&keep, 8, 14) && overlaps(&keep, end, 8 + preview.len()), "the rest of the preview lost its promise");
    }

    /// A rewritten segment that a kept entry also points at, or a directory
    /// lies in, would be read back as that entry's data or that directory, so
    /// the file is refused; as it is when two streams with rewrites overlap,
    /// since each would read the other's rewrite as pixels.
    #[test]
    fn a_rewrite_shared_with_other_kept_data_is_refused() {
        let exif = exif_payload(6, b"51.5074 N 0.1278 W ");
        let preview = jpeg(&[(0xE1, &exif)]);
        let inside = 8 + 6 + 20; // within the EXIF payload
        let profile = tiff_from(&[&preview], &[(0x0201, 4, 1, 8), (0x0202, 4, 1, preview.len() as u32), (ICC, 7, 8, inside as u32)]);
        assert_eq!(strip_tiff_checked(&profile), Err(Refused::Layout), "the colour profile was rewritten");
        let mut directory = tiff_with_preview(&preview);
        let dir_at = u32::from_le_bytes(directory[4..8].try_into().unwrap()) as usize;
        let entries = directory[dir_at..].to_vec();
        directory[inside..inside + entries.len().min(exif.len() - 20)].copy_from_slice(&entries[..entries.len().min(exif.len() - 20)]);
        directory[4..8].copy_from_slice(&(inside as u32).to_le_bytes());
        assert_eq!(strip_tiff_checked(&directory), Err(Refused::Layout), "a directory was rewritten");
        let pair = [preview.clone(), preview.clone()].concat();
        let shifted = tiff_from(
            &[&pair],
            &[(0x0103, 3, 1, 7), (0x0111, 4, 1, 8), (0x0117, 4, 1, pair.len() as u32), (0x0201, 4, 1, 8), (0x0202, 4, 1, preview.len() as u32)],
        );
        assert_eq!(strip_tiff_checked(&shifted), Err(Refused::Layout), "two streams with rewrites overlapped");
    }

    /// A comment holding text in any script, and an APP0 that is not JFIF,
    /// are blanked; a comment of one byte repeated is filler and stays.
    #[test]
    fn comments_and_app0_holding_anything_are_blanked() {
        for (marker, payload) in [(0xFEu8, "東京都渋谷区".as_bytes()), (0xFE, b"ab"), (0xE0, b"Acme maker note")] {
            let (cleaned, _) = stripped_preview(&jpeg(&[(marker, payload)]));
            assert!(payload_of(&cleaned, marker).iter().all(|&b| b == 0), "segment {marker:02X} {payload:?} was not blanked");
        }
        let filler = jpeg(&[(0xFE, &[0x99; 7]), (0xE0, &[0; 3])]);
        assert_eq!(stripped_preview(&filler).0, filler);
    }

    /// A JFIF segment keeps its header and loses the thumbnail it can carry,
    /// a picture of the scene; a JFXX segment, which holds a thumbnail and
    /// nothing else, is blanked.
    #[test]
    fn a_jfif_thumbnail_and_a_jfxx_segment_are_removed() {
        let jfif = b"JFIF\0\x01\x02\0\0\x01\0\x01\x02\x01\xAA\xBB\xCC\xDD\xEE\xFF"; // a 2 by 1 thumbnail
        let (cleaned, _) = stripped_preview(&jpeg(&[(0xE0, jfif)]));
        assert_eq!(payload_of(&cleaned, 0xE0), b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0\0\0\0\0\0\0");
        let (cleaned, _) = stripped_preview(&jpeg(&[(0xE0, b"JFXX\0\x10\xFF\xD8 thumbnail")]));
        assert!(payload_of(&cleaned, 0xE0).iter().all(|&b| b == 0));
    }

    /// A Multi-Picture index payload listing the first image and one more at
    /// each of `offsets` (from the index's TIFF header, ten bytes into a
    /// stream that opens with it), with `extra` directory entries and `tail`
    /// bytes after the entries. Sizes are not read, so each is the largest a
    /// LONG holds.
    fn mpf_index(offsets: &[u32], extra: &[[u8; 12]], tail: &[u8]) -> Vec<u8> {
        let n = 1 + offsets.len() as u32;
        let count = 3 + extra.len();
        let mut p = b"MPF\0MM\0\x2A\0\0\0\x08".to_vec();
        p.extend_from_slice(&(count as u16).to_be_bytes());
        p.extend_from_slice(&[0xB0, 0x00, 0, 7, 0, 0, 0, 4, b'0', b'1', b'0', b'0']);
        p.extend_from_slice(&[0xB0, 0x01, 0, 4, 0, 0, 0, 1]);
        p.extend_from_slice(&n.to_be_bytes());
        p.extend_from_slice(&[0xB0, 0x02, 0, 7]);
        p.extend_from_slice(&(16 * n).to_be_bytes());
        p.extend_from_slice(&((8 + 2 + 12 * count + 4) as u32).to_be_bytes());
        for e in extra {
            p.extend_from_slice(e);
        }
        p.extend_from_slice(&[0; 4]); // no next directory
        for offset in std::iter::once(0).chain(offsets.iter().copied()) {
            p.extend_from_slice(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
            p.extend_from_slice(&offset.to_be_bytes());
            p.extend_from_slice(&[0; 4]);
        }
        p.extend_from_slice(tail);
        p
    }

    /// The first image's index keeps its version, image count and entries,
    /// which is all a renderer reads, and loses its unique image IDs and
    /// anything after them. An index that cannot be read is blanked, as is a
    /// later image's own index.
    #[test]
    fn a_multi_picture_index_keeps_only_what_finds_the_images() {
        let ids = [0xB0, 0x03, 0, 7, 0, 0, 0, 8, 0, 0, 0, 94]; // eight bytes after the entries
        let with = |offset| mpf_index(&[offset], &[ids], b"UID-4242Acme Camera");
        let first_len = jpeg(&[(0xE2, &with(0))]).len();
        let index = with(first_len as u32 - 10);
        let first = jpeg(&[(0xE2, &index)]);
        let map = jpeg(&[(0xE2, b"MPF\0MM\0\x2A\0\0\0\x08 Acme"), (0xE1, IPHONE_GAIN_MAP_XMP)]);
        let (cleaned, _) = stripped_preview(&[first.clone(), map.clone()].concat());
        let mut expected = index.clone();
        expected[12..14].copy_from_slice(&[0, 3]);
        expected[12 + 2 + 36..12 + 2 + 48].fill(0);
        let tail_at = expected.len() - 19;
        expected[tail_at..].fill(0);
        assert_eq!(payload_of(&cleaned, 0xE2), &expected[..]);
        assert!(payload_of(&cleaned[first.len()..], 0xE2).iter().all(|&b| b == 0), "the gain map kept its own index");
        let (cleaned, _) = stripped_preview(&jpeg(&[(0xE2, b"MPF\0MM\0\x2A\0\0\0\x08 Acme")]));
        assert!(payload_of(&cleaned, 0xE2).iter().all(|&b| b == 0), "an index that cannot be read stayed");
        // An entry table claiming more bytes than the segment holds.
        let mut overlong = mpf_index(&[], &[], &[]);
        overlong[12 + 2 + 24 + 4..12 + 2 + 24 + 8].copy_from_slice(&[0, 1, 0, 0]);
        let (cleaned, _) = stripped_preview(&jpeg(&[(0xE2, &overlong)]));
        assert!(payload_of(&cleaned, 0xE2).iter().all(|&b| b == 0), "an index with an overlong table stayed");
        // A second entry table, a copy of the first.
        let mut table = [0; 12];
        table.copy_from_slice(&mpf_index(&[], &[], &[])[12 + 2 + 24..12 + 2 + 36]);
        let twice = mpf_index(&[], &[table], &[]);
        let (cleaned, _) = stripped_preview(&jpeg(&[(0xE2, &twice)]));
        assert!(payload_of(&cleaned, 0xE2).iter().all(|&b| b == 0), "an index with two entry tables stayed");
    }

    /// A kept range the same bytes as a stream with rewrites, here a colour
    /// profile, is still kept data the rewrite would land on.
    #[test]
    fn a_rewrite_onto_a_kept_range_of_the_same_bounds_is_refused() {
        let preview = jpeg(&[(0xE1, &exif_payload(6, b"51.5074 N 0.1278 W\0"))]);
        let n = preview.len() as u32;
        let tiff = tiff_from(&[&preview], &[(0x0201, 4, 1, 8), (0x0202, 4, 1, n), (ICC, 7, n, 8)]);
        assert_eq!(strip_tiff_checked(&tiff), Err(Refused::Layout));
    }

    /// Past the first scan a segment is found by reading the scan data, so a
    /// rewrite there needs the stream to end as a JPEG does; one that runs
    /// out instead could be raw sensor data read as a segment.
    #[test]
    fn a_rewrite_past_a_scan_needs_an_end_of_image() {
        let mut stream = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xE3, 0x00, 0x20];
        stream.extend_from_slice(&[0x5A; 30]);
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&stream)), Err(Refused::Layout), "sensor data was rewritten");
        let ended = [&stream[..], &[0xFF, 0xD9]].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&ended)), Err(Refused::Layout), "sensor data before an end of image was rewritten");
        let scanned = [&stream[..], &[0xFF, 0xDA, 0x00, 0x02, 0x33, 0xFF, 0xD9]].concat();
        let (cleaned, _) = stripped_preview(&scanned);
        assert!(cleaned[12..42].iter().all(|&b| b == 0), "the segment between scans was not blanked");
    }

    /// A Multi-Picture index listing a second image the walk never reaches,
    /// past the stream's declared end or behind bytes that are not padding,
    /// refuses the file: a reader following the index would find that
    /// image's metadata. Reached, the image is cleaned. The index is read
    /// where the walk finds it, after fill bytes too, and each entry on its
    /// own, whatever size it claims.
    #[test]
    fn an_image_the_index_lists_must_be_reached() {
        let second = jpeg(&[(0xE1, &exif_payload(1, b"Acme Camera Co\0"))]);
        let with = |offset| jpeg(&[(0xE2, &mpf_index(&[offset], &[], &[]))]);
        let len = with(0).len() as u32;
        let first = with(len - 10);
        let pair = [first.clone(), second.clone()].concat();
        let (cleaned, _) = stripped_preview(&pair);
        assert!(!contains(&cleaned, b"Acme Camera"));
        let short = tiff_from(&[&pair], &[(0x0201, 4, 1, 8), (0x0202, 4, 1, first.len() as u32)]);
        assert_eq!(strip_tiff_checked(&short), Err(Refused::Layout), "an image past the stream's end kept its EXIF");
        let hidden = [with(len - 9), vec![0x42], second.clone()].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&hidden)), Err(Refused::Layout), "an image behind a stray byte kept its EXIF");
        let filled = [&[0xFF, 0xD8, 0xFF][..], &with(len - 9)[2..], &[0x42], &second].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&filled)), Err(Refused::Layout), "an index after a fill byte went unread");
        // The first entry pointing at the hidden image rather than at the first.
        let mut first_entry = with(0);
        let entry0 = 6 + 4 + 50 + 8;
        first_entry[entry0..entry0 + 4].copy_from_slice(&(len - 9).to_be_bytes());
        let pointed = [first_entry, vec![0x42], second.clone()].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&pointed)), Err(Refused::Layout), "an image the first entry names kept its EXIF");
    }

    /// A directory naming its strips, tiles, embedded JPEG or JPEG tables
    /// twice is refused: the second would never be read, and a JPEG there
    /// would keep its metadata.
    #[test]
    fn pixels_or_tables_named_twice_are_refused() {
        let clean = jpeg(&[(0xDB, &[0; 65])]);
        let carrying = jpeg(&[(0xE1, &exif_payload(1, b"Acme Camera Co\0"))]);
        let at = 8 + clean.len() as u32;
        for tag in [0x0201u16, 0x015B] {
            let length = if tag == 0x0201 { vec![(0x0202, 4, 1, clean.len() as u32), (0x0202, 4, 1, carrying.len() as u32)] } else { vec![] };
            let (kind, first, second) = if tag == 0x0201 { (4, (1, 8), (1, at)) } else { (7, (clean.len() as u32, 8), (carrying.len() as u32, at)) };
            let mut entries = vec![(0x0103, 3, 1, 7), (tag, kind, first.0, first.1), (tag, kind, second.0, second.1)];
            entries.extend(length);
            let tiff = tiff_from(&[&clean, &carrying], &entries);
            assert_eq!(strip_tiff_checked(&tiff), Err(Refused::Layout), "tag {tag:04X} named twice");
        }
    }

    /// Strips naming the same bytes are read once: a clean JPEG strip named
    /// fifty thousand times strips, where reading it each time would run out
    /// the budget and refuse the file.
    #[test]
    fn repeated_jpeg_strips_are_read_once() {
        let piece = jpeg(&[(0xDB, &[0; 65])]);
        let n = 50_000u32;
        let offsets: Vec<u8> = (0..n).flat_map(|_| 8u32.to_le_bytes()).collect();
        let lengths: Vec<u8> = (0..n).flat_map(|_| (piece.len() as u32).to_le_bytes()).collect();
        let at = 8 + piece.len();
        let tiff = tiff_from(
            &[&piece, &offsets, &lengths],
            &[(0x0103, 3, 1, 7), (0x0111, 4, n, at as u32), (0x0117, 4, n, (at + offsets.len()) as u32)],
        );
        assert!(strip_tiff(&tiff).is_some());
    }

    /// Every byte a JPEG walk reads is charged to one budget for the file,
    /// and running out refuses rather than reading on.
    #[test]
    fn a_jpeg_walk_stops_when_its_budget_runs_out() {
        let piece = jpeg(&[(0xDB, &[0; 200])]);
        assert!(clean_jpeg(&piece, 0, piece.len(), &mut 100).is_none());
        // Padding after the end of image is read too.
        let padded = [&[0xFF, 0xD8, 0xFF, 0xD9][..], &[0; 200]].concat();
        assert!(clean_jpeg(&padded, 0, padded.len(), &mut 100).is_none());
        assert!(matches!(clean_jpeg(&piece, 0, piece.len(), &mut 10_000), Some(Scan::Clean(edits)) if edits.is_empty()));
    }

    /// A chain longer than the walk will follow is refused, not half stripped.
    #[test]
    fn a_directory_chain_longer_than_the_bound_is_refused() {
        let pages: Vec<_> = (0..MAX_DIRECTORIES + 1).map(|_| Vec::new()).collect();
        let (tiff, _) = tiff_pages(&pages);
        assert!(strip_tiff(&tiff).is_none());
    }

    #[test]
    fn recognises_a_tiff_by_either_byte_order() {
        assert!(is_tiff(b"II\x2A\x00\x08\0\0\0"));
        assert!(is_tiff(b"MM\x00\x2A\0\0\0\x08"));
        // BigTIFF announces 43 and is laid out differently.
        assert!(!is_tiff(b"II\x2B\x00\x08\0\0\0"));
        assert!(!is_tiff(b"\xFF\xD8\xFF\xE0\0\0\0\0"));
    }

    #[test]
    fn removes_the_naming_tags_and_leaves_the_picture_and_profile() {
        let (tiff, pixels) = tiff_with_metadata();
        assert!(contains(&tiff, b"A Camera Company"));
        assert!(contains(&tiff, b"51.5074 N"));

        let (out, wiped) = strip_tiff(&tiff).expect("a well formed tiff");

        assert!(wiped > 0, "something should have been destroyed");
        assert_eq!(out.len(), tiff.len(), "nothing moves, so nothing changes length");
        assert!(!contains(&out, b"A Camera Company"), "the maker survived");
        assert!(!contains(&out, b"51.5074 N"), "the location survived");

        // The picture and the profile are what this must never touch.
        assert_eq!(out[pixels].iter().filter(|b| **b == 0xAA).count(), 32);
        assert!(contains(&out, &[0xCC; 20]), "the colour profile was destroyed");
    }

    /// A reader given a directory entry that points at nothing rejects the whole
    /// file, so the entries have to go rather than be emptied.
    #[test]
    fn drops_the_entries_rather_than_blanking_them() {
        let (tiff, _) = tiff_with_metadata();
        let (out, _) = strip_tiff(&tiff).expect("a well formed tiff");

        let dir = u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize;
        let before = u16::from_le_bytes([tiff[dir], tiff[dir + 1]]);
        let after = u16::from_le_bytes([out[dir], out[dir + 1]]);
        assert_eq!(before, 6);
        assert_eq!(after, 4, "Make and the Exif pointer should both be gone");

        // What remains must still be readable as entries, in order.
        let tags: Vec<u16> = (0..after as usize)
            .map(|i| {
                let at = dir + 2 + i * 12;
                u16::from_le_bytes([out[at], out[at + 1]])
            })
            .collect();
        assert_eq!(tags, vec![0x0111, 0x0117, 0x0112, 0x8773]);
    }

    #[test]
    fn refuses_something_that_is_not_a_tiff() {
        assert!(strip_tiff(b"not a tiff, not even a little").is_none());
    }

    /// Found by the fuzzer in its first thirty seconds: forty bytes whose one
    /// entry declares a count in the billions. Every read past the end of the
    /// file fails, but the loop still ran once per declared element, and the
    /// walk took longer than the fuzzer's twenty-second timeout. The count is
    /// now bounded by what the file holds, so this returns at once. The
    /// deadline is generous so a slow machine cannot fail it by accident; the
    /// unfixed code took tens of seconds.
    #[test]
    fn a_count_in_the_billions_does_not_hang_the_walk() {
        let hostile: [u8; 40] = [
            0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x06, 0x02, 0x02, 0x02, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x02, 0x01, 0x00, 0x08, 0x4d, 0x00, 0xc8, 0x00,
            0x00, 0x00, 0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x2a, 0x00, 0x0a,
        ];
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(strip_tiff(&hostile).map(|(out, wiped)| (out.len(), wiped)));
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the walk must finish in seconds, not hang on a declared count");
        // Whatever it decides about the file, it decided it quickly.
        let _ = outcome;
    }

    /// Also from the fuzzer: 220 bytes whose directory chain points back at
    /// itself and whose entries name the same bytes more than once. Each pass
    /// zeroed the same region again and added its size again, so the count of
    /// bytes destroyed came out larger than the file. The count is the number
    /// of distinct bytes zeroed, and a directory is walked once.
    #[test]
    fn destroyed_bytes_never_exceed_the_file() {
        let hostile: [u8; 220] = [
            0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x16, 0x00, 0x00, 0x01, 0x2a, 0x00, 0x0f,
            0x00, 0x10, 0x02, 0x00, 0x00, 0x00, 0x02, 0x01, 0x00, 0x52, 0x51, 0x51, 0x51, 0x02,
            0x02, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x22, 0x01, 0x31, 0x00, 0x00, 0x4d, 0x4d,
            0x00, 0x2a, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02,
            0x02, 0x01, 0x00, 0x00, 0x4d, 0x00, 0x2a, 0x40, 0x02, 0x00, 0x00, 0x00, 0x00, 0x10,
            0x02, 0x22, 0x01, 0x31, 0x00, 0x02, 0x00, 0x00, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x06,
            0x02, 0x02, 0xff, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x00, 0x02, 0x01,
            0x00, 0x00, 0x4d, 0x00, 0x00, 0x00, 0x02, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
            0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x2f, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
            0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02,
            0x02, 0x01, 0x00, 0x52, 0x51, 0x51, 0x51, 0x02, 0x02, 0x00, 0x00, 0x00, 0x00, 0x10,
            0x02, 0x22, 0x01, 0x31, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0x00, 0x51, 0x02, 0x02,
            0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x22, 0x01, 0x32, 0x00, 0x00, 0x00, 0x00, 0x32,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02,
            0x01, 0x00, 0x00, 0x4d, 0x00, 0x2a, 0x40, 0x02, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02,
            0x22, 0x01, 0x31, 0x0f, 0xf7, 0x00, 0x00, 0x00, 0x00, 0x0a,
        ];
        if let Some((out, wiped)) = strip_tiff(&hostile) {
            assert_eq!(out.len(), hostile.len());
            assert!(wiped <= hostile.len(), "{wiped} bytes destroyed in a {}-byte file", hostile.len());
        }
    }
}
