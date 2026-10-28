//! `SpikeApp`: the live viewer state — open tabs, the active `Viewer`,
//! keyboard/mouse cursor position, search, and the background render/
//! size-measurement plumbing. `ui.rs` (`impl eframe::App`) is the only
//! other place that touches these fields directly; everything else goes
//! through the methods here.

use crate::document::{self, PdfLibrary};
use crate::render_worker::{self, RenderWorker};
use crate::search::{SearchDirection, SearchMatch};
use crate::viewer::Viewer;
use crate::viewport::{to_egui_rect, ZoomAnchor, SCALE, SCROLL_MAX};
use crate::window::bundled_resources;
use crate::{app, config, input, selection, text};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn is_pdf_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

pub struct SpikeApp {
    pub app: app::App<'static>,
    pub cfg: config::Config,
    pub dispatcher: input::Dispatcher,
    pub zoom: f32,
    pub dark: bool,
    pub viewer: Option<Viewer>,
    /// One already-rendered `Viewer` per tab, keyed by tab index, so
    /// switching back to a previously-viewed tab reuses its decoded
    /// pages/textures instead of re-rasterizing the whole document from
    /// scratch (found live: "switching tabs is slow as hell" — every
    /// switch used to call `load_viewer` unconditionally, discarding
    /// and rebuilding the `Viewer`, even for a tab visited moments
    /// before). `None` entries are tabs not yet loaded; the active
    /// tab's entry is always `None` while `viewer` holds it instead (see
    /// `switch_to_active_tab`).
    pub viewers: Vec<Option<Viewer>>,
    pub show_boxes: bool,
    /// Click/drag selection is scoped to one page at a time; `cursor_idx`/
    /// `anchor_idx` index into that page's `chars`, not a global range.
    pub active_page: Option<usize>,
    pub cursor_idx: Option<usize>,
    pub anchor_idx: Option<usize>,
    /// Keyboard motion cursor (`selection.rs`-driven, `h`/`j`/`k`/`l`/
    /// word motions/`is`/`ip`), separate from the mouse-drag selection
    /// above. Synced into `active_page`/`cursor_idx`/`anchor_idx` after
    /// every motion so both share the same on-screen highlight. `None`
    /// until the first keyboard motion (defaults to page 0, line 0,
    /// character 0 then).
    pub page_cursor: Option<selection::Cursor>,
    /// Sticky target column for `Up`/`Down` motions (`selection.rs`
    /// reads, never writes, this across vertical moves).
    pub sticky_column: usize,
    /// Set by `apply_action` for `scroll_down`/`scroll_up`/
    /// `document_start`/`document_end`; consumed once inside the
    /// `ScrollArea` closure (that's the only place `ui.scroll_with_delta`
    /// can be called from).
    pub pending_scroll: Option<egui::Vec2>,
    /// Last painted page rectangles, in screen coordinates. Page padding
    /// and gaps do not scale with zoom, so the scroll offset alone cannot
    /// identify the content point beneath the pointer.
    pub page_rects: Vec<egui::Rect>,
    pub viewport_rect: egui::Rect,
    pub pending_zoom_anchor: Option<ZoomAnchor>,
    pub close_requested: bool,
    /// Per-motion page/line/char feedback. Not rendered — `style.css`'s
    /// `.pdf-visual-status` is screen-reader-only (clip-path trick)
    /// except for `.is-notice` messages (see `notice`/`notice_until`
    /// below); this is the non-notice half, kept for parity/future
    /// accessibility hookup rather than a visible permanent status bar.
    pub status: String,
    /// Transient visible message (copy confirmations, errors) — matches
    /// `style.css`'s `.pdf-visual-status.is-notice`: a small floating
    /// box, not a permanent toolbar. `notice_until` is an `egui` time
    /// (seconds) past which it's no longer shown; set via `notify()`.
    pub notice: String,
    pub notice_until: f64,
    /// (active tab index, zoom) the currently-loaded `viewer` reflects.
    /// `None` forces a load on the first frame (texture creation needs a
    /// live `egui::Context`, unavailable at construction time).
    pub loaded_key: Option<usize>,
    /// `Some(direction)` while actively typing a `/`/`?` query (not yet
    /// confirmed with Enter or cancelled with Esc) — matches Vim's
    /// command-line mode: every other key (chrome shortcuts, the modal
    /// `Dispatcher`) is swallowed while this is set, and printable text
    /// events go into `search_query` instead.
    pub search_editing: Option<SearchDirection>,
    /// Unconfirmed command-line text. Escape discards it.
    pub search_draft: String,
    /// Last confirmed query; `n`/`N` repeat it.
    pub search_query: String,
    /// Which way `n` repeats; `N` repeats `search_direction.opposite()`
    /// for just that one jump, same as Vim.
    pub search_direction: SearchDirection,
    /// Full-document text matches, independent of raster cache. Sorted by
    /// `(page, start)` so `nearest_match_index` can wrap correctly.
    pub search_matches: Vec<SearchMatch>,
    /// Index into `search_matches` of the one `n`/`N` last jumped to —
    /// drawn more prominently than the others. `None` if nothing has
    /// been searched yet, or the last search found nothing.
    pub search_current: Option<usize>,
    /// Background render thread (see `render_worker.rs`); `progressive_render`
    /// submits jobs to it and drains finished results instead of calling
    /// `Viewer::render_page` synchronously, keeping PDFium rasterization
    /// and text extraction off the UI thread.
    pub render_worker: RenderWorker,
    /// `(tab, receiver)` for every tab whose real per-page sizes are
    /// still being measured on a background thread (see `load_viewer`
    /// and `viewer::Viewer::measure_page_sizes`) — a `Vec`, not a single
    /// slot, because opening a second large document while the first
    /// one's measurement is still running must not lose track of it.
    /// Drained every frame by `apply_pending_page_sizes`.
    pub pending_sizes: Vec<(usize, std::sync::mpsc::Receiver<Vec<document::PageSize>>)>,
    #[cfg(target_os = "macos")]
    pub document_events: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    pub sample_tab: bool,
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
    pub fn new(library: &'static PdfLibrary, cfg: config::Config, initial_paths: &[String]) -> Self {
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
            // real viewing experience — it's gate-validation scaffolding.
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

    pub fn open_document_event(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
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
    pub fn notify(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.notice = text.into();
        self.notice_until = ctx.input(|i| i.time) + 3.0;
    }

    /// Both the picker and OS drag-and-drop use the same open path. The
    /// document layer only changes tabs after PDFium has opened the file.
    pub fn open_pdf_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>, ctx: &egui::Context) {
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

    /// `(page index, lines)` the keyboard cursor applies to: the mouse's
    /// `active_page` if one was clicked, else whichever page is
    /// currently nearest the viewport center (see `page_near`) — found
    /// live: entering Cursor mode after only scrolling (no click yet)
    /// always planted the cursor on page 0, regardless of how far the
    /// user had scrolled, because this previously fell back to a fixed
    /// `0` instead of asking where the view actually is. Falls back to
    /// `0` itself only before the first frame has painted anything (so
    /// `page_near` has nothing to go on yet).
    pub fn keyboard_page(&self) -> Option<usize> {
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
    pub fn cursor_for_page(&self, page_idx: usize) -> selection::Cursor {
        let lines = self.viewer.as_ref().and_then(|v| Self::page_lines(v, page_idx));
        self.page_cursor
            .filter(|_| self.active_page == Some(page_idx))
            .filter(|cursor| lines.and_then(|l| l.get(cursor.line)).is_some_and(|line| cursor.character < line.chars.len()))
            .unwrap_or_else(|| self.initial_cursor_for_page(page_idx))
    }

    /// Where the keyboard cursor should appear in `page_idx` the first
    /// time a keyboard mode is entered this session (`page_cursor` still
    /// `None`) — the line nearest the viewport's current vertical
    /// center, not always line 0. Falls back to line 0 if the page isn't
    /// rendered yet, has no lines, or `page_rects` has no entry for it
    /// yet (nothing painted this run).
    pub fn initial_cursor_for_page(&self, page_idx: usize) -> selection::Cursor {
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
    pub fn flat_index(lines: &[selection::Line], cursor: selection::Cursor) -> usize {
        lines[..cursor.line].iter().map(|l| l.chars.len()).sum::<usize>() + cursor.character
    }

    /// `None` if the page index is out of range or that page hasn't
    /// finished rendering yet (progressive_render hasn't reached it) —
    /// both treated the same by callers: nothing to act on this frame.
    pub fn page_lines(viewer: &Viewer, page_idx: usize) -> Option<&[selection::Line]> {
        viewer.pages.get(page_idx)?.rendered.as_ref().map(|r| r.lines.as_slice())
    }

    pub fn page_chars(viewer: &Viewer, page_idx: usize) -> Option<&[text::PositionedChar]> {
        viewer.pages.get(page_idx)?.rendered.as_ref().map(|r| r.chars.as_slice())
    }

    /// Index of (and screen rect for) whichever page's painted rect is
    /// vertically closest to `point`, from last frame's `page_rects` —
    /// `None` before the first frame has painted anything. Shared by
    /// `zoom_to` (anchor the zoom at the pointer or viewport center) and
    /// `progressive_render` (prioritize rendering/eviction around
    /// whatever's actually on screen, not document order).
    pub fn page_near(&self, point: egui::Pos2) -> Option<(usize, egui::Rect)> {
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
    pub fn zoom_to(&mut self, new_zoom: f32, pointer: Option<egui::Pos2>) {
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

    pub fn apply_motion(&mut self, motion: selection::Motion, count: usize) {
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
    pub fn move_vertical_cross_page(
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
    /// the on-screen highlight. Mirrors `visual.js`'s `message()`
    /// ("CURSOR · Page X · Line Y · ...").
    pub fn update_cursor_status(&mut self, page_idx: usize, cursor: selection::Cursor) {
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
    pub fn apply_text_object(&mut self, bounds: Option<(selection::Cursor, selection::Cursor)>) {
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

    pub fn apply_action(&mut self, action: config::Action, count: usize, ctx: &egui::Context) {
        use config::Action::*;
        let count = count.max(1);
        match action {
            // Confirmed backwards by live testing — flipped from the
            // original (unverified) guess.
            ScrollDown => self.pending_scroll = Some(egui::vec2(0.0, -crate::viewport::SCROLL_STEP * count as f32)),
            ScrollUp => self.pending_scroll = Some(egui::vec2(0.0, crate::viewport::SCROLL_STEP * count as f32)),
            DocumentStart => match self.page_rects.get(count.min(self.page_rects.len()).saturating_sub(1)) {
                Some(rect) if count > 1 => self.pending_scroll = Some(egui::vec2(0.0, self.viewport_rect.min.y - rect.min.y)),
                _ => self.pending_scroll = Some(egui::vec2(0.0, SCROLL_MAX)),
            },
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
                // it did nothing.
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
    /// other page, not rasterization — that's still `progressive_render`'s
    /// job). Called only for a tab with no cached `Viewer` yet; see
    /// `switch_to_active_tab`.
    pub fn load_viewer(&mut self, ctx: &egui::Context) {
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
    pub fn spawn_page_size_measurement(&mut self, tab: usize, document: Arc<document::Document<'static>>, ctx: egui::Context) {
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
    pub fn apply_pending_page_sizes(&mut self) {
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
    pub fn activate_viewer(&mut self, viewer: Viewer) {
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
    pub fn switch_to_active_tab(&mut self, ctx: &egui::Context) {
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
    /// keeping any single frame fast regardless of document length.
    pub fn progressive_render(&mut self, ctx: &egui::Context) {
        // This PDF renders at ~27 MB raw pixels per page at 100% zoom
        // on a 2x display. The old radius 40 held up to 81 textures
        // (over 2 GB raw, before upload copies). Keep the visible page
        // and a few neighbors; evict everything farther away.
        const EVICT_KEEP_RADIUS: usize = 3;

        let Some(active) = self.app.active() else { return };
        let document = active.document.clone();
        let tab = self.app.active_index();
        // Default (2048) is egui's conservative fallback when a backend
        // doesn't report its real limit; a live GPU backend reports the
        // actual one. Either way, `viewer::prepare_page` now clamps to it
        // per page instead of trusting the target scale blindly.
        let max_texture_side = ctx.input(|i| i.max_texture_side);
        let target_scale = crate::viewport::target_render_scale(self.zoom, ctx.pixels_per_point());
        // This frame's actual scroll position, including tab restoration
        // and completed zoom anchoring. Evicting from old rectangles made
        // zoomed pages disappear until a fresh render completed.
        let current_page = self.page_near(self.viewport_rect.center()).map(|(page, _)| page).unwrap_or(0);
        // Read every frame, not just when submitting something new: the
        // worker picks its next job by comparing each *already queued*
        // job against this live value (see `render_worker.rs`), so a
        // fast scroll or page jump reprioritizes in-flight requests
        // immediately instead of only the next job submitted.
        self.render_worker.set_focus(tab, current_page);
        let Some(viewer) = self.viewer.as_mut() else { return };

        // See `viewport::page_needs_render`'s own doc comment for the
        // staleness rule.
        let needs_render = |i: usize| {
            crate::viewport::page_needs_render(viewer.pages[i].rendered.as_ref().map(|r| r.requested_scale), viewer.pages[i].pending, target_scale)
        };

        // Queue only this small window. The worker wakes egui after each
        // result, so no continuous repaint loop is needed while waiting.
        let candidates = crate::render::nearby_pages_to_render(current_page, viewer.pages.len(), EVICT_KEEP_RADIUS, needs_render);
        for &idx in &candidates {
            viewer.pages[idx].pending = Some(target_scale);
            self.render_worker.submit(tab, idx, render_worker::RenderJobSpec { requested_scale: target_scale, max_texture_side, document: document.clone() });
        }
        for idx in crate::render::pages_to_evict(current_page, viewer.pages.len(), |i| viewer.pages[i].rendered.is_some(), EVICT_KEEP_RADIUS) {
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

    /// Submits any pending render job's finished result before layout
    /// (`progressive_render`, after layout, decides what to submit next).
    pub fn collect_render_results(&mut self, ctx: &egui::Context) {
        self.render_worker.set_repaint_context(ctx);
        self.apply_pending_page_sizes();
        if let Some(result) = self.render_worker.poll_one() {
            self.apply_render_result(result, ctx);
            ctx.request_repaint();
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
    pub fn apply_render_result(&mut self, result: render_worker::RenderResult, ctx: &egui::Context) {
        let render_worker::RenderResult { tab, prepared } = result;
        if tab >= self.app.tab_count() {
            return;
        }
        let viewer = if self.loaded_key == Some(tab) { self.viewer.as_mut() } else { self.viewers.get_mut(tab).and_then(Option::as_mut) };
        let Some(viewer) = viewer else { return };
        let page_idx = prepared.index;
        let Some(page) = viewer.pages.get_mut(page_idx) else { return };
        if page.pending != Some(prepared.requested_scale()) {
            return; // canceled or superseded while the worker was rendering
        }
        page.pending = None;
        if page.rendered.as_ref().is_some_and(|old| old.requested_scale > prepared.requested_scale()) {
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
    pub fn run_search(&mut self) -> Result<(), document::DocumentError> {
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
    pub fn confirm_search(&mut self, direction: SearchDirection, ctx: &egui::Context) {
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
            crate::search::nearest_match_index(&self.search_matches, start_page, start_char, direction)
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
    pub fn jump_search(&mut self, count: usize, reverse: bool, ctx: &egui::Context) {
        if self.search_matches.is_empty() {
            self.notify(ctx, if self.search_query.is_empty() { "No previous search".to_string() } else { "No search matches".to_string() });
            return;
        }
        let direction = if reverse { self.search_direction.opposite() } else { self.search_direction };
        let current = self.search_current.unwrap_or(0);
        let idx = crate::search::step_match_index(current, self.search_matches.len(), count, direction);
        self.search_current = Some(idx);
        self.scroll_to_match(idx);
        self.notify(ctx, format!("/{} [{}/{}]", self.search_query, idx + 1, self.search_matches.len()));
    }

    /// Scrolls so `search_matches[idx]` lands at the viewport's vertical
    /// center — reuses the one-frame-stale `page_rects` lookup (same
    /// tolerance `zoom_to`/`page_near` already rely on) and the existing
    /// `pending_scroll`/`ui.scroll_with_delta` mechanism every other
    /// scroll action in this file already goes through.
    pub fn scroll_to_match(&mut self, idx: usize) {
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

    pub fn selected_range(&self) -> Option<(usize, usize)> {
        let a = self.anchor_idx?;
        let b = self.cursor_idx?;
        Some((a.min(b), a.max(b)))
    }

    /// Which line a flat `page.chars` index falls in, or `None` if out
    /// of range. Mirrors the offset accounting `selection_span_rects`
    /// and `flat_index` already use.
    pub fn line_containing_flat(lines: &[selection::Line], flat: usize) -> Option<usize> {
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
    /// mode (matches `visual.js`'s `linewise` selection).
    pub fn display_selected_range(&self, page_idx: usize) -> Option<(usize, usize)> {
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

    pub fn copy_selection(&mut self, ctx: &egui::Context) {
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
    pub fn char_at_pos(&self, page_idx: usize, pos: egui::Pos2) -> Option<usize> {
        let viewer = self.viewer.as_ref()?;
        let render_scale = viewer.pages.get(page_idx)?.rendered.as_ref()?.render_scale;
        let chars = Self::page_chars(viewer, page_idx)?;
        let inv_scale = render_scale / self.zoom;
        let render_pos = egui::pos2(pos.x * inv_scale, pos.y * inv_scale);
        chars.iter().position(|c| to_egui_rect(c.rect, 1.0).expand(1.0).contains(render_pos))
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
        app.viewer.as_mut().unwrap().render_page(&doc, 0, SCALE, 2048, &ctx);
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

#[cfg(test)]
mod search_integration_tests {
    use super::*;

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

#[cfg(test)]
mod cursor_focus_tests {
    use super::*;
    use crate::viewer;

    fn line_with_rect(top: f32, bottom: f32) -> selection::Line {
        selection::Line { chars: vec![text::PositionedChar { ch: 'x', rect: text::Rect { left: 0.0, top, right: 10.0, bottom } }] }
    }

    fn app_with_lines(lines: Vec<selection::Line>) -> SpikeApp {
        let library = document::test_library();
        let mut app = SpikeApp::new(library, config::Config::default(), &["assets/test-pdfs/letter.pdf".to_string()]);
        let ctx = egui::Context::default();
        app.switch_to_active_tab(&ctx);
        let texture = ctx.load_texture("test", egui::ColorImage::new([1, 1], vec![egui::Color32::TRANSPARENT; 1]), egui::TextureOptions::LINEAR);
        let dark_texture = texture.clone();
        let rendered = viewer::RenderedPage { texture, dark_texture, chars: Vec::new(), lines, render_scale: 1.0, requested_scale: 1.0 };
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
        app.viewer.as_mut().unwrap().render_page(&doc, 1, 1.0, 2048, &ctx);
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
        let prepared = viewer::prepare_page(&doc, 1, 1.0, 2048);
        app.viewer.as_mut().unwrap().pages[1].pending = Some(1.0);
        app.apply_render_result(render_worker::RenderResult { tab: 0, prepared }, &ctx);
        assert_eq!(app.active_page, Some(1));
        assert!(app.page_cursor.is_some());
        assert!(app.cursor_idx.is_some());
    }
}

#[cfg(test)]
mod viewer_smoke_tests {
    use super::*;
    use crate::viewer;
    use crate::viewport::target_render_scale;

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
        app.viewer.as_mut().unwrap().render_page(&doc, 0, target, 2048, &ctx);
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
        app.viewer.as_mut().unwrap().render_page(&doc, 0, 1.5, 2048, &ctx);
        let original_id = app.viewer.as_ref().unwrap().pages[0].rendered.as_ref().unwrap().texture.id();
        let prepared = viewer::prepare_page(&doc, 0, 1.0, 2048);
        app.viewer.as_mut().unwrap().pages[0].pending = Some(1.0);
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
        let prepared = viewer::prepare_page(&doc, 0, 1.0, 2048);
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
        assert_eq!(v.next_unrendered(), Some(0));

        let ctx = egui::Context::default();
        for i in 0..v.pages.len() {
            v.render_page(&doc, i, SCALE, 2048, &ctx);
            assert!(v.pages[i].rendered.as_ref().unwrap().texture.size_vec2().x > 0.0);
        }
        assert_eq!(v.next_unrendered(), None, "every page should be rendered now");
    }

    #[test]
    fn rendering_a_page_builds_both_theme_textures_so_a_dark_toggle_needs_no_rerender() {
        let library = document::test_library();
        let doc = library.open("assets/test-pdfs/letter.pdf").expect("open doc");
        let mut v = Viewer::load(&doc).expect("viewer load");
        let ctx = egui::Context::default();

        v.render_page(&doc, 0, SCALE, 2048, &ctx);
        let page = v.pages[0].rendered.as_ref().unwrap();
        assert!(page.texture.size_vec2().x > 0.0);
        assert!(page.dark_texture.size_vec2().x > 0.0);
        assert_ne!(page.texture.id(), page.dark_texture.id(), "a dark toggle must switch textures, not share one");
        assert_eq!(v.next_unrendered(), None, "both themes are already cached — nothing left to render");
    }

    /// Regression test for the GPU-texture-size gap: a page whose point
    /// size would rasterize past `max_texture_side` at the requested
    /// scale must come out no larger than that limit on either side,
    /// not panic on texture upload.
    #[test]
    fn render_page_clamps_to_max_texture_side() {
        let library = document::test_library();
        let doc = library.open("assets/test-pdfs/letter.pdf").expect("open doc");
        let mut v = Viewer::load(&doc).expect("viewer load");
        let ctx = egui::Context::default();

        // Letter-size at SCALE=2.0 is already ~1224x1584px; clamp hard
        // to something smaller than that to force the clamp path.
        v.render_page(&doc, 0, SCALE, 512, &ctx);
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
        v1.render_page(&doc, 0, SCALE, 2048, &ctx);
        let mut v2 = Viewer::load(&doc).expect("load 2");
        v2.render_page(&doc, 0, SCALE * 1.2, 2048, &ctx);

        let w1 = v1.pages[0].rendered.as_ref().unwrap().texture.size_vec2().x;
        let w2 = v2.pages[0].rendered.as_ref().unwrap().texture.size_vec2().x;
        assert!((w2 - w1 * 1.2).abs() < 1.0, "1.2x scale should scale pixel width by 1.2x: {w1} vs {w2}");
    }
}
