//! End-to-end tests against real files. FFmpeg tests skip themselves when no
//! ffmpeg binary (or a needed encoder) is available.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use uberconv_core::{Context, Format, Registry};
use uberconv_formats::{default_registry, ffmpeg::locate_ffmpeg};

fn convert(r: &Registry, input: &Path, to: Format) -> PathBuf {
    let from = Format::from_path(input).unwrap();
    let output = Format::with_format_extension(input, to);
    r.convert(input, &output, from, to, &Context::default())
        .unwrap_or_else(|e| panic!("{from} -> {to} failed: {e}"));
    assert!(fs::metadata(&output).unwrap().len() > 0, "{to} output is empty");
    output
}

#[test]
fn images_convert_between_all_formats() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("pic.png");
    let img = image::RgbaImage::from_fn(64, 48, |x, y| image::Rgba([(x * 4) as u8, (y * 5) as u8, 128, if x < 8 { 0 } else { 255 }]));
    img.save(&src).unwrap();

    for to in [Format::Jpeg, Format::Gif, Format::Bmp, Format::Tiff, Format::WebP, Format::Ico] {
        let out = convert(&r, &src, to);
        let back = image::open(&out).unwrap_or_else(|e| panic!("{to} output unreadable: {e}"));
        assert_eq!((back.width(), back.height()), (64, 48), "{to} dims");
    }
}

#[test]
fn sixteen_bit_images_convert_everywhere() {
    // Regression: 16-bit sources used to fail for WebP, GIF and BMP.
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("deep.png");
    image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(64, 48, |x, y| image::Rgb([(x * 1000) as u16, (y * 1200) as u16, 40000]))
        .save(&src)
        .unwrap();
    for to in [Format::Jpeg, Format::Gif, Format::Bmp, Format::Tiff, Format::WebP, Format::Ico] {
        let out = convert(&r, &src, to);
        assert!(image::open(&out).is_ok(), "{to} output unreadable");
    }
    // Regression: sources without alpha (JPEG) must also produce readable ICOs.
    let ico = convert(&r, &dir.path().join("deep.jpg"), Format::Ico);
    assert!(image::open(&ico).is_ok(), "JPEG -> ICO unreadable");
    let src16a = dir.path().join("deep_alpha.png");
    image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_fn(16, 16, |x, _| image::Rgba([x as u16 * 4000, 0, 0, 30000]))
        .save(&src16a)
        .unwrap();
    for to in [Format::Gif, Format::Bmp, Format::WebP, Format::Jpeg] {
        convert(&r, &src16a, to);
    }
}

#[test]
fn batch_converts_many_images_in_parallel() {
    use uberconv_core::{BatchEvent, BatchItem, CancelToken, Options};
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let mut items = Vec::new();
    for i in 0..24 {
        let input = dir.path().join(format!("p{i}.png"));
        image::RgbaImage::from_fn(200, 150, |x, y| image::Rgba([(x + i) as u8, y as u8, 99, 255])).save(&input).unwrap();
        let to = if i % 2 == 0 { Format::Jpeg } else { Format::WebP };
        items.push(BatchItem::new(input.clone(), Format::with_format_extension(&input, to), Format::Png, to));
    }
    let mut done = 0;
    r.convert_batch(&items, &Options::default(), &CancelToken::new(), |_, ev| match ev {
        BatchEvent::Done => done += 1,
        BatchEvent::Failed(e) => panic!("batch item failed: {e}"),
        BatchEvent::Started | BatchEvent::Progress(_) => {}
    });
    assert_eq!(done, 24);
    for it in &items {
        assert_eq!(image::open(&it.output).unwrap().width(), 200);
    }
}

#[test]
fn big_images_are_capped_for_ico_and_by_option() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.png");
    image::RgbaImage::new(600, 300).save(&src).unwrap();
    let ico = convert(&r, &src, Format::Ico);
    let im = image::open(&ico).unwrap();
    assert_eq!((im.width(), im.height()), (256, 128));

    let mut opts = uberconv_core::Options::default();
    opts.image_max_dimension = Some(100);
    let out = dir.path().join("small.jpg");
    r.convert(&src, &out, Format::Png, Format::Jpeg, &Context::new(opts)).unwrap();
    assert_eq!(image::open(&out).unwrap().width(), 100);
}

#[test]
fn data_formats_chain_through_json() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("people.yaml");
    fs::write(&src, "- name: Ann\n  age: 31\n- name: Bo\n  age: 27\n  tags: [a, b]\n").unwrap();

    let plan = r.plan(Format::Yaml, Format::Csv).unwrap();
    assert_eq!(plan.describe(), "YAML -> JSON -> CSV");
    let csv = fs::read_to_string(convert(&r, &src, Format::Csv)).unwrap();
    assert!(csv.starts_with("name,age,tags\nAnn,31,\nBo,27,"), "{csv}");

    // CSV -> TOML must fail cleanly: the root is an array, TOML needs a table.
    let csv_path = dir.path().join("people.csv");
    let err = r
        .convert(&csv_path, &dir.path().join("x.toml"), Format::Csv, Format::Toml, &Context::default())
        .unwrap_err();
    assert!(err.to_string().len() > 0);
    assert!(!dir.path().join("x.toml").exists());

    let obj = dir.path().join("cfg.json");
    fs::write(&obj, r#"{"title":"x","server":{"port":8080}}"#).unwrap();
    let toml = fs::read_to_string(convert(&r, &obj, Format::Toml)).unwrap();
    assert!(toml.contains("[server]") && toml.contains("port = 8080"));
}

#[test]
fn text_formats() {
    let r = default_registry();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("doc.md");
    fs::write(&src, "# Hello\n\nWorld **bold**\n").unwrap();
    let html = fs::read_to_string(convert(&r, &src, Format::Html)).unwrap();
    assert!(html.contains("<h1>Hello</h1>") && html.contains("<strong>bold</strong>"));
    let txt = fs::read_to_string(convert(&r, &src, Format::Text)).unwrap();
    assert!(txt.starts_with("Hello"));
}

fn ffmpeg_make(args: &[&str], out: &Path) -> bool {
    let Some(bin) = locate_ffmpeg() else { return false };
    Command::new(bin)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn audio_via_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    if !ffmpeg_make(&["-f", "lavfi", "-i", "sine=frequency=440:duration=2"], &wav) {
        eprintln!("skipping: no ffmpeg");
        return;
    }
    let r = default_registry();
    for to in [Format::Mp3, Format::Flac, Format::Ogg, Format::Opus, Format::M4a, Format::Aac] {
        convert(&r, &wav, to);
    }
    // multi-step: opus -> flac -> mp3 is a single direct edge here, but make
    // sure converting *from* a converted file works too.
    let flac = dir.path().join("tone.flac");
    convert(&r, &flac, Format::Mp3);
}

#[test]
fn video_via_ffmpeg_reports_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mp4 = dir.path().join("clip.mp4");
    let made = ffmpeg_make(
        &[
            "-f", "lavfi", "-i", "testsrc=duration=2:size=320x240:rate=15",
            "-f", "lavfi", "-i", "sine=frequency=330:duration=2",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest",
        ],
        &mp4,
    );
    if !made {
        eprintln!("skipping: ffmpeg missing or has no libx264");
        return;
    }
    let r = default_registry();

    let seen = Arc::new(Mutex::new(Vec::<f32>::new()));
    let s2 = seen.clone();
    let ctx = Context::default().with_progress(move |p| s2.lock().unwrap().push(p));
    let out = dir.path().join("clip.mkv");
    r.convert(&mp4, &out, Format::Mp4, Format::Mkv, &ctx).unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(*seen.last().unwrap(), 1.0);
    assert!(seen.windows(2).all(|w| w[0] <= w[1] + 1e-6), "progress went backwards: {seen:?}");

    for to in [Format::WebM, Format::Mov, Format::Avi, Format::Gif, Format::Mp3] {
        convert(&r, &mp4, to);
    }
    // GIF -> MP4 closes the loop
    convert(&r, &dir.path().join("clip.gif"), Format::Mp4);
}

#[test]
fn ffmpeg_cancellation_removes_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("long.wav");
    if !ffmpeg_make(&["-f", "lavfi", "-i", "sine=frequency=440:duration=600"], &wav) {
        return;
    }
    let r = default_registry();
    let ctx = Context::default();
    let cancel = ctx.cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        cancel.cancel();
    });
    let out = dir.path().join("long.flac");
    let res = r.convert(&wav, &out, Format::Wav, Format::Flac, &ctx);
    assert!(matches!(res, Err(uberconv_core::Error::Cancelled)) || res.is_ok());
    if res.is_err() {
        assert!(!out.exists());
    }
}
