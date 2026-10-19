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
const REMOVE: [u16; 42] = [
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

/// Whether a JPEG segment can name a person, a place, a device or a moment:
/// EXIF and XMP (APP1), the vendor, Photoshop and content-credential segments
/// (APP3 to APP13, APP15), and a comment holding text. JFIF (APP0), the
/// colour profile and the Multi-Picture index (APP2), Adobe's colour
/// transform (APP14), and binary filler in a comment carry none of that.
fn segment_carries_metadata(marker: u8, payload: &[u8]) -> bool {
    match marker {
        0xE0 => false,
        0xE2 => !(payload.starts_with(b"ICC_PROFILE\0") || payload.starts_with(b"MPF\0")),
        0xEE => !(payload.len() == 12 && payload.starts_with(b"Adobe")),
        0xE1 | 0xE3..=0xED | 0xEF => true,
        0xFE => payload.windows(4).any(|w| w.iter().all(|b| b.is_ascii_graphic() || *b == b' ')),
        _ => false, // tables, frame and scan headers
    }
}

/// Whether the JPEG stream at `start..end` carries metadata in its own
/// segments, before the first scan, between scans, or in a second image a
/// Multi-Picture file puts after the first. Such a stream is pixels to this
/// format and cannot be changed, so a preview carrying one would leave it in
/// a file reported clean.
///
/// A stream that breaks before its first scan is `Scan::Broken`, refused
/// like a file laid out wrongly: a broken segment there could hide the ones
/// after it. One that breaks inside its scans is read as far as it goes:
/// some cameras write their raw sensor data as JPEG scans laid out in their
/// own way. Bytes that do not open as a JPEG are not one.
///
/// `budget` is what one walk of a file may still read. Strips can name the
/// same bytes many times, and a stream of fill bytes is read one at a time,
/// so a file built to be read for hours runs it out and `None` refuses it.
fn jpeg_carries_metadata(bytes: &[u8], start: usize, end: usize, budget: &mut usize) -> Option<Scan> {
    let end = end.min(bytes.len());
    if start.saturating_add(2) > end || bytes[start..start + 2] != [0xFF, 0xD8] {
        return Some(Scan::Clean);
    }
    let broken = |scanning: bool| Some(if scanning { Scan::Clean } else { Scan::Broken });
    let mut at = start + 2;
    let mut scanning = false;
    while at + 1 < end {
        let before = at;
        if bytes[at] != 0xFF {
            return broken(scanning);
        }
        let marker = bytes[at + 1];
        match marker {
            0xFF => at += 1, // a fill byte before the marker
            0x01 | 0xD0..=0xD7 => at += 2,
            0xD9 => {
                // A second image, such as a gain map, can follow, after
                // padding.
                let mut next = at + 2;
                while next < end && (bytes[next] == 0x00 || (bytes[next] == 0xFF && bytes.get(next + 1) == Some(&0xFF))) {
                    next += 1;
                }
                if next + 2 <= end && bytes[next..next + 2] == [0xFF, 0xD8] {
                    at = next + 2;
                    scanning = false;
                } else {
                    return Some(Scan::Clean);
                }
            }
            0xD8 => return broken(scanning), // a start of image inside one
            _ => {
                if at + 4 > end {
                    return broken(scanning);
                }
                let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
                if length < 2 || at + 2 + length > end {
                    return broken(scanning);
                }
                if marker == 0xDA {
                    // The scan header, then entropy-coded data up to the next
                    // marker: a literal FF is stored as FF00, and restarts,
                    // TEM and fill bytes may appear inside it.
                    scanning = true;
                    let mut next = at + 2 + length;
                    while next + 1 < end
                        && (bytes[next] != 0xFF || matches!(bytes[next + 1], 0x00 | 0x01 | 0xFF | 0xD0..=0xD7))
                    {
                        next += 1;
                    }
                    at = next;
                } else if segment_carries_metadata(marker, &bytes[at + 4..at + 2 + length]) {
                    return Some(Scan::Carries);
                } else {
                    at += 2 + length;
                }
            }
        }
        *budget = budget.checked_sub(at - before)?;
    }
    broken(scanning)
}

/// What a JPEG walk found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// Nothing to remove, or a break inside the scans, past which a raw
    /// file's own layout is not followed.
    Clean,
    /// A segment holding metadata.
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

/// Read the JPEG stream at `start..end` once: refused as a preview when it
/// carries metadata, as a layout when it breaks before its first scan or the
/// walk's budget runs out.
fn check_jpeg(
    bytes: &[u8],
    start: usize,
    end: usize,
    scanned: &mut std::collections::HashSet<(usize, usize)>,
    budget: &mut usize,
) -> Result<(), Refused> {
    if !scanned.insert((start, end)) {
        return Ok(());
    }
    match jpeg_carries_metadata(bytes, start, end, budget) {
        Some(Scan::Clean) => Ok(()),
        Some(Scan::Carries) => Err(Refused::Preview),
        Some(Scan::Broken) | None => Err(Refused::Layout),
    }
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
/// pixels. `Refused::Preview` when a JPEG among the pixels carries metadata,
/// which would stay in a file reported clean.
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
    // such a file is refused.
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
    // Every entry and every strip is at least two bytes of the file unless
    // directories share them, so a walk reading far more than the file holds
    // is a file built to exhaust time or memory.
    let mut entry_budget = reader.bytes.len() / 12 + 1;
    let mut pieces = reader.bytes.len() / 2 + 3 * MAX_DIRECTORIES;

    while let Some((at, is_page)) = queue.pop() {
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
                    check_jpeg(reader.bytes, start, end, &mut scanned, &mut jpeg_budget)?;
                }
                out.push((start, end));
            }
        }
        if let Some((e, _, k, n)) = find(JPEG_TABLES) {
            if let Some((start, size)) = reader.data_at(e, k, n) {
                check_jpeg(reader.bytes, start, start + size, &mut scanned, &mut jpeg_budget)?;
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
                    if is_page && RESOLUTION.contains(tag) {
                        resolutions.push((at, at + size));
                    } else {
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
            if let Some(next) = reader.u32(at + 2 + entries.len() * 12) {
                if queued.insert((next as usize, true)) {
                    queue.push((next as usize, true));
                }
            }
        }
    }
    extents.sort_unstable();
    if extents.windows(2).any(|w| w[1].0 < w[0].1) {
        return Err(Refused::Layout);
    }
    let all = merged(out.iter().chain(resolutions.iter()).copied().collect());
    Ok(Kept { all, strict: merged(out) })
}

/// The bytes Strip promises never to change in a file it strips: the pixels,
/// the colour profile, and what kept entries other than a resolution point at.
/// `None` where `strip_tiff` would refuse before writing. The fuzz target
/// checks a successful strip against it.
#[doc(hidden)]
pub fn unchangeable(bytes: &[u8]) -> Option<Vec<(usize, usize)>> {
    if !is_tiff(bytes) {
        return None;
    }
    let reader = Reader { bytes, big_endian: bytes[0] == b'M' };
    Some(protected(&reader, reader.u32(4)? as usize).ok()?.strict)
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
    /// A JPEG among its pixels, usually the embedded preview, carries
    /// metadata, which Strip cannot remove without rewriting that JPEG.
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
    // Pages still to strip: the chain from the header, and every directory a
    // page lists in SubIFDs. Each offset is queued once.
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

    /// An embedded preview is a JPEG in its own right and can carry its own
    /// EXIF, which Strip cannot remove without changing the preview's bytes.
    /// The file is refused rather than reported clean with it inside.
    #[test]
    fn a_preview_carrying_exif_is_refused() {
        let preview = jpeg(&[(0xE1, b"Exif\0\0MM\0\x2A GPS 51.5074 N")]);
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&preview)), Err(Refused::Preview));
    }

    #[test]
    fn a_preview_carrying_xmp_is_refused() {
        let preview = jpeg(&[(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>")]);
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&preview)), Err(Refused::Preview));
    }

    #[test]
    fn a_preview_carrying_iptc_is_refused() {
        let preview = jpeg(&[(0xED, b"Photoshop 3.0\08BIM\x04\x04 By-line")]);
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&preview)), Err(Refused::Preview));
    }

    /// The same EXIF in a page whose strips are JPEG-compressed.
    #[test]
    fn jpeg_compressed_strips_carrying_exif_are_refused() {
        let strip = jpeg(&[(0xE1, b"Exif\0\0II\x2A\0 Acme Camera Co")]);
        let tiff = tiff_from(&[&strip], &[(0x0103, 3, 1, 7), (0x0111, 4, 1, 8), (0x0117, 4, 1, strip.len() as u32)]);
        assert_eq!(strip_tiff_checked(&tiff), Err(Refused::Preview));
    }

    /// And in the tables a JPEG-compressed page shares between its strips.
    #[test]
    fn jpeg_tables_carrying_exif_are_refused() {
        let tables = jpeg(&[(0xE1, b"Exif\0\0II\x2A\0 Acme Camera Co")]);
        let pixels_at = 8 + tables.len();
        let tiff = tiff_from(
            &[&tables, &[0xAA; 16]],
            &[
                (0x0103, 3, 1, 7),
                (0x0111, 4, 1, pixels_at as u32),
                (0x0117, 4, 1, 16),
                (0x015B, 7, tables.len() as u32, 8),
            ],
        );
        assert_eq!(strip_tiff_checked(&tiff), Err(Refused::Preview));
    }

    /// A preview with only the segments a decoder needs, JFIF, a colour
    /// profile, Adobe's colour transform and its tables, carries nothing to
    /// remove; nor does a progressive one with restarts and fill bytes in its
    /// scans.
    #[test]
    fn previews_with_nothing_to_remove_still_strip() {
        let plain = jpeg(&[
            (0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"),
            (0xE2, b"ICC_PROFILE\0\x01\x01 profile"),
            (0xEE, b"Adobe\0\x64\0\0\0\0\x01"),
            (0xDB, &[0; 65]),
        ]);
        assert!(strip_tiff(&tiff_with_preview(&plain)).is_some());
        // Pentax's empty APP0, the Multi-Picture index, Blackmagic's filler.
        let quirks = jpeg(&[(0xE0, b"\0\0"), (0xE2, b"MPF\0MM\0\x2A\0\0\0\x08"), (0xFE, &[0x99; 4])]);
        assert!(strip_tiff(&tiff_with_preview(&quirks)).is_some());
        let mut progressive = jpeg(&[(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0")]);
        progressive.truncate(progressive.len() - 2);
        progressive.extend_from_slice(&[0x44, 0xFF, 0x00, 0x55, 0xFF, 0xD0, 0x66, 0xFF, 0xFF, 0xDA, 0x00, 0x02, 0x77, 0xFF, 0xD9]);
        assert!(strip_tiff(&tiff_with_preview(&progressive)).is_some());
    }

    /// A progressive preview can carry XMP between its scans, past where a
    /// walk stopping at the first scan would look; Squint's JPEG Strip
    /// removes it there.
    #[test]
    fn metadata_between_a_previews_scans_is_refused() {
        let mut preview = jpeg(&[]);
        preview.truncate(preview.len() - 2); // before EOI
        preview.extend_from_slice(&[0x44, 0xFF, 0x00, 0xFF, 0xD3, 0x55, 0xFF, 0x01, 0x66, 0xFF, 0xFF]);
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<exif:GPSLatitude>51.5</exif:GPSLatitude>";
        preview.extend_from_slice(&[0xFF, 0xE1]);
        preview.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
        preview.extend_from_slice(xmp);
        preview.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x66, 0xFF, 0xD9]);
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&preview)), Err(Refused::Preview));
    }

    /// Whatever Squint's JPEG Strip removes counts: a comment, the older
    /// Photoshop identifier, content credentials (APP11), Ducky (APP12), and
    /// XMP's extension segment.
    #[test]
    fn every_segment_jpeg_strip_removes_is_refused_in_a_preview() {
        let segments: [(u8, &[u8]); 5] = [
            (0xFE, b"taken at home"),
            (0xED, b"Adobe_Photoshop2.5:\x1c\x02\x50 By-line"),
            (0xEB, b"JP\0\x01 c2pa"),
            (0xEC, b"Ducky\0\x01"),
            (0xE1, b"http://ns.adobe.com/xmp/extension/\0 GPS"),
        ];
        for segment in segments {
            assert_eq!(strip_tiff_checked(&tiff_with_preview(&jpeg(&[segment]))), Err(Refused::Preview), "segment {:02X} passed", segment.0);
        }
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

    /// A Multi-Picture preview puts a second image, such as a gain map, after
    /// the first one's end; its own metadata counts.
    #[test]
    fn a_second_image_carrying_exif_is_refused() {
        let pair = [jpeg(&[(0xE2, b"MPF\0MM\0\x2A")]), jpeg(&[(0xE1, b"Exif\0\0MM\0\x2A GPS")])].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&pair)), Err(Refused::Preview));
        let clean = [jpeg(&[(0xE2, b"MPF\0MM\0\x2A")]), jpeg(&[])].concat();
        assert!(strip_tiff(&tiff_with_preview(&clean)).is_some());
    }

    /// The second image need not follow the first's end directly.
    #[test]
    fn a_second_image_after_padding_is_read() {
        let pair = [jpeg(&[(0xE2, b"MPF\0MM\0\x2A")]), vec![0, 0, 0xFF, 0xFF], jpeg(&[(0xE1, b"Exif\0\0MM\0\x2A GPS")])].concat();
        assert_eq!(strip_tiff_checked(&tiff_with_preview(&pair)), Err(Refused::Preview));
    }

    /// Two references to one start can end in different places: a strip
    /// ending inside the first scan claims nothing, and the embedded JPEG
    /// running on to a second image must still be read.
    #[test]
    fn jpeg_reads_are_kept_apart_by_where_they_end() {
        let stream = [jpeg(&[]), jpeg(&[(0xE1, b"Exif\0\0MM\0\x2A GPS")])].concat();
        let tiff = tiff_from(
            &[&stream],
            &[(0x0103, 3, 1, 7), (0x0111, 4, 1, 8), (0x0117, 4, 1, 7), (0x0201, 4, 1, 8), (0x0202, 4, 1, stream.len() as u32)],
        );
        assert_eq!(strip_tiff_checked(&tiff), Err(Refused::Preview));
    }

    /// Old-style JPEG (6) and DNG's lossy JPEG (34892) strips, and JPEG tiles.
    #[test]
    fn every_jpeg_compression_and_tiles_are_read() {
        let piece = jpeg(&[(0xE1, b"Exif\0\0II\x2A\0 Acme Camera Co")]);
        for compression in [6, 34892] {
            let tiff = tiff_from(&[&piece], &[(0x0103, 3, 1, compression), (0x0111, 4, 1, 8), (0x0117, 4, 1, piece.len() as u32)]);
            assert_eq!(strip_tiff_checked(&tiff), Err(Refused::Preview), "compression {compression} passed");
        }
        let tiles = tiff_from(&[&piece], &[(0x0103, 3, 1, 7), (0x0144, 4, 1, 8), (0x0145, 4, 1, piece.len() as u32)]);
        assert_eq!(strip_tiff_checked(&tiles), Err(Refused::Preview), "a JPEG tile passed");
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
        assert_eq!(jpeg_carries_metadata(&piece, 0, piece.len(), &mut 100), None);
        assert_eq!(jpeg_carries_metadata(&piece, 0, piece.len(), &mut 10_000), Some(Scan::Clean));
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
