//! Continuous multi-page scroll layout (Port order item 1, Document
//! viewer shell — the rendering/zoom slice of it). `selection.rs`'s
//! motions/text objects are single-page only (no cross-page `j`/`k`
//! yet); this module just groups each page's chars into `selection::Line`s
//! once at load time so `main.rs` doesn't re-run that grouping every
//! frame.
//!
//! Rendering is lazy/progressive, not eager: `load()` only reads each
//! page's size (cheap — no rasterization), so opening a document is fast
//! regardless of page count. `render_page()` rasterizes and caches one
//! page's bitmap + text geometry on demand; `main.rs` calls it a few
//! pages at a time per frame (see its `progressive_render`) instead of
//! blocking startup on decoding every page up front.

use crate::document::{Document, DocumentError, PageSize};
use crate::selection::{self, Line};
use crate::text::{self, PositionedChar};
use eframe::egui;
use image::Pixel;

pub struct RenderedPage {
    pub texture: egui::TextureHandle,
    /// Same bitmap with `apply_dark_filter` baked in, uploaded alongside
    /// `texture` from the same PDFium decode — so toggling dark mode on
    /// an already-rendered page just switches which of these two
    /// textures `ui.rs` draws. No new PDFium render, text extraction, or
    /// even a fresh filter pass is needed on a theme toggle, since both
    /// textures are already sitting in GPU memory.
    pub dark_texture: egui::TextureHandle,
    pub chars: Vec<PositionedChar>,
    pub lines: Vec<Line>,
    /// Pixels-per-point this page was actually rasterized at. Usually
    /// the caller's requested scale, but `render_page` clamps it down
    /// for a page large enough that the full-resolution bitmap would
    /// exceed the backend's `max_texture_side` (previously unchecked —
    /// an unusually large/landscape page at a high zoom level could
    /// panic on texture upload). `main.rs` derives each page's
    /// display-space scale factor from this instead of assuming every
    /// page shares one constant.
    pub render_scale: f32,
    /// What `render_scale` would have been without the `max_texture_side`
    /// clamp above — i.e. what was actually asked for. `main.rs`'s
    /// `progressive_render` compares a fresh zoom-driven target against
    /// *this*, not `render_scale`, to decide whether a page needs a
    /// sharper re-render: comparing against the (possibly clamped)
    /// `render_scale` instead would re-request the same clamped result
    /// forever for an oversized page, since the clamp never lets
    /// `render_scale` reach the target.
    pub requested_scale: f32,
}

/// The CPU-only half of rendering a page — rasterizing via PDFium,
/// applying the dark filter, and extracting positioned text — with no
/// `egui::Context` dependency, so it can run on `render_worker.rs`'s
/// background thread. `into_rendered` does the remaining GPU-upload step
/// (`egui::Context::load_texture`), which must stay on the thread that
/// owns the `Context`.
pub struct PreparedPage {
    pub index: usize,
    image: image::RgbaImage,
    /// `image` with `apply_dark_filter` already applied, computed once
    /// here from the same decode rather than on demand later — see
    /// `RenderedPage::dark_texture`'s own doc comment for why both
    /// themes are always prepared together.
    dark_image: image::RgbaImage,
    chars: Vec<PositionedChar>,
    lines: Vec<Line>,
    render_scale: f32,
    requested_scale: f32,
}

impl PreparedPage {
    /// Readable before `into_rendered` consumes `self` — `main.rs`'s
    /// `apply_render_result` needs this to decide whether this result
    /// still matches the page's `pending` request (see that field's own
    /// doc comment) before clearing it.
    pub fn requested_scale(&self) -> f32 {
        self.requested_scale
    }

    fn empty(index: usize, requested_scale: f32) -> PreparedPage {
        let image = image::RgbaImage::new(1, 1);
        let dark_image = image.clone();
        PreparedPage { index, image, dark_image, chars: Vec::new(), lines: Vec::new(), render_scale: requested_scale, requested_scale }
    }

    /// Uploads both already-decoded bitmaps as textures. Cheap relative
    /// to `prepare_page` (no PDFium/image-filter work left, just two GPU
    /// buffer copies) — safe to call on the UI thread once per result.
    pub fn into_rendered(self, ctx: &egui::Context) -> RenderedPage {
        let upload = |suffix: &str, image: &image::RgbaImage| {
            let (w, h) = (image.width() as usize, image.height() as usize);
            let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], image.as_raw());
            ctx.load_texture(format!("page-{}{}", self.index, suffix), color_image, egui::TextureOptions::LINEAR)
        };
        let texture = upload("", &self.image);
        let dark_texture = upload("-dark", &self.dark_image);
        RenderedPage { texture, dark_texture, chars: self.chars, lines: self.lines, render_scale: self.render_scale, requested_scale: self.requested_scale }
    }
}

/// One viewport-sized window of a page, single theme. Pixel window
/// (`x`, `y`, `width`, `height`) is in the full page bitmap at `scale`.
pub struct PreparedTile {
    pub image: image::RgbaImage,
    pub x: i32,
    pub y: i32,
    pub scale: f32,
}

/// Renders just a pixel window of page `index`, applying the dark filter
/// only when `dark`. Window is clamped to the page bitmap. No text work.
pub fn prepare_tile(doc: &Document, index: usize, scale: f32, window: (i32, i32, i32, i32), dark: bool) -> Result<PreparedTile, DocumentError> {
    let page = doc.page(index as u16)?;
    let size = page.size();
    let full_w = (size.width_pt * scale) as i32;
    let full_h = (size.height_pt * scale) as i32;
    let x = window.0.clamp(0, full_w.max(1) - 1);
    let y = window.1.clamp(0, full_h.max(1) - 1);
    let width = window.2.min(full_w - x).max(1);
    let height = window.3.min(full_h - y).max(1);
    let mut image = page.render_region(scale, x, y, width, height)?;
    if dark {
        apply_dark_filter(&mut image);
    }
    Ok(PreparedTile { image, x, y, scale })
}

/// Rasterizes page `index` and extracts its text geometry — the part of
/// `Viewer::render_page` with no `egui::Context` dependency, factored
/// out so `render_worker.rs`'s background thread can call it directly.
/// `requested_scale` is clamped down, per page, so neither dimension of
/// the rasterized bitmap exceeds `max_texture_side` (see
/// `RenderedPage::requested_scale`'s own doc comment for why the
/// pre-clamp value is kept too).
///
/// Always decodes once and derives both the light and dark bitmap from
/// that single PDFium render — previously this only produced whichever
/// theme's bitmap the caller currently wanted, so toggling dark mode
/// re-ran the PDFium render and text extraction for every on-screen page
/// just to apply a cheap per-pixel recolor. The dark filter itself is
/// fast; the PDFium render and `FPDFText_*` extraction are not, and
/// neither depends on theme.
pub fn prepare_page(doc: &Document, index: usize, requested_scale: f32, max_texture_side: usize) -> PreparedPage {
    let result = (|| -> Result<PreparedPage, DocumentError> {
        let page = doc.page(index as u16)?;
        let size = page.size();
        let rotation_degrees = page.rotation_degrees();

        let max_side = max_texture_side as f32;
        let scale = requested_scale.min(max_side / size.width_pt.max(1.0)).min(max_side / size.height_pt.max(1.0));

        let image = page.render(scale)?;
        let mut dark_image = image.clone();
        apply_dark_filter(&mut dark_image);

        let raw_chars = page.raw_chars()?;
        let chars = text::position_chars(&raw_chars, size, rotation_degrees, scale);
        let lines = selection::group_into_lines(&chars);

        Ok(PreparedPage { index, image, dark_image, chars, lines, render_scale: scale, requested_scale })
    })();

    result.unwrap_or_else(|_| PreparedPage::empty(index, requested_scale))
}

/// Approximates the old app's CSS `filter: invert(0.9) hue-rotate(180deg)`
/// (`theme.css`'s `--preview-dark-page-filter`) directly on the
/// rasterized page bitmap — previously dark mode only recolored the
/// canvas background; the page bitmap stayed whatever PDFium rendered
/// (usually white), a flagged gap from the `viewer.rs` step. The two
/// matrices below are the CSS Filter Effects spec's own definitions for
/// `invert()` and `hue-rotate()`, applied in the filter list's order
/// (invert first, then hue-rotate), not reinvented; `hue-rotate(180deg)`
/// is pre-evaluated (`cosA = -1`, `sinA = 0`) since the angle is fixed.
/// Alpha is untouched.
pub fn apply_dark_filter(image: &mut image::RgbaImage) {
    const INVERT_FACTOR: f32 = 1.0 - 2.0 * 0.9;
    const INVERT_OFFSET: f32 = 0.9;
    const HUE_ROTATE_180: [[f32; 3]; 3] = [[-0.574, 1.430, 0.144], [0.426, 0.430, 0.144], [0.426, 1.430, -0.856]];

    for pixel in image.pixels_mut() {
        let c = pixel.channels_mut();
        let inverted = [0, 1, 2].map(|i| (c[i] as f32 / 255.0) * INVERT_FACTOR + INVERT_OFFSET);
        for (channel, row) in c.iter_mut().zip(HUE_ROTATE_180) {
            let value = row[0] * inverted[0] + row[1] * inverted[1] + row[2] * inverted[2];
            *channel = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

pub struct PageView {
    /// May briefly be a placeholder (page 0's size, copied to every
    /// page) rather than this page's real size — see `Viewer::load`'s
    /// doc comment. Corrected in place once `apply_measured_sizes` runs.
    pub size_pt: PageSize,
    /// `None` until `Viewer::render_page` has rasterized this page.
    pub rendered: Option<RenderedPage>,
    /// `Some(requested_scale)` while `main.rs`'s background render
    /// worker has a job in flight for this page — set when the job is
    /// submitted, cleared when its result is applied (see `main.rs`'s
    /// `apply_render_result`), so `progressive_render` doesn't submit
    /// the same page twice while waiting on the first job.
    pub pending: Option<f32>,
}

/// Every page of one open document. `load()` populates sizes only;
/// bitmaps/text geometry fill in progressively via `render_page()`.
pub struct Viewer {
    pub pages: Vec<PageView>,
    /// Start the full-document size scan only after nearby pages have
    /// rendered. The scan reads hundreds of pages through the same PDFium
    /// bindings as the renderer and otherwise competes with first paint.
    pub sizes_requested: bool,
}

impl Viewer {
    /// Cheap: page sizes and rotation only, no PDFium rasterization — so
    /// opening even a long document is fast. Call `render_page` to
    /// actually decode a page's bitmap and text.
    /// Reads only page 0's real size (needed immediately — it's the
    /// first thing shown) and gives every other page that same size as
    /// a placeholder, rather than reading every page's size up front.
    /// `main.rs` measures the rest on a background thread (see
    /// `measure_page_sizes`) and calls `apply_measured_sizes` once that
    /// finishes, correcting any page whose placeholder was wrong.
    ///
    /// Profiled live on a 486-page technical book: the old version of
    /// this function — a loop opening *every* page just to read its
    /// `/MediaBox` — took 1.5 *seconds* of synchronous UI-thread time,
    /// running before the very first frame could paint anything. That's
    /// a direct, measured cause of a multi-second freeze on opening a
    /// large document, entirely separate from — and upstream of —
    /// `progressive_render`'s own background-worker rendering: a
    /// document with 1,000 pages and a document with 10 pages cost
    /// exactly the same to show *this* way (one page read), where they
    /// didn't before.
    pub fn load(doc: &Document) -> Result<Viewer, DocumentError> {
        let page_count = doc.page_count();
        if page_count == 0 {
            return Ok(Viewer { pages: Vec::new(), sizes_requested: false });
        }
        let first_size = doc.page(0)?.size();
        let pages = (0..page_count).map(|_| PageView { size_pt: first_size, rendered: None, pending: None }).collect();
        Ok(Viewer { pages, sizes_requested: false })
    }

    /// Reads every page's real size — the same per-page work the old
    /// `load` did inline, kept available as its own function so
    /// `main.rs` can run it on a background thread instead (see
    /// `load`'s own doc comment for why that matters). Pure PDFium
    /// reads, no `egui::Context` dependency, same shape as
    /// `prepare_page`.
    pub fn measure_page_sizes(doc: &Document) -> Result<Vec<PageSize>, DocumentError> {
        (0..doc.page_count()).map(|index| doc.page(index).map(|p| p.size())).collect()
    }

    /// Overwrites every page's `size_pt` with a freshly measured value
    /// — called once `measure_page_sizes` finishes. A no-op (not a
    /// panic) if `sizes.len()` doesn't match `self.pages.len()`,
    /// defensive against a measurement meant for a document that's
    /// since changed shape somehow — shouldn't happen in practice,
    /// since an already-open `PdfDocument`'s page count can't change.
    pub fn apply_measured_sizes(&mut self, sizes: &[PageSize]) {
        if sizes.len() != self.pages.len() {
            return;
        }
        for (page, &size) in self.pages.iter_mut().zip(sizes) {
            page.size_pt = size;
        }
    }

    /// Index of the first page not rendered yet, if any. Superseded in
    /// `main.rs`'s `progressive_render` by `render::pages_to_render`'s
    /// viewport-priority scan (this "first in document order" scan
    /// would defeat the point of that prioritization, and would also
    /// never report "done" once `progressive_render`'s eviction is
    /// active — see its own comment); kept as a simple, direct check
    /// for tests.
    #[cfg(test)]
    pub fn next_unrendered(&self) -> Option<usize> {
        self.pages.iter().position(|p| p.rendered.is_none())
    }

    /// Rasterizes and caches page `index`. No-op if already rendered —
    /// both themes are prepared together (see `prepare_page`'s own doc
    /// comment), so there's no theme to re-check here the way there used
    /// to be. A render failure is recorded as an empty page (see
    /// `PreparedPage::empty`) rather than left to retry forever.
    ///
    /// `pixels_per_point` is clamped down, per page, so neither
    /// dimension of the rasterized bitmap exceeds `max_texture_side` —
    /// without this, an unusually large/landscape page at a high zoom
    /// level could panic on texture upload (previously unchecked).
    ///
    /// Superseded in `main.rs`'s `progressive_render` by submitting a
    /// `render_worker::RenderJob` and uploading the result via
    /// `PreparedPage::into_rendered` instead — this synchronous,
    /// UI-thread-blocking path is kept for tests, which don't need a
    /// background thread for a single-page smoke check.
    #[cfg(test)]
    pub fn render_page(&mut self, doc: &Document, index: usize, pixels_per_point: f32, max_texture_side: usize, ctx: &egui::Context) {
        if self.pages[index].rendered.is_some() {
            return;
        }
        let prepared = prepare_page(doc, index, pixels_per_point, max_texture_side);
        self.pages[index].rendered = Some(prepared.into_rendered(ctx));
    }
}

#[cfg(test)]
mod page_size_tests {
    use super::*;
    use crate::document::test_library;

    #[test]
    fn load_uses_page_zeros_size_as_a_placeholder_for_every_page() {
        let library = test_library();
        let doc = library.open("assets/test-pdfs/two-column.pdf").expect("open doc");
        let page_count = doc.page_count();
        assert!(page_count > 1, "fixture should be multi-page, got {page_count}");

        let v = Viewer::load(&doc).expect("viewer load");
        let page0 = doc.page(0).expect("page 0").size();
        for page in &v.pages {
            assert_eq!((page.size_pt.width_pt, page.size_pt.height_pt), (page0.width_pt, page0.height_pt), "every page should start out sized like page 0");
        }
    }

    #[test]
    fn measure_page_sizes_and_apply_measured_sizes_correct_the_placeholder() {
        let library = test_library();
        let doc = library.open("assets/test-pdfs/two-column.pdf").expect("open doc");
        let page_count = doc.page_count();

        let mut v = Viewer::load(&doc).expect("viewer load");
        let real_sizes: Vec<PageSize> = (0..page_count).map(|i| doc.page(i).expect("page").size()).collect();

        let measured = Viewer::measure_page_sizes(&doc).expect("measure sizes");
        assert_eq!(measured.len(), real_sizes.len());
        for (m, real) in measured.iter().zip(&real_sizes) {
            assert_eq!((m.width_pt, m.height_pt), (real.width_pt, real.height_pt));
        }

        v.apply_measured_sizes(&measured);
        for (page, real) in v.pages.iter().zip(&real_sizes) {
            assert_eq!((page.size_pt.width_pt, page.size_pt.height_pt), (real.width_pt, real.height_pt));
        }
    }

    #[test]
    fn apply_measured_sizes_is_a_noop_on_a_length_mismatch() {
        let library = test_library();
        let doc = library.open("assets/test-pdfs/letter.pdf").expect("open doc");
        let mut v = Viewer::load(&doc).expect("viewer load");
        let before: Vec<(f32, f32)> = v.pages.iter().map(|p| (p.size_pt.width_pt, p.size_pt.height_pt)).collect();

        v.apply_measured_sizes(&[]);

        let after: Vec<(f32, f32)> = v.pages.iter().map(|p| (p.size_pt.width_pt, p.size_pt.height_pt)).collect();
        assert_eq!(before, after, "a mismatched measurement must not touch any page's size");
    }
}

#[cfg(test)]
mod dark_filter_tests {
    use super::*;

    fn filtered_pixel(rgb: [u8; 3]) -> [u8; 3] {
        let mut image = image::RgbaImage::new(1, 1);
        image.put_pixel(0, 0, image::Rgba([rgb[0], rgb[1], rgb[2], 200]));
        apply_dark_filter(&mut image);
        let p = image.get_pixel(0, 0);
        assert_eq!(p.0[3], 200, "alpha must pass through untouched");
        [p.0[0], p.0[1], p.0[2]]
    }

    /// A gray pixel (equal R/G/B) is unaffected by `hue-rotate`, since
    /// every row of its matrix sums to 1 (up to `f32` rounding — hence
    /// the +-1 tolerance here) — so a page's typical white
    /// background/black text should land on `invert(0.9)`'s own formula
    /// with no further shift from the rotation step.
    #[test]
    fn gray_pixels_only_see_the_invert_step() {
        // White background -> near-black (dark page, matching the old
        // app's CSS trick), not a full invert to pure black.
        for channel in filtered_pixel([255, 255, 255]) {
            assert!((25..=27).contains(&channel), "got {channel}");
        }
        // Black text -> light gray, not pure white.
        for channel in filtered_pixel([0, 0, 0]) {
            assert!((229..=231).contains(&channel), "got {channel}");
        }
    }

    /// A colored pixel is shifted by both steps together — pinned
    /// against the CSS spec's own `invert()`/`hue-rotate()` matrices
    /// hand-evaluated for pure red, so a future refactor of the
    /// constants can't silently drift from the spec.
    #[test]
    fn pure_red_becomes_a_light_warm_tint_not_cyan() {
        let [r, g, b] = filtered_pixel([255, 0, 0]);
        assert_eq!(r, 255, "clamped at the top of the 0..=255 range");
        assert!((140..145).contains(&g), "got {g}");
        assert!((140..145).contains(&b), "got {b}");
    }
}
