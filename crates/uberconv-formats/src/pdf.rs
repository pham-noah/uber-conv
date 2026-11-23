//! PDF <-> images.
//!
//! * PDF -> image: rendered with `hayro` (pure Rust, CPU) on a white page.
//!   One page per conversion (`Options::pdf_page`), or every page at once with
//!   `Options::pdf_all_pages` (page 1 keeps the planned name, the rest are
//!   `name-002.png`, `name-003.png`, …).
//! * image -> PDF: a PDF sized to the image at 150 dpi. JPEGs are embedded as-is
//!   (no re-encoding); everything else is stored Flate-compressed, with a soft
//!   mask when the image has transparency. [`images_to_pdf`] puts several images
//!   into one document, one page each.

use std::{
    fs::{self, File},
    io::{BufReader, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
};

use flate2::{write::ZlibEncoder, Compression};
use image::{DynamicImage, GenericImageView};
use uberconv_core::{Context, Converter, Cost, Edge, Error, Family, Format, Job, Result};

use crate::image_conv::{decode, exif_orientation, save_image};

/// Raster formats a PDF page can be rendered to / assembled from.
const RASTER: &[Format] = &[
    Format::Png,
    Format::Jpeg,
    Format::Gif,
    Format::Bmp,
    Format::Tiff,
    Format::WebP,
    Format::Ico,
];

/// Pixels per inch used both ways. 150 dpi is a good screen/print compromise.
const DPI: f32 = 150.0;

pub struct PdfConverter;

impl Converter for PdfConverter {
    fn name(&self) -> &'static str {
        "pdf"
    }

    fn edges(&self) -> Vec<Edge> {
        let mut edges = Vec::new();
        for &r in RASTER {
            // Rasterising a page can't be undone, so it is flagged lossy.
            edges.push(Edge::new(Format::Pdf, r, Cost::lossy(40)));
            edges.push(Edge::new(r, Format::Pdf, Cost::lossless(30)));
        }
        edges
    }

    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()> {
        if job.from == Format::Pdf {
            pdf_to_image(job, ctx)
        } else {
            image_to_pdf(job, ctx)
        }
    }
}

// ───────────────────────── PDF -> image ─────────────────────────

fn pdf_to_image(job: &Job<'_>, ctx: &Context) -> Result<()> {
    ctx.report(0.02);
    let data = fs::read(job.input)?;
    let max_dim = ctx.options.image_max_dimension;
    let all_pages = ctx.options.pdf_all_pages;
    let wanted = ctx.options.pdf_page.max(1) as usize;

    // Rendering untrusted PDFs can hit bugs in any renderer; never let one take
    // the whole app down.
    let pdf = catch_unwind(AssertUnwindSafe(|| open_pdf(data)))
        .map_err(|_| Error::failed("the PDF renderer crashed on this file"))??;
    let page_count = pdf.pages().len();
    if page_count == 0 {
        return Err(Error::failed("this PDF has no pages"));
    }
    ctx.cancel.check()?;

    let first_page = if all_pages { 1 } else { wanted };
    let rendered = catch_unwind(AssertUnwindSafe(|| render_page(&pdf, first_page, max_dim)))
        .map_err(|_| Error::failed("the PDF renderer crashed on this file"))??;
    ctx.cancel.check()?;
    ctx.report(0.4);
    save_image(rendered, job.to, job.output, ctx)?;

    if all_pages {
        // Pages 2..n land beside page 1, zero-padded so they sort in order.
        let stem = job.output.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = job.output.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let dir = job.output.parent().map(Path::to_path_buf).unwrap_or_default();
        for n in 2..=page_count {
            ctx.cancel.check()?;
            let rendered = catch_unwind(AssertUnwindSafe(|| render_page(&pdf, n, max_dim)))
                .map_err(|_| Error::failed(format!("the PDF renderer crashed on page {n}")))?;
            let out = dir.join(format!("{stem}-{n:03}.{ext}"));
            save_image(rendered?, job.to, &out, ctx)?;
            ctx.report(0.4 + 0.6 * n as f32 / page_count as f32);
        }
    }
    ctx.report(1.0);
    Ok(())
}

fn open_pdf(data: Vec<u8>) -> Result<hayro::hayro_syntax::Pdf> {
    hayro::hayro_syntax::Pdf::new(data)
        .map_err(|e| Error::failed(format!("could not open the PDF (encrypted or damaged?): {e:?}")))
}

fn render_page(pdf: &hayro::hayro_syntax::Pdf, page_number: usize, max_dim: Option<u32>) -> Result<DynamicImage> {
    use hayro::{hayro_interpret::InterpreterSettings, PixmapSettings, RenderCache, RenderSettings};

    let pages = pdf.pages();
    let page = pages.get(page_number - 1).ok_or_else(|| {
        Error::failed(format!(
            "this PDF has {} page{}, page {page_number} doesn't exist",
            pages.len(),
            if pages.len() == 1 { "" } else { "s" }
        ))
    })?;

    let (w, h) = page.render_dimensions();
    let longest = w.max(h).max(1.0);
    let scale = match max_dim {
        Some(m) => m as f32 / longest,
        None => DPI / 72.0,
    }
    .min(16000.0 / longest); // pixmaps are limited to 16 bit dimensions

    let settings = PixmapSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
    };
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &settings,
    );

    let (pw, ph) = (pixmap.width() as u32, pixmap.height() as u32);
    // On an opaque white background every pixel is fully opaque, so the
    // premultiplied buffer is also valid straight RGBA.
    image::RgbaImage::from_raw(pw, ph, pixmap.data_as_u8_slice().to_vec())
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| Error::failed("the PDF renderer returned an unexpected buffer"))
}

// ───────────────────────── image -> PDF ─────────────────────────

/// How an image is stored inside the PDF.
struct Embedded {
    width: u32,
    height: u32,
    /// `/DeviceRGB` or `/DeviceGray`
    color_space: &'static str,
    /// Extra dictionary entries: `/Filter ...`
    filter: &'static str,
    data: Vec<u8>,
    /// Alpha channel as 8-bit gray, Flate-compressed.
    soft_mask: Option<Vec<u8>>,
}

fn zlib(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(6));
    enc.write_all(bytes)?;
    Ok(enc.finish()?)
}

/// A JPEG we can embed untouched: upright, and 1 or 3 components.
fn jpeg_passthrough(path: &Path) -> Option<Embedded> {
    use image::{codecs::jpeg::JpegDecoder, ColorType, ImageDecoder};
    if exif_orientation(path) != 1 {
        return None; // pixels need rotating, so re-encode instead
    }
    let decoder = JpegDecoder::new(BufReader::new(File::open(path).ok()?)).ok()?;
    let (width, height) = decoder.dimensions();
    let color_space = match decoder.color_type() {
        ColorType::L8 => "/DeviceGray",
        ColorType::Rgb8 => "/DeviceRGB",
        _ => return None,
    };
    Some(Embedded { width, height, color_space, filter: "/Filter /DCTDecode", data: fs::read(path).ok()?, soft_mask: None })
}

fn embed_pixels(img: DynamicImage) -> Result<Embedded> {
    let (width, height) = img.dimensions();
    let has_alpha = img.color().has_alpha();
    let rgba = img.to_rgba8();

    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    let mut alpha = Vec::with_capacity((width * height) as usize);
    for p in rgba.pixels() {
        rgb.extend_from_slice(&p.0[..3]);
        alpha.push(p.0[3]);
    }
    let soft_mask = if has_alpha && alpha.iter().any(|&a| a != 255) { Some(zlib(&alpha)?) } else { None };
    Ok(Embedded { width, height, color_space: "/DeviceRGB", filter: "/Filter /FlateDecode", data: zlib(&rgb)?, soft_mask })
}

fn image_to_pdf(job: &Job<'_>, ctx: &Context) -> Result<()> {
    ctx.report(0.1);
    let embedded = embed_one(job.from, job.input, ctx)?;
    ctx.cancel.check()?;
    ctx.report(0.7);
    fs::write(job.output, build_pdf(&[embedded]))?;
    ctx.report(1.0);
    Ok(())
}

fn embed_one(from: Format, path: &Path, ctx: &Context) -> Result<Embedded> {
    if from == Format::Jpeg {
        if let Some(e) = jpeg_passthrough(path) {
            return Ok(e);
        }
    }
    // decode() already applies EXIF orientation
    embed_pixels(decode(from, path, ctx)?)
}

/// Assemble several images into one PDF: one page per image, in the order given.
///
/// Not part of the conversion graph (the graph is one input to one output, this
/// is many to one), so callers that want a single document use this directly:
/// the CLI's `--combine` flag and the app's "Combine into PDF".
pub fn images_to_pdf(inputs: &[PathBuf], output: &Path, ctx: &Context) -> Result<()> {
    if inputs.is_empty() {
        return Err(Error::failed("nothing to combine"));
    }
    let mut pages = Vec::with_capacity(inputs.len());
    for (i, path) in inputs.iter().enumerate() {
        ctx.cancel.check()?;
        let from = Format::from_path(path)
            .ok_or_else(|| Error::failed(format!("{}: can't tell the input format from its extension", path.display())))?;
        if from.family() != Family::Image && from != Format::Pdf {
            return Err(Error::failed(format!("{}: only images (and PDF pages) can go into a PDF", path.display())));
        }
        if from == Format::Pdf {
            return Err(Error::failed(format!("{}: combining PDFs is not supported yet", path.display())));
        }
        pages.push(embed_one(from, path, ctx)?);
        ctx.report((i + 1) as f32 / (inputs.len() + 1) as f32);
    }
    fs::write(output, build_pdf(&pages))?;
    ctx.report(1.0);
    Ok(())
}

/// Assemble a PDF with one page per image.
fn build_pdf(images: &[Embedded]) -> Vec<u8> {
    // Page size in points, capped at the 200 inch PDF limit.
    let sizes: Vec<(f32, f32)> = images
        .iter()
        .map(|img| {
            let mut pt_w = img.width as f32 * 72.0 / DPI;
            let mut pt_h = img.height as f32 * 72.0 / DPI;
            let cap = 14400.0 / pt_w.max(pt_h);
            if cap < 1.0 {
                pt_w *= cap;
                pt_h *= cap;
            }
            (pt_w, pt_h)
        })
        .collect();

    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    let stream = |dict: String, data: &[u8]| -> Vec<u8> {
        let mut b = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        b.extend_from_slice(data);
        b.extend_from_slice(b"\nendstream");
        b
    };

    // Object ids: 1 Catalog, 2 Pages, then (page, contents, image[, smask]) per
    // page, then Info. They are emitted in exactly that order, so the ids line up.
    let mut next = 3;
    let mut ids: Vec<(usize, usize, usize, Option<usize>)> = Vec::with_capacity(images.len());
    for img in images {
        let smask = img.soft_mask.as_ref().map(|_| next + 3);
        ids.push((next, next + 1, next + 2, smask));
        next += if smask.is_some() { 4 } else { 3 };
    }
    let info_id = next;

    object(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = ids.iter().map(|(page, ..)| format!("{page} 0 R")).collect();
    object(
        &mut out,
        &mut offsets,
        format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), images.len()).as_bytes(),
    );

    for (i, img) in images.iter().enumerate() {
        let (_, content_id, image_id, smask_id) = ids[i];
        let (pt_w, pt_h) = sizes[i];
        object(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pt_w:.2} {pt_h:.2}] /Resources << /XObject << /Im0 {image_id} 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .as_bytes(),
        );
        let content = format!("q {pt_w:.2} 0 0 {pt_h:.2} 0 0 cm /Im0 Do Q");
        object(&mut out, &mut offsets, &stream(String::new(), content.as_bytes()));

        let smask = if smask_id.is_some() { format!(" /SMask {} 0 R", smask_id.unwrap()) } else { String::new() };
        object(
            &mut out,
            &mut offsets,
            &stream(
                format!(
                    "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {} /BitsPerComponent 8 {}{}",
                    img.width, img.height, img.color_space, img.filter, smask
                ),
                &img.data,
            ),
        );
        if let Some(mask) = &img.soft_mask {
            object(
                &mut out,
                &mut offsets,
                &stream(
                    format!(
                        "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
                        img.width, img.height
                    ),
                    mask,
                ),
            );
        }
    }
    object(&mut out, &mut offsets, b"<< /Producer (uberconv) >>");

    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info {info_id} 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}
