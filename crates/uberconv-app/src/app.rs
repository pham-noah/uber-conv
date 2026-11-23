//! Application state and behaviour (rendering lives in `views.rs`).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use gpui::{px, ClipboardItem, Context, FocusHandle, ListAlignment, ListState, ScrollHandle};
use uberconv_core::{BatchEvent, BatchItem, CancelToken, Context as CoreContext, Error, Family, Format, Options, Registry};

use crate::{
    dialogs, files,
    settings::{OutputMode, Settings},
};

pub enum State {
    Ready,
    /// In the batch, waiting for a free worker.
    Queued,
    Running(f32),
    Done { out: PathBuf, out_size: Option<u64> },
    Failed(String),
    Cancelled,
}

/// One output of one file. A file can be converted to several formats at once,
/// so each (file, format) pair carries its own progress and result; the item's
/// own `state` is what the row shows.
pub struct Part {
    pub target: Format,
    pub state: State,
}

pub struct Item {
    pub id: u64,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub from: Option<Format>,
    /// Every format this file can be converted to.
    pub targets: Vec<Format>,
    /// The formats selected for it, in click order. Clicking a second format
    /// adds an output rather than replacing the first.
    pub selected: Vec<Format>,
    /// Progress per selected format for the current (or last) run.
    pub parts: Vec<Part>,
    /// Aggregate of `parts`, for the row.
    pub state: State,
    pub token: Option<CancelToken>,
    pub cancelling: bool,
    pub expanded: bool,
    /// Per-file options. `None` means "use the settings".
    pub opts: Option<Options>,
    pub options_open: bool,
}

impl Item {
    pub fn in_flight(&self) -> bool {
        matches!(self.state, State::Queued | State::Running(_))
    }
    pub fn can_start(&self) -> bool {
        matches!(self.state, State::Ready | State::Failed(_) | State::Cancelled)
            && self.from.is_some()
            && !self.selected.is_empty()
    }
    /// The first chosen format — what the row's options panel configures.
    pub fn primary(&self) -> Option<Format> {
        self.selected.first().copied()
    }
    pub fn is_selected(&self, fmt: Format) -> bool {
        self.selected.contains(&fmt)
    }
    /// Fold the per-output states into the one the row shows.
    pub fn recompute(&mut self) {
        if self.parts.is_empty() {
            return;
        }
        let total = self.parts.len() as f32;
        let (mut sum, mut running, mut queued) = (0.0f32, false, false);
        let (mut done, mut cancelled) = (0usize, 0usize);
        let mut first_out: Option<PathBuf> = None;
        let mut out_size = 0u64;
        let mut size_known = false;
        let mut errors: Vec<String> = Vec::new();
        for part in &self.parts {
            match &part.state {
                State::Queued => queued = true,
                State::Running(p) => {
                    running = true;
                    sum += *p;
                }
                State::Done { out, out_size: s } => {
                    done += 1;
                    sum += 1.0;
                    first_out.get_or_insert_with(|| out.clone());
                    if let Some(s) = s {
                        out_size += s;
                        size_known = true;
                    }
                }
                State::Failed(m) => {
                    sum += 1.0;
                    errors.push(m.clone());
                }
                State::Cancelled => {
                    sum += 1.0;
                    cancelled += 1;
                }
                State::Ready => {}
            }
        }
        self.state = if running || queued {
            State::Running(sum / total)
        } else if let Some(first) = errors.first() {
            let msg = if errors.len() == 1 {
                first.clone()
            } else {
                format!("{} of {} outputs failed · {first}", errors.len(), self.parts.len())
            };
            State::Failed(msg)
        } else if done == self.parts.len() {
            State::Done { out: first_out.unwrap_or_default(), out_size: size_known.then_some(out_size) }
        } else if cancelled > 0 {
            State::Cancelled
        } else {
            State::Ready
        };
        if !matches!(self.state, State::Running(_)) {
            self.cancelling = false;
        }
    }
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }
}

/// Which options matter for an item, given its source and chosen target.
pub struct Relevant {
    pub quality: bool,
    pub size: bool,
    pub bitrate: bool,
    pub crf: bool,
    pub page: bool,
}

impl Relevant {
    pub fn of(item: &Item) -> Relevant {
        use Format::*;
        let to = item.primary();
        let to_image = to.is_some_and(|t| t.family() == uberconv_core::Family::Image);
        Relevant {
            quality: matches!(to, Some(Jpeg | WebP | Avif)),
            size: to_image,
            bitrate: matches!(to, Some(Mp3 | Ogg | Opus | M4a | Aac | Mp4 | Mkv | Mov | WebM | Avi)),
            crf: matches!(to, Some(Mp4 | Mkv | Mov | WebM | Avi)),
            page: item.from == Some(Pdf) && to_image,
        }
    }
    pub fn any(&self) -> bool {
        self.quality || self.size || self.bitrate || self.crf || self.page
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Type,
}

pub struct Notice {
    pub msg: String,
    pub seq: u64,
    /// Show an Undo button.
    pub undo: bool,
}

enum Event {
    Started(u64, Format),
    Progress(u64, Format, f32),
    Done(u64, Format, PathBuf),
    Failed(u64, Format, String),
    Cancelled(u64, Format),
}

pub struct ConverterApp {
    pub registry: Arc<Registry>,
    pub items: Vec<Item>,
    /// Virtualised list over `items`: only visible rows are built each frame.
    pub list: ListState,
    next_id: u64,
    pub busy: bool,
    batch_cancel: CancelToken,
    batch_ids: HashSet<u64>,
    batch_started: Option<Instant>,
    /// Result line of the last finished batch, and whether everything succeeded.
    pub summary: Option<(String, bool)>,
    pub ffmpeg: bool,
    pub settings: Settings,
    pub settings_open: bool,
    settings_closed_at: Option<Instant>,
    pub focus: FocusHandle,
    pub settings_scroll: ScrollHandle,
    pub notice: Option<Notice>,
    notice_seq: u64,
    /// Removed items, newest group last, for Undo.
    removed: Vec<Vec<(usize, Item)>>,
    pub sort: Option<(SortKey, bool)>,
    /// Folder a queued file was dropped in, so output can mirror the tree.
    roots: HashMap<PathBuf, PathBuf>,
    /// Row last clicked, for the keyboard shortcuts (Del, Ctrl+Shift+C, Space).
    pub picked: Option<u64>,
    last_size: Option<(f32, f32)>,
    last_saved: Instant,
}

impl ConverterApp {
    pub fn new(settings: Settings, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(900.));
        // The custom scrollbar thumb lives in the view, so repaint it on scroll.
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            let _ = weak.update(cx, |_, cx| cx.notify());
        });
        ConverterApp {
            registry: Arc::new(uberconv_formats::default_registry()),
            items: Vec::new(),
            list,
            next_id: 0,
            busy: false,
            batch_cancel: CancelToken::new(),
            batch_ids: HashSet::new(),
            batch_started: None,
            summary: None,
            ffmpeg: uberconv_formats::ffmpeg_available(),
            settings,
            settings_open: false,
            settings_closed_at: None,
            focus: cx.focus_handle(),
            settings_scroll: ScrollHandle::new(),
            notice: None,
            notice_seq: 0,
            removed: Vec::new(),
            sort: None,
            roots: HashMap::new(),
            picked: None,
            last_size: None,
            last_saved: Instant::now(),
        }
    }

    // ───────────── small helpers ─────────────

    /// Re-render shortly after the next layout pass, so scrollbar geometry that
    /// only exists once a frame has been laid out shows up.
    pub fn refresh_soon(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(60)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// Tell the list that the row at `ix` changed height (details opened, ...).
    fn remeasure(&self, ix: usize) {
        if ix < self.items.len() {
            self.list.splice(ix..ix + 1, 1);
        }
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    /// Show a short-lived message at the bottom of the window.
    pub fn flash(&mut self, msg: String, undo: bool, cx: &mut Context<Self>) {
        self.notice_seq += 1;
        let seq = self.notice_seq;
        self.notice = Some(Notice { msg, seq, undo });
        let lifetime = if undo { Duration::from_secs(8) } else { Duration::from_secs(4) };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(lifetime).await;
            let _ = this.update(cx, |this, cx| {
                if this.notice.as_ref().is_some_and(|n| n.seq == seq) {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Change a setting, persist it and repaint.
    pub fn tweak(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings);
        self.settings.save();
        cx.notify();
    }

    /// Called from `render`: remember the window size, saving at most once a second.
    pub fn track_window(&mut self, w: f32, h: f32) {
        if self.last_size != Some((w, h)) {
            self.last_size = Some((w, h));
            self.settings.window = Some((w, h));
            if self.last_saved.elapsed() > Duration::from_secs(1) {
                self.settings.save();
                self.last_saved = Instant::now();
            }
        }
    }

    // ───────────── settings panel ─────────────

    pub fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings_open = false;
        } else if self.settings_closed_at.is_none_or(|t| t.elapsed() > Duration::from_millis(250)) {
            // (A click on the gear while the panel is open first triggers the
            // panel's click-outside handler; don't immediately reopen it.)
            self.settings_open = true;
            self.refresh_soon(cx); // so the panel's scrollbar appears after first layout
        }
        cx.notify();
    }

    pub fn dismiss_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings_open = false;
            self.settings_closed_at = Some(Instant::now());
            cx.notify();
        }
    }

    pub fn choose_output_folder(&mut self, cx: &mut Context<Self>) {
        let start = self
            .settings
            .output_pick_dir
            .clone()
            .or_else(|| match &self.settings.output {
                OutputMode::Folder(dir) => Some(dir.clone()),
                OutputMode::NextToSource => self.settings.last_dir.clone(),
            })
            .or_else(dialogs::first_folder);
        let rx = dialogs::pick_folder(start);
        cx.spawn(async move |this, cx| {
            if let Ok(Some(dir)) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.tweak(cx, |s| {
                        s.output = OutputMode::Folder(dir.clone());
                        s.output_pick_dir = Some(dir);
                    })
                });
            }
        })
        .detach();
    }

    // ───────────── queue management ─────────────

    pub fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let remembered = paths.first().and_then(|p| dialogs::folder_of(p));
        let got = files::collect(paths);
        let known: HashSet<PathBuf> = self.items.iter().map(|i| i.path.clone()).collect();
        let mut targets_cache: HashMap<Format, Vec<Format>> = HashMap::new();
        let before = self.items.len();
        let mut dupes = 0usize;
        let mut seen = known;
        let truncated = got.truncated;
        let from_folders = got.from_folders;

        for (path, root) in got.files.iter().map(|p| (p.clone(), got.roots.get(p).cloned())) {
            if !seen.insert(path.clone()) {
                dupes += 1;
                continue;
            }
            if let Some(root) = root {
                self.roots.insert(path.clone(), root);
            }
            let from = Format::from_path(&path);
            let targets = from
                .map(|f| targets_cache.entry(f).or_insert_with(|| self.registry.reachable_from(f)).clone())
                .unwrap_or_default();
            // Default to a sibling format (PNG -> JPEG, MP4 -> MKV), else the first option.
            let target = from
                .and_then(|f| targets.iter().copied().find(|t| t.family() == f.family()))
                .or_else(|| targets.first().copied());
            let size = std::fs::metadata(&path).map(|m| m.len()).ok();
            self.items.push(Item {
                id: self.next_id,
                path,
                size,
                from,
                targets,
                selected: target.into_iter().collect(),
                parts: Vec::new(),
                state: State::Ready,
                token: None,
                cancelling: false,
                expanded: false,
                opts: None,
                options_open: false,
            });
            self.next_id += 1;
        }
        let added = self.items.len() - before;
        if added > 0 {
            self.list.splice(before..before, added);
            self.summary = None;
        }
        let mut msg = format!("Added {added} file{}", if added == 1 { "" } else { "s" });
        if got.skipped > 0 {
            msg.push_str(&format!(" · skipped {} unsupported", got.skipped));
        }
        if dupes > 0 {
            msg.push_str(&format!(" · {dupes} already in the queue"));
        }
        if truncated {
            msg.push_str(&format!(" · stopped at {} files, the rest of the folder was not read", files::MAX_FILES));
        }
        if from_folders > 0 || got.skipped > 0 || dupes > 0 || truncated {
            self.flash(msg, false, cx);
        }
        // Remember where these came from, so the next picker opens there.
        if let Some(dir) = remembered {
            if self.settings.last_dir.as_ref() != Some(&dir) {
                self.settings.last_dir = Some(dir);
                self.settings.save();
            }
        }
        self.refresh_soon(cx);
        cx.notify();
    }

    pub fn open_dialog(&mut self, cx: &mut Context<Self>) {
        // Open where the user last picked from, so a session of converting one
        // folder's files does not start at the desktop every time.
        let start = self.settings.last_dir.clone().or_else(dialogs::first_folder);
        let rx = dialogs::pick_files(start);
        cx.spawn(async move |this, cx| {
            if let Ok(Some(paths)) = rx.recv().await {
                let _ = this.update(cx, |this, cx| this.add_paths(paths, cx));
            }
        })
        .detach();
    }

    /// Click a format chip: add it as another output, or drop it if it is
    /// already selected. A file must keep at least one output selected unless
    /// nothing is left, which the row shows as "choose a format".
    pub fn toggle_target(&mut self, id: u64, fmt: Format, cx: &mut Context<Self>) {
        let Some(ix) = self.index_of(id) else { return };
        if self.items[ix].in_flight() {
            return;
        }
        {
            let item = &mut self.items[ix];
            if let Some(at) = item.selected.iter().position(|t| *t == fmt) {
                item.selected.remove(at);
            } else {
                item.selected.push(fmt);
            }
            item.state = State::Ready;
            // a rerun has to forget the previous results
            item.parts.clear();
        }
        self.remeasure(ix);
        cx.notify();
    }

    /// Apply `fmt` to every file that can be converted to it (one output each).
    pub fn set_all(&mut self, fmt: Format, cx: &mut Context<Self>) {
        let mut changed = Vec::new();
        for (ix, item) in self.items.iter_mut().enumerate().filter(|(_, i)| !i.in_flight()) {
            if item.targets.contains(&fmt) {
                item.selected = vec![fmt];
                item.parts.clear();
                item.state = State::Ready;
                changed.push(ix);
            }
        }
        for ix in changed {
            self.remeasure(ix);
        }
        cx.notify();
    }

    // ───────────── presets ─────────────

    /// Preset chips: the three built-ins, then anything the user saved.
    pub fn presets(&self) -> Vec<crate::settings::Preset> {
        let mut all: Vec<_> = crate::settings::builtin_presets().to_vec();
        all.extend(self.settings.presets.iter().cloned());
        all
    }

    pub fn apply_preset(&mut self, name: String, cx: &mut Context<Self>) {
        let p = self.presets().into_iter().find(|p| p.name == name);
        if let Some(p) = p {
            self.tweak(cx, |s| {
                s.image_quality = p.image_quality;
                s.image_max_dimension = p.image_max_dimension;
                s.audio_bitrate_kbps = p.audio_bitrate_kbps;
                s.video_crf = p.video_crf;
            });
            self.flash(format!("Preset “{}” applied · {}", p.name, p.describe()), false, cx);
        }
    }

    /// Save the current numbers as a chip, so a hand-tuned combo is one click
    /// away next time.
    pub fn save_preset(&mut self, cx: &mut Context<Self>) {
        let p = crate::settings::Preset {
            name: format!("{} {}", self.next_preset_number(), self.settings.options().image_max_dimension.map(|d| format!("{d}px")).unwrap_or_else(|| "full".into())),
            image_quality: self.settings.image_quality,
            image_max_dimension: self.settings.image_max_dimension,
            audio_bitrate_kbps: self.settings.audio_bitrate_kbps,
            video_crf: self.settings.video_crf,
        };
        let name = p.name.clone();
        self.tweak(cx, |s| s.presets.push(p));
        self.flash(format!("Saved preset “{name}”"), false, cx);
    }

    fn next_preset_number(&self) -> String {
        format!("Custom {}", self.settings.presets.len() + 1)
    }

    pub fn remove_preset(&mut self, name: String, cx: &mut Context<Self>) {
        self.tweak(cx, |s| s.presets.retain(|p| p.name != name));
        cx.notify();
    }

    /// Remove one file; Undo (Ctrl+Z) brings it back.
    pub fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.index_of(id) else { return };
        if self.items[ix].in_flight() {
            return;
        }
        let item = self.items.remove(ix);
        self.list.splice(ix..ix + 1, 0);
        let name = item.name();
        self.push_removed(vec![(ix, item)]);
        self.flash(format!("Removed {name}"), true, cx);
        self.refresh_soon(cx);
    }

    fn push_removed(&mut self, group: Vec<(usize, Item)>) {
        self.removed.push(group);
        if self.removed.len() > 20 {
            self.removed.remove(0);
        }
    }

    pub fn clear_finished(&mut self, cx: &mut Context<Self>) {
        let mut kept = Vec::new();
        let mut group = Vec::new();
        for (ix, item) in std::mem::take(&mut self.items).into_iter().enumerate() {
            if matches!(item.state, State::Done { .. } | State::Cancelled) {
                group.push((ix, item));
            } else {
                kept.push(item);
            }
        }
        self.items = kept;
        self.list.reset(self.items.len());
        self.summary = None;
        let n = group.len();
        if n > 0 {
            self.push_removed(group);
            self.flash(format!("Cleared {n} file{}", if n == 1 { "" } else { "s" }), true, cx);
        }
        self.refresh_soon(cx);
        cx.notify();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(group) = self.removed.pop() else { return };
        let n = group.len();
        // Reinserting in ascending order rebuilds the original positions.
        for (ix, item) in group {
            let at = ix.min(self.items.len());
            self.items.insert(at, item);
        }
        self.list.reset(self.items.len());
        self.flash(format!("Restored {n} file{}", if n == 1 { "" } else { "s" }), false, cx);
        self.refresh_soon(cx);
    }

    /// Drag-and-drop reordering: put `dragged` where `target` is.
    pub fn move_item(&mut self, dragged: u64, target: u64, cx: &mut Context<Self>) {
        let (Some(from), Some(to)) = (self.index_of(dragged), self.index_of(target)) else { return };
        if from == to {
            return;
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
        self.sort = None;
        let (lo, hi) = (from.min(to), from.max(to));
        self.list.splice(lo..hi + 1, hi + 1 - lo);
        cx.notify();
    }

    /// Sort the queue; choosing the same key again flips the direction.
    pub fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        let descending = matches!(self.sort, Some((k, false)) if k == key);
        self.items.sort_by(|a, b| {
            let ord = match key {
                SortKey::Name => a.name().to_lowercase().cmp(&b.name().to_lowercase()),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Type => (a.from.map(|f| (f.family(), f.name())), a.name().to_lowercase())
                    .cmp(&(b.from.map(|f| (f.family(), f.name())), b.name().to_lowercase())),
            };
            if descending { ord.reverse() } else { ord }
        });
        self.sort = Some((key, descending));
        self.list.reset(self.items.len());
        cx.notify();
    }

    pub fn has_finished(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.state, State::Done { .. } | State::Cancelled))
    }

    pub fn reveal(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(Item { state: State::Done { out, .. }, .. }) = self.items.iter().find(|i| i.id == id) {
            cx.reveal_path(out);
        }
    }

    pub fn toggle_details(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(ix) = self.index_of(id) {
            self.items[ix].expanded = !self.items[ix].expanded;
            self.remeasure(ix);
            cx.notify();
        }
    }

    pub fn copy_error(&mut self, id: u64, cx: &mut Context<Self>) {
        let msg = self.items.iter().find(|i| i.id == id).and_then(|i| match &i.state {
            State::Failed(m) => Some(m.clone()),
            _ => None,
        });
        if let Some(m) = msg {
            cx.write_to_clipboard(ClipboardItem::new_string(m));
            self.flash("Error copied to clipboard".into(), false, cx);
        }
    }

    // ───────────── per-file options ─────────────

    pub fn toggle_options(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(ix) = self.index_of(id) {
            self.items[ix].options_open = !self.items[ix].options_open;
            self.remeasure(ix);
            cx.notify();
        }
    }

    /// Change one option for one file (starting from the current settings).
    pub fn tweak_item(&mut self, id: u64, cx: &mut Context<Self>, f: impl FnOnce(&mut Options)) {
        let global = self.settings.options();
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            if item.in_flight() {
                return;
            }
            let mut o = item.opts.clone().unwrap_or(global);
            f(&mut o);
            item.opts = Some(o);
            item.state = State::Ready;
            cx.notify();
        }
    }

    pub fn reset_item_options(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.opts = None;
            cx.notify();
        }
    }

    // ───────────── progress ─────────────

    pub fn convertible(&self) -> usize {
        self.items.iter().filter(|i| i.can_start()).count()
    }

    /// (files finished, files in the batch, overall fraction 0..1)
    pub fn batch_progress(&self) -> (usize, usize, f32) {
        let (mut finished, mut total, mut sum) = (0, 0, 0.0f32);
        for item in self.items.iter().filter(|i| self.batch_ids.contains(&i.id)) {
            total += 1;
            match &item.state {
                State::Done { .. } | State::Failed(_) | State::Cancelled => {
                    finished += 1;
                    sum += 1.0;
                }
                State::Running(p) => sum += p,
                State::Ready | State::Queued => {}
            }
        }
        let fraction = if total == 0 { 0.0 } else { sum / total as f32 };
        (finished, total, fraction)
    }

    /// Rough time left for the running batch, once there's enough data.
    pub fn eta(&self) -> Option<Duration> {
        let elapsed = self.batch_started?.elapsed();
        let (_, _, fraction) = self.batch_progress();
        if elapsed < Duration::from_secs(3) || !(0.05..0.995).contains(&fraction) {
            return None;
        }
        Some(Duration::from_secs_f32(elapsed.as_secs_f32() * (1.0 - fraction) / fraction))
    }

    // ───────────── cancel / retry ─────────────

    pub fn cancel_item(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            if let Some(t) = &item.token {
                t.cancel();
            }
            for part in item.parts.iter_mut() {
                if matches!(part.state, State::Queued) {
                    part.state = State::Cancelled;
                }
            }
            item.cancelling = item.parts.iter().any(|p| matches!(p.state, State::Running(_)));
            item.recompute();
            cx.notify();
        }
    }

    pub fn cancel_all(&mut self, cx: &mut Context<Self>) {
        if !self.busy {
            return;
        }
        self.batch_cancel.cancel();
        for item in self.items.iter_mut() {
            for part in item.parts.iter_mut() {
                if matches!(part.state, State::Queued) {
                    part.state = State::Cancelled;
                }
            }
            item.cancelling = item.parts.iter().any(|p| matches!(p.state, State::Running(_)));
            item.recompute();
        }
        cx.notify();
    }

    pub fn retry(&mut self, id: u64, cx: &mut Context<Self>) {
        self.start(Some(id), cx);
    }

    // ───────────── running conversions ─────────────

    pub fn start(&mut self, only: Option<u64>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let batch_cancel = CancelToken::new();
        /// (item id, input, from, to, cancel token, per-file options)
        type Job = (u64, PathBuf, Format, Format, CancelToken, Option<Options>);
        let mut jobs: Vec<Job> = Vec::new();
        let mut collapsed = Vec::new();
        for (ix, item) in self.items.iter_mut().enumerate() {
            if only.is_some_and(|id| id != item.id) || !item.can_start() {
                continue;
            }
            let Some(from) = item.from else { continue };
            // Everything selected runs, except outputs that already came out of
            // this item — a retry only redoes what failed.
            let targets: Vec<Format> = item
                .selected
                .iter()
                .copied()
                .filter(|t| !item.parts.iter().any(|p| p.target == *t && matches!(p.state, State::Done { .. })))
                .collect();
            if targets.is_empty() {
                continue;
            }
            let token = batch_cancel.child();
            item.token = Some(token.clone());
            item.cancelling = false;
            if item.expanded {
                item.expanded = false;
                collapsed.push(ix);
            }
            item.parts.retain(|p| !targets.contains(&p.target));
            for t in &targets {
                item.parts.push(Part { target: *t, state: State::Queued });
                jobs.push((item.id, item.path.clone(), from, *t, token.clone(), item.opts.clone()));
            }
            item.recompute();
        }
        for ix in collapsed {
            self.remeasure(ix);
        }
        if jobs.is_empty() {
            return;
        }

        self.busy = true;
        self.summary = None;
        self.batch_cancel = batch_cancel.clone();
        self.batch_ids = jobs.iter().map(|j| j.0).collect();
        self.batch_started = Some(Instant::now());
        spawn_batch(
            jobs,
            self.registry.clone(),
            self.settings.options(),
            self.settings.output.clone(),
            self.settings.overwrite,
            self.settings.recycle_replaced,
            self.roots.clone(),
            batch_cancel,
            cx,
        );
        cx.notify();
    }

    /// "Retry 3 failed": rerun just the items that ended in an error.
    pub fn retry_failed(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let failed: Vec<u64> =
            self.items.iter().filter(|i| matches!(i.state, State::Failed(_))).map(|i| i.id).collect();
        if failed.is_empty() {
            return;
        }
        self.start_subset(&failed, cx);
    }

    /// Start the given items as one batch.
    fn start_subset(&mut self, wanted: &[u64], cx: &mut Context<Self>) {
        if self.busy || wanted.is_empty() {
            return;
        }
        let batch_cancel = CancelToken::new();
        type Job = (u64, PathBuf, Format, Format, CancelToken, Option<Options>);
        let mut jobs: Vec<Job> = Vec::new();
        for item in self.items.iter_mut().filter(|i| wanted.contains(&i.id) && i.can_start()) {
            let Some(from) = item.from else { continue };
            let targets: Vec<Format> = item
                .selected
                .iter()
                .copied()
                .filter(|t| !item.parts.iter().any(|p| p.target == *t && matches!(p.state, State::Done { .. })))
                .collect();
            if targets.is_empty() {
                continue;
            }
            let token = batch_cancel.child();
            item.token = Some(token.clone());
            item.cancelling = false;
            item.parts.retain(|p| !targets.contains(&p.target));
            for t in &targets {
                item.parts.push(Part { target: *t, state: State::Queued });
                jobs.push((item.id, item.path.clone(), from, *t, token.clone(), item.opts.clone()));
            }
            item.recompute();
            item.expanded = false;
        }
        if jobs.is_empty() {
            return;
        }
        self.busy = true;
        self.summary = None;
        self.batch_cancel = batch_cancel.clone();
        self.batch_ids = jobs.iter().map(|j| j.0).collect();
        self.batch_started = Some(Instant::now());
        spawn_batch(
            jobs,
            self.registry.clone(),
            self.settings.options(),
            self.settings.output.clone(),
            self.settings.overwrite,
            self.settings.recycle_replaced,
            self.roots.clone(),
            batch_cancel,
            cx,
        );
        cx.notify();
    }

    fn apply(&mut self, ev: Event) {
        let (id, to) = match &ev {
            Event::Started(id, to)
            | Event::Progress(id, to, _)
            | Event::Done(id, to, _)
            | Event::Failed(id, to, _)
            | Event::Cancelled(id, to) => (*id, *to),
        };
        let Some(item) = self.items.iter_mut().find(|i| i.id == id) else { return };
        let Some(part) = item.parts.iter_mut().find(|p| p.target == to) else { return };
        match ev {
            Event::Started(..) => {
                if matches!(part.state, State::Queued) {
                    part.state = State::Running(0.0);
                }
            }
            Event::Progress(.., p) => {
                if matches!(part.state, State::Queued | State::Running(_)) {
                    part.state = State::Running(p);
                }
            }
            Event::Done(_, _, out) => {
                let out_size = std::fs::metadata(&out).map(|m| m.len()).ok();
                part.state = State::Done { out, out_size };
            }
            Event::Failed(_, _, msg) => part.state = State::Failed(msg),
            Event::Cancelled(..) => part.state = State::Cancelled,
        }
        item.recompute();
    }

    fn finish_batch(&mut self, cx: &mut Context<Self>) {
        self.busy = false;
        let elapsed = self.batch_started.take().map(|t| t.elapsed()).unwrap_or_default();
        let (mut ok, mut failed, mut cancelled) = (0usize, 0usize, 0usize);
        let (mut before, mut after) = (0u64, 0u64);
        for item in self.items.iter_mut() {
            item.token = None;
            if !self.batch_ids.contains(&item.id) {
                continue;
            }
            for part in &item.parts {
                match &part.state {
                    State::Done { out_size, .. } => {
                        ok += 1;
                        if let Some(a) = out_size {
                            after += *a;
                        }
                    }
                    State::Failed(_) => failed += 1,
                    State::Cancelled => cancelled += 1,
                    _ => {}
                }
            }
            if let Some(b) = item.size {
                before += b;
            }
        }

        let mut text = if ok > 0 {
            format!("{ok} file{} converted in {}", if ok == 1 { "" } else { "s" }, format_duration(elapsed))
        } else {
            format!("Nothing converted ({})", format_duration(elapsed))
        };
        if before > 0 && ok > 0 {
            if after < before {
                text.push_str(&format!(" · saved {}", files::human_size(before - after)));
            } else if after > before {
                text.push_str(&format!(" · {} larger", files::human_size(after - before)));
            }
        }
        if failed > 0 {
            text.push_str(&format!(" · {failed} failed"));
        }
        if cancelled > 0 {
            text.push_str(&format!(" · {cancelled} cancelled"));
        }
        if self.settings.notify && elapsed >= Duration::from_secs(8) && ok + failed > 0 {
            // Toast for the history, taskbar flash for the moment: banners can be
            // switched off, a blinking taskbar button cannot.
            files::flash_taskbar();
            files::notify("uberconv: conversion finished", &text);
        }
        self.summary = Some((text, failed == 0));
        cx.notify();
    }

    // ───────────── row selection and bulk actions ─────────────

    pub fn pick(&mut self, id: u64, cx: &mut Context<Self>) {
        self.picked = Some(id);
        cx.notify();
    }

    /// Delete the row the keyboard is on.
    pub fn remove_picked(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.picked {
            self.remove(id, cx);
        }
    }

    /// Space: open/close the picked row's options.
    pub fn toggle_picked_options(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.picked {
            self.toggle_options(id, cx);
        }
    }

    /// Ctrl+Shift+C: every error in the queue, one per line, ready to paste.
    pub fn copy_all_errors(&mut self, cx: &mut Context<Self>) {
        let mut lines = Vec::new();
        for item in &self.items {
            if let State::Failed(msg) = &item.state {
                lines.push(format!("{}: {msg}", item.path.display()));
            }
        }
        if lines.is_empty() {
            self.flash("No errors to copy".into(), false, cx);
            return;
        }
        let n = lines.len();
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        self.flash(format!("Copied {n} error{}", if n == 1 { "" } else { "s" }), false, cx);
    }

    /// "Combine into PDF": every image in the queue becomes one page, in queue order.
    pub fn combine_into_pdf(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let images: Vec<PathBuf> = self
            .items
            .iter()
            .filter(|i| i.from.is_some_and(|f| f.family() == Family::Image))
            .map(|i| i.path.clone())
            .collect();
        if images.len() < 2 {
            self.flash("Add at least two images to combine".into(), false, cx);
            return;
        }
        let start = match &self.settings.output {
            OutputMode::Folder(dir) => Some(dir.clone()),
            OutputMode::NextToSource => None,
        }
        .or_else(|| self.settings.last_dir.clone())
        .or_else(dialogs::first_folder);
        let suggested = format!(
            "{}.pdf",
            images
                .first()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "combined".into())
        );
        let options = self.settings.options();
        let rx = dialogs::save_pdf(start, &suggested);
        cx.spawn(async move |this, cx| {
            let Ok(Some(out)) = rx.recv().await else { return };
            // Merging decodes and re-encodes every page: keep it off the UI thread.
            let (tx, back) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let ctx = CoreContext::new(options);
                let res = uberconv_formats::pdf::images_to_pdf(&images, &out, &ctx);
                let _ = tx.send_blocking((res.map_err(|e| e.to_string()), out));
            });
            let Ok((res, out)) = back.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => this.flash(format!("Wrote {}", out.display()), false, cx),
                    Err(e) => this.flash(format!("Couldn't combine: {e}"), false, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open the folder the last batch wrote into.
    pub fn open_output_folder(&mut self, cx: &mut Context<Self>) {
        let folder = match &self.settings.output {
            OutputMode::Folder(dir) => Some(dir.clone()),
            OutputMode::NextToSource => self
                .items
                .iter()
                .find_map(|i| match &i.state {
                    State::Done { out, .. } => out.parent().map(|p| p.to_path_buf()),
                    _ => None,
                })
                .or_else(|| self.settings.last_dir.clone()),
        };
        match folder {
            Some(dir) => cx.reveal_path(&dir),
            None => self.flash("No output folder yet".into(), false, cx),
        }
    }
}

/// Run a batch on a worker thread and stream its events back to the entity.
fn spawn_batch(
    jobs: Vec<(u64, PathBuf, Format, Format, CancelToken, Option<Options>)>,
    registry: Arc<Registry>,
    options: Options,
    mode: OutputMode,
    overwrite: bool,
    recycle: bool,
    roots: HashMap<PathBuf, PathBuf>,
    batch_cancel: CancelToken,
    cx: &mut Context<ConverterApp>,
) {
    let (tx, rx) = async_channel::unbounded::<Event>();
    thread::spawn(move || {
        let mut claimed = HashSet::new();
        let ids: Vec<u64> = jobs.iter().map(|j| j.0).collect();
        let formats: Vec<Format> = jobs.iter().map(|j| j.3).collect();
        let items: Vec<BatchItem> = jobs
            .into_iter()
            .map(|(_, input, from, to, token, opts)| {
                let root = roots.get(&input);
                let output = files::output_path(&input, to, &mode, overwrite, &mut claimed, root.map(|p| p.as_path()));
                let mut item = BatchItem::new(input, output, from, to);
                item.cancel = Some(token);
                item.options = opts;
                item
            })
            .collect();
        if overwrite && recycle {
            for item in &items {
                files::retire(&item.output);
            }
        }
        for item in &items {
            files::ensure_parent(&item.output);
        }
        registry.convert_batch(&items, &options, &batch_cancel, |i, ev| {
            let (id, to) = (ids[i], formats[i]);
            let event = match ev {
                BatchEvent::Started => Event::Started(id, to),
                BatchEvent::Progress(p) => Event::Progress(id, to, p),
                BatchEvent::Done => Event::Done(id, to, items[i].output.clone()),
                BatchEvent::Failed(Error::Cancelled) => Event::Cancelled(id, to),
                BatchEvent::Failed(e) => Event::Failed(id, to, e.to_string()),
            };
            let _ = tx.send_blocking(event);
        });
    });
    cx.spawn(async move |this, cx| {
        while let Ok(ev) = rx.recv().await {
            let _ = this.update(cx, |this, cx| {
                this.apply(ev);
                cx.notify();
            });
        }
        let _ = this.update(cx, |this, cx| this.finish_batch(cx));
    })
    .detach();
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f32();
    if secs < 60.0 {
        format!("{secs:.1} s")
    } else {
        format!("{}m {:02}s", (secs / 60.0) as u32, (secs % 60.0) as u32)
    }
}
