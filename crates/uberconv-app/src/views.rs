//! Rendering.

use std::collections::BTreeMap;

use gpui::{
    div, linear_color_stop, linear_gradient, list, prelude::*, px, relative, rgb, AnyElement, Context,
    ExternalPaths, FontWeight, ListState, MouseButton, ScrollHandle, SharedString, Window, WindowAppearance,
};
use uberconv_core::{Family, Format};

use crate::{
    actions::*,
    app::{ConverterApp, Item, Relevant, SortKey, State},
    assets::FONT_FAMILY,
    files,
    settings::{OutputMode, ThemeChoice},
    theme as th,
    ui::*,
};

impl ConverterApp {
    // ───────────────────────── header ─────────────────────────

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (dot, label, hint) = if self.ffmpeg {
            (th::good(), "FFmpeg ready", "FFmpeg found: audio and video conversion is available.")
        } else {
            (
                th::bad(),
                "FFmpeg missing",
                "Audio and video need FFmpeg. Put the ffmpeg binary next to the app, or set UBERCONV_FFMPEG.",
            )
        };
        let open = self.settings_open;
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(logo_badge(44.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_xl().font_weight(FontWeight::BOLD).child("uberconv"))
                    .child(div().text_xs().text_color(th::muted()).child("Convert anything to anything")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("ffmpeg-pill")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(12.))
                    .py(px(6.))
                    .rounded_full()
                    .bg(th::wash())
                    .border_1()
                    .border_color(th::line())
                    .text_xs()
                    .text_color(th::muted())
                    .tooltip(tip(hint))
                    .child(div().size(px(7.)).rounded_full().bg(dot))
                    .child(label),
            )
            .child(
                icon_button("settings-btn", "icons/settings.svg", format!("Settings  ({}, )", mod_key()))
                    .when(open, |b| b.bg(th::wash_hi()))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
            )
    }

    // ───────────────────────── drop zone ─────────────────────────

    fn render_drop_zone(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let badge = if compact { 36. } else { 56. };
        // Outer 1px gradient ring + inner panel = gradient border.
        div()
            .id("drop-zone")
            .tab_index(0)
            .p(px(1.))
            .rounded(px(20.))
            .cursor_pointer()
            .bg(linear_gradient(
                135.,
                linear_color_stop(th::with_alpha(th::VIOLET, 0x66), 0.),
                linear_color_stop(th::with_alpha(th::CYAN, 0x33), 1.),
            ))
            .focus(|s| s.bg(th::brand(135.)))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(th::brand(135.)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_paths(paths.paths().to_vec(), cx);
            }))
            .on_click(cx.listener(|this, _, _, cx| this.open_dialog(cx)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .rounded(px(19.))
                    .bg(th::panel())
                    .when(compact, |d| d.flex_row().py(px(14.)))
                    .when(!compact, |d| d.flex_col().py(px(30.)))
                    .child(
                        div()
                            .size(px(badge))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(th::brand(135.))
                            .shadow_md()
                            .child(icon("icons/upload.svg", badge * 0.46, rgb(0xffffff))),
                    )
                    .when(compact, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Drop more files or folders, or click to browse"),
                        )
                    })
                    .when(!compact, |d| {
                        d.child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Drop files or folders to convert"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(th::muted())
                                    .child(format!("or click to browse · {}O", mod_key())),
                            )
                    }),
            )
    }

    // ───────────────────────── empty state ─────────────────────────

    fn render_capabilities(&self) -> impl IntoElement {
        let cards: Vec<AnyElement> = [Family::Image, Family::Audio, Family::Video, Family::Data, Family::Text, Family::Archive]
            .into_iter()
            .map(|fam| {
                let color = th::family_color(fam);
                let needs_ffmpeg = matches!(fam, Family::Audio | Family::Video) && !self.ffmpeg;
                let names: Vec<&str> =
                    Format::ALL.iter().filter(|f| f.family() == fam).map(|f| f.name()).collect();
                div()
                    .flex_1()
                    .min_w(px(150.))
                    .max_w(px(320.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_xl()
                    .bg(th::panel())
                    .border_1()
                    .border_color(th::line())
                    .when(needs_ffmpeg, |d| d.opacity(0.6))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon_tile(th::family_icon(fam), color, 30.))
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(fam.name())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if needs_ffmpeg { th::bad() } else { th::muted() })
                            .child(SharedString::from(if needs_ffmpeg {
                                "Needs FFmpeg".to_string()
                            } else {
                                names.join(" · ")
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let k = mod_key();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(section_label("WHAT IT CAN CONVERT"))
            .child(div().flex().flex_wrap().gap_3().children(cards))
            .child(
                div()
                    .text_xs()
                    .text_color(th::faint())
                    .child(format!("{k}O open files   ·   {k}Enter convert all   ·   {k}Z undo   ·   {k}, settings   ·   Tab to move around")),
            )
    }

    // ───────────────────────── bulk bar ─────────────────────────

    /// "Set all to…" (formats that at least two files can become) and sorting.
    fn render_bulk_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.items.len() < 2 {
            return None;
        }
        let busy = self.busy;
        // While converting the bar stays (dimmed, inert) so the list below doesn't jump.
        let eligible: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| (busy || !i.in_flight()) && i.from.is_some() && !i.targets.is_empty())
            .collect();
        let mut counts: BTreeMap<Format, usize> = BTreeMap::new();
        for item in &eligible {
            for &t in &item.targets {
                *counts.entry(t).or_default() += 1;
            }
        }
        // Only formats that apply to at least two files; "all" with one is noise.
        let mut ranked: Vec<(Format, usize)> = counts.into_iter().filter(|(_, n)| *n >= 2).collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ranked.truncate(12);
        let total = eligible.len();

        let chips: Vec<AnyElement> = ranked
            .into_iter()
            .map(|(fmt, n)| {
                let all_selected = eligible.iter().all(|i| i.is_selected(fmt));
                format_chip(SharedString::from(format!("bulk-{}", fmt.ext())), fmt, all_selected)
                    .tooltip(tip(format!(
                        "Convert {n} of {total} file{} to {}",
                        if n == 1 { "" } else { "s" },
                        fmt.name()
                    )))
                    .when(!busy, |c| c.on_click(cx.listener(move |this, _, _, cx| this.set_all(fmt, cx))))
                    .into_any_element()
            })
            .collect();
        let has_chips = !chips.is_empty();

        let sort_button = |key: SortKey, label: &'static str, hint: &'static str| {
            let active = self.sort.filter(|(k, _)| *k == key);
            let text = match active {
                Some((_, false)) => format!("{label} ↑"),
                Some((_, true)) => format!("{label} ↓"),
                None => label.to_string(),
            };
            text_button(SharedString::from(format!("sort-{label}")), text, None, hint)
                .when(active.is_some(), |b| b.text_color(th::text()).bg(th::wash_hi()))
                .on_click(cx.listener(move |this, _, _, cx| this.sort_by(key, cx)))
        };

        Some(
            div()
                .flex()
                .items_start()
                .gap_3()
                .px_3()
                .py(px(10.))
                .rounded_xl()
                .bg(th::wash())
                .border_1()
                .border_color(th::line())
                .when(has_chips, |d| {
                    d.child(
                        div()
                            .flex_1()
                            .flex()
                            .items_start()
                            .gap_3()
                            .when(busy, |d| d.opacity(0.45))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pt(px(4.))
                                    .flex_none()
                                    .child(icon("icons/layers.svg", 14., th::muted()))
                                    .child(
                                        div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(th::muted()).child("Set all to"),
                                    ),
                            )
                            .child(div().flex_1().flex().flex_wrap().gap_2().children(chips)),
                    )
                })
                .when(!has_chips, |d| d.child(div().flex_1()))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(th::muted()).pr_1().child("Sort"))
                        .child(sort_button(SortKey::Name, "Name", "Sort by file name (click again to reverse)"))
                        .child(sort_button(SortKey::Size, "Size", "Sort by file size"))
                        .child(sort_button(SortKey::Type, "Type", "Group by file type")),
                )
                .into_any_element(),
        )
    }

    // ───────────────────────── queue ─────────────────────────

    /// (top, height) of a scrollbar thumb for `handle`, or None if nothing scrolls.
    fn scroll_thumb(handle: &ScrollHandle) -> Option<(f32, f32)> {
        let max = f32::from(handle.max_offset().height);
        let view = f32::from(handle.bounds().size.height);
        let offset = -f32::from(handle.offset().y);
        if max > 1.0 && view > 0.0 {
            let thumb = (view * view / (view + max)).clamp(36.0, view);
            let top = (offset / max).clamp(0.0, 1.0) * (view - thumb);
            Some((top, thumb))
        } else {
            None
        }
    }

    /// (top, height) of the thumb for the virtualised queue.
    ///
    /// GPUI's list only knows the heights of rows it has measured, so pixel
    /// totals are wrong for long queues. Position the thumb by item index and
    /// estimate row height from the rows already scrolled past instead.
    fn list_thumb(list: &ListState, len: usize) -> Option<(f32, f32)> {
        let view = f32::from(list.viewport_bounds().size.height);
        if len == 0 || view <= 0.0 {
            return None;
        }
        let top_item = list.logical_scroll_top();
        let above_px = -f32::from(list.scroll_px_offset_for_scrollbar().y); // measured rows above the viewport
        let avg = if top_item.item_ix > 3 { (above_px / top_item.item_ix as f32).max(40.0) } else { 240.0 };
        let est_total = avg * len as f32;
        if est_total <= view * 1.02 {
            return None; // everything fits
        }
        let thumb = (view * view / est_total).clamp(36.0, view);
        let position = top_item.item_ix as f32 + (f32::from(top_item.offset_in_item) / avg).clamp(0.0, 1.0);
        let last = (len as f32 - view / avg).max(1.0);
        let top = (position / last).clamp(0.0, 1.0) * (view - thumb);
        Some((top, thumb))
    }

    /// Only the visible rows are built each frame, so thousands of files stay smooth.
    fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let thumb = Self::list_thumb(&self.list, self.items.len());
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                list(self.list.clone(), cx.processor(|this, ix: usize, _window, cx| this.render_row(ix, cx)))
                    .size_full(),
            )
            .when_some(thumb, |d, (top, height)| {
                d.child(
                    div()
                        .absolute()
                        .top(px(top))
                        .right(px(1.))
                        .w(px(4.))
                        .h(px(height))
                        .rounded_full()
                        .bg(th::with_alpha(th::t().muted, 0x70)),
                )
            })
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        ROWS_BUILT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if ix >= self.items.len() {
            return div().into_any_element();
        }
        let card = self.render_item(&self.items[ix], cx);
        div().pb(px(12.)).pr(px(12.)).child(card).into_any_element()
    }

    fn render_item(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id;
        let accent = item.from.map(|f| th::family_color(f.family())).unwrap_or(th::t().muted);
        let tile_icon = item.from.map(|f| th::family_icon(f.family())).unwrap_or("icons/file.svg");
        let file_name: String = item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut meta = vec![item.from.map(|f| f.name().to_string()).unwrap_or_else(|| "Unknown type".into())];
        if let Some(sz) = item.size {
            meta.push(files::human_size(sz));
        }

        let plan = match (item.from, item.primary()) {
            (Some(f), Some(t)) => self.registry.plan(f, t),
            _ => None,
        };
        let in_flight = item.in_flight();
        let done = matches!(item.state, State::Done { .. });
        let multi_out = item.selected.len() > 1;
        let picked = self.picked == Some(item.id);
        let relevant = Relevant::of(item);
        let has_custom = item.opts.is_some();
        let drag_label = SharedString::from(file_name.clone());
        let options_panel = (item.options_open && relevant.any() && !in_flight)
            .then(|| self.render_item_options(item, &relevant, cx));

        // (progress fraction, status text, colour, icon)
        let (fraction, status, status_color, status_icon) = match &item.state {
            State::Ready if item.from.is_none() => {
                (0.0, "Unsupported file type".to_string(), th::bad(), Some("icons/alert.svg"))
            }
            State::Ready if item.targets.is_empty() => {
                (0.0, "Nothing to convert this to".to_string(), th::bad(), Some("icons/alert.svg"))
            }
            State::Ready if item.selected.is_empty() => {
                (0.0, "Pick at least one format".to_string(), th::muted(), Some("icons/alert.svg"))
            }
            State::Ready => (0.0, "Ready to convert".to_string(), th::muted(), None),
            State::Queued => (0.0, "Queued".to_string(), th::muted(), Some("icons/clock.svg")),
            State::Running(_) if item.cancelling => (
                if let State::Running(p) = item.state { p } else { 0.0 },
                "Cancelling…".to_string(),
                th::warn(),
                None,
            ),
            State::Running(p) => (*p, format!("Converting… {}%", (p * 100.0) as u32), rgb(th::BLUE), None),
            State::Done { out, .. } => (
                1.0,
                format!(
                    "Saved as {}",
                    out.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                ),
                th::good(),
                Some("icons/check.svg"),
            ),
            State::Failed(msg) => (
                0.0,
                msg.lines().next().unwrap_or("Conversion failed").to_string(),
                th::bad(),
                Some("icons/alert.svg"),
            ),
            State::Cancelled => (0.0, "Cancelled".to_string(), th::muted(), Some("icons/stop.svg")),
        };

        // "3.3 MB → 512 KB · -85%" once finished
        let size_delta = match (&item.state, item.size) {
            (State::Done { out_size: Some(after), .. }, Some(before)) => Some(files::size_change(before, *after)),
            _ => None,
        };

        // With several outputs the single line becomes a per-format tally.
        let status = if multi_out {
            match &item.state {
                State::Done { .. } => format!("Saved {} files", item.parts.len()),
                State::Failed(msg) => msg.lines().next().unwrap_or("Conversion failed").to_string(),
                State::Running(_) | State::Queued => {
                    if item.parts.is_empty() {
                        format!("Converting to {} formats…", item.selected.len())
                    } else {
                        item.parts
                            .iter()
                            .map(|p| {
                                let state = match &p.state {
                                    State::Queued => "queued".to_string(),
                                    State::Running(x) => format!("{}%", (*x * 100.0) as u32),
                                    State::Done { .. } => "done".to_string(),
                                    State::Failed(_) => "failed".to_string(),
                                    State::Cancelled => "cancelled".to_string(),
                                    State::Ready => "…".to_string(),
                                };
                                format!("{} {state}", p.target.name())
                            })
                            .collect::<Vec<_>>()
                            .join(" · ")
                    }
                }
                _ => status,
            }
        } else {
            status
        };

        // Format chips, grouped by family when the list spans several.
        let mut groups: Vec<(Family, Vec<Format>)> = Vec::new();
        for &f in &item.targets {
            match groups.last_mut() {
                Some((fam, v)) if *fam == f.family() => v.push(f),
                _ => groups.push((f.family(), vec![f])),
            }
        }
        let multi = groups.len() > 1;
        let from = item.from;
        let group_rows: Vec<AnyElement> = groups
            .into_iter()
            .map(|(fam, formats)| {
                let chips: Vec<AnyElement> = formats
                    .into_iter()
                    .map(|fmt| {
                        let route = from
                            .and_then(|f| self.registry.plan(f, fmt))
                            .map(|p| format!("{}{}", p.describe(), if p.is_lossy() { " (lossy)" } else { "" }))
                            .unwrap_or_default();
                        format_chip(
                            SharedString::from(format!("chip-{id}-{}", fmt.ext())),
                            fmt,
                            item.is_selected(fmt),
                        )
                        .tooltip(tip(if item.is_selected(fmt) {
                            format!("{route}\nClick to drop this output")
                        } else {
                            format!("{route}\nClick to add this output — a file can be converted to several formats at once")
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_target(id, fmt, cx)))
                        .into_any_element()
                    })
                    .collect();
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .when(multi, |d| {
                        d.child(
                            div()
                                .w(px(72.))
                                .flex_none()
                                .pt(px(5.))
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(th::faint())
                                .child(fam.name().to_uppercase()),
                        )
                    })
                    .child(div().flex_1().flex().flex_wrap().gap_2().children(chips))
                    .into_any_element()
            })
            .collect();

        let mut tags: Vec<AnyElement> = Vec::new();
        if let Some(p) = &plan {
            if p.is_lossy() {
                tags.push(
                    tag(SharedString::from(format!("lossy-{id}")), "lossy", th::t().warn)
                        .tooltip(tip("Some quality is lost with this target format. Pick a lossless one (PNG, WAV, FLAC…) to avoid that."))
                        .into_any_element(),
                );
            }
            if p.len() > 1 {
                let via: Vec<&str> = p.steps[..p.len() - 1].iter().map(|s| s.to.name()).collect();
                tags.push(
                    tag(SharedString::from(format!("via-{id}")), format!("via {}", via.join(" → ")), th::t().muted)
                        .tooltip(tip(format!(
                            "There is no direct converter, so the file is converted in {} steps ({}). Temporary files are deleted afterwards.",
                            p.len(),
                            p.describe()
                        )))
                        .into_any_element(),
                );
            }
        }

        // Controls on the right of the status row.
        let mut actions: Vec<AnyElement> = Vec::new();
        match &item.state {
            State::Queued | State::Running(_) if !item.cancelling => actions.push(
                text_button(SharedString::from(format!("cancel-{id}")), "Cancel", Some("icons/stop.svg"), "Cancel this file")
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_item(id, cx)))
                    .into_any_element(),
            ),
            State::Done { .. } => actions.push(
                text_button(
                    SharedString::from(format!("reveal-{id}")),
                    "Show in folder",
                    Some("icons/folder.svg"),
                    "Reveal the converted file",
                )
                .on_click(cx.listener(move |this, _, _, cx| this.reveal(id, cx)))
                .into_any_element(),
            ),
            State::Failed(msg) => {
                if msg.lines().count() > 1 || msg.chars().count() > 70 {
                    actions.push(
                        text_button(
                            SharedString::from(format!("details-{id}")),
                            if item.expanded { "Hide details" } else { "Details" },
                            Some(if item.expanded { "icons/chevron-up.svg" } else { "icons/chevron-down.svg" }),
                            "Show the full error message",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_details(id, cx)))
                        .into_any_element(),
                    );
                }
                if !self.busy {
                    actions.push(
                        text_button(SharedString::from(format!("retry-{id}")), "Retry", Some("icons/retry.svg"), "Try this file again")
                            .on_click(cx.listener(move |this, _, _, cx| this.retry(id, cx)))
                            .into_any_element(),
                    );
                }
            }
            State::Cancelled if !self.busy => actions.push(
                text_button(SharedString::from(format!("retry-{id}")), "Retry", Some("icons/retry.svg"), "Convert this file again")
                    .on_click(cx.listener(move |this, _, _, cx| this.retry(id, cx)))
                    .into_any_element(),
            ),
            _ => {}
        }

        let error_box = match (&item.state, item.expanded) {
            (State::Failed(msg), true) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .bg(th::wash())
                    .border_1()
                    .border_color(th::line())
                    .child(div().text_xs().text_color(th::muted()).child(SharedString::from(msg.clone())))
                    .child(
                        div().flex().child(
                            text_button(SharedString::from(format!("copy-{id}")), "Copy", Some("icons/copy.svg"), "Copy the error to the clipboard")
                                .on_click(cx.listener(move |this, _, _, cx| this.copy_error(id, cx))),
                        ),
                    ),
            ),
            _ => None,
        };

        div()
            .id(SharedString::from(format!("card-{id}")))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .bg(th::panel())
            .border_1()
            .when(picked, |d| d.border_color(th::with_alpha(th::VIOLET, 0xaa)))
            .when(!picked, |d| d.border_color(th::line()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| this.pick(id, cx)))
            .drag_over::<DraggedItem>(|s, _, _, _| s.border_color(rgb(th::CYAN)))
            .on_drop(cx.listener(move |this, dragged: &DraggedItem, _, cx| this.move_item(dragged.id, id, cx)))
            // header row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(SharedString::from(format!("grip-{id}")))
                            .flex_none()
                            .p_1()
                            .cursor_grab()
                            .tooltip(tip("Drag to reorder"))
                            .on_drag(DraggedItem { id, label: drag_label }, |d, _, _, cx| {
                                cx.new(|_| DragPreview { label: d.label.clone() })
                            })
                            .child(icon("icons/grip.svg", 14., th::faint())),
                    )
                    .child(icon_tile(tile_icon, accent, 40.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id(SharedString::from(format!("name-{id}")))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .tooltip(tip(item.path.display().to_string()))
                                    .child(SharedString::from(file_name)),
                            )
                            .child(div().text_xs().text_color(th::muted()).child(SharedString::from(meta.join(" · ")))),
                    )
                    .children(tags)
                    .when(has_custom, |d| {
                        d.child(
                            tag(SharedString::from(format!("custom-{id}")), "custom", th::VIOLET)
                                .tooltip(tip("This file has its own options")),
                        )
                    })
                    .when(relevant.any() && !in_flight, |d| {
                        d.child(
                            icon_button(SharedString::from(format!("opt-{id}")), "icons/settings.svg", "Options for this file")
                                .when(item.options_open, |b| b.bg(th::wash_hi()))
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_options(id, cx))),
                        )
                    })
                    .when(!in_flight, |d| {
                        d.child(
                            icon_button(SharedString::from(format!("rm-{id}")), "icons/x.svg", "Remove from the list")
                                .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx))),
                        )
                    }),
            )
            // format picker
            .when(!group_rows.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(section_label("CONVERT TO"))
                        .child(div().flex().flex_col().gap_2().children(group_rows)),
                )
            })
            .children(options_panel)
            // progress
            .child(
                div()
                    .h(px(6.))
                    .w_full()
                    .rounded_full()
                    .bg(th::wash_hi())
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .rounded_full()
                            .when(done, |d| d.bg(th::good()))
                            .when(!done, |d| d.bg(th::brand(90.0))),
                    ),
            )
            // status row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(status_color)
                    .when_some(status_icon, |d, path| d.child(icon(path, 14., status_color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(SharedString::from(status)),
                    )
                    .when_some(size_delta, |d, (text, good)| {
                        d.child(
                            div()
                                .flex_none()
                                .text_color(if good { th::good() } else { th::warn() })
                                .child(SharedString::from(text)),
                        )
                    })
                    .children(actions),
            )
            .children(error_box)
            .into_any_element()
    }

    /// Per-file options: only the controls that matter for this file's target.
    fn render_item_options(&self, item: &Item, rel: &Relevant, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id;
        let o = item.opts.clone().unwrap_or_else(|| self.settings.options());

        fn row(label: &'static str, control: impl IntoElement) -> gpui::Div {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(th::muted()).child(label))
                .child(div().flex_none().child(control))
        }

        let mut rows: Vec<AnyElement> = Vec::new();
        if rel.quality {
            let group = seg_group(
                [("Good", 75u8), ("High", 90), ("Maximum", 100)]
                    .into_iter()
                    .map(|(label, v)| {
                        seg_button(SharedString::from(format!("iq-{id}-{v}")), label, o.image_quality == v)
                            .on_click(cx.listener(move |this, _, _, cx| this.tweak_item(id, cx, |o| o.image_quality = v)))
                    })
                    .collect(),
            );
            rows.push(row("Quality", group).into_any_element());
        }
        if rel.size {
            let group = seg_group(
                [("Original", None), ("3840", Some(3840u32)), ("1920", Some(1920)), ("1280", Some(1280))]
                    .into_iter()
                    .map(|(label, v)| {
                        seg_button(SharedString::from(format!("is-{id}-{}", v.unwrap_or(0))), label, o.image_max_dimension == v)
                            .on_click(cx.listener(move |this, _, _, cx| this.tweak_item(id, cx, |o| o.image_max_dimension = v)))
                    })
                    .collect(),
            );
            rows.push(row("Largest size (px)", group).into_any_element());
        }
        if rel.bitrate {
            let group = seg_group(
                [128u32, 192, 256, 320]
                    .into_iter()
                    .map(|v| {
                        seg_button(SharedString::from(format!("ib-{id}-{v}")), format!("{v}"), o.audio_bitrate_kbps == v)
                            .on_click(cx.listener(move |this, _, _, cx| this.tweak_item(id, cx, |o| o.audio_bitrate_kbps = v)))
                    })
                    .collect(),
            );
            rows.push(row("Audio bitrate (kbps)", group).into_any_element());
        }
        if rel.crf {
            let group = seg_group(
                [("Best", 18u8), ("Balanced", 23), ("Small", 28)]
                    .into_iter()
                    .map(|(label, v)| {
                        seg_button(SharedString::from(format!("ic-{id}-{v}")), label, o.video_crf == v)
                            .on_click(cx.listener(move |this, _, _, cx| this.tweak_item(id, cx, |o| o.video_crf = v)))
                    })
                    .collect(),
            );
            rows.push(row("Video quality", group).into_any_element());
        }
        if rel.page {
            let page = o.pdf_page.max(1);
            let stepper = div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    text_button(SharedString::from(format!("pg-dec-{id}")), "−", None, "Previous page")
                        .on_click(cx.listener(move |this, _, _, cx| this.tweak_item(id, cx, |o| o.pdf_page = o.pdf_page.saturating_sub(1).max(1)))),
                )
                .child(div().min_w(px(44.)).flex().justify_center().text_sm().font_weight(FontWeight::SEMIBOLD).child(format!("{page}")))
                .child(
                    text_button(SharedString::from(format!("pg-inc-{id}")), "+", None, "Next page")
                        .on_click(cx.listener(move |this, _, _, cx| this.tweak_item(id, cx, |o| o.pdf_page += 1))),
                );
            rows.push(row("PDF page", stepper).into_any_element());
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(th::wash())
            .border_1()
            .border_color(th::line())
            .children(rows)
            .when(item.opts.is_some(), |d| {
                d.child(
                    div().flex().child(
                        text_button(SharedString::from(format!("io-reset-{id}")), "Use default options", Some("icons/retry.svg"), "Go back to the options from Settings")
                            .on_click(cx.listener(move |this, _, _, cx| this.reset_item_options(id, cx))),
                    ),
                )
            })
            .into_any_element()
    }

    // ───────────────────────── footer ─────────────────────────

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.items.len();
        let convertible = self.convertible();
        let can_convert = !self.busy && convertible > 0;
        let k = mod_key();

        let left: AnyElement = if self.busy {
            let (finished, batch_total, fraction) = self.batch_progress();
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(th::muted())
                        .child(format!(
                            "{finished} of {batch_total} done · {}%{}",
                            (fraction * 100.0) as u32,
                            self.eta().map(|d| format!(" · ~{} left", short_duration(d))).unwrap_or_default()
                        )),
                )
                .child(
                    div()
                        .w(px(160.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(th::wash_hi())
                        .overflow_hidden()
                        .child(div().h_full().w(relative(fraction)).rounded_full().bg(th::brand(90.0))),
                )
                .into_any_element()
        } else if let Some((text, ok)) = &self.summary {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(if *ok { th::good() } else { th::warn() })
                .child(icon(if *ok { "icons/check.svg" } else { "icons/alert.svg" }, 15., if *ok { th::good() } else { th::warn() }))
                .child(SharedString::from(text.clone()))
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(th::muted())
                .child(format!("{total} file{}", if total == 1 { "" } else { "s" }))
                .into_any_element()
        };

        let clear = ghost_button("clear", "Clear finished", "icons/broom.svg")
            .tooltip(tip(format!("Remove finished and cancelled files  ({k}K)")))
            .when(!self.has_finished(), |b| b.opacity(0.4))
            .when(self.has_finished(), |b| b.on_click(cx.listener(|this, _, _, cx| this.clear_finished(cx))));

        // What to do next with the result: both only matter once a batch has run.
        let failed_count = self.items.iter().filter(|i| matches!(i.state, State::Failed(_))).count();
        let open_folder = (!self.busy && self.summary.is_some())
            .then(|| {
                ghost_button("open-folder", "Open folder", "icons/folder.svg")
                    .tooltip(tip("Show the converted files in Explorer"))
                    .on_click(cx.listener(|this, _, _, cx| this.open_output_folder(cx)))
            })
            .filter(|_| !matches!(self.settings.output, OutputMode::NextToSource) || self.has_finished());
        let retry = (failed_count > 0 && !self.busy).then(|| {
            text_button(
                SharedString::from("retry-failed"),
                format!("Retry {failed_count} failed"),
                Some("icons/retry.svg"),
                "Convert the files that failed again",
            )
            .text_color(th::warn())
            .on_click(cx.listener(|this, _, _, cx| this.retry_failed(cx)))
        });

        // Many images, one document: the reverse of PDF -> image.
        let image_count = self.items.iter().filter(|i| i.from.is_some_and(|f| f.family() == Family::Image)).count();
        let combine = (image_count >= 2 && !self.busy).then(|| {
            ghost_button("combine-pdf", format!("Combine {image_count} into PDF"), "icons/layers.svg")
                .tooltip(tip("One page per image, in queue order"))
                .on_click(cx.listener(|this, _, _, cx| this.combine_into_pdf(cx)))
        });

        let main_button: AnyElement = if self.busy {
            danger_button("cancel-all", "Cancel all", "icons/stop.svg")
                .tooltip(tip(format!("Stop every running and queued file  ({k}.)")))
                .on_click(cx.listener(|this, _, _, cx| this.cancel_all(cx)))
                .into_any_element()
        } else {
            let label = if convertible > 1 { format!("Convert all ({convertible})") } else { "Convert".to_string() };
            primary_button("convert", label, "icons/play.svg")
                .tooltip(tip(format!("Start converting  ({k}Enter)")))
                .when(!can_convert, |b| b.opacity(0.4))
                .when(can_convert, |b| b.on_click(cx.listener(|this, _, _, cx| this.start(None, cx))))
                .into_any_element()
        };

        div()
            .flex()
            .items_center()
            .gap_2()
            .child(left)
            .child(div().flex_1())
            .children(retry)
            .children(combine)
            .children(open_folder)
            .child(clear)
            .child(main_button)
    }

    // ───────────────────────── settings panel ─────────────────────────

    fn render_settings(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.settings;
        let max_h = (f32::from(window.viewport_size().height) - 110.0).max(240.0);

        // Short controls sit next to their label; long ones go underneath.
        fn inline(label: &'static str, control: impl IntoElement) -> gpui::Div {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
                .child(div().flex_none().child(control))
        }
        fn stacked(label: &'static str, control: impl IntoElement) -> gpui::Div {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
                .child(control)
        }
        fn heading(text: &'static str) -> gpui::Div {
            div().pt(px(6.)).child(section_label(text))
        }

        let theme = seg_group(vec![
            seg_button("theme-auto".into(), "Auto", s.theme == ThemeChoice::Auto)
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.theme = ThemeChoice::Auto))),
            seg_button("theme-dark".into(), "Dark", s.theme == ThemeChoice::Dark)
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.theme = ThemeChoice::Dark))),
            seg_button("theme-light".into(), "Light", s.theme == ThemeChoice::Light)
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.theme = ThemeChoice::Light))),
        ]);
        let notify = seg_group(vec![
            seg_button("nt-on".into(), "On", s.notify)
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.notify = true))),
            seg_button("nt-off".into(), "Off", !s.notify)
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.notify = false))),
        ]);

        let folder_label = match &s.output {
            OutputMode::Folder(p) => Some(p.display().to_string()),
            OutputMode::NextToSource => None,
        };
        let output = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(seg_group(vec![
                seg_button("out-next".into(), "Next to original", folder_label.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.output = OutputMode::NextToSource))),
                seg_button("out-folder".into(), "Choose folder…", folder_label.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.choose_output_folder(cx))),
            ]))
            .when_some(folder_label, |d, path| {
                d.child(
                    div()
                        .id("out-path")
                        .text_xs()
                        .text_color(th::muted())
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .tooltip(tip(path.clone()))
                        .child(SharedString::from(path)),
                )
            });
        let conflict = seg_group(vec![
            seg_button("ow-rename".into(), "Keep both", !s.overwrite)
                .tooltip(tip("Adds “-converted” to the name, e.g. photo-converted.jpg"))
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.overwrite = false))),
            seg_button("ow-over".into(), "Overwrite", s.overwrite)
                .tooltip(tip("Replaces an existing file with the same name. The original input file is never overwritten."))
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.overwrite = true))),
        ]);

        let quality = seg_group(
            [("Good", 75u8), ("High", 90), ("Maximum", 100)]
                .into_iter()
                .map(|(label, v)| {
                    seg_button(SharedString::from(format!("q-{v}")), label, s.image_quality == v)
                        .on_click(cx.listener(move |this, _, _, cx| this.tweak(cx, |s| s.image_quality = v)))
                })
                .collect(),
        );
        let max_size = seg_group(
            [("Original", None), ("3840 px", Some(3840u32)), ("1920 px", Some(1920)), ("1280 px", Some(1280))]
                .into_iter()
                .map(|(label, v)| {
                    seg_button(SharedString::from(format!("ms-{}", v.unwrap_or(0))), label, s.image_max_dimension == v)
                        .tooltip(tip("Larger images are scaled down to fit; smaller ones are never enlarged."))
                        .on_click(cx.listener(move |this, _, _, cx| this.tweak(cx, |s| s.image_max_dimension = v)))
                })
                .collect(),
        );
        let bitrate = seg_group(
            [128u32, 192, 256, 320]
                .into_iter()
                .map(|v| {
                    seg_button(SharedString::from(format!("br-{v}")), format!("{v} kbps"), s.audio_bitrate_kbps == v)
                        .on_click(cx.listener(move |this, _, _, cx| this.tweak(cx, |s| s.audio_bitrate_kbps = v)))
                })
                .collect(),
        );
        let video = seg_group(
            [("Best", 18u8), ("Balanced", 23), ("Small file", 28)]
                .into_iter()
                .map(|(label, v)| {
                    seg_button(SharedString::from(format!("crf-{v}")), label, s.video_crf == v)
                        .tooltip(tip("Higher quality means bigger files and slower conversions."))
                        .on_click(cx.listener(move |this, _, _, cx| this.tweak(cx, |s| s.video_crf = v)))
                })
                .collect(),
        );

        // Preset chips: one click sets quality, size, bitrate and CRF together.
        let mut preset_chips: Vec<AnyElement> = Vec::new();
        let builtin_names: Vec<String> = crate::settings::builtin_presets().iter().map(|p| p.name.clone()).collect();
        for p in self.presets() {
            let name = p.name.clone();
            let active = p.matches(s);
            let label = format!("{} · {}", p.name, p.describe());
            preset_chips.push(
                seg_button(SharedString::from(format!("preset-{name}")), label, active)
                    .tooltip(tip(format!(
                        "Quality {}, {} px{}",
                        p.image_quality,
                        p.image_max_dimension.map(|d| d.to_string()).unwrap_or_else(|| "original".into()),
                        if builtin_names.contains(&p.name) { "" } else { " (saved by you)" }
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| this.apply_preset(name.clone(), cx)))
                    .into_any_element(),
            );
        }
        let user_presets: Vec<AnyElement> = s
            .presets
            .iter()
            .map(|p| {
                let name = p.name.clone();
                let shown = name.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(tag(SharedString::from(format!("keep-{name}")), shown, th::VIOLET))
                    .child(
                        icon_button(SharedString::from(format!("del-preset-{name}")), "icons/x.svg", "Delete this preset")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_preset(name.clone(), cx))),
                    )
                    .into_any_element()
            })
            .collect();
        let save_preset = ghost_button("save-preset", "Save current as preset", "icons/plus.svg")
            .tooltip(tip("Remembers the quality, size, bitrate and video settings below"))
            .on_click(cx.listener(|this, _, _, cx| this.save_preset(cx)));
        let presets_block = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().flex().flex_wrap().gap_2().children(preset_chips))
            .child(div().flex().items_center().gap_2().child(save_preset).children(user_presets));

        let metadata = seg_group(vec![
            seg_button("md-on".into(), "Keep", s.keep_metadata)
                .tooltip(tip("Copies the colour profile and EXIF (camera, date, orientation) into the converted image"))
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.keep_metadata = true))),
            seg_button("md-off".into(), "Drop", !s.keep_metadata)
                .tooltip(tip("Writes a plain image with no metadata"))
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.keep_metadata = false))),
        ]);
        let recycle = seg_group(vec![
            seg_button("rc-on".into(), "Recycle Bin", s.recycle_replaced)
                .tooltip(tip("The file being replaced goes to the Recycle Bin, so it can be restored"))
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.recycle_replaced = true))),
            seg_button("rc-off".into(), "Delete", !s.recycle_replaced)
                .tooltip(tip("The file being replaced is deleted outright"))
                .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.recycle_replaced = false))),
        ]);

        let thumb = Self::scroll_thumb(&self.settings_scroll);

        div()
            .id("settings-panel")
            .occlude()
            .absolute()
            .top(px(84.))
            .right(px(24.))
            .w(px(420.))
            .max_h(px(max_h))
            .flex()
            .flex_col()
            .rounded_xl()
            .bg(th::raised())
            .border_1()
            .border_color(th::line())
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss_settings(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .child(div().flex_1().text_base().font_weight(FontWeight::BOLD).child("Settings"))
                    .child(
                        icon_button("settings-close", "icons/x.svg", "Close  (Esc)")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_settings(cx))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("settings-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.settings_scroll)
                            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .pl_4()
                            .pr(px(20.))
                            .pb_4()
                            .child(heading("GENERAL"))
                            .child(inline("Theme", theme))
                            .child(inline("Notify when a long batch ends", notify))
                            .child(heading("OUTPUT"))
                            .child(stacked("Save converted files", output))
                            .child(inline("If the name already exists", conflict))
                            .child(inline("Replace by moving the old file to", recycle))
                            .child(heading("IMAGES"))
                            .child(stacked("Presets", presets_block))
                            .child(stacked("Quality (JPEG, WebP, AVIF)", quality))
                            .child(stacked("Largest image size", max_size))
                            .child(inline("Metadata", metadata))
                            .child(heading("AUDIO & VIDEO"))
                            .child(stacked("Audio bitrate (MP3, Ogg, Opus, AAC)", bitrate))
                            .child(inline("Video quality", video))
                            .child(
                                div().pt(px(6.)).flex().child(
                                    ghost_button("reset", "Reset to defaults", "icons/retry.svg")
                                        .on_click(cx.listener(|this, _, _, cx| this.tweak(cx, |s| s.reset_preferences()))),
                                ),
                            ),
                    )
                    .when_some(thumb, |d, (top, height)| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(top))
                                .right(px(8.))
                                .w(px(4.))
                                .h(px(height))
                                .rounded_full()
                                .bg(th::with_alpha(th::t().muted, 0x70)),
                        )
                    }),
            )
    }
}

/// Rows built since the last frame. Set `UBERCONV_DEBUG_ROWS=1` to print it:
/// with the virtualised list this stays at a handful however long the queue is.
static ROWS_BUILT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn short_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 90 {
        format!("{} s", secs.max(1))
    } else {
        format!("{} min", (secs + 30) / 60)
    }
}

impl Render for ConverterApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Theme: follow the system unless the user picked one.
        let light = match self.settings.theme {
            ThemeChoice::Light => true,
            ThemeChoice::Dark => false,
            ThemeChoice::Auto => matches!(window.appearance(), WindowAppearance::Light | WindowAppearance::VibrantLight),
        };
        th::set_light(light);
        let size = window.viewport_size();
        self.track_window(f32::from(size.width), f32::from(size.height));
        // Keep the native handle so a finished batch can flash the taskbar.
        files::remember_window(window);

        if std::env::var_os("UBERCONV_DEBUG_ROWS").is_some() {
            let built = ROWS_BUILT.swap(0, std::sync::atomic::Ordering::Relaxed);
            eprintln!("frame: {} items in queue, {built} rows built last frame", self.items.len());
        }
        let has_items = !self.items.is_empty();
        let bulk = self.render_bulk_bar(cx);

        div()
            .size_full()
            .relative()
            .bg(th::ink())
            .text_color(th::text())
            .font_family(FONT_FAMILY)
            .key_context("ConverterApp")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenFiles, _, cx| this.open_dialog(cx)))
            .on_action(cx.listener(|this, _: &ConvertAll, _, cx| this.start(None, cx)))
            .on_action(cx.listener(|this, _: &CancelAll, _, cx| this.cancel_all(cx)))
            .on_action(cx.listener(|this, _: &ClearFinished, _, cx| this.clear_finished(cx)))
            .on_action(cx.listener(|this, _: &ToggleSettings, _, cx| this.toggle_settings(cx)))
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| this.dismiss_settings(cx)))
            .on_action(cx.listener(|_, _: &NextField, window, _| window.focus_next()))
            .on_action(cx.listener(|_, _: &PrevField, window, _| window.focus_prev()))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &RemovePicked, _, cx| this.remove_picked(cx)))
            .on_action(cx.listener(|this, _: &TogglePickedOptions, _, cx| this.toggle_picked_options(cx)))
            .on_action(cx.listener(|this, _: &CopyErrors, _, cx| this.copy_all_errors(cx)))
            .on_action(cx.listener(|this, _: &CombinePdf, _, cx| this.combine_into_pdf(cx)))
            // soft violet glow at the top of the window
            .child(
                div().absolute().top_0().left_0().right_0().h(px(260.)).bg(linear_gradient(
                    180.,
                    linear_color_stop(th::glow(), 0.),
                    linear_color_stop(th::with_alpha(th::VIOLET, 0x00), 1.),
                )),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p(px(24.))
                    .child(self.render_header(cx))
                    .child(self.render_drop_zone(has_items, cx))
                    .children(bulk)
                    .when(!has_items, |d| d.child(self.render_capabilities()))
                    .when(has_items, |d| d.child(self.render_queue(cx)).child(self.render_footer(cx))),
            )
            .when_some(self.notice.as_ref().map(|n| (n.msg.clone(), n.undo)), |d, (msg, undo)| {
                d.child(
                    div().absolute().bottom(px(84.)).left_0().right_0().flex().justify_center().child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pl(px(14.))
                            .pr(px(if undo { 6. } else { 14. }))
                            .py(px(6.))
                            .rounded_full()
                            .bg(th::raised())
                            .border_1()
                            .border_color(th::line())
                            .shadow_md()
                            .text_sm()
                            .child(SharedString::from(msg))
                            .when(undo, |p| {
                                p.child(
                                    text_button("undo", "Undo", Some("icons/undo.svg"), format!("Undo  ({}Z)", mod_key()))
                                        .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                                )
                            }),
                    ),
                )
            })
            .when(self.settings_open, |d| d.child(self.render_settings(window, cx)))
    }
}
