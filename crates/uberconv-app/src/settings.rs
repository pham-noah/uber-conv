//! User settings, persisted as JSON in the platform config directory.
//! (`$UBERCONV_CONFIG_DIR` overrides the location, handy for tests.)

use std::{
    env, fs,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use uberconv_core::Options;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ThemeChoice {
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default, Debug)]
pub enum OutputMode {
    #[default]
    NextToSource,
    Folder(PathBuf),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub output: OutputMode,
    /// Overwrite an existing file instead of writing `name-converted.ext`.
    pub overwrite: bool,
    pub image_quality: u8,
    pub image_max_dimension: Option<u32>,
    pub audio_bitrate_kbps: u32,
    pub video_crf: u8,
    /// Desktop notification when a long batch finishes.
    pub notify: bool,
    /// Folder the last file picker was in ("remember last folder").
    pub last_dir: Option<PathBuf>,
    /// Output folder the last folder picker was in.
    pub output_pick_dir: Option<PathBuf>,
    pub window: Option<(f32, f32)>,
}

impl Default for Settings {
    fn default() -> Self {
        let o = Options::default();
        Settings {
            theme: ThemeChoice::Auto,
            output: OutputMode::NextToSource,
            overwrite: false,
            image_quality: o.image_quality,
            image_max_dimension: o.image_max_dimension,
            audio_bitrate_kbps: o.audio_bitrate_kbps,
            video_crf: o.video_crf,
            notify: true,
            last_dir: None,
            output_pick_dir: None,
            window: None,
        }
    }
}

impl Settings {
    pub fn options(&self) -> Options {
        Options {
            image_quality: self.image_quality,
            image_max_dimension: self.image_max_dimension,
            audio_bitrate_kbps: self.audio_bitrate_kbps,
            video_crf: self.video_crf,
            ..Options::default()
        }
    }

    /// Reset everything except the remembered window size.
    pub fn reset_preferences(&mut self) {
        let window = self.window;
        *self = Settings { window, ..Settings::default() };
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best effort: a failure to save settings should never bother the user.
    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
        }
    }

    fn path() -> Option<PathBuf> {
        Some(config_dir()?.join("settings.json"))
    }
}

// Only the macOS/Linux config paths call this; Windows uses %APPDATA%.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn home() -> Option<PathBuf> {
    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("UBERCONV_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(target_os = "windows")]
    let base = env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = home().map(|h| h.join("Library").join("Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".config")));
    base.map(|b| b.join("uberconv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_forward_compat() {
        let dir = tempfile::tempdir().unwrap();
        env::set_var("UBERCONV_CONFIG_DIR", dir.path());
        let s = Settings {
            theme: ThemeChoice::Light,
            output: OutputMode::Folder("/tmp/out".into()),
            image_quality: 75,
            window: Some((900.0, 700.0)),
            ..Settings::default()
        };
        s.save();
        let back = Settings::load();
        assert_eq!(back.theme, ThemeChoice::Light);
        assert_eq!(back.output, OutputMode::Folder("/tmp/out".into()));
        assert_eq!(back.image_quality, 75);
        assert_eq!(back.window, Some((900.0, 700.0)));

        // unknown / missing fields must not break loading
        fs::write(dir.path().join("settings.json"), r#"{"theme":"Dark","future_option":true}"#).unwrap();
        let partial = Settings::load();
        assert_eq!(partial.theme, ThemeChoice::Dark);
        assert_eq!(partial.audio_bitrate_kbps, 192);

        // a corrupt file falls back to defaults instead of panicking
        fs::write(dir.path().join("settings.json"), "{{{ nope").unwrap();
        assert_eq!(Settings::load().theme, ThemeChoice::Auto);

        // reset keeps only the window size
        let mut s = back;
        s.reset_preferences();
        assert_eq!(s.image_quality, 90);
        assert_eq!(s.window, Some((900.0, 700.0)));
    }
}
