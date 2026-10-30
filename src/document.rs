//! PDFium document/page lifetime management (see PLAN.md Project structure).
//!
//! This is the only module allowed to import `pdfium_render`. Everything
//! it hands back to callers is an owned, plain value (`image::RgbaImage`,
//! `f32` points, `char`) — never a PDFium handle — per PLAN.md: "Keep
//! PDFium handles behind the document.rs/render.rs boundary. The UI layer
//! consumes owned page images and text geometry, never raw PDFium
//! pointers."
//!
//! Char bounds are returned in the page's *unrotated* coordinate space
//! (PDFium's `FPDFText_GetCharBox` convention) — mapping them into the
//! rotated display space of a rendered page is a geometry/rotation
//! concern for a later `text.rs`/`render.rs`, not this module.

use pdfium_render::prelude::*;
use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub enum DocumentError {
    LoadLibrary(PdfiumError),
    Pdfium(PdfiumError),
    /// No document is open / no tab is active. Not a PDFium failure — the
    /// UI layer asked for a page before anything was opened.
    NoActiveDocument,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::LoadLibrary(e) => write!(f, "could not load PDFium library: {e}"),
            DocumentError::Pdfium(e) => write!(f, "{e}"),
            DocumentError::NoActiveDocument => write!(f, "no document is open"),
        }
    }
}

impl std::error::Error for DocumentError {}

impl From<PdfiumError> for DocumentError {
    fn from(e: PdfiumError) -> Self {
        DocumentError::Pdfium(e)
    }
}

/// Owns the loaded PDFium library bindings. One per process.
pub struct PdfLibrary {
    pdfium: Pdfium,
}

impl PdfLibrary {
    /// Loads the external PDFium library from `lib_dir` (a directory
    /// containing the platform-appropriate shared library, e.g.
    /// `libpdfium.dylib` on macOS).
    pub fn load(lib_dir: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let lib_path = Pdfium::pdfium_platform_library_name_at_path(lib_dir.as_ref());
        let bindings = Pdfium::bind_to_library(&lib_path).map_err(DocumentError::LoadLibrary)?;
        Ok(Self {
            pdfium: Pdfium::new(bindings),
        })
    }

    pub fn open<'lib>(&'lib self, path: impl AsRef<Path>) -> Result<Document<'lib>, DocumentError> {
        let doc = self.pdfium.load_pdf_from_file(path.as_ref(), None)?;
        Ok(Document { doc })
    }
}

pub struct Document<'lib> {
    doc: PdfDocument<'lib>,
}

impl<'lib> Document<'lib> {
    pub fn page_count(&self) -> u16 {
        self.doc.pages().len() as u16
    }

    pub fn page(&self, index: u16) -> Result<Page<'_>, DocumentError> {
        let page = self.doc.pages().get(index.into())?;
        Ok(Page { page })
    }
}

pub struct Page<'a> {
    page: PdfPage<'a>,
}

/// Page size in PDF points (1/72in), already reflecting the page's
/// intrinsic `/Rotate` (PDFium swaps width/height for 90°/270°).
#[derive(Debug, Clone, Copy)]
pub struct PageSize {
    pub width_pt: f32,
    pub height_pt: f32,
}

/// One extracted character: its Unicode value and bounding box in the
/// page's *unrotated* point space (`left`/`right`/`bottom`/`top`, origin
/// bottom-left).
#[derive(Debug, Clone, Copy)]
pub struct RawChar {
    pub ch: char,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

impl<'a> Page<'a> {
    pub fn size(&self) -> PageSize {
        PageSize {
            width_pt: self.page.width().value,
            height_pt: self.page.height().value,
        }
    }

    /// Degrees of intrinsic `/Rotate` (0, 90, 180, or 270).
    pub fn rotation_degrees(&self) -> i32 {
        match self.page.rotation().unwrap_or(PdfPageRenderRotation::None) {
            PdfPageRenderRotation::None => 0,
            PdfPageRenderRotation::Degrees90 => 90,
            PdfPageRenderRotation::Degrees180 => 180,
            PdfPageRenderRotation::Degrees270 => 270,
        }
    }

    /// Renders the page at `pixels_per_point` scale (e.g. 2.0 == 144 DPI).
    /// The returned image's dimensions match `size()` (already
    /// rotation-aware) times the scale.
    pub fn render(&self, pixels_per_point: f32) -> Result<image::RgbaImage, DocumentError> {
        let size = self.size();
        let config = PdfRenderConfig::new()
            .set_target_width((size.width_pt * pixels_per_point) as i32)
            .set_target_height((size.height_pt * pixels_per_point) as i32);

        let bitmap = self.page.render_with_config(&config)?;
        Ok(bitmap.as_image()?.to_rgba8())
    }

    /// Renders only the `width` x `height` pixel window whose top-left
    /// sits at (`x`, `y`) in the full page bitmap at `pixels_per_point`.
    /// The bitmap allocated is window-sized, not page-sized.
    pub fn render_region(&self, pixels_per_point: f32, x: i32, y: i32, width: i32, height: i32) -> Result<image::RgbaImage, DocumentError> {
        let size = self.size();
        let config = PdfRenderConfig::new()
            .set_target_width((size.width_pt * pixels_per_point) as i32)
            .set_target_height((size.height_pt * pixels_per_point) as i32)
            .set_origin(-x, -y);

        let mut bitmap = PdfBitmap::empty(width, height, PdfBitmapFormat::BGRA)?;
        self.page.render_into_bitmap_with_config(&mut bitmap, &config)?;
        Ok(bitmap.as_image()?.to_rgba8())
    }

    /// Extracts every character with a Unicode value and its bounds.
    /// Characters PDFium cannot map to a Unicode scalar (e.g. some
    /// embedded symbol-font glyphs) are skipped.
    pub fn raw_chars(&self) -> Result<Vec<RawChar>, DocumentError> {
        let text = self.page.text()?;
        let mut out = Vec::new();
        for char_obj in text.chars().iter() {
            let Some(ch) = char_obj.unicode_char() else { continue };
            let Ok(bounds) = char_obj.loose_bounds() else { continue };
            out.push(RawChar {
                ch,
                left: bounds.left().value,
                bottom: bounds.bottom().value,
                right: bounds.right().value,
                top: bounds.top().value,
            });
        }
        Ok(out)
    }
}

/// Shared `PdfLibrary` for tests. `Pdfium::bind_to_library` can only
/// succeed once per process (its underlying bindings are a global
/// `OnceCell`), but `cargo test` runs `#[test]` functions in parallel
/// threads of the same process — every test module that needs a
/// `PdfLibrary` must go through this instead of calling `PdfLibrary::load`
/// directly, or they'll race and some will fail with
/// `PdfiumLibraryBindingsAlreadyInitialized`.
#[cfg(test)]
pub(crate) fn test_library() -> &'static PdfLibrary {
    static LIB: std::sync::OnceLock<PdfLibrary> = std::sync::OnceLock::new();
    LIB.get_or_init(|| PdfLibrary::load("pdfium").expect("load pdfium for tests"))
}
