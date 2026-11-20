//! Audio / video conversion through a bundled FFmpeg sidecar binary.
//!
//! We shell out instead of linking libav*: it keeps the build trivial, lets
//! you ship whichever FFmpeg build (and licence) you want, and a crashing
//! codec can't take the UI down with it.

use std::{
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    thread,
    time::Duration,
};

use uberconv_core::{Context, Converter, Cost, Edge, Error, Format, Job, Result};

const AUDIO: &[Format] = &[
    Format::Mp3,
    Format::Wav,
    Format::Flac,
    Format::Ogg,
    Format::Opus,
    Format::M4a,
    Format::Aac,
];
const VIDEO: &[Format] = &[Format::Mp4, Format::Mkv, Format::WebM, Format::Mov, Format::Avi];

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// Where we look for FFmpeg, in order:
/// 1. `$UBERCONV_FFMPEG`
/// 2. next to the executable (`./ffmpeg`, `./binaries/ffmpeg`)
/// 3. macOS bundle `../Resources/` and Linux `../lib/uberconv/`
/// 4. `$PATH`
pub fn locate_ffmpeg() -> Option<PathBuf> {
    if let Some(p) = env::var_os("UBERCONV_FFMPEG").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(dir) = env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let candidates = [
            dir.join(exe_name()),
            dir.join("binaries").join(exe_name()),
            dir.join("../Resources").join(exe_name()),
            dir.join("../Resources/binaries").join(exe_name()),
            dir.join("../lib/uberconv").join(exe_name()),
        ];
        if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
            return Some(found);
        }
    }
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|d| d.join(exe_name()))
            .find(|c| c.is_file())
    })
}

pub struct FfmpegConverter {
    binary: PathBuf,
}

impl FfmpegConverter {
    pub fn locate() -> Option<Self> {
        locate_ffmpeg().map(|binary| FfmpegConverter { binary })
    }

    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        FfmpegConverter { binary: binary.into() }
    }
}

fn lossy_audio(f: Format) -> bool {
    matches!(f, Format::Mp3 | Format::Ogg | Format::Opus | Format::M4a | Format::Aac)
}

impl Converter for FfmpegConverter {
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    fn heavy(&self) -> bool {
        true // FFmpeg is multi-threaded itself; don't run several at once
    }

    fn edges(&self) -> Vec<Edge> {
        let mut edges = Vec::new();
        let audio_cost = |to: Format, w: u32| if lossy_audio(to) { Cost::lossy(w) } else { Cost::lossless(w) };

        for &from in AUDIO {
            for &to in AUDIO {
                if from != to {
                    edges.push(Edge::new(from, to, audio_cost(to, 20)));
                }
            }
        }
        for &from in VIDEO {
            for &to in VIDEO {
                if from != to {
                    edges.push(Edge::new(from, to, Cost::lossy(60)));
                }
            }
            for &to in AUDIO {
                edges.push(Edge::new(from, to, audio_cost(to, 40))); // extract audio track
            }
            edges.push(Edge::new(from, Format::Gif, Cost::lossy(80)));
        }
        // AVIF still images (libaom / dav1d inside FFmpeg). Other raster formats
        // reach AVIF through PNG via the conversion graph.
        edges.push(Edge::new(Format::Avif, Format::Png, Cost::lossless(30)));
        edges.push(Edge::new(Format::Png, Format::Avif, Cost::lossy(60)));
        edges.push(Edge::new(Format::Gif, Format::Mp4, Cost::lossy(50)));
        edges.push(Edge::new(Format::Gif, Format::WebM, Cost::lossy(50)));
        edges
    }

    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()> {
        let args = output_args(job.to, ctx);
        run_ffmpeg(&self.binary, job.input, job.output, &args, ctx)
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

fn output_args(to: Format, ctx: &Context) -> Vec<String> {
    let br = format!("{}k", ctx.options.audio_bitrate_kbps);
    let crf = ctx.options.video_crf.min(51);
    let even = s("scale=trunc(iw/2)*2:trunc(ih/2)*2");
    let h264 = |movflags: bool| {
        let mut a = vec![
            s("-c:v"), s("libx264"), s("-preset"), s("medium"), s("-crf"), crf.to_string(),
            s("-pix_fmt"), s("yuv420p"), s("-vf"), even.clone(),
            s("-c:a"), s("aac"), s("-b:a"), br.clone(),
        ];
        if movflags {
            a.extend([s("-movflags"), s("+faststart")]);
        }
        a
    };
    let mut a: Vec<String> = match to {
        Format::Mp3 => vec![s("-vn"), s("-c:a"), s("libmp3lame"), s("-b:a"), br.clone(), s("-f"), s("mp3")],
        Format::Wav => vec![s("-vn"), s("-c:a"), s("pcm_s16le"), s("-f"), s("wav")],
        Format::Flac => vec![s("-vn"), s("-c:a"), s("flac"), s("-f"), s("flac")],
        Format::Ogg => vec![s("-vn"), s("-c:a"), s("libvorbis"), s("-b:a"), br.clone(), s("-f"), s("ogg")],
        Format::Opus => vec![s("-vn"), s("-c:a"), s("libopus"), s("-b:a"), br.clone(), s("-f"), s("opus")],
        Format::M4a => vec![s("-vn"), s("-c:a"), s("aac"), s("-b:a"), br.clone(), s("-f"), s("ipod")],
        Format::Aac => vec![s("-vn"), s("-c:a"), s("aac"), s("-b:a"), br.clone(), s("-f"), s("adts")],
        Format::Mp4 => {
            let mut v = h264(true);
            v.extend([s("-f"), s("mp4")]);
            v
        }
        Format::Mkv => {
            let mut v = h264(false);
            v.extend([s("-f"), s("matroska")]);
            v
        }
        Format::Mov => {
            let mut v = h264(false);
            v.extend([s("-f"), s("mov")]);
            v
        }
        Format::WebM => vec![
            s("-c:v"), s("libvpx-vp9"), s("-crf"), (crf as u32 + 9).min(63).to_string(), s("-b:v"), s("0"),
            s("-row-mt"), s("1"), s("-pix_fmt"), s("yuv420p"), s("-vf"), even.clone(),
            s("-c:a"), s("libopus"), s("-b:a"), br.clone(), s("-f"), s("webm"),
        ],
        Format::Avi => vec![
            s("-c:v"), s("mpeg4"), s("-q:v"), s("4"), s("-vf"), even.clone(),
            s("-c:a"), s("libmp3lame"), s("-b:a"), br.clone(), s("-f"), s("avi"),
        ],
        Format::Png => vec![s("-frames:v"), s("1"), s("-update"), s("1"), s("-f"), s("image2"), s("-c:v"), s("png")],
        Format::Avif => {
            // quality 100..1 -> CRF 10..~99 (clamped to libaom's 0..63)
            let q = ctx.options.image_quality.clamp(1, 100) as f32;
            let avif_crf = ((100.0 - q) * 0.9 + 10.0).round().min(63.0);
            vec![
                s("-frames:v"), s("1"), s("-c:v"), s("libaom-av1"), s("-crf"), format!("{avif_crf}"), s("-b:v"), s("0"),
                s("-cpu-used"), s("6"), s("-still-picture"), s("1"), s("-pix_fmt"), s("yuv420p"),
                s("-vf"), even.clone(), s("-f"), s("avif"),
            ]
        }
        Format::Gif => vec![
            s("-an"), s("-vf"),
            s("fps=12,scale=480:-2:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse"),
            s("-loop"), s("0"), s("-f"), s("gif"),
        ],
        _ => vec![],
    };
    a.shrink_to_fit();
    a
}

/// `Duration: 00:01:23.45` -> seconds
fn parse_duration(ffmpeg_stderr: &str) -> Option<f64> {
    let rest = ffmpeg_stderr.split("Duration:").nth(1)?.trim_start();
    let stamp = rest.split(',').next()?.trim();
    let mut parts = stamp.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    let total = h * 3600.0 + m * 60.0 + sec;
    (total > 0.0).then_some(total)
}

fn run_ffmpeg(bin: &Path, input: &Path, output: &Path, args: &[String], ctx: &Context) -> Result<()> {
    ctx.cancel.check()?;

    let mut cmd = Command::new(bin);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-loglevel", "info", "-i"])
        .arg(input)
        .args(args)
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| Error::failed(format!("could not start ffmpeg ({}): {e}", bin.display())))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Read both pipes on threads so neither can fill up and stall ffmpeg, and
    // so the main loop can keep polling for cancellation.
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // ffmpeg prints `Duration: ...` while opening the input, so we learn the
    // total length from stderr instead of paying for a separate probe process.
    let duration_bits = Arc::new(AtomicU64::new(0));
    let duration_writer = duration_bits.clone();
    let err_thread = thread::spawn(move || {
        let mut log = String::new();
        for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            if duration_writer.load(Ordering::Relaxed) == 0 {
                if let Some(d) = parse_duration(&line) {
                    duration_writer.store(d.to_bits(), Ordering::Relaxed);
                }
            }
            log.push_str(&line);
            log.push('\n');
        }
        log
    });

    loop {
        if ctx.cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(output);
            return Err(Error::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                // Despite the name, out_time_ms is microseconds too.
                let value = line
                    .strip_prefix("out_time_us=")
                    .or_else(|| line.strip_prefix("out_time_ms="));
                let total = f64::from_bits(duration_bits.load(Ordering::Relaxed));
                if let (Some(v), true) = (value, total > 0.0) {
                    if let Ok(us) = v.trim().parse::<i64>() {
                        if us >= 0 {
                            ctx.report(((us as f64 / 1e6 / total) as f32).min(0.99));
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child.wait()?;
    let stderr_text = err_thread.join().unwrap_or_default();
    if !status.success() {
        let tail: Vec<&str> = stderr_text.lines().rev().take(4).collect();
        let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(Error::failed(format!("ffmpeg exited with {status}:\n{tail}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duration() {
        let log = "Input #0, wav\n  Duration: 00:01:02.50, bitrate: 705 kb/s\n";
        assert_eq!(parse_duration(log), Some(62.5));
        assert_eq!(parse_duration("Duration: N/A, start"), None);
    }
}
