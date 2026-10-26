//! Page render scheduling (Port order item 1/5's "Rendering/performance
//! rules" section: render near the viewport first, with a small
//! prefetch margin, and bound memory for long documents). Pure
//! page-index arithmetic — no PDFium, no egui, consistent with
//! `text.rs`/`selection.rs`'s own plain-data boundary. `main.rs`
//! supplies "which page is near the viewport" (derived from painted
//! page rects — see its `page_near`) and whether a page is already
//! rendered/stale; this module only decides *which indices* to render
//! or evict.
//!
//! Generation tagging (PLAN.md's "tag render jobs with a generation id
//! so a stale zoom/rotation/closed-doc result can never overwrite
//! current state") is deliberately not implemented: every `render_page`
//! call here runs synchronously on the UI thread, one call per frame's
//! budget slot, so there is no background job whose result could ever
//! arrive stale after a zoom/tab/doc change — the exact race that
//! tagging exists to prevent doesn't occur in this threading model.
//! Revisit if rendering ever moves off the UI thread.

/// Up to `budget` page indices to render next, expanding outward from
/// `current_page` (current, then +-1, +-2, ...) among pages where
/// `is_unrendered(index)` is true. Replaces always-index-order
/// scanning: jumping to a late page (`G`, a future "goto page") used to
/// render every earlier page first before ever reaching the one
/// actually on screen — the "prefetch margin" PLAN.md's Rendering/
/// performance rules call for is this outward expansion, not a fixed
/// lookahead distance, since the margin should follow wherever the
/// user actually is, not just forward from page 0.
pub fn pages_to_render(current_page: usize, page_count: usize, is_unrendered: impl Fn(usize) -> bool, budget: usize) -> Vec<usize> {
    if page_count == 0 || budget == 0 {
        return Vec::new();
    }
    let current_page = current_page.min(page_count - 1);
    let mut out = Vec::new();
    let mut offset = 0usize;
    loop {
        let mut any_in_range = false;
        if offset == 0 {
            any_in_range = true;
            if is_unrendered(current_page) {
                out.push(current_page);
            }
        } else {
            if let Some(i) = current_page.checked_sub(offset) {
                any_in_range = true;
                if is_unrendered(i) {
                    out.push(i);
                }
            }
            if current_page + offset < page_count {
                any_in_range = true;
                if is_unrendered(current_page + offset) {
                    out.push(current_page + offset);
                }
            }
        }
        if out.len() >= budget || !any_in_range {
            break;
        }
        offset += 1;
    }
    out.truncate(budget);
    out
}

/// Only schedule pages close enough to become visible on the next scroll.
/// Unlike a budget over the whole document, this does not fill unused
/// slots with distant pages when the nearby window is already cached.
pub fn nearby_pages_to_render(current_page: usize, page_count: usize, radius: usize, is_unrendered: impl Fn(usize) -> bool) -> Vec<usize> {
    let width = radius.saturating_mul(2).saturating_add(1);
    pages_to_render(current_page, page_count, |i| i.abs_diff(current_page) <= radius && is_unrendered(i), width)
}

/// Indices of already-rendered pages more than `keep_radius` pages away
/// from `current_page` (either direction) — `main.rs` frees their GPU
/// texture (sets `rendered = None`, to be lazily re-rasterized if
/// scrolled back to) instead of holding every page's bitmap in memory
/// for as long as the document stays open, satisfying PLAN.md's "sane
/// memory limits for large PDFs" rule (previously unimplemented: pages
/// only ever got rendered, never freed).
pub fn pages_to_evict(current_page: usize, page_count: usize, is_rendered: impl Fn(usize) -> bool, keep_radius: usize) -> Vec<usize> {
    (0..page_count).filter(|&i| is_rendered(i) && i.abs_diff(current_page) > keep_radius).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_outward_from_current_page_not_from_zero() {
        // All pages unrendered, jumped straight to page 50 of 100 pages
        // (mirrors `G`/document_end): the pages nearest page 50 should
        // come first, not pages 0-2.
        let rendered = vec![false; 100];
        let picked = pages_to_render(50, 100, |i| !rendered[i], 3);
        assert_eq!(picked, vec![50, 49, 51]);
    }

    #[test]
    fn skips_already_rendered_pages_within_the_budget() {
        let mut rendered = vec![false; 10];
        rendered[5] = true; // current page already rendered
        rendered[4] = true; // one neighbor already rendered too
        let picked = pages_to_render(5, 10, |i| !rendered[i], 2);
        assert_eq!(picked, vec![6, 3], "should skip 5 and 4, continuing outward for the budget");
    }

    #[test]
    fn stops_expanding_once_both_directions_run_off_the_document() {
        // Only 2 pages, current page 0: offset 1 covers page 1, offset 2
        // has nothing left on either side and must terminate, not loop.
        let rendered = vec![false; 2];
        let picked = pages_to_render(0, 2, |i| !rendered[i], 10);
        assert_eq!(picked, vec![0, 1]);
    }

    #[test]
    fn current_page_past_the_end_clamps_to_the_last_page() {
        let rendered = vec![false; 5];
        let picked = pages_to_render(999, 5, |i| !rendered[i], 1);
        assert_eq!(picked, vec![4]);
    }

    #[test]
    fn empty_document_or_zero_budget_renders_nothing() {
        assert_eq!(pages_to_render(0, 0, |_| true, 5), Vec::<usize>::new());
        assert_eq!(pages_to_render(0, 10, |_| true, 0), Vec::<usize>::new());
    }

    #[test]
    fn evicts_only_pages_outside_the_keep_radius() {
        let rendered = vec![true; 20];
        let picked = pages_to_evict(10, 20, |i| rendered[i], 3);
        // Within [7, 13] is kept; everything else rendered gets evicted.
        assert_eq!(picked, vec![0, 1, 2, 3, 4, 5, 6, 14, 15, 16, 17, 18, 19]);
    }

    #[test]
    fn evict_ignores_pages_that_were_never_rendered() {
        let rendered = vec![false; 5];
        assert_eq!(pages_to_evict(0, 5, |i| rendered[i], 0), Vec::<usize>::new());
    }

    #[test]
    fn nearby_window_does_not_schedule_distant_pages_to_fill_budget() {
        assert_eq!(nearby_pages_to_render(200, 486, 3, |_| true), vec![200, 199, 201, 198, 202, 197, 203]);
        assert!(nearby_pages_to_render(200, 486, 3, |i| i == 210).is_empty());
        assert_eq!(nearby_pages_to_render(0, 486, 3, |_| true), vec![0, 1, 2, 3]);
    }
}
