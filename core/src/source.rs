//! Image source loading and orientation normalization.
//!
//! Provides a uniform entry point `Source::open` to load images (JPEG, PNG, HEIF)
//! into RGB8 memory with orientation applied and dimensions capped if requested.

use crate::{capped, extract_icc, extract_orientation, Error, Image, MAX_PIXELS};

pub struct Source {
    /// Pixels with the EXIF orientation already baked in, capped to `max_dimension` (Lanczos, never enlarged).
    pub image: Image,
    pub icc: Option<Vec<u8>>,
    /// Set when the output format will differ from the input's: `Some("HEIC")` for a HEIF source. None for JPEG and PNG.
    pub converted_from: Option<&'static str>,
}

impl Source {
    pub fn open(bytes: &[u8], max_dimension: Option<u32>) -> Result<Source, Error> {
        if crate::heif::is_heif(bytes) {
            #[cfg(target_os = "macos")]
            {
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

        // Build a 300x200 RGB8 image whose pixel at (x, y) is (x as u8, y as u8, 200)
        let mut img_buf = image::RgbImage::new(300, 200);
        for y in 0..200u32 {
            for x in 0..300u32 {
                img_buf.put_pixel(x, y, image::Rgb([x as u8, y as u8, 200]));
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
        assert_eq!(src.image.width, 300);
        assert_eq!(src.image.height, 200);
        assert_eq!(src.converted_from, Some("HEIC"));

        // assert the pixel at (10, 10) is within ±16 per channel of (10, 10, 200)
        // and the pixel at (10, 190) within ±16 of (10, 190, 200).
        let p_10_10 = src.image.pixels[10 * 300 + 10];
        let p_10_190 = src.image.pixels[190 * 300 + 10];

        assert!((p_10_10[0] as i16 - 10).abs() <= 16, "p_10_10 red out of range: {}", p_10_10[0]);
        assert!((p_10_10[1] as i16 - 10).abs() <= 16, "p_10_10 green out of range: {}", p_10_10[1]);
        assert!((p_10_10[2] as i16 - 200).abs() <= 16, "p_10_10 blue out of range: {}", p_10_10[2]);

        assert!((p_10_190[0] as i16 - 10).abs() <= 16, "p_10_190 red out of range: {}", p_10_190[0]);
        assert!((p_10_190[1] as i16 - 190).abs() <= 16, "p_10_190 green out of range: {}", p_10_190[1]);
        assert!((p_10_190[2] as i16 - 200).abs() <= 16, "p_10_190 blue out of range: {}", p_10_190[2]);

        // Then optimize(&heic, Mode::Fast, 80.0, 75.0, Some(70), 6, Some(150))
        // → Ok, data starts with 0xFF 0xD8, converted_from == Some("HEIC"), and Image::decode(&data) is 150x100.
        let opt = crate::optimize(&heic_bytes, Mode::Fast, 80.0, 75.0, Some(70), 6, Some(150))
            .expect("optimize failed");
        assert!(opt.data.len() >= 2 && opt.data[0] == 0xFF && opt.data[1] == 0xD8);
        assert_eq!(opt.converted_from, Some("HEIC"));

        let dec = Image::decode(&opt.data).expect("decode optimized jpeg");
        assert_eq!(dec.width, 150);
        assert_eq!(dec.height, 100);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
