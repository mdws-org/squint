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
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
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
const REMOVE: [u16; 32] = [
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

/// Pairs of tags giving where the picture is and how long each piece runs.
const PIXEL_RANGES: [(u16, u16); 3] = [
    (0x0111, 0x0117), // strips
    (0x0144, 0x0145), // tiles
    (0x0201, 0x0202), // an embedded JPEG
];

/// The colour profile, which is data this must protect rather than remove.
const ICC: u16 = 0x8773;

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

/// Every region the picture occupies, gathered before anything is written.
///
/// A tag's declared size cannot be trusted to stay inside the metadata. On a
/// real photograph the Exif directory sits at offset 8 and one of its entries
/// reaches past 2606, which is where the first image strip begins; zeroing on
/// the strength of the declaration alone destroyed the picture.
///
/// `None` when the directories run past `MAX_DIRECTORIES`: a page left out of
/// this walk has no protection, so stripping it could zero its pixels.
fn protected(reader: &Reader, first: usize) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Each directory with whether it is a page. A page's kept entries point
    // at data the file still needs; an Exif, GPS or interoperability directory
    // is removed whole, so nothing in it is kept.
    let mut queue = vec![(first, true)];

    while let Some((at, is_page)) = queue.pop() {
        if at == 0 || seen.contains(&at) {
            continue;
        }
        if seen.len() >= MAX_DIRECTORIES {
            return None;
        }
        seen.insert(at);
        let entries = reader.entries(at);

        for (offsets_tag, lengths_tag) in PIXEL_RANGES {
            let find = |want: u16| entries.iter().find(|(_, tag, _, _)| *tag == want).copied();
            if let (Some((oe, _, ok, on)), Some((le, _, lk, ln))) =
                (find(offsets_tag), find(lengths_tag))
            {
                let offsets = reader.numbers(oe, ok, on);
                let lengths = reader.numbers(le, lk, ln);
                for (o, l) in offsets.iter().zip(lengths.iter()) {
                    out.push((*o as usize, (*o + *l) as usize));
                }
            }
        }

        for (entry, tag, kind, count) in &entries {
            // The profile, and on a page every entry that survives the strip:
            // strip offsets and counts, a colour map, tile tables. An overrun
            // from a removed entry must not zero what a kept one points at.
            let kept = is_page && !REMOVE.contains(tag) && !SUB_IFDS.contains(tag);
            if *tag == ICC || kept {
                if let Some((at, size)) = reader.data_at(*entry, *kind, *count) {
                    out.push((at, at + size));
                }
            }
            if SUB_IFDS.contains(tag) {
                if let Some(sub) = reader.u32(entry + 8) {
                    queue.push((sub as usize, false));
                }
            }
        }

        if let Some(next) = reader.u32(at + 2 + entries.len() * 12) {
            queue.push((next as usize, is_page));
        }
    }
    Some(merged(out))
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
    if !is_tiff(bytes) {
        return None;
    }
    let reader = Reader { bytes, big_endian: bytes[0] == b'M' };
    let first = reader.u32(4)? as usize;
    let keep_clear = protected(&reader, first)?;

    let mut out = bytes.to_vec();
    let mut destroyed = Destroyed::default();
    let mut directory = first;
    let mut visited = std::collections::HashSet::new();

    // Every page is walked. Stopping early left the later pages' tags in place
    // while the file was reported clean, so running out of bound refuses.
    while directory != 0 && !visited.contains(&directory) {
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

        let mut kept: Vec<u8> = Vec::with_capacity(entries.len() * 12);
        for (entry, tag, kind, count) in &entries {
            let unwanted = REMOVE.contains(tag) || SUB_IFDS.contains(tag);
            if !unwanted {
                kept.extend_from_slice(&bytes[*entry..*entry + 12]);
                continue;
            }

            // Destroy what the entry pointed at, so that dropping the entry
            // does not merely hide the value from a parser.
            if SUB_IFDS.contains(tag) {
                if let Some(sub) = reader.u32(entry + 8).map(|v| v as usize) {
                    for (sub_entry, _, sub_kind, sub_count) in reader.entries(sub) {
                        if let Some((at, size)) = reader.data_at(sub_entry, sub_kind, sub_count) {
                            destroyed.wipe_outside(&mut out, &keep_clear, at, at + size);
                        }
                    }
                    let end = sub + 2 + reader.entries(sub).len() * 12 + 4;
                    if end <= out.len() {
                        destroyed.wipe_outside(&mut out, &keep_clear, sub, end);
                    }
                }
            } else if let Some((at, size)) = reader.data_at(*entry, *kind, *count) {
                destroyed.wipe_outside(&mut out, &keep_clear, at, at + size);
            }
        }

        // Write the directory back shorter. What follows it is no longer read,
        // because the count says how many entries there are and the pointer to
        // the next directory now sits earlier.
        let count = (kept.len() / 12) as u16;
        let order = if reader.big_endian { u16::to_be_bytes } else { u16::to_le_bytes };
        let order32 = if reader.big_endian { u32::to_be_bytes } else { u32::to_le_bytes };
        out[directory..directory + 2].copy_from_slice(&order(count));
        out[directory + 2..directory + 2 + kept.len()].copy_from_slice(&kept);
        let tail = directory + 2 + kept.len();
        out[tail..tail + 4].copy_from_slice(&order32(next));
        // The slots the dropped entries leave after the new end still hold
        // their bytes, and a value of four bytes or fewer lives in its slot.
        destroyed.wipe(&mut out, tail + 4, directory + 2 + entries.len() * 12 + 4);

        directory = next as usize;
    }

    Some((out, destroyed.bytes))
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
