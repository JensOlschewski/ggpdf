//! Turns a config appearance color string into the `egui::Color32` the
//! renderer actually draws with. Kept out of `config.rs` on purpose:
//! `config.rs` only parses/validates plain data (no `egui` dependency),
//! while interpreting that data as a drawable color is a rendering
//! concern.

use eframe::egui::Color32;

/// Parses a `#rrggbb` or `#rrggbbaa` hex color from config. Falls back to
/// `fallback` on anything else — a config file is user-edited text, not a
/// trusted format, and a bad color string shouldn't crash the viewer.
pub fn parse_hex_color(s: &str, fallback: Color32) -> Color32 {
    let s = s.trim_start_matches('#');
    match s.len() {
        6 => match u32::from_str_radix(s, 16) {
            Ok(v) => Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
            Err(_) => fallback,
        },
        8 => match u32::from_str_radix(s, 16) {
            Ok(v) => Color32::from_rgba_unmultiplied((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8),
            Err(_) => fallback,
        },
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_and_rgba_hex_with_or_without_a_leading_hash() {
        assert_eq!(parse_hex_color("#ff0000", Color32::BLACK), Color32::from_rgb(255, 0, 0));
        assert_eq!(parse_hex_color("00ff00", Color32::BLACK), Color32::from_rgb(0, 255, 0));
        assert_eq!(parse_hex_color("#0000ff80", Color32::BLACK), Color32::from_rgba_unmultiplied(0, 0, 255, 0x80));
    }

    #[test]
    fn falls_back_on_anything_unparsable() {
        assert_eq!(parse_hex_color("not-a-color", Color32::RED), Color32::RED);
        assert_eq!(parse_hex_color("#abc", Color32::RED), Color32::RED);
    }
}
