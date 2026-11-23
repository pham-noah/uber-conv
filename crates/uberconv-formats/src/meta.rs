//! Carrying colour profiles and EXIF into converted images.
//!
//! Photos are written with an ICC profile (and often a rotation tag). The
//! encoder we use has no way to set either, so the profile is lifted out of the
//! source container and spliced into the freshly encoded one:
//!
//! * read  — JPEG (APP1/APP2), PNG (iCCP/eXIf), WebP (EXIF/ICCP chunks), TIFF (tag 34675)
//! * write — JPEG (APP1/APP2 after SOI), PNG (iCCP/eXIf chunks after IHDR)
//!
//! Everything here is best effort: a file that converts but loses its profile is
//! still better than a failed conversion, so callers ignore errors.

use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use uberconv_core::Format;

#[derive(Default, Debug)]
pub struct Meta {
    /// Raw ICC profile bytes.
    pub icc: Option<Vec<u8>>,
    /// A JPEG-style `Exif\0\0` + TIFF block.
    pub exif: Option<Vec<u8>>,
}

impl Meta {
    fn is_empty(&self) -> bool {
        self.icc.is_none() && self.exif.is_none()
    }
}

/// `None` when nothing useful came out; the caller never has to care.
pub fn read(from: Format, path: &Path) -> Option<Meta> {
    let bytes = fs::read(path).ok()?;
    let meta = match from {
        Format::Jpeg => read_jpeg(&bytes),
        Format::Png => read_png(&bytes),
        Format::WebP => read_webp(&bytes),
        Format::Tiff => read_tiff(&bytes),
        _ => Meta::default(),
    };
    (!meta.is_empty()).then_some(meta)
}

/// Splice `meta` into the file at `output`. Errors are the caller's to ignore.
pub fn write(to: Format, output: &Path, meta: &Meta) -> std::io::Result<()> {
    match to {
        Format::Jpeg => write_jpeg(output, meta),
        Format::Png => write_png(output, meta),
        // WebP would need the VP8X feature byte rewritten as well; skipped on
        // purpose rather than risk emitting a file decoders reject.
        _ => Ok(()),
    }
}

/// Copy the source file's profile/EXIF into the file we just encoded.
pub fn carry(from: Format, to: Format, input: &Path, output: &Path) {
    if !matches!(to, Format::Jpeg | Format::Png | Format::WebP) {
        return;
    }
    let Some(meta) = read(from, input) else { return };
    let _ = write(to, output, &meta);
}

// ───────────────────────────── JPEG ─────────────────────────────

const ICC_TAG: &[u8] = b"ICC_PROFILE\0";
const EXIF_TAG: &[u8] = b"Exif\0\0";

fn read_jpeg(bytes: &[u8]) -> Meta {
    let mut meta = Meta::default();
    // ICC profiles can span several APP2 segments; keep them by sequence number.
    let mut chunks: Vec<(u8, Vec<u8>)> = Vec::new();
    for (marker, payload) in jpeg_segments(bytes) {
        match marker {
            0xE1 if payload.starts_with(EXIF_TAG) => {
                meta.exif.get_or_insert_with(|| payload.to_vec());
            }
            0xE2 if payload.starts_with(ICC_TAG) && payload.len() > ICC_TAG.len() + 2 => {
                let seq = payload[ICC_TAG.len()];
                chunks.push((seq, payload[ICC_TAG.len() + 2..].to_vec()));
            }
            _ => {}
        }
    }
    if !chunks.is_empty() {
        chunks.sort_by_key(|(seq, _)| *seq);
        meta.icc = Some(chunks.into_iter().flat_map(|(_, c)| c).collect());
    }
    meta
}

/// `(marker, payload)` for each metadata segment before the image scan starts.
fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return out;
    }
    let mut i = 2usize;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            break;
        }
        let marker = bytes[i + 1];
        // Standalone markers carry no length field.
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        // Start of scan / end of image: everything after is entropy-coded data.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            break;
        }
        out.push((marker, &bytes[i + 4..i + 2 + len]));
        i += 2 + len;
    }
    out
}

fn write_jpeg(output: &Path, meta: &Meta) -> std::io::Result<()> {
    let bytes = fs::read(output)?;
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Ok(());
    }
    let mut segments: Vec<u8> = Vec::new();
    if let Some(exif) = &meta.exif {
        // The pixels were already rotated on decode, so a surviving orientation
        // tag would rotate them twice. Drop the block rather than risk that.
        let mut exif = exif.clone();
        if reset_orientation(&mut exif) {
            push_segment(&mut segments, 0xE1, &exif);
        }
    }
    if let Some(icc) = &meta.icc {
        // 65519 bytes of payload per segment, minus the 14-byte ICC header.
        const MAX: usize = 65519 - 14;
        let total = icc.len().div_ceil(MAX).max(1).min(255) as u8;
        for (i, chunk) in icc.chunks(MAX).take(total as usize).enumerate() {
            let mut payload = Vec::with_capacity(14 + chunk.len());
            payload.extend_from_slice(ICC_TAG);
            payload.push(i as u8 + 1);
            payload.push(total);
            payload.extend_from_slice(chunk);
            push_segment(&mut segments, 0xE2, &payload);
        }
    }
    if segments.is_empty() {
        return Ok(());
    }
    let mut out = Vec::with_capacity(bytes.len() + segments.len());
    out.extend_from_slice(&bytes[..2]);
    out.extend_from_slice(&segments);
    out.extend_from_slice(&bytes[2..]);
    fs::write(output, out)
}

fn push_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    let len = payload.len() + 2;
    if len > u16::MAX as usize {
        return;
    }
    out.push(0xFF);
    out.push(marker);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

/// Set the EXIF Orientation tag to 1. `true` when the block is well formed.
fn reset_orientation(exif: &mut [u8]) -> bool {
    if exif.len() < 6 + 8 || !exif.starts_with(EXIF_TAG) {
        return false;
    }
    let tiff = &mut exif[6..];
    let le = match &tiff[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return false,
    };
    let u16_at = |b: &[u8], i: usize| -> Option<u16> {
        let s: [u8; 2] = b.get(i..i + 2)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
    };
    let u32_at = |b: &[u8], i: usize| -> Option<u32> {
        let s: [u8; 4] = b.get(i..i + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    };
    let Some(ifd0) = u32_at(tiff, 4).map(|v| v as usize) else { return false };
    let Some(count) = u16_at(tiff, ifd0).map(|v| v as usize) else { return false };
    for n in 0..count {
        let at = ifd0 + 2 + n * 12;
        if u16_at(tiff, at) == Some(0x0112) {
            let v = if le { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() };
            if at + 10 > tiff.len() {
                return false;
            }
            tiff[at + 8..at + 10].copy_from_slice(&v);
            return true;
        }
    }
    // No orientation tag: nothing to undo, so the block is safe to copy as-is.
    true
}

// ───────────────────────────── PNG ─────────────────────────────

fn read_png(bytes: &[u8]) -> Meta {
    let mut meta = Meta::default();
    for (kind, data) in png_chunks(bytes) {
        match kind.as_str() {
            "iCCP" => {
                // name\0 method\0 zlib(profile)
                if let Some(nul) = data.iter().position(|b| *b == 0) {
                    let rest = &data[nul + 1..];
                    if rest.len() > 1 && rest[0] == 0 {
                        let mut raw = Vec::new();
                        if ZlibDecoder::new(&rest[1..]).read_to_end(&mut raw).is_ok() && !raw.is_empty() {
                            meta.icc = Some(raw);
                        }
                    }
                }
            }
            "eXIf" => {
                let mut exif = EXIF_TAG.to_vec();
                exif.extend_from_slice(&data);
                meta.exif = Some(exif);
            }
            _ => {}
        }
    }
    meta
}

fn png_chunks(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 8usize;
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let kind = String::from_utf8_lossy(&bytes[i + 4..i + 8]).to_string();
        let start = i + 8;
        if start + len + 4 > bytes.len() {
            break;
        }
        out.push((kind.clone(), bytes[start..start + len].to_vec()));
        if kind == "IEND" {
            break;
        }
        i = start + len + 4;
    }
    out
}

fn write_png(output: &Path, meta: &Meta) -> std::io::Result<()> {
    let bytes = fs::read(output)?;
    // IHDR is always the first chunk: 8-byte signature + 25-byte chunk.
    if bytes.len() < 33 || &bytes[12..16] != b"IHDR" {
        return Ok(());
    }
    let mut extra: Vec<u8> = Vec::new();
    if let Some(icc) = &meta.icc {
        let mut packed = Vec::new();
        {
            let mut enc = ZlibEncoder::new(&mut packed, Compression::fast());
            enc.write_all(icc)?;
            enc.finish()?;
        }
        let mut data = b"ICC Profile\0".to_vec();
        data.push(0); // deflate
        data.extend_from_slice(&packed);
        push_png_chunk(&mut extra, "iCCP", &data);
    }
    if let Some(exif) = &meta.exif {
        if let Some(tiff) = exif.strip_prefix(EXIF_TAG) {
            push_png_chunk(&mut extra, "eXIf", tiff);
        }
    }
    if extra.is_empty() {
        return Ok(());
    }
    let mut out = Vec::with_capacity(bytes.len() + extra.len());
    out.extend_from_slice(&bytes[..33]);
    out.extend_from_slice(&extra);
    out.extend_from_slice(&bytes[33..]);
    fs::write(output, out)
}

fn push_png_chunk(out: &mut Vec<u8>, kind: &str, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind.as_bytes());
    crc_input.extend_from_slice(data);
    out.extend_from_slice(kind.as_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ───────────────────────────── WebP ─────────────────────────────

fn read_webp(bytes: &[u8]) -> Meta {
    let mut meta = Meta::default();
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return meta;
    }
    let mut i = 12usize;
    while i + 8 <= bytes.len() {
        let kind = &bytes[i..i + 4];
        let len = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        let start = i + 8;
        if start + len > bytes.len() {
            break;
        }
        match kind {
            b"ICCP" => meta.icc = Some(bytes[start..start + len].to_vec()),
            b"EXIF" => {
                let raw = &bytes[start..start + len];
                let mut exif = EXIF_TAG.to_vec();
                exif.extend_from_slice(raw.strip_prefix(EXIF_TAG).unwrap_or(raw));
                meta.exif = Some(exif);
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        i = start + len + (len & 1);
    }
    meta
}

// ───────────────────────────── TIFF ─────────────────────────────

fn read_tiff(bytes: &[u8]) -> Meta {
    let mut meta = Meta::default();
    if bytes.len() < 8 {
        return meta;
    }
    let le = match &bytes[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return meta,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let s: [u8; 2] = bytes.get(i..i + 2)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let s: [u8; 4] = bytes.get(i..i + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    };
    let Some(ifd0) = u32_at(4).map(|v| v as usize) else { return meta };
    let Some(count) = u16_at(ifd0).map(|v| v as usize) else { return meta };
    for n in 0..count {
        let at = ifd0 + 2 + n * 12;
        // 34675 = InterColorProfile: LONG(4)/UNDEFINED(7), count = byte length.
        if u16_at(at) == Some(34675) {
            if let (Some(len), Some(off)) = (u32_at(at + 4), u32_at(at + 8)) {
                let (len, off) = (len as usize, off as usize);
                if off + len <= bytes.len() {
                    meta.icc = Some(bytes[off..off + len].to_vec());
                }
            }
            break;
        }
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_segment_walk_stops_at_scan() {
        // SOI, APP0 (len 6), SOS, then entropy data that looks like a marker.
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x06, 0x00, 0x01, 0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xE2];
        jpg.extend_from_slice(b"junk");
        let segs = jpeg_segments(&jpg);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, 0xE0);
    }

    #[test]
    fn icc_survives_a_png_to_jpeg_to_png_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        // A tiny PNG whose only interesting feature is an iCCP chunk.
        let profile: Vec<u8> = (0..600u16).map(|i| (i % 251) as u8).collect();
        let mut packed = Vec::new();
        ZlibEncoder::new(&mut packed, Compression::fast()).write_all(&profile).unwrap();
        let (dir_p, src) = (dir.path(), dir.path().join("src.png"));
        let _ = dir_p;
        let mut data = b"ICC Profile\0".to_vec();
        data.push(0);
        data.extend_from_slice(&packed);
        let mut chunks = Vec::new();
        push_png_chunk(&mut chunks, "iCCP", &data);

        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let mut with_icc = png[..33].to_vec();
        with_icc.extend_from_slice(&chunks);
        with_icc.extend_from_slice(&png[33..]);
        fs::write(&src, with_icc).unwrap();

        let meta = read(Format::Png, &src).expect("profile should be found");
        assert_eq!(meta.icc.as_ref().unwrap(), &profile);

        // ...and it lands intact in a freshly encoded JPEG.
        let jpg = dir.path().join("out.jpg");
        img.write_to(&mut std::io::Cursor::new(Vec::new()), image::ImageFormat::Jpeg).ok();
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg).unwrap();
        fs::write(&jpg, buf).unwrap();
        write(Format::Jpeg, &jpg, &meta).unwrap();
        assert_eq!(read(Format::Jpeg, &jpg).unwrap().icc.unwrap(), profile);

        // ...and back into a PNG.
        let back = dir.path().join("back.png");
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
        fs::write(&back, buf).unwrap();
        write(Format::Png, &back, &meta).unwrap();
        assert_eq!(read(Format::Png, &back).unwrap().icc.unwrap(), profile);
    }
}
