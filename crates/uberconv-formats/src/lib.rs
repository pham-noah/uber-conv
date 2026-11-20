//! Built-in converters for uberconv.
//!
//! * `image`   - PNG, JPEG, GIF, BMP, TIFF, WebP, ICO + SVG and HEIC input (in-process)
//! * `data`    - JSON, YAML, TOML, CSV
//! * `text`    - Markdown, HTML, plain text
//! * `docx`    - Word <-> Markdown
//! * `pdf`     - PDF <-> images
//! * `archive` - ZIP, TAR, TAR.GZ
//! * `ffmpeg`  - audio + video (+ video -> GIF, AVIF), via a bundled FFmpeg sidecar binary

pub mod archive;
pub mod data;
pub mod docx;
pub mod ffmpeg;
pub mod heic;
pub mod image_conv;
pub mod pdf;
pub mod text;

use uberconv_core::Registry;

/// A registry with everything this build can do. FFmpeg-backed formats are
/// only registered when an FFmpeg binary can be located (see
/// [`ffmpeg::locate_ffmpeg`]).
pub fn default_registry() -> Registry {
    let mut r = Registry::new();
    r.register(Box::new(image_conv::ImageConverter));
    r.register(Box::new(data::DataConverter));
    r.register(Box::new(text::TextConverter));
    r.register(Box::new(docx::DocxConverter));
    r.register(Box::new(pdf::PdfConverter));
    r.register(Box::new(archive::ArchiveConverter));
    if let Some(ff) = ffmpeg::FfmpegConverter::locate() {
        r.register(Box::new(ff));
    }
    r
}

pub fn ffmpeg_available() -> bool {
    ffmpeg::locate_ffmpeg().is_some()
}
