use std::{
    collections::HashSet,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use uberconv_core::{BatchEvent, BatchItem, CancelToken, Context, Family, Format, Options};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
uberconv - convert anything to anything (that it knows how)

Images (incl. SVG and HEIC input, AVIF), audio, video, data files, documents (Markdown,
HTML, Word, PDF) and archives (ZIP, TAR, TAR.GZ).

USAGE
  uberconv <INPUT> <OUTPUT>              format is taken from the file extensions
  uberconv <INPUT>... --to <FORMAT>      writes next to each input
  uberconv <INPUT>... --to <FORMAT> -o <DIR>
  uberconv <DIR> --to mp3 -r             every file under DIR, recursively
  uberconv <A.png> <B.png> --combine out.pdf
  uberconv formats                       list supported formats
  uberconv targets <FORMAT|FILE>         what can this be converted to?

OPTIONS
  -t, --to <FORMAT>      target format (png, mp3, yaml, ...)
  -o, --out <DIR>        write outputs into DIR (mirrors the input tree)
  -r, --recursive        walk given folders all the way down
      --combine <PDF>    put every input image on its own page of one PDF
  -f, --force            overwrite an existing output
  -q, --quality <1-100>  JPEG/WebP/AVIF quality (default 90; 100 = lossless WebP)
      --max-size <PX>    downscale images to fit PX x PX
      --bitrate <KBPS>   lossy audio bitrate (default 192)
      --crf <0-51>       video quality, lower is better (default 23)
      --page <N>         which PDF page to render (default 1)
      --all-pages        render every PDF page (page.png, page-002.png, ...)
      --keep-metadata    carry the colour profile and EXIF into images (default)
      --drop-metadata    do not carry the colour profile or EXIF
  -n, --dry-run          print what would happen, convert nothing
      --json             machine-readable result on stdout
      --quiet            only print failures
  -h, --help
  -V, --version
";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("uberconv {VERSION}");
        return Ok(ExitCode::SUCCESS);
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
            Ok(ExitCode::SUCCESS)
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
            Ok(ExitCode::SUCCESS)
        }
        _ => convert_cmd(&registry, args),
    }
}

/// Everything the command line can say, in one place.
struct Flags {
    positional: Vec<PathBuf>,
    to: Option<Format>,
    out: Option<PathBuf>,
    combine: Option<PathBuf>,
    force: bool,
    recursive: bool,
    json: bool,
    quiet: bool,
    dry_run: bool,
    opts: Options,
}

impl Flags {
    fn parse(args: Vec<String>) -> Result<Flags, String> {
        let mut f = Flags {
            positional: Vec::new(),
            to: None,
            out: None,
            combine: None,
            force: false,
            recursive: false,
            json: false,
            quiet: false,
            dry_run: false,
            opts: Options::default(),
        };
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
                    f.to = Some(v.parse::<Format>().map_err(|e| e.to_string())?);
                }
                "-o" | "--out" => f.out = Some(PathBuf::from(it.next().ok_or("--out needs a folder")?)),
                "-r" | "--recursive" => f.recursive = true,
                "--combine" => f.combine = Some(PathBuf::from(it.next().ok_or("--combine needs an output PDF")?)),
                "-f" | "--force" => f.force = true,
                "-q" | "--quality" => f.opts.image_quality = num("--quality", it.next())?,
                "--max-size" => f.opts.image_max_dimension = Some(num("--max-size", it.next())?),
                "--bitrate" => f.opts.audio_bitrate_kbps = num("--bitrate", it.next())?,
                "--crf" => f.opts.video_crf = num("--crf", it.next())?,
                "--page" => f.opts.pdf_page = num::<u32>("--page", it.next())?.max(1),
                "--all-pages" => f.opts.pdf_all_pages = true,
                "--keep-metadata" => f.opts.keep_metadata = true,
                "--drop-metadata" => f.opts.keep_metadata = false,
                "-n" | "--dry-run" => f.dry_run = true,
                "--json" => f.json = true,
                "--quiet" => f.quiet = true,
                s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
                _ => f.positional.push(PathBuf::from(a)),
            }
        }
        Ok(f)
    }
}

/// One line of the report, human or JSON.
struct Outcome {
    input: PathBuf,
    output: Option<PathBuf>,
    status: &'static str,
    plan: Option<String>,
    bytes: Option<u64>,
    error: Option<String>,
}

/// Expand the inputs: files stay, folders are walked (all the way down with -r).
fn expand(inputs: Vec<PathBuf>, recursive: bool, out: &mut Vec<(PathBuf, Option<PathBuf>)>, skipped: &mut usize) {
    for p in inputs {
        if p.is_dir() {
            walk(&p, &p, recursive, out, skipped);
        } else if p.is_file() {
            out.push((p, None));
        } else {
            *skipped += 1;
        }
    }
}

fn walk(root: &Path, dir: &Path, recursive: bool, out: &mut Vec<(PathBuf, Option<PathBuf>)>, skipped: &mut usize) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            if recursive {
                walk(root, &path, recursive, out, skipped);
            }
        } else if Format::from_path(&path).is_some() {
            out.push((path, Some(root.to_path_buf())));
        } else {
            *skipped += 1;
        }
    }
}

/// Where `input` goes, mirroring its folder under `--out`.
fn output_for(input: &Path, to: Format, out: &Option<PathBuf>, root: Option<&Path>) -> PathBuf {
    let name = format!("{}.{}", Format::stem_of(input), to.ext());
    match out {
        Some(base) => match root.and_then(|r| input.parent().and_then(|p| p.strip_prefix(r).ok())) {
            Some(rel) if !rel.as_os_str().is_empty() => base.join(rel).join(name),
            _ => base.join(name),
        },
        None => Format::with_format_extension(input, to),
    }
}

fn convert_cmd(registry: &uberconv_core::Registry, args: Vec<String>) -> Result<ExitCode, String> {
    let mut f = Flags::parse(args)?;

    if let Some(pdf) = f.combine.clone() {
        return combine_images(&pdf, f);
    }

    // Without --to, the last positional is the output file.
    let mut explicit_output: Option<PathBuf> = None;
    if f.to.is_none() {
        if f.positional.len() < 2 {
            return Err("expected <INPUT> <OUTPUT> (or use --to). See --help".into());
        }
        explicit_output = f.positional.pop();
    }
    if f.positional.is_empty() {
        return Err("no input files".into());
    }

    let to = f.to;
    let mut found = Vec::new();
    let mut skipped = 0usize;
    expand(std::mem::take(&mut f.positional), f.recursive, &mut found, &mut skipped);
    if found.is_empty() {
        return Err("no convertible files in the given input".into());
    }

    let started = Instant::now();
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut batch: Vec<BatchItem> = Vec::new();
    let mut plans: Vec<String> = Vec::new();

    for (input, root) in found {
        let output = match (&explicit_output, to) {
            (Some(o), _) => o.clone(),
            (None, Some(t)) => {
                let candidate = output_for(&input, t, &f.out, root.as_deref());
                // Never hand out a name twice, and never clobber without --force.
                if !f.force && (candidate.exists() || claimed.contains(&candidate)) {
                    let stem = Format::stem_of(&input);
                    let dir = candidate.parent().map(Path::to_path_buf).unwrap_or_default();
                    let mut n = 1;
                    loop {
                        let name = if n == 1 {
                            format!("{stem}-converted.{}", t.ext())
                        } else {
                            format!("{stem}-converted{n}.{}", t.ext())
                        };
                        let c = dir.join(name);
                        if !c.exists() && !claimed.contains(&c) {
                            break c;
                        }
                        n += 1;
                    }
                } else {
                    candidate
                }
            }
            (None, None) => unreachable!("--to is required when there is no explicit output"),
        };

        match prepare(registry, &input, &output, to, f.force, &mut claimed) {
            Ok((item, plan)) => {
                if !f.quiet && !f.json {
                    eprintln!(
                        "{} -> {}   [{}{}]",
                        item.input.display(),
                        item.output.display(),
                        plan.describe(),
                        if plan.is_lossy() { ", lossy" } else { "" }
                    );
                }
                plans.push(plan.describe());
                batch.push(item);
            }
            Err(e) => outcomes.push(Outcome {
                input,
                output: Some(output),
                status: "failed",
                plan: None,
                bytes: None,
                error: Some(e),
            }),
        }
    }
    if skipped > 0 && !f.quiet && !f.json {
        eprintln!("skipped {skipped} file(s) with no known converter");
    }

    let mut converted = 0usize;
    let mut failed = outcomes.len();
    let had_jobs = !batch.is_empty();

    if f.dry_run {
        for (i, item) in batch.iter().enumerate() {
            outcomes.push(Outcome {
                input: item.input.clone(),
                output: Some(item.output.clone()),
                status: "planned",
                plan: plans.get(i).cloned(),
                bytes: None,
                error: None,
            });
        }
    } else if had_jobs {
        let single = batch.len() == 1;
        registry.convert_batch(&batch, &f.opts, &CancelToken::new(), |i, ev| match ev {
            BatchEvent::Progress(p) if single && !f.quiet && !f.json => eprint!("\r  {:>3}%", (p * 100.0) as u32),
            BatchEvent::Started | BatchEvent::Progress(_) => {}
            BatchEvent::Done => {
                converted += 1;
                if single && !f.quiet && !f.json {
                    eprintln!();
                }
                if !f.quiet && !f.json {
                    eprintln!("  ok    {}", batch[i].output.display());
                }
                let bytes = fs::metadata(&batch[i].output).map(|m| m.len()).ok();
                outcomes.push(Outcome {
                    input: batch[i].input.clone(),
                    output: Some(batch[i].output.clone()),
                    status: "ok",
                    plan: plans.get(i).cloned(),
                    bytes,
                    error: None,
                });
            }
            BatchEvent::Failed(e) => {
                failed += 1;
                if single && !f.quiet && !f.json {
                    eprintln!();
                }
                if !f.json {
                    eprintln!("  FAIL  {}: {e}", batch[i].input.display());
                }
                outcomes.push(Outcome {
                    input: batch[i].input.clone(),
                    output: Some(batch[i].output.clone()),
                    status: "failed",
                    plan: plans.get(i).cloned(),
                    bytes: None,
                    error: Some(e.to_string()),
                });
            }
        });
    }

    let seconds = started.elapsed().as_secs_f32();
    if f.json {
        print_json(&outcomes, converted, failed, skipped, seconds, f.dry_run);
    } else if had_jobs && !f.quiet {
        let verb = if f.dry_run { "planned" } else { "converted" };
        eprintln!("{converted} {verb} in {seconds:.1}s");
    }

    if failed > 0 {
        // Exit code 1 on any failure, so shell loops can stop.
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// `<A.png> <B.png> ... --combine book.pdf`: one page per image.
fn combine_images(pdf: &Path, f: Flags) -> Result<ExitCode, String> {
    if f.positional.is_empty() {
        return Err("--combine needs at least one image".into());
    }
    let mut found = Vec::new();
    let mut skipped = 0usize;
    expand(f.positional.clone(), f.recursive, &mut found, &mut skipped);
    let inputs: Vec<PathBuf> = found.into_iter().map(|(p, _)| p).collect();
    if inputs.is_empty() {
        return Err("no convertible files in the given input".into());
    }
    let pages = inputs.len();
    if f.dry_run {
        if f.json {
            let outcomes: Vec<Outcome> = inputs
                .into_iter()
                .map(|input| Outcome {
                    input,
                    output: Some(pdf.to_path_buf()),
                    status: "planned",
                    plan: Some("-> PDF page".to_string()),
                    bytes: None,
                    error: None,
                })
                .collect();
            print_json(&outcomes, 0, 0, skipped, 0.0, true);
        } else if !f.quiet {
            for input in &inputs {
                eprintln!("{} -> {}", input.display(), pdf.display());
            }
            eprintln!("planned {pages} page(s), nothing written");
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(dir) = pdf.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = fs::create_dir_all(dir);
        }
    }
    let ctx = Context::new(f.opts.clone());
    match uberconv_formats::pdf::images_to_pdf(&inputs, pdf, &ctx) {
        Ok(()) => {
            let bytes = fs::metadata(pdf).map(|m| m.len()).ok();
            if f.json {
                let outcomes = [Outcome {
                    input: pdf.to_path_buf(),
                    output: Some(pdf.to_path_buf()),
                    status: "ok",
                    plan: Some(format!("{pages} page(s)")),
                    bytes,
                    error: None,
                }];
                print_json(&outcomes, 1, 0, skipped, 0.0, false);
            } else if !f.quiet {
                eprintln!("combined {pages} image(s) into {}", pdf.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            if f.json {
                let outcomes = [Outcome {
                    input: pdf.to_path_buf(),
                    output: Some(pdf.to_path_buf()),
                    status: "failed",
                    plan: None,
                    bytes: None,
                    error: Some(e.to_string()),
                }];
                print_json(&outcomes, 0, 1, skipped, 0.0, false);
            } else {
                eprintln!("error: {}: {e}", pdf.display());
            }
            Ok(ExitCode::FAILURE)
        }
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn print_json(outcomes: &[Outcome], converted: usize, failed: usize, skipped: usize, seconds: f32, dry_run: bool) {
    let results: Vec<String> = outcomes
        .iter()
        .map(|o| {
            let field = |v: Option<&str>| match v {
                Some(v) => format!("\"{}\"", json_escape(&v.replace('\\', "/"))),
                None => "null".to_string(),
            };
            format!(
                "{{\"input\":{},\"output\":{},\"status\":\"{}\",\"plan\":{},\"bytes\":{},\"error\":{}}}",
                field(Some(&o.input.to_string_lossy())),
                field(o.output.as_ref().map(|p| p.to_string_lossy().into_owned()).as_deref()),
                o.status,
                field(o.plan.as_deref()),
                o.bytes.map(|b| b.to_string()).unwrap_or_else(|| "null".into()),
                field(o.error.as_deref()),
            )
        })
        .collect();
    println!(
        "{{\"version\":\"{VERSION}\",\"dry_run\":{dry_run},\"converted\":{converted},\"failed\":{failed},\"skipped\":{skipped},\"seconds\":{seconds:.3},\"results\":[{}]}}",
        results.join(",")
    );
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
