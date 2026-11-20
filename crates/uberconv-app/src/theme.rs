//! Design tokens. Two palettes (dark/light) behind `t()`, plus brand colours.
//!
//! All text colours meet WCAG AA contrast (>= 4.5:1) against the surfaces they
//! are used on, including the "faint" section labels.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{linear_color_stop, linear_gradient, rgb, rgba, Background, Rgba};
use uberconv_core::Family;

pub const VIOLET: u32 = 0x8b5cf6;
pub const BLUE: u32 = 0x3b82f6;
pub const CYAN: u32 = 0x22d3ee;

pub struct Theme {
    pub ink: u32,    // window background
    pub panel: u32,  // cards
    pub raised: u32, // tooltips, popovers
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub line: u32,    // hairline borders (RRGGBBAA)
    pub wash: u32,    // subtle fills (RRGGBBAA)
    pub wash_hi: u32, // hover fills (RRGGBBAA)
    pub good: u32,
    pub bad: u32,
    pub warn: u32,
    pub glow: u8, // alpha of the violet glow behind the header
    pub light: bool,
}

static DARK: Theme = Theme {
    ink: 0x0a0a12,
    panel: 0x13131f,
    raised: 0x1d1d2e,
    text: 0xeceef7,
    muted: 0xa0a2bb,
    faint: 0x8486a3,
    line: 0xffffff14,
    wash: 0xffffff0a,
    wash_hi: 0xffffff16,
    good: 0x34d399,
    bad: 0xfb7185,
    warn: 0xfbbf24,
    glow: 0x30,
    light: false,
};

static LIGHT: Theme = Theme {
    ink: 0xf3f4fa,
    panel: 0xffffff,
    raised: 0xffffff,
    text: 0x161630,
    muted: 0x575976,
    faint: 0x666885,
    line: 0x16163022,
    wash: 0x1616300a,
    wash_hi: 0x16163014,
    good: 0x047857,
    bad: 0xbe123c,
    warn: 0xb45309,
    glow: 0x1e,
    light: true,
};

static LIGHT_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_light(light: bool) {
    LIGHT_MODE.store(light, Ordering::Relaxed);
}

pub fn t() -> &'static Theme {
    if LIGHT_MODE.load(Ordering::Relaxed) {
        &LIGHT
    } else {
        &DARK
    }
}

pub fn with_alpha(color: u32, alpha: u8) -> Rgba {
    rgba((color << 8) | alpha as u32)
}

/// The brand gradient: violet -> cyan.
pub fn brand(angle: f32) -> Background {
    linear_gradient(angle, linear_color_stop(rgb(VIOLET), 0.0), linear_color_stop(rgb(CYAN), 1.0))
}

pub fn ink() -> Rgba { rgb(t().ink) }
pub fn panel() -> Rgba { rgb(t().panel) }
pub fn raised() -> Rgba { rgb(t().raised) }
pub fn text() -> Rgba { rgb(t().text) }
pub fn muted() -> Rgba { rgb(t().muted) }
pub fn faint() -> Rgba { rgb(t().faint) }
pub fn line() -> Rgba { rgba(t().line) }
pub fn wash() -> Rgba { rgba(t().wash) }
pub fn wash_hi() -> Rgba { rgba(t().wash_hi) }
pub fn good() -> Rgba { rgb(t().good) }
pub fn bad() -> Rgba { rgb(t().bad) }
pub fn warn() -> Rgba { rgb(t().warn) }
pub fn glow() -> Rgba { with_alpha(VIOLET, t().glow) }

/// Family accent colours, darkened in the light theme to keep text readable.
pub fn family_color(f: Family) -> u32 {
    if t().light {
        match f {
            Family::Image => 0xdb2777,
            Family::Audio => 0xb45309,
            Family::Video => 0x7c3aed,
            Family::Data => 0x047857,
            Family::Text => 0x0369a1,
            Family::Archive => 0x4d7c0f,
        }
    } else {
        match f {
            Family::Image => 0xf472b6,
            Family::Audio => 0xfbbf24,
            Family::Video => 0xa78bfa,
            Family::Data => 0x34d399,
            Family::Text => 0x38bdf8,
            Family::Archive => 0xa3e635,
        }
    }
}

pub fn family_icon(f: Family) -> &'static str {
    match f {
        Family::Image => "icons/image.svg",
        Family::Audio => "icons/audio.svg",
        Family::Video => "icons/video.svg",
        Family::Data => "icons/data.svg",
        Family::Text => "icons/text.svg",
        Family::Archive => "icons/archive.svg",
    }
}
