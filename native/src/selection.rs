//! Keyboard cursor/motion model over one page's reading-order lines
//! (Port order item 3's motions, and the model item 4 "Selection UI"
//! will render). Ported from `visual.js`'s `move()`/`textObject()`; the
//! unit tests below are adapted directly from `visual.test.js`'s
//! data-driven cases, which is the behavioral oracle for this file.
//!
//! Scope of this port: single-page motions and text objects only. `j`/`k`
//! crossing a page boundary, the full `v`/`V` mark-and-yank interactive
//! loop, and numeric count-prefix dispatch (`input.rs` doesn't accumulate
//! counts yet) are not wired up — see `main.rs`'s `apply_action` for what
//! is and isn't connected to the live viewer yet.
//!
//! `text.rs`'s `position_chars` already ports `visual.js`'s `collect()`
//! synthetic-space insertion (a PDF whose content stream omits an
//! explicit space glyph between words no longer misjoins them by the
//! time characters reach this module).

use crate::text::{PositionedChar, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub line: usize,
    pub character: usize,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub chars: Vec<PositionedChar>,
}

impl Line {
    /// Bounding rect over every character on this line, or `None` for an
    /// empty line. Used by `text_object_paragraph`'s gap heuristic and by
    /// `main.rs` to position gutter line-number labels.
    pub fn rect(&self) -> Option<Rect> {
        let mut iter = self.chars.iter();
        let first = iter.next()?;
        let mut r = first.rect;
        for c in iter {
            r.left = r.left.min(c.rect.left);
            r.top = r.top.min(c.rect.top);
            r.right = r.right.max(c.rect.right);
            r.bottom = r.bottom.max(c.rect.bottom);
        }
        Some(r)
    }
}

/// Groups a page's raw reading-order characters into visual lines, using
/// the same heuristic as `visual.js`'s `collect()`: a character starts a
/// new line when it doesn't vertically overlap the current line's rect,
/// or when its left edge is well left of the last character seen (covers
/// PDFs whose extraction omits explicit line breaks). Relies on PDFium
/// already yielding characters in reading order (confirmed for the
/// multi-column gate fixture — see PLAN.md).
pub fn group_into_lines(chars: &[PositionedChar]) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut current: Vec<PositionedChar> = Vec::new();
    let mut current_rect: Option<Rect> = None;
    let mut last_rect: Option<Rect> = None;

    for &c in chars {
        let starts_new = match (current_rect, last_rect) {
            (Some(cr), Some(lr)) => c.rect.top >= cr.bottom || c.rect.bottom <= cr.top || c.rect.left < lr.left - 2.0,
            _ => false,
        };
        if starts_new && !current.is_empty() {
            lines.push(Line { chars: std::mem::take(&mut current) });
            current_rect = None;
        }
        current_rect = Some(match current_rect {
            Some(mut r) => {
                r.left = r.left.min(c.rect.left);
                r.top = r.top.min(c.rect.top);
                r.right = r.right.max(c.rect.right);
                r.bottom = r.bottom.max(c.rect.bottom);
                r
            }
            None => c.rect,
        });
        last_rect = Some(c.rect);
        current.push(c);
    }
    if !current.is_empty() {
        lines.push(Line { chars: current });
    }
    lines
}

/// One merged bounding rect per line covered by the flat character range
/// `[start_flat, end_flat]` (inclusive both ends, indices into the same
/// flattened order `group_into_lines` consumed — i.e. `main.rs`'s
/// `cursor_idx`/`anchor_idx` space). For rendering a selection highlight:
/// filling each character's own rect individually leaves visible gaps
/// wherever a glyph's box is narrow or doesn't quite touch its neighbor
/// (most visibly at space characters, whose PDFium-reported bounds are
/// often degenerate/narrow) — merging each line's covered span into one
/// rect avoids that regardless of individual glyph box quirks.
pub fn selection_span_rects(lines: &[Line], start_flat: usize, end_flat: usize) -> Vec<Rect> {
    let mut rects = Vec::new();
    let mut offset = 0usize;
    for line in lines {
        let line_len = line.chars.len();
        if line_len == 0 {
            continue;
        }
        let line_start = offset;
        let line_last = line_start + line_len - 1;
        offset += line_len;
        if start_flat > line_last || end_flat < line_start {
            continue;
        }
        let from = start_flat.max(line_start) - line_start;
        let to = end_flat.min(line_last) - line_start;
        let mut r = line.chars[from].rect;
        for c in &line.chars[from + 1..=to] {
            r.left = r.left.min(c.rect.left);
            r.top = r.top.min(c.rect.top);
            r.right = r.right.max(c.rect.right);
            r.bottom = r.bottom.max(c.rect.bottom);
        }
        rects.push(r);
    }
    rects
}

/// Every `(start, end)` flat-index match of `query` in `chars` (`end`
/// inclusive) — directly comparable with the rest of the codebase's
/// existing flat per-page character indices (`cursor_idx`, `flat_index`,
/// `selection_span_rects`). Case-sensitive, matching Vim's own `/`/`?`
/// default (no `ignorecase`). Naive O(n*m) scan: fine for interactive
/// per-page searches, not meant for a huge corpus.
pub fn find_matches(chars: &[PositionedChar], query: &[char]) -> Vec<(usize, usize)> {
    if query.is_empty() || chars.len() < query.len() {
        return Vec::new();
    }
    (0..=chars.len() - query.len())
        .filter(|&start| (0..query.len()).all(|i| chars[start + i].ch == query[i]))
        .map(|start| (start, start + query.len() - 1))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBackward,
    WordEnd,
    BigWordForward,
    BigWordBackward,
    BigWordEnd,
    LineStart,
    LineFirstNonBlank,
    LineEnd,
}

/// Moves `cursor` by `motion`, repeated `count` times. `column` is the
/// sticky target column for `Up`/`Down` (read, not written, by vertical
/// motions — matches `visual.js`'s `this.column`); horizontal and word
/// motions update it to the new character position.
pub fn move_cursor(lines: &[Line], cursor: Cursor, motion: Motion, count: usize, column: &mut usize) -> Cursor {
    if lines.is_empty() || lines.get(cursor.line).is_none() {
        return cursor;
    }
    let count = count.max(1);
    let next = match motion {
        Motion::Left | Motion::Right => horizontal(lines, cursor, motion, count),
        Motion::Up | Motion::Down => vertical(lines, cursor, motion, count, *column),
        Motion::LineStart | Motion::LineFirstNonBlank | Motion::LineEnd => line_motion(lines, cursor, motion),
        _ => word_motion(lines, cursor, motion, count),
    };
    if !matches!(motion, Motion::Up | Motion::Down) {
        *column = next.character;
    }
    next
}

fn horizontal(lines: &[Line], cursor: Cursor, motion: Motion, count: usize) -> Cursor {
    let line = &lines[cursor.line];
    let delta = count as isize * if motion == Motion::Left { -1 } else { 1 };
    let max_idx = line.chars.len().saturating_sub(1) as isize;
    let character = (cursor.character as isize + delta).clamp(0, max_idx) as usize;
    Cursor { line: cursor.line, character }
}

fn vertical(lines: &[Line], cursor: Cursor, motion: Motion, count: usize, column: usize) -> Cursor {
    let direction: isize = if motion == Motion::Down { 1 } else { -1 };
    let max_line = lines.len() - 1;
    let line = (cursor.line as isize + direction * count as isize).clamp(0, max_line as isize) as usize;
    let max_char = lines[line].chars.len().saturating_sub(1);
    Cursor { line, character: column.min(max_char) }
}

fn line_motion(lines: &[Line], cursor: Cursor, motion: Motion) -> Cursor {
    let line = &lines[cursor.line];
    let character = match motion {
        Motion::LineStart => 0,
        Motion::LineEnd => line.chars.len().saturating_sub(1),
        Motion::LineFirstNonBlank => line.chars.iter().position(|c| !c.ch.is_whitespace()).unwrap_or(0),
        _ => unreachable!(),
    };
    Cursor { line: cursor.line, character }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharType {
    Space,
    Word,
    Punct,
}

fn char_type(ch: char, big_word: bool) -> CharType {
    if ch.is_whitespace() {
        CharType::Space
    } else if big_word || ch.is_alphanumeric() || ch == '_' {
        CharType::Word
    } else {
        CharType::Punct
    }
}

fn word_motion(lines: &[Line], cursor: Cursor, motion: Motion, count: usize) -> Cursor {
    let big = matches!(motion, Motion::BigWordForward | Motion::BigWordBackward | Motion::BigWordEnd);
    let positions: Vec<(usize, usize, CharType)> = lines
        .iter()
        .enumerate()
        .flat_map(|(li, line)| line.chars.iter().enumerate().map(move |(ci, c)| (li, ci, char_type(c.ch, big))))
        .collect();

    let Some(current) = positions.iter().position(|&(l, c, _)| l == cursor.line && c == cursor.character) else {
        return cursor;
    };

    let mut index = current;
    for _ in 0..count {
        let before = index;
        match motion {
            Motion::WordForward | Motion::BigWordForward => {
                let ty = positions[index].2;
                if ty != CharType::Space {
                    while index + 1 < positions.len() && positions[index + 1].0 == positions[index].0 && positions[index + 1].2 == ty {
                        index += 1;
                    }
                }
                index += 1;
                while index < positions.len() && positions[index].2 == CharType::Space {
                    index += 1;
                }
                if index == positions.len() {
                    index = before;
                }
            }
            Motion::WordBackward | Motion::BigWordBackward => {
                index = index.saturating_sub(1);
                while index > 0 && positions[index].2 == CharType::Space {
                    index -= 1;
                }
                let ty = positions[index].2;
                while index > 0 && positions[index - 1].0 == positions[index].0 && positions[index - 1].2 == ty {
                    index -= 1;
                }
            }
            Motion::WordEnd | Motion::BigWordEnd => {
                let ty = positions[index].2;
                if ty == CharType::Space {
                    while index + 1 < positions.len() && positions[index].2 == CharType::Space {
                        index += 1;
                    }
                } else if index + 1 < positions.len() && (positions[index + 1].2 != ty || positions[index + 1].0 != positions[index].0) {
                    index += 1;
                    while index + 1 < positions.len() && positions[index].2 == CharType::Space {
                        index += 1;
                    }
                }
                let word = positions[index].2;
                while index + 1 < positions.len() && positions[index + 1].0 == positions[index].0 && positions[index + 1].2 == word {
                    index += 1;
                }
            }
            _ => unreachable!(),
        }
        if index == before {
            break;
        }
    }

    let (line, character, _) = positions[index];
    Cursor { line, character }
}

/// `is`/`ip` in `visual.js`'s terms. Returns the inclusive `(start, end)`
/// bounds of the text object containing `cursor`, or `None` if the page
/// has no text to select (only possible for `sentence`).
pub fn text_object_paragraph(lines: &[Line], cursor: Cursor) -> (Cursor, Cursor) {
    let starts_paragraph = |index: usize| -> bool {
        if index == 0 {
            return true;
        }
        let (Some(prev), Some(cur)) = (lines[index - 1].rect(), lines[index].rect()) else {
            return true;
        };
        let height = prev.bottom - prev.top;
        cur.top < prev.top || cur.top - prev.bottom > height * 0.6 || cur.left - prev.left > height * 0.6
    };

    let mut first = cursor.line;
    let mut last = cursor.line;
    while first > 0 && !starts_paragraph(first) {
        first -= 1;
    }
    while last + 1 < lines.len() && !starts_paragraph(last + 1) {
        last += 1;
    }
    let last_char = lines[last].chars.len().saturating_sub(1);
    (Cursor { line: first, character: 0 }, Cursor { line: last, character: last_char })
}

struct SentenceEntry {
    start: usize,
    end: usize,
    position: Cursor,
    is_whitespace: bool,
}

/// Splits `chars` into sentence spans `[start, end)` using a punctuation
/// + trailing-whitespace heuristic (`.`/`!`/`?`, optionally followed by a
/// closing quote/bracket, then whitespace or end of text). This is not
/// full UAX #29 sentence segmentation (no abbreviation handling, e.g.
/// "Dr. Smith" breaks after "Dr.") — a reasonable approximation for
/// prose, not a guarantee; `visual.js` uses the browser's
/// `Intl.Segmenter` instead, which Rust has no equivalent built in for.
fn split_sentences(chars: &[char]) -> Vec<(usize, usize)> {
    let mut bounds = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if matches!(chars[i], '.' | '!' | '?') {
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j], '"' | '\'' | ')' | ']') {
                j += 1;
            }
            if j >= chars.len() || chars[j].is_whitespace() {
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                bounds.push((start, j));
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if start < chars.len() {
        bounds.push((start, chars.len()));
    }
    if bounds.is_empty() && !chars.is_empty() {
        bounds.push((0, chars.len()));
    }
    bounds
}

pub fn text_object_sentence(lines: &[Line], cursor: Cursor) -> Option<(Cursor, Cursor)> {
    let mut text_len = 0usize;
    let mut entries: Vec<SentenceEntry> = Vec::new();
    let mut full_text = String::new();
    let mut cursor_offset = 0usize;

    for (li, line) in lines.iter().enumerate() {
        if li != 0 {
            full_text.push(' ');
            text_len += 1;
        }
        for (ci, c) in line.chars.iter().enumerate() {
            if li == cursor.line && ci == cursor.character {
                cursor_offset = text_len;
            }
            entries.push(SentenceEntry {
                start: text_len,
                end: text_len + 1,
                position: Cursor { line: li, character: ci },
                is_whitespace: c.ch.is_whitespace(),
            });
            full_text.push(c.ch);
            text_len += 1;
        }
    }

    let chars: Vec<char> = full_text.chars().collect();
    let segments = split_sentences(&chars);
    let &(seg_start, seg_end) = segments
        .iter()
        .find(|&&(s, e)| cursor_offset >= s && cursor_offset < e)
        .or_else(|| segments.last())?;

    let members: Vec<&SentenceEntry> = entries.iter().filter(|e| e.start < seg_end && e.end > seg_start && !e.is_whitespace).collect();

    let first = members.first()?;
    let last = members.last()?;
    Some((first.position, last.position))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Rect;

    fn char_at(ch: char, left: f32, top: f32, width: f32, height: f32) -> PositionedChar {
        PositionedChar { ch, rect: Rect { left, top, right: left + width, bottom: top + height } }
    }

    /// Builds lines the way the JS test helper's `viewer()` does: one
    /// line per string, each line 10px tall at `index * 12`, with an
    /// extra 15px gap for any index in `breaks` (simulating a paragraph
    /// break).
    fn lines_from(texts: &[&str], breaks: &[usize]) -> Vec<Line> {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let top = (index * 12) as f32 + if breaks.contains(&index) { 15.0 } else { 0.0 };
                let chars: Vec<PositionedChar> = text
                    .chars()
                    .enumerate()
                    .map(|(i, ch)| char_at(ch, i as f32, top, 1.0, 10.0))
                    .collect();
                Line { chars }
            })
            .collect()
    }

    fn selected_text(lines: &[Line], start: Cursor, end: Cursor) -> String {
        let (start, end) = if (start.line, start.character) <= (end.line, end.character) { (start, end) } else { (end, start) };
        let mut out = Vec::new();
        for li in start.line..=end.line {
            let line = &lines[li];
            let from = if li == start.line { start.character } else { 0 };
            let to = if li == end.line { end.character + 1 } else { line.chars.len() };
            out.push(line.chars[from..to].iter().map(|c| c.ch).collect::<String>());
        }
        out.join("\n")
    }

    #[test]
    fn sentence_text_object_spans_lines_and_stops_before_next_sentence() {
        let lines = lines_from(&["First sentence", "continues. Second one."], &[]);
        let (start, end) = text_object_sentence(&lines, Cursor { line: 1, character: 3 }).unwrap();
        assert_eq!(selected_text(&lines, start, end), "First sentence\ncontinues.");

        let (start, end) = text_object_sentence(&lines, Cursor { line: 1, character: 18 }).unwrap();
        assert_eq!(selected_text(&lines, start, end), "Second one.");
    }

    #[test]
    fn paragraph_text_object_uses_line_gap_and_stays_on_cursor_line_group() {
        let lines = lines_from(&["First line", "still first", "Next paragraph"], &[2]);
        let (start, end) = text_object_paragraph(&lines, Cursor { line: 1, character: 0 });
        assert_eq!((start.line, end.line), (0, 1));

        let (start, end) = text_object_paragraph(&lines, Cursor { line: 2, character: 0 });
        assert_eq!((start.line, end.line), (2, 2));
    }

    #[test]
    fn word_motions_cross_lines_and_distinguish_punctuation() {
        let lines = lines_from(&["one, two", "three"], &[]);
        let mut column = 0;
        let cursor = Cursor { line: 0, character: 0 };

        let cursor = move_cursor(&lines, cursor, Motion::WordForward, 1, &mut column);
        assert_eq!((cursor.line, cursor.character), (0, 3)); // ","
        let cursor = move_cursor(&lines, cursor, Motion::WordForward, 1, &mut column);
        assert_eq!((cursor.line, cursor.character), (0, 5)); // "two"
        let cursor = move_cursor(&lines, cursor, Motion::WordEnd, 1, &mut column);
        assert_eq!(cursor.character, 7); // end of "two"
        let cursor = move_cursor(&lines, cursor, Motion::WordForward, 1, &mut column);
        assert_eq!((cursor.line, cursor.character), (1, 0)); // "three"
        let cursor = move_cursor(&lines, cursor, Motion::WordBackward, 1, &mut column);
        assert_eq!((cursor.line, cursor.character), (0, 5));
    }

    #[test]
    fn counted_word_and_big_word_motions_handle_punctuation_differently() {
        let lines = lines_from(&["one,two three", "four-five six"], &[]);
        let mut column = 0;
        let cursor = Cursor { line: 0, character: 0 };

        let cursor = move_cursor(&lines, cursor, Motion::WordForward, 2, &mut column);
        assert_eq!(cursor.character, 4); // "two"
        let cursor = move_cursor(&lines, cursor, Motion::BigWordBackward, 1, &mut column);
        assert_eq!(cursor.character, 0); // "one,two"
        let cursor = move_cursor(&lines, cursor, Motion::BigWordForward, 2, &mut column);
        assert_eq!((cursor.line, cursor.character), (1, 0));
        let cursor = move_cursor(&lines, cursor, Motion::BigWordEnd, 1, &mut column);
        assert_eq!(cursor.character, 8); // end of "four-five"
        let cursor = move_cursor(&lines, cursor, Motion::WordBackward, 4, &mut column);
        assert_eq!((cursor.line, cursor.character), (0, 8)); // "three"
    }

    #[test]
    fn counted_horizontal_motions_clamp_and_caret_finds_first_nonblank() {
        let lines = lines_from(&["  hello"], &[]);
        let mut column = 0;
        let cursor = Cursor { line: 0, character: 0 };

        let cursor = move_cursor(&lines, cursor, Motion::LineFirstNonBlank, 1, &mut column);
        assert_eq!(cursor.character, 2);
        let cursor = move_cursor(&lines, cursor, Motion::Right, 9, &mut column);
        assert_eq!(cursor.character, 6); // clamped to last character
        let cursor = move_cursor(&lines, cursor, Motion::Left, 3, &mut column);
        assert_eq!(cursor.character, 3);
    }

    #[test]
    fn counted_line_motions_move_cursor_and_preserve_sticky_column() {
        let lines = lines_from(&["abc", "d", "ef", "ghi", "jkl"], &[]);
        let mut column = 2;
        let cursor = Cursor { line: 0, character: 2 };

        let cursor = move_cursor(&lines, cursor, Motion::Down, 3, &mut column);
        assert_eq!((cursor.line, cursor.character), (3, 2));
        let cursor = move_cursor(&lines, cursor, Motion::Up, 2, &mut column);
        // line 1 ("d") only has 1 character, so the sticky column (2)
        // clamps to its last character (0) without changing `column`.
        assert_eq!((cursor.line, cursor.character), (1, 0));
        assert_eq!(column, 2, "vertical motion must not overwrite the sticky column");

        let cursor = move_cursor(&lines, cursor, Motion::LineStart, 1, &mut column);
        assert_eq!(cursor.character, 0);
        assert_eq!(column, 0, "horizontal/line motion should update the sticky column");
    }

    #[test]
    fn selection_span_rects_merges_a_line_into_one_continuous_rect() {
        // Regression check: filling each character's own (possibly
        // narrow/degenerate, especially for spaces) rect individually
        // left visible gaps in the highlight between words (found live:
        // "visual selection has gaps between the words").
        let lines = lines_from(&["one two"], &[]);
        // "one" = 0..=2, " " = 3, "two" = 4..=6 — select across the space.
        let rects = selection_span_rects(&lines, 0, 6);
        assert_eq!(rects.len(), 1, "a single line's selection should merge into one rect");
        let r = rects[0];
        assert_eq!(r.left, 0.0, "should start at the first selected char's left edge");
        assert_eq!(r.right, 7.0, "should end at the last selected char's right edge, with no gap at the space");
    }

    #[test]
    fn selection_span_rects_produces_one_rect_per_covered_line() {
        let lines = lines_from(&["abc", "def", "ghi"], &[]);
        // Flat indices: "abc"=0..=2, "def"=3..=5, "ghi"=6..=8. Select
        // from the middle of line 0 through the middle of line 2.
        let rects = selection_span_rects(&lines, 1, 7);
        assert_eq!(rects.len(), 3, "should emit one merged rect per covered line, not per character");
    }

    #[test]
    fn group_into_lines_splits_on_vertical_gap() {
        let chars = vec![
            char_at('a', 0.0, 0.0, 5.0, 10.0),
            char_at('b', 5.0, 0.0, 5.0, 10.0),
            char_at('c', 0.0, 12.0, 5.0, 10.0),
        ];
        let lines = group_into_lines(&chars);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].chars.len(), 2);
        assert_eq!(lines[1].chars.len(), 1);
    }

    #[test]
    fn group_into_lines_splits_on_leftward_jump_without_vertical_gap() {
        // Same vertical band, but the third char restarts far to the
        // left: a second column on the same text-row band.
        let chars = vec![
            char_at('a', 0.0, 0.0, 5.0, 10.0),
            char_at('b', 5.0, 0.0, 5.0, 10.0),
            char_at('c', 0.0, 2.0, 5.0, 10.0),
        ];
        let lines = group_into_lines(&chars);
        assert_eq!(lines.len(), 2);
    }

    fn chars_from(text: &str) -> Vec<PositionedChar> {
        text.chars().enumerate().map(|(i, ch)| char_at(ch, i as f32, 0.0, 1.0, 10.0)).collect()
    }

    #[test]
    fn find_matches_finds_every_non_overlapping_and_overlapping_occurrence() {
        // "aa" in "banana" overlaps nowhere, but "ana" in "banana" does
        // (indices 1 and 3) — a naive scan should report both starts,
        // not skip past a match the way a replace-as-you-go scan would.
        let chars = chars_from("banana");
        assert_eq!(find_matches(&chars, &['a', 'n', 'a']), vec![(1, 3), (3, 5)]);
    }

    #[test]
    fn find_matches_is_case_sensitive_like_vims_default() {
        let chars = chars_from("Banana");
        assert_eq!(find_matches(&chars, &['b', 'a']), Vec::<(usize, usize)>::new());
        assert_eq!(find_matches(&chars, &['B', 'a']), vec![(0, 1)]);
    }

    #[test]
    fn find_matches_handles_empty_query_and_query_longer_than_text() {
        let chars = chars_from("hi");
        assert_eq!(find_matches(&chars, &[]), Vec::<(usize, usize)>::new());
        assert_eq!(find_matches(&chars, &['h', 'i', '!']), Vec::<(usize, usize)>::new());
    }
}
