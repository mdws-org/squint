//! One way in for every picture the engine can re-encode.
//!
//! JPEG and PNG are decoded by the `image` crate; a HEIF is handed to the
//! system's Image I/O, which is the only HEVC decoder on the machine. Whichever
//! path a picture takes, it arrives here the same way: RGB8 in its own colour
//! space, orientation baked in, capped to the long edge that was asked for.
//! Both sides of a perceptual comparison must come through this one door, since
//! a reference and a candidate that were decoded differently do not score
//! against each other honestly.

use crate::{extract_icc, extract_orientation, Error, Image};
#[cfg(target_os = "macos")]
use crate::{capped, MAX_PIXELS};

pub struct Source {
    /// Pixels with the EXIF orientation already baked in, capped to `max_dimension` (Lanczos, never enlarged).
    pub image: Image,
    pub icc: Option<Vec<u8>>,
    /// The EXIF orientation that was applied, 1 when the picture was already upright.
    pub orientation: u16,
    /// Whether a HEIF declared an HDR gain map. Always false for JPEG and PNG,
    /// whose maps the caller detects from the bytes.
    pub has_gain_map: bool,
    /// Set when the output format will differ from the input's: `Some("HEIC")` for a HEIF source. None for JPEG and PNG.
    pub converted_from: Option<&'static str>,
}

impl Source {
    pub fn open(bytes: &[u8], max_dimension: Option<u32>) -> Result<Source, Error> {
        if crate::heif::is_heif(bytes) {
            #[cfg(target_os = "macos")]
            {
                if !crate::heif::is_complete(bytes) {
                    return Err(Error::Decode("this HEIC is cut short: its index points past the end of the file".into()));
                }
                let decoded = crate::imageio::decode(bytes, MAX_PIXELS)?;
                let rgb_buf = image::RgbImage::from_raw(
                    decoded.width as u32,
                    decoded.height as u32,
                    decoded.rgb,
                )
                .ok_or_else(|| Error::Decode("Could not create RGB image buffer".into()))?;

                let dyn_img = image::DynamicImage::ImageRgb8(rgb_buf);
                let capped_img = capped(dyn_img, max_dimension).to_rgb8();
                let (w, h) = (capped_img.width() as usize, capped_img.height() as usize);
                let mut image = Image::from_rgb8(&capped_img.into_raw(), w, h);
                image.apply_orientation(decoded.orientation);

                Ok(Source {
                    image,
                    icc: decoded.icc,
                    orientation: decoded.orientation,
                    has_gain_map: decoded.has_gain_map,
                    converted_from: Some("HEIC"),
                })
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(Error::ReadOnlyFormat { format: "HEIC" })
            }
        } else {
            let mut image = Image::decode_capped(bytes, max_dimension)?;
            let icc = extract_icc(bytes);
            let orientation = extract_orientation(bytes);
            image.apply_orientation(orientation);

            Ok(Source {
                image,
                icc,
                orientation,
                has_gain_map: false,
                converted_from: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_macos_heic_decode_and_optimize() {
        use std::process::Command;
        use crate::Mode;

        if !std::path::Path::new("/usr/bin/sips").exists() {
            return;
        }

        let temp_dir = std::env::temp_dir().join(format!("squint_test_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).expect("create temp dir");
        let png_path = temp_dir.join("in.png");
        let heic_path = temp_dir.join("out.heic");

        // A 1200x800 picture whose pixel at (x, y) is (x, y, 200) plus a few
        // counts of deterministic noise. The coordinates make a vertical flip
        // or a swapped channel show up in the corner checks below; the noise
        // makes the HEVC-coded file large enough that a 150 px JPEG made from
        // it is genuinely smaller. A smooth gradient codes to under 2 KB as
        // HEIC, and the never-grow check then refuses the conversion — which is
        // correct behaviour, and not what this test is for.
        // 1201 wide so that a row is 4,804 bytes, which no alignment Quartz
        // prefers divides: a decode that reads rows at the requested stride
        // instead of the reported one shears the picture and fails the corner
        // checks below.
        let (w, h) = (1201u32, 800u32);
        let mut img_buf = image::RgbImage::new(w, h);
        let mut seed = 0x9E37_79B9u32;
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 29) as i16 - 3; // -3..=4
                let px = |base: i16| (base + noise).clamp(0, 255) as u8;
                img_buf.put_pixel(x, y, image::Rgb([px(x as u8 as i16), px(y as u8 as i16), px(200)]));
            }
        }
        img_buf.save(&png_path).expect("save png");

        // Run sips -s format heic <in.png> --out <out.heic>
        let status = Command::new("/usr/bin/sips")
            .arg("-s")
            .arg("format")
            .arg("heic")
            .arg(&png_path)
            .arg("--out")
            .arg(&heic_path)
            .status()
            .expect("execute sips");
        assert!(status.success(), "sips failed");

        let heic_bytes = std::fs::read(&heic_path).expect("read heic");
        assert!(crate::heif::is_heif(&heic_bytes));

        let src = Source::open(&heic_bytes, None).expect("Source::open failed");
        assert_eq!((src.image.width, src.image.height), (w as usize, h as usize));
        assert_eq!(src.converted_from, Some("HEIC"));
        assert_eq!(src.orientation, 1);
        assert!(!src.has_gain_map, "sips writes no gain map");

        // Two rows whose green values differ by far more than the tolerance,
        // so a picture drawn upside down cannot pass. Row 700 is 188 after the
        // u8 wrap; the bottom row would be 31, and a flip would put it at the
        // top.
        let at = |x: usize, y: usize| src.image.pixels[y * w as usize + x];
        for (p, want) in [(at(10, 10), [10i16, 10, 200]), (at(10, 700), [10, 700u32 as u8 as i16, 200])] {
            for c in 0..3 {
                assert!((p[c] as i16 - want[c]).abs() <= 16, "pixel {p:?} is not near {want:?}");
            }
        }

        let opt = crate::optimize(&heic_bytes, Mode::Fast, 80.0, 75.0, Some(70), 6, Some(150))
            .expect("optimize failed");
        assert!(opt.data.len() >= 2 && opt.data[0] == 0xFF && opt.data[1] == 0xD8);
        assert_eq!(opt.converted_from, Some("HEIC"));

        let dec = Image::decode(&opt.data).expect("decode optimized jpeg");
        assert_eq!(dec.width, 150);
        assert_eq!(dec.height, 100);

        // Cut short, the file must be refused rather than drawn black.
        let truncated = &heic_bytes[..heic_bytes.len() * 6 / 10];
        assert!(
            matches!(Source::open(truncated, None), Err(Error::Decode(_))),
            "a truncated HEIC must be refused"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
