//! Reusable building blocks: icons, buttons, chips, tooltips, segmented controls.

use gpui::{
    div, prelude::*, px, rgb, svg, AnyView, App, Context, Div, ElementId, FontWeight, Hsla, Render,
    SharedString, Stateful, Svg, Window,
};
use uberconv_core::Format;

use crate::{assets::FONT_FAMILY, theme as th};

/// GPUI draws SVGs as a mask and tints them with the text colour.
pub fn icon(path: &'static str, size_px: f32, color: impl Into<Hsla>) -> Svg {
    svg().path(path).size(px(size_px)).flex_none().text_color(color)
}

/// "Ctrl+" or the Command symbol, for shortcut hints.
pub fn mod_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    }
}

// ─────────────────────────── tooltips ───────────────────────────

pub struct TextTooltip {
    text: SharedString,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(360.))
            .px(px(10.))
            .py(px(6.))
            .rounded_lg()
            .bg(th::raised())
            .border_1()
            .border_color(th::line())
            .shadow_md()
            .font_family(FONT_FAMILY)
            .text_xs()
            .text_color(th::text())
            .child(self.text.clone())
    }
}

/// `.tooltip(tip("..."))`
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| TextTooltip { text: text.clone() }).into()
}

// ─────────────────────────── logo & tiles ───────────────────────────

/// The logo: gradient squircle with the "u-turn" mark.
pub fn logo_badge(size_px: f32) -> Div {
    div()
        .size(px(size_px))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size_px * 0.28))
        .bg(th::brand(135.0))
        .shadow_md()
        .child(icon("icons/mark.svg", size_px * 0.6, rgb(0xffffff)))
}

pub fn icon_tile(path: &'static str, color: u32, tile: f32) -> Div {
    div()
        .size(px(tile))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(tile * 0.28))
        .bg(th::with_alpha(color, if th::t().light { 0x1c } else { 0x24 }))
        .child(icon(path, tile * 0.5, rgb(color)))
}

pub fn tag(id: impl Into<ElementId>, text: impl Into<SharedString>, color: u32) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .bg(th::with_alpha(color, 0x22))
        .text_color(rgb(color))
        .child(text.into())
}

// ─────────────────────────── buttons ───────────────────────────

fn focus_ring(s: gpui::StyleRefinement) -> gpui::StyleRefinement {
    s.border_color(rgb(th::CYAN))
}

fn button_base(id: &'static str, label: impl Into<SharedString>, icon_path: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .items_center()
        .gap_2()
        .px(px(16.))
        .py(px(9.))
        .rounded_lg()
        .border_1()
        .border_color(gpui::rgba(0x00000000))
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .focus(focus_ring)
        .child(icon(icon_path, 14., th::text()))
        .child(label.into())
}

pub fn primary_button(id: &'static str, label: impl Into<SharedString>, icon_path: &'static str) -> Stateful<Div> {
    button_base(id, label, icon_path)
        .bg(th::brand(100.0))
        .text_color(rgb(0xffffff))
        .shadow_md()
        .hover(|s| s.opacity(0.9))
}

pub fn ghost_button(id: &'static str, label: impl Into<SharedString>, icon_path: &'static str) -> Stateful<Div> {
    button_base(id, label, icon_path)
        .bg(th::wash())
        .border_color(th::line())
        .text_color(th::text())
        .hover(|s| s.bg(th::wash_hi()))
        .focus(focus_ring)
}

pub fn danger_button(id: &'static str, label: impl Into<SharedString>, icon_path: &'static str) -> Stateful<Div> {
    button_base(id, label, icon_path)
        .bg(th::with_alpha(th::t().bad, 0x1c))
        .border_color(th::with_alpha(th::t().bad, 0x55))
        .text_color(th::bad())
        .hover(|s| s.bg(th::with_alpha(th::t().bad, 0x2c)))
        .focus(focus_ring)
}

/// Small square icon-only button.
pub fn icon_button(id: impl Into<ElementId>, path: &'static str, tooltip: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(gpui::rgba(0x00000000))
        .cursor_pointer()
        .hover(|s| s.bg(th::wash_hi()))
        .focus(focus_ring)
        .tooltip(tip(tooltip))
        .child(icon(path, 15., th::muted()))
}

/// Small text link-style button (e.g. "Retry", "Details").
pub fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon_path: Option<&'static str>,
    tooltip: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(gpui::rgba(0x00000000))
        .cursor_pointer()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(th::muted())
        .hover(|s| s.bg(th::wash_hi()).text_color(th::text()))
        .focus(focus_ring)
        .tooltip(tip(tooltip))
        .when_some(icon_path, |d, p| d.child(icon(p, 13., th::muted())))
        .child(label.into())
}

// ─────────────────────────── chips & segmented controls ───────────────────────────

/// `lossy` puts a "~" inside the chip: with several outputs chosen, the trade
/// off belongs on each target, not on the file.
pub fn format_chip(id: SharedString, fmt: Format, selected: bool, lossy: bool) -> Stateful<Div> {
    let c = th::family_color(fmt.family());
    div()
        .id(id)
        .tab_index(0)
        .px(px(10.))
        .py(px(4.))
        .rounded_full()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .border_1()
        .focus(focus_ring)
        .when(selected, |d| {
            d.bg(th::with_alpha(c, 0x30)).border_color(th::with_alpha(c, 0xb0)).text_color(rgb(c))
        })
        .when(!selected, |d| {
            d.bg(th::wash())
                .border_color(th::line())
                .text_color(th::muted())
                .hover(|s| s.bg(th::wash_hi()).text_color(th::text()))
        })
        .child(div().flex().items_center().gap(px(4.)).child(fmt.name()).when(lossy, |d| {
            d.child(div().text_color(th::warn()).child("~"))
        }))
}

pub fn seg_button(id: SharedString, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .px(px(12.))
        .py(px(5.))
        .rounded_md()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .whitespace_nowrap()
        .when(selected, |d| d.bg(th::brand(135.)).text_color(rgb(0xffffff)))
        .when(!selected, |d| {
            d.text_color(th::muted()).hover(|s| s.bg(th::wash_hi()).text_color(th::text()))
        })
        .focus(|s| s.bg(th::wash_hi()))
        .child(label.into())
}

pub fn seg_group(buttons: Vec<Stateful<Div>>) -> Div {
    div()
        .flex()
        .flex_none()
        .gap(px(2.))
        .p(px(2.))
        .rounded_lg()
        .bg(th::wash())
        .border_1()
        .border_color(th::line())
        .children(buttons)
}

/// Small all-caps section label.
pub fn section_label(text: &'static str) -> Div {
    div()
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(th::faint())
        .child(text)
}

// ─────────────────────────── drag & drop ───────────────────────────

/// Payload while a queue card is being dragged to a new position.
#[derive(Clone)]
pub struct DraggedItem {
    pub id: u64,
    pub label: SharedString,
}

/// The floating chip that follows the cursor during a drag.
pub struct DragPreview {
    pub label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.))
            .py(px(8.))
            .rounded_lg()
            .bg(th::raised())
            .border_1()
            .border_color(rgb(th::CYAN))
            .shadow_lg()
            .font_family(FONT_FAMILY)
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(th::text())
            .child(self.label.clone())
    }
}
