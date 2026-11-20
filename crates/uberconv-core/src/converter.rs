use std::path::Path;

use crate::{context::Context, error::Result, format::Format};

/// Extra path cost per lossy step, so the planner avoids lossy routes when a
/// lossless one of reasonable length exists.
pub const LOSSY_PENALTY: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    /// Relative effort. Rough scale: data 5, image 10, audio 20, video 60+.
    pub weight: u32,
    /// Does this step throw information away?
    pub lossy: bool,
}

impl Cost {
    pub const fn lossless(weight: u32) -> Self {
        Cost { weight, lossy: false }
    }
    pub const fn lossy(weight: u32) -> Self {
        Cost { weight, lossy: true }
    }
    pub fn total(self) -> u32 {
        self.weight + if self.lossy { LOSSY_PENALTY } else { 0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub from: Format,
    pub to: Format,
    pub cost: Cost,
}

impl Edge {
    pub fn new(from: Format, to: Format, cost: Cost) -> Self {
        Edge { from, to, cost }
    }
}

pub struct Job<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub from: Format,
    pub to: Format,
}

pub trait Converter: Send + Sync {
    fn name(&self) -> &'static str;

    /// `true` for converters that already use several cores themselves or
    /// spawn heavy external processes (FFmpeg). Batches run those one at a time
    /// and spread the light, in-process converters across all cores.
    fn heavy(&self) -> bool {
        false
    }

    /// Every `(from, to)` pair this converter can do directly.
    fn edges(&self) -> Vec<Edge>;

    /// Convert `job.input` into `job.output`. Should call `ctx.report` as it
    /// goes and bail out with `Error::Cancelled` when `ctx.cancel` fires.
    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()>;
}
