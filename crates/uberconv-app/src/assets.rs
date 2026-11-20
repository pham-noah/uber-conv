use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};

macro_rules! embed {
    ($($name:literal),+ $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/", $name)) as &[u8])),+]
    };
}

static ASSETS: &[(&str, &[u8])] = embed![
    "icons/mark.svg",
    "icons/upload.svg",
    "icons/check.svg",
    "icons/x.svg",
    "icons/image.svg",
    "icons/audio.svg",
    "icons/video.svg",
    "icons/data.svg",
    "icons/text.svg",
    "icons/file.svg",
    "icons/play.svg",
    "icons/stop.svg",
    "icons/arrow.svg",
    "icons/folder.svg",
    "icons/alert.svg",
    "icons/sparkle.svg",
    "icons/broom.svg",
    "icons/settings.svg",
    "icons/retry.svg",
    "icons/chevron-down.svg",
    "icons/chevron-up.svg",
    "icons/sun.svg",
    "icons/moon.svg",
    "icons/monitor.svg",
    "icons/clock.svg",
    "icons/copy.svg",
    "icons/layers.svg",
    "icons/archive.svg",
    "icons/undo.svg",
    "icons/grip.svg",
];

/// Inter (SIL OFL 1.1, see assets/fonts/Inter-LICENSE.txt), bundled so weights
/// and spacing look the same on every platform.
static FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
];

pub const FONT_FAMILY: &str = "Inter";

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ASSETS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

pub fn install_fonts(cx: &App) {
    let fonts = FONTS.iter().map(|b| Cow::Borrowed(*b)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("could not load bundled fonts: {e}");
    }
}
