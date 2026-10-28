//! Background page rendering (PLAN.md's "Keep PDFium work off the UI
//! thread" rule, and the live-testing reports that scrolling/zooming/
//! entering Cursor mode felt slow or unresponsive once a document had
//! more than a handful of pages, or after a fast scroll/jump).
//!
//! `main.rs`'s `progressive_render` used to call `Viewer::render_page`
//! directly inside the per-frame update — PDFium rasterization, the dark
//! filter's per-pixel matrix math, and `FPDFText_*` extraction all ran
//! synchronously on the UI thread, blocking input and repainting for as
//! long as that took. This module moves the CPU-only part of that work
//! (`viewer::prepare_page`) onto one dedicated background thread.
//!
//! A first version of this module used a plain FIFO channel of jobs,
//! matching the shape of the synchronous call it replaced. That had a
//! real bug, not just a style choice: a fast scroll or page jump queues
//! several frames' worth of "nearby" pages before the worker can drain
//! any of them, and a plain FIFO processes them in *submission* order —
//! so the page the user is actually looking at *right now* could sit
//! behind a dozen now-irrelevant jobs for pages scrolled past moments
//! ago, found live as pages (and Cursor mode, which needs the current
//! page's text already extracted) staying stuck for a noticeable beat.
//! Every viewer surveyed for this (PDF.js's render queue, which always
//! promotes the most-visible page; Sioyek's `pdf_renderer`) instead
//! tracks a *current* priority and lets the next job picked reflect it,
//! not whatever order requests arrived in. This module does the same:
//! `submit` stores jobs in a shared map keyed by `(tab, page)` (so a
//! second request for the same page just replaces the first, rather
//! than queuing twice), and the worker always picks whichever queued job
//! is nearest the live `focus` (the on-screen page `set_focus` reports
//! every frame) — a job for a page the user scrolled away from before
//! the worker ever got to it is simply reprioritized below the one it's
//! now looking at, with no separate cancellation step needed.
//!
//! One worker thread, not a pool: `pdfium-render`'s `thread_safe` feature
//! (on by default) serializes every PDFium call behind a single internal
//! mutex, so a second thread calling PDFium concurrently gets no
//! throughput benefit — see the crate's own "Multi-threading" docs. The
//! benefit here is purely about *which* thread blocks and *in what
//! order* queued work resolves, not about doing more work in parallel.
//!
//! No generation tagging: a job already in flight (already picked off
//! the queue, mid-render) when its page is evicted or its tab closed
//! just produces a result nothing wants — `main.rs`'s
//! `apply_render_result` drops it with a bounds check rather than
//! applying it somewhere wrong (see its own comment). Revisit if this
//! ever grows a worker pool, where out-of-order completion would make a
//! stale result overwrite a newer one.

use crate::document::Document;
use crate::viewer::{prepare_page, prepare_tile, PreparedPage, PreparedTile};
use eframe::egui;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Condvar, Mutex};

/// Identifies one page's render request. A second `submit` for the same
/// key replaces whatever was queued for it, rather than adding a
/// duplicate — `main.rs`'s `PageView::pending` already prevents most
/// re-submission while a job is in flight, but this is what prevents it
/// from ever queueing twice regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct JobKey {
    tab: usize,
    page: usize,
}

pub struct RenderJobSpec {
    pub requested_scale: f32,
    pub max_texture_side: usize,
    pub document: Arc<Document<'static>>,
}

pub struct RenderResult {
    pub tab: usize,
    pub prepared: PreparedPage,
}

/// A viewport-window render (`viewer::prepare_tile`). `generation` is
/// compared with the live `tile_generation` when the worker picks the
/// job: a mismatch means the view moved on, so the render is skipped.
pub struct TileJobSpec {
    pub scale: f32,
    pub window: (i32, i32, i32, i32),
    pub dark: bool,
    pub generation: u64,
    pub document: Arc<Document<'static>>,
}

/// `tile` is `None` when the job was skipped as stale or failed, so the
/// UI can always clear its pending marker for `(tab, page, generation)`.
pub struct TileResult {
    pub tab: usize,
    pub page: usize,
    pub generation: u64,
    pub tile: Option<PreparedTile>,
}

struct SharedState {
    queue: HashMap<JobKey, RenderJobSpec>,
    /// Tile jobs always run before full-page jobs: they are what the
    /// user sees, and they are small.
    tile_queue: HashMap<JobKey, TileJobSpec>,
    tile_generation: u64,
    /// `(tab, page)` the UI considers "current" right now — `main.rs`
    /// calls `set_focus` every frame via `progressive_render`. Read by
    /// the worker each time it needs to pick the next job, not just at
    /// submission time, so a job's priority stays live even while it
    /// sits queued across several frames of scrolling.
    focus: (usize, usize),
}

/// Lowest-distance-from-focus job in `queue`, strongly preferring the
/// focused tab over any other open tab (a background tab's pages still
/// render eventually, just never ahead of whatever the visible tab
/// still needs) — pulled out of the worker loop so the priority rule
/// itself has direct unit tests independent of thread timing.
fn pick_next<V>(queue: &HashMap<JobKey, V>, focus: (usize, usize)) -> Option<JobKey> {
    queue
        .keys()
        .copied()
        .min_by_key(|key| {
            let distance = key.page.abs_diff(focus.1);
            let same_tab = key.tab == focus.0;
            (!same_tab, distance)
        })
}

pub struct RenderWorker {
    state: Arc<Mutex<SharedState>>,
    work_available: Arc<Condvar>,
    results: mpsc::Receiver<RenderResult>,
    tile_results: mpsc::Receiver<TileResult>,
    repaint: Arc<Mutex<Option<egui::Context>>>,
}

impl RenderWorker {
    /// Spawns the background thread. `Document<'static>` (via the
    /// `&'static PdfLibrary` `main.rs` already leaks at startup) is what
    /// makes moving a job across threads possible without a lifetime
    /// that would outlive the thread itself.
    pub fn spawn() -> RenderWorker {
        let state = Arc::new(Mutex::new(SharedState { queue: HashMap::new(), tile_queue: HashMap::new(), tile_generation: 0, focus: (0, 0) }));
        let work_available = Arc::new(Condvar::new());
        let (result_tx, result_rx) = mpsc::channel::<RenderResult>();
        let (tile_tx, tile_rx) = mpsc::channel::<TileResult>();
        let repaint = Arc::new(Mutex::new(None::<egui::Context>));

        let thread_state = state.clone();
        let thread_cv = work_available.clone();
        let thread_repaint = repaint.clone();
        std::thread::Builder::new()
            .name("ggpdf-render".to_string())
            .spawn(move || loop {
                let picked = {
                    let mut guard = thread_state.lock().unwrap();
                    loop {
                        if let Some(key) = pick_next(&guard.tile_queue, guard.focus) {
                            let spec = guard.tile_queue.remove(&key).expect("pick_next only returns keys present in the map");
                            let stale = spec.generation != guard.tile_generation;
                            break Err((key, spec, stale));
                        }
                        if let Some(key) = pick_next(&guard.queue, guard.focus) {
                            let spec = guard.queue.remove(&key).expect("pick_next only returns keys present in the map");
                            break Ok((key, spec));
                        }
                        // Releases the lock while idle; `submit` holds it
                        // only to insert and notify, so this never misses
                        // a wakeup.
                        guard = thread_cv.wait(guard).unwrap();
                    }
                };
                let (key, spec) = match picked {
                    Ok(job) => job,
                    Err((key, spec, stale)) => {
                        let tile = if stale { None } else { prepare_tile(&spec.document, key.page, spec.scale, spec.window, spec.dark).ok() };
                        let result = TileResult { tab: key.tab, page: key.page, generation: spec.generation, tile };
                        if tile_tx.send(result).is_ok() {
                            if let Some(ctx) = thread_repaint.lock().unwrap().as_ref() {
                                ctx.request_repaint();
                            }
                        }
                        continue;
                    }
                };
                let prepared = prepare_page(&spec.document, key.page, spec.requested_scale, spec.max_texture_side);
                // The UI thread may have exited (window closed) —
                // nothing to do with a result nobody will read.
                if result_tx.send(RenderResult { tab: key.tab, prepared }).is_ok() {
                    if let Some(ctx) = thread_repaint.lock().unwrap().as_ref() {
                        ctx.request_repaint();
                    }
                }
            })
            .expect("spawn render worker thread");

        RenderWorker { state, work_available, results: result_rx, tile_results: tile_rx, repaint }
    }

    /// Queues a viewport-window render, replacing any not-yet-started
    /// tile request for the same page.
    pub fn submit_tile(&self, tab: usize, page: usize, spec: TileJobSpec) {
        let mut guard = self.state.lock().unwrap();
        guard.tile_queue.insert(JobKey { tab, page }, spec);
        self.work_available.notify_one();
    }

    /// Marks every tile job with a different `generation` as stale. Call
    /// when zoom or scroll changes the wanted window.
    pub fn set_tile_generation(&self, generation: u64) {
        self.state.lock().unwrap().tile_generation = generation;
    }

    pub fn poll_tile(&self) -> Option<TileResult> {
        self.tile_results.try_recv().ok()
    }

    /// An egui context can safely be used from the render thread to wake
    /// the UI once a page is ready. Do not spin repaint frames while idle.
    pub fn set_repaint_context(&self, ctx: &egui::Context) {
        *self.repaint.lock().unwrap() = Some(ctx.clone());
    }

    /// Queues a render for `(tab, page)`, replacing any not-yet-started
    /// request already queued for it (see this module's doc comment).
    pub fn submit(&self, tab: usize, page: usize, spec: RenderJobSpec) {
        let mut guard = self.state.lock().unwrap();
        guard.queue.insert(JobKey { tab, page }, spec);
        self.work_available.notify_one();
    }

    /// Drops a queued-but-not-yet-started request for `(tab, page)`, if
    /// any — called on eviction so a page scrolled far out of view
    /// doesn't cost a render the moment it's freed, only to be evicted
    /// again shortly after. A no-op, not an error, if the worker already
    /// started that job or it was never queued.
    pub fn cancel(&self, tab: usize, page: usize) {
        self.state.lock().unwrap().queue.remove(&JobKey { tab, page });
    }

    /// Updates the live priority focus. Doesn't wake the worker by
    /// itself (moving the focus never adds new work, only reorders
    /// existing queued work for whenever the worker next picks) —
    /// `submit`'s own `notify_one` is what wakes an idle worker.
    pub fn set_focus(&self, tab: usize, page: usize) {
        self.state.lock().unwrap().focus = (tab, page);
    }

    /// Drains every result currently available in worker tests.
    #[cfg(test)]
    pub fn poll(&self) -> Vec<RenderResult> {
        self.results.try_iter().collect()
    }

    /// The UI uploads at most one large page texture per frame.
    pub fn poll_one(&self) -> Option<RenderResult> {
        self.results.try_recv().ok()
    }
}

#[cfg(test)]
mod pick_next_tests {
    use super::*;
    use crate::document::test_library;

    /// `pick_next` never reads `document` itself (only the `JobKey` it's
    /// stored under), but `RenderJobSpec` needs a real value for the
    /// field regardless — opened once per test and cloned into every
    /// queued entry, same pattern as this module's other tests.
    fn spec(document: &Arc<Document<'static>>) -> RenderJobSpec {
        RenderJobSpec { requested_scale: 1.0, max_texture_side: 2048, document: document.clone() }
    }

    fn open_test_doc() -> Arc<Document<'static>> {
        Arc::new(test_library().open("assets/test-pdfs/letter.pdf").expect("open doc"))
    }

    #[test]
    fn picks_the_queued_page_nearest_the_focus_page() {
        let doc = open_test_doc();
        let mut queue = HashMap::new();
        queue.insert(JobKey { tab: 0, page: 50 }, spec(&doc));
        queue.insert(JobKey { tab: 0, page: 5 }, spec(&doc));
        queue.insert(JobKey { tab: 0, page: 7 }, spec(&doc));
        // 5 and 7 are equally close (distance 1) to focus page 6; either
        // is an acceptable pick, but it must never be the far one (50).
        assert_ne!(pick_next(&queue, (0, 6)), Some(JobKey { tab: 0, page: 50 }));
    }

    #[test]
    fn prefers_the_focused_tab_over_a_closer_page_in_another_tab() {
        let doc = open_test_doc();
        let mut queue = HashMap::new();
        queue.insert(JobKey { tab: 1, page: 6 }, spec(&doc)); // same page, background tab
        queue.insert(JobKey { tab: 0, page: 40 }, spec(&doc)); // far page, focused tab
        assert_eq!(pick_next(&queue, (0, 6)), Some(JobKey { tab: 0, page: 40 }));
    }

    #[test]
    fn stale_entries_naturally_lose_priority_without_being_removed() {
        // Simulates a fast scroll/jump: a job queued while the user was
        // near page 2, then focus moves on to page 90 before the worker
        // gets to it — the near-90 request must win, with no separate
        // cancellation of the page-2 request needed.
        let doc = open_test_doc();
        let mut queue = HashMap::new();
        queue.insert(JobKey { tab: 0, page: 2 }, spec(&doc));
        queue.insert(JobKey { tab: 0, page: 90 }, spec(&doc));
        assert_eq!(pick_next(&queue, (0, 90)), Some(JobKey { tab: 0, page: 90 }));
    }

    #[test]
    fn empty_queue_has_nothing_to_pick() {
        assert_eq!(pick_next::<()>(&HashMap::new(), (0, 0)), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::test_library;

    /// Polls for up to five seconds, returning every result collected.
    /// The worker thread runs independently; this avoids requiring the
    /// very first `poll()` to already have a result (which would make
    /// these tests flaky under load) while still bounding worst-case
    /// runtime if a result never arrives.
    fn poll_for_a_while(worker: &RenderWorker) -> Vec<RenderResult> {
        let mut results = Vec::new();
        for _ in 0..1000 {
            results.extend(worker.poll());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        results
    }

    #[test]
    fn worker_renders_a_page_without_blocking_the_caller() {
        let doc = Arc::new(test_library().open("assets/test-pdfs/letter.pdf").expect("open doc"));
        let worker = RenderWorker::spawn();

        worker.set_focus(0, 0);
        worker.submit(0, 0, RenderJobSpec { requested_scale: 1.0, max_texture_side: 2048, document: doc });

        let results = poll_for_a_while(&worker);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tab, 0);
        assert_eq!(results[0].prepared.index, 0);
    }

    fn tile_spec(doc: &Arc<Document<'static>>, generation: u64) -> TileJobSpec {
        TileJobSpec { scale: 2.0, window: (10, 20, 100, 50), dark: false, generation, document: doc.clone() }
    }

    fn wait_for_tile(worker: &RenderWorker) -> TileResult {
        for _ in 0..1000 {
            if let Some(result) = worker.poll_tile() {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no tile result");
    }

    #[test]
    fn tile_job_renders_only_the_requested_window() {
        let doc = Arc::new(test_library().open("assets/test-pdfs/letter.pdf").expect("open doc"));
        let worker = RenderWorker::spawn();
        worker.submit_tile(0, 0, tile_spec(&doc, 0));
        let result = wait_for_tile(&worker);
        let tile = result.tile.expect("tile rendered");
        assert_eq!((tile.image.width(), tile.image.height()), (100, 50));
    }

    #[test]
    fn stale_tile_job_is_skipped_but_still_reported() {
        let doc = Arc::new(test_library().open("assets/test-pdfs/letter.pdf").expect("open doc"));
        let worker = RenderWorker::spawn();
        worker.set_tile_generation(5);
        worker.submit_tile(0, 0, tile_spec(&doc, 4));
        let result = wait_for_tile(&worker);
        assert_eq!(result.generation, 4);
        assert!(result.tile.is_none());
    }

    #[test]
    fn a_later_submit_for_the_same_page_replaces_the_earlier_one_rather_than_queuing_twice() {
        let doc = Arc::new(test_library().open("assets/test-pdfs/letter.pdf").expect("open doc"));
        let worker = RenderWorker::spawn();

        // Hold the queue lock so the worker cannot pick the first job
        // between the two inserts; otherwise two results are legitimate.
        let mut state = worker.state.lock().unwrap();
        let key = JobKey { tab: 0, page: 0 };
        state.queue.insert(key, RenderJobSpec { requested_scale: 1.0, max_texture_side: 2048, document: doc.clone() });
        state.queue.insert(key, RenderJobSpec { requested_scale: 1.5, max_texture_side: 2048, document: doc });
        assert_eq!(state.queue.len(), 1);
        drop(state);
        worker.work_available.notify_one();

        let results = poll_for_a_while(&worker);
        assert_eq!(results.len(), 1, "one submit should have replaced the other, not queued both");
        assert_eq!(results[0].prepared.requested_scale(), 1.5);
    }
}
