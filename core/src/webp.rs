//! WebP, which is a RIFF file: a twelve byte header and then a chain of chunks,
//! each one a four byte type, a little endian payload length, the payload, and a
//! pad byte when that length is odd.
//!
//! That chain is what makes this format unlike the ISOBMFF containers in `heif`,
//! and it decides how a strip works here. Nothing in a RIFF points at another
//! part of the file by offset — the single size field sits at the front and can
//! be recomputed from whatever is kept — so a dropped chunk genuinely leaves and
//! every chunk after it still reads. The file shrinks, which the overwrite-in-
//! place used for a HEIF never does.
//!
//! What must not happen is a length field being taken at its word. A length is
//! data like any other, and a file cut short in transit keeps its front intact
//! while its last chunk declares a payload that is not all there. A walk that
//! reads past the end, or that stops short of it, has not read this file, and
//! the answer is to refuse rather than to return the part it managed to reach.

/// Chunks that carry picture, one of which every WebP must have.
///
/// `VP8 ` is a lossy frame, `VP8L` a lossless one, and `VP8X` the extended
/// header that a file with transparency, animation, or any chunk beyond the
/// bare frame begins with.
const PICTURE_CHUNKS: [&[u8; 4]; 3] = [b"VP8 ", b"VP8L", b"VP8X"];

/// Chunks a strip keeps, listed by what they hold rather than by what is
/// removed.
///
/// Writing the list this way is what lets an `EXIF`, an `XMP `, and whatever a
/// future encoder invents all leave without being named: anything not here goes,
/// so the only chunks that need a reason to stay are the ones that stay.
///
/// Besides the three picture chunks: `ALPH` is the transparency plane and is
/// picture rather than a record of one; `ANIM` and `ANMF` are the animation
/// header and its frames; and `ICCP` is the colour profile, which every mode in
/// this project preserves, because dropping it desaturates a wide-gamut
/// photograph.
const KEPT_CHUNKS: [&[u8; 4]; 7] = [b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"ICCP"];

/// Chunks whose presence means the file is an animation rather than a picture.
///
/// Either one alone identifies one. An animation the reference encoder wrote
/// carries both, but a stream that has been through an editor may have lost its
/// header chunk, and a file holding an `ANMF` is not a still whatever else is
/// missing from it.
const ANIMATION_CHUNKS: [&[u8; 4]; 2] = [b"ANIM", b"ANMF"];

/// Bits in the first byte of a `VP8X` payload saying which optional chunks the
/// file carries. A strip removes the chunks these two announce.
const VP8X_EXIF_FLAG: u8 = 0x08;
const VP8X_XMP_FLAG: u8 = 0x04;

/// Whether these bytes are a WebP.
pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Whether this WebP holds more than one frame.
///
/// Read from the chunks rather than from the animation flag in a `VP8X` header.
/// The flag is one bit in a block that a stripped or rewritten file may not have
/// updated, while a frame chunk is a thing the decoder will actually draw; the
/// presence of the chunks is the fact, and the flag is a claim about it.
pub fn is_animated(bytes: &[u8]) -> bool {
    let mut animated = false;
    // The walk's own answer is not wanted here: a chain that does not close
    // yields no chunks, which reads as a still, and the file then reaches a
    // decoder that will refuse it — the outcome a malformed file should have.
    let _ = walk(bytes, |chunk| {
        if ANIMATION_CHUNKS.iter().any(|id| id[..] == chunk.id[..]) {
            animated = true;
        }
    });
    animated
}

/// Remove a WebP's metadata, keeping every chunk that carries picture.
///
/// Returns the rebuilt file and how many payload bytes were dropped. Zero means
/// the file carried none of what a strip removes, which is a real outcome rather
/// than a failure — the caller decides whether a file that cannot shrink is
/// worth rewriting.
///
/// `None` means the file is not laid out as this module reads it, and covers
/// both a chain that does not close cleanly and a container that holds no
/// picture at all. A chain that failed partway through has already handed over
/// some of its chunks, so `kept` is only ever built on by a walk that returned
/// `Some`, and a refusal never leaves a partial file behind.
pub fn strip_webp(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut kept: Vec<&[u8]> = Vec::new();
    let mut has_picture = false;
    let mut wiped = 0;

    walk(bytes, |chunk| {
        if !KEPT_CHUNKS.iter().any(|id| id[..] == chunk.id[..]) {
            wiped += chunk.payload.len();
            return;
        }
        if PICTURE_CHUNKS.iter().any(|id| id[..] == chunk.id[..]) {
            has_picture = true;
        }
        // The whole chunk travels, header and pad byte included, so the result
        // is a chunk chain that reads exactly as the original did.
        kept.push(&bytes[chunk.at..chunk.at + chunk.span]);
    })?;

    if !has_picture {
        return None;
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(b"RIFF");
    // Filled in once the chunks are in place; the header is assembled first so
    // that the result is a file rather than a chain of chunks with a prefix.
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(b"WEBP");
    for chunk in kept {
        out.extend_from_slice(chunk);
    }
    let size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&size.to_le_bytes());

    // A `VP8X` header announces which optional chunks the file carries, and two
    // of those flags speak for chunks a strip takes away. A flag left standing
    // for a chunk that is no longer here describes a file that does not exist,
    // and a reader that believes the header rather than the chain will go
    // looking for an `EXIF` it will not find. The bits come down with the
    // chunks. `VP8X` is the first chunk when it is present at all, so its
    // payload begins at a known place: four bytes of type, four of length.
    if out.len() > 20 && &out[12..16] == b"VP8X" {
        out[20] &= !(VP8X_EXIF_FLAG | VP8X_XMP_FLAG);
    }

    Some((out, wiped))
}

/// The colour profile this WebP carries, read from its `ICCP` chunk.
///
/// Taken from the chunk rather than from the decoder, which hands back pixels
/// alone. The profile has to reach the JPEG written beside the original, because
/// a wide-gamut picture that arrives untagged is read as sRGB and comes out
/// visibly flat — every mode in this project keeps the profile for that reason.
pub fn icc_profile(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut icc = None;
    walk(bytes, |chunk| {
        if &chunk.id == b"ICCP" {
            icc = Some(chunk.payload.to_vec());
        }
    })?;
    icc
}

/// One chunk of the chain: what it is, what it carries, and where it sits.
struct Chunk<'a> {
    id: [u8; 4],
    payload: &'a [u8],
    /// Offset of the chunk's first byte, from the start of the file.
    at: usize,
    /// The chunk's whole extent as it sits in the file: the eight byte header,
    /// the payload, and the pad byte an odd length needs.
    span: usize,
}

/// Read the chunk chain, handing each chunk to `f`.
///
/// `None` means the structure does not close: a size field that disagrees with
/// the bytes present, a chunk header or payload that runs past the end, or a
/// chain that stops short of the end instead of landing on it. Every one of
/// those is a file whose lengths cannot be believed, and the caller's answer to
/// all of them is the same refusal. A walk that fails stops calling `f` partway
/// through, so a caller collecting chunks keeps only the ones that reached it.
fn walk<'a>(bytes: &'a [u8], mut f: impl FnMut(&Chunk<'a>)) -> Option<()> {
    if !is_webp(bytes) {
        return None;
    }

    // The size field counts everything after itself: the four bytes of "WEBP"
    // and every chunk that follows. A field that disagrees with what is actually
    // here is the one lie a chunk-by-chunk walk cannot see for itself, since the
    // last chunk's own length still adds up against the bytes present.
    let declared = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if declared != bytes.len() - 8 {
        return None;
    }

    let mut at = 12;
    while at < bytes.len() {
        // A header that does not fit, which is also what a stray byte after the
        // final chunk looks like.
        if at + 8 > bytes.len() {
            return None;
        }
        let size =
            u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        // The pad byte is not counted in the length, so it has to be stepped
        // over here. Landing on the header one byte early would desynchronise
        // the rest of the walk, and the final chunk would then end somewhere
        // other than the end of the file.
        let span = 8 + size + (size & 1);
        let start = at + 8;
        if start + size + (size & 1) > bytes.len() {
            return None;
        }

        let mut id = [0u8; 4];
        id.copy_from_slice(&bytes[at..at + 4]);
        f(&Chunk { id, payload: &bytes[start..start + size], at, span });
        at += span;
    }

    // Landing exactly on the end is the whole test. A walk that stopped short
    // leaves bytes behind that no chunk header accounted for, which means one of
    // the lengths above was wrong.
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assemble a WebP from `(FourCC, payload)` pairs, writing the pad byte after
    /// an odd payload and the RIFF size field from what was written.
    ///
    /// The result is a well formed file, which is what makes it usable as the
    /// control in tests that then edit one field of it to make it lie.
    fn webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&[0, 0, 0, 0]); // filled in below
        out.extend_from_slice(b"WEBP");
        for chunk in chunks {
            let (id, payload) = (chunk.0, chunk.1);
            out.extend_from_slice(&id[..]);
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                out.push(0);
            }
        }
        let size = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&size.to_le_bytes());
        out
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn a_bare_still_is_recognised_and_a_strip_leaves_it_alone() {
        let still = webp(&[(b"VP8 ", &[0x9D, 0x01, 0x2A, 0x10, 0x00, 0x00])]);
        assert!(is_webp(&still));
        assert!(!is_animated(&still));

        let (out, wiped) = strip_webp(&still).expect("a bare frame is a picture");
        assert_eq!(wiped, 0, "there was no metadata to drop");
        assert_eq!(out, still, "and nothing was dropped, so the file is unchanged");
    }

    #[test]
    fn the_location_goes_and_the_colour_profile_stays() {
        const EXIF: &[u8] = b"Exif\0\0GPS 51.5 N 0.1 W\0";
        let source = webp(&[
            (b"VP8X", &[0u8; 10]),
            (b"ICCP", b"profile bytes"),
            (b"EXIF", EXIF),
            (b"VP8 ", &[0xAA; 16]),
        ]);

        let (out, wiped) = strip_webp(&source).expect("a well formed extended file");
        assert_eq!(wiped, EXIF.len(), "the count is the dropped payload, not the dropped chunk");
        assert!(!contains(&out, b"GPS 51.5 N"), "the location survived");
        assert!(contains(&out, b"profile bytes"), "the colour profile was dropped");
        assert!(contains(&out, &[0xAA; 16]), "the picture was dropped");
        assert!(out.len() < source.len(), "the chunk left with its bytes");
    }

    #[test]
    fn a_file_whose_only_metadata_is_a_profile_reports_nothing_dropped() {
        // The profile is kept in every mode, so a file carrying one and nothing
        // else removable is a file a strip does not change. That answer has to
        // be zero rather than a refusal, because the caller reports the two
        // outcomes differently.
        let source = webp(&[
            (b"VP8X", &[0u8; 10]),
            (b"ICCP", b"a colour profile"),
            (b"VP8 ", &[0xAA; 16]),
        ]);
        let (out, wiped) = strip_webp(&source).expect("a well formed file");
        assert_eq!(wiped, 0);
        assert_eq!(out, source);
    }

    #[test]
    fn an_animation_reports_itself_and_keeps_its_frames() {
        let animated = webp(&[
            (b"VP8X", &[0u8; 10]),
            (b"ANIM", &[0u8; 6]),
            (b"ANMF", &[0u8; 20]),
            (b"EXIF", b"a location"),
        ]);
        assert!(is_animated(&animated));

        // Frames are picture, so a strip may not take them: removing a camera's
        // GPS is not licence to leave the file with one frame of many.
        let (out, wiped) = strip_webp(&animated).expect("a well formed animation");
        assert_eq!(wiped, "a location".len());
        assert!(contains(&out, b"ANIM"));
        assert!(contains(&out, b"ANMF"));

        let still = webp(&[(b"VP8X", &[0u8; 10]), (b"VP8L", &[0x2F, 0x00, 0x00, 0x00])]);
        assert!(!is_animated(&still), "a still with an extended header is not an animation");
    }

    #[test]
    fn an_odd_payload_is_padded_and_the_walk_still_closes() {
        // The pad byte is not counted by the length field, so the walk has to
        // step over it. Getting that wrong puts the next chunk header a byte
        // early and the chain ends somewhere other than the end of the file.
        // The metadata here is odd in length and sits between a chunk that
        // stays and one that stays, which is where a mis-step shows.
        const LOCATION: &[u8] = b"GPS 51.5 N, 0.1 W"; // seventeen bytes
        let source = webp(&[
            (b"VP8X", &[0u8; 10]),
            (b"EXIF", LOCATION),
            (b"VP8 ", &[0xAA; 16]),
        ]);
        assert_eq!(LOCATION.len() % 2, 1, "the fixture has to exercise a pad byte");

        let (out, wiped) = strip_webp(&source).expect("the walk lands on the end");
        assert_eq!(wiped, LOCATION.len());
        assert!(contains(&out, &[0xAA; 16]), "the frame after the padded chunk was lost");
        assert_eq!(out.len(), source.len() - LOCATION.len() - 8 - 1, "chunk and pad both left");

        // The rebuilt file has to be one this module reads as a file, not merely
        // one whose bytes happen to look right.
        let (again, wiped_again) = strip_webp(&out).expect("the rebuilt file is well formed");
        assert_eq!(again, out);
        assert_eq!(wiped_again, 0);
    }

    #[test]
    fn a_chunk_length_pointing_past_the_end_is_refused() {
        let mut lying = webp(&[(b"VP8 ", &[0xAA; 8])]);
        // The first chunk's length field, which sits four bytes past its type.
        lying[16..20].copy_from_slice(&4096u32.to_le_bytes());
        assert!(strip_webp(&lying).is_none(), "a payload that is not all there");
    }

    #[test]
    fn a_size_field_that_disagrees_with_the_bytes_present_is_refused() {
        let honest = webp(&[(b"VP8 ", &[0xAA; 8])]);
        assert!(strip_webp(&honest).is_some(), "the control must strip");

        let mut over = honest.clone();
        over[4..8].copy_from_slice(&4096u32.to_le_bytes());
        assert!(strip_webp(&over).is_none(), "a size claiming bytes that are not here");

        let mut under = honest.clone();
        under[4..8].copy_from_slice(&4u32.to_le_bytes());
        assert!(strip_webp(&under).is_none(), "a size denying chunks that are here");
    }

    #[test]
    fn a_file_truncated_mid_payload_is_refused_rather_than_half_stripped() {
        let full = webp(&[
            (b"VP8X", &[0u8; 10]),
            (b"EXIF", b"a location"),
            (b"VP8 ", &[0xAA; 16]),
        ]);
        assert!(strip_webp(&full).is_some(), "the control must strip");

        // Cut six bytes off the last payload, which also puts the size field out
        // of step with the file.
        let cut = &full[..full.len() - 6];
        assert!(strip_webp(cut).is_none(), "a size field past the bytes given");

        // The same cut with the size field corrected, which is the shape this
        // walk has to catch for itself: every length is now believable except
        // the last chunk's, whose payload runs past the end. A walk that trusted
        // it would return a picture with its bottom rows missing as a success.
        let mut repaired = cut.to_vec();
        let size = (repaired.len() - 8) as u32;
        repaired[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(strip_webp(&repaired).is_none(), "a payload running past the end");
    }

    #[test]
    fn a_container_holding_no_picture_is_not_a_picture() {
        // The header alone, declaring no chunks at all.
        let empty = b"RIFF\x04\x00\x00\x00WEBP".to_vec();
        assert!(is_webp(&empty));
        assert!(strip_webp(&empty).is_none());

        // And one holding only metadata, which a strip would leave as a shell.
        let metadata_only = webp(&[(b"EXIF", b"a location")]);
        assert!(strip_webp(&metadata_only).is_none());
    }

    #[test]
    fn the_header_stops_announcing_the_chunks_that_left() {
        // A VP8X whose flags say the file carries a colour profile, an EXIF
        // block and an XMP packet. Two of those three are about to go, and the
        // header has to stop claiming them; the profile's flag stays, because
        // the profile stays.
        let flags = VP8X_EXIF_FLAG | VP8X_XMP_FLAG | 0x20;
        let mut vp8x = [0u8; 10];
        vp8x[0] = flags;

        let source = webp(&[
            (b"VP8X", &vp8x),
            (b"ICCP", b"a colour profile"),
            (b"EXIF", b"a location"),
            (b"XMP ", b"<x:xmpmeta/>"),
            (b"VP8 ", &[0xAA; 16]),
        ]);

        let (out, wiped) = strip_webp(&source).expect("a well formed extended file");
        assert_eq!(wiped, "a location".len() + "<x:xmpmeta/>".len());

        let header = out[20];
        assert_eq!(header & VP8X_EXIF_FLAG, 0, "the EXIF flag outlived its chunk");
        assert_eq!(header & VP8X_XMP_FLAG, 0, "the XMP flag outlived its chunk");
        assert_eq!(header & 0x20, 0x20, "the profile is still here and still announced");
    }

    #[test]
    fn the_colour_profile_is_read_out_for_the_picture_written_beside_it() {
        const PROFILE: &[u8] = b"not really a profile, but these bytes travel";

        let tagged = webp(&[
            (b"VP8X", &[0u8; 10]),
            (b"ICCP", PROFILE),
            (b"VP8 ", &[0xAA; 16]),
        ]);
        assert_eq!(icc_profile(&tagged).as_deref(), Some(PROFILE));

        let untagged = webp(&[(b"VP8 ", &[0xAA; 16])]);
        assert_eq!(icc_profile(&untagged), None, "a file with no profile has none to give");

        // A file this module will not read has no profile to offer either, and
        // must not hand back one read out of a chain that does not close.
        let mut lying = tagged.clone();
        lying[4..8].copy_from_slice(&4096u32.to_le_bytes());
        assert_eq!(icc_profile(&lying), None);
    }

    #[test]
    fn something_that_is_not_a_webp_is_refused() {
        assert!(!is_webp(b"not a webp at all"));
        assert!(!is_webp(b"RIFF")); // too short to hold a form type
        assert!(!is_webp(b"GIF89a\x0a\x00\x03\x00\x00\x00\x00\x00"));
        // A RIFF of a different kind: the same header, another format inside.
        assert!(!is_webp(b"RIFF\x04\x00\x00\x00WAVE"));
        assert!(strip_webp(b"not a webp at all, not even close").is_none());
    }
}
