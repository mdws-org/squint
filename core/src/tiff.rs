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
/// way up the picture goes, the other how to read its colour.
const REMOVE: [u16; 11] = [
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
];

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
    fn numbers(&self, entry: usize, kind: u16, count: u32) -> Vec<u64> {
        let width = type_size(kind);
        if width == 0 || count == 0 {
            return Vec::new();
        }
        let at = match self.data_at(entry, kind, count) {
            Some((at, _)) => at,
            None => entry + 8,
        };
        (0..count as usize)
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
fn protected(reader: &Reader, first: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    let mut queue = vec![first];

    while let Some(at) = queue.pop() {
        if at == 0 || seen.contains(&at) || seen.len() > 8 {
            continue;
        }
        seen.push(at);
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
            if *tag == ICC {
                if let Some((at, size)) = reader.data_at(*entry, *kind, *count) {
                    out.push((at, at + size));
                }
            }
            if SUB_IFDS.contains(tag) {
                if let Some(sub) = reader.u32(entry + 8) {
                    queue.push(sub as usize);
                }
            }
        }

        if let Some(next) = reader.u32(at + 2 + entries.len() * 12) {
            queue.push(next as usize);
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
    let keep_clear = protected(&reader, first);
    let safe = |start: usize, end: usize| {
        !keep_clear.iter().any(|(ps, pe)| start < *pe && *ps < end)
    };

    let mut out = bytes.to_vec();
    let mut wiped = 0usize;
    let mut directory = first;
    let mut visited = 0;

    while directory != 0 && visited < 8 {
        visited += 1;
        let entries = reader.entries(directory);
        if entries.is_empty() {
            break;
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
                            if safe(at, at + size) {
                                out[at..at + size].fill(0);
                                wiped += size;
                            }
                        }
                    }
                    let end = sub + 2 + reader.entries(sub).len() * 12 + 4;
                    if end <= out.len() && safe(sub, end) {
                        out[sub..end].fill(0);
                        wiped += end - sub;
                    }
                }
            } else if let Some((at, size)) = reader.data_at(*entry, *kind, *count) {
                if safe(at, at + size) {
                    out[at..at + size].fill(0);
                    wiped += size;
                }
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

        directory = next as usize;
    }

    Some((out, wiped))
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
}
