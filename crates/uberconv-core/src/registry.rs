use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use crate::{
    context::{CancelToken, Context, Options},
    converter::{Converter, Edge, Job},
    error::{Error, Result},
    format::Format,
};

struct IndexedEdge {
    edge: Edge,
    converter: usize,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub converter: usize,
    pub converter_name: &'static str,
    pub from: Format,
    pub to: Format,
    pub lossy: bool,
    /// Runs through a heavy converter (see [`Converter::heavy`]).
    pub heavy: bool,
}

/// One file in a batch.
#[derive(Clone, Debug)]
pub struct BatchItem {
    pub input: PathBuf,
    pub output: PathBuf,
    pub from: Format,
    pub to: Format,
    /// Cancels just this item. Make it a [`CancelToken::child`] of the batch's
    /// token so cancelling the whole batch also stops it. `None` means the
    /// item only follows the batch token.
    pub cancel: Option<CancelToken>,
    /// Overrides the batch's options for this item only.
    pub options: Option<Options>,
}

impl BatchItem {
    pub fn new(input: PathBuf, output: PathBuf, from: Format, to: Format) -> Self {
        BatchItem { input, output, from, to, cancel: None, options: None }
    }
}

#[derive(Debug)]
pub enum BatchEvent {
    /// A worker picked this item up (it was only queued until now).
    Started,
    Progress(f32),
    Done,
    Failed(Error),
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn is_lossy(&self) -> bool {
        self.steps.iter().any(|s| s.lossy)
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// e.g. `YAML -> JSON -> CSV`
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for (i, s) in self.steps.iter().enumerate() {
            if i == 0 {
                out.push_str(s.from.name());
            }
            out.push_str(" -> ");
            out.push_str(s.to.name());
        }
        out
    }
}

#[derive(Default)]
pub struct Registry {
    converters: Vec<Box<dyn Converter>>,
    edges: Vec<IndexedEdge>,
    adjacency: HashMap<Format, Vec<usize>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, converter: Box<dyn Converter>) {
        let idx = self.converters.len();
        for edge in converter.edges() {
            let ei = self.edges.len();
            self.adjacency.entry(edge.from).or_default().push(ei);
            self.edges.push(IndexedEdge { edge, converter: idx });
        }
        self.converters.push(converter);
    }

    pub fn converter_names(&self) -> Vec<&'static str> {
        self.converters.iter().map(|c| c.name()).collect()
    }

    /// Every format that can be reached from `from` (excluding `from` itself).
    pub fn reachable_from(&self, from: Format) -> Vec<Format> {
        Format::ALL
            .iter()
            .copied()
            .filter(|&to| to != from && self.plan(from, to).is_some())
            .collect()
    }

    /// Cheapest chain of conversions (Dijkstra; lossy steps are penalised).
    ///
    /// Changing media family (e.g. video -> GIF) is only allowed as one direct
    /// step. That keeps routes sensible: MP4 -> GIF and GIF -> MP4 are fine, but
    /// MP4 -> GIF -> PNG or PNG -> GIF -> MP4 aren't offered just because GIF
    /// happens to sit in the middle.
    pub fn plan(&self, from: Format, to: Format) -> Option<Plan> {
        if from == to {
            return None;
        }
        let mut dist: HashMap<Format, u32> = HashMap::new();
        let mut prev: HashMap<Format, usize> = HashMap::new();
        let mut heap = BinaryHeap::new();
        dist.insert(from, 0);
        heap.push(Reverse((0u32, from)));

        while let Some(Reverse((d, node))) = heap.pop() {
            if node == to {
                break;
            }
            if d > *dist.get(&node).unwrap_or(&u32::MAX) {
                continue;
            }
            for &ei in self.adjacency.get(&node).map(|v| v.as_slice()).unwrap_or(&[]) {
                let e = &self.edges[ei].edge;
                if e.from.family() != e.to.family() && !(e.from == from && e.to == to) {
                    continue;
                }
                let nd = d + e.cost.total();
                if nd < *dist.get(&e.to).unwrap_or(&u32::MAX) {
                    dist.insert(e.to, nd);
                    prev.insert(e.to, ei);
                    heap.push(Reverse((nd, e.to)));
                }
            }
        }

        if !prev.contains_key(&to) {
            return None;
        }
        let mut steps = Vec::new();
        let mut cur = to;
        while cur != from {
            let ie = &self.edges[prev[&cur]];
            steps.push(Step {
                converter: ie.converter,
                converter_name: self.converters[ie.converter].name(),
                from: ie.edge.from,
                to: ie.edge.to,
                lossy: ie.edge.cost.lossy,
                heavy: self.converters[ie.converter].heavy(),
            });
            cur = ie.edge.from;
        }
        steps.reverse();
        Some(Plan { steps })
    }

    /// Convert many files at once.
    ///
    /// In-process ("light") conversions fan out across all CPU cores; anything
    /// that goes through a heavy converter (FFmpeg) runs one at a time on its
    /// own worker, since FFmpeg is multi-threaded already. `on_event` is called
    /// on the calling thread, so it doesn't need to be `Sync`.
    pub fn convert_batch<F>(&self, items: &[BatchItem], options: &Options, cancel: &CancelToken, mut on_event: F)
    where
        F: FnMut(usize, BatchEvent),
    {
        let plans: Vec<Option<Plan>> = items.iter().map(|it| self.plan(it.from, it.to)).collect();
        let (tx, rx) = mpsc::channel::<(usize, BatchEvent)>();

        let mut light = Vec::new();
        let mut heavy = Vec::new();
        for (i, plan) in plans.iter().enumerate() {
            match plan {
                None => {
                    let err = Error::NoPath { from: items[i].from, to: items[i].to };
                    let _ = tx.send((i, BatchEvent::Failed(err)));
                }
                Some(p) if p.steps.iter().any(|s| s.heavy) => heavy.push(i),
                Some(_) => light.push(i),
            }
        }

        let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let light_workers = cores.min(light.len());
        let (light_next, heavy_next) = (AtomicUsize::new(0), AtomicUsize::new(0));

        let run = |queue: &Vec<usize>, next: &AtomicUsize, tx: mpsc::Sender<(usize, BatchEvent)>| loop {
            let k = next.fetch_add(1, Ordering::SeqCst);
            let Some(&i) = queue.get(k) else { break };
            let plan = plans[i].as_ref().expect("queued items have plans");
            let token = items[i].cancel.clone().unwrap_or_else(|| cancel.clone());
            if cancel.is_cancelled() || token.is_cancelled() {
                let _ = tx.send((i, BatchEvent::Failed(Error::Cancelled)));
                continue;
            }
            let _ = tx.send((i, BatchEvent::Started));
            let progress_tx = tx.clone();
            let item_options = items[i].options.as_ref().unwrap_or(options);
            let ctx = Context::new(item_options.clone())
                .with_cancel(token)
                .with_progress(move |p| {
                    let _ = progress_tx.send((i, BatchEvent::Progress(p)));
                });
            let event = match self.run_plan(plan, &items[i].input, &items[i].output, &ctx) {
                Ok(()) => BatchEvent::Done,
                Err(e) => BatchEvent::Failed(e),
            };
            let _ = tx.send((i, event));
        };

        thread::scope(|scope| {
            for _ in 0..light_workers {
                let (tx, queue, next, run) = (tx.clone(), &light, &light_next, &run);
                scope.spawn(move || run(queue, next, tx));
            }
            if !heavy.is_empty() {
                let (tx, queue, next, run) = (tx.clone(), &heavy, &heavy_next, &run);
                scope.spawn(move || run(queue, next, tx));
            }
            drop(tx);
            for (i, event) in rx {
                on_event(i, event);
            }
        });
    }

    /// Plan and run in one go.
    pub fn convert(
        &self,
        input: &Path,
        output: &Path,
        from: Format,
        to: Format,
        ctx: &Context,
    ) -> Result<Plan> {
        let plan = self.plan(from, to).ok_or(Error::NoPath { from, to })?;
        self.run_plan(&plan, input, output, ctx)?;
        Ok(plan)
    }

    pub fn run_plan(&self, plan: &Plan, input: &Path, output: &Path, ctx: &Context) -> Result<()> {
        if input == output {
            return Err(Error::failed("input and output are the same file"));
        }
        let existed = output.exists();
        let result = self.run_steps(plan, input, output, ctx);
        if result.is_err() && !existed {
            let _ = fs::remove_file(output); // don't leave half-written files behind
        }
        result
    }

    fn run_steps(&self, plan: &Plan, input: &Path, output: &Path, ctx: &Context) -> Result<()> {
        let n = plan.steps.len();
        let tmp = if n > 1 { Some(tempfile::tempdir()?) } else { None };
        let mut current: PathBuf = input.to_path_buf();

        for (i, step) in plan.steps.iter().enumerate() {
            ctx.cancel.check()?;
            let out = if i + 1 == n {
                output.to_path_buf()
            } else {
                tmp.as_ref()
                    .expect("tempdir exists for multi-step plans")
                    .path()
                    .join(format!("step{}.{}", i, step.to.ext()))
            };
            if let Some(dir) = out.parent() {
                if !dir.as_os_str().is_empty() {
                    fs::create_dir_all(dir)?;
                }
            }
            let sub = ctx.scaled(i as f32 / n as f32, 1.0 / n as f32);
            let job = Job { input: &current, output: &out, from: step.from, to: step.to };
            self.converters[step.converter].convert(&job, &sub)?;
            current = out;
        }
        ctx.report(1.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::Cost;

    /// Copies the file; only exists to exercise planning and chaining.
    struct Copy(Vec<Edge>);
    impl Converter for Copy {
        fn name(&self) -> &'static str {
            "copy"
        }
        fn edges(&self) -> Vec<Edge> {
            self.0.clone()
        }
        fn convert(&self, job: &Job<'_>, _ctx: &Context) -> Result<()> {
            fs::copy(job.input, job.output)?;
            Ok(())
        }
    }

    fn e(from: Format, to: Format, cost: Cost) -> Edge {
        Edge::new(from, to, cost)
    }

    #[test]
    fn chains_through_intermediate_formats() {
        use Format::*;
        let mut r = Registry::new();
        r.register(Box::new(Copy(vec![
            e(Yaml, Json, Cost::lossless(5)),
            e(Json, Csv, Cost::lossless(5)),
        ])));
        let plan = r.plan(Yaml, Csv).unwrap();
        assert_eq!(plan.describe(), "YAML -> JSON -> CSV");
        assert!(r.plan(Csv, Yaml).is_none());
        assert_eq!(r.reachable_from(Yaml), vec![Json, Csv]);
    }

    #[test]
    fn prefers_lossless_route_over_short_lossy_one() {
        use Format::*;
        let mut r = Registry::new();
        r.register(Box::new(Copy(vec![
            e(Png, Jpeg, Cost::lossy(10)),
            e(Png, Bmp, Cost::lossless(10)),
            e(Bmp, Tiff, Cost::lossless(10)),
            e(Tiff, Jpeg, Cost::lossless(10)),
        ])));
        // direct = 110, via BMP/TIFF = 30
        let plan = r.plan(Png, Jpeg).unwrap();
        assert_eq!(plan.len(), 3);
    }

    #[test]
    fn family_change_is_only_allowed_as_a_direct_step() {
        use Format::*;
        let mut r = Registry::new();
        r.register(Box::new(Copy(vec![
            e(Mp4, Gif, Cost::lossy(80)),
            e(Gif, Png, Cost::lossless(10)),
            e(Png, Gif, Cost::lossless(10)),
            e(Gif, WebM, Cost::lossy(50)),
        ])));
        assert!(r.plan(Mp4, Gif).is_some());
        assert!(r.plan(Mp4, Png).is_none()); // would cross video -> image mid-route
        assert!(r.plan(Png, WebM).is_none()); // would cross image -> video after a hop
        assert_eq!(r.plan(Gif, WebM).unwrap().describe(), "GIF -> WebM");
        assert_eq!(r.reachable_from(Mp4), vec![Gif]);
    }

    #[test]
    fn runs_multi_step_plan_and_reports_progress() {
        use std::sync::{Arc, Mutex};
        use Format::*;
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("a.yaml");
        let output = dir.path().join("sub").join("a.csv");
        fs::write(&input, "hello").unwrap();

        let mut r = Registry::new();
        r.register(Box::new(Copy(vec![
            e(Yaml, Json, Cost::lossless(5)),
            e(Json, Csv, Cost::lossless(5)),
        ])));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let ctx = Context::default().with_progress(move |p| s2.lock().unwrap().push(p));
        r.convert(&input, &output, Yaml, Csv, &ctx).unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "hello");
        assert_eq!(*seen.lock().unwrap().last().unwrap(), 1.0);
    }

    #[test]
    fn batch_runs_everything_and_reports_failures() {
        use std::{collections::HashMap, sync::Mutex};
        use Format::*;
        let dir = tempfile::tempdir().unwrap();
        let mut r = Registry::new();
        r.register(Box::new(Copy(vec![e(Json, Yaml, Cost::lossless(5))])));

        let mut items = Vec::new();
        for i in 0..40 {
            let input = dir.path().join(format!("f{i}.json"));
            fs::write(&input, format!("{i}")).unwrap();
            items.push(BatchItem::new(input, dir.path().join(format!("f{i}.yaml")), Json, Yaml));
        }
        // one impossible job
        items.push(BatchItem::new(dir.path().join("x.json"), dir.path().join("x.csv"), Json, Csv));

        let done = Mutex::new(HashMap::new());
        r.convert_batch(&items, &Options::default(), &CancelToken::new(), |i, ev| match ev {
            BatchEvent::Started | BatchEvent::Progress(_) => {}
            BatchEvent::Done => {
                done.lock().unwrap().insert(i, true);
            }
            BatchEvent::Failed(_) => {
                done.lock().unwrap().insert(i, false);
            }
        });
        let done = done.into_inner().unwrap();
        assert_eq!(done.len(), 41);
        assert!((0..40).all(|i| done[&i]));
        assert!(!done[&40]);
        for i in 0..40 {
            assert_eq!(fs::read_to_string(dir.path().join(format!("f{i}.yaml"))).unwrap(), i.to_string());
        }
    }

    #[test]
    fn a_cancelled_item_is_skipped_without_touching_the_others() {
        use std::sync::Mutex;
        use Format::*;
        let dir = tempfile::tempdir().unwrap();
        let mut r = Registry::new();
        r.register(Box::new(Copy(vec![e(Json, Yaml, Cost::lossless(5))])));
        let batch = CancelToken::new();
        let mut items = Vec::new();
        for i in 0..3 {
            let input = dir.path().join(format!("f{i}.json"));
            fs::write(&input, "x").unwrap();
            let mut it = BatchItem::new(input, dir.path().join(format!("f{i}.yaml")), Json, Yaml);
            it.cancel = Some(batch.child());
            items.push(it);
        }
        items[1].cancel.as_ref().unwrap().cancel();

        let log = Mutex::new(Vec::new());
        r.convert_batch(&items, &Options::default(), &batch, |i, ev| {
            let tag = match ev {
                BatchEvent::Done => "done",
                BatchEvent::Failed(Error::Cancelled) => "cancelled",
                BatchEvent::Failed(_) => "failed",
                _ => return,
            };
            log.lock().unwrap().push((i, tag));
        });
        let mut log = log.into_inner().unwrap();
        log.sort();
        assert_eq!(log, vec![(0, "done"), (1, "cancelled"), (2, "done")]);
        assert!(!dir.path().join("f1.yaml").exists());
    }

    #[test]
    fn cancelled_run_leaves_no_output() {
        use Format::*;
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("a.json");
        let output = dir.path().join("a.csv");
        fs::write(&input, "x").unwrap();
        let mut r = Registry::new();
        r.register(Box::new(Copy(vec![e(Json, Csv, Cost::lossless(5))])));
        let ctx = Context::default();
        ctx.cancel.cancel();
        assert!(matches!(r.convert(&input, &output, Json, Csv, &ctx), Err(Error::Cancelled)));
        assert!(!output.exists());
    }
}
