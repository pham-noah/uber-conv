use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use uberconv_core::Format;

use crate::settings::OutputMode;

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// `3.3 MB -> 512 KB` plus whether that's a saving.
pub fn size_change(before: u64, after: u64) -> (String, bool) {
    let pct = if before > 0 { (after as f64 / before as f64 - 1.0) * 100.0 } else { 0.0 };
    (format!("{} → {} · {:+.0}%", human_size(before), human_size(after), pct), after <= before)
}

#[derive(Default, Debug)]
pub struct Collected {
    pub files: Vec<PathBuf>,
    /// Files inside dropped folders that we have no converter for.
    pub skipped: usize,
    pub from_folders: usize,
}

const MAX_FILES: usize = 5000;
const MAX_DEPTH: usize = 8;

/// Expand dropped paths: files are kept as-is (so unsupported ones can show an
/// error), folders are walked and only supported files are taken.
pub fn collect(paths: Vec<PathBuf>) -> Collected {
    let mut out = Collected::default();
    for p in paths {
        if p.is_dir() {
            walk(&p, 0, &mut out);
        } else if p.is_file() {
            out.files.push(p);
        }
    }
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Collected) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if out.files.len() >= MAX_FILES {
            return;
        }
        let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            if depth < MAX_DEPTH {
                walk(&path, depth + 1, out);
            }
        } else if path.is_file() {
            if Format::from_path(&path).is_some() {
                out.files.push(path);
                out.from_folders += 1;
            } else {
                out.skipped += 1;
            }
        }
    }
}

/// Where should `input` converted to `to` be written?
///
/// Respects the output-folder setting and the overwrite preference, and never
/// hands out the same name twice within a batch (`claimed`).
pub fn output_path(
    input: &Path,
    to: Format,
    mode: &OutputMode,
    overwrite: bool,
    claimed: &mut HashSet<PathBuf>,
) -> PathBuf {
    let dir: PathBuf = match mode {
        OutputMode::Folder(d) => d.clone(),
        OutputMode::NextToSource => input.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let stem = Format::stem_of(input);

    let first = dir.join(format!("{stem}.{}", to.ext()));
    let free = overwrite || !first.exists();
    if first != input && free && claimed.insert(first.clone()) {
        return first;
    }
    for n in 1.. {
        let name = if n == 1 {
            format!("{stem}-converted.{}", to.ext())
        } else {
            format!("{stem}-converted{n}.{}", to.ext())
        };
        let candidate = dir.join(name);
        if !candidate.exists() && claimed.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

/// Fire-and-forget desktop notification (Linux: notify-send, macOS: osascript).
pub fn notify(title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        {
            let _ = Command::new("notify-send").args(["-a", "uberconv", &title, &body]).status();
        }
        #[cfg(target_os = "macos")]
        {
            let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!("display notification \"{}\" with title \"{}\"", esc(&body), esc(&title));
            let _ = Command::new("osascript").args(["-e", &script]).status();
        }
        #[cfg(target_os = "windows")]
        {
            // A toast through the WinRT API, using PowerShell's own app id so no
            // registration is needed. Untested: I could not run Windows here.
            let esc = |s: &str| s.replace('\'', "''").replace('<', "&lt;").replace('>', "&gt;").replace('&', "&amp;");
            let script = format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
                 $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
                 $t = $x.GetElementsByTagName('text'); $t.Item(0).AppendChild($x.CreateTextNode('{}')) | Out-Null; \
                 $t.Item(1).AppendChild($x.CreateTextNode('{}')) | Out-Null; \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($x))",
                esc(&title),
                esc(&body)
            );
            let _ = Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]).status();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names_never_collide() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        fs::write(&a, "x").unwrap();
        let mut claimed = HashSet::new();
        let m = OutputMode::NextToSource;
        let first = output_path(&a, Format::Jpeg, &m, false, &mut claimed);
        assert_eq!(first, dir.path().join("a.jpg"));
        // another input that wants the same name gets a different one
        let b = dir.path().join("a.bmp");
        let second = output_path(&b, Format::Jpeg, &m, false, &mut claimed);
        assert_eq!(second, dir.path().join("a-converted.jpg"));
        // existing files are protected unless overwrite is on
        fs::write(dir.path().join("a.webp"), "old").unwrap();
        assert_eq!(output_path(&a, Format::WebP, &m, false, &mut HashSet::new()), dir.path().join("a-converted.webp"));
        assert_eq!(output_path(&a, Format::WebP, &m, true, &mut HashSet::new()), dir.path().join("a.webp"));
    }

    #[test]
    fn output_folder_is_honoured() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let a = src.path().join("song.wav");
        let p = output_path(&a, Format::Mp3, &OutputMode::Folder(out.path().to_path_buf()), false, &mut HashSet::new());
        assert_eq!(p, out.path().join("song.mp3"));
    }

    #[test]
    fn folders_are_walked_and_filtered() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
        fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        for f in ["a.png", "b.txt", "notes.xyz", "sub/c.mp3", "sub/deep/d.json", ".hidden/e.png"] {
            fs::write(dir.path().join(f), "x").unwrap();
        }
        let got = collect(vec![dir.path().to_path_buf()]);
        assert_eq!(got.files.len(), 4); // a.png b.txt c.mp3 d.json
        assert_eq!(got.skipped, 1); // notes.xyz
        // an explicitly dropped unsupported file is kept so the UI can explain it
        let explicit = collect(vec![dir.path().join("notes.xyz")]);
        assert_eq!(explicit.files.len(), 1);
    }

    #[test]
    fn size_change_text() {
        let (t, good) = size_change(3_000_000, 500_000);
        assert!(good && t.contains("-83%"), "{t}");
        assert!(!size_change(100, 200).1);
    }
}
