//! Open documents and active-tab state (Port order item 1, Document
//! viewer shell — the open/tab-switching slice of it). Rendering, zoom,
//! rotation, dark mode, and search stay out of scope here; they belong to
//! `render.rs`/`viewer.rs` as those modules get built. "Open with" OS
//! handoff and single-instance IPC are deferred to Port order item 5
//! (Lifecycle) — not needed to exercise CLI-arg multi-document opening.

use crate::document::{Document, DocumentError, PdfLibrary};
use std::path::PathBuf;

pub struct OpenDocument<'lib> {
    pub path: PathBuf,
    pub document: Document<'lib>,
}

/// Open documents plus which one is active. Page position within a
/// document is the caller's concern (kept in the UI layer, not here) —
/// this only tracks *which document*, not where in it.
pub struct App<'lib> {
    library: &'lib PdfLibrary,
    docs: Vec<OpenDocument<'lib>>,
    active: usize,
}

impl<'lib> App<'lib> {
    pub fn new(library: &'lib PdfLibrary) -> Self {
        Self {
            library,
            docs: Vec::new(),
            active: 0,
        }
    }

    /// Opens `path` as a new tab and makes it active.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), DocumentError> {
        let path = path.into();
        let document = self.library.open(&path)?;
        self.docs.push(OpenDocument { path, document });
        self.active = self.docs.len() - 1;
        Ok(())
    }

    pub fn active(&self) -> Option<&OpenDocument<'lib>> {
        self.docs.get(self.active)
    }

    pub fn tab_count(&self) -> usize {
        self.docs.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn tab_label(&self, index: usize) -> Option<&str> {
        self.docs
            .get(index)
            .and_then(|d| d.path.file_name())
            .and_then(|n| n.to_str())
    }

    pub fn next_tab(&mut self) {
        if !self.docs.is_empty() {
            self.active = (self.active + 1) % self.docs.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.docs.is_empty() {
            self.active = (self.active + self.docs.len() - 1) % self.docs.len();
        }
    }

    /// Jumps directly to tab `index`. No-op if out of range.
    pub fn jump_to_tab(&mut self, index: usize) {
        if index < self.docs.len() {
            self.active = index;
        }
    }

    /// Closes the active tab. The next tab (or the previous one, if the
    /// closed tab was last) becomes active.
    ///
    /// Not yet wired to a key in `main.rs`'s spike UI — no close-tab
    /// binding exists there yet (only whole-app close via `q`/`Esc`).
    #[allow(dead_code)]
    pub fn close_active(&mut self) {
        if self.docs.is_empty() {
            return;
        }
        self.docs.remove(self.active);
        if self.active >= self.docs.len() && self.active > 0 {
            self.active -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::document::test_library;

    #[test]
    fn open_makes_new_tab_active() {
        let library = test_library();
        let mut app = App::new(library);
        app.open("assets/test-pdfs/two-column.pdf").unwrap();
        assert_eq!(app.tab_count(), 1);
        assert_eq!(app.active_index(), 0);

        app.open("assets/test-pdfs/rotated.pdf").unwrap();
        assert_eq!(app.tab_count(), 2);
        assert_eq!(app.active_index(), 1);
    }

    #[test]
    fn next_and_prev_tab_wrap_around() {
        let library = test_library();
        let mut app = App::new(library);
        app.open("assets/test-pdfs/two-column.pdf").unwrap();
        app.open("assets/test-pdfs/rotated.pdf").unwrap();
        app.open("assets/test-pdfs/ligatures.pdf").unwrap();
        assert_eq!(app.active_index(), 2);

        app.next_tab();
        assert_eq!(app.active_index(), 0, "next_tab should wrap past the last tab");

        app.prev_tab();
        assert_eq!(app.active_index(), 2, "prev_tab should wrap before the first tab");
    }

    #[test]
    fn jump_to_tab_ignores_out_of_range() {
        let library = test_library();
        let mut app = App::new(library);
        app.open("assets/test-pdfs/two-column.pdf").unwrap();
        app.open("assets/test-pdfs/rotated.pdf").unwrap();

        app.jump_to_tab(0);
        assert_eq!(app.active_index(), 0);

        app.jump_to_tab(99);
        assert_eq!(app.active_index(), 0, "out-of-range jump should be a no-op");
    }

    #[test]
    fn close_active_falls_back_to_previous_tab_when_last_is_closed() {
        let library = test_library();
        let mut app = App::new(library);
        app.open("assets/test-pdfs/two-column.pdf").unwrap();
        app.open("assets/test-pdfs/rotated.pdf").unwrap();
        assert_eq!(app.active_index(), 1);

        app.close_active();
        assert_eq!(app.tab_count(), 1);
        assert_eq!(app.active_index(), 0, "closing the last tab should select the previous one");
    }

    #[test]
    fn close_active_on_empty_app_does_not_panic() {
        let library = test_library();
        let mut app = App::new(library);
        app.close_active();
        assert_eq!(app.tab_count(), 0);
    }

    #[test]
    fn open_nonexistent_file_errors_without_changing_state() {
        let library = test_library();
        let mut app = App::new(library);
        app.open("assets/test-pdfs/two-column.pdf").unwrap();

        let result = app.open("assets/test-pdfs/does-not-exist.pdf");
        assert!(result.is_err());
        assert_eq!(app.tab_count(), 1, "a failed open should not leave a half-open tab");
        assert_eq!(app.active_index(), 0);
    }

    #[test]
    fn tab_label_is_file_name() {
        let library = test_library();
        let mut app = App::new(library);
        app.open("assets/test-pdfs/two-column.pdf").unwrap();
        assert_eq!(app.tab_label(0), Some("two-column.pdf"));
    }
}
