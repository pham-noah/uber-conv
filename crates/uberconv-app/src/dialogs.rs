//! Native file dialogs via `rfd`, because GPUI's own `prompt_for_paths` cannot be
//! given a starting folder. Dialogs block, so each runs on a thread and the caller
//! awaits a channel.

use std::path::{Path, PathBuf};

use async_channel::{bounded, Receiver};

/// Spawn a blocking dialog on its own thread and hand back a receiver for its result.
fn off_thread<T, F>(f: F) -> Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx
}

fn usable(dir: &Option<PathBuf>) -> Option<PathBuf> {
    dir.as_ref().filter(|p| p.is_dir()).cloned()
}

/// Pick one or more files, starting in `start` when it still exists.
pub fn pick_files(start: Option<PathBuf>) -> Receiver<Option<Vec<PathBuf>>> {
    off_thread(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Add files");
        if let Some(dir) = usable(&start) {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_files()
    })
}

/// Pick one folder, starting in `start` when it still exists.
pub fn pick_folder(start: Option<PathBuf>) -> Receiver<Option<PathBuf>> {
    off_thread(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Choose output folder");
        if let Some(dir) = usable(&start) {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder()
    })
}

/// Ask where to write one combined PDF.
pub fn save_pdf(start: Option<PathBuf>, suggested: &str) -> Receiver<Option<PathBuf>> {
    let suggested = suggested.to_string();
    off_thread(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save combined PDF")
            .add_filter("PDF", &["pdf"])
            .set_file_name(&suggested);
        if let Some(dir) = usable(&start) {
            dialog = dialog.set_directory(dir);
        }
        let picked = dialog.save_file()?;
        // rfd doesn't always append the extension the user typed around.
        if picked.extension().is_none() {
            Some(picked.with_extension("pdf"))
        } else {
            Some(picked)
        }
    })
}

/// The folder a picked path should be remembered by: its parent, or the path itself.
pub fn folder_of(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    path.parent().map(Path::to_path_buf)
}

/// A folder that exists for the first launch: the desktop, else home.
pub fn first_folder() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)?;
    let desktop = home.join("Desktop");
    Some(if desktop.is_dir() { desktop } else { home })
}
