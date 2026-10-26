//! Pure zoom/scroll/display-space math: zoom stops, render-scale sizing,
//! the zoom-anchor-at-cursor correction, and the `text::Rect` -> egui
//! coordinate conversion every page-drawing call site shares. No
//! `SpikeApp`, no `egui::Ui` — kept as free functions so the tricky parts
//! (zoom stops, anchor correction, render staleness) have direct unit
//! tests instead of only being exercised indirectly through a live
//! `egui::Context`.

use crate::text;
use eframe::egui;

/// Pixels per PDF point at 100% zoom. 2.0 == 144 DPI (72 * 2).
pub const SCALE: f32 = 2.0;

/// Matches `keys.js`'s `ZOOM_LEVELS` bounds (0.5 to 3.0).
pub const MIN_ZOOM_FACTOR: f32 = 0.5;
pub const MAX_ZOOM_FACTOR: f32 = 3.0;
/// How far above the exact device-pixel-crisp resolution a page is
/// rendered, so one `ZOOM_LEVELS` step still reuses the current texture
/// (a cheap GPU upscale) instead of immediately re-rendering. Stops are
/// ~10-20% apart near 100%, so 1.15 covers roughly one step; the next
/// step re-renders, which is fine now that rendering runs on
/// `render_worker.rs`'s background thread instead of blocking a frame.
pub const RENDER_HEADROOM: f32 = 1.15;
/// Safety ceiling on rendered resolution, independent of zoom/DPI, so a
/// pathological combination (maximum PDF zoom on a very high-density
/// display) can't allocate an unreasonably large bitmap before
/// `viewer::prepare_page`'s own per-page `max_texture_side` clamp even
/// gets a chance to run. Generous on purpose — `MAX_ZOOM_FACTOR`'s zoom
/// ceiling times a 2x device pixel ratio times `RENDER_HEADROOM`, with a
/// little extra margin for a 3x display.
pub const MAX_RENDER_SCALE: f32 = SCALE * MAX_ZOOM_FACTOR * 3.0 * RENDER_HEADROOM;

/// The pixels-per-point a page should be rendered at for display zoom
/// `zoom` on a display reporting `pixels_per_point` (`egui::Context::
/// pixels_per_point`, 2.0 on a typical Retina/HiDPI display) — the exact
/// resolution needed for that page to look pixel-for-pixel crisp on
/// *this* screen at *this* zoom, not a fixed constant picked without
/// knowing the display. Matches how PDF.js and Chrome's own PDF viewer
/// size their canvas: `scale * devicePixelRatio`, re-rendering (cheaply,
/// asynchronously — see `render_worker.rs`) when either changes, rather
/// than pre-rendering extra resolution "just in case" to avoid that.
///
/// A previous version of this function ignored `pixels_per_point`
/// entirely and instead pre-rendered every page at a single fixed high
/// scale regardless of zoom, reasoning that re-rendering on zoom was too
/// slow before rendering moved to a background thread. That version
/// happened to look fine at exactly 100% zoom on a 2x-density display
/// (its fixed scale, chosen before this function existed, was generously
/// above what 100%-zoom-at-2x needed) but was quietly under-resolved —
/// blurry — at *every* zoom level on a 3x display, and (found live,
/// described as "rendering looks weird") became visibly blurry even at
/// 100% zoom once a zoom-proportional headroom factor replaced that
/// fixed scale without also accounting for display density. Fixed by
/// multiplying by `pixels_per_point` directly instead of assuming it.
pub fn target_render_scale(zoom: f32, pixels_per_point: f32) -> f32 {
    (zoom * pixels_per_point * RENDER_HEADROOM).min(MAX_RENDER_SCALE)
}

/// Whether page `i` needs a fresh render job submitted, given its
/// current `rendered` state's `requested_scale` and any in-flight
/// `pending` job, against the live `target_scale`. Dark mode plays no
/// part here: `viewer::prepare_page` always builds both theme textures
/// together, so a dark/light toggle never needs a new render job, only
/// a different texture already sitting in `RenderedPage`. Pulled out of
/// `SpikeApp::progressive_render`'s submission loop so this decision —
/// the one piece of the worker integration most likely to regress into
/// either starving a page or looping forever — has its own direct unit
/// tests rather than only being exercised indirectly through a live
/// `egui::Context`.
///
/// Compares against `requested_scale`, not the possibly-clamped
/// `render_scale` a page's bitmap actually ended up at (see
/// `viewer::RenderedPage::requested_scale`'s own doc comment): comparing
/// against the clamped value would re-request the same clamped result
/// forever for an oversized page at a high zoom, since a clamped render
/// can never reach `target_scale` no matter how many times it's
/// re-requested.
pub fn page_needs_render(rendered: Option<f32>, pending: Option<f32>, target_scale: f32) -> bool {
    if pending.is_some_and(|pending_scale| pending_scale + 0.01 >= target_scale) {
        return false;
    }
    match rendered {
        None => true,
        Some(requested_scale) => target_scale > requested_scale + 0.01,
    }
}

/// Match keys.js's zoom stops. Small steps near 100% avoid the large
/// 20% jump the native viewer previously made on every keypress.
pub const ZOOM_LEVELS: &[f32] = &[
    0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.3, 1.5, 1.7, 1.9, 2.1, 2.4, 2.7, 3.0,
];

pub fn zoom_step(current: f32, direction: i8) -> f32 {
    let factor = current / SCALE;
    let next = if direction > 0 {
        ZOOM_LEVELS.iter().copied().find(|&level| level > factor + 0.001).unwrap_or(MAX_ZOOM_FACTOR)
    } else {
        ZOOM_LEVELS.iter().copied().rfind(|&level| level < factor - 0.001).unwrap_or(MIN_ZOOM_FACTOR)
    };
    next * SCALE
}

/// Points scrolled per `scroll_down`/`scroll_up` action — matches
/// `keys.js`'s plain `j`/`k` (`scroll(50)`/`scroll(-50)`). Sign
/// confirmed (and originally flipped) by live testing.
pub const SCROLL_STEP: f32 = 50.0;
/// Matches `keys.js`'s `Ctrl+u`/`Ctrl+d` (`scroll(±150)`).
pub const HALF_PAGE_SCROLL: f32 = 150.0;
/// Large enough to hit the top/bottom clamp of any realistic document for
/// `document_start`/`document_end`.
pub const SCROLL_MAX: f32 = 1_000_000.0;

/// Where a zoom step should keep the view visually anchored: `page`'s
/// index, the screen point the pointer (or viewport center) was over,
/// and that same point in `page`-local PDF-point coordinates.
pub struct ZoomAnchor {
    pub page: usize,
    pub screen_point: egui::Pos2,
    /// PDF page-local coordinates, measured in points.
    pub page_point: egui::Vec2,
}

impl ZoomAnchor {
    pub fn correction(&self, new_rect: egui::Rect, zoom: f32) -> egui::Vec2 {
        self.screen_point - (new_rect.min + self.page_point * zoom)
    }
}

/// Converts a `text::Rect` (in a page's own `render_scale` pixel space —
/// the space its texture and `page.chars` were computed in) into display
/// space at the given scale factor (`display_zoom / render_scale`).
pub fn to_egui_rect(r: text::Rect, scale: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(r.left * scale, r.top * scale), egui::pos2(r.right * scale, r.bottom * scale))
}

#[cfg(test)]
mod render_scale_tests {
    use super::*;

    #[test]
    fn target_render_scale_multiplies_by_device_pixel_ratio_and_headroom() {
        // 2x device pixel ratio (a typical Retina/HiDPI display).
        assert_eq!(target_render_scale(SCALE, 2.0), SCALE * 2.0 * RENDER_HEADROOM);
        // 1x (a plain display) needs proportionally less.
        assert_eq!(target_render_scale(SCALE, 1.0), SCALE * RENDER_HEADROOM);
    }

    #[test]
    fn target_render_scale_caps_at_the_safety_ceiling() {
        assert_eq!(target_render_scale(MAX_RENDER_SCALE, 1.0), MAX_RENDER_SCALE, "must not exceed the ceiling even with headroom applied");
        assert_eq!(target_render_scale(MAX_RENDER_SCALE * 10.0, 4.0), MAX_RENDER_SCALE);
    }

    #[test]
    fn an_unrendered_page_needs_a_render() {
        assert!(page_needs_render(None, None, 3.0), "never rendered");
    }

    #[test]
    fn a_page_rendered_at_or_above_target_scale_does_not_need_rerendering() {
        assert!(!page_needs_render(Some(3.0), None, 3.0), "exactly at target");
        assert!(!page_needs_render(Some(5.0), None, 3.0), "zoomed back out — reuse the higher-res texture");
    }

    #[test]
    fn zooming_in_past_the_last_requested_scale_needs_a_rerender() {
        assert!(page_needs_render(Some(2.0), None, 3.0));
    }

    #[test]
    fn a_page_clamped_below_the_target_does_not_loop_forever() {
        // An oversized page requested at scale 3.0 but clamped down to
        // 1.0 by `max_texture_side`: `rendered` reports the clamped
        // *delivered* resolution is irrelevant here because this
        // function is given `requested_scale` (3.0), not the clamped
        // one — so as long as the target hasn't grown past what was
        // already requested, this must say "no" even though the texture
        // itself is objectively lower-resolution than target_scale.
        assert!(!page_needs_render(Some(3.0), None, 3.0));
    }

    #[test]
    fn an_in_flight_job_blocks_resubmission() {
        assert!(!page_needs_render(None, Some(3.0), 3.0));
        assert!(!page_needs_render(Some(2.0), Some(3.0), 3.0), "already requested the higher scale, don't duplicate");
    }

    #[test]
    fn zooming_past_pending_resolution_supersedes_the_old_request() {
        assert!(page_needs_render(Some(2.0), Some(2.5), 3.0));
        assert!(page_needs_render(None, Some(2.5), 3.0));
    }

    #[test]
    fn zoom_keys_use_pdfjs_stops() {
        assert_eq!(zoom_step(SCALE, 1), SCALE * 1.1);
        assert_eq!(zoom_step(SCALE * 1.1, 1), SCALE * 1.3);
        assert_eq!(zoom_step(SCALE, -1), SCALE * 0.9);
        assert_eq!(zoom_step(SCALE * 0.5, -1), SCALE * 0.5);
        assert_eq!(zoom_step(SCALE * 3.0, 1), SCALE * 3.0);
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    #[test]
    fn scroll_area_keeps_separate_offsets_per_document_tab() {
        let ctx = egui::Context::default();
        let frame = |tab: usize, delta: Option<egui::Vec2>| {
            let mut top = 0.0;
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::both().id_salt(("pdf-scroll", tab)).animated(false).show(ui, |ui| {
                        if let Some(delta) = delta { ui.scroll_with_delta(delta); }
                        top = ui.allocate_exact_size(egui::vec2(500.0, 3000.0), egui::Sense::hover()).0.top();
                    });
                });
            });
            output.textures_delta.clear();
            top
        };
        frame(0, None);
        frame(0, Some(egui::vec2(0.0, -900.0)));
        let tab_zero = frame(0, None);
        frame(1, None);
        let tab_one = frame(1, None);
        assert!(tab_zero < tab_one - 500.0, "tab 1 must open at its own start, not tab 0's offset");
        frame(1, Some(egui::vec2(0.0, -400.0)));
        frame(1, None);
        assert_eq!(frame(0, None), tab_zero, "returning to tab 0 must restore its scroll offset");
    }

    #[test]
    fn zoom_anchor_accounts_for_centering_and_page_gaps() {
        let point = egui::pos2(400.0, 350.0);
        let old_rect = egui::Rect::from_min_size(egui::pos2(200.0, 250.0), egui::vec2(400.0, 600.0));
        let anchor = ZoomAnchor { page: 1, screen_point: point, page_point: (point - old_rect.min) / 2.0 };
        // The new page has a different centering pad and a shifted top due
        // to earlier pages and fixed-size gaps in the scroll layout.
        let new_rect = egui::Rect::from_min_size(egui::pos2(100.0, 420.0), egui::vec2(800.0, 1200.0));
        let correction = anchor.correction(new_rect, 4.0);
        assert_eq!(new_rect.min + anchor.page_point * 4.0 + correction, point);
        assert_eq!(correction, egui::vec2(-100.0, -270.0));
    }

    #[test]
    fn egui_discard_hides_the_uncorrected_zoom_pass() {
        let ctx = egui::Context::default();
        let mut page_positions = Vec::new();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut corrected = false;
                egui::ScrollArea::vertical().animated(false).show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(200.0, 800.0), egui::Sense::hover());
                    page_positions.push(rect.min.y);
                    if page_positions.len() == 1 {
                        ui.scroll_with_delta(egui::vec2(0.0, -100.0));
                        corrected = true;
                    }
                });
                if corrected {
                    ui.ctx().request_discard("zoom anchor scroll correction");
                }
            });
        });
        let passes = output.platform_output.num_completed_passes;
        output.drop_without_applying_deltas();
        assert_eq!(passes, 2);
        assert_eq!(page_positions.len(), 2);
        assert!((page_positions[1] - page_positions[0] + 100.0).abs() < 1.0, "{page_positions:?}");
    }
}
