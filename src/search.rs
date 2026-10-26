//! In-document `/`/`?` text search: match bookkeeping and the pure index
//! arithmetic behind `n`/`N`. `SpikeApp::run_search` (in `app_state.rs`)
//! builds the `SearchMatch` list by extracting each page's text; this
//! module only has the data types and the "which match is next" math,
//! kept free of `egui`/`Viewer` so it has direct unit tests.

use crate::{selection, text};

/// `/` (forward) vs `?` (backward) — which way `n` repeats a search;
/// `N` repeats the other direction for just that one jump, matching
/// Vim, without changing this stored direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

impl SearchDirection {
    pub fn opposite(self) -> SearchDirection {
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
pub struct SearchMatch {
    pub page: usize,
    pub start: usize,
    pub cursor: selection::Cursor,
    /// Page-local rectangles in PDF points, independent of texture cache.
    pub spans: Vec<text::Rect>,
}

/// Index into `matches` (sorted by `(page, start)` ascending, as
/// `SpikeApp::run_search` builds it) nearest to `(page, char_idx)` in
/// `direction`: the first match strictly after it for `Forward`,
/// wrapping to the first match in the document if none remain ahead;
/// the last match strictly before it for `Backward`, wrapping to the
/// last match. Mirrors Vim's `/pattern<CR>` jumping to the next match
/// from the cursor, not always restarting at the top of the document.
/// Panics if `matches` is empty — callers must check that first.
pub fn nearest_match_index(matches: &[SearchMatch], page: usize, char_idx: usize, direction: SearchDirection) -> usize {
    match direction {
        SearchDirection::Forward => matches.iter().position(|m| (m.page, m.start) > (page, char_idx)).unwrap_or(0),
        SearchDirection::Backward => matches.iter().rposition(|m| (m.page, m.start) < (page, char_idx)).unwrap_or(matches.len() - 1),
    }
}

/// `current` stepped `count` matches in `direction` among `len` total
/// matches, wrapping around — pure index arithmetic for `n`/`N`,
/// independent of how the match list itself was built. Panics if `len`
/// is 0 — callers must check that first.
pub fn step_match_index(current: usize, len: usize, count: usize, direction: SearchDirection) -> usize {
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
}
