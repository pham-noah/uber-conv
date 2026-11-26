use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicIsize, Ordering},
};

use uberconv_core::{Format, Options};

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
    /// The folder each file was found under, for files that came out of a
    /// dropped folder. Output mirrors that shape, so `2023/a.png` and
    /// `2024/a.png` no longer fight over one output name.
    pub roots: HashMap<PathBuf, PathBuf>,
    /// Files inside dropped folders that we have no converter for.
    pub skipped: usize,
    pub from_folders: usize,
    /// The walk hit [`MAX_FILES`] and stopped: the queue is not all of it.
    pub truncated: bool,
}

pub const MAX_FILES: usize = 5000;
const MAX_DEPTH: usize = 8;

/// Expand dropped paths: files are kept as-is (so unsupported ones can show an
/// error), folders are walked and only supported files are taken.
pub fn collect(paths: Vec<PathBuf>) -> Collected {
    let mut out = Collected::default();
    for p in paths {
        if p.is_dir() {
            walk(&p, &p, 0, &mut out);
        } else if p.is_file() {
            out.files.push(p);
        }
    }
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Collected) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if out.files.len() >= MAX_FILES {
            out.truncated = true;
            return;
        }
        let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, depth + 1, out);
            }
        } else if path.is_file() {
            if Format::from_path(&path).is_some() {
                out.roots.insert(path.clone(), root.to_path_buf());
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
/// * "next to the original" writes beside the source file;
/// * an output *folder* mirrors the source tree under it, so a dropped folder
///   comes back out looking like itself (`raw/2023/a.png` → `out/2023/a.png`).
///   Files dropped one at a time land straight in `out`.
///
/// Never hands out the same name twice within a batch (`claimed`).
pub fn output_path(
    input: &Path,
    to: Format,
    mode: &OutputMode,
    overwrite: bool,
    claimed: &mut HashSet<PathBuf>,
    root: Option<&Path>,
) -> PathBuf {
    let dir: PathBuf = match mode {
        OutputMode::Folder(base) => {
            let rel = root
                .and_then(|r| input.parent().and_then(|p| p.strip_prefix(r).ok()))
                .filter(|rel| !rel.as_os_str().is_empty());
            match rel {
                Some(rel) => base.join(rel),
                None => base.clone(),
            }
        }
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

/// Create the folder a planned output lives in (mirrored trees need this).
pub fn ensure_parent(path: &Path) {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = fs::create_dir_all(dir);
        }
    }
}

/// Move the file we are about to replace to the Recycle Bin (Trash on macOS,
/// plain delete elsewhere). "Overwrite" should never silently destroy a file.
pub fn retire(path: &Path) {
    if !path.exists() {
        return;
    }
    #[cfg(windows)]
    let ok = trash::delete(path).is_ok();
    #[cfg(not(windows))]
    let ok = fs::remove_file(path).is_ok();
    if !ok {
        let _ = fs::remove_file(path);
    }
}

// ───────────────────────── windows plumbing ─────────────────────────

/// Windows only shows toasts for app ids it knows; register ours once per user.
#[cfg(target_os = "windows")]
const APP_ID: &str = "NoahPham.uberconv";
#[cfg(not(target_os = "windows"))]
const APP_ID: &str = "uberconv";

/// `CREATE_NO_WINDOW`: a GUI process that shells out must not flash a console.
#[cfg(windows)]
fn quiet(command: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW)
}

/// Fire-and-forget desktop notification (Linux: notify-send, macOS: osascript,
/// Windows: a WinRT toast).
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
            ensure_app_id();
            let esc = |s: &str| s.replace('\'', "''").replace('<', "&lt;").replace('>', "&gt;").replace('&', "&amp;");
            let script = format!(
                "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
                 $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
                 $t = $x.GetElementsByTagName('text'); $t.Item(0).AppendChild($x.CreateTextNode('{}')) | Out-Null; \
                 $t.Item(1).AppendChild($x.CreateTextNode('{}')) | Out-Null; \
                 [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{APP_ID}').Show([Windows.UI.Notifications.ToastNotification]::new($x))",
                esc(&title),
                esc(&body)
            );
            let _ = quiet(&mut Command::new("powershell"))
                .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    });
}

/// Register our app id in `HKCU` so Windows treats the toast as ours (name and
/// icon in Action Center). Only needed once per machine.
#[cfg(target_os = "windows")]
fn ensure_app_id() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let key = format!(r"HKCU\Software\Classes\AppUserModelId\{APP_ID}");
        let reg = |value: &str, data: &str| {
            let _ = quiet(&mut Command::new("reg"))
                .args(["add", &key, "/v", value, "/t", "REG_SZ", "/d", data, "/f"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        };
        reg("DisplayName", "uberconv");
        if let Ok(exe) = std::env::current_exe() {
            reg("IconUri", &exe.display().to_string());
        }
    });
}

/// Last window handle we saw, so a finished batch can flash the taskbar.
static HWND: AtomicIsize = AtomicIsize::new(0);

pub fn remember_window(window: &gpui::Window) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        // `Window::window_handle` is also an inherent method (it returns gpui's
        // own handle), so name the trait explicitly.
        if let Ok(handle) = HasWindowHandle::window_handle(window) {
            if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                HWND.store(win32.hwnd.get(), Ordering::Relaxed);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// Nudge the taskbar button. Unlike a toast this works with banners switched
/// off, so it is the "look over here" cue that can't be configured away.
pub fn flash_taskbar() {
    #[cfg(windows)]
    unsafe {
        #[repr(C)]
        struct FlashInfo {
            size: u32,
            hwnd: *mut core::ffi::c_void,
            flags: u32,
            count: u32,
            timeout: u32,
        }
        const FLASHW_ALL: u32 = 0x3;
        const FLASHW_TIMERNOFG: u32 = 0xC;
        extern "system" {
            fn FlashWindowEx(info: *const FlashInfo) -> i32;
        }
        let hwnd = HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return;
        }
        let info = FlashInfo {
            size: std::mem::size_of::<FlashInfo>() as u32,
            hwnd: hwnd as *mut core::ffi::c_void,
            flags: FLASHW_ALL | FLASHW_TIMERNOFG,
            count: 0,
            timeout: 0,
        };
        FlashWindowEx(&info);
    }
}

/// Pixel size of an image, read from the file header rather than decoded:
/// cheap enough to run over a whole queue of photos.
pub fn image_dims(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// Rough output size in bytes, from the source's pixel count and the quality
/// knob. This is the "≈ 340 KB" on a row: a rule of thumb, not a promise.
pub fn estimate_size(to: Format, (w, h): (u32, u32), opts: &Options) -> Option<u64> {
    let (mut w, mut h) = (f64::from(w), f64::from(h));
    if let Some(max) = opts.image_max_dimension {
        let scale = (f64::from(max) / w.max(h)).min(1.0);
        w *= scale;
        h *= scale;
    }
    let px = w * h;
    if px < 1.0 {
        return None;
    }
    let q = f64::from(opts.image_quality.clamp(1, 100)) / 100.0;
    // Fitted to ordinary photographs: ~0.07 B/px at q75, ~0.15 at q90,
    // ~0.25 at q100.
    let jpeg = 0.0015 * (5.1 * q).exp();
    let per_px = match to {
        Format::Jpeg => jpeg,
        Format::WebP => jpeg * 0.65,
        Format::Avif => jpeg * 0.55,
        Format::Png => 1.1,
        Format::Tiff | Format::Bmp => 3.0,
        Format::Gif => 0.35,
        _ => return None,
    };
    Some((px * per_px) as u64)
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
        let first = output_path(&a, Format::Jpeg, &m, false, &mut claimed, None);
        assert_eq!(first, dir.path().join("a.jpg"));
        // another input that wants the same name gets a different one
        let b = dir.path().join("a.bmp");
        let second = output_path(&b, Format::Jpeg, &m, false, &mut claimed, None);
        assert_eq!(second, dir.path().join("a-converted.jpg"));
        // existing files are protected unless overwrite is on
        fs::write(dir.path().join("a.webp"), "old").unwrap();
        assert_eq!(
            output_path(&a, Format::WebP, &m, false, &mut HashSet::new(), None),
            dir.path().join("a-converted.webp")
        );
        assert_eq!(output_path(&a, Format::WebP, &m, true, &mut HashSet::new(), None), dir.path().join("a.webp"));
    }

    #[test]
    fn output_folder_is_honoured() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let a = src.path().join("song.wav");
        let p = output_path(
            &a,
            Format::Mp3,
            &OutputMode::Folder(out.path().to_path_buf()),
            false,
            &mut HashSet::new(),
            None,
        );
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
        assert!(!got.truncated);
        // an explicitly dropped unsupported file is kept so the UI can explain it
        let explicit = collect(vec![dir.path().join("notes.xyz")]);
        assert_eq!(explicit.files.len(), 1);
    }

    #[test]
    fn dropped_folders_keep_their_shape() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("2023")).unwrap();
        fs::create_dir_all(src.path().join("2024")).unwrap();
        let (a, b) = (src.path().join("2023/a.png"), src.path().join("2024/a.png"));
        fs::write(&a, "x").unwrap();
        fs::write(&b, "x").unwrap();

        let got = collect(vec![src.path().to_path_buf()]);
        assert_eq!(got.roots.len(), 2);
        let mode = OutputMode::Folder(out.path().to_path_buf());
        let mut claimed = HashSet::new();
        // Same file name, different years: no "-converted" suffix, shape kept.
        let first = output_path(&a, Format::Jpeg, &mode, false, &mut claimed, got.roots.get(&a).map(|p| p.as_path()));
        let second = output_path(&b, Format::Jpeg, &mode, false, &mut claimed, got.roots.get(&b).map(|p| p.as_path()));
        assert_eq!(first, out.path().join("2023/a.jpg"));
        assert_eq!(second, out.path().join("2024/a.jpg"));
        ensure_parent(&second);
        assert!(out.path().join("2024").is_dir());

        // A file dropped on its own still lands directly in the output folder.
        let solo = dir_file(&src.path().join("2023"), "solo.png");
        let p = output_path(&solo, Format::Jpeg, &mode, false, &mut HashSet::new(), None);
        assert_eq!(p, out.path().join("solo.jpg"));
    }

    fn dir_file(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, "x").unwrap();
        p
    }

    #[test]
    fn size_change_text() {
        let (t, good) = size_change(3_000_000, 500_000);
        assert!(good && t.contains("-83%"), "{t}");
        assert!(!size_change(100, 200).1);
    }
}
