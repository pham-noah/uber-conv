//! PDF <-> images.
//!
//! * PDF -> image: rendered with `hayro` (pure Rust, CPU) on a white page.
//!   One page per conversion; pick it with `Options::pdf_page`.
//! * image -> PDF: a single-page PDF sized to the image at 150 dpi. JPEGs are
//!   embedded as-is (no re-encoding); everything else is stored Flate-compressed,
//!   with a soft mask when the image has transparency.

use std::{
    fs::{self, File},
    io::{BufReader, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
};

use flate2::{write::ZlibEncoder, Compression};
use image::{DynamicImage, GenericImageView};
use uberconv_core::{Context, Converter, Cost, Edge, Error, Format, Job, Result};

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
    ctx.report(0.05);
    let data = fs::read(job.input)?;
    let wanted = ctx.options.pdf_page.max(1) as usize;
    let max_dim = ctx.options.image_max_dimension;

    // Rendering untrusted PDFs can hit bugs in any renderer; never let one take
    // the whole app down.
    let rendered = catch_unwind(AssertUnwindSafe(|| render_page(data, wanted, max_dim)))
        .map_err(|_| Error::failed("the PDF renderer crashed on this file"))??;
    ctx.cancel.check()?;
    ctx.report(0.8);

    save_image(rendered, job.to, job.output, ctx)?;
    ctx.report(1.0);
    Ok(())
}

fn render_page(data: Vec<u8>, page_number: usize, max_dim: Option<u32>) -> Result<DynamicImage> {
    use hayro::{hayro_interpret::InterpreterSettings, hayro_syntax::Pdf, PixmapSettings, RenderCache, RenderSettings};

    let pdf = Pdf::new(data).map_err(|e| Error::failed(format!("could not open the PDF (encrypted or damaged?): {e:?}")))?;
    let pages = pdf.pages();
    let page = pages.get(page_number - 1).ok_or_else(|| {
        Error::failed(format!("this PDF has {} page{}, page {page_number} doesn't exist", pages.len(), if pages.len() == 1 { "" } else { "s" }))
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
    let embedded = match (job.from == Format::Jpeg).then(|| jpeg_passthrough(job.input)).flatten() {
        Some(e) => e,
        None => {
            // decode() already applies EXIF orientation
            embed_pixels(decode(job.from, job.input, ctx)?)?
        }
    };
    ctx.cancel.check()?;
    ctx.report(0.7);
    fs::write(job.output, build_pdf(&embedded))?;
    ctx.report(1.0);
    Ok(())
}

/// Assemble a one-page PDF around `img`.
fn build_pdf(img: &Embedded) -> Vec<u8> {
    // Page size in points, capped at the 200 inch PDF limit.
    let mut pt_w = img.width as f32 * 72.0 / DPI;
    let mut pt_h = img.height as f32 * 72.0 / DPI;
    let cap = 14400.0 / pt_w.max(pt_h);
    if cap < 1.0 {
        pt_w *= cap;
        pt_h *= cap;
    }

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

    let has_mask = img.soft_mask.is_some();
    // 1 Catalog, 2 Pages, 3 Page, 4 Contents, 5 Image, [6 SMask], last Info
    object(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(&mut out, &mut offsets, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    object(
        &mut out,
        &mut offsets,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pt_w:.2} {pt_h:.2}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
        )
        .as_bytes(),
    );
    let content = format!("q {pt_w:.2} 0 0 {pt_h:.2} 0 0 cm /Im0 Do Q");
    object(&mut out, &mut offsets, &stream(String::new(), content.as_bytes()));

    let smask = if has_mask { " /SMask 6 0 R" } else { "" };
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
    let info_id = offsets.len() + 1;
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
