//! Native file dialogs.
//!
//! GPUI 0.2's own `prompt_for_paths` cannot be given a starting folder, so
//! "remember the last folder" is impossible with it. `rfd` wraps the same
//! platform dialogs (`IFileOpenDialog` with `SetFolder` on Windows, NSOpenPanel
//! on macOS, the XDG portal / GTK chooser on Linux) and does take one.
//!
//! Dialogs are modal and block, so they run on their own thread; the GPUI side
//! awaits a channel instead of the main thread.

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
