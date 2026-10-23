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
    /// Whether this texture already has the dark-mode color filter
    /// baked in (see `apply_dark_filter`). `render_page` re-rasterizes
    /// on a mismatch against the caller's current theme rather than
    /// filtering an already-filtered bitmap again.
    pub dark: bool,
}

impl RenderedPage {
    /// Stand-in for a page that failed to render, so the progressive
    /// render budget can move on instead of retrying it every frame.
    fn empty(ctx: &egui::Context, index: usize, render_scale: f32, dark: bool) -> RenderedPage {
        let color_image = egui::ColorImage::new([1, 1], vec![egui::Color32::TRANSPARENT; 1]);
        let texture = ctx.load_texture(format!("page-{index}-empty"), color_image, egui::TextureOptions::LINEAR);
        RenderedPage { texture, chars: Vec::new(), lines: Vec::new(), render_scale, dark }
    }
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
    pub size_pt: PageSize,
    /// `None` until `Viewer::render_page` has rasterized this page.
    pub rendered: Option<RenderedPage>,
}

/// Every page of one open document. `load()` populates sizes only;
/// bitmaps/text geometry fill in progressively via `render_page()`.
pub struct Viewer {
    pub pages: Vec<PageView>,
}

impl Viewer {
    /// Cheap: page sizes and rotation only, no PDFium rasterization — so
    /// opening even a long document is fast. Call `render_page` to
    /// actually decode a page's bitmap and text.
    pub fn load(doc: &Document) -> Result<Viewer, DocumentError> {
        let mut pages = Vec::with_capacity(doc.page_count() as usize);

        for index in 0..doc.page_count() {
            let page = doc.page(index)?;
            pages.push(PageView {
                size_pt: page.size(),
                rendered: None,
            });
        }

        Ok(Viewer { pages })
    }

    /// Index of the first page not rendered under the current theme, if
    /// any — a page rendered under the other theme (`dark` mismatch)
    /// counts as unrendered too. Superseded in `main.rs`'s
    /// `progressive_render` by `render::pages_to_render`'s
    /// viewport-priority scan (this "first in document order" scan
    /// would defeat the point of that prioritization, and would also
    /// never report "done" once `progressive_render`'s eviction is
    /// active — see its own comment); kept as a simple, direct check
    /// for tests.
    #[allow(dead_code)]
    pub fn next_unrendered(&self, dark: bool) -> Option<usize> {
        self.pages.iter().position(|p| p.rendered.as_ref().is_none_or(|r| r.dark != dark))
    }

    /// Rasterizes and caches page `index`. No-op if already rendered
    /// under the requested `dark` theme. A render failure is recorded as
    /// an empty page (see `RenderedPage::empty`) rather than left to
    /// retry forever (except across a `dark` toggle — see
    /// `next_unrendered`).
    ///
    /// `pixels_per_point` is clamped down, per page, so neither
    /// dimension of the rasterized bitmap exceeds `max_texture_side` —
    /// without this, an unusually large/landscape page at a high zoom
    /// level could panic on texture upload (previously unchecked).
    pub fn render_page(&mut self, doc: &Document, index: usize, pixels_per_point: f32, max_texture_side: usize, dark: bool, ctx: &egui::Context) {
        if self.pages[index].rendered.as_ref().is_some_and(|r| r.dark == dark) {
            return;
        }

        let result = (|| -> Result<RenderedPage, DocumentError> {
            let page = doc.page(index as u16)?;
            let size = page.size();
            let rotation_degrees = page.rotation_degrees();

            let max_side = max_texture_side as f32;
            let scale = pixels_per_point.min(max_side / size.width_pt.max(1.0)).min(max_side / size.height_pt.max(1.0));

            let mut image = page.render(scale)?;
            if dark {
                apply_dark_filter(&mut image);
            }
            let (w, h) = (image.width() as usize, image.height() as usize);
            let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], image.as_raw());
            let texture = ctx.load_texture(format!("page-{index}"), color_image, egui::TextureOptions::LINEAR);

            let raw_chars = page.raw_chars()?;
            let chars = text::position_chars(&raw_chars, size, rotation_degrees, scale);
            let lines = selection::group_into_lines(&chars);

            Ok(RenderedPage { texture, chars, lines, render_scale: scale, dark })
        })();

        self.pages[index].rendered = Some(result.unwrap_or_else(|_| RenderedPage::empty(ctx, index, pixels_per_point, dark)));
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
