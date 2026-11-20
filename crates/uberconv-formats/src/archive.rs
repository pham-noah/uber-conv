//! ZIP <-> TAR <-> TAR.GZ, repacked as a stream.
//!
//! Entries are copied straight from one archive into the other without
//! extracting to disk. Files, directories and symlinks are kept, with their
//! permissions and modification times. Anything that could escape the
//! extraction directory later (absolute paths, `..`) is rejected up front.

use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Component, Path},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use uberconv_core::{Context, Converter, Cost, Edge, Error, Format, Job, Result};
use zip::{write::SimpleFileOptions, CompressionMethod, DateTime, ZipArchive, ZipWriter};

const ARCHIVES: &[Format] = &[Format::Zip, Format::Tar, Format::TarGz];

pub struct ArchiveConverter;

impl Converter for ArchiveConverter {
    fn name(&self) -> &'static str {
        "archive"
    }

    fn edges(&self) -> Vec<Edge> {
        let mut edges = Vec::new();
        for &from in ARCHIVES {
            for &to in ARCHIVES {
                if from != to {
                    edges.push(Edge::new(from, to, Cost::lossless(25)));
                }
            }
        }
        edges
    }

    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()> {
        let result = repack(job, ctx);
        // An I/O error caused by our own cancellation should read as "cancelled".
        result.map_err(|e| if ctx.cancel.is_cancelled() { Error::Cancelled } else { e })
    }
}

// ───────────────────────── entries ─────────────────────────

enum Kind {
    File,
    Dir,
    Symlink(String),
}

struct Entry {
    name: String,
    kind: Kind,
    mode: u32,
    /// Seconds since the Unix epoch.
    mtime: u64,
    size: u64,
}

/// Normalise and vet an entry name.
fn safe_name(raw: &str) -> Result<String> {
    let name = raw.replace('\\', "/");
    let name = name.trim_start_matches("./");
    let bad = Path::new(name)
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)));
    if bad || name.is_empty() {
        return Err(Error::failed(format!("unsafe path in archive: {raw:?}")));
    }
    Ok(name.to_string())
}

// civil <-> unix day conversions (Howard Hinnant's algorithms)
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn zip_time_to_unix(dt: DateTime) -> u64 {
    let days = days_from_civil(dt.year() as i64, dt.month() as i64, dt.day() as i64);
    (days * 86400 + dt.hour() as i64 * 3600 + dt.minute() as i64 * 60 + dt.second() as i64).max(0) as u64
}

fn unix_to_zip_time(secs: u64) -> Option<DateTime> {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    DateTime::from_date_and_time(u16::try_from(y).ok()?, m as u8, d as u8, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8).ok()
}

// ───────────────────────── progress / cancellation ─────────────────────────

/// Wraps a reader to count bytes, honour cancellation and throttle progress.
struct Tracked<'a, R> {
    inner: R,
    ctx: &'a Context,
    done: Arc<AtomicU64>,
    total: u64,
    last_pct: u32,
}

impl<R: Read> Read for Tracked<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.ctx.cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = self.inner.read(buf)?;
        let done = self.done.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        if self.total > 0 {
            let pct = ((done as f64 / self.total as f64) * 100.0).min(99.0) as u32;
            if pct != self.last_pct {
                self.last_pct = pct;
                self.ctx.report(pct as f32 / 100.0);
            }
        }
        Ok(n)
    }
}

// ───────────────────────── output side ─────────────────────────

enum Sink {
    Plain(BufWriter<File>),
    Gz(GzEncoder<BufWriter<File>>),
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Sink::Plain(w) => w.write(buf),
            Sink::Gz(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Sink::Plain(w) => w.flush(),
            Sink::Gz(w) => w.flush(),
        }
    }
}

impl Sink {
    fn finish(self) -> io::Result<()> {
        match self {
            Sink::Plain(mut w) => w.flush(),
            Sink::Gz(w) => w.finish()?.flush(),
        }
    }
}

enum Output {
    Zip(ZipWriter<BufWriter<File>>),
    Tar(tar::Builder<Sink>),
}

fn zip_err(e: zip::result::ZipError) -> Error {
    Error::failed(format!("zip error: {e}"))
}

impl Output {
    fn create(to: Format, path: &Path) -> Result<Output> {
        let file = BufWriter::new(File::create(path)?);
        Ok(match to {
            Format::Zip => Output::Zip(ZipWriter::new(file)),
            Format::Tar => Output::Tar(tar::Builder::new(Sink::Plain(file))),
            Format::TarGz => Output::Tar(tar::Builder::new(Sink::Gz(GzEncoder::new(file, Compression::new(6))))),
            other => return Err(Error::UnknownFormat(other.name().into())),
        })
    }

    fn add(&mut self, entry: &Entry, data: &mut dyn Read) -> Result<()> {
        match self {
            Output::Zip(zip) => {
                // The zip crate marks archives written on Windows as DOS-made, which
                // makes readers ignore the stored unix mode (they fall back to 0664, so
                // the executable bit and the group/other bits are lost). Always declare
                // Unix so the mode we preserve is the mode every reader sees.
                let mut opts = SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .system(zip::System::Unix)
                    .unix_permissions(entry.mode & 0o7777)
                    .large_file(entry.size >= 0xFFFF_FFFF);
                if let Some(t) = unix_to_zip_time(entry.mtime) {
                    opts = opts.last_modified_time(t);
                }
                match &entry.kind {
                    Kind::Dir => zip.add_directory(entry.name.trim_end_matches('/'), opts).map_err(zip_err)?,
                    Kind::Symlink(target) => zip.add_symlink(&entry.name, target, opts).map_err(zip_err)?,
                    Kind::File => {
                        zip.start_file(&entry.name, opts).map_err(zip_err)?;
                        io::copy(data, zip)?;
                    }
                }
            }
            Output::Tar(tar) => {
                let mut header = tar::Header::new_gnu();
                header.set_mode(entry.mode & 0o7777);
                header.set_mtime(entry.mtime);
                match &entry.kind {
                    Kind::Dir => {
                        header.set_entry_type(tar::EntryType::Directory);
                        header.set_size(0);
                        let name = format!("{}/", entry.name.trim_end_matches('/'));
                        tar.append_data(&mut header, name, io::empty())?;
                    }
                    Kind::Symlink(target) => {
                        header.set_entry_type(tar::EntryType::Symlink);
                        header.set_size(0);
                        tar.append_link(&mut header, &entry.name, target)?;
                    }
                    Kind::File => {
                        header.set_entry_type(tar::EntryType::Regular);
                        header.set_size(entry.size);
                        tar.append_data(&mut header, &entry.name, data)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<()> {
        match self {
            Output::Zip(zip) => {
                zip.finish().map_err(zip_err)?.flush()?;
            }
            Output::Tar(tar) => tar.into_inner()?.finish()?,
        }
        Ok(())
    }
}

// ───────────────────────── the conversion ─────────────────────────

fn repack(job: &Job<'_>, ctx: &Context) -> Result<()> {
    ctx.report(0.02);
    let mut out = Output::create(job.to, job.output)?;
    match job.from {
        Format::Zip => read_zip(job.input, ctx, &mut out)?,
        Format::Tar | Format::TarGz => read_tar(job.from == Format::TarGz, job.input, ctx, &mut out)?,
        other => return Err(Error::UnknownFormat(other.name().into())),
    }
    out.finish()?;
    ctx.report(1.0);
    Ok(())
}

fn read_zip(path: &Path, ctx: &Context, out: &mut Output) -> Result<()> {
    let mut zip = ZipArchive::new(BufReader::new(File::open(path)?)).map_err(|e| Error::failed(format!("not a valid ZIP file: {e}")))?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        total += zip.by_index(i).map(|f| f.size()).unwrap_or(0);
    }
    let done = Arc::new(AtomicU64::new(0));

    for i in 0..zip.len() {
        ctx.cancel.check()?;
        let file = zip.by_index(i).map_err(zip_err)?;
        let raw_name = file.name().map_err(zip_err)?.to_string();
        let name = safe_name(&raw_name)?;
        let mode = file.unix_mode().unwrap_or(if file.is_dir() { 0o755 } else { 0o644 });
        let mtime = file.last_modified().map(zip_time_to_unix).unwrap_or(0);
        let size = file.size();
        let is_symlink = file.is_symlink();
        let is_dir = file.is_dir();

        let mut tracked = Tracked { inner: file, ctx, done: done.clone(), total, last_pct: 0 };
        let kind = if is_dir {
            Kind::Dir
        } else if is_symlink {
            let mut target = String::new();
            tracked.read_to_string(&mut target)?;
            Kind::Symlink(target)
        } else {
            Kind::File
        };
        out.add(&Entry { name, kind, mode, mtime, size }, &mut tracked)?;
    }
    Ok(())
}

fn read_tar(gz: bool, path: &Path, ctx: &Context, out: &mut Output) -> Result<()> {
    let file = File::open(path)?;
    let total = file.metadata()?.len();
    let done = Arc::new(AtomicU64::new(0));
    // Count compressed bytes taken from disk: that's the honest progress measure.
    let counted = Tracked { inner: BufReader::new(file), ctx, done, total, last_pct: 0 };
    let reader: Box<dyn Read + '_> = if gz { Box::new(GzDecoder::new(counted)) } else { Box::new(counted) };

    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        ctx.cancel.check()?;
        let mut entry = entry?;
        let header = entry.header().clone();
        let name = safe_name(&entry.path()?.to_string_lossy())?;
        let (mode, mtime, size) = (header.mode().unwrap_or(0o644), header.mtime().unwrap_or(0), header.size().unwrap_or(0));
        let kind = match header.entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => Kind::File,
            tar::EntryType::Directory => Kind::Dir,
            tar::EntryType::Symlink => match entry.link_name()? {
                Some(target) => Kind::Symlink(target.to_string_lossy().into_owned()),
                None => continue,
            },
            _ => continue, // hard links, devices, fifos, PAX headers...
        };
        out.add(&Entry { name, kind, mode, mtime, size }, &mut entry)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_vetted() {
        assert_eq!(safe_name("dir\\file.txt").unwrap(), "dir/file.txt");
        assert_eq!(safe_name("./a/b").unwrap(), "a/b");
        assert!(safe_name("../evil").is_err());
        assert!(safe_name("a/../../evil").is_err());
        assert!(safe_name("/etc/passwd").is_err());
    }

    #[test]
    fn dates_roundtrip() {
        for secs in [0u64, 86400 * 365, 1_700_000_000, 1_234_567_890, 1_600_000_000 + 12345] {
            if let Some(dt) = unix_to_zip_time(secs) {
                // zip stores 2-second resolution
                assert!(zip_time_to_unix(dt).abs_diff(secs) <= 2, "{secs}");
            }
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(19000), (2022, 1, 8));
    }
}
