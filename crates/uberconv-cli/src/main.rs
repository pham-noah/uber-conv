use std::{
    collections::HashSet,
    env,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use uberconv_core::{BatchEvent, BatchItem, CancelToken, Family, Format, Options};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
uberconv - convert anything to anything (that it knows how)

Images (incl. SVG and HEIC input, AVIF), audio, video, data files, documents (Markdown,
HTML, Word, PDF) and archives (ZIP, TAR, TAR.GZ).

USAGE
  uberconv <INPUT> <OUTPUT>              format is taken from the file extensions
  uberconv <INPUT>... --to <FORMAT>      writes next to each input
  uberconv formats                       list supported formats
  uberconv targets <FORMAT|FILE>         what can this be converted to?

OPTIONS
  -t, --to <FORMAT>      target format (png, mp3, yaml, ...)
  -f, --force            overwrite existing output
  -q, --quality <1-100>  JPEG/WebP/AVIF quality (default 90; 100 = lossless WebP)
      --max-size <PX>    downscale images to fit PX x PX
      --bitrate <KBPS>   lossy audio bitrate (default 192)
      --crf <0-51>       video quality, lower is better (default 23)
      --page <N>         which PDF page to render (default 1)
  -h, --help
  -V, --version
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("uberconv {VERSION}");
        return Ok(());
    }
    let registry = uberconv_formats::default_registry();

    match args[0].as_str() {
        "formats" => {
            for fam in [Family::Image, Family::Audio, Family::Video, Family::Data, Family::Text, Family::Archive] {
                let names: Vec<String> = Format::ALL
                    .iter()
                    .filter(|f| f.family() == fam)
                    .map(|f| format!("{} (.{})", f.name(), f.ext()))
                    .collect();
                println!("{:<10} {}", fam.name(), names.join(", "));
            }
            if !uberconv_formats::ffmpeg_available() {
                println!("\nnote: no ffmpeg found - audio/video conversion is disabled.\nPut ffmpeg next to this executable, or set UBERCONV_FFMPEG.");
            }
            Ok(())
        }
        "targets" => {
            let what = args.get(1).ok_or("targets needs a format or file name")?;
            let fmt = what
                .parse::<Format>()
                .ok()
                .or_else(|| Format::from_path(Path::new(what)))
                .ok_or_else(|| format!("unknown format: {what}"))?;
            let t = registry.reachable_from(fmt);
            let mut out = std::io::stdout().lock();
            if t.is_empty() {
                let _ = writeln!(out, "{fmt} has no conversions available");
            }
            for to in t {
                let plan = registry.plan(fmt, to).expect("reachable implies plannable");
                let lossy = if plan.is_lossy() { "  (lossy)" } else { "" };
                if writeln!(out, "{:<12} {}{}", to.name(), plan.describe(), lossy).is_err() {
                    break; // e.g. piped into `head`
                }
            }
            Ok(())
        }
        _ => convert_cmd(&registry, args),
    }
}

fn convert_cmd(registry: &uberconv_core::Registry, args: Vec<String>) -> Result<(), String> {
    let mut positional: Vec<PathBuf> = Vec::new();
    let mut to: Option<Format> = None;
    let mut force = false;
    let mut opts = Options::default();

    fn num<T: std::str::FromStr>(name: &str, v: Option<String>) -> Result<T, String> {
        v.ok_or_else(|| format!("{name} needs a value"))?
            .parse::<T>()
            .map_err(|_| format!("invalid value for {name}"))
    }

    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-t" | "--to" => {
                let v = it.next().ok_or("--to needs a value")?;
                to = Some(v.parse::<Format>().map_err(|e| e.to_string())?);
            }
            "-f" | "--force" => force = true,
            "-q" | "--quality" => opts.image_quality = num("--quality", it.next())?,
            "--max-size" => opts.image_max_dimension = Some(num("--max-size", it.next())?),
            "--bitrate" => opts.audio_bitrate_kbps = num("--bitrate", it.next())?,
            "--crf" => opts.video_crf = num("--crf", it.next())?,
            "--page" => opts.pdf_page = num::<u32>("--page", it.next())?.max(1),
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
            _ => positional.push(PathBuf::from(a)),
        }
    }

    let jobs: Vec<(PathBuf, PathBuf, Option<Format>)> = match to {
        Some(t) => positional
            .into_iter()
            .map(|p| {
                let out = Format::with_format_extension(&p, t);
                (p, out, Some(t))
            })
            .collect(),
        None => {
            if positional.len() != 2 {
                return Err("expected <INPUT> <OUTPUT> (or use --to). See --help".into());
            }
            let mut p = positional.into_iter();
            vec![(p.next().unwrap(), p.next().unwrap(), None)]
        }
    };
    if jobs.is_empty() {
        return Err("no input files".into());
    }

    let mut failures = 0;
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut batch: Vec<BatchItem> = Vec::new();
    for (input, output, to) in jobs {
        match prepare(registry, &input, &output, to, force, &mut claimed) {
            Ok((item, plan)) => {
                eprintln!(
                    "{} -> {}   [{}{}]",
                    item.input.display(),
                    item.output.display(),
                    plan.describe(),
                    if plan.is_lossy() { ", lossy" } else { "" }
                );
                batch.push(item);
            }
            Err(e) => {
                eprintln!("error: {}: {e}", input.display());
                failures += 1;
            }
        }
    }

    let single = batch.len() == 1;
    let started = Instant::now();
    let mut converted = 0;
    registry.convert_batch(&batch, &opts, &CancelToken::new(), |i, ev| match ev {
        BatchEvent::Progress(p) if single => eprint!("\r  {:>3}%", (p * 100.0) as u32),
        BatchEvent::Started | BatchEvent::Progress(_) => {}
        BatchEvent::Done => {
            converted += 1;
            if single {
                eprintln!();
            }
            eprintln!("  ok    {}", batch[i].output.display());
        }
        BatchEvent::Failed(e) => {
            failures += 1;
            if single {
                eprintln!();
            }
            eprintln!("  FAIL  {}: {e}", batch[i].input.display());
        }
    });
    if !batch.is_empty() {
        eprintln!("{converted} converted in {:.1}s", started.elapsed().as_secs_f32());
    }
    if failures > 0 {
        Err(format!("{failures} file(s) failed"))
    } else {
        Ok(())
    }
}

fn prepare(
    registry: &uberconv_core::Registry,
    input: &Path,
    output: &Path,
    to: Option<Format>,
    force: bool,
    claimed: &mut HashSet<PathBuf>,
) -> Result<(BatchItem, uberconv_core::Plan), String> {
    let from = Format::from_path(input).ok_or("can't tell the input format from its extension")?;
    let to = match to {
        Some(t) => t,
        None => Format::from_path(output).ok_or("can't tell the output format from its extension")?,
    };
    if from == to {
        return Err(format!("already {from}"));
    }
    if output.exists() && !force {
        return Err(format!("{} exists (use --force to overwrite)", output.display()));
    }
    if !claimed.insert(output.to_path_buf()) {
        return Err(format!("another input also maps to {}", output.display()));
    }
    let plan = registry
        .plan(from, to)
        .ok_or_else(|| format!("no conversion path from {from} to {to}"))?;
    Ok((
        BatchItem::new(input.to_path_buf(), output.to_path_buf(), from, to),
        plan,
    ))
}
