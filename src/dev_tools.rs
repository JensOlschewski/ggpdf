//! Headless, display-free diagnostics for developing the renderer: `--dump`
//! renders annotated fixture PNGs for eyes-on box-alignment review, and
//! `--profile <path>` times each per-page step on a real document. Both
//! bypass `app.rs`/`viewer.rs`/`SpikeApp` deliberately — no `egui::Context`
//! or window exists in either path, and that's the point: a slow or
//! misaligned step shows up directly instead of being guessed at from
//! on-screen symptoms (this sandbox also has no screen-recording
//! permission, so `--dump`'s PNGs are the only way to review alignment).

use crate::document::{self, PdfLibrary};
use crate::viewer;
use crate::viewport::{target_render_scale, SCALE};
use crate::{selection, text};

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
/// `debug_out/`, so box alignment can be reviewed without a live
/// display. Run with `cargo run -- --dump`.
pub fn dump_fixtures(library: &'static PdfLibrary) -> Result<(), document::DocumentError> {
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
                // kept only as the gate's visual counter-example, not
                // part of the real text model in text.rs.
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

/// Headless diagnostic for a live "it's slow" report on a specific file,
/// with no display needed: opens `path` and times exactly the steps
/// `SpikeApp::progressive_render`/`viewer::prepare_page` do per page
/// (PDFium rasterization, raw text extraction, `text::position_chars`,
/// `selection::group_into_lines`), without any `egui`/GUI machinery in
/// the way, so a pathologically slow step shows up directly instead of
/// being guessed at from on-screen symptoms. Run with `cargo run --
/// --profile <path>`.
pub fn profile_document(library: &'static PdfLibrary, path: &str) -> Result<(), document::DocumentError> {
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
