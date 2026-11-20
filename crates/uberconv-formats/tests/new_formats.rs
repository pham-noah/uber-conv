//! Integration tests for EXIF orientation, SVG, WebP, PDF, DOCX, archives,
//! streaming JSON/YAML and AVIF. Binary fixtures live in tests/fixtures and are
//! produced by independent tools (see make_fixtures.py), so these tests check
//! uberconv against other implementations, not against itself.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use uberconv_core::{Context, Format, Options, Registry};
use uberconv_formats::{default_registry, ffmpeg::locate_ffmpeg};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Copy a fixture next to a temp output so conversions never touch the repo.
fn staged(dir: &Path, name: &str) -> PathBuf {
    let dst = dir.join(name);
    fs::copy(fixture(name), &dst).unwrap();
    dst
}

fn run(r: &Registry, input: &Path, to: Format, opts: Options) -> Result<PathBuf, String> {
    let from = Format::from_path(input).unwrap_or_else(|| panic!("unknown input type: {}", input.display()));
    let output = Format::with_format_extension(input, to);
    r.convert(input, &output, from, to, &Context::new(opts))
        .map(|_| output)
        .map_err(|e| e.to_string())
}

fn convert(r: &Registry, input: &Path, to: Format) -> PathBuf {
    run(r, input, to, Options::default()).unwrap_or_else(|e| panic!("{} -> {to} failed: {e}", input.display()))
}

fn rgba(path: &Path) -> (u32, u32, Vec<u8>) {
    let img = image::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).to_rgba8();
    (img.width(), img.height(), img.into_raw())
}

// ───────────────────────── EXIF orientation ─────────────────────────

#[test]
fn exif_orientation_matches_pil_for_all_eight_cases() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    for o in 1..=8 {
        let input = staged(dir.path(), &format!("orient-{o}.png"));
        let out = convert(&r, &input, Format::Bmp); // BMP is lossless
        let (w, h, got) = rgba(&out);
        let (ew, eh, want) = rgba(&fixture(&format!("orient-{o}-expected.png")));
        assert_eq!((w, h), (ew, eh), "orientation {o}: dimensions");
        assert_eq!(got, want, "orientation {o}: pixels differ from PIL's exif_transpose");
    }
}

// ───────────────────────── SVG ─────────────────────────

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
  <circle cx="10" cy="10" r="8" fill="#ff0000"/>
</svg>"##;

#[test]
fn svg_renders_with_transparency_and_respects_the_size_option() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let svg = dir.path().join("dot.svg");
    fs::write(&svg, SVG).unwrap();

    // default: vector art is rendered large (longest side >= 1024)
    let png = convert(&r, &svg, Format::Png);
    let (w, h, px) = rgba(&png);
    assert_eq!((w, h), (1024, 512));
    let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..][..4];
    assert_eq!(at(256, 256)[..3], [255, 0, 0]); // circle centre is red
    assert_eq!(at(256, 256)[3], 255);
    assert_eq!(at(1000, 10)[3], 0); // outside the circle: transparent

    // capped size
    let small = run(&r, &svg, Format::Png, Options { image_max_dimension: Some(100), ..Options::default() }).unwrap();
    assert_eq!(rgba(&small).0, 100);

    // other targets work too
    convert(&r, &svg, Format::WebP);
    convert(&r, &svg, Format::Jpeg);
    assert!(run(&r, &dir.path().join("missing.svg"), Format::Png, Options::default()).is_err());
}

// ───────────────────────── WebP ─────────────────────────

#[test]
fn webp_is_lossy_by_default_and_lossless_at_quality_100() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("grad.png");
    image::RgbImage::from_fn(256, 256, |x, y| image::Rgb([x as u8, y as u8, ((x * y) % 251) as u8])).save(&src).unwrap();

    let lossy = run(&r, &src, Format::WebP, Options { image_quality: 60, ..Options::default() }).unwrap();
    let lossy_size = fs::metadata(&lossy).unwrap().len();
    let lossless = {
        let out = dir.path().join("lossless.webp");
        r.convert(&src, &out, Format::Png, Format::WebP, &Context::new(Options { image_quality: 100, ..Options::default() })).unwrap();
        out
    };
    assert!(lossy_size < fs::metadata(&lossless).unwrap().len(), "lossy should be smaller");

    // lossless round trip is bit exact; lossy is close but not equal
    assert_eq!(rgba(&lossless).2, rgba(&src).2);
    let (w, h, lp) = rgba(&lossy);
    assert_eq!((w, h), (256, 256));
    assert_ne!(lp, rgba(&src).2);
    // and our own output decodes back through every other format
    convert(&r, &lossy, Format::Png);
}

// ───────────────────────── PDF ─────────────────────────

fn dominant(path: &Path) -> [u8; 3] {
    let (w, h, px) = rgba(path);
    let i = (((h / 2) * w + w / 2) * 4) as usize;
    [px[i], px[i + 1], px[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 12)
}

#[test]
fn image_to_pdf_and_back() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("solid.png");
    image::RgbImage::from_pixel(300, 200, image::Rgb([200, 40, 40])).save(&png).unwrap();

    let pdf = convert(&r, &png, Format::Pdf);
    let bytes = fs::read(&pdf).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.4") && bytes.ends_with(b"%%EOF\n"));

    // back to PNG: same pixel size (150 dpi both ways), same colour
    let back = dir.path().join("solid-back.png");
    r.convert(&pdf, &back, Format::Pdf, Format::Png, &Context::default()).unwrap();
    let (w, h, _) = rgba(&back);
    assert!(w.abs_diff(300) <= 1 && h.abs_diff(200) <= 1, "{w}x{h}");
    assert!(near(dominant(&back), [200, 40, 40]), "{:?}", dominant(&back));
}

#[test]
fn jpegs_are_embedded_untouched_and_alpha_becomes_a_soft_mask() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("photo.png");
    image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 90])).save(&png).unwrap();
    let jpg = convert(&r, &png, Format::Jpeg);
    let jpeg_bytes = fs::read(&jpg).unwrap();
    let pdf_bytes = fs::read(convert(&r, &jpg, Format::Pdf)).unwrap();
    assert!(String::from_utf8_lossy(&pdf_bytes).contains("/DCTDecode"));
    assert!(pdf_bytes.windows(jpeg_bytes.len()).any(|w| w == jpeg_bytes.as_slice()), "JPEG bytes must be copied verbatim");

    let ghost = dir.path().join("ghost.png");
    image::RgbaImage::from_fn(32, 32, |x, _| image::Rgba([10, 200, 10, if x < 16 { 0 } else { 255 }])).save(&ghost).unwrap();
    let pdf = convert(&r, &ghost, Format::Pdf);
    assert!(String::from_utf8_lossy(&fs::read(&pdf).unwrap()).contains("/SMask"));
}

#[test]
fn pdf_pages_can_be_chosen() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let pdf = staged(dir.path(), "three-pages.pdf");
    let expected = [[220, 30, 30], [30, 200, 60], [30, 60, 220]];
    for (i, want) in expected.iter().enumerate() {
        let out = dir.path().join(format!("page{}.png", i + 1));
        let opts = Options { pdf_page: i as u32 + 1, ..Options::default() };
        r.convert(&pdf, &out, Format::Pdf, Format::Png, &Context::new(opts)).unwrap();
        assert!(near(dominant(&out), *want), "page {}: {:?}", i + 1, dominant(&out));
    }
    // page 300 DPI-independent size: 300x200 pt at 150 dpi
    let (w, h, _) = rgba(&dir.path().join("page1.png"));
    assert!(w.abs_diff(625) <= 2 && h.abs_diff(417) <= 2, "{w}x{h}");

    let err = run(&r, &pdf, Format::Png, Options { pdf_page: 4, ..Options::default() }).unwrap_err();
    assert!(err.contains("3 pages"), "{err}");
    // the largest-size option scales the page
    let capped = run(&r, &pdf, Format::Jpeg, Options { image_max_dimension: Some(200), ..Options::default() }).unwrap();
    assert_eq!(rgba(&capped).0, 200);
}

#[test]
fn broken_pdfs_fail_cleanly() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("junk.pdf");
    fs::write(&junk, b"this is not a pdf at all").unwrap();
    assert!(run(&r, &junk, Format::Png, Options::default()).is_err());
    assert!(!dir.path().join("junk.png").exists());
}

// ───────────────────────── DOCX ─────────────────────────

#[test]
fn reads_a_word_document_made_by_python_docx() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let md = fs::read_to_string(convert(&r, &staged(dir.path(), "sample.docx"), Format::Markdown)).unwrap();
    for needle in [
        "# Quarterly report",
        "## Details",
        "**bold**",
        "*italic*",
        r"R&D \<draft\> \*star\*", // special characters are escaped, not eaten
        "- first bullet",
        "- second bullet",
        "1. step one",
        "1. step two",
        "| Name | Score |",
        "| Ann | 9 |",
    ] {
        assert!(md.contains(needle), "missing {needle:?} in:\n{md}");
    }
}

const MARKDOWN: &str = "# Title\n\nSome **bold**, *italic*, ~~gone~~ and `code` with a [link](https://example.com/a?b=1&c=2).\n\n- one\n  - nested\n- two\n\n1. first\n2. second\n\n> quoted words\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```\nfn main() {}\nlet x = 1;\n```\n";

#[test]
fn markdown_survives_a_trip_through_docx() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("note.md");
    fs::write(&src, MARKDOWN).unwrap();
    let docx = convert(&r, &src, Format::Docx);

    // the package is a real zip with the expected parts
    let mut zip = zip::ZipArchive::new(File::open(&docx).unwrap()).unwrap();
    for part in ["[Content_Types].xml", "word/document.xml", "word/styles.xml", "word/numbering.xml", "word/_rels/document.xml.rels"] {
        assert!(zip.by_name(part).is_ok(), "missing {part}");
    }

    let back_path = dir.path().join("back.md");
    r.convert(&docx, &back_path, Format::Docx, Format::Markdown, &Context::default()).unwrap();
    let back = fs::read_to_string(back_path).unwrap();
    for needle in [
        "# Title",
        "**bold**",
        "*italic*",
        "~~gone~~",
        "`code`",
        "[link](https://example.com/a?b=1&c=2)",
        "- one",
        "  - nested",
        "- two",
        "1. first",
        "> quoted words",
        "| A | B |",
        "| 1 | 2 |",
        "```\nfn main() {}\nlet x = 1;\n```",
    ] {
        assert!(back.contains(needle), "missing {needle:?} in:\n{back}");
    }

    // and the graph reaches HTML and plain text from a Word file
    let html = fs::read_to_string(convert(&r, &docx, Format::Html)).unwrap();
    assert!(html.contains("<h1>Title</h1>") && html.contains("<strong>bold</strong>"));
    assert!(fs::read_to_string(convert(&r, &docx, Format::Text)).unwrap().contains("Title"));
    // ...and Word from plain text, via Markdown
    let txt = dir.path().join("plain.txt");
    fs::write(&txt, "hello world").unwrap();
    assert!(zip::ZipArchive::new(File::open(convert(&r, &txt, Format::Docx)).unwrap()).is_ok());
}

#[test]
fn bad_word_files_are_rejected() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.docx");
    fs::write(&bad, b"definitely not a zip").unwrap();
    assert!(run(&r, &bad, Format::Markdown, Options::default()).is_err());
    // a zip that is not a Word document
    let not_word = staged(dir.path(), "sample.zip");
    let renamed = dir.path().join("renamed.docx");
    fs::rename(not_word, &renamed).unwrap();
    let err = run(&r, &renamed, Format::Markdown, Options::default()).unwrap_err();
    assert!(err.contains("word/document.xml"), "{err}");
}

// ───────────────────────── archives ─────────────────────────

/// name -> (kind, mode, content-or-target)
type Listing = BTreeMap<String, (&'static str, u32, Vec<u8>)>;

fn list_zip(path: &Path) -> Listing {
    let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut out = Listing::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().trim_end_matches('/').to_string();
        let mode = f.unix_mode().unwrap_or(0) & 0o7777;
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        let kind = if f.is_dir() { "dir" } else if f.is_symlink() { "link" } else { "file" };
        out.insert(name, (kind, mode, data));
    }
    out
}

fn list_tar(path: &Path, gz: bool) -> Listing {
    let file = File::open(path).unwrap();
    let reader: Box<dyn Read> = if gz { Box::new(flate2::read::GzDecoder::new(file)) } else { Box::new(file) };
    let mut out = Listing::new();
    for entry in tar::Archive::new(reader).entries().unwrap() {
        let mut e = entry.unwrap();
        let name = e.path().unwrap().to_string_lossy().trim_end_matches('/').to_string();
        let mode = e.header().mode().unwrap() & 0o7777;
        let (kind, data) = match e.header().entry_type() {
            tar::EntryType::Directory => ("dir", Vec::new()),
            tar::EntryType::Symlink => ("link", e.link_name().unwrap().unwrap().to_string_lossy().as_bytes().to_vec()),
            _ => {
                let mut d = Vec::new();
                e.read_to_end(&mut d).unwrap();
                ("file", d)
            }
        };
        out.insert(name, (kind, mode, data));
    }
    out
}

fn expect_sample(l: &Listing) {
    assert_eq!(l["docs"].0, "dir");
    assert_eq!(l["docs/readme.txt"], ("file", 0o644, b"hello archive\n".to_vec()));
    assert_eq!(l["bin/run.sh"].1, 0o755, "executable bit must survive");
    assert_eq!(l["ünï/çödé.txt"].2, b"unicode name");
    assert_eq!(l["big.bin"].2.len(), 512_000);
    assert_eq!(l["docs/link"].0, "link", "symlinks stay symlinks");
    assert_eq!(l["docs/link"].2, b"readme.txt");
}

#[test]
fn archives_convert_between_all_three_formats() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();

    // Python-made zip -> tar.gz -> tar -> zip
    let zip_in = staged(dir.path(), "sample.zip");
    expect_sample(&list_zip(&zip_in));
    let tgz = convert(&r, &zip_in, Format::TarGz);
    assert!(tgz.to_string_lossy().ends_with("sample.tar.gz"), "compound extension: {}", tgz.display());
    expect_sample(&list_tar(&tgz, true));
    let tar = convert(&r, &tgz, Format::Tar);
    assert!(tar.to_string_lossy().ends_with("sample.tar"));
    expect_sample(&list_tar(&tar, false));
    let zip_again = dir.path().join("again.zip");
    r.convert(&tar, &zip_again, Format::Tar, Format::Zip, &Context::default()).unwrap();
    expect_sample(&list_zip(&zip_again));

    // Python-made tar.gz -> zip
    let tgz_in = staged(dir.path(), "sample.tar.gz");
    let out = dir.path().join("from-tgz.zip");
    r.convert(&tgz_in, &out, Format::TarGz, Format::Zip, &Context::default()).unwrap();
    expect_sample(&list_zip(&out));
}

#[test]
fn unsafe_archives_are_refused() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let evil = staged(dir.path(), "evil.zip");
    let err = run(&r, &evil, Format::TarGz, Options::default()).unwrap_err();
    assert!(err.contains("unsafe path"), "{err}");
    assert!(!dir.path().join("evil.tar.gz").exists(), "no half-written output");
}

// ───────────────────────── streaming JSON <-> YAML ─────────────────────────

#[test]
fn streamed_json_yaml_matches_the_in_memory_path() {
    use serde_json::Value;
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("data.json");
    let doc = r#"{"name":"Zoë","n":[1,2.5,-3,1e3],"nested":{"ok":true,"none":null,"text":"line1\nline2: colon # hash"},"empty":{},"list":[],"big":12345678901234}"#;
    fs::write(&json, doc).unwrap();

    let yaml = convert(&r, &json, Format::Yaml);
    let from_yaml: Value = serde_yaml::from_str(&fs::read_to_string(&yaml).unwrap()).unwrap();
    assert_eq!(from_yaml, serde_json::from_str::<Value>(doc).unwrap());

    let back = dir.path().join("back.json");
    r.convert(&yaml, &back, Format::Yaml, Format::Json, &Context::default()).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&fs::read_to_string(&back).unwrap()).unwrap(), from_yaml);
    assert!(fs::read_to_string(back).unwrap().ends_with("}\n"));

    // a BOM and invalid input are handled
    let bom = dir.path().join("bom.json");
    fs::write(&bom, format!("\u{feff}{doc}")).unwrap();
    convert(&r, &bom, Format::Yaml);
    let bad = dir.path().join("bad.json");
    fs::write(&bad, "{\"a\": ").unwrap();
    assert!(run(&r, &bad, Format::Yaml, Options::default()).is_err());
    assert!(!dir.path().join("bad.yaml").exists());
}

// ───────────────────────── HEIC (libheif / Windows WIC) ─────────────────────────

/// `example.heic` is libheif's own sample: an HEVC-coded 1280x854 photo. Decoding
/// it exercises whichever backend this build has (bundled libheif, or the
/// Windows HEIF codec via WIC), so the check is against another implementation.
#[test]
fn heic_decodes_and_is_input_only() {
    if !uberconv_formats::heic::available() {
        eprintln!("skipping: {}", uberconv_formats::heic::unavailable_reason());
        return;
    }
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let input = staged(dir.path(), "example.heic");

    let png = convert(&r, &input, Format::Png);
    let (w, h, px) = rgba(&png);
    assert_eq!((w, h), (1280, 854), "HEIC decoded at the wrong size");
    // A photo, not a black frame: the sample's average is well above zero.
    let mean = px.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum::<u64>()
        / (w as u64 * h as u64 * 3);
    assert!(mean > 8, "decoded pixels look empty (mean {mean})");

    // JPEG is the everyday target; the result must be a readable image of the same shape.
    let jpg = convert(&r, &input, Format::Jpeg);
    assert_eq!(rgba(&jpg).0, 1280);

    // HEIC is input-only: nothing may be offered as converging *to* it, and the
    // picker offers the raster formats from it.
    for f in Format::ALL.iter().filter(|f| **f != Format::Heic) {
        assert!(r.plan(*f, Format::Heic).is_none(), "{f} must not be convertible to HEIC");
    }
    let reach = r.reachable_from(Format::Heic);
    for f in [Format::Png, Format::Jpeg, Format::Gif, Format::Bmp, Format::Tiff, Format::WebP, Format::Ico] {
        assert!(reach.contains(&f), "HEIC -> {f} should be offered");
    }
    assert_eq!(r.plan(Format::Heic, Format::Jpeg).unwrap().describe(), "HEIC -> JPEG");
}


#[test]
fn avif_roundtrip_when_ffmpeg_has_av1() {
    if locate_ffmpeg().is_none() {
        eprintln!("skipping: no ffmpeg");
        return;
    }
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("pic.png");
    image::RgbImage::from_fn(96, 64, |x, y| image::Rgb([(x * 2) as u8, (y * 3) as u8, 120])).save(&png).unwrap();
    match run(&r, &png, Format::Avif, Options::default()) {
        Err(e) if e.contains("ncoder") || e.contains("libaom") => {
            eprintln!("skipping: ffmpeg without libaom ({e})");
        }
        Err(e) => panic!("PNG -> AVIF failed: {e}"),
        Ok(avif) => {
            assert!(fs::metadata(&avif).unwrap().len() > 0);
            let back = dir.path().join("pic-back.png");
            r.convert(&avif, &back, Format::Avif, Format::Png, &Context::default()).unwrap();
            let (w, h, _) = rgba(&back);
            assert_eq!((w, h), (96, 64));
            // other raster formats reach AVIF through PNG
            let jpg = convert(&r, &png, Format::Jpeg);
            assert_eq!(r.plan(Format::Jpeg, Format::Avif).unwrap().describe(), "JPEG -> PNG -> AVIF");
            let _ = jpg;
        }
    }
}
