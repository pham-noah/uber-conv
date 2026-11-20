//! HEIC/HEIF decoding.
//!
//! HEIC is HEVC-in-ISOBMFF; the `image` crate can't touch it, so there are two
//! ways in, tried in this order:
//!
//! 1. **libheif** — bundled, cross-platform. Detected at build time through
//!    `UBERCONV_LIBHEIF_DIR` (a directory holding `include/libheif/heif.h` and
//!    `lib/heif.lib` | `lib/libheif.a`); see `scripts/build-libheif.sh`.
//! 2. **Windows Imaging Component** — present on every Windows box that has
//!    Microsoft's "HEIF Image Extensions" (and, for HEVC-coded files, the HEVC
//!    video extension). No build-time dependency, which is why Windows builds
//!    can decode HEIC out of the box.
//!
//! Without either, `decode` returns an error naming the missing piece rather
//! than a generic format failure.

use std::path::Path;

use uberconv_core::{Error, Result};

/// Can this build decode HEIC at all?
pub fn available() -> bool {
    #[cfg(heic_libheif)]
    {
        return true;
    }
    #[cfg(all(windows, not(heic_libheif)))]
    {
        return true;
    }
    #[allow(unreachable_code)]
    false
}

/// One-line explanation for the UI/CLI when HEIC is not decodable here.
pub fn unavailable_reason() -> &'static str {
    "HEIC needs libheif. Rebuild with UBERCONV_LIBHEIF_DIR pointing at a libheif \
     install (see scripts/build-libheif.sh)."
}

/// Decode the primary image of a HEIC/HEIF file to 8-bit RGBA.
pub fn decode(path: &Path) -> Result<image::RgbaImage> {
    #[cfg(heic_libheif)]
    {
        return libheif::decode(path);
    }
    #[cfg(all(windows, not(heic_libheif)))]
    {
        return wic::decode(path);
    }
    #[allow(unreachable_code)]
    {
        let _ = path;
        Err(Error::failed(unavailable_reason()))
    }
}

// ───────────────────────────── libheif ─────────────────────────────

#[cfg(heic_libheif)]
mod libheif {
    use std::{
        ffi::{c_char, c_int, c_void, CString},
        os::windows::ffi::OsStrExt,
        path::Path,
        ptr,
    };

    use uberconv_core::{Error, Result};

    #[repr(C)]
    struct HeifContext {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct HeifImageHandle {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct HeifImage {
        _private: [u8; 0],
    }

    // heif_colorspace / heif_chroma / heif_channel values from heif.h.
    const HEIF_COLORSPACE_RGB: c_int = 1;
    const HEIF_CHROMA_INTERLEAVED_RGBA: c_int = 12;
    const HEIF_CHANNEL_INTERLEAVED: c_int = 10;

    #[link(name = "heif")]
    extern "C" {
        fn heif_context_alloc() -> *mut HeifContext;
        fn heif_context_free(ctx: *mut HeifContext);
        fn heif_context_read_from_file(
            ctx: *mut HeifContext,
            filename: *const c_char,
            read_options: *const c_void,
        ) -> c_int;
        fn heif_context_get_primary_image_handle(
            ctx: *mut HeifContext,
            handle: *mut *mut HeifImageHandle,
        ) -> c_int;
        fn heif_image_handle_release(handle: *mut HeifImageHandle);
        fn heif_image_handle_get_width(handle: *const HeifImageHandle) -> c_int;
        fn heif_image_handle_get_height(handle: *const HeifImageHandle) -> c_int;
        fn heif_decode_image(
            handle: *const HeifImageHandle,
            out: *mut *mut HeifImage,
            colorspace: c_int,
            chroma: c_int,
            options: *const c_void,
        ) -> c_int;
        fn heif_image_release(img: *mut HeifImage);
        fn heif_image_get_plane_readonly(
            img: *const HeifImage,
            channel: c_int,
            out_stride: *mut c_int,
        ) -> *const u8;
    }

    fn err(code: c_int, what: &str) -> Error {
        Error::failed(format!("libheif: {what} failed (code {code})"))
    }

    pub fn decode(path: &Path) -> Result<image::RgbaImage> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // libheif wants a UTF-8 path on every platform; build one from the wide form.
        let utf8 = String::from_utf16_lossy(&wide[..wide.len() - 1]);
        let c_path = CString::new(utf8).map_err(|_| Error::failed("path contains a NUL byte"))?;

        unsafe {
            let ctx = heif_context_alloc();
            if ctx.is_null() {
                return Err(Error::failed("libheif: out of memory"));
            }
            let code = heif_context_read_from_file(ctx, c_path.as_ptr(), ptr::null());
            if code != 0 {
                heif_context_free(ctx);
                return Err(err(code, "reading the file"));
            }
            let mut handle: *mut HeifImageHandle = ptr::null_mut();
            let code = heif_context_get_primary_image_handle(ctx, &mut handle);
            heif_context_free(ctx);
            if code != 0 || handle.is_null() {
                return Err(err(code, "the primary image"));
            }

            let (w, h) = (
                heif_image_handle_get_width(handle),
                heif_image_handle_get_height(handle),
            );
            let mut img: *mut HeifImage = ptr::null_mut();
            let code = heif_decode_image(
                handle,
                &mut img,
                HEIF_COLORSPACE_RGB,
                HEIF_CHROMA_INTERLEAVED_RGBA,
                ptr::null(),
            );
            heif_image_handle_release(handle);
            if code != 0 || img.is_null() {
                return Err(err(code, "decoding RGBA pixels"));
            }

            let mut stride: c_int = 0;
            let plane = heif_image_get_plane_readonly(img, HEIF_CHANNEL_INTERLEAVED, &mut stride);
            if plane.is_null() || w <= 0 || h <= 0 || stride < w * 4 {
                heif_image_release(img);
                return Err(Error::failed("libheif: no RGBA plane"));
            }
            let (w, h, stride) = (w as usize, h as usize, stride as usize);
            let mut out = vec![0u8; w * h * 4];
            for y in 0..h {
                let src = std::slice::from_raw_parts(plane.add(y * stride), w * 4);
                out[y * w * 4..(y + 1) * w * 4].copy_from_slice(src);
            }
            heif_image_release(img);
            image::RgbaImage::from_raw(w as u32, h as u32, out)
                .ok_or_else(|| Error::failed("libheif: decoded buffer has the wrong size"))
        }
    }
}

// ─────────────────────── Windows Imaging Component ───────────────────────

#[cfg(all(windows, not(heic_libheif)))]
mod wic {
    use std::path::Path;

    use uberconv_core::{Error, Result};
    use windows::{
        core::HSTRING,
        Win32::{
            Foundation::GENERIC_READ,
            Graphics::Imaging::{
                CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
                WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad,
            },
            System::Com::{
                CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
            },
        },
    };

    fn wic_err(what: &str, e: windows::core::Error) -> Error {
        Error::failed(format!(
            "HEIC: {what} failed ({e}). Windows needs the “HEIF Image Extensions” (and, for HEVC files, the HEVC video extension) from the Microsoft Store."
        ))
    }

    pub fn decode(path: &Path) -> Result<image::RgbaImage> {
        // Workers are not COM-initialised; S_FALSE (already initialised) is fine too.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| wic_err("starting WIC", e))?;

            let hpath = HSTRING::from(path.as_os_str());
            let decoder = factory
                .CreateDecoderFromFilename(
                    &hpath,
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnLoad,
                )
                .map_err(|e| wic_err("opening the file (unsupported HEIC variant?)", e))?;

            let frame = decoder.GetFrame(0).map_err(|e| wic_err("reading frame 0", e))?;
            let (mut w, mut h) = (0u32, 0u32);
            frame
                .GetSize(&mut w, &mut h)
                .map_err(|e| wic_err("reading the size", e))?;

            let converter = factory
                .CreateFormatConverter()
                .map_err(|e| wic_err("creating the pixel converter", e))?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppRGBA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeMedianCut,
                )
                .map_err(|e| wic_err("converting to RGBA", e))?;

            let stride = w * 4;
            let mut buf = vec![0u8; (stride * h) as usize];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut buf)
                .map_err(|e| wic_err("copying pixels", e))?;

            image::RgbaImage::from_raw(w, h, buf)
                .ok_or_else(|| Error::failed("HEIC: decoded buffer has the wrong size"))
        }
    }
}
