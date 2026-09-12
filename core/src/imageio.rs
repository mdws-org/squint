//! Image I/O decoding for macOS.
//!
//! Uses macOS Image I/O and Core Graphics frameworks directly to decode HEIF/HEIC
//! and other system-supported image formats without external crate dependencies.

use std::ffi::c_void;
use std::ptr;

use crate::Error;

pub struct Decoded {
    pub rgb: Vec<u8>,          // width*height*3, source colour space, NOT converted
    pub width: usize,
    pub height: usize,
    pub icc: Option<Vec<u8>>,  // ICC profile of the source colour space
    pub orientation: u16,      // EXIF orientation 1..=8, 1 if absent
}

type CFTypeRef = *const c_void;
type CFDataRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFStringRef = *const c_void;
type CFNumberRef = *const c_void;
type CGImageSourceRef = *const c_void;
type CGImageRef = *const c_void;
type CGColorSpaceRef = *const c_void;
type CGContextRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

const K_CF_NUMBER_SINT64_TYPE: i32 = 4;
const K_CG_COLOR_SPACE_MODEL_RGB: i32 = 1;
const K_CG_IMAGE_ALPHA_NONE_SKIP_LAST: u32 = 5;
const K_CG_BITMAP_BYTE_ORDER_DEFAULT: u32 = 0;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFDataRef;
    fn CFDataGetLength(theData: CFDataRef) -> isize;
    fn CFDataGetBytePtr(theData: CFDataRef) -> *const u8;
    fn CFDictionaryGetValue(theDict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: CFNumberRef, theType: i32, valuePtr: *mut c_void) -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorSpaceGetModel(space: CGColorSpaceRef) -> i32;
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceCopyICCData(space: CGColorSpaceRef) -> CFDataRef;
    fn CGImageGetWidth(image: CGImageRef) -> usize;
    fn CGImageGetHeight(image: CGImageRef) -> usize;
    fn CGImageGetColorSpace(image: CGImageRef) -> CGColorSpaceRef;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bitsPerComponent: usize,
        bytesPerRow: usize,
        space: CGColorSpaceRef,
        bitmapInfo: u32,
    ) -> CGContextRef;
    fn CGBitmapContextGetData(context: CGContextRef) -> *mut c_void;
    fn CGContextDrawImage(c: CGContextRef, rect: CGRect, image: CGImageRef);
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    static kCGImagePropertyPixelWidth: CFStringRef;
    static kCGImagePropertyPixelHeight: CFStringRef;
    static kCGImagePropertyOrientation: CFStringRef;

    fn CGImageSourceCreateWithData(data: CFDataRef, options: CFDictionaryRef) -> CGImageSourceRef;
    fn CGImageSourceGetCount(isrc: CGImageSourceRef) -> usize;
    fn CGImageSourceCopyPropertiesAtIndex(
        isrc: CGImageSourceRef,
        index: usize,
        options: CFDictionaryRef,
    ) -> CFDictionaryRef;
    fn CGImageSourceCreateImageAtIndex(
        isrc: CGImageSourceRef,
        index: usize,
        options: CFDictionaryRef,
    ) -> CGImageRef;
}

struct Cf<T>(*const T);

impl<T> Cf<T> {
    fn wrap(ptr: *const T) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    fn as_ptr(&self) -> *const T {
        self.0
    }
}

impl<T> Drop for Cf<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0 as CFTypeRef);
            }
        }
    }
}

pub fn decode(bytes: &[u8], max_pixels: usize) -> Result<Decoded, Error> {
    unsafe {
        let cf_data = Cf::wrap(CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize))
            .ok_or_else(|| Error::Decode("Image I/O could not create data buffer".into()))?;

        let src = Cf::wrap(CGImageSourceCreateWithData(cf_data.as_ptr() as CFDataRef, ptr::null()))
            .ok_or_else(|| Error::Decode("Image I/O could not open this file".into()))?;

        let count = CGImageSourceGetCount(src.as_ptr() as CGImageSourceRef);
        if count < 1 {
            return Err(Error::Decode("Image I/O found no images in file".into()));
        }

        let props = Cf::wrap(CGImageSourceCopyPropertiesAtIndex(
            src.as_ptr() as CGImageSourceRef,
            0,
            ptr::null(),
        ));

        let mut declared_width: i64 = 0;
        let mut declared_height: i64 = 0;
        let mut orientation: i64 = 1;

        if let Some(props_ref) = &props {
            let dict = props_ref.as_ptr() as CFDictionaryRef;
            let w_num = CFDictionaryGetValue(dict, kCGImagePropertyPixelWidth as *const c_void);
            if !w_num.is_null() {
                CFNumberGetValue(
                    w_num as CFNumberRef,
                    K_CF_NUMBER_SINT64_TYPE,
                    &mut declared_width as *mut i64 as *mut c_void,
                );
            }

            let h_num = CFDictionaryGetValue(dict, kCGImagePropertyPixelHeight as *const c_void);
            if !h_num.is_null() {
                CFNumberGetValue(
                    h_num as CFNumberRef,
                    K_CF_NUMBER_SINT64_TYPE,
                    &mut declared_height as *mut i64 as *mut c_void,
                );
            }

            let o_num = CFDictionaryGetValue(dict, kCGImagePropertyOrientation as *const c_void);
            if !o_num.is_null() {
                CFNumberGetValue(
                    o_num as CFNumberRef,
                    K_CF_NUMBER_SINT64_TYPE,
                    &mut orientation as *mut i64 as *mut c_void,
                );
            }
        }

        let dw = declared_width.max(0) as usize;
        let dh = declared_height.max(0) as usize;
        if dw.saturating_mul(dh) > max_pixels {
            return Err(Error::TooLarge { width: dw, height: dh });
        }

        let image = Cf::wrap(CGImageSourceCreateImageAtIndex(
            src.as_ptr() as CGImageSourceRef,
            0,
            ptr::null(),
        ))
        .ok_or_else(|| Error::Decode("Image I/O could not create image at index 0".into()))?;

        let w = CGImageGetWidth(image.as_ptr() as CGImageRef);
        let h = CGImageGetHeight(image.as_ptr() as CGImageRef);

        if w.saturating_mul(h) > max_pixels {
            return Err(Error::TooLarge { width: w, height: h });
        }

        if w == 0 || h == 0 {
            return Err(Error::Decode("Image has zero width or height".into()));
        }

        let cs = CGImageGetColorSpace(image.as_ptr() as CGImageRef);
        let (ctx_cs, icc, _cs_guard) = if !cs.is_null() && CGColorSpaceGetModel(cs) == K_CG_COLOR_SPACE_MODEL_RGB {
            let icc_data = Cf::wrap(CGColorSpaceCopyICCData(cs));
            let icc_bytes = icc_data.map(|d| {
                let len = CFDataGetLength(d.as_ptr() as CFDataRef) as usize;
                let ptr = CFDataGetBytePtr(d.as_ptr() as CFDataRef);
                std::slice::from_raw_parts(ptr, len).to_vec()
            });
            (cs, icc_bytes, None)
        } else {
            let dev_rgb = Cf::wrap(CGColorSpaceCreateDeviceRGB())
                .ok_or_else(|| Error::Decode("Could not create device RGB colour space".into()))?;
            let dev_ptr = dev_rgb.as_ptr() as CGColorSpaceRef;
            (dev_ptr, None, Some(dev_rgb))
        };

        let total_bytes = w.checked_mul(h)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| Error::TooLarge { width: w, height: h })?;

        let bytes_per_row = w.checked_mul(4)
            .ok_or_else(|| Error::TooLarge { width: w, height: h })?;

        let ctx = Cf::wrap(CGBitmapContextCreate(
            ptr::null_mut(),
            w,
            h,
            8,
            bytes_per_row,
            ctx_cs,
            K_CG_IMAGE_ALPHA_NONE_SKIP_LAST | K_CG_BITMAP_BYTE_ORDER_DEFAULT,
        ))
        .ok_or_else(|| Error::Decode("Could not create bitmap context".into()))?;

        CGContextDrawImage(
            ctx.as_ptr() as CGContextRef,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: w as f64, height: h as f64 },
            },
            image.as_ptr() as CGImageRef,
        );

        let data_ptr = CGBitmapContextGetData(ctx.as_ptr() as CGContextRef) as *const u8;
        if data_ptr.is_null() {
            return Err(Error::Decode("Bitmap context data is null".into()));
        }

        let raw_slice = std::slice::from_raw_parts(data_ptr, total_bytes);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for chunk in raw_slice.chunks_exact(4) {
            rgb.extend_from_slice(&chunk[..3]);
        }

        let orient_u16 = if (1..=8).contains(&orientation) {
            orientation as u16
        } else {
            1
        };

        Ok(Decoded {
            rgb,
            width: w,
            height: h,
            icc,
            orientation: orient_u16,
        })
    }
}
