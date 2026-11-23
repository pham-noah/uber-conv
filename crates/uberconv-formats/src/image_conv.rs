//! Raster images (PNG, JPEG, GIF, BMP, TIFF, WebP, ICO) in-process, plus SVG input.
//!
//! * EXIF orientation is applied on decode, so phone photos don't come out sideways.
//! * 16-bit sources are squashed to 8-bit where the target can't hold them.
//! * WebP uses libwebp: lossy at the configured quality, lossless at 100.
//! * SVG is rasterised with resvg at a resolution you control.

use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::Path,
    sync::{Arc, OnceLock},
};

use image::{
    codecs::{gif::GifEncoder, jpeg::JpegEncoder},
    imageops::FilterType,
    ColorType, DynamicImage, Frame, ImageFormat, Rgb, RgbImage, RgbaImage,
};
use uberconv_core::{Context, Converter, Cost, Edge, Error, Format, Job, Result};

/// Formats the `image` crate reads and writes (WebP goes through libwebp).
const RASTER: &[(Format, ImageFormat)] = &[
    (Format::Png, ImageFormat::Png),
    (Format::Jpeg, ImageFormat::Jpeg),
    (Format::Gif, ImageFormat::Gif),
    (Format::Bmp, ImageFormat::Bmp),
    (Format::Tiff, ImageFormat::Tiff),
    (Format::WebP, ImageFormat::WebP),
    (Format::Ico, ImageFormat::Ico),
];

fn lookup(f: Format) -> Result<ImageFormat> {
    RASTER
        .iter()
        .find(|(x, _)| *x == f)
        .map(|(_, i)| *i)
        .ok_or_else(|| Error::UnknownFormat(f.name().into()))
}

fn img_err(e: image::ImageError) -> Error {
    Error::failed(format!("image error: {e}"))
}

/// JPEG has no alpha channel; composite onto white instead of black.
fn flatten_on_white(img: &DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        out.put_pixel(x, y, Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    out
}

/// Most encoders only take 8 bits per channel (and ICO wants RGBA). Camera and
/// screenshot PNGs/TIFFs are often 16-bit, so squash those down for everything
/// except PNG and TIFF, which can keep the extra precision.
fn normalise_depth(img: DynamicImage, dst: ImageFormat) -> DynamicImage {
    if dst == ImageFormat::Ico {
        return DynamicImage::ImageRgba8(img.to_rgba8());
    }
    let eight_bit = matches!(
        img.color(),
        ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8
    );
    if eight_bit || matches!(dst, ImageFormat::Png | ImageFormat::Tiff) {
        return img;
    }
    if img.color().has_alpha() {
        DynamicImage::ImageRgba8(img.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(img.to_rgb8())
    }
}

// ───────────────────────── EXIF orientation ─────────────────────────

/// EXIF orientation (1..=8); 1 when absent or unreadable.
pub(crate) fn exif_orientation(path: &Path) -> u32 {
    let Ok(file) = File::open(path) else { return 1 };
    exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()
        .and_then(|e| {
            e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
        })
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1)
}

/// Rotate/flip so the pixels are upright. Values follow the EXIF spec.
pub(crate) fn apply_orientation(img: DynamicImage, orientation: u32) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.fliph().rotate270(),
        6 => img.rotate90(),
        7 => img.fliph().rotate90(),
        8 => img.rotate270(),
        _ => img,
    }
}

fn carries_exif(f: Format) -> bool {
    matches!(f, Format::Jpeg | Format::Png | Format::Tiff | Format::WebP)
}

// ───────────────────────── decoding ─────────────────────────

/// Decode any supported image file into pixels (orientation applied).
pub(crate) fn decode(from: Format, path: &Path, ctx: &Context) -> Result<DynamicImage> {
    let img = match from {
        Format::Svg => decode_svg(path, ctx)?,
        Format::WebP => decode_webp(path)?,
        // HEIC carries its rotation as container properties, which both decoders
        // apply themselves; no EXIF step needed.
        Format::Heic => DynamicImage::ImageRgba8(crate::heic::decode(path)?),
        other => {
            let src = lookup(other)?;
            image::io::Reader::with_format(BufReader::new(File::open(path)?), src)
                .decode()
                .map_err(img_err)?
        }
    };
    Ok(if carries_exif(from) { apply_orientation(img, exif_orientation(path)) } else { img })
}

fn decode_webp(path: &Path) -> Result<DynamicImage> {
    let bytes = fs::read(path)?;
    // libwebp handles lossy and lossless stills; animated files fall back to
    // the `image` crate, which reads the first frame.
    if let Some(img) = webp::Decoder::new(&bytes).decode() {
        let (w, h) = (img.width(), img.height());
        let data = img.to_vec();
        let decoded = if img.is_alpha() {
            RgbaImage::from_raw(w, h, data).map(DynamicImage::ImageRgba8)
        } else {
            RgbImage::from_raw(w, h, data).map(DynamicImage::ImageRgb8)
        };
        if let Some(d) = decoded {
            return Ok(d);
        }
    }
    image::io::Reader::with_format(BufReader::new(File::open(path)?), ImageFormat::WebP)
        .decode()
        .map_err(img_err)
}

fn font_db() -> Arc<resvg::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// Rasterise an SVG. Vector art has no native resolution, so unless the user
/// capped the size we render the longest side at 1024 px or its own size,
/// whichever is larger.
fn decode_svg(path: &Path, ctx: &Context) -> Result<DynamicImage> {
    let data = fs::read(path)?;
    let options = resvg::usvg::Options { fontdb: font_db(), ..Default::default() };
    let tree = resvg::usvg::Tree::from_data(&data, &options)
        .map_err(|e| Error::failed(format!("invalid SVG: {e}")))?;
    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    let target = match ctx.options.image_max_dimension {
        Some(m) => m as f32,
        None => longest.max(1024.0),
    }
    .min(16384.0);
    let scale = target / longest;
    let (w, h) = ((size.width() * scale).round().max(1.0) as u32, (size.height() * scale).round().max(1.0) as u32);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| Error::failed("SVG is too large to render"))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());

    // tiny-skia is premultiplied; image formats expect straight alpha.
    let mut buf = pixmap.take();
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(w, h, buf)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| Error::failed("SVG render produced an invalid buffer"))
}

// ───────────────────────── encoding ─────────────────────────

fn encode_webp(img: &DynamicImage, quality: u8, output: &Path) -> Result<()> {
    let (w, h) = (img.width(), img.height());
    if w > 16383 || h > 16383 {
        return Err(Error::failed("WebP supports at most 16383 px per side. Try a smaller “Largest image size”."));
    }
    let lossless = quality >= 100;
    let memory = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        let enc = webp::Encoder::from_rgba(rgba.as_raw(), w, h);
        if lossless { enc.encode_lossless() } else { enc.encode(quality as f32) }
    } else {
        let rgb = img.to_rgb8();
        let enc = webp::Encoder::from_rgb(rgb.as_raw(), w, h);
        if lossless { enc.encode_lossless() } else { enc.encode(quality as f32) }
    };
    fs::write(output, &*memory)?;
    Ok(())
}

/// Resize per the options, then write `img` as `dst`.
pub(crate) fn save_image(mut img: DynamicImage, dst: Format, output: &Path, ctx: &Context) -> Result<()> {
    let target = lookup(dst)?;

    let mut limit = ctx.options.image_max_dimension;
    if dst == Format::Ico {
        limit = Some(limit.map_or(256, |m| m.min(256)));
    }
    if let Some(m) = limit {
        if img.width() > m || img.height() > m {
            img = img.resize(m, m, FilterType::Lanczos3);
        }
    }
    let img = normalise_depth(img, target);

    match target {
        ImageFormat::Gif => {
            // The default quantiser speed (1) takes ~15 s on a 12 MP photo.
            // 10 is ~10x faster with barely visible differences.
            let out = BufWriter::new(File::create(output)?);
            let mut enc = GifEncoder::new_with_speed(out, 10);
            enc.encode_frame(Frame::new(img.to_rgba8())).map_err(img_err)?;
        }
        ImageFormat::Jpeg => {
            let rgb = flatten_on_white(&img);
            let mut out = BufWriter::new(File::create(output)?);
            let quality = ctx.options.image_quality.clamp(1, 100);
            let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
            enc.encode(rgb.as_raw(), rgb.width(), rgb.height(), ColorType::Rgb8).map_err(img_err)?;
            out.flush()?;
        }
        ImageFormat::WebP => encode_webp(&img, ctx.options.image_quality.clamp(1, 100), output)?,
        other => img.save_with_format(output, other).map_err(img_err)?,
    }
    Ok(())
}

pub struct ImageConverter;

impl Converter for ImageConverter {
    fn name(&self) -> &'static str {
        "image"
    }

    fn edges(&self) -> Vec<Edge> {
        let cost = |to: Format, weight: u32| {
            // WebP is lossy below quality 100, so it is flagged lossy.
            if matches!(to, Format::Jpeg | Format::Gif | Format::WebP) {
                Cost::lossy(weight)
            } else {
                Cost::lossless(weight)
            }
        };
        let mut edges = Vec::new();
        for &(from, _) in RASTER {
            for &(to, _) in RASTER {
                if from != to {
                    edges.push(Edge::new(from, to, cost(to, 10)));
                }
            }
        }
        // SVG is input-only: it can become any raster format.
        for &(to, _) in RASTER {
            edges.push(Edge::new(Format::Svg, to, cost(to, 15)));
        }
        // HEIC is input-only too (we can read it, not write it).
        for &(to, _) in RASTER {
            edges.push(Edge::new(Format::Heic, to, cost(to, 15)));
        }
        edges
    }

    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()> {
        ctx.report(0.05);
        let img = decode(job.from, job.input, ctx)?;
        ctx.cancel.check()?;
        ctx.report(0.5);
        save_image(img, job.to, job.output, ctx)?;
        if ctx.options.keep_metadata {
            // Colour profile and EXIF ride along; never fatal.
            crate::meta::carry(job.from, job.to, job.input, job.output);
        }
        ctx.report(1.0);
        Ok(())
    }
}
