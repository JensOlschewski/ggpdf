//! Disposable validation spike (see PLAN.md "Biggest risk — validate first").
//! Not the real app. Continuously scrolls every page of the active
//! document via `viewer.rs`, overlays per-character bounds, and supports
//! single-page click/drag selection + copy, to test whether PDFium's text
//! geometry can support visual.js-style motions and whether the viewer
//! layout can look/behave like the current Chrome/PDF.js-based ggpdf.

mod app;
mod config;
mod document;
mod input;
#[cfg(target_os = "macos")]
mod macos_documents;
mod render;
mod render_worker;
mod selection;
mod text;
mod viewer;

use document::PdfLibrary;
use eframe::egui;
use render_worker::RenderWorker;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use viewer::Viewer;

/// Resources shipped beside the executable in a macOS `.app`. Development
/// builds keep using paths relative to the current working directory.
fn bundled_resources() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents")
        .then(|| contents.join("Resources"))
}

/// Pixels per PDF point at 100% zoom. 2.0 == 144 DPI (72 * 2).
const SCALE: f32 = 2.0;

/// Matches `keys.js`'s `ZOOM_LEVELS` bounds (0.5 to 3.0).
const MIN_ZOOM_FACTOR: f32 = 0.5;
const MAX_ZOOM_FACTOR: f32 = 3.0;
/// How far above the exact device-pixel-crisp resolution a page is
/// rendered, so one `ZOOM_LEVELS` step still reuses the current texture
/// (a cheap GPU upscale) instead of immediately re-rendering. Stops are
/// ~10-20% apart near 100%, so 1.15 covers roughly one step; the next
/// step re-renders, which is fine now that rendering runs on
/// `render_worker.rs`'s background thread instead of blocking a frame.
const RENDER_HEADROOM: f32 = 1.15;
/// Safety ceiling on rendered resolution, independent of zoom/DPI, so a
/// pathological combination (maximum PDF zoom on a very high-density
/// display) can't allocate an unreasonably large bitmap before
/// `viewer::prepare_page`'s own per-page `max_texture_side` clamp even
/// gets a chance to run. Generous on purpose — `MAX_ZOOM_FACTOR`'s zoom
/// ceiling times a 2x device pixel ratio times `RENDER_HEADROOM`, with a
/// little extra margin for a 3x display.
const MAX_RENDER_SCALE: f32 = SCALE * MAX_ZOOM_FACTOR * 3.0 * RENDER_HEADROOM;

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
fn target_render_scale(zoom: f32, pixels_per_point: f32) -> f32 {
    (zoom * pixels_per_point * RENDER_HEADROOM).min(MAX_RENDER_SCALE)
}

/// Asks for another frame, same as `ctx.request_repaint()`, except
/// while this window isn't focused: then it asks again only after a
/// short delay instead of immediately, so background work (filling in
/// pages, a notice-box countdown) still completes without this window
/// continuously demanding frames from the OS compositor while some
/// *other* app has focus — see `progressive_render`'s call site for the
/// live symptom this fixes.
fn request_repaint(ctx: &egui::Context) {
    if ctx.input(|i| i.focused) {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Whether page `i` needs a fresh render job submitted, given its
/// current `rendered` state (`requested_scale`, `dark`) and any
/// in-flight `pending` job, against the live `target_scale`/`dark`.
/// Pulled out of `progressive_render`'s submission loop so this
/// decision — the one piece of the worker integration most likely to
/// regress into either starving a page or looping forever — has its own
/// direct unit tests rather than only being exercised indirectly through
/// a live `egui::Context`.
///
/// Compares against `requested_scale`, not the possibly-clamped
/// `render_scale` a page's bitmap actually ended up at (see
/// `viewer::RenderedPage::requested_scale`'s own doc comment): comparing
/// against the clamped value would re-request the same clamped result
/// forever for an oversized page at a high zoom, since a clamped render
/// can never reach `target_scale` no matter how many times it's
/// re-requested.
fn page_needs_render(rendered: Option<(f32, bool)>, pending: Option<(f32, bool)>, target_scale: f32, dark: bool) -> bool {
    if pending.is_some_and(|(pending_scale, pending_dark)| pending_dark == dark && pending_scale + 0.01 >= target_scale) {
        return false;
    }
    match rendered {
        None => true,
        Some((requested_scale, rendered_dark)) => rendered_dark != dark || target_scale > requested_scale + 0.01,
    }
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
    fn unrendered_or_wrong_theme_pages_need_a_render() {
        assert!(page_needs_render(None, None, 3.0, false), "never rendered");
        assert!(page_needs_render(Some((3.0, true)), None, 3.0, false), "rendered dark, now wants light");
    }

    #[test]
    fn a_page_rendered_at_or_above_target_scale_does_not_need_rerendering() {
        assert!(!page_needs_render(Some((3.0, false)), None, 3.0, false), "exactly at target");
        assert!(!page_needs_render(Some((5.0, false)), None, 3.0, false), "zoomed back out — reuse the higher-res texture");
    }

    #[test]
    fn zooming_in_past_the_last_requested_scale_needs_a_rerender() {
        assert!(page_needs_render(Some((2.0, false)), None, 3.0, false));
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
        assert!(!page_needs_render(Some((3.0, false)), None, 3.0, false));
    }

    #[test]
    fn an_in_flight_job_for_the_same_theme_blocks_resubmission() {
        assert!(!page_needs_render(None, Some((3.0, false)), 3.0, false));
        assert!(!page_needs_render(Some((2.0, false)), Some((3.0, false)), 3.0, false), "already requested the higher scale, don't duplicate");
    }

    #[test]
    fn zooming_past_pending_resolution_supersedes_the_old_request() {
        assert!(page_needs_render(Some((2.0, false)), Some((2.5, false)), 3.0, false));
        assert!(page_needs_render(None, Some((2.5, false)), 3.0, false));
    }

    #[test]
    fn an_in_flight_job_for_the_other_theme_does_not_block_a_theme_toggle() {
        assert!(page_needs_render(Some((3.0, true)), Some((3.0, true)), 3.0, false), "dark job in flight, but we now want light");
    }
}
/// Match keys.js's zoom stops. Small steps near 100% avoid the large
/// 20% jump the native viewer previously made on every keypress.
const ZOOM_LEVELS: &[f32] = &[
    0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.3, 1.5, 1.7, 1.9, 2.1, 2.4, 2.7, 3.0,
];

fn zoom_step(current: f32, direction: i8) -> f32 {
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
const SCROLL_STEP: f32 = 50.0;
/// Matches `keys.js`'s `Ctrl+u`/`Ctrl+d` (`scroll(±150)`).
const HALF_PAGE_SCROLL: f32 = 150.0;
/// Large enough to hit the top/bottom clamp of any realistic document for
/// `document_start`/`document_end`.
const SCROLL_MAX: f32 = 1_000_000.0;

struct Fixture {
    name: &'static str,
    path: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture { name: "two-column", path: "assets/test-pdfs/two-column.pdf" },
    Fixture { name: "rotated", path: "assets/test-pdfs/rotated.pdf" },
    Fixture { name: "ligatures", path: "assets/test-pdfs/ligatures.pdf" },
    Fixture { name: "no-text", path: "assets/test-pdfs/no-text.pdf" },
];

fn dark_mode_shortcut(mode: config::Mode, modifiers: egui::Modifiers) -> bool {
    mode == config::Mode::Normal && !modifiers.any()
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn dark_mode_shortcut_only_works_in_normal_mode() {
        for mode in [config::Mode::Cursor, config::Mode::Visual, config::Mode::VisualLine] {
            assert!(!dark_mode_shortcut(mode, egui::Modifiers::NONE));
        }
        assert!(dark_mode_shortcut(config::Mode::Normal, egui::Modifiers::NONE));
        assert!(!dark_mode_shortcut(config::Mode::Normal, egui::Modifiers::CTRL));
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
mod tab_cache_tests {
    use super::*;

    /// Regression test for "switching tabs is slow as hell": switching
    /// away from a tab must park its `Viewer` (with whatever pages it
    /// already rasterized) instead of dropping it, so switching back
    /// doesn't re-render anything.
    #[test]
    fn switching_tabs_keeps_previously_rendered_pages_cached() {
        let library = document::test_library();
        let paths = vec!["assets/test-pdfs/two-column.pdf".to_string(), "assets/test-pdfs/rotated.pdf".to_string()];
        let mut app = SpikeApp::new(library, config::Config::default(), &paths);
        let ctx = egui::Context::default();

        app.switch_to_active_tab(&ctx);
        // `SCALE`, not `progressive_render`'s zoom-driven target scale,
        // to keep the rasterized texture within `egui::Context::
        // default()`'s texture size limit — unrelated to what this test
        // is pinning down.
        let doc = library.open(&paths[0]).expect("open doc for rendering");
        app.viewer.as_mut().unwrap().render_page(&doc, 0, SCALE, 2048, false, &ctx);
        let tab0_rendered = app.viewer.as_ref().unwrap().pages.iter().filter(|p| p.rendered.is_some()).count();
        assert!(tab0_rendered > 0, "render_page should have rasterized the page");

        app.app.next_tab();
        app.switch_to_active_tab(&ctx);
        assert!(app.viewers[0].is_some(), "tab 0's viewer should be parked, not dropped, when switching away");
        let parked_tab0_rendered = app.viewers[0].as_ref().unwrap().pages.iter().filter(|p| p.rendered.is_some()).count();
        assert_eq!(parked_tab0_rendered, tab0_rendered, "parking shouldn't lose tab 0's already-rendered pages");

        app.app.prev_tab();
        app.switch_to_active_tab(&ctx);
        assert!(app.viewers[1].is_some(), "tab 1's viewer should now be parked in turn");
        let restored_tab0_rendered = app.viewer.as_ref().unwrap().pages.iter().filter(|p| p.rendered.is_some()).count();
        assert_eq!(restored_tab0_rendered, tab0_rendered, "switching back should restore tab 0 without re-rendering");
    }
}

struct ZoomAnchor {
    page: usize,
    screen_point: egui::Pos2,
    /// PDF page-local coordinates, measured in points.
    page_point: egui::Vec2,
}

impl ZoomAnchor {
    fn correction(&self, new_rect: egui::Rect, zoom: f32) -> egui::Vec2 {
        self.screen_point - (new_rect.min + self.page_point * zoom)
    }
}

/// `/` (forward) vs `?` (backward) — which way `n` repeats a search;
/// `N` repeats the other direction for just that one jump, matching
/// Vim, without changing this stored direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchDirection {
    Forward,
    Backward,
}

impl SearchDirection {
    fn opposite(self) -> SearchDirection {
        match self {
            SearchDirection::Forward => SearchDirection::Backward,
            SearchDirection::Backward => SearchDirection::Forward,
        }
    }
}

/// One `/`/`?` match: a flat `(start, end)` character range (`end`
/// inclusive) on page `page`, in the same index space as `cursor_idx`/
/// `flat_index`/`selection::selection_span_rects`.
#[derive(Debug, Clone)]
struct SearchMatch {
    page: usize,
    start: usize,
    cursor: selection::Cursor,
    /// Page-local rectangles in PDF points, independent of texture cache.
    spans: Vec<text::Rect>,
}

/// Index into `matches` (sorted by `(page, start)` ascending, as
/// `run_search` builds it) nearest to `(page, char_idx)` in `direction`:
/// the first match strictly after it for `Forward`, wrapping to the
/// first match in the document if none remain ahead; the last match
/// strictly before it for `Backward`, wrapping to the last match.
/// Mirrors Vim's `/pattern<CR>` jumping to the next match from the
/// cursor, not always restarting at the top of the document. Panics if
/// `matches` is empty — callers must check that first.
fn nearest_match_index(matches: &[SearchMatch], page: usize, char_idx: usize, direction: SearchDirection) -> usize {
    match direction {
        SearchDirection::Forward => matches.iter().position(|m| (m.page, m.start) > (page, char_idx)).unwrap_or(0),
        SearchDirection::Backward => matches.iter().rposition(|m| (m.page, m.start) < (page, char_idx)).unwrap_or(matches.len() - 1),
    }
}

/// `current` stepped `count` matches in `direction` among `len` total
/// matches, wrapping around — pure index arithmetic for `n`/`N`,
/// independent of how the match list itself was built. Panics if `len`
/// is 0 — callers must check that first.
fn step_match_index(current: usize, len: usize, count: usize, direction: SearchDirection) -> usize {
    let step = count.max(1) % len;
    match direction {
        SearchDirection::Forward => (current + step) % len,
        SearchDirection::Backward => (current + len - step) % len,
    }
}

#[cfg(test)]
mod search_index_tests {
    use super::*;

    fn m(page: usize, start: usize) -> SearchMatch {
        SearchMatch { page, start, cursor: selection::Cursor { line: 0, character: 0 }, spans: Vec::new() }
    }

    #[test]
    fn nearest_match_forward_finds_the_first_one_after_the_cursor() {
        let matches = [m(0, 5), m(0, 20), m(1, 0), m(2, 3)];
        assert_eq!(nearest_match_index(&matches, 0, 10, SearchDirection::Forward), 1);
        assert_eq!(nearest_match_index(&matches, 0, 20, SearchDirection::Forward), 2, "strictly after, not at");
    }

    #[test]
    fn nearest_match_forward_wraps_to_the_first_match_past_the_end() {
        let matches = [m(0, 5), m(1, 0)];
        assert_eq!(nearest_match_index(&matches, 2, 0, SearchDirection::Forward), 0);
    }

    #[test]
    fn nearest_match_backward_finds_the_last_one_before_the_cursor_and_wraps() {
        let matches = [m(0, 5), m(0, 20), m(1, 0)];
        assert_eq!(nearest_match_index(&matches, 1, 0, SearchDirection::Backward), 1);
        assert_eq!(nearest_match_index(&matches, 0, 0, SearchDirection::Backward), 2, "wraps to the last match");
    }

    #[test]
    fn step_match_index_wraps_in_both_directions() {
        assert_eq!(step_match_index(0, 3, 1, SearchDirection::Forward), 1);
        assert_eq!(step_match_index(2, 3, 1, SearchDirection::Forward), 0, "wraps forward past the end");
        assert_eq!(step_match_index(0, 3, 1, SearchDirection::Backward), 2, "wraps backward past the start");
        assert_eq!(step_match_index(1, 3, 4, SearchDirection::Forward), 2, "count larger than len still steps correctly");
    }

    #[test]
    fn search_indexes_unrendered_pages_and_keeps_page_geometry() {
        let library = document::test_library();
        let doc = library.open("assets/test-pdfs/two-column.pdf").expect("open fixture");
        let second_page = doc.page(1).expect("second page");
        let chars = text::position_chars(&second_page.raw_chars().unwrap(), second_page.size(), second_page.rotation_degrees(), 1.0);
        let query: String = chars.iter().take(3).map(|c| c.ch).collect();
        assert_eq!(query.chars().count(), 3);
        let mut app = SpikeApp::new(library, config::Config::default(), &["assets/test-pdfs/two-column.pdf".to_string()]);
        assert!(app.viewer.is_none(), "no texture or viewer required");
        app.search_query = query;
        app.run_search().expect("extract all pages");
        assert!(app.search_matches.iter().any(|m| m.page == 1 && m.start == 0 && !m.spans.is_empty()));
    }
}

/// Parses a `#rrggbb` or `#rrggbbaa` hex color from config. Falls back to
/// `fallback` on anything else — a config file is user-edited text, not a
/// trusted format, and a bad color string shouldn't crash the viewer.
fn parse_hex_color(s: &str, fallback: egui::Color32) -> egui::Color32 {
    let s = s.trim_start_matches('#');
    match s.len() {
        6 => match u32::from_str_radix(s, 16) {
            Ok(v) => egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
            Err(_) => fallback,
        },
        8 => match u32::from_str_radix(s, 16) {
            Ok(v) => egui::Color32::from_rgba_unmultiplied((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8),
            Err(_) => fallback,
        },
        _ => fallback,
    }
}

fn is_pdf_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

#[cfg(test)]
mod open_file_tests {
    use super::*;

    #[test]
    fn pdf_extension_is_case_insensitive() {
        assert!(is_pdf_path(Path::new("/somewhere/document.PDF")));
        assert!(!is_pdf_path(Path::new("/somewhere/document.txt")));
        assert!(!is_pdf_path(Path::new("/somewhere/folder")));
    }

    #[test]
    fn dropped_paths_open_tabs_without_adding_failed_files() {
        let library = document::test_library();
        let mut viewer = SpikeApp::new(library, config::Config::default(), &["assets/test-pdfs/letter.pdf".to_string()]);
        let ctx = egui::Context::default();
        viewer.open_pdf_paths([
            PathBuf::from("assets/test-pdfs/rotated.pdf"),
            PathBuf::from("assets/test-pdfs/missing.pdf"),
            PathBuf::from("assets/test-pdfs/no-text.pdf"),
            PathBuf::from("assets/test-pdfs/not-a-pdf.txt"),
        ], &ctx);
        assert_eq!(viewer.app.tab_count(), 3);
        assert_eq!(viewer.app.active_index(), 2);
        assert_eq!(viewer.app.tab_label(2), Some("no-text.pdf"));
    }

    #[test]
    fn finder_event_replaces_sample_then_opens_more_tabs() {
        let library = document::test_library();
        let mut viewer = SpikeApp::new(library, config::Config::default(), &[]);
        let ctx = egui::Context::default();
        assert_eq!(viewer.app.tab_label(0), Some("letter.pdf"));
        viewer.open_document_event(vec![PathBuf::from("assets/test-pdfs/rotated.pdf")], &ctx);
        assert_eq!(viewer.app.tab_count(), 1);
        assert_eq!(viewer.app.tab_label(0), Some("rotated.pdf"));
        viewer.open_document_event(vec![PathBuf::from("assets/test-pdfs/two-column.pdf")], &ctx);
        assert_eq!(viewer.app.tab_count(), 2);
        assert_eq!(viewer.app.tab_label(1), Some("two-column.pdf"));
        assert_eq!(viewer.app.active_index(), 1);
    }
}

struct SpikeApp {
    app: app::App<'static>,
    cfg: config::Config,
    dispatcher: input::Dispatcher,
    zoom: f32,
    dark: bool,
    viewer: Option<Viewer>,
    /// One already-rendered `Viewer` per tab, keyed by tab index, so
    /// switching back to a previously-viewed tab reuses its decoded
    /// pages/textures instead of re-rasterizing the whole document from
    /// scratch (found live: "switching tabs is slow as hell" — every
    /// switch used to call `load_viewer` unconditionally, discarding
    /// and rebuilding the `Viewer`, even for a tab visited moments
    /// before). `None` entries are tabs not yet loaded; the active
    /// tab's entry is always `None` while `viewer` holds it instead (see
    /// `switch_to_active_tab`).
    viewers: Vec<Option<Viewer>>,
    show_boxes: bool,
    /// Click/drag selection is scoped to one page at a time; `cursor_idx`/
    /// `anchor_idx` index into that page's `chars`, not a global range.
    active_page: Option<usize>,
    cursor_idx: Option<usize>,
    anchor_idx: Option<usize>,
    /// Keyboard motion cursor (`selection.rs`-driven, `h`/`j`/`k`/`l`/
    /// word motions/`is`/`ip`), separate from the mouse-drag selection
    /// above. Synced into `active_page`/`cursor_idx`/`anchor_idx` after
    /// every motion so both share the same on-screen highlight. `None`
    /// until the first keyboard motion (defaults to page 0, line 0,
    /// character 0 then).
    page_cursor: Option<selection::Cursor>,
    /// Sticky target column for `Up`/`Down` motions (`selection.rs`
    /// reads, never writes, this across vertical moves).
    sticky_column: usize,
    /// Set by `apply_action` for `scroll_down`/`scroll_up`/
    /// `document_start`/`document_end`; consumed once inside the
    /// `ScrollArea` closure (that's the only place `ui.scroll_with_delta`
    /// can be called from).
    pending_scroll: Option<egui::Vec2>,
    /// Last painted page rectangles, in screen coordinates. Page padding
    /// and gaps do not scale with zoom, so the scroll offset alone cannot
    /// identify the content point beneath the pointer.
    page_rects: Vec<egui::Rect>,
    viewport_rect: egui::Rect,
    pending_zoom_anchor: Option<ZoomAnchor>,
    close_requested: bool,
    /// Per-motion page/line/char feedback. Not rendered — `style.css`'s
    /// `.pdf-visual-status` is screen-reader-only (clip-path trick)
    /// except for `.is-notice` messages (see `notice`/`notice_until`
    /// below); this is the non-notice half, kept for parity/future
    /// accessibility hookup rather than a visible permanent status bar.
    status: String,
    /// Transient visible message (copy confirmations, errors) — matches
    /// `style.css`'s `.pdf-visual-status.is-notice`: a small floating
    /// box, not a permanent toolbar. `notice_until` is an `egui` time
    /// (seconds) past which it's no longer shown; set via `notify()`.
    notice: String,
    notice_until: f64,
    /// (active tab index, zoom) the currently-loaded `viewer` reflects.
    /// `None` forces a load on the first frame (texture creation needs a
    /// live `egui::Context`, unavailable at construction time).
    loaded_key: Option<usize>,
    /// `Some(direction)` while actively typing a `/`/`?` query (not yet
    /// confirmed with Enter or cancelled with Esc) — matches Vim's
    /// command-line mode: every other key (chrome shortcuts, the modal
    /// `Dispatcher`) is swallowed while this is set, and printable text
    /// events go into `search_query` instead.
    search_editing: Option<SearchDirection>,
    /// Unconfirmed command-line text. Escape discards it.
    search_draft: String,
    /// Last confirmed query; `n`/`N` repeat it.
    search_query: String,
    /// Which way `n` repeats; `N` repeats `search_direction.opposite()`
    /// for just that one jump, same as Vim.
    search_direction: SearchDirection,
    /// Full-document text matches, independent of raster cache. Sorted by
    /// `(page, start)` so `nearest_match_index` can wrap correctly.
    search_matches: Vec<SearchMatch>,
    /// Index into `search_matches` of the one `n`/`N` last jumped to —
    /// drawn more prominently than the others. `None` if nothing has
    /// been searched yet, or the last search found nothing.
    search_current: Option<usize>,
    /// Background render thread (see `render_worker.rs`); `progressive_render`
    /// submits jobs to it and drains finished results instead of calling
    /// `Viewer::render_page` synchronously, keeping PDFium rasterization
    /// and text extraction off the UI thread.
    render_worker: RenderWorker,
    /// `(tab, receiver)` for every tab whose real per-page sizes are
    /// still being measured on a background thread (see `load_viewer`
    /// and `viewer::Viewer::measure_page_sizes`) — a `Vec`, not a single
    /// slot, because opening a second large document while the first
    /// one's measurement is still running must not lose track of it.
    /// Drained every frame by `apply_pending_page_sizes`.
    pending_sizes: Vec<(usize, std::sync::mpsc::Receiver<Vec<document::PageSize>>)>,
    #[cfg(target_os = "macos")]
    document_events: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    sample_tab: bool,
}

impl SpikeApp {
    /// `initial_paths` is opened as tabs in order; if empty, falls back
    /// to a single plain example document (one page, ordinary prose —
    /// not the dev test fixtures, which include a two-column paper, a
    /// rotated page, and a blank-shapes page; opening all four by
    /// default is confusing for a normal launch and was the direct
    /// cause of an earlier live-testing round landing on the blank page
    /// instead of real text). The dev fixtures are still reachable
    /// explicitly (`cargo run -- assets/test-pdfs/two-column.pdf ...`)
    /// and are what `--dump` always uses regardless of this. Open
    /// failures are printed to stderr and skipped rather than aborting
    /// startup.
    fn new(library: &'static PdfLibrary, cfg: config::Config, initial_paths: &[String]) -> Self {
        let mut doc_app = app::App::new(library);
        let sample_tab = initial_paths.is_empty();
        let fallback;
        let paths: Vec<&str> = if initial_paths.is_empty() {
            fallback = bundled_resources()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("assets/test-pdfs/letter.pdf");
            vec![fallback.to_str().expect("bundle resource path is UTF-8")]
        } else {
            initial_paths.iter().map(String::as_str).collect()
        };
        for path in paths {
            if let Err(e) = doc_app.open(path) {
                eprintln!("ggpdf: could not open {path}: {e}");
            }
        }
        // App::open makes each newly-opened tab active (correct for
        // opening one file, e.g. a future "Open with" handoff), but that
        // means opening several files in a row at startup would leave
        // the *last* one active rather than the first.
        doc_app.jump_to_tab(0);

        Self {
            app: doc_app,
            cfg,
            dispatcher: input::Dispatcher::new(),
            zoom: SCALE,
            dark: false,
            viewer: None,
            viewers: Vec::new(),
            // The debug per-character box overlay was never part of the
            // real viewing experience — it's gate-validation scaffolding
            // (see PLAN.md). Left on by default this whole time, which
            // is almost certainly the "lines look weird" complaint.
            // Toggle with Cmd+B if needed again.
            show_boxes: false,
            active_page: None,
            cursor_idx: None,
            anchor_idx: None,
            page_cursor: None,
            sticky_column: 0,
            pending_scroll: None,
            page_rects: Vec::new(),
            viewport_rect: egui::Rect::NOTHING,
            pending_zoom_anchor: None,
            close_requested: false,
            status: String::new(),
            notice: String::new(),
            notice_until: 0.0,
            loaded_key: None,
            search_editing: None,
            search_draft: String::new(),
            search_query: String::new(),
            search_direction: SearchDirection::Forward,
            search_matches: Vec::new(),
            search_current: None,
            render_worker: RenderWorker::spawn(),
            pending_sizes: Vec::new(),
            #[cfg(target_os = "macos")]
            document_events: None,
            sample_tab,
        }
    }

    fn open_document_event(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        // A plain launch shows a sample. Replace it when Finder's cold-launch
        // event arrives after app creation instead of leaving an extra tab.
        if self.sample_tab && self.app.tab_count() == 1 {
            self.app.close_active();
            self.viewer = None;
            self.viewers.clear();
            self.loaded_key = None;
            self.pending_sizes.clear();
            self.render_worker = RenderWorker::spawn();
        }
        self.sample_tab = false;
        self.open_pdf_paths(paths, ctx);
    }

    /// Shows a transient visible message (copy confirmations, errors) —
    /// matches `visual.js`'s `message(text, true)`, auto-hidden after a
    /// few seconds, never a permanent toolbar.
    fn notify(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.notice = text.into();
        self.notice_until = ctx.input(|i| i.time) + 3.0;
    }

    /// Both the picker and OS drag-and-drop use the same open path. The
    /// document layer only changes tabs after PDFium has opened the file.
    fn open_pdf_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>, ctx: &egui::Context) {
        for path in paths {
            if !is_pdf_path(&path) {
                self.notify(ctx, format!("Not a PDF: {}", path.display()));
                continue;
            }
            match self.app.open(&path) {
                Ok(()) => {
                    self.dispatcher.reset();
                    self.pending_scroll = Some(egui::vec2(0.0, SCROLL_MAX));
                }
                Err(e) => self.notify(ctx, format!("Cannot open {}: {e}", path.display())),
            }
        }
    }

    /// Applies the effect of a resolved `Action`. Scroll/close/mode
    /// transitions are real; motions and text objects that need a
    /// keyboard cursor model (`selection.rs`, not built yet) report
    /// themselves as not implemented instead of silently doing nothing.
    /// `(page index, lines)` the keyboard cursor applies to: the mouse's
    /// `active_page` if one was clicked, else whichever page is
    /// currently nearest the viewport center (see `page_near`) — found
    /// live: entering Cursor mode after only scrolling (no click yet)
    /// always planted the cursor on page 0, regardless of how far the
    /// user had scrolled, because this previously fell back to a fixed
    /// `0` instead of asking where the view actually is. Falls back to
    /// `0` itself only before the first frame has painted anything (so
    /// `page_near` has nothing to go on yet).
    fn keyboard_page(&self) -> Option<usize> {
        let viewer = self.viewer.as_ref()?;
        if viewer.pages.is_empty() {
            return None;
        }
        let focused = self.page_near(self.viewport_rect.center()).map(|(p, _)| p);
        // Once the old cursor's page scrolls out of view, motions must
        // follow the viewport rather than remain pinned to that page.
        let active_visible = self.active_page.filter(|&p| self.page_rects.get(p).is_some_and(|r| r.intersects(self.viewport_rect)));
        let page = active_visible.or(focused).unwrap_or(0);
        Some(page.min(viewer.pages.len() - 1))
    }

    /// A cursor position belongs to its page. Reusing its line number
    /// after a scroll to another page can index beyond that page's lines.
    fn cursor_for_page(&self, page_idx: usize) -> selection::Cursor {
        let lines = self.viewer.as_ref().and_then(|v| Self::page_lines(v, page_idx));
        self.page_cursor
            .filter(|_| self.active_page == Some(page_idx))
            .filter(|cursor| lines.and_then(|l| l.get(cursor.line)).is_some_and(|line| cursor.character < line.chars.len()))
            .unwrap_or_else(|| self.initial_cursor_for_page(page_idx))
    }

    /// Where the keyboard cursor should appear in `page_idx` the first
    /// time a keyboard mode is entered this session (`page_cursor` still
    /// `None`) — the line nearest the viewport's current vertical
    /// center, not always line 0. Found live: `keyboard_page` picking
    /// the right *page* to land on wasn't enough on its own — every
    /// caller still hardcoded line 0/character 0 as `page_cursor`'s
    /// fallback, so the cursor landed at the top of that page even when
    /// the user had scrolled to, say, the middle of a long page (that
    /// page's own top wasn't even on screen). Falls back to line 0 if
    /// the page isn't rendered yet, has no lines, or `page_rects` has no
    /// entry for it yet (nothing painted this run).
    fn initial_cursor_for_page(&self, page_idx: usize) -> selection::Cursor {
        let fallback = selection::Cursor { line: 0, character: 0 };
        let Some(page) = self.viewer.as_ref().and_then(|v| v.pages.get(page_idx)).and_then(|p| p.rendered.as_ref()) else {
            return fallback;
        };
        if page.lines.is_empty() {
            return fallback;
        }
        let Some(&rect) = self.page_rects.get(page_idx) else {
            return fallback;
        };
        // Same page-local, render-scale-space conversion `char_at_pos`
        // uses for mouse clicks, just for the viewport's center y
        // instead of a pointer position.
        let inv_scale = page.render_scale / self.zoom;
        let focus_render_y = (self.viewport_rect.center().y - rect.min.y) * inv_scale;
        let line = page
            .lines
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let distance = |l: &selection::Line| l.rect().map_or(f32::MAX, |r| (focus_render_y - focus_render_y.clamp(r.top, r.bottom)).abs());
                distance(a).total_cmp(&distance(b))
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        selection::Cursor { line, character: 0 }
    }

    /// Converts a `selection::Cursor` (line, character) into a flat index
    /// into that page's `chars` — valid because `group_into_lines` never
    /// reorders characters, only splits the already-in-order slice, so
    /// concatenating all lines' lengths up to `cursor.line` plus
    /// `cursor.character` reproduces the original flat position.
    fn flat_index(lines: &[selection::Line], cursor: selection::Cursor) -> usize {
        lines[..cursor.line].iter().map(|l| l.chars.len()).sum::<usize>() + cursor.character
    }

    /// `None` if the page index is out of range or that page hasn't
    /// finished rendering yet (progressive_render hasn't reached it) —
    /// both treated the same by callers: nothing to act on this frame.
    fn page_lines(viewer: &Viewer, page_idx: usize) -> Option<&[selection::Line]> {
        viewer.pages.get(page_idx)?.rendered.as_ref().map(|r| r.lines.as_slice())
    }

    fn page_chars(viewer: &Viewer, page_idx: usize) -> Option<&[text::PositionedChar]> {
        viewer.pages.get(page_idx)?.rendered.as_ref().map(|r| r.chars.as_slice())
    }

    /// Index of (and screen rect for) whichever page's painted rect is
    /// vertically closest to `point`, from last frame's `page_rects` —
    /// `None` before the first frame has painted anything. Shared by
    /// `zoom_to` (anchor the zoom at the pointer or viewport center) and
    /// `progressive_render` (prioritize rendering/eviction around
    /// whatever's actually on screen, not document order).
    fn page_near(&self, point: egui::Pos2) -> Option<(usize, egui::Rect)> {
        self.page_rects
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let distance = |r: &egui::Rect| (point.y - point.y.clamp(r.top(), r.bottom())).abs();
                distance(a).total_cmp(&distance(b))
            })
            .map(|(i, &r)| (i, r))
    }

    /// Keep the point beneath the pointer stationary across a zoom.
    fn zoom_to(&mut self, new_zoom: f32, pointer: Option<egui::Pos2>) {
        let old_zoom = self.zoom;
        if new_zoom == old_zoom {
            return;
        }
        if self.pending_zoom_anchor.is_none() {
            let point = pointer.filter(|p| self.viewport_rect.contains(*p)).unwrap_or_else(|| self.viewport_rect.center());
            if let Some((page, rect)) = self.page_near(point) {
                self.pending_zoom_anchor = Some(ZoomAnchor {
                    page,
                    screen_point: point,
                    page_point: (point - rect.min) / old_zoom,
                });
            }
        }
        self.zoom = new_zoom;
    }

    fn apply_motion(&mut self, motion: selection::Motion, count: usize) {
        let Some(page_idx) = self.keyboard_page() else {
            self.status = "no document open".to_string();
            return;
        };
        let viewer = self.viewer.as_ref().unwrap();
        let Some(lines) = Self::page_lines(viewer, page_idx) else {
            self.status = "page not rendered yet".to_string();
            return;
        };
        if lines.is_empty() {
            self.status = "page has no extractable text".to_string();
            return;
        }
        let cursor = self.cursor_for_page(page_idx);

        let (page_idx, cursor) = if matches!(motion, selection::Motion::Up | selection::Motion::Down) {
            self.move_vertical_cross_page(page_idx, cursor, motion, count)
        } else {
            (page_idx, selection::move_cursor(lines, cursor, motion, count, &mut self.sticky_column))
        };

        let viewer = self.viewer.as_ref().unwrap();
        let Some(lines) = Self::page_lines(viewer, page_idx) else { return };
        self.page_cursor = Some(cursor);
        self.active_page = Some(page_idx);
        self.cursor_idx = Some(Self::flat_index(lines, cursor));
        self.update_cursor_status(page_idx, cursor);
    }

    /// `j`/`k` (`LineDown`/`LineUp`) crossing page boundaries once the
    /// current page's lines run out — mirrors `visual.js`'s `move()` for
    /// `j`/`k`, including skipping pages with no extractable text.
    /// Multi-count steps that cross more than one boundary (e.g. `9j`
    /// over several short pages) are handled by repeating the
    /// boundary-cross once per remaining step, same as the JS original.
    fn move_vertical_cross_page(
        &mut self,
        mut page_idx: usize,
        cursor: selection::Cursor,
        motion: selection::Motion,
        count: usize,
    ) -> (usize, selection::Cursor) {
        let viewer = self.viewer.as_ref().unwrap();
        let direction: isize = if motion == selection::Motion::Down { 1 } else { -1 };
        let mut line = cursor.line;
        let mut remaining = count;

        // A neighboring page not rendered yet (progressive_render hasn't
        // reached it) is treated the same as one with no extractable
        // text: skip past it rather than crossing into it — it'll be
        // rendered by the time the user scrolls there anyway.
        let is_empty = |p: usize| Self::page_lines(viewer, p).is_none_or(|l| l.is_empty());

        loop {
            let Some(lines) = Self::page_lines(viewer, page_idx) else { break };
            let available = if direction > 0 { lines.len() - 1 - line } else { line };
            let step = remaining.min(available);
            line = (line as isize + direction * step as isize) as usize;
            remaining -= step;
            if remaining == 0 {
                break;
            }

            let mut next_page = page_idx as isize + direction;
            while next_page >= 0 && (next_page as usize) < viewer.pages.len() && is_empty(next_page as usize) {
                next_page += direction;
            }
            if next_page < 0 || next_page as usize >= viewer.pages.len() || is_empty(next_page as usize) {
                break; // no further non-empty, rendered page in this direction
            }
            page_idx = next_page as usize;
            let Some(next_lines) = Self::page_lines(viewer, page_idx) else { break };
            line = if direction > 0 { 0 } else { next_lines.len() - 1 };
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }

        let character = Self::page_lines(viewer, page_idx)
            .and_then(|lines| lines.get(line))
            .map(|l| self.sticky_column.min(l.chars.len().saturating_sub(1)))
            .unwrap_or(0);
        (page_idx, selection::Cursor { line, character })
    }

    /// Textual confirmation that a motion actually ran, independent of
    /// the on-screen highlight (which a user can easily miss — a single
    /// highlighted character is subtle, and the cursor may have landed
    /// off-screen if the view was already scrolled elsewhere). Mirrors
    /// `visual.js`'s `message()` ("CURSOR · Page X · Line Y · ...").
    fn update_cursor_status(&mut self, page_idx: usize, cursor: selection::Cursor) {
        let viewer = self.viewer.as_ref().unwrap();
        let Some(lines) = Self::page_lines(viewer, page_idx) else { return };
        let ch = lines[cursor.line].chars.get(cursor.character).map(|c| c.ch).unwrap_or(' ');
        self.status = format!(
            "page {}/{} line {}/{} char {} [{}]",
            page_idx + 1,
            viewer.pages.len(),
            cursor.line + 1,
            lines.len(),
            cursor.character + 1,
            if ch.is_whitespace() { ' ' } else { ch },
        );
    }

    /// Applies `is`/`ip`: selects the text object as the mouse-selection
    /// range (`anchor_idx`..`cursor_idx`) on the keyboard's current page.
    fn apply_text_object(&mut self, bounds: Option<(selection::Cursor, selection::Cursor)>) {
        let Some(page_idx) = self.keyboard_page() else {
            self.status = "no document open".to_string();
            return;
        };
        let viewer = self.viewer.as_ref().unwrap();
        let Some(lines) = Self::page_lines(viewer, page_idx) else {
            self.status = "page not rendered yet".to_string();
            return;
        };
        let Some((start, end)) = bounds else {
            self.status = "text object: nothing found at cursor".to_string();
            return;
        };
        self.page_cursor = Some(end);
        self.active_page = Some(page_idx);
        self.anchor_idx = Some(Self::flat_index(lines, start));
        self.cursor_idx = Some(Self::flat_index(lines, end));
        self.update_cursor_status(page_idx, end);
    }

    fn apply_action(&mut self, action: config::Action, count: usize, ctx: &egui::Context) {
        use config::Action::*;
        let count = count.max(1);
        match action {
            // Confirmed backwards by live testing — flipped from the
            // original (unverified) guess.
            ScrollDown => self.pending_scroll = Some(egui::vec2(0.0, -SCROLL_STEP * count as f32)),
            ScrollUp => self.pending_scroll = Some(egui::vec2(0.0, SCROLL_STEP * count as f32)),
            DocumentStart => self.pending_scroll = Some(egui::vec2(0.0, SCROLL_MAX)),
            DocumentEnd => self.pending_scroll = Some(egui::vec2(0.0, -SCROLL_MAX)),
            Close => self.close_requested = true,
            Cancel => {
                self.cursor_idx = None;
                self.anchor_idx = None;
            }
            Yank => self.copy_selection(ctx),
            CursorMode => {
                // Mode switch already applied by Dispatcher::feed. Clear
                // any mark — this action also fires for "v"/"V" toggle-off
                // from Visual/VisualLine mode (config.rs binds those to
                // CursorMode), which should deselect.
                let from_visual = self.anchor_idx.is_some();
                self.anchor_idx = None;
                // Show page/line/char immediately on entry, not just
                // after the first motion — otherwise "c" alone looks like
                // it did nothing (found live: "cursor mode enter works
                // but nothing else").
                if let Some(page_idx) = self.page_near(self.viewport_rect.center()).map(|(p, _)| p).or_else(|| self.keyboard_page()) {
                    if !from_visual {
                        // A fresh entry starts where the reader is now,
                        // even if the previous cursor was on this page.
                        self.page_cursor = None;
                    }
                    self.active_page = Some(page_idx);
                    self.cursor_idx = None;
                    if let Some(lines) = Self::page_lines(self.viewer.as_ref().unwrap(), page_idx).filter(|l| !l.is_empty()) {
                        // Enter at the reading position even when an old
                        // cursor remains on a different page.
                        let cursor = self.cursor_for_page(page_idx);
                        self.page_cursor = Some(cursor);
                        self.active_page = Some(page_idx);
                        self.cursor_idx = Some(Self::flat_index(lines, cursor));
                        self.update_cursor_status(page_idx, cursor);
                    } else {
                        self.status = "page not rendered yet".to_string();
                    }
                }
            }
            NormalMode => {
                // Mode switch already applied by Dispatcher::feed; the
                // status line picks up dispatcher.mode() on every frame.
            }
            VisualMode | VisualLineMode => {
                // Mode switch already applied by Dispatcher::feed. If
                // there's already a mark, this is a v<->V toggle between
                // Visual and VisualLine while marking — config.rs binds
                // both to the other's action for that — so leave the
                // mark alone and only the mode label changes. Only a
                // fresh entry (from Cursor mode, no existing mark) should
                // anchor a new selection at the current cursor.
                // The anchor/cursor stored here are still character
                // positions in both Visual and VisualLine — the linewise
                // *display*/copy expansion to whole lines happens in
                // `display_selected_range`, not here.
                if self.anchor_idx.is_none() {
                    if let Some(page_idx) = self.keyboard_page() {
                        if let Some(lines) = Self::page_lines(self.viewer.as_ref().unwrap(), page_idx).filter(|l| !l.is_empty()) {
                            let cursor = self.cursor_for_page(page_idx);
                            self.page_cursor = Some(cursor);
                            self.active_page = Some(page_idx);
                            let idx = Self::flat_index(lines, cursor);
                            self.anchor_idx = Some(idx);
                            self.cursor_idx = Some(idx);
                            self.update_cursor_status(page_idx, cursor);
                        }
                    }
                }
            }
            CharacterLeft => self.apply_motion(selection::Motion::Left, count),
            CharacterRight => self.apply_motion(selection::Motion::Right, count),
            WordForward => self.apply_motion(selection::Motion::WordForward, count),
            WordBackward => self.apply_motion(selection::Motion::WordBackward, count),
            WordEnd => self.apply_motion(selection::Motion::WordEnd, count),
            BigWordForward => self.apply_motion(selection::Motion::BigWordForward, count),
            BigWordBackward => self.apply_motion(selection::Motion::BigWordBackward, count),
            BigWordEnd => self.apply_motion(selection::Motion::BigWordEnd, count),
            LineStart => self.apply_motion(selection::Motion::LineStart, count),
            LineFirstNonBlank => self.apply_motion(selection::Motion::LineFirstNonBlank, count),
            LineEnd => self.apply_motion(selection::Motion::LineEnd, count),
            LineDown => self.apply_motion(selection::Motion::Down, count),
            LineUp => self.apply_motion(selection::Motion::Up, count),
            // "y i s"/"y i p" in visual.js terms: select the text object
            // and copy it immediately (unlike the bare motions above,
            // which only move the cursor).
            CopySentence => {
                if let Some(page_idx) = self.keyboard_page() {
                    if let Some(lines) = Self::page_lines(self.viewer.as_ref().unwrap(), page_idx) {
                        let cursor = self.cursor_for_page(page_idx);
                        let bounds = selection::text_object_sentence(lines, cursor);
                        self.apply_text_object(bounds);
                        self.copy_selection(ctx);
                    }
                }
            }
            CopyParagraph => {
                if let Some(page_idx) = self.keyboard_page() {
                    if let Some(lines) = Self::page_lines(self.viewer.as_ref().unwrap(), page_idx).filter(|l| !l.is_empty()) {
                        let cursor = self.cursor_for_page(page_idx);
                        let bounds = selection::text_object_paragraph(lines, cursor);
                        self.apply_text_object(Some(bounds));
                        self.copy_selection(ctx);
                    }
                }
            }
            // "is"/"ip" in visual.js terms: select the text object but
            // don't yank it — lets the user keep extending/inspecting
            // the mark (or press y afterward) instead of copying
            // immediately.
            SelectSentence => {
                if let Some(page_idx) = self.keyboard_page() {
                    if let Some(lines) = Self::page_lines(self.viewer.as_ref().unwrap(), page_idx) {
                        let cursor = self.cursor_for_page(page_idx);
                        let bounds = selection::text_object_sentence(lines, cursor);
                        self.apply_text_object(bounds);
                    }
                }
            }
            SelectParagraph => {
                if let Some(page_idx) = self.keyboard_page() {
                    if let Some(lines) = Self::page_lines(self.viewer.as_ref().unwrap(), page_idx).filter(|l| !l.is_empty()) {
                        let cursor = self.cursor_for_page(page_idx);
                        let bounds = selection::text_object_paragraph(lines, cursor);
                        self.apply_text_object(Some(bounds));
                    }
                }
            }
            SearchNext => self.jump_search(count, false, ctx),
            SearchPrev => self.jump_search(count, true, ctx),
        }
    }

    /// Builds a fresh `Viewer` for the active tab (cheap now: `Viewer::
    /// load` only reads page 0's size plus a placeholder for every
    /// other page — see its own doc comment for why that changed — not
    /// rasterization, that's still `progressive_render`'s job). Called
    /// only for a tab with no cached `Viewer` yet; see
    /// `switch_to_active_tab`.
    fn load_viewer(&mut self, ctx: &egui::Context) {
        let result = (|| -> Result<Viewer, document::DocumentError> {
            let Some(active) = self.app.active() else {
                return Err(document::DocumentError::NoActiveDocument);
            };
            Viewer::load(&active.document)
        })();

        match result {
            Ok(viewer) => self.activate_viewer(viewer),
            Err(e) => {
                self.status = format!("load error: {e}");
                self.notify(ctx, format!("Cannot open document: {e}"));
                self.viewer = None;
            }
        }
    }

    /// Reads every page's real size on a dedicated one-shot background
    /// thread (see `viewer::Viewer::measure_page_sizes`'s doc comment
    /// for why `load_viewer` no longer does this inline), sent back
    /// through a channel this stores in `pending_sizes` keyed by `tab`.
    /// `apply_pending_page_sizes` drains it once it finishes.
    fn spawn_page_size_measurement(&mut self, tab: usize, document: Arc<document::Document<'static>>, ctx: egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ggpdf-measure-sizes".to_string())
            .spawn(move || {
                if let Ok(sizes) = Viewer::measure_page_sizes(&document) {
                    if tx.send(sizes).is_ok() {
                        ctx.request_repaint();
                    }
                }
            })
            .expect("spawn size-measurement thread");
        self.pending_sizes.push((tab, rx));
    }

    /// Applies any size measurement that's finished — corrects a tab's
    /// pages from `Viewer::load`'s page-0-sized placeholder to their
    /// real, possibly differing sizes. Routes by the stored `tab` index
    /// into whichever of `self.viewer`/`self.viewers` currently holds
    /// it, same pattern as `apply_render_result`; also shares that
    /// method's known gap (a `tab` index can go stale across a tab
    /// close — not reachable from the UI yet).
    fn apply_pending_page_sizes(&mut self) {
        let pending = std::mem::take(&mut self.pending_sizes);
        for (tab, rx) in pending {
            match rx.try_recv() {
                Ok(sizes) => {
                    let viewer = if self.loaded_key == Some(tab) { self.viewer.as_mut() } else { self.viewers.get_mut(tab).and_then(Option::as_mut) };
                    if let Some(viewer) = viewer {
                        viewer.apply_measured_sizes(&sizes);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => self.pending_sizes.push((tab, rx)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Measurement thread failed/panicked — pages keep
                    // page 0's placeholder size rather than retrying.
                }
            }
        }
    }

    /// Makes `viewer` (freshly built or recovered from `viewers`) the
    /// one on screen, resetting the per-tab mouse/keyboard cursor state
    /// and status line to match it.
    fn activate_viewer(&mut self, viewer: Viewer) {
        self.search_editing = None;
        self.search_draft.clear();
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = None;
        self.active_page = None;
        self.cursor_idx = None;
        self.anchor_idx = None;
        self.page_cursor = None;
        self.sticky_column = 0;
        let tab_label = self.app.tab_label(self.app.active_index()).unwrap_or("?");
        self.status = format!(
            "[{}/{}] {} — {} pages, zoom {:.0}%",
            self.app.active_index() + 1,
            self.app.tab_count(),
            tab_label,
            viewer.pages.len(),
            self.zoom / SCALE * 100.0,
        );
        self.viewer = Some(viewer);
    }

    /// Switches `self.viewer` to match `self.app.active_index()`, a cache
    /// lookaside in front of `load_viewer`: the outgoing tab's `Viewer`
    /// (with all its already-rasterized page textures) is parked in
    /// `viewers` rather than dropped, and the incoming tab's `Viewer` is
    /// taken back out if it was parked there earlier. Only a tab that
    /// has never been shown this run falls through to `load_viewer`'s
    /// full rebuild. No-op if the active tab is already loaded.
    fn switch_to_active_tab(&mut self, ctx: &egui::Context) {
        let key = self.app.active_index();
        if self.loaded_key == Some(key) {
            return;
        }
        if self.viewers.len() < self.app.tab_count() {
            self.viewers.resize_with(self.app.tab_count(), || None);
        }
        if let Some(old_key) = self.loaded_key {
            if let Some(slot) = self.viewers.get_mut(old_key) {
                *slot = self.viewer.take();
            }
        }
        match self.viewers.get_mut(key).and_then(Option::take) {
            Some(cached) => self.activate_viewer(cached),
            None => self.load_viewer(ctx),
        }
        self.loaded_key = Some(key);
        self.page_rects.clear();
        self.pending_zoom_anchor = None;
    }

    /// Submits not-yet-rendered pages to `render_worker` (see
    /// `viewer::Viewer`'s doc comment). `collect_render_results` uploads
    /// one completed result before layout. This runs after layout, so
    /// eviction never relies on the previous zoom/tab's page rects,
    /// keeping any single frame fast regardless of document
    /// length — opening a 300-page PDF shows page 1 instantly and fills
    /// in the rest over the next several frames instead of blocking the
    /// whole window until every page is decoded. PDFium rasterization
    /// and text extraction run on `render_worker`'s background thread,
    /// not here — only `collect_render_results` uploads completed textures
    /// (found live: "scrolling and zooming is slow" on
    /// bigger PDFs, traced to this method previously calling
    /// `Viewer::render_page` — PDFium decode, dark-filter math, and text
    /// extraction together — synchronously on the UI thread every frame
    /// there was unrendered work nearby).
    fn collect_render_results(&mut self, ctx: &egui::Context) {
        self.render_worker.set_repaint_context(ctx);
        self.apply_pending_page_sizes();
        if let Some(result) = self.render_worker.poll_one() {
            self.apply_render_result(result, ctx);
            ctx.request_repaint();
        }
    }

    /// Called after ScrollArea lays out pages, so its viewport and page
    /// rectangles refer to this frame, not to the old zoom or old tab.
    fn progressive_render(&mut self, ctx: &egui::Context) {
        // This PDF renders at ~27 MB raw pixels per page at 100% zoom
        // on a 2x display. The old radius 40 held up to 81 textures
        // (over 2 GB raw, before upload copies). Keep the visible page
        // and a few neighbors; evict everything farther away.
        const EVICT_KEEP_RADIUS: usize = 3;

        let Some(active) = self.app.active() else { return };
        let document = active.document.clone();
        let tab = self.app.active_index();
        let dark = self.dark;
        // Default (2048) is egui's conservative fallback when a backend
        // doesn't report its real limit; a live GPU backend reports the
        // actual one. Either way, `viewer::prepare_page` now clamps to it
        // per page instead of trusting the target scale blindly.
        let max_texture_side = ctx.input(|i| i.max_texture_side);
        let target_scale = target_render_scale(self.zoom, ctx.pixels_per_point());
        // This frame's actual scroll position, including tab restoration
        // and completed zoom anchoring. Evicting from old rectangles made
        // zoomed pages disappear until a fresh render completed.
        let current_page = self.page_near(self.viewport_rect.center()).map(|(page, _)| page).unwrap_or(0);
        // Read every frame, not just when submitting something new: the
        // worker picks its next job by comparing each *already queued*
        // job against this live value (see `render_worker.rs`), so a
        // fast scroll or page jump reprioritizes in-flight requests
        // immediately instead of only the next job submitted — this is
        // what fixes a slow-to-appear current page (and, since Cursor
        // mode needs the current page's text already extracted, Cursor
        // mode feeling stuck) after scrolling or jumping quickly past
        // many still-unrendered pages: previously a plain FIFO queue
        // kept processing those *earlier*, now-irrelevant requests
        // first, in the order they were submitted, before ever reaching
        // the page actually on screen now.
        self.render_worker.set_focus(tab, current_page);
        let Some(viewer) = self.viewer.as_mut() else { return };

        // See `page_needs_render`'s own doc comment for the staleness
        // rule; it's pulled out as a free function so this decision has
        // direct unit tests.
        let needs_render =
            |i: usize| page_needs_render(viewer.pages[i].rendered.as_ref().map(|r| (r.requested_scale, r.dark)), viewer.pages[i].pending, target_scale, dark);

        // Queue only this small window. The worker wakes egui after each
        // result, so no continuous repaint loop is needed while waiting.
        let candidates = render::nearby_pages_to_render(current_page, viewer.pages.len(), EVICT_KEEP_RADIUS, needs_render);
        for &idx in &candidates {
            viewer.pages[idx].pending = Some((target_scale, dark));
            self.render_worker.submit(tab, idx, render_worker::RenderJobSpec { requested_scale: target_scale, max_texture_side, dark, document: document.clone() });
        }
        for idx in render::pages_to_evict(current_page, viewer.pages.len(), |i| viewer.pages[i].rendered.is_some(), EVICT_KEEP_RADIUS) {
            viewer.pages[idx].rendered = None;
            // Drops a not-yet-started request for this page, if the
            // worker hasn't already picked it up — no point spending a
            // render on a page that's about to be evicted again the
            // next time it drifts this far from the viewport.
            self.render_worker.cancel(tab, idx);
            viewer.pages[idx].pending = None;
        }
        // Cancel queued pages too. The eviction scan above only visits
        // rendered pages; an off-screen pending page would otherwise
        // still rasterize and later upload a texture outside the window.
        for (idx, page) in viewer.pages.iter_mut().enumerate() {
            if idx.abs_diff(current_page) > EVICT_KEEP_RADIUS && page.pending.take().is_some() {
                self.render_worker.cancel(tab, idx);
            }
        }
        // The all-pages PDFium scan takes ~1.6 seconds on the reported
        // 486-page book. Keep it off the critical path for first paint:
        // start only after the focused page and its prefetched neighbors
        // have finished rendering. It still corrects mixed page sizes
        // once complete, without competing with startup rasterization.
        let nearby_ready = candidates.is_empty()
            && !viewer.pages.iter().enumerate().any(|(i, page)| i.abs_diff(current_page) <= EVICT_KEEP_RADIUS && page.pending.is_some());
        if nearby_ready && !viewer.sizes_requested && !viewer.pages.is_empty() {
            viewer.sizes_requested = true;
            self.spawn_page_size_measurement(tab, document, ctx.clone());
        }
    }

    /// Applies one finished background render job: uploads its bitmap as
    /// a texture (the one piece of `viewer::prepare_page`'s work that
    /// must happen on the UI thread, since it needs a live
    /// `egui::Context`) and stores it on whichever tab the job was
    /// submitted for — the active `self.viewer`, or a parked
    /// `self.viewers` slot if the user switched tabs while the job was
    /// in flight.
    ///
    /// No generation tagging (see `render_worker.rs`'s own doc comment):
    /// the bounds check below only guards against a tab that no longer
    /// exists (closing tabs isn't reachable from the UI yet, so this is
    /// defensive, not exercised) — it does *not* detect a tab *index*
    /// that now refers to a different document after an earlier close
    /// shifted indices. Revisit once `App::close_active` is wired up.
    fn apply_render_result(&mut self, result: render_worker::RenderResult, ctx: &egui::Context) {
        let render_worker::RenderResult { tab, prepared } = result;
        if tab >= self.app.tab_count() {
            return;
        }
        let viewer = if self.loaded_key == Some(tab) { self.viewer.as_mut() } else { self.viewers.get_mut(tab).and_then(Option::as_mut) };
        let Some(viewer) = viewer else { return };
        let page_idx = prepared.index;
        let Some(page) = viewer.pages.get_mut(page_idx) else { return };
        if page.pending != Some((prepared.requested_scale(), prepared.dark())) {
            return; // canceled or superseded while the worker was rendering
        }
        page.pending = None;
        if page.rendered.as_ref().is_some_and(|old| old.dark == prepared.dark() && old.requested_scale > prepared.requested_scale()) {
            return; // never replace a sharper visible texture with stale work
        }
        page.rendered = Some(prepared.into_rendered(ctx));
        if self.loaded_key == Some(tab)
            && self.active_page == Some(page_idx)
            && self.page_cursor.is_none()
            && self.dispatcher.mode() == config::Mode::Cursor
            && self.page_near(self.viewport_rect.center()).is_some_and(|(focused, _)| focused == page_idx)
        {
            // Cursor mode was entered while this page was still loading.
            // Finish placement when text becomes available; no extra key
            // press needed.
            self.apply_action(config::Action::CursorMode, 1, ctx);
        }
    }

    /// Extracts every page's text at one pixel per PDF point, without
    /// rasterizing. Geometry stays valid across texture eviction and zoom.
    fn run_search(&mut self) -> Result<(), document::DocumentError> {
        self.search_matches.clear();
        let query: Vec<char> = self.search_query.chars().collect();
        if query.is_empty() {
            return Ok(());
        }
        let document = &self.app.active().ok_or(document::DocumentError::NoActiveDocument)?.document;
        for page in 0..document.page_count() {
            let pdf_page = document.page(page)?;
            let chars = text::position_chars(&pdf_page.raw_chars()?, pdf_page.size(), pdf_page.rotation_degrees(), 1.0);
            let lines = selection::group_into_lines(&chars);
            for (start, end) in selection::find_matches(&chars, &query) {
                let spans = selection::selection_span_rects(&lines, start, end);
                let line = Self::line_containing_flat(&lines, start).unwrap_or(0);
                let offset: usize = lines[..line].iter().map(|line| line.chars.len()).sum();
                let cursor = selection::Cursor { line, character: start - offset };
                self.search_matches.push(SearchMatch { page: page as usize, start, cursor, spans });
            }
        }
        Ok(())
    }

    /// Confirms a `/`/`?` query typed via `search_editing` (Enter
    /// pressed): runs the search and jumps to the nearest match from
    /// wherever the keyboard/mouse cursor currently is, in `direction` —
    /// matches Vim's `/pattern<CR>` jumping from the cursor, not always
    /// restarting at the top of the document.
    fn confirm_search(&mut self, direction: SearchDirection, ctx: &egui::Context) {
        if self.search_draft.is_empty() && self.search_query.is_empty() {
            self.notify(ctx, "No previous search");
            return;
        }
        if !self.search_draft.is_empty() {
            self.search_query.clone_from(&self.search_draft);
        }
        self.search_direction = direction;
        if let Err(e) = self.run_search() {
            self.search_current = None;
            self.notify(ctx, format!("Search failed: {e}"));
            return;
        }
        if self.search_matches.is_empty() {
            self.search_current = None;
            self.notify(ctx, format!("Pattern not found: {}", self.search_query));
            return;
        }
        let start_page = self.active_page.or_else(|| self.page_near(self.viewport_rect.center()).map(|(p, _)| p)).unwrap_or(0);
        let idx = if let Some(start_char) = self.cursor_idx.filter(|_| self.active_page == Some(start_page)) {
            nearest_match_index(&self.search_matches, start_page, start_char, direction)
        } else {
            // Without a text cursor, include a match at the viewport's
            // first character rather than skipping it as "current".
            match direction {
                SearchDirection::Forward => self.search_matches.iter().position(|m| m.page >= start_page).unwrap_or(0),
                SearchDirection::Backward => self.search_matches.iter().rposition(|m| m.page <= start_page).unwrap_or(self.search_matches.len() - 1),
            }
        };
        self.search_current = Some(idx);
        self.scroll_to_match(idx);
        self.notify(ctx, format!("/{} [{}/{}]", self.search_query, idx + 1, self.search_matches.len()));
    }

    /// `n` (`reverse = false`) or `N` (`reverse = true`) — steps
    /// `search_current` by `count` matches, in `search_direction` or
    /// its opposite for just this one jump (`N`'s Vim semantics; it
    /// never changes `search_direction` itself).
    fn jump_search(&mut self, count: usize, reverse: bool, ctx: &egui::Context) {
        if self.search_matches.is_empty() {
            self.notify(ctx, if self.search_query.is_empty() { "No previous search".to_string() } else { "No search matches".to_string() });
            return;
        }
        let direction = if reverse { self.search_direction.opposite() } else { self.search_direction };
        let current = self.search_current.unwrap_or(0);
        let idx = step_match_index(current, self.search_matches.len(), count, direction);
        self.search_current = Some(idx);
        self.scroll_to_match(idx);
        self.notify(ctx, format!("/{} [{}/{}]", self.search_query, idx + 1, self.search_matches.len()));
    }

    /// Scrolls so `search_matches[idx]` lands at the viewport's vertical
    /// center — reuses the one-frame-stale `page_rects` lookup (same
    /// tolerance `zoom_to`/`page_near` already rely on) and the existing
    /// `pending_scroll`/`ui.scroll_with_delta` mechanism every other
    /// scroll action in this file already goes through, rather than
    /// adding a second scrolling code path.
    fn scroll_to_match(&mut self, idx: usize) {
        let Some(m) = self.search_matches.get(idx) else { return };
        if self.active_page != Some(m.page) || !matches!(self.dispatcher.mode(), config::Mode::Visual | config::Mode::VisualLine) {
            self.anchor_idx = None;
        }
        self.active_page = Some(m.page);
        self.cursor_idx = Some(m.start);
        self.page_cursor = Some(m.cursor);
        self.sticky_column = m.cursor.character;
        let Some(&rect) = self.page_rects.get(m.page) else { return };
        let Some(char_rect) = m.spans.first() else { return };
        let screen_y = rect.min.y + (char_rect.top + char_rect.bottom) * 0.5 * self.zoom;
        let shift = self.viewport_rect.center().y - screen_y;
        self.pending_scroll = Some(egui::vec2(0.0, shift));
    }

    fn selected_range(&self) -> Option<(usize, usize)> {
        let a = self.anchor_idx?;
        let b = self.cursor_idx?;
        Some((a.min(b), a.max(b)))
    }

    /// Which line a flat `page.chars` index falls in, or `None` if out
    /// of range. Mirrors the offset accounting `selection_span_rects`
    /// and `flat_index` already use.
    fn line_containing_flat(lines: &[selection::Line], flat: usize) -> Option<usize> {
        let mut offset = 0;
        for (i, line) in lines.iter().enumerate() {
            let len = line.chars.len();
            if len == 0 {
                continue;
            }
            if flat < offset + len {
                return Some(i);
            }
            offset += len;
        }
        None
    }

    /// `selected_range()`, expanded to whole lines when in VisualLine
    /// mode (matches `visual.js`'s `linewise` selection — previously
    /// VisualLine behaved identically to character-wise Visual, a known
    /// gap).
    fn display_selected_range(&self, page_idx: usize) -> Option<(usize, usize)> {
        let (a, b) = self.selected_range()?;
        if self.dispatcher.mode() != config::Mode::VisualLine {
            return Some((a, b));
        }
        let lines = Self::page_lines(self.viewer.as_ref()?, page_idx)?;
        let line_a = Self::line_containing_flat(lines, a)?;
        let line_b = Self::line_containing_flat(lines, b)?;
        let mut offset = 0;
        let (mut start, mut end) = (a, b);
        for (i, line) in lines.iter().enumerate() {
            let len = line.chars.len();
            if len == 0 {
                continue;
            }
            if i == line_a {
                start = offset;
            }
            if i == line_b {
                end = offset + len - 1;
            }
            offset += len;
        }
        Some((start, end))
    }

    fn copy_selection(&mut self, ctx: &egui::Context) {
        let Some(page_idx) = self.active_page else { return };
        let Some(viewer) = &self.viewer else { return };
        let Some(chars) = Self::page_chars(viewer, page_idx) else { return };
        let Some((a, b)) = self.display_selected_range(page_idx) else { return };
        let mut text: String = chars[a..=b].iter().map(|c| c.ch).collect();
        // visual.js appends a trailing newline for a linewise yank
        // (`this.linewise && this.anchor`) — matches `V` selecting
        // "whole lines" semantically, not just their visible chars.
        if self.dispatcher.mode() == config::Mode::VisualLine {
            text.push('\n');
        }
        ctx.copy_text(text.clone());
        eprintln!("copied: {text:?}");
        self.notify(ctx, "Copied selection to clipboard");
    }

    /// `pos` is in display space (the already-zoomed on-screen image);
    /// converted to the page's own `render_scale` space (what
    /// `page.chars`' rects are stored in — the scale `prepare_page`
    /// actually rasterized this page at, see `RenderedPage`) before
    /// comparing.
    fn char_at_pos(&self, page_idx: usize, pos: egui::Pos2) -> Option<usize> {
        let viewer = self.viewer.as_ref()?;
        let render_scale = viewer.pages.get(page_idx)?.rendered.as_ref()?.render_scale;
        let chars = Self::page_chars(viewer, page_idx)?;
        let inv_scale = render_scale / self.zoom;
        let render_pos = egui::pos2(pos.x * inv_scale, pos.y * inv_scale);
        chars.iter().position(|c| to_egui_rect(c.rect, 1.0).expand(1.0).contains(render_pos))
    }
}

/// Converts a `text::Rect` (in a page's own `render_scale` pixel space —
/// the space its texture and `page.chars` were computed in) into display
/// space at the given scale factor (`display_zoom / render_scale`).
fn to_egui_rect(r: text::Rect, scale: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(r.left * scale, r.top * scale), egui::pos2(r.right * scale, r.bottom * scale))
}

#[cfg(test)]
mod cursor_focus_tests {
    use super::*;

    fn line_with_rect(top: f32, bottom: f32) -> selection::Line {
        selection::Line { chars: vec![text::PositionedChar { ch: 'x', rect: text::Rect { left: 0.0, top, right: 10.0, bottom } }] }
    }

    fn app_with_lines(lines: Vec<selection::Line>) -> SpikeApp {
        let library = document::test_library();
        let mut app = SpikeApp::new(library, config::Config::default(), &["assets/test-pdfs/letter.pdf".to_string()]);
        let ctx = egui::Context::default();
        app.switch_to_active_tab(&ctx);
        let texture = ctx.load_texture("test", egui::ColorImage::new([1, 1], vec![egui::Color32::TRANSPARENT; 1]), egui::TextureOptions::LINEAR);
        let rendered = viewer::RenderedPage { texture, chars: Vec::new(), lines, render_scale: 1.0, requested_scale: 1.0, dark: false };
        app.viewer.as_mut().unwrap().pages[0].rendered = Some(rendered);
        app.zoom = 1.0;
        app.page_rects = vec![egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 1000.0))];
        // viewport_rect's center.y lands at 50 — right on the second line below.
        app.viewport_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        app
    }

    #[test]
    fn initial_cursor_picks_the_line_nearest_the_viewport_center_not_line_zero() {
        let app = app_with_lines(vec![line_with_rect(0.0, 10.0), line_with_rect(45.0, 55.0), line_with_rect(900.0, 910.0)]);
        let cursor = app.initial_cursor_for_page(0);
        assert_eq!(cursor.line, 1, "should land on the line nearest the viewport's vertical center, not line 0");
        assert_eq!(cursor.character, 0);
    }

    #[test]
    fn fresh_cursor_entry_repositions_within_the_same_page() {
        let mut app = app_with_lines(vec![line_with_rect(0.0, 10.0), line_with_rect(45.0, 55.0)]);
        let ctx = egui::Context::default();
        app.active_page = Some(0);
        app.page_cursor = Some(selection::Cursor { line: 0, character: 0 });
        app.dispatcher.feed(&app.cfg, "c");
        app.apply_action(config::Action::CursorMode, 1, &ctx);
        assert_eq!(app.page_cursor.unwrap().line, 1);
    }

    #[test]
    fn initial_cursor_falls_back_to_line_zero_when_the_page_has_not_rendered_yet() {
        let library = document::test_library();
        let mut app = SpikeApp::new(library, config::Config::default(), &["assets/test-pdfs/letter.pdf".to_string()]);
        let ctx = egui::Context::default();
        app.switch_to_active_tab(&ctx);
        let cursor = app.initial_cursor_for_page(0);
        assert_eq!((cursor.line, cursor.character), (0, 0));
    }

    fn app_on_second_page() -> (SpikeApp, egui::Context) {
        let library = document::test_library();
        let mut app = SpikeApp::new(library, config::Config::default(), &["assets/test-pdfs/two-column.pdf".to_string()]);
        let ctx = egui::Context::default();
        app.switch_to_active_tab(&ctx);
        assert!(app.viewer.as_ref().unwrap().pages.len() > 1);
        app.zoom = 1.0;
        app.page_rects = vec![
            egui::Rect::from_min_size(egui::pos2(0.0, -1000.0), egui::vec2(100.0, 900.0)),
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 900.0)),
        ];
        app.viewport_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        app.active_page = Some(0);
        app.page_cursor = Some(selection::Cursor { line: 999, character: 0 });
        (app, ctx)
    }

    #[test]
    fn entering_cursor_on_next_page_does_not_reuse_previous_pages_line() {
        let (mut app, ctx) = app_on_second_page();
        let doc = document::test_library().open("assets/test-pdfs/two-column.pdf").unwrap();
        app.viewer.as_mut().unwrap().render_page(&doc, 1, 1.0, 2048, false, &ctx);
        app.dispatcher.feed(&app.cfg, "c");
        app.apply_action(config::Action::CursorMode, 1, &ctx);
        assert_eq!(app.active_page, Some(1));
        assert!(app.page_cursor.unwrap().line < app.viewer.as_ref().unwrap().pages[1].rendered.as_ref().unwrap().lines.len());
    }

    #[test]
    fn entering_cursor_before_page_finishes_rendering_places_it_on_completion() {
        let (mut app, ctx) = app_on_second_page();
        app.dispatcher.feed(&app.cfg, "c");
        app.apply_action(config::Action::CursorMode, 1, &ctx);
        assert_eq!(app.active_page, Some(1));
        assert!(app.page_cursor.is_none());
        let doc = document::test_library().open("assets/test-pdfs/two-column.pdf").unwrap();
        let prepared = viewer::prepare_page(&doc, 1, 1.0, 2048, false);
        app.viewer.as_mut().unwrap().pages[1].pending = Some((1.0, false));
        app.apply_render_result(render_worker::RenderResult { tab: 0, prepared }, &ctx);
        assert_eq!(app.active_page, Some(1));
        assert!(app.page_cursor.is_some());
        assert!(app.cursor_idx.is_some());
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

/// Draws a 1px rectangle outline directly into pixel data, clamped to image
/// bounds. No GUI/display needed, so this can run headless and produce a
/// PNG for eyes-on review of box alignment.
fn draw_rect_outline(img: &mut image::RgbaImage, rect: text::Rect, color: [u8; 4]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (x0, y0, x1, y1) = (
        rect.left.round() as i32,
        rect.top.round() as i32,
        rect.right.round() as i32,
        rect.bottom.round() as i32,
    );
    let mut put = |x: i32, y: i32| {
        if x >= 0 && x < w && y >= 0 && y < h {
            img.put_pixel(x as u32, y as u32, image::Rgba(color));
        }
    };
    for x in x0..=x1 {
        put(x, y0);
        put(x, y1);
    }
    for y in y0..=y1 {
        put(x0, y);
        put(x1, y);
    }
}

/// Headless validation path: render page 0 of each fixture with and
/// without the rotation fix and save annotated PNGs under
/// `native/debug_out/`, so box alignment can be reviewed without a live
/// display (this sandbox has no screen-recording permission). Run with
/// `cargo run -- --dump`. Deliberately bypasses `viewer.rs`/`SpikeApp`
/// (no `egui::Context`/window exists here) and calls `document.rs`/
/// `text.rs` directly.
fn dump_fixtures(library: &'static PdfLibrary) -> Result<(), document::DocumentError> {
    std::fs::create_dir_all("debug_out").ok();

    for fixture in FIXTURES {
        for use_rotation_fix in [true, false] {
            let doc = library.open(fixture.path)?;
            let page = doc.page(0)?;
            let rotation_degrees = page.rotation_degrees();
            let size = page.size();
            let raw_chars = page.raw_chars()?;
            let mut image = page.render(SCALE)?;

            let rects: Vec<text::Rect> = if use_rotation_fix {
                text::position_chars(&raw_chars, size, rotation_degrees, SCALE)
                    .into_iter()
                    .map(|p| p.rect)
                    .collect()
            } else {
                // Naive mapping (ignores /Rotate entirely, flips Y only):
                // kept only as the gate's visual counter-example — see
                // PLAN.md "Biggest risk" — not part of the real text model
                // in text.rs.
                let rotated_h = size.height_pt;
                raw_chars
                    .iter()
                    .map(|raw| text::Rect {
                        left: raw.left * SCALE,
                        right: raw.right * SCALE,
                        top: (rotated_h - raw.top) * SCALE,
                        bottom: (rotated_h - raw.bottom) * SCALE,
                    })
                    .collect()
            };

            for r in &rects {
                draw_rect_outline(&mut image, *r, [255, 0, 0, 255]);
            }

            let suffix = if use_rotation_fix { "fixed" } else { "naive" };
            let out_path = format!("debug_out/{}_{suffix}.png", fixture.name);
            image.save(&out_path).expect("save png");
            println!("wrote {out_path} ({} chars)", rects.len());
        }
    }

    Ok(())
}

impl eframe::App for SpikeApp {
    // Only restore the outer window. Persisting egui's ScrollArea memory
    // across launches would reuse tab indices for potentially different
    // documents; per-tab scroll positions still work within one session.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(target_os = "macos")]
        if let Some(receiver) = &self.document_events {
            let batches: Vec<_> = receiver.try_iter().collect();
            for paths in batches {
                self.open_document_event(paths, &ctx);
            }
        }
        let mut resolved_actions: Vec<(config::Action, usize)> = Vec::new();
        // `copy_selection`/`confirm_search` call `ctx.copy_text`/
        // `self.notify` (which itself calls `ctx.input`) — real `egui::
        // Context` methods, not just `self` field writes. Calling either
        // directly from inside the `ctx.input(|i| {...})` closure below
        // would re-enter `Context`'s single internal write lock on the
        // same thread: confirmed by a standalone reproduction to
        // deadlock and (in a debug build) panic after egui's own 10s
        // deadlock-detection timeout, not just a theoretical risk. Every
        // other chrome shortcut here only touches plain `self` fields,
        // which is why this is the only spot that needs deferring —
        // collected into flags during the closure, same as
        // `resolved_actions` above, and acted on once it's closed.
        let mut copy_requested = false;
        let mut confirm_search: Option<SearchDirection> = None;
        let mut open_dialog_requested = false;
        let mut dropped_paths = Vec::new();
        ctx.input(|i| {
            dropped_paths.extend(i.raw.dropped_files.iter().map(|file| file.path().to_path_buf()));
            for event in &i.events {
                // While typing a `/`/`?` query, every other key below
                // (chrome shortcuts, the modal `Dispatcher`) is
                // swallowed — matches Vim's command-line mode, where
                // e.g. Ctrl+K zooming or entering Cursor mode mid-query
                // would make no sense.
                if let Some(direction) = self.search_editing {
                    match event {
                        egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => {
                            self.search_editing = None;
                            confirm_search = Some(direction);
                        }
                        egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                            self.search_editing = None;
                        }
                        egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                            self.search_draft.pop();
                        }
                        egui::Event::Text(text) if !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt => self.search_draft.push_str(text),
                        _ => {}
                    }
                    continue;
                }
                // `/` starts a forward search, `?` a backward one —
                // checked before the modal dispatcher further down so
                // they don't just dead-end as unbound keys (neither is
                // in `config.rs`'s token vocabulary). Shift is allowed
                // through (`?` is Shift+/ on a US keyboard), matching
                // every other plain-key check in this file.
                if let egui::Event::Text(text) = event {
                    if !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt && (text == "/" || text == "?") {
                        self.search_editing = Some(if text == "/" { SearchDirection::Forward } else { SearchDirection::Backward });
                        self.search_draft.clear();
                        self.dispatcher.reset();
                        continue;
                    }
                }

                // App-chrome shortcuts (tab/zoom/debug toggles) are kept
                // modifier-gated or on keys outside config.rs's token
                // vocabulary, so they never collide with a vim-style
                // single-letter binding — e.g. plain "b" is word_backward
                // in Cursor mode by default, so the debug box toggle lives
                // on Cmd+B instead of plain "b".
                if let egui::Event::Key { key, pressed: true, .. } = event {
                    match key {
                        egui::Key::Num1 => {
                            self.app.jump_to_tab(0);
                            self.dispatcher.reset();
                        }
                        egui::Key::Num2 => {
                            self.app.jump_to_tab(1);
                            self.dispatcher.reset();
                        }
                        egui::Key::Num3 => {
                            self.app.jump_to_tab(2);
                            self.dispatcher.reset();
                        }
                        egui::Key::Num4 => {
                            self.app.jump_to_tab(3);
                            self.dispatcher.reset();
                        }
                        egui::Key::Tab if i.modifiers.shift => {
                            self.app.prev_tab();
                            self.dispatcher.reset();
                        }
                        egui::Key::Tab => {
                            self.app.next_tab();
                            self.dispatcher.reset();
                        }
                        // Matches keys.js's plainCtrl table exactly:
                        // Ctrl+K/J zoom in/out, Ctrl+0 resets to 100%,
                        // Ctrl+U/D half-page scroll. keys.js checks
                        // ctrlKey && !metaKey && !altKey && !shiftKey;
                        // egui doesn't expose a held-modifiers check per
                        // match arm directly, so the `if` guards below do
                        // the equivalent (shift is allowed through, same
                        // as keys.js's plainCtrl which also permits it —
                        // that was a latent inconsistency there too, kept
                        // for fidelity rather than "fixed" here).
                        egui::Key::K if i.modifiers.ctrl => {
                            self.zoom_to(zoom_step(self.zoom, 1), i.pointer.hover_pos());
                        }
                        egui::Key::J if i.modifiers.ctrl => self.zoom_to(zoom_step(self.zoom, -1), i.pointer.hover_pos()),
                        egui::Key::Num0 if i.modifiers.ctrl => self.zoom_to(SCALE, i.pointer.hover_pos()),
                        egui::Key::U if i.modifiers.ctrl => self.pending_scroll = Some(egui::vec2(0.0, HALF_PAGE_SCROLL)),
                        egui::Key::D if i.modifiers.ctrl => self.pending_scroll = Some(egui::vec2(0.0, -HALF_PAGE_SCROLL)),
                        // In cursor and visual modes, "i" starts text objects
                        // such as "is" and "ip"; only Normal mode toggles dark.
                        egui::Key::I if dark_mode_shortcut(self.dispatcher.mode(), i.modifiers) => self.dark = !self.dark,
                        egui::Key::B if i.modifiers.command => self.show_boxes = !self.show_boxes,
                        egui::Key::C if i.modifiers.command => copy_requested = true,
                        egui::Key::O if i.modifiers.ctrl && !i.modifiers.command && !i.modifiers.alt => open_dialog_requested = true,
                        egui::Key::Escape => {
                            // Unconditional mouse-selection clear, on top
                            // of whatever config.rs's "Esc" binding (if
                            // any) resolves to via the dispatcher below.
                            self.cursor_idx = None;
                            self.anchor_idx = None;
                        }
                        _ => {}
                    }
                }

                // Modal key-sequence dispatch (config.rs-driven). Skip
                // while any chrome modifier is held so e.g. Cmd+C's text
                // artifact (if any) can't also feed the dispatcher a "c".
                if i.modifiers.command || i.modifiers.ctrl || i.modifiers.alt {
                    continue;
                }
                if let Some(token) = input::token_for_event(event) {
                    if let Some(resolved) = self.dispatcher.feed(&self.cfg, &token) {
                        resolved_actions.push(resolved);
                    }
                }
            }
        });
        if copy_requested {
            self.copy_selection(&ctx);
        }
        if let Some(direction) = confirm_search {
            self.confirm_search(direction, &ctx);
        }
        // Native file pickers and PDFium both interact with the OS; never
        // run them while holding egui's input lock.
        if open_dialog_requested {
            if let Some(paths) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).pick_files() {
                self.open_pdf_paths(paths, &ctx);
            }
        }
        self.open_pdf_paths(dropped_paths, &ctx);
        for (action, count) in resolved_actions {
            self.apply_action(action, count, &ctx);
        }
        if self.close_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // shell.css's ".pdf-shell-tabs" has `background: transparent`,
        // but that's transparent onto `body`'s own `--shell-bg` (the
        // canvas color) — not onto whatever sits further behind it. Here
        // the tab bar is painted before `CentralPanel`, straight onto
        // the window's default background, so an actually-transparent
        // fill left a near-black strip behind it that didn't match the
        // canvas color on every other edge (found live: "background not
        // black same as left and right margins"). Computed early so both
        // this panel and `CentralPanel` below paint the same color.
        let canvas_fallback = if self.dark { egui::Color32::from_gray(40) } else { egui::Color32::from_gray(237) };
        let canvas_hex = if self.dark { &self.cfg.appearance.canvas_dark } else { &self.cfg.appearance.canvas_light };
        let canvas_color = parse_hex_color(canvas_hex, canvas_fallback);

        // Matches shell.css's ".pdf-shell-tabs": a centered, numbered tab
        // row, canvas-colored background, small monospace text, current
        // tab in plain red/bold (no button chrome) — hidden entirely
        // with one document open ("no pane-bar height" for the
        // single-pane case).
        let shell_fg = if self.dark { egui::Color32::from_rgb(0xcc, 0xcc, 0xcc) } else { egui::Color32::from_rgb(0x33, 0x33, 0x33) };
        if self.app.tab_count() > 1 {
            egui::Panel::top("tabs")
                // `show_separator_line` is on by default and paints a
                // dim 1px line at the panel's bottom edge regardless of
                // `frame`'s fill (found live: "not transparent" — that
                // line, not a background fill, was what showed).
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(canvas_color).inner_margin(egui::Margin::symmetric(6, 3)))
                .show(ui, |ui| {
                    let labels: Vec<(String, bool)> = (0..self.app.tab_count())
                        .map(|index| {
                            let label = self.app.tab_label(index).unwrap_or("?");
                            (format!("{}: {}", index + 1, label), index == self.app.active_index())
                        })
                        .collect();
                    // `ui.horizontal` always claims the panel's full
                    // width (so `with_layout(.., Align::Center)` around
                    // it has nothing to center — found live: tabs sat
                    // flush left instead of centered). Center the row
                    // by hand instead: measure every label at the same
                    // font used to paint it, then pad the left edge by
                    // the leftover space.
                    let font_id = egui::FontId::monospace(12.0);
                    let spacing = ui.spacing().item_spacing.x;
                    let content_width: f32 = labels
                        .iter()
                        .map(|(text, _)| ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.clone(), font_id.clone(), shell_fg).size().x))
                        .sum::<f32>()
                        + spacing * labels.len().saturating_sub(1) as f32;
                    ui.horizontal(|ui| {
                        ui.add_space(((ui.available_width() - content_width) / 2.0).max(0.0));
                        for (index, (text, is_current)) in labels.into_iter().enumerate() {
                            let rich = egui::RichText::new(text).monospace().size(12.0);
                            let rich = if is_current { rich.color(egui::Color32::from_rgb(0xd9, 0x30, 0x25)).strong() } else { rich.color(shell_fg) };
                            if ui.add(egui::Label::new(rich).sense(egui::Sense::click())).clicked() {
                                self.app.jump_to_tab(index);
                                self.dispatcher.reset();
                            }
                        }
                    });
                });
        }

        // Matches style.css's ".pdf-visual-status.is-notice": a small
        // floating box, not a permanent toolbar — shown only briefly
        // after something notable happens (see `notify()`), not for
        // every cursor motion (those update `self.status` for parity/
        // future accessibility use, but aren't painted at all, matching
        // "Keep mode/cursor announcements accessible without an
        // on-screen mode bar.").
        if self.search_editing.is_some() || ctx.input(|i| i.time) < self.notice_until {
            let (bg, fg) = if self.dark {
                (egui::Color32::from_rgb(0x33, 0x33, 0x33), egui::Color32::from_rgb(0xee, 0xee, 0xee))
            } else {
                (egui::Color32::WHITE, egui::Color32::from_rgb(0x33, 0x33, 0x33))
            };
            let prompt = self.search_editing.map(|direction| format!("{}{}▏", if direction == SearchDirection::Forward { "/" } else { "?" }, self.search_draft));
            egui::Area::new(egui::Id::unique("notice"))
                .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(bg)
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .shadow(egui::epaint::Shadow { offset: [0, 1], blur: 6, spread: 0, color: egui::Color32::from_black_alpha(50) })
                        .show(ui, |ui| {
                            // Area reuses its previous frame's width. A wrapping
                            // label fits that width instead of growing with the
                            // query, stacking characters vertically. Extend
                            // measures one line and lets the area resize.
                            ui.add(egui::Label::new(egui::RichText::new(prompt.as_deref().unwrap_or(&self.notice)).monospace().size(12.0).color(fg)).wrap_mode(egui::TextWrapMode::Extend));
                        });
                });
            request_repaint(&ctx);
        }

        // zoom is intentionally not part of this key: it's a pure
        // display-space scale (see `target_render_scale`), never
        // triggers a reload.
        self.switch_to_active_tab(&ctx);
        self.collect_render_results(&ctx);

        let selection_color = parse_hex_color(
            &self.cfg.appearance.selection,
            egui::Color32::from_rgba_unmultiplied(66, 133, 244, 90),
        );
        let cursor_color = parse_hex_color(
            &self.cfg.appearance.cursor,
            egui::Color32::from_rgba_unmultiplied(120, 120, 120, 166),
        );
        let search_color = parse_hex_color(&self.cfg.appearance.search, egui::Color32::from_rgba_unmultiplied(255, 215, 0, 89));
        let search_current_color = parse_hex_color(&self.cfg.appearance.search_current, egui::Color32::from_rgba_unmultiplied(255, 140, 0, 166));
        let page_gap = self.cfg.appearance.page_gap as f32;

        let mut zoom_correction_pending = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(canvas_color))
            .show(ui, |ui| {
                // Captured before entering the ScrollArea: inside it,
                // ui.available_width() is effectively unbounded (that's
                // what lets horizontal scrolling work for a page wider
                // than the window), so it can't be used for centering
                // math — this is the actual visible viewport size.
                let viewport_size = ui.available_size();

                self.viewport_rect = ui.clip_rect();
                let mut page_rects = Vec::new();
                let mut corrected_zoom = false;

                egui::ScrollArea::both().id_salt(("pdf-scroll", self.app.active_index())).animated(false).show(ui, |ui| {
                    if let Some(delta) = self.pending_scroll.take() {
                        ui.scroll_with_delta(delta);
                    }

                    let Some(viewer) = &self.viewer else {
                        ui.label("no document open");
                        return;
                    };

                    let gutter_width = self.cfg.appearance.gutter_width as f32;
                    for page_idx in 0..viewer.pages.len() {
                        let page_view = &viewer.pages[page_idx];
                        // Display size from the page's point size directly
                        // (works whether or not it's rendered yet: for a
                        // rendered page this equals texture_px * display_scale,
                        // since the texture was rasterized at its own
                        // `render_scale`, computed from the live zoom —
                        // see `target_render_scale`).
                        let size = egui::vec2(page_view.size_pt.width_pt, page_view.size_pt.height_pt) * self.zoom;
                        // Centers the page horizontally when it's narrower
                        // than the viewport; 0 (flush left, scrollable)
                        // once zoomed past the window width — found live:
                        // pages were pinned to the left edge instead of
                        // centered.
                        let pad = ((viewport_size.x - size.x) / 2.0).max(0.0);

                        let Some(page) = &page_view.rendered else {
                            // Not rendered yet (progressive_render hasn't
                            // reached it) — reserve its layout space with
                            // a plain placeholder so scrolling/positions
                            // stay correct, and don't do any interaction/
                            // overlay work for it this frame.
                            ui.horizontal(|ui| {
                                ui.add_space(pad);
                                let (placeholder_rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                page_rects.push(placeholder_rect);
                                ui.painter_at(placeholder_rect).rect_filled(placeholder_rect, 0.0, canvas_color.linear_multiply(0.85));
                            });
                            ui.add_space(page_gap);
                            continue;
                        };
                        // This page's own `render_scale` (the scale
                        // `target_render_scale` computed when it was last
                        // rasterized, possibly clamped lower for an
                        // unusually large page — see `RenderedPage`), not
                        // a single scale shared by every page.
                        let display_scale = self.zoom / page.render_scale;
                        // Gutter line numbers (relative to the cursor's
                        // line, absolute on the cursor's own line — same
                        // convention as visual.js's lineNumber()) only on
                        // the page the keyboard cursor is actually on,
                        // only while Cursor/Visual/VisualLine is active.
                        // Simplification versus the original: that app
                        // computes relative numbers *across* pages too;
                        // this only does same-page relative numbering,
                        // to keep the gutter correct without needing a
                        // cross-page line-count pass every frame.
                        let show_gutter = matches!(self.dispatcher.mode(), config::Mode::Cursor | config::Mode::Visual | config::Mode::VisualLine)
                            && self.keyboard_page() == Some(page_idx);

                        // Recompute centering pad including the gutter's
                        // width when shown, so the page+gutter together
                        // stay centered rather than just the page.
                        let pad = ((viewport_size.x - size.x - if show_gutter { gutter_width } else { 0.0 }) / 2.0).max(0.0);

                        let (response, gutter_rect) = ui
                            .horizontal(|ui| {
                                ui.add_space(pad);
                                let gutter_rect = show_gutter.then(|| ui.allocate_exact_size(egui::vec2(gutter_width, size.y), egui::Sense::hover()).0);
                                let response = ui.add(egui::Image::new(&page.texture).fit_to_exact_size(size));
                                (response, gutter_rect)
                            })
                            .inner;
                        let rect = response.rect;
                        page_rects.push(rect);

                        if let Some(gutter_rect) = gutter_rect {
                            let cursor_line = self.page_cursor.filter(|_| self.active_page == Some(page_idx)).map(|c| c.line);
                            let gutter_painter = ui.painter_at(gutter_rect);
                            for (i, line) in page.lines.iter().enumerate() {
                                let Some(lr) = line.rect() else { continue };
                                let label = match cursor_line {
                                    Some(cl) if cl == i => (i + 1).to_string(),
                                    Some(cl) => (i as isize - cl as isize).unsigned_abs().to_string(),
                                    None => (i + 1).to_string(),
                                };
                                let y = gutter_rect.top() + (lr.top + lr.bottom) * 0.5 * display_scale;
                                gutter_painter.text(
                                    egui::pos2(gutter_rect.right() - 4.0, y),
                                    egui::Align2::RIGHT_CENTER,
                                    label,
                                    egui::FontId::monospace(11.0),
                                    // style.css: .pdf-visual-line color
                                    // light-dark(#777,#aaa), .is-current
                                    // #d93025 bold.
                                    if cursor_line == Some(i) {
                                        egui::Color32::from_rgb(0xd9, 0x30, 0x25)
                                    } else if self.dark {
                                        egui::Color32::from_rgb(0xaa, 0xaa, 0xaa)
                                    } else {
                                        egui::Color32::from_rgb(0x77, 0x77, 0x77)
                                    },
                                );
                            }
                        }

                        if let Some(pos) = response.hover_pos() {
                            let local = pos - rect.min.to_vec2();
                            if response.drag_started() {
                                self.active_page = Some(page_idx);
                                self.anchor_idx = self.char_at_pos(page_idx, local);
                                self.cursor_idx = self.anchor_idx;
                            } else if response.dragged() && self.active_page == Some(page_idx) {
                                self.cursor_idx = self.char_at_pos(page_idx, local);
                            } else if response.clicked() {
                                self.active_page = Some(page_idx);
                                self.anchor_idx = self.char_at_pos(page_idx, local);
                                self.cursor_idx = self.anchor_idx;
                            }
                        }

                        let painter = ui.painter_at(rect);
                        for (match_idx, m) in self.search_matches.iter().enumerate().filter(|(_, m)| m.page == page_idx) {
                            let current = self.search_current == Some(match_idx);
                            let color = if current { search_current_color } else { search_color };
                            for span in &m.spans {
                                let r = to_egui_rect(*span, self.zoom).translate(rect.min.to_vec2());
                                painter.rect_filled(r, 0.0, color);
                                if current {
                                    painter.rect_stroke(r, 0.0, (1.5, search_current_color), egui::StrokeKind::Outside);
                                }
                            }
                        }
                        if self.show_boxes {
                            for c in &page.chars {
                                let r = to_egui_rect(c.rect, display_scale).translate(rect.min.to_vec2());
                                painter.rect_stroke(
                                    r,
                                    0.0,
                                    (0.5, egui::Color32::from_rgb(0, 150, 255)),
                                    egui::StrokeKind::Outside,
                                );
                            }
                        }

                        if self.active_page == Some(page_idx) {
                            if let Some((a, b)) = self.display_selected_range(page_idx) {
                                // One merged rect per covered line, not
                                // one rect per character — filling each
                                // character's own (sometimes narrow or
                                // degenerate, especially for spaces) rect
                                // individually left visible gaps in the
                                // highlight, most visibly between words
                                // (found live: "visual selection has gaps
                                // between the words"). The cursor's own
                                // filled cell is drawn on top of this
                                // afterward, so it still stands out as a
                                // distinct marker within the bar.
                                for span in selection::selection_span_rects(&page.lines, a, b) {
                                    let r = to_egui_rect(span, display_scale).translate(rect.min.to_vec2());
                                    painter.rect_filled(r, 0.0, selection_color);
                                }
                            }
                            if let Some(idx) = self.cursor_idx {
                                if let Some(c) = page.chars.get(idx) {
                                    // A thin outline on one character was
                                    // easy to miss entirely (found live:
                                    // "cursor mode works but nothing else
                                    // visible") — a filled block cursor
                                    // plus a thicker outline is much
                                    // harder not to notice.
                                    let r = to_egui_rect(c.rect, display_scale).translate(rect.min.to_vec2());
                                    painter.rect_filled(r, 0.0, cursor_color);
                                    painter.rect_stroke(r, 0.0, (3.0, cursor_color), egui::StrokeKind::Outside);
                                }
                            }
                        }

                        ui.add_space(page_gap);
                    }
                    if let Some(anchor) = self.pending_zoom_anchor.take() {
                        if let Some(rect) = page_rects.get(anchor.page) {
                            ui.scroll_with_delta(anchor.correction(*rect, self.zoom));
                            corrected_zoom = true;
                        }
                    }
                });
                self.page_rects = page_rects;
                // egui applies scroll_with_delta after laying out and painting
                // the content. Do not show that uncorrected pass: run one more
                // pass using the updated scroll offset, like Sioyek's atomic
                // zoom-and-offset update before its next paint.
                if corrected_zoom {
                    zoom_correction_pending = true;
                    ctx.request_discard("zoom anchor scroll correction");
                }
            });
        if !zoom_correction_pending {
            self.progressive_render(&ctx);
        }
    }
}

/// Headless diagnostic for a live "it's slow" report on a specific file,
/// with no display needed: opens `path` and times exactly the steps
/// `progressive_render`/`viewer::prepare_page` do per page (PDFium
/// rasterization, raw text extraction, `text::position_chars`,
/// `selection::group_into_lines`), without any `egui`/GUI machinery in
/// the way, so a pathologically slow step shows up directly instead of
/// being guessed at from on-screen symptoms. Bypasses `app.rs`/
/// `viewer.rs` deliberately — same reasoning as `dump_fixtures` above.
fn profile_document(library: &'static PdfLibrary, path: &str) -> Result<(), document::DocumentError> {
    let open_start = std::time::Instant::now();
    let doc = library.open(path)?;
    println!("opened {path:?} in {:.1?}", open_start.elapsed());

    let count_start = std::time::Instant::now();
    let page_count = doc.page_count();
    println!("page_count() = {page_count} in {:.1?}", count_start.elapsed());

    // What `Viewer::load` itself now does on the UI thread: one page's
    // size, not all of them — this is what actually gates when the
    // window can first paint something.
    let load_start = std::time::Instant::now();
    let viewer = viewer::Viewer::load(&doc)?;
    println!("Viewer::load (UI-thread cost on open) took {:.1?} for {} pages", load_start.elapsed(), viewer.pages.len());

    // Every page's size/rotation — what the old `Viewer::load` did
    // inline on the UI thread, and what `measure_page_sizes` now does on
    // a background thread instead (see its own doc comment). Reported
    // here for comparison against the line above, not as something this
    // app still waits on to paint the first frame.
    let sizes_start = std::time::Instant::now();
    let sizes = viewer::Viewer::measure_page_sizes(&doc)?;
    let max_dims = sizes.iter().fold((0.0f32, 0.0f32), |acc, s| (acc.0.max(s.width_pt), acc.1.max(s.height_pt)));
    println!(
        "measure_page_sizes (background-thread cost) took {:.1?} for all {page_count} pages (largest page: {:.0}x{:.0}pt)",
        sizes_start.elapsed(),
        max_dims.0,
        max_dims.1
    );

    // A handful of representative pages: first, a couple in the middle,
    // and last — not every page, so this stays fast even on a huge
    // document, while still catching a page-size- or content-dependent
    // slowdown that a uniform fixture wouldn't show.
    let mut sample: Vec<u16> = vec![0, 19, 20, page_count / 4, page_count / 2, (page_count * 3) / 4, page_count.saturating_sub(1)];
    sample.sort_unstable();
    sample.dedup();

    for index in sample {
        if index >= page_count {
            continue;
        }
        let page = doc.page(index)?;
        let size = page.size();

        let render_start = std::time::Instant::now();
        // Same target this app actually uses at 100% zoom on a 2x
        // display: SCALE(2.0) * pixels_per_point(2.0) * RENDER_HEADROOM.
        let scale = target_render_scale(SCALE, 2.0);
        let image = page.render(scale)?;
        let render_elapsed = render_start.elapsed();

        let chars_start = std::time::Instant::now();
        let raw_chars = page.raw_chars()?;
        let chars_elapsed = chars_start.elapsed();

        let position_start = std::time::Instant::now();
        let positioned = text::position_chars(&raw_chars, size, page.rotation_degrees(), scale);
        let position_elapsed = position_start.elapsed();

        let lines_start = std::time::Instant::now();
        let lines = selection::group_into_lines(&positioned);
        let lines_elapsed = lines_start.elapsed();

        println!(
            "page {}/{page_count}: {:.0}x{:.0}pt -> {}x{} px | render {:.1?} | raw_chars {:.1?} ({} chars) | position_chars {:.1?} | group_into_lines {:.1?} ({} lines)",
            index + 1,
            size.width_pt,
            size.height_pt,
            image.width(),
            image.height(),
            render_elapsed,
            chars_elapsed,
            raw_chars.len(),
            position_elapsed,
            lines_elapsed,
            lines.len(),
        );
    }

    Ok(())
}

const WINDOW_TITLE: &str = "ggPdf";
const WINDOW_APP_ID: &str = "local.jens.ggpdf.native";

/// Decodes `icon.png` into the raw RGBA egui/winit wants for a window/app
/// icon. Checked first beside the executable (`bundled_resources()`, set
/// by `install-macos.sh`), then relative to the dev checkout — so `cargo
/// run` picks up the same icon without needing it installed anywhere.
///
/// Without this, `eframe::NativeOptions::viewport` has no icon set, and
/// `AppTitleIconSetter` (see eframe's `native/app_icon.rs`) falls back to
/// egui's own generic icon — then calls macOS's `NSApplication::
/// setApplicationIconImage` with *that* fallback, overwriting the Dock
/// icon the `.app` bundle's own `CFBundleIconFile` would otherwise show.
/// Found live: the Dock/app-switcher icon after launching the installed
/// app was egui's generic icon, not `ggPdf.icns`.
fn load_app_icon() -> Option<egui::IconData> {
    let candidates = [bundled_resources().map(|r| r.join("icon.png")), Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../icon.png"))];
    for path in candidates.into_iter().flatten() {
        if let Ok(image) = image::open(&path) {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            return Some(egui::IconData { rgba: rgba.into_raw(), width, height });
        }
    }
    None
}

fn native_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default().with_app_id(WINDOW_APP_ID).with_inner_size([1200.0, 800.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::NativeOptions {
        // eframe persists the outer window geometry under this stable ID.
        // This initial size matters only before any window state is saved.
        viewport,
        persist_window: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod window_options_tests {
    use super::*;

    #[test]
    fn window_geometry_is_saved_under_stable_id_without_forcing_maximize() {
        let options = native_options();
        assert_eq!(options.viewport.app_id.as_deref(), Some(WINDOW_APP_ID));
        assert_eq!(options.viewport.inner_size, Some(egui::vec2(1200.0, 800.0)));
        assert_ne!(options.viewport.maximized, Some(true));
        assert!(options.persist_window);
    }

    #[test]
    fn app_icon_loads_from_the_dev_checkout_so_cargo_run_also_gets_it() {
        let icon = load_app_icon().expect("icon.png should decode from the repo root");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}

fn main() -> eframe::Result<()> {
    let startup = std::time::Instant::now();
    let cfg = config::load_or_default();
    let config_elapsed = startup.elapsed();
    let total_bindings: usize = cfg.keys.values().map(|m| m.len()).sum();
    println!(
        "ggpdf: loaded config — {} modes, {total_bindings} key bindings, page_gap={}",
        cfg.keys.len(),
        cfg.appearance.page_gap
    );

    let lib_dir = env::var_os("PDFIUM_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| bundled_resources().map_or_else(|| PathBuf::from("pdfium"), |resources| resources.join("pdfium")));
    let library = PdfLibrary::load(&lib_dir)
        .unwrap_or_else(|e| panic!("failed to load PDFium from {lib_dir:?}: {e}. Set PDFIUM_LIB_DIR."));
    let library_elapsed = startup.elapsed();
    let library: &'static PdfLibrary = Box::leak(Box::new(library));

    let args: Vec<String> = env::args().skip(1).collect();
    let dump = args.iter().any(|a| a == "--dump");
    let profile_flag_index = args.iter().position(|a| a == "--profile");
    let profile_path = profile_flag_index.and_then(|i| args.get(i + 1)).cloned();
    let file_args: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| a.as_str() != "--dump" && *i != profile_flag_index.unwrap_or(usize::MAX) && *i != profile_flag_index.map_or(usize::MAX, |fi| fi + 1))
        .map(|(_, a)| a.clone())
        .collect();

    if dump {
        dump_fixtures(library).expect("dump failed");
        return Ok(());
    }
    if let Some(path) = profile_path {
        println!("startup before document: config {:.1?}, PDFium {:.1?}", config_elapsed, library_elapsed - config_elapsed);
        profile_document(library, &path).unwrap_or_else(|e| println!("profile failed: {e}"));
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
        let mut event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let repaint = Arc::new(std::sync::Mutex::new(None));
        macos_documents::install(sender, Arc::clone(&repaint));
        let mut app = eframe::create_native(
            WINDOW_TITLE,
            native_options(),
            Box::new(move |cc| {
                *repaint.lock().unwrap() = Some(cc.egui_ctx.clone());
                // Finder can send the cold-launch event before eframe creates
                // its UI. Consume it now, before deciding on a sample tab.
                let initial_events: Vec<_> = receiver.try_iter().flatten().collect();
                let mut paths = file_args.clone();
                paths.extend(initial_events.iter().map(|p| p.to_string_lossy().into_owned()));
                let mut viewer = SpikeApp::new(library, cfg, &paths);
                viewer.document_events = Some(receiver);
                Ok(Box::new(viewer))
            }),
            &event_loop,
        );
        let result = event_loop.run_app_on_demand(&mut app);
        result?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    eframe::run_native(
        WINDOW_TITLE,
        native_options(),
        Box::new(move |_cc| Ok(Box::new(SpikeApp::new(library, cfg, &file_args)))),
    )
}

#[cfg(test)]
mod viewer_smoke_tests {
    use super::*;

    #[test]
    fn full_page_size_scan_waits_for_initial_render() {
        let library = document::test_library();
        let mut app = SpikeApp::new(library, config::Config::default(), &["assets/test-pdfs/letter.pdf".to_string()]);
        let ctx = egui::Context::default();
        app.switch_to_active_tab(&ctx);
        app.progressive_render(&ctx);
        assert!(!app.viewer.as_ref().unwrap().sizes_requested);

        let doc = library.open("assets/test-pdfs/letter.pdf").unwrap();
        let target = target_render_scale(app.zoom, ctx.pixels_per_point());
        app.viewer.as_mut().unwrap().render_page(&doc, 0, target, 2048, false, &ctx);
        app.viewer.as_mut().unwrap().pages[0].pending = None;
        app.progressive_render(&ctx);
        assert!(app.viewer.as_ref().unwrap().sizes_requested);
    }

    #[test]
    fn late_low_resolution_result_does_not_replace_sharper_visible_page() {
        let library = document::test_library();
        let path = "assets/test-pdfs/letter.pdf".to_string();
        let mut app = SpikeApp::new(library, config::Config::default(), &[path.clone()]);
        let ctx = egui::Context::default();
        app.switch_to_active_tab(&ctx);
        let doc = library.open(&path).unwrap();
        app.viewer.as_mut().unwrap().render_page(&doc, 0, 1.5, 2048, false, &ctx);
        let original_id = app.viewer.as_ref().unwrap().pages[0].rendered.as_ref().unwrap().texture.id();
        let prepared = viewer::prepare_page(&doc, 0, 1.0, 2048, false);
        app.viewer.as_mut().unwrap().pages[0].pending = Some((1.0, false));
        app.apply_render_result(render_worker::RenderResult { tab: 0, prepared }, &ctx);
        let page = &app.viewer.as_ref().unwrap().pages[0];
        assert_eq!(page.rendered.as_ref().unwrap().texture.id(), original_id);
        assert!(page.pending.is_none());
    }

    #[test]
    fn canceled_render_result_does_not_restore_an_evicted_page() {
        let library = document::test_library();
        let path = "assets/test-pdfs/letter.pdf".to_string();
        let mut app = SpikeApp::new(library, config::Config::default(), &[path.clone()]);
        let ctx = egui::Context::default();
        app.switch_to_active_tab(&ctx);
        let doc = library.open(&path).expect("open doc");
        let prepared = viewer::prepare_page(&doc, 0, 1.0, 2048, false);
        app.viewer.as_mut().unwrap().pages[0].pending = None;
        app.apply_render_result(render_worker::RenderResult { tab: 0, prepared }, &ctx);
        assert!(app.viewer.as_ref().unwrap().pages[0].rendered.is_none());
    }

    #[test]
    fn viewer_load_is_cheap_and_render_page_fills_in_each_page() {
        let library = document::test_library();
        let doc = library.open("assets/test-pdfs/two-column.pdf").expect("open doc");
        let page_count = doc.page_count();
        assert!(page_count > 1, "fixture should be multi-page, got {page_count}");

        // load() alone (no ctx/scale needed) only reads sizes — nothing
        // rendered yet.
        let mut v = Viewer::load(&doc).expect("viewer load");
        assert_eq!(v.pages.len() as u16, page_count);
        assert!(v.pages.iter().all(|p| p.rendered.is_none()), "load() shouldn't rasterize anything");
        assert_eq!(v.next_unrendered(false), Some(0));

        let ctx = egui::Context::default();
        for i in 0..v.pages.len() {
            v.render_page(&doc, i, SCALE, 2048, false, &ctx);
            assert!(v.pages[i].rendered.as_ref().unwrap().texture.size_vec2().x > 0.0);
        }
        assert_eq!(v.next_unrendered(false), None, "every page should be rendered now");
    }

    #[test]
    fn dark_toggle_marks_an_already_rendered_page_as_unrendered_again() {
        let library = document::test_library();
        let doc = library.open("assets/test-pdfs/letter.pdf").expect("open doc");
        let mut v = Viewer::load(&doc).expect("viewer load");
        let ctx = egui::Context::default();

        v.render_page(&doc, 0, SCALE, 2048, false, &ctx);
        assert_eq!(v.next_unrendered(false), None, "page 0 is rendered for the light theme");
        assert_eq!(v.next_unrendered(true), Some(0), "but not yet for dark — toggling must re-render it");

        v.render_page(&doc, 0, SCALE, 2048, true, &ctx);
        assert!(v.pages[0].rendered.as_ref().unwrap().dark);
        assert_eq!(v.next_unrendered(true), None);
    }

    /// Regression test for the GPU-texture-size gap flagged in an
    /// earlier round: a page whose point size would rasterize past
    /// `max_texture_side` at the requested scale must come out no larger
    /// than that limit on either side, not panic on texture upload.
    #[test]
    fn render_page_clamps_to_max_texture_side() {
        let library = document::test_library();
        let doc = library.open("assets/test-pdfs/letter.pdf").expect("open doc");
        let mut v = Viewer::load(&doc).expect("viewer load");
        let ctx = egui::Context::default();

        // Letter-size at SCALE=2.0 is already ~1224x1584px; clamp hard
        // to something smaller than that to force the clamp path.
        v.render_page(&doc, 0, SCALE, 512, false, &ctx);
        let size = v.pages[0].rendered.as_ref().unwrap().texture.size_vec2();
        assert!(size.x <= 512.0 && size.y <= 512.0, "got {size:?}");
    }

    #[test]
    fn render_page_scale_changes_texture_size() {
        let library = document::test_library();
        let doc = library.open("assets/test-pdfs/ligatures.pdf").expect("open doc");
        // A bare egui::Context (no live GPU backend) defaults to a 2048px
        // max texture side; a real eframe/wgpu backend reports its own,
        // much larger limit. Stay under that so this test exercises the
        // scale math, not the test harness's texture-size cap.
        let ctx = egui::Context::default();

        let mut v1 = Viewer::load(&doc).expect("load 1");
        v1.render_page(&doc, 0, SCALE, 2048, false, &ctx);
        let mut v2 = Viewer::load(&doc).expect("load 2");
        v2.render_page(&doc, 0, SCALE * 1.2, 2048, false, &ctx);

        let w1 = v1.pages[0].rendered.as_ref().unwrap().texture.size_vec2().x;
        let w2 = v2.pages[0].rendered.as_ref().unwrap().texture.size_vec2().x;
        assert!((w2 - w1 * 1.2).abs() < 1.0, "1.2x scale should scale pixel width by 1.2x: {w1} vs {w2}");
    }
}
