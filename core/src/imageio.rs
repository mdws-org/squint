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

use std::ffi::{c_void, CStr};
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
type CFMutableDataRef = *const c_void;
type CGImageDestinationRef = *const c_void;

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

const K_CF_NUMBER_DOUBLE_TYPE: i32 = 13;
const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;

    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFDataRef;
    fn CFDataCreateMutable(allocator: CFTypeRef, capacity: isize) -> CFMutableDataRef;
    fn CFDataGetLength(theData: CFDataRef) -> isize;
    fn CFDataGetBytePtr(theData: CFDataRef) -> *const u8;
    fn CFDictionaryGetValue(theDict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        numValues: isize,
        keyCallBacks: *const c_void,
        valueCallBacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFNumberGetValue(number: CFNumberRef, theType: i32, valuePtr: *mut c_void) -> bool;
    fn CFNumberCreate(allocator: CFTypeRef, theType: i32, valuePtr: *const c_void) -> CFNumberRef;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        cStr: *const std::ffi::c_char,
        encoding: u32,
    ) -> CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGColorSpaceSRGB: CFStringRef;

    fn CGColorSpaceGetModel(space: CGColorSpaceRef) -> i32;
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> CGColorSpaceRef;
    fn CGColorSpaceCopyICCData(space: CGColorSpaceRef) -> CFDataRef;
    fn CGColorSpaceCreateWithICCData(data: CFDataRef) -> CGColorSpaceRef;
    fn CGBitmapContextCreateImage(context: CGContextRef) -> CGImageRef;
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
    static kCGImagePropertyMakerAppleDictionary: CFStringRef;
    static kCGImageAuxiliaryDataTypeHDRGainMap: CFStringRef;
    static kCGImageAuxiliaryDataTypeISOGainMap: CFStringRef;
    static kCGImageDestinationLossyCompressionQuality: CFStringRef;

    fn CGImageDestinationCreateWithData(
        data: CFMutableDataRef,
        type_: CFStringRef,
        count: usize,
        options: CFDictionaryRef,
    ) -> CGImageDestinationRef;
    fn CGImageDestinationAddImage(
        idst: CGImageDestinationRef,
        image: CGImageRef,
        properties: CFDictionaryRef,
    );
    fn CGImageDestinationFinalize(idst: CGImageDestinationRef) -> bool;
    fn CGImageDestinationAddAuxiliaryDataInfo(
        idst: CGImageDestinationRef,
        auxiliary_image_data_type: CFStringRef,
        auxiliary_data_info_dictionary: CFDictionaryRef,
    );

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

/// A Core Foundation object this module owns one retain count on.
///
/// Every `Cf` here is built by `wrap` straight from the result of a Create- or
/// Copy-rule function, never from a Get-rule one, so releasing it once on drop
/// balances the count it arrived with.
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
            // SAFETY: the pointer is a live object this `Cf` owns one count
            // on (see the type), and this is the only place that count is
            // given back.
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }
}

/// A CFData holding a copy of `bytes`.
fn data_from(bytes: &[u8]) -> Option<Cf<c_void>> {
    // SAFETY: the pointer and length come from one live slice, whose length
    // never exceeds `isize::MAX`; CFDataCreate copies the bytes, so nothing
    // borrows `bytes` once it returns.
    Cf::wrap(unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize) })
}

/// The bytes a CFData holds, copied out.
fn data_bytes(data: &Cf<c_void>) -> Vec<u8> {
    // SAFETY: `data` is a live CFData.
    let len = unsafe { CFDataGetLength(data.as_ptr() as CFDataRef) } as usize;
    // SAFETY: as above.
    let bytes = unsafe { CFDataGetBytePtr(data.as_ptr() as CFDataRef) };
    if len == 0 || bytes.is_null() {
        return Vec::new();
    }
    // SAFETY: CFDataGetBytePtr points at the `len` bytes `data` owns, and
    // `data` is borrowed, so it outlives the copy made here.
    unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()
}

/// A CFString of a NUL-terminated UTF-8 string.
fn cf_string(s: &CStr) -> Option<Cf<c_void>> {
    // SAFETY: `s` is NUL-terminated by its type; CFStringCreateWithCString
    // copies it.
    Cf::wrap(unsafe { CFStringCreateWithCString(ptr::null(), s.as_ptr(), K_CF_STRING_ENCODING_UTF8) })
}

/// A CFDictionary of the given pairs, retaining each key and value.
///
/// # Safety
/// Every pointer in `keys` and `values` must be a live Core Foundation object.
unsafe fn dictionary(keys: &[*const c_void], values: &[*const c_void]) -> Option<Cf<c_void>> {
    assert_eq!(keys.len(), values.len());
    // SAFETY: both arrays hold `keys.len()` live CF objects (the caller's
    // contract, and the lengths are asserted equal above); the
    // CFType callbacks retain each one, so the dictionary does not borrow
    // the arrays or the objects past this call. Taking the address of the
    // callback constants reads nothing.
    Cf::wrap(unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    })
}

/// The sRGB colour space.
fn srgb() -> Option<Cf<c_void>> {
    // SAFETY: a framework constant, initialized before any code here runs and
    // never written.
    let name = unsafe { kCGColorSpaceSRGB };
    // SAFETY: `name` is a live CFString.
    Cf::wrap(unsafe { CGColorSpaceCreateWithName(name) })
}

/// The two auxiliary data types a gain map is stored under, the ISO 21496-1
/// one first.
fn gain_map_kinds() -> [CFStringRef; 2] {
    // SAFETY: framework constants, initialized before any code here runs and
    // never written.
    let iso = unsafe { kCGImageAuxiliaryDataTypeISOGainMap };
    // SAFETY: as above.
    let apple = unsafe { kCGImageAuxiliaryDataTypeHDRGainMap };
    [iso, apple]
}

/// The gain map description an image source holds at `index` under `kind`.
///
/// # Safety
/// `kind` must be a live CFString.
unsafe fn auxiliary_data(src: &Cf<c_void>, index: usize, kind: CFStringRef) -> Option<Cf<c_void>> {
    // SAFETY: `src` is a live image source and `kind` a live CFString (the
    // caller's contract); an index past the end returns null rather than
    // reading out of bounds.
    Cf::wrap(unsafe { CGImageSourceCopyAuxiliaryDataInfoAtIndex(src.as_ptr() as CGImageSourceRef, index, kind) })
}

/// An image source over `data`.
fn image_source(data: &Cf<c_void>) -> Option<Cf<c_void>> {
    // SAFETY: `data` is a live CFData, which the source retains; a null
    // options dictionary is allowed.
    Cf::wrap(unsafe { CGImageSourceCreateWithData(data.as_ptr() as CFDataRef, ptr::null()) })
}

/// The index of the image the container names as its picture.
fn primary_index(src: &Cf<c_void>) -> usize {
    // SAFETY: `src` is a live image source.
    unsafe { CGImageSourceGetPrimaryImageIndex(src.as_ptr() as CGImageSourceRef) }
}

/// The properties dictionary of the image at `index`.
fn properties(src: &Cf<c_void>, index: usize) -> Option<Cf<c_void>> {
    // SAFETY: `src` is a live image source, a null options dictionary is
    // allowed, and an index past the end returns null.
    Cf::wrap(unsafe { CGImageSourceCopyPropertiesAtIndex(src.as_ptr() as CGImageSourceRef, index, ptr::null()) })
}

/// The value under `key`, borrowed from `dict`: valid only while `dict` is.
///
/// # Safety
/// `dict` must be a live CFDictionary and `key` a live CFString.
unsafe fn lookup(dict: CFDictionaryRef, key: CFStringRef) -> *const c_void {
    // SAFETY: the caller's contract; a missing key returns null.
    unsafe { CFDictionaryGetValue(dict, key) }
}

pub fn decode(bytes: &[u8], max_pixels: usize) -> Result<Decoded, Error> {
    let cf_data = data_from(bytes).ok_or_else(|| Error::Decode("Image I/O could not create data buffer".into()))?;

    let src = image_source(&cf_data).ok_or_else(|| Error::Decode("Image I/O could not open this file".into()))?;

    // `CGContextDrawImage` returns nothing. A stream that dies part way
    // through decoding leaves the context zero-filled and the draw reports
    // success, so the container's own status is the only place a
    // truncated file can be caught before it becomes a black photograph.
    // SAFETY: `src` is a live image source.
    if unsafe { CGImageSourceGetStatus(src.as_ptr() as CGImageSourceRef) } != K_CG_IMAGE_STATUS_COMPLETE {
        return Err(Error::Decode("Image I/O could not read this file to the end".into()));
    }
    // SAFETY: as above.
    let count = unsafe { CGImageSourceGetCount(src.as_ptr() as CGImageSourceRef) };
    if count < 1 {
        return Err(Error::Decode("Image I/O found no images in file".into()));
    }
    // A HEIF holds several items: the picture, its thumbnail, a gain map, a
    // depth map. Which one is the picture is declared by the container, and
    // it is not always the first.
    let primary = primary_index(&src);
    // SAFETY: `src` is a live image source; an index past the end reports a
    // failure status rather than reading out of bounds.
    if unsafe { CGImageSourceGetStatusAtIndex(src.as_ptr() as CGImageSourceRef, primary) } != K_CG_IMAGE_STATUS_COMPLETE {
        return Err(Error::Decode("Image I/O could not read the picture to the end".into()));
    }

    // The declared size is read from the header before the picture is
    // created, so a file lying about its size costs a dictionary lookup
    // rather than an allocation. A file that declares no size at all is
    // refused for the same reason: the check cannot be made, so the
    // allocation must not be either.
    let props = properties(&src, primary)
        .ok_or_else(|| Error::Decode("Image I/O reported no properties for this file".into()))?;
    let dict = props.as_ptr() as CFDictionaryRef;
    let number = |key: CFStringRef| -> Option<i64> {
        // SAFETY: `dict` is borrowed from `props`, which outlives this
        // closure, and every key passed is a framework constant.
        let num = unsafe { lookup(dict, key) };
        if num.is_null() {
            return None;
        }
        let mut value: i64 = 0;
        // SAFETY: `num` is a live value borrowed from `props`, which outlives
        // this closure. Image I/O stores the size and orientation properties
        // as CFNumbers, and `value` is an i64, the size SInt64 writes.
        unsafe { CFNumberGetValue(num as CFNumberRef, K_CF_NUMBER_SINT64_TYPE, (&raw mut value).cast::<c_void>()) }
            .then_some(value)
    };
    // SAFETY: framework constants, initialized before any code here runs and
    // never written.
    let width_key = unsafe { kCGImagePropertyPixelWidth };
    // SAFETY: as above.
    let height_key = unsafe { kCGImagePropertyPixelHeight };
    // SAFETY: as above.
    let orientation_key = unsafe { kCGImagePropertyOrientation };
    let (dw, dh) = match (number(width_key), number(height_key)) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as usize, h as usize),
        _ => return Err(Error::Decode("Image I/O reported no dimensions for this file".into())),
    };
    if dw.saturating_mul(dh) > max_pixels {
        return Err(Error::TooLarge { width: dw, height: dh });
    }
    let orientation = number(orientation_key).unwrap_or(1);

    // Asked of the container rather than guessed from its bytes. Apple's
    // map and the ISO 21496-1 map are separate auxiliary types; a
    // photograph carries one or the other.
    // SAFETY: each kind is a framework constant.
    let has_gain_map = gain_map_kinds().into_iter().any(|kind| unsafe { auxiliary_data(&src, primary, kind) }.is_some());

    // SAFETY: `src` is a live image source, a null options dictionary is
    // allowed, and an index past the end returns null.
    let image = Cf::wrap(unsafe {
        CGImageSourceCreateImageAtIndex(src.as_ptr() as CGImageSourceRef, primary, ptr::null())
    })
    .ok_or_else(|| Error::Decode("Image I/O could not decode the picture".into()))?;

    // SAFETY: `image` is a live CGImage.
    let w = unsafe { CGImageGetWidth(image.as_ptr() as CGImageRef) };
    // SAFETY: as above.
    let h = unsafe { CGImageGetHeight(image.as_ptr() as CGImageRef) };
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
    // SAFETY: `image` is a live CGImage.
    let cs = unsafe { CGImageGetColorSpace(image.as_ptr() as CGImageRef) };
    // SAFETY: `cs` is checked non-null first and is kept alive by `image`.
    let icc = if !cs.is_null() && unsafe { CGColorSpaceGetModel(cs) } == K_CG_COLOR_SPACE_MODEL_RGB {
        // SAFETY: `cs` is a live colour space, kept alive by `image`.
        Cf::wrap(unsafe { CGColorSpaceCopyICCData(cs) })
            .map(|d| data_bytes(&d))
            .filter(|profile| !profile.is_empty())
    } else {
        None
    };
    let srgb_space;
    let ctx_cs = if icc.is_some() {
        cs
    } else {
        srgb_space = srgb().ok_or_else(|| Error::Decode("could not create the sRGB colour space".into()))?;
        srgb_space.as_ptr() as CGColorSpaceRef
    };

    let total_bytes = w.checked_mul(h)
        .and_then(|px| px.checked_mul(4))
        .ok_or(Error::TooLarge { width: w, height: h })?;

    let bytes_per_row = w.checked_mul(4)
        .ok_or(Error::TooLarge { width: w, height: h })?;

    // SAFETY: with a null data pointer Quartz allocates and owns the buffer
    // itself; `ctx_cs` is a live colour space (kept alive by `image` or
    // `srgb_space`, both of which outlive `ctx`).
    let ctx = Cf::wrap(unsafe {
        CGBitmapContextCreate(
            ptr::null_mut(),
            w,
            h,
            8,
            bytes_per_row,
            ctx_cs,
            K_CG_IMAGE_ALPHA_NONE_SKIP_LAST | K_CG_BITMAP_BYTE_ORDER_DEFAULT,
        )
    })
    .ok_or_else(|| Error::Decode("Could not create bitmap context".into()))?;

    // SAFETY: `ctx` and `image` are live; the draw writes only into the
    // context's own buffer.
    unsafe {
        CGContextDrawImage(
            ctx.as_ptr() as CGContextRef,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: w as f64, height: h as f64 },
            },
            image.as_ptr() as CGImageRef,
        )
    };

    // SAFETY: `ctx` is a live bitmap context.
    let data_ptr = unsafe { CGBitmapContextGetData(ctx.as_ptr() as CGContextRef) } as *const u8;
    if data_ptr.is_null() {
        return Err(Error::Decode("Bitmap context data is null".into()));
    }

    // The stride asked for is a request. With no buffer supplied, Quartz
    // allocates its own and may pad each row to an alignment it prefers,
    // so the rows are walked at the stride it reports rather than the one
    // that was requested. Reading at the requested stride stays inside the
    // buffer and shears the picture diagonally, with nothing to say so.
    // SAFETY: `ctx` is a live bitmap context.
    let stride = unsafe { CGBitmapContextGetBytesPerRow(ctx.as_ptr() as CGContextRef) };
    if stride < bytes_per_row {
        return Err(Error::Decode("bitmap context rows are narrower than the picture".into()));
    }
    // SAFETY: the buffer Quartz allocated for `ctx` holds `h` rows of the
    // `stride` bytes it reports, and the draw above initialized it. `ctx`
    // owns it and outlives `raw_slice`. If `stride * h` overflowed no such
    // buffer could exist, and `total_bytes` is no larger than it.
    let raw_slice = unsafe { std::slice::from_raw_parts(data_ptr, stride.checked_mul(h).unwrap_or(total_bytes)) };
    let mut rgb = Vec::with_capacity(total_bytes / 4 * 3);
    for row in raw_slice.chunks_exact(stride) {
        for px in row[..bytes_per_row].as_chunks::<4>().0 {
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

/// Encode as AVIF through the system, carrying the colour profile across.
///
/// The encoder is the system's for one reason: no pure-Rust AVIF encoder can
/// embed an ICC profile. `ravif` has no colour handling at all, and the
/// container writer beneath it, `avif-serialize`, writes only an `nclx` colour
/// box — its `ColrBox` hardcodes the four bytes. Measured against a Display P3
/// photograph, `ravif` produced roughly 30% smaller files than JPEG at the same
/// perceptual score and every one of them was untagged, which is the failure
/// this project exists to avoid: read as sRGB, a wide-gamut picture comes out
/// flat. Image I/O writes a `colr` box of type `prof` with the profile inside.
///
/// The cost is that AVIF is written only on macOS, which is the same bargain
/// already struck for reading HEIC.
pub fn encode_avif(image: &crate::Image, quality: f32, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    encode_system(image, quality, icc, c"public.avif", "AVIF", None, None)
}

/// Encode a JPEG through the system encoder, without a gain map.
///
/// Used to search for a quality when the result will carry a gain map: the
/// map does not change the picture, so the picture is searched on its own and
/// the map is attached once, to the encode that was chosen.
pub fn encode_jpeg(image: &crate::Image, quality: f32, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    encode_system(image, quality, icc, c"public.jpeg", "JPEG", None, None)
}

/// Encode a JPEG through the system encoder, carrying the source's gain map.
///
/// Only the system encoder is used here. Measured on an iPhone photograph,
/// macOS refuses to decode a file whose primary came from any libjpeg-family
/// encoder once a gain map is attached (`CMPhotoJFIFUtilities` error -17102),
/// whatever the settings, tables, headers or restart interval; the index and
/// the map were not the cause, since the same ones attached to Apple's own
/// primary decode. So the system writes the whole file and the map is copied
/// across from the source as the system read it.
///
/// `image` must be laid out as the source's pixels were, with `orientation`
/// saying how to turn it, because the map is laid out that way too.
pub fn encode_jpeg_with_gain_map(
    image: &crate::Image,
    quality: f32,
    icc: Option<&[u8]>,
    orientation: u16,
    source: &[u8],
) -> Result<Vec<u8>, Error> {
    encode_system(image, quality, icc, c"public.jpeg", "JPEG", Some(orientation), Some(source))
}

/// A source's gain map as the system reads it.
///
/// The ISO 21496-1 map is preferred and Apple's own kind is the fallback; an
/// iPhone writes both, describing the same map. Apple's kind is applied by two
/// values that live in the maker note rather than beside the map, tags 33 and
/// 48; without them a photograph measured at 4.00 of headroom came back at
/// 3.48. They say how bright the picture may go and nothing about who took it
/// or where, so they are carried and the rest of the maker note is not.
struct GainMap {
    _data: Cf<c_void>,
    _source: Cf<c_void>,
    kind: CFStringRef,
    info: Cf<c_void>,
    headroom: Option<Cf<c_void>>,
}

fn read_gain_map(source: &[u8]) -> Result<GainMap, Error> {
    let data = data_from(source).ok_or_else(|| Error::Encode("Image I/O could not read the source".into()))?;
    let src = image_source(&data).ok_or_else(|| Error::Encode("Image I/O could not read the source".into()))?;
    let index = primary_index(&src);
    let (kind, info) = gain_map_kinds()
        .into_iter()
        // SAFETY: each kind is a framework constant.
        .find_map(|kind| unsafe { auxiliary_data(&src, index, kind) }.map(|info| (kind, info)))
        .ok_or_else(|| Error::Encode("the source's gain map could not be read".into()))?;
    let headroom = maker_headroom(&src, index);
    Ok(GainMap { _data: data, _source: src, kind, info, headroom })
}

/// Maker-note tags 33 and 48 from the source, alone in a dictionary of their
/// own, or nothing if the source has neither.
fn maker_headroom(src: &Cf<c_void>, index: usize) -> Option<Cf<c_void>> {
    let props = properties(src, index)?;
    // SAFETY: a framework constant, initialized before any code here runs and
    // never written.
    let maker_key = unsafe { kCGImagePropertyMakerAppleDictionary };
    // SAFETY: `props` is a live dictionary and `maker_key` a framework constant.
    let maker = unsafe { lookup(props.as_ptr() as CFDictionaryRef, maker_key) };
    if maker.is_null() {
        return None;
    }
    let mut names = Vec::new();
    let (mut keys, mut values) = (Vec::new(), Vec::new());
    for tag in [c"33", c"48"] {
        let name = cf_string(tag)?;
        // SAFETY: `maker` is the maker-note dictionary, borrowed from
        // `props`, which lives to the end of this function; `name` is a live
        // CFString.
        let value = unsafe { lookup(maker as CFDictionaryRef, name.as_ptr() as CFStringRef) };
        if !value.is_null() {
            keys.push(name.as_ptr());
            values.push(value);
        }
        names.push(name);
    }
    if keys.is_empty() {
        return None;
    }
    // SAFETY: the keys are CFStrings held in `names` and the values are
    // borrowed from `props`; both live until this function returns.
    unsafe { dictionary(&keys, &values) }
}

fn encode_system(
    image: &crate::Image,
    quality: f32,
    icc: Option<&[u8]>,
    uti: &CStr,
    what: &str,
    orientation: Option<u16>,
    gain_map_from: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    // Four bytes per pixel with the last ignored: the only RGB layout
    // `CGBitmapContextCreate` accepts at eight bits per component.
    let (w, h) = (image.width, image.height);
    let mut rgbx = Vec::with_capacity(w * h * 4);
    for px in &image.pixels {
        rgbx.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
    }
    // The bitmap context below reads `h` rows of `w * 4` bytes from `rgbx`.
    // `Image` does not tie its pixel count to its size, so the buffer is
    // checked here rather than read past its end.
    if w.checked_mul(h).and_then(|px| px.checked_mul(4)) != Some(rgbx.len()) {
        return Err(Error::Encode(format!("the picture's pixels do not fill its size; no {what} was written")));
    }

    let profile_data = icc.and_then(data_from);
    // A profile the system will not parse is not a reason to write an
    // untagged file; sRGB is what an untagged picture is read as anyway,
    // and saying so explicitly is the closest honest answer.
    let space = profile_data
        .as_ref()
        // SAFETY: `d` is a live CFData holding the profile bytes.
        .and_then(|d| Cf::wrap(unsafe { CGColorSpaceCreateWithICCData(d.as_ptr() as CFDataRef) }))
        .or_else(srgb)
        .ok_or_else(|| Error::Encode("Image I/O could not make a colour space".into()))?;

    // SAFETY: `rgbx` holds exactly `h` rows of `w * 4` bytes (checked above)
    // and is declared before `ctx`, so it outlives the context and every
    // image made from it; `space` is a live colour space.
    let ctx = Cf::wrap(unsafe {
        CGBitmapContextCreate(
            rgbx.as_mut_ptr() as *mut c_void,
            w,
            h,
            8,
            w * 4,
            space.as_ptr() as CGColorSpaceRef,
            K_CG_BITMAP_BYTE_ORDER_DEFAULT | K_CG_IMAGE_ALPHA_NONE_SKIP_LAST,
        )
    })
    .ok_or_else(|| Error::Encode("Image I/O could not make a bitmap context".into()))?;

    // SAFETY: `ctx` is a live bitmap context.
    let cg_image = Cf::wrap(unsafe { CGBitmapContextCreateImage(ctx.as_ptr() as CGContextRef) })
        .ok_or_else(|| Error::Encode("Image I/O could not make a picture".into()))?;

    // SAFETY: a zero capacity means the buffer may grow without limit.
    let out_data = Cf::wrap(unsafe { CFDataCreateMutable(ptr::null(), 0) })
        .ok_or_else(|| Error::Encode("Image I/O could not make an output buffer".into()))?;
    let uti = cf_string(uti).ok_or_else(|| Error::Encode(format!("Image I/O could not name the {what} type")))?;

    // SAFETY: `out_data` is a live mutable CFData and `uti` a live CFString,
    // both retained by the destination; a null options dictionary is allowed.
    let dest = Cf::wrap(unsafe {
        CGImageDestinationCreateWithData(
            out_data.as_ptr() as CFMutableDataRef,
            uti.as_ptr() as CFStringRef,
            1,
            ptr::null(),
        )
    })
    .ok_or_else(|| Error::Encode(format!("this system cannot write {what}")))?;

    // Image I/O takes quality as a fraction, where the engine and every
    // other encoder here count from zero to a hundred.
    let fraction = (quality as f64 / 100.0).clamp(0.0, 1.0);
    // SAFETY: `fraction` is an f64, the size kCFNumberDoubleType reads, and
    // CFNumberCreate copies it.
    let number = Cf::wrap(unsafe {
        CFNumberCreate(ptr::null(), K_CF_NUMBER_DOUBLE_TYPE, (&raw const fraction).cast::<c_void>())
    })
    .ok_or_else(|| Error::Encode("Image I/O could not carry the quality".into()))?;

    // The orientation, when given, is written as a tag rather than applied
    // to the pixels: a gain map copied from the source is laid out the way
    // the source's pixels were, and the picture must stay laid out the same.
    let turn = match orientation {
        Some(o) => {
            let o = i32::from(o);
            // SAFETY: `o` is an i32, the size kCFNumberSInt32Type reads, and
            // CFNumberCreate copies it.
            Some(
                Cf::wrap(unsafe { CFNumberCreate(ptr::null(), K_CF_NUMBER_SINT32_TYPE, (&raw const o).cast::<c_void>()) })
                    .ok_or_else(|| Error::Encode("Image I/O could not carry the orientation".into()))?,
            )
        }
        None => None,
    };
    let carried = match gain_map_from {
        Some(source) => Some(read_gain_map(source)?),
        None => None,
    };
    // SAFETY: framework constants, initialized before any code here runs and
    // never written.
    let quality_key = unsafe { kCGImageDestinationLossyCompressionQuality };
    let mut keys = vec![quality_key];
    let mut values = vec![number.as_ptr()];
    if let Some(t) = &turn {
        // SAFETY: as above.
        keys.push(unsafe { kCGImagePropertyOrientation });
        values.push(t.as_ptr());
    }
    if let Some(h) = carried.as_ref().and_then(|g| g.headroom.as_ref()) {
        // SAFETY: as above.
        keys.push(unsafe { kCGImagePropertyMakerAppleDictionary });
        values.push(h.as_ptr());
    }
    // SAFETY: the keys are framework constants and the values are CFNumbers
    // and a dictionary this function holds until it returns.
    let props = unsafe { dictionary(&keys, &values) }.ok_or_else(|| Error::Encode("Image I/O could not carry the options".into()))?;

    // SAFETY: `dest`, `cg_image` and `props` are live; the destination
    // retains what it keeps.
    unsafe {
        CGImageDestinationAddImage(
            dest.as_ptr() as CGImageDestinationRef,
            cg_image.as_ptr() as CGImageRef,
            props.as_ptr() as CFDictionaryRef,
        )
    };
    if let Some(g) = &carried {
        // SAFETY: `dest` is live, `g.kind` is a framework constant and
        // `g.info` a dictionary `g` owns; the destination retains it.
        unsafe {
            CGImageDestinationAddAuxiliaryDataInfo(
                dest.as_ptr() as CGImageDestinationRef,
                g.kind,
                g.info.as_ptr() as CFDictionaryRef,
            )
        };
    }
    // Finalize is the only place a refused encode is reported. Skipping the
    // check would hand back whatever the buffer happened to hold, which for
    // a refusal is nothing at all.
    // SAFETY: `dest` is a live image destination, finalized once.
    if !unsafe { CGImageDestinationFinalize(dest.as_ptr() as CGImageDestinationRef) } {
        return Err(Error::Encode(format!("Image I/O refused to write this {what}")));
    }

    let encoded = data_bytes(&out_data);
    if encoded.is_empty() {
        return Err(Error::Encode(format!("Image I/O wrote an empty {what}")));
    }
    Ok(encoded)
}
