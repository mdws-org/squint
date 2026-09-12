//! Decoding through the system's Image I/O, for the formats no crate reads.
//!
//! A HEIC is HEVC inside, and the only HEVC decoder on a Mac is the one macOS
//! ships. It is reached here through hand-written bindings to the dozen C
//! functions needed rather than through a bindings crate, so the dependency
//! set stays at what `Cargo.lock` already carries.
//!
//! The pixels come back in the picture's own colour space, not converted to
//! sRGB. The engine embeds the source profile in the JPEG it writes, so the
//! numbers must still mean what that profile says they mean; an iPhone shoots
//! Display P3, and converting to sRGB would clip its colours before the
//! profile could describe them.
//!
//! What is not caught here: a HEIF whose range lives in a PQ or HLG transfer
//! function rather than a gain map. An iPhone writes gain maps, so the gain
//! map is what is asked about; a single-image PQ file would be flattened to
//! 8 bits by the draw and reported as having had no range to lose.

use std::ffi::c_void;
use std::ptr;

use crate::Error;

pub struct Decoded {
    /// width*height*3, in the colour space `icc` describes, or sRGB when `icc` is None.
    pub rgb: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// ICC profile of the source colour space, when the system could produce one.
    pub icc: Option<Vec<u8>>,
    /// EXIF orientation 1..=8, 1 if absent.
    pub orientation: u16,
    /// Whether the container declares an HDR gain map for the primary image,
    /// asked of Image I/O directly rather than guessed from the bytes.
    pub has_gain_map: bool,
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
/// `kCGImageStatusComplete`. Anything else means the container is truncated,
/// unreadable or still being parsed, and a draw from it would come out black
/// or half-filled with no error raised anywhere.
const K_CG_IMAGE_STATUS_COMPLETE: i32 = 0;
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
    static kCGColorSpaceSRGB: CFStringRef;

    fn CGColorSpaceGetModel(space: CGColorSpaceRef) -> i32;
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> CGColorSpaceRef;
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
    fn CGBitmapContextGetBytesPerRow(context: CGContextRef) -> usize;
    fn CGContextDrawImage(c: CGContextRef, rect: CGRect, image: CGImageRef);
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    static kCGImagePropertyPixelWidth: CFStringRef;
    static kCGImagePropertyPixelHeight: CFStringRef;
    static kCGImagePropertyOrientation: CFStringRef;
    static kCGImageAuxiliaryDataTypeHDRGainMap: CFStringRef;
    static kCGImageAuxiliaryDataTypeISOGainMap: CFStringRef;

    fn CGImageSourceCreateWithData(data: CFDataRef, options: CFDictionaryRef) -> CGImageSourceRef;
    fn CGImageSourceGetCount(isrc: CGImageSourceRef) -> usize;
    fn CGImageSourceGetPrimaryImageIndex(isrc: CGImageSourceRef) -> usize;
    fn CGImageSourceGetStatus(isrc: CGImageSourceRef) -> i32;
    fn CGImageSourceGetStatusAtIndex(isrc: CGImageSourceRef, index: usize) -> i32;
    fn CGImageSourceCopyAuxiliaryDataInfoAtIndex(
        isrc: CGImageSourceRef,
        index: usize,
        auxiliary_image_data_type: CFStringRef,
    ) -> CFDictionaryRef;
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

        // `CGContextDrawImage` returns nothing. A stream that dies part way
        // through decoding leaves the context zero-filled and the draw reports
        // success, so the container's own status is the only place a
        // truncated file can be caught before it becomes a black photograph.
        if CGImageSourceGetStatus(src.as_ptr() as CGImageSourceRef) != K_CG_IMAGE_STATUS_COMPLETE {
            return Err(Error::Decode("Image I/O could not read this file to the end".into()));
        }
        let count = CGImageSourceGetCount(src.as_ptr() as CGImageSourceRef);
        if count < 1 {
            return Err(Error::Decode("Image I/O found no images in file".into()));
        }
        // A HEIF holds several items: the picture, its thumbnail, a gain map, a
        // depth map. Which one is the picture is declared by the container, and
        // it is not always the first.
        let primary = CGImageSourceGetPrimaryImageIndex(src.as_ptr() as CGImageSourceRef);
        if CGImageSourceGetStatusAtIndex(src.as_ptr() as CGImageSourceRef, primary) != K_CG_IMAGE_STATUS_COMPLETE {
            return Err(Error::Decode("Image I/O could not read the picture to the end".into()));
        }

        // The declared size is read from the header before the picture is
        // created, so a file lying about its size costs a dictionary lookup
        // rather than an allocation. A file that declares no size at all is
        // refused for the same reason: the check cannot be made, so the
        // allocation must not be either.
        let props = Cf::wrap(CGImageSourceCopyPropertiesAtIndex(
            src.as_ptr() as CGImageSourceRef,
            primary,
            ptr::null(),
        ))
        .ok_or_else(|| Error::Decode("Image I/O reported no properties for this file".into()))?;
        let dict = props.as_ptr() as CFDictionaryRef;
        let number = |key: CFStringRef| -> Option<i64> {
            let num = CFDictionaryGetValue(dict, key as *const c_void);
            if num.is_null() {
                return None;
            }
            let mut value: i64 = 0;
            CFNumberGetValue(num as CFNumberRef, K_CF_NUMBER_SINT64_TYPE, &mut value as *mut i64 as *mut c_void)
                .then_some(value)
        };
        let (dw, dh) = match (number(kCGImagePropertyPixelWidth), number(kCGImagePropertyPixelHeight)) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as usize, h as usize),
            _ => return Err(Error::Decode("Image I/O reported no dimensions for this file".into())),
        };
        if dw.saturating_mul(dh) > max_pixels {
            return Err(Error::TooLarge { width: dw, height: dh });
        }
        let orientation = number(kCGImagePropertyOrientation).unwrap_or(1);

        // Asked of the container rather than guessed from its bytes. Apple's
        // map and the ISO 21496-1 map are separate auxiliary types; a
        // photograph carries one or the other.
        let has_gain_map = [kCGImageAuxiliaryDataTypeISOGainMap, kCGImageAuxiliaryDataTypeHDRGainMap]
            .into_iter()
            .any(|kind| {
                Cf::wrap(CGImageSourceCopyAuxiliaryDataInfoAtIndex(src.as_ptr() as CGImageSourceRef, primary, kind))
                    .is_some()
            });

        let image = Cf::wrap(CGImageSourceCreateImageAtIndex(
            src.as_ptr() as CGImageSourceRef,
            primary,
            ptr::null(),
        ))
        .ok_or_else(|| Error::Decode("Image I/O could not decode the picture".into()))?;

        let w = CGImageGetWidth(image.as_ptr() as CGImageRef);
        let h = CGImageGetHeight(image.as_ptr() as CGImageRef);
        if w == 0 || h == 0 {
            return Err(Error::Decode("Image I/O decoded a picture with no pixels".into()));
        }
        if w.saturating_mul(h) > max_pixels {
            return Err(Error::TooLarge { width: w, height: h });
        }

        // The pixels are kept in the picture's own space only when its profile
        // can travel with them. A wide-gamut picture written without its
        // profile is read as sRGB and comes out dull, so when the system has
        // no profile to give, the picture is converted to sRGB here and left
        // untagged, which is the one untagged interpretation every reader
        // agrees on. `cs` is owned by `image` (Get rule) and is not released.
        let cs = CGImageGetColorSpace(image.as_ptr() as CGImageRef);
        let icc = if !cs.is_null() && CGColorSpaceGetModel(cs) == K_CG_COLOR_SPACE_MODEL_RGB {
            Cf::wrap(CGColorSpaceCopyICCData(cs)).and_then(|d| {
                let len = CFDataGetLength(d.as_ptr() as CFDataRef) as usize;
                let ptr = CFDataGetBytePtr(d.as_ptr() as CFDataRef);
                (len > 0 && !ptr.is_null()).then(|| std::slice::from_raw_parts(ptr, len).to_vec())
            })
        } else {
            None
        };
        let srgb;
        let ctx_cs = if icc.is_some() {
            cs
        } else {
            srgb = Cf::wrap(CGColorSpaceCreateWithName(kCGColorSpaceSRGB))
                .ok_or_else(|| Error::Decode("could not create the sRGB colour space".into()))?;
            srgb.as_ptr() as CGColorSpaceRef
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

        // The stride asked for is a request. With no buffer supplied, Quartz
        // allocates its own and may pad each row to an alignment it prefers,
        // so the rows are walked at the stride it reports rather than the one
        // that was requested. Reading at the requested stride stays inside the
        // buffer and shears the picture diagonally, with nothing to say so.
        let stride = CGBitmapContextGetBytesPerRow(ctx.as_ptr() as CGContextRef);
        if stride < bytes_per_row {
            return Err(Error::Decode("bitmap context rows are narrower than the picture".into()));
        }
        let raw_slice = std::slice::from_raw_parts(data_ptr, stride.checked_mul(h).unwrap_or(total_bytes));
        let mut rgb = Vec::with_capacity(total_bytes / 4 * 3);
        for row in raw_slice.chunks_exact(stride) {
            for px in row[..bytes_per_row].chunks_exact(4) {
                rgb.extend_from_slice(&px[..3]);
            }
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
            has_gain_map,
        })
    }
}
