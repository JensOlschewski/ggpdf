//! Reading-order character geometry (see PLAN.md Project structure:
//! "text.rs — reading-order lines, graphemes, geometry model").
//!
//! This is the plain-data layer between `document.rs` (raw PDFium
//! extraction, unrotated point space) and the UI (`main.rs` today,
//! `viewer.rs`/`selection.rs` later): no PDFium types, no egui types.
//! Grapheme clustering, line joins, and multi-column ordering (the rest
//! of this module's eventual job per PLAN.md) aren't implemented yet —
//! this first slice only carries over the rotation-mapping math already
//! validated in the spike (see PLAN.md "Biggest risk" gate).

use crate::document::{PageSize, RawChar};

/// An axis-aligned pixel-space rectangle, origin top-left — matches the
/// rendered page bitmap's coordinate system, not PDF point space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    fn from_corners(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect {
            left: x0.min(x1),
            right: x0.max(x1),
            top: y0.min(y1),
            bottom: y0.max(y1),
        }
    }
}

/// One character positioned in display pixel space.
#[derive(Debug, Clone, Copy)]
pub struct PositionedChar {
    pub ch: char,
    pub rect: Rect,
}

/// Maps every raw character on a page into display pixel space at
/// `pixels_per_point` scale, accounting for the page's intrinsic
/// `/Rotate` (raw char bounds from `document.rs` are in the *unrotated*
/// point space; the rendered bitmap is already rotated — see
/// `document.rs` module docs and PLAN.md's rotation finding).
pub fn position_chars(raw_chars: &[RawChar], size: PageSize, rotation_degrees: i32, pixels_per_point: f32) -> Vec<PositionedChar> {
    let rotated_w = size.width_pt;
    let rotated_h = size.height_pt;

    // Original (unrotated, /Rotate == 0) page dimensions, needed to map
    // raw_chars (reported in the unrotated page coordinate system) into
    // the rotated display space the renderer produces.
    let (orig_w, orig_h) = if rotation_degrees == 90 || rotation_degrees == 270 {
        (rotated_h, rotated_w)
    } else {
        (rotated_w, rotated_h)
    };

    let positioned = raw_chars
        .iter()
        .map(|raw| {
            let rect = rotate_rect_to_display(
                raw.left, raw.bottom, raw.right, raw.top, orig_w, orig_h, rotation_degrees,
                rotated_h, pixels_per_point,
            );
            PositionedChar { ch: raw.ch, rect }
        })
        .collect();
    insert_synthesized_spaces(positioned)
}

/// Inserts a synthetic `' '` character between two consecutive
/// characters that sit on the same visual line with a horizontal gap
/// wide enough to read as a word boundary, when neither is already
/// whitespace — a direct port of `visual.js`'s `collect()`, which does
/// the same for PDF.js's text-layer runs (previously flagged, not
/// fixed: a PDF whose content stream never emits an explicit space
/// glyph between two words would otherwise misjoin them here, since
/// PDFium's raw character stream carries only what the content stream
/// itself encodes). `GAP_RATIO` (15% of the shorter rect's height) is
/// `visual.js`'s own constant, carried over rather than re-derived.
fn insert_synthesized_spaces(chars: Vec<PositionedChar>) -> Vec<PositionedChar> {
    const GAP_RATIO: f32 = 0.15;
    let mut out: Vec<PositionedChar> = Vec::with_capacity(chars.len());
    for current in chars {
        if let Some(prev) = out.last().copied() {
            let same_line = current.rect.top < prev.rect.bottom && current.rect.bottom > prev.rect.top;
            let gap = current.rect.left - prev.rect.right;
            let threshold = (prev.rect.bottom - prev.rect.top).min(current.rect.bottom - current.rect.top) * GAP_RATIO;
            if same_line && gap > threshold && !prev.ch.is_whitespace() && !current.ch.is_whitespace() {
                out.push(PositionedChar {
                    ch: ' ',
                    rect: Rect {
                        left: prev.rect.right,
                        right: current.rect.left,
                        top: prev.rect.top.min(current.rect.top),
                        bottom: prev.rect.bottom.max(current.rect.bottom),
                    },
                });
            }
        }
        out.push(current);
    }
    out
}

/// Maps a char's unrotated-space bounding box into rotated display pixel
/// space: rotates all four corners through the clockwise `/Rotate`
/// transform, takes the bounding box, then flips Y for pixel space
/// (origin top-left) and scales to pixels.
#[allow(clippy::too_many_arguments)]
fn rotate_rect_to_display(
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
    orig_w: f32,
    orig_h: f32,
    rotation_degrees: i32,
    display_h: f32,
    pixels_per_point: f32,
) -> Rect {
    let corners = [(left, bottom), (left, top), (right, bottom), (right, top)];
    let mapped: Vec<(f32, f32)> = corners
        .iter()
        .map(|&(x, y)| match rotation_degrees {
            90 => (y, orig_w - x),
            180 => (orig_w - x, orig_h - y),
            270 => (orig_h - y, x),
            _ => (x, y),
        })
        .collect();

    let min_x = mapped.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let max_x = mapped.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let min_y = mapped.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = mapped.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);

    Rect::from_corners(
        min_x * pixels_per_point,
        (display_h - max_y) * pixels_per_point,
        max_x * pixels_per_point,
        (display_h - min_y) * pixels_per_point,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: f32, h: f32) -> PageSize {
        PageSize { width_pt: w, height_pt: h }
    }

    #[test]
    fn unrotated_char_maps_with_simple_y_flip() {
        let raw = RawChar { ch: 'A', left: 10.0, bottom: 20.0, right: 15.0, top: 30.0 };
        let positioned = position_chars(&[raw], size(100.0, 100.0), 0, 1.0);
        assert_eq!(positioned.len(), 1);
        let r = positioned[0].rect;
        assert_eq!((r.left, r.right), (10.0, 15.0));
        assert_eq!((r.top, r.bottom), (70.0, 80.0));
    }

    #[test]
    fn scale_multiplies_all_coordinates() {
        let raw = RawChar { ch: 'A', left: 10.0, bottom: 20.0, right: 15.0, top: 30.0 };
        let positioned = position_chars(&[raw], size(100.0, 100.0), 0, 2.0);
        let r = positioned[0].rect;
        assert_eq!((r.left, r.right), (20.0, 30.0));
        assert_eq!((r.top, r.bottom), (140.0, 160.0));
    }

    #[test]
    fn rotation_90_moves_top_left_glyph_to_top_right_of_display() {
        // A tiny glyph at the unrotated page's top-left corner should end
        // up at the top-right of the rotated display, matching the
        // manual derivation in PLAN.md's rotation finding (also verified
        // visually against rotated.pdf via the --dump PNGs).
        let orig_w = 200.0;
        let orig_h = 300.0;
        // after 90 deg rotation, displayed size swaps: width=orig_h, height=orig_w
        let raw = RawChar { ch: 'X', left: 0.0, bottom: orig_h - 5.0, right: 5.0, top: orig_h };
        let positioned = position_chars(&[raw], size(orig_h, orig_w), 90, 1.0);
        let r = positioned[0].rect;
        // Displayed width is orig_h; glyph should hug the right edge, top.
        assert!(r.right > orig_h - 1.0, "expected near right edge, got {r:?}");
        assert!(r.top < 1.0, "expected near top edge, got {r:?}");
    }

    #[test]
    fn no_chars_gives_no_positions() {
        assert!(position_chars(&[], size(100.0, 100.0), 0, 1.0).is_empty());
    }

    /// Two words on the same line, 10pt apart (char height 10pt — a gap
    /// well past the 15%-of-height threshold), with no explicit space
    /// glyph in the content stream — the exact case `visual.js`'s
    /// `collect()` handles for PDF.js's text-layer runs and
    /// `document.rs`/`text.rs` didn't (flagged gap, now closed).
    #[test]
    fn synthesizes_a_space_across_a_wide_same_line_gap_with_no_explicit_space() {
        let one = RawChar { ch: 'e', left: 0.0, bottom: 0.0, right: 5.0, top: 10.0 };
        let two = RawChar { ch: 't', left: 15.0, bottom: 0.0, right: 20.0, top: 10.0 };
        let positioned = position_chars(&[one, two], size(100.0, 100.0), 0, 1.0);
        let chars: String = positioned.iter().map(|p| p.ch).collect();
        assert_eq!(chars, "e t");
    }

    /// A narrow gap (well under the threshold) between two glyphs is
    /// ordinary letter spacing, not a word boundary — no space inserted.
    #[test]
    fn does_not_synthesize_a_space_across_a_narrow_same_line_gap() {
        let one = RawChar { ch: 'e', left: 0.0, bottom: 0.0, right: 5.0, top: 10.0 };
        let two = RawChar { ch: 't', left: 5.5, bottom: 0.0, right: 10.5, top: 10.0 };
        let positioned = position_chars(&[one, two], size(100.0, 100.0), 0, 1.0);
        let chars: String = positioned.iter().map(|p| p.ch).collect();
        assert_eq!(chars, "et");
    }

    /// A wide horizontal gap between two glyphs on *different* lines
    /// (e.g. the right edge of one line's last word and the left edge
    /// of the next line's first word, which can be arbitrarily far
    /// apart) must not be mistaken for a same-line word gap.
    #[test]
    fn does_not_synthesize_a_space_across_a_gap_on_different_lines() {
        let one = RawChar { ch: 'e', left: 0.0, bottom: 0.0, right: 5.0, top: 10.0 };
        let two = RawChar { ch: 't', left: 50.0, bottom: 20.0, right: 55.0, top: 30.0 };
        let positioned = position_chars(&[one, two], size(100.0, 100.0), 0, 1.0);
        let chars: String = positioned.iter().map(|p| p.ch).collect();
        assert_eq!(chars, "et");
    }

    /// A gap already adjacent to a real space character (either side)
    /// must not get a second, redundant space inserted.
    #[test]
    fn does_not_double_up_a_space_already_present() {
        let one = RawChar { ch: 'e', left: 0.0, bottom: 0.0, right: 5.0, top: 10.0 };
        let space = RawChar { ch: ' ', left: 5.0, bottom: 0.0, right: 15.0, top: 10.0 };
        let two = RawChar { ch: 't', left: 15.0, bottom: 0.0, right: 20.0, top: 10.0 };
        let positioned = position_chars(&[one, space, two], size(100.0, 100.0), 0, 1.0);
        let chars: String = positioned.iter().map(|p| p.ch).collect();
        assert_eq!(chars, "e t");
    }
}
