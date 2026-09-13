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
            // Trailer
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

            let mut is_first_sub_block = true;
            let mut is_netscape = false;

            while pos < bytes.len() {
                let sub_len = bytes[pos] as usize;
                pos += 1;
                if sub_len == 0 {
                    break;
                }
                if pos + sub_len > bytes.len() {
                    return None;
                }
                if is_first_sub_block {
                    if label == 0xFF && sub_len >= 11 {
                        if &bytes[pos..pos + 11] == b"NETSCAPE2.0" {
                            is_netscape = true;
                        }
                    }
                    is_first_sub_block = false;
                }
                pos += sub_len;
            }

            // Must end cleanly on the zero-length sub-block terminator
            if pos > bytes.len() || bytes[pos - 1] != 0x00 {
                return None;
            }

            // Keep Graphic Control Extension (0xF9) and the NETSCAPE2.0 loop block (0xFF).
            let keep = (label == 0xF9) || (label == 0xFF && is_netscape);
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
            while pos < bytes.len() {
                let sub_len = bytes[pos] as usize;
                pos += 1;
                if sub_len == 0 {
                    break;
                }
                if pos + sub_len > bytes.len() {
                    return None;
                }
                pos += sub_len;
            }
            if pos > bytes.len() || bytes[pos - 1] != 0x00 {
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
