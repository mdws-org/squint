//! GIF parsing and metadata stripping.
//!
//! Squint does not encode GIF; this module exists only so `Mode::Strip` can
//! remove metadata blocks (comments, XMP, ICC, etc.) from GIF files while
//! preserving picture data, frame delays, and animation loop counts.

/// Check whether the slice starts with a GIF signature ("GIF87a" or "GIF89a").
pub fn is_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

/// Strip non-essential extension blocks from a GIF stream.
///
/// Returns `Some((output_bytes, bytes_removed))` on success, or `None` if the
/// stream is malformed or does not reach the trailer cleanly.
pub fn strip_gif(bytes: &[u8]) -> Option<(Vec<u8>, usize)> {
    if !is_gif(bytes) {
        return None;
    }

    // A GIF begins with a 6-byte header and a 7-byte logical screen descriptor.
    if bytes.len() < 13 {
        return None;
    }

    let packed = bytes[10];
    let gct_flag = (packed & 0x80) != 0;
    let gct_size_power = (packed & 0x07) + 1;
    let gct_len = if gct_flag {
        3 * (1usize << gct_size_power)
    } else {
        0
    };

    let header_and_lsd_end = 13 + gct_len;
    if bytes.len() < header_and_lsd_end {
        return None;
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..header_and_lsd_end]);

    let mut pos = header_and_lsd_end;
    let mut trailer_reached = false;

    while pos < bytes.len() {
        let block_type = bytes[pos];
        if block_type == 0x3B {
            // The trailer is the last byte of a GIF. Requiring that, rather
            // than merely finding a 0x3B, is what makes this walk structural
            // instead of lucky: a colour table whose declared size is wrong
            // sends the walk into compressed data, where a 0x00 will
            // eventually pass for a sub-block terminator and some later byte
            // for this trailer. The walk would then return a truncated picture
            // as a success, which is the shape of the 2026-08-19 defect. A
            // desync leaves bytes behind, and bytes left over mean the file is
            // not laid out the way it was read.
            if pos + 1 != bytes.len() {
                return None;
            }
            out.push(0x3B);
            trailer_reached = true;
            break;
        } else if block_type == 0x21 {
            // Extension block: introducer 0x21, label byte, followed by sub-blocks.
            if pos + 2 > bytes.len() {
                return None;
            }
            let label = bytes[pos + 1];
            let ext_start = pos;
            pos += 2;

            let mut first_sub_block = true;
            // An application extension is identified by exactly eleven bytes in
            // its first sub-block. Accepting a longer one that merely starts
            // with the name would let a file carry anything it likes through a
            // stripper by prefixing it with a name the stripper keeps.
            let mut application = None;
            let mut terminated = false;

            while pos < bytes.len() {
                let sub_len = bytes[pos] as usize;
                pos += 1;
                if sub_len == 0 {
                    terminated = true;
                    break;
                }
                if pos + sub_len > bytes.len() {
                    return None;
                }
                if first_sub_block {
                    if label == 0xFF && sub_len == 11 {
                        application = Some(&bytes[pos..pos + 11]);
                    }
                    first_sub_block = false;
                }
                pos += sub_len;
            }
            if !terminated {
                return None;
            }

            // Kept: the Graphic Control Extension, which carries frame delay
            // and transparency; the Plain Text Extension, which a viewer draws
            // into the frame and so is picture rather than a record of one; the
            // loop count, without which an animation plays once; and the colour
            // profile, because dropping it desaturates a wide-gamut picture and
            // keeping it is what this project does in every other format.
            //
            // Everything else goes without being named, which is how a comment,
            // XMP, and whatever a future encoder invents all leave.
            let keep = matches!(label, 0xF9 | 0x01)
                || matches!(application, Some(b"NETSCAPE2.0") | Some(b"ICCRGBG1012"));
            if keep {
                out.extend_from_slice(&bytes[ext_start..pos]);
            }
        } else if block_type == 0x2C {
            // Image separator 0x2C, 9-byte descriptor, optional local colour table,
            // 1-byte LZW minimum code size, then sub-blocks.
            let img_start = pos;
            if pos + 10 > bytes.len() {
                return None;
            }
            let packed_img = bytes[pos + 9];
            let lct_flag = (packed_img & 0x80) != 0;
            let lct_size_power = (packed_img & 0x07) + 1;
            let lct_len = if lct_flag {
                3 * (1usize << lct_size_power)
            } else {
                0
            };
            pos += 10 + lct_len;
            if pos >= bytes.len() {
                return None;
            }
            // LZW minimum code size byte
            pos += 1;

            // Image data sub-blocks ending in 0x00
            let mut terminated = false;
            while pos < bytes.len() {
                let sub_len = bytes[pos] as usize;
                pos += 1;
                if sub_len == 0 {
                    terminated = true;
                    break;
                }
                if pos + sub_len > bytes.len() {
                    return None;
                }
                pos += sub_len;
            }
            if !terminated {
                return None;
            }

            out.extend_from_slice(&bytes[img_start..pos]);
        } else {
            // Unrecognized block; desynced walk cannot guarantee complete output
            return None;
        }
    }

    if !trailer_reached {
        return None;
    }

    let bytes_removed = bytes.len() - out.len();
    Some((out, bytes_removed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GIF built from parts, so a test can lie about one field at a time.
    fn gif_with(extensions: &[Vec<u8>], lct_size_power: Option<u8>) -> Vec<u8> {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&[0x02, 0x00, 0x02, 0x00, 0x80, 0x00, 0x00]);
        g.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
        for e in extensions {
            g.extend_from_slice(e);
        }
        g.push(0x2C);
        g.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x02, 0x00]);
        // The packed byte declares whether a local colour table follows and how
        // big it is. A test can overstate it to send the walk into the pixels.
        g.push(match lct_size_power {
            Some(power) => 0x80 | power,
            None => 0x00,
        });
        if let Some(power) = lct_size_power {
            g.extend(std::iter::repeat_n(0x11, 3 * (1usize << (power + 1))));
        }
        g.push(0x02);
        g.extend_from_slice(&[0x03, 0x44, 0x00, 0x3B, 0x00]);
        g.push(0x3B);
        g
    }

    fn app_extension(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut e = vec![0x21, 0xFF, id.len() as u8];
        e.extend_from_slice(id);
        e.push(payload.len() as u8);
        e.extend_from_slice(payload);
        e.push(0x00);
        e
    }

    #[test]
    fn a_lying_colour_table_size_is_refused_rather_than_truncated() {
        // The declared table is larger than the one written, so the walk lands
        // inside the compressed data, where the bytes above offer a plausible
        // terminator and trailer. Accepting that would return a picture with
        // its pixels cut off as a success.
        let honest = gif_with(&[], Some(0));
        assert!(strip_gif(&honest).is_some(), "the control must strip");
        // The packed byte is the tenth of the image descriptor, which follows
        // the image separator. Found rather than counted, so the test keeps
        // aiming at the right byte when the fixture changes.
        let separator = honest.iter().position(|&b| b == 0x2C).expect("an image block");
        let packed_at = separator + 9;
        let mut lying = honest;
        assert_eq!(lying[packed_at] & 0x80, 0x80, "the test is aimed at the packed byte");
        lying[packed_at] = 0x80 | 0x02;
        assert!(strip_gif(&lying).is_none());
    }

    #[test]
    fn bytes_after_the_trailer_are_refused() {
        let mut trailing = gif_with(&[], Some(0));
        trailing.extend_from_slice(b"appended");
        assert!(strip_gif(&trailing).is_none());
    }

    #[test]
    fn the_colour_profile_and_the_loop_count_survive_and_a_comment_does_not() {
        let comment = {
            let mut c = vec![0x21, 0xFE, 12];
            c.extend_from_slice(b"123 Elm St  ");
            c.push(0x00);
            c
        };
        let source = gif_with(
            &[
                comment,
                app_extension(b"NETSCAPE2.0", &[0x01, 0x00, 0x00]),
                app_extension(b"ICCRGBG1012", b"profile bytes"),
            ],
            Some(0),
        );
        let (out, wiped) = strip_gif(&source).expect("walks cleanly");
        assert!(!out.windows(10).any(|w| w == b"123 Elm St"), "the comment survived");
        assert!(out.windows(11).any(|w| w == b"NETSCAPE2.0"), "the loop count was dropped");
        assert!(out.windows(11).any(|w| w == b"ICCRGBG1012"), "the colour profile was dropped");
        assert!(wiped > 0);
    }

    #[test]
    fn an_application_name_with_a_payload_stuck_to_it_is_not_kept() {
        // Eleven bytes identify an application extension. A first sub-block
        // longer than that is something else wearing the name, and keeping it
        // would carry whatever it holds through a stripper.
        let mut spoof = vec![0x21, 0xFF, 25];
        spoof.extend_from_slice(b"NETSCAPE2.0gps 51.5,-0.12");
        spoof.push(0x00);
        let source = gif_with(&[spoof], Some(0));
        let (out, _) = strip_gif(&source).expect("walks cleanly");
        assert!(!out.windows(3).any(|w| w == b"gps"), "a spoofed name kept its payload");
    }

    fn build_test_gif() -> Vec<u8> {
        let mut gif = Vec::new();
        // Header: GIF89a
        gif.extend_from_slice(b"GIF89a");
        // LSD: 1x1, GCT flag (0x80), 2 colors (0x00), bg=0, aspect=0
        gif.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00]);
        // GCT: 2 colors (black, white)
        gif.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
        // NETSCAPE2.0 Application extension (keep)
        // 0x21 0xFF 0x0B NETSCAPE2.0 0x03 0x01 0x00 0x00 0x00
        gif.extend_from_slice(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01\x00\x00\x00");
        // Comment extension (remove)
        // 0x21 0xFE 0x05 hello 0x00
        gif.extend_from_slice(b"\x21\xFE\x05hello\x00");
        // Image descriptor + data: 1x1 image
        // 0x2C, left 0, top 0, width 1, height 1, no LCT (0x00)
        // LZW min code size 2 (0x02)
        // Sub-block len 2: 0x4C 0x01, then terminator 0x00
        gif.extend_from_slice(b"\x2C\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x4C\x01\x00");
        // Trailer
        gif.push(0x3B);
        gif
    }

    #[test]
    fn test_strip_gif_preserves_netscape_and_removes_comment() {
        let gif = build_test_gif();
        let (stripped, bytes_removed) = strip_gif(&gif).expect("strip_gif succeeded");

        assert!(bytes_removed > 0, "bytes were removed");
        assert_eq!(bytes_removed, gif.len() - stripped.len());

        // Check comment was removed
        assert!(!stripped.windows(5).any(|w| w == b"hello"));

        // Check NETSCAPE2.0 was kept
        assert!(stripped.windows(11).any(|w| w == b"NETSCAPE2.0"));

        // Check image block was kept
        assert!(stripped.windows(15).any(|w| w == b"\x2C\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x4C\x01\x00"));

        // Check reaches trailer
        assert_eq!(stripped.last(), Some(&0x3B));
    }

    #[test]
    fn test_strip_gif_truncated_before_trailer_returns_none() {
        let mut gif = build_test_gif();
        // Remove trailer
        gif.pop();
        assert!(strip_gif(&gif).is_none());
    }
}
