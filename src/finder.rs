//! The `Ctrl+F` fuzzy PDF finder: a background directory scan feeding a
//! fuzzy-ranked, keyboard-driven result list. Drawing lives in `ui.rs`.

use eframe::egui;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

pub fn is_pdf_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

pub struct Candidate {
    pub path: PathBuf,
    /// Shown and matched against: the path below the scan root's parent,
    /// so the root's own name disambiguates results from different roots.
    pub label: String,
}

/// A candidate that matched the query, with the label characters to highlight.
pub struct Match {
    pub candidate: usize,
    pub indices: Vec<u32>,
}

pub struct FinderState {
    pub query: String,
    pub candidates: Vec<Candidate>,
    pub matches: Vec<Match>,
    /// Index into `matches`.
    pub selected: usize,
    scan: Receiver<Vec<Candidate>>,
    matcher: Matcher,
}

impl FinderState {
    /// Starts scanning `roots` on a background thread.
    pub fn open(roots: Vec<PathBuf>, max_depth: usize, ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ggpdf-finder-scan".to_string())
            .spawn(move || {
                scan(&roots, max_depth, &mut |batch| {
                    if tx.send(batch).is_ok() {
                        ctx.request_repaint();
                    }
                });
            })
            .expect("spawn finder scan thread");
        Self::new(rx)
    }

    fn new(scan: Receiver<Vec<Candidate>>) -> Self {
        Self {
            query: String::new(),
            candidates: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            scan,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }

    /// Takes in whatever the scan has found since the last call.
    pub fn poll(&mut self) {
        let known = self.candidates.len();
        self.candidates.extend(self.scan.try_iter().flatten());
        if self.candidates.len() != known {
            self.refresh();
        }
    }

    pub fn type_text(&mut self, text: &str) {
        self.query.push_str(text);
        self.query_changed();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.query_changed();
    }

    pub fn move_selection(&mut self, delta: isize) {
        let last = self.matches.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn selected_path(&self) -> Option<&Path> {
        let found = self.matches.get(self.selected)?;
        Some(&self.candidates[found.candidate].path)
    }

    fn query_changed(&mut self) {
        self.selected = 0;
        self.refresh();
    }

    fn refresh(&mut self) {
        self.matches = rank(&mut self.matcher, &self.query, &self.candidates);
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }
}

/// Best matches first; an empty query lists everything alphabetically.
fn rank(matcher: &mut Matcher, query: &str, candidates: &[Candidate]) -> Vec<Match> {
    if query.is_empty() {
        let mut order: Vec<usize> = (0..candidates.len()).collect();
        order.sort_by(|&a, &b| candidates[a].label.cmp(&candidates[b].label));
        return order.into_iter().map(|candidate| Match { candidate, indices: Vec::new() }).collect();
    }
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buffer = Vec::new();
    let mut scored: Vec<(u32, Match)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(candidate, c)| {
            let mut indices = Vec::new();
            let score = pattern.indices(Utf32Str::new(&c.label, &mut buffer), matcher, &mut indices)?;
            indices.sort_unstable();
            indices.dedup();
            Some((score, Match { candidate, indices }))
        })
        .collect();
    // Among equal scores the shorter path is the likelier target.
    scored.sort_by(|(a, am), (b, bm)| b.cmp(a).then_with(|| candidates[am.candidate].label.len().cmp(&candidates[bm.candidate].label.len())));
    scored.into_iter().map(|(_, found)| found).collect()
}

/// Walks each root to `max_depth` levels and passes the PDFs of every
/// directory to `emit` as one batch. Unreadable and missing directories
/// are skipped, as are hidden entries and directories already visited
/// (which breaks symlink loops).
fn scan(roots: &[PathBuf], max_depth: usize, emit: &mut impl FnMut(Vec<Candidate>)) {
    let mut visited = HashSet::new();
    for root in roots {
        let base = root.parent().unwrap_or(root);
        walk(root, base, 0, max_depth, &mut visited, emit);
    }
}

fn walk(dir: &Path, base: &Path, depth: usize, max_depth: usize, visited: &mut HashSet<PathBuf>, emit: &mut impl FnMut(Vec<Candidate>)) {
    let Ok(canonical) = dir.canonicalize() else { return };
    if !visited.insert(canonical) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut found = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        // `fs::metadata` follows symlinks, unlike `DirEntry::file_type`.
        let Ok(metadata) = fs::metadata(&path) else { continue };
        if metadata.is_dir() {
            subdirs.push(path);
        } else if is_pdf_path(&path) {
            let label = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().into_owned();
            found.push(Candidate { path, label });
        }
    }
    if !found.is_empty() {
        emit(found);
    }
    if depth < max_depth {
        for subdir in subdirs {
            walk(&subdir, base, depth + 1, max_depth, visited, emit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(labels: &[&str]) -> Vec<Candidate> {
        labels.iter().map(|label| Candidate { path: PathBuf::from(label), label: label.to_string() }).collect()
    }

    fn ranked_labels<'a>(query: &str, candidates: &'a [Candidate]) -> Vec<&'a str> {
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        rank(&mut matcher, query, candidates).iter().map(|m| candidates[m.candidate].label.as_str()).collect()
    }

    fn scanned_labels(roots: &[PathBuf], max_depth: usize) -> Vec<String> {
        let mut labels = Vec::new();
        scan(roots, max_depth, &mut |batch| labels.extend(batch.into_iter().map(|c| c.label)));
        labels.sort();
        labels
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ggpdf-finder-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rank_orders_better_matches_first() {
        let c = candidates(&["papers/a_long_report_final.pdf", "papers/report.pdf"]);
        assert_eq!(ranked_labels("report", &c), ["papers/report.pdf", "papers/a_long_report_final.pdf"]);
    }

    #[test]
    fn rank_drops_non_matches() {
        let c = candidates(&["papers/report.pdf", "papers/notes.pdf"]);
        assert_eq!(ranked_labels("zzz", &c), Vec::<&str>::new());
    }

    #[test]
    fn rank_is_case_insensitive_until_query_has_capitals() {
        let c = candidates(&["papers/Report.pdf", "papers/report.pdf"]);
        assert_eq!(ranked_labels("report", &c).len(), 2);
        assert_eq!(ranked_labels("Report", &c), ["papers/Report.pdf"]);
    }

    #[test]
    fn rank_reports_matched_character_positions() {
        let c = candidates(&["abc.pdf"]);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        assert_eq!(rank(&mut matcher, "ac", &c)[0].indices, [0, 2]);
    }

    #[test]
    fn empty_query_lists_everything_alphabetically() {
        let c = candidates(&["b.pdf", "a.pdf"]);
        assert_eq!(ranked_labels("", &c), ["a.pdf", "b.pdf"]);
    }

    #[test]
    fn scan_finds_nested_pdfs_and_skips_other_files_and_hidden_dirs() {
        let root = temp_dir("scan").join("papers");
        touch(&root.join("a.pdf"));
        touch(&root.join("sub/b.PDF"));
        touch(&root.join("notes.txt"));
        touch(&root.join(".hidden/c.pdf"));
        let labels = scanned_labels(std::slice::from_ref(&root), 4);
        assert_eq!(labels, ["papers/a.pdf", "papers/sub/b.PDF"].map(|l| Path::new(l).to_string_lossy().into_owned()));
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[test]
    fn scan_stops_at_max_depth() {
        let root = temp_dir("depth").join("papers");
        touch(&root.join("a.pdf"));
        touch(&root.join("one/b.pdf"));
        touch(&root.join("one/two/c.pdf"));
        assert_eq!(scanned_labels(std::slice::from_ref(&root), 0).len(), 1);
        assert_eq!(scanned_labels(std::slice::from_ref(&root), 1).len(), 2);
        assert_eq!(scanned_labels(std::slice::from_ref(&root), 2).len(), 3);
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[test]
    fn scan_ignores_missing_roots() {
        assert!(scanned_labels(&[PathBuf::from("/nonexistent/ggpdf-finder")], 4).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn scan_survives_symlink_loops() {
        let root = temp_dir("loop").join("papers");
        touch(&root.join("a.pdf"));
        std::os::unix::fs::symlink(&root, root.join("again")).unwrap();
        assert_eq!(scanned_labels(std::slice::from_ref(&root), 8).len(), 1);
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    fn state_with(labels: &[&str]) -> FinderState {
        let (tx, rx) = mpsc::channel();
        tx.send(candidates(labels)).unwrap();
        let mut state = FinderState::new(rx);
        state.poll();
        state
    }

    #[test]
    fn selection_is_clamped_to_the_matches() {
        let mut state = state_with(&["a1.pdf", "a2.pdf", "b.pdf"]);
        state.move_selection(10);
        assert_eq!(state.selected, 2);
        state.move_selection(-10);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn selection_resets_and_stays_valid_when_the_filter_shrinks() {
        let mut state = state_with(&["a1.pdf", "a2.pdf", "b.pdf"]);
        state.move_selection(2);
        state.type_text("b");
        assert_eq!(state.matches.len(), 1);
        assert_eq!(state.selected, 0);
        assert_eq!(state.selected_path(), Some(Path::new("b.pdf")));
    }

    #[test]
    fn selection_with_no_matches_has_no_path() {
        let mut state = state_with(&["a.pdf"]);
        state.type_text("zzz");
        state.move_selection(1);
        assert_eq!(state.selected_path(), None);
    }
}
