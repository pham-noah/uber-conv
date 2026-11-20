use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::error::{Error, Result};

/// Cooperative cancellation, cheap to clone and share across threads.
///
/// Tokens form a tree: a [`child`](CancelToken::child) is cancelled when its
/// parent is, but cancelling a child leaves the parent (and its siblings)
/// alone. That's how "cancel this file" and "cancel everything" coexist.
#[derive(Clone, Default, Debug)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    parent: Option<Arc<CancelToken>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn child(&self) -> CancelToken {
        CancelToken { flag: Arc::new(AtomicBool::new(false)), parent: Some(Arc::new(self.clone())) }
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst) || self.parent.as_ref().is_some_and(|p| p.is_cancelled())
    }
    /// `Err(Cancelled)` if cancellation was requested.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_follows_parent_but_not_the_other_way() {
        let parent = CancelToken::new();
        let a = parent.child();
        let b = parent.child();
        a.cancel();
        assert!(a.is_cancelled() && !b.is_cancelled() && !parent.is_cancelled());
        parent.cancel();
        assert!(b.is_cancelled());
        assert!(matches!(b.check(), Err(Error::Cancelled)));
    }
}

/// User-facing knobs. Converters read what they care about and ignore the rest.
#[derive(Clone, Debug)]
pub struct Options {
    /// JPEG / WebP / AVIF quality, 1-100 (100 = lossless WebP).
    pub image_quality: u8,
    /// Downscale images so neither side exceeds this many pixels.
    pub image_max_dimension: Option<u32>,
    /// Target bitrate for lossy audio, kbit/s.
    pub audio_bitrate_kbps: u32,
    /// Constant rate factor for H.264 (lower = better); VP9 derives from it.
    pub video_crf: u8,
    /// Which page of a PDF to render (1-based).
    pub pdf_page: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            image_quality: 90,
            image_max_dimension: None,
            audio_bitrate_kbps: 192,
            video_crf: 23,
            pdf_page: 1,
        }
    }
}

type ProgressFn = Arc<dyn Fn(f32) + Send + Sync>;

/// Everything a converter gets besides the job itself.
#[derive(Clone)]
pub struct Context {
    pub options: Options,
    pub cancel: CancelToken,
    progress: ProgressFn,
}

impl Context {
    pub fn new(options: Options) -> Self {
        Context {
            options,
            cancel: CancelToken::new(),
            progress: Arc::new(|_| {}),
        }
    }

    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_progress(mut self, f: impl Fn(f32) + Send + Sync + 'static) -> Self {
        self.progress = Arc::new(f);
        self
    }

    /// Report progress of the current step, 0.0..=1.0.
    pub fn report(&self, fraction: f32) {
        (self.progress)(fraction.clamp(0.0, 1.0));
    }

    /// A context whose 0..1 maps onto `start..start+span` of this one. Used to
    /// stitch multi-step conversions into one smooth progress bar.
    pub fn scaled(&self, start: f32, span: f32) -> Context {
        let parent = self.progress.clone();
        Context {
            options: self.options.clone(),
            cancel: self.cancel.clone(),
            progress: Arc::new(move |p| parent(start + span * p.clamp(0.0, 1.0))),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Context::new(Options::default())
    }
}
