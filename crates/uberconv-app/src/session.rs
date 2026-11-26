//! The staged queue, on disk.
//!
//! Closing the window with two thousand files in the list used to lose all of
//! them. Whatever has not run yet is written to `session.json` next to the
//! settings and comes back on the next launch; results are not stored, because
//! a converted file on disk is the result.

use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use uberconv_core::Format;

#[derive(Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub items: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    /// Chosen targets, by extension: ["jpg", "pdf"].
    #[serde(default)]
    pub targets: Vec<String>,
    /// A "Save as…" path, when the row carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_as: Option<PathBuf>,
}

impl Session {
    /// The queue as it was left, minus anything deleted in the meantime.
    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        let Ok(text) = fs::read_to_string(&path) else { return Self::default() };
        let mut session: Session = serde_json::from_str(&text).unwrap_or_default();
        // A stale file (moved, deleted, on an unplugged drive) would fail at
        // convert time; drop it here instead of staging a guaranteed error.
        session.items.retain(|e| e.path.is_file());
        session
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let Ok(json) = serde_json::to_string(self) else { return };
        // Written aside then renamed, so a crash mid-write cannot leave a
        // half-written queue that fails to parse on the next launch.
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }

    fn path() -> Option<PathBuf> {
        Some(config_dir()?.join("session.json"))
    }
}

/// Targets as extensions, which survive a change of format enum.
pub fn ext_of(f: Format) -> String {
    f.ext().to_string()
}

pub fn format_of(ext: &str) -> Option<Format> {
    Format::from_extension(ext)
}

fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("UBERCONV_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(target_os = "windows")]
    let base = env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = None::<PathBuf>;
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = None::<PathBuf>;

    #[cfg(target_os = "windows")]
    {
        base.map(|p| p.join("uberconv"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        base.or_else(|| {
            env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        })
        .map(|p| p.join("uberconv"))
    }
}
