//! Modal key-sequence dispatcher (Port order item 3: Modal input — the
//! mode-tracking and sequence-matching mechanism). Consumes `config.rs`'s
//! already-validated `Action` bindings. The motions/text-objects it
//! resolves to (word/WORD motions, `is`/`ip`, yank, ...) are mostly
//! `selection.rs`'s job and not implemented yet — see `main.rs`'s
//! `apply_action` for which `Action`s currently have a real effect versus
//! a "not implemented yet" status message.

use crate::config::{Action, Config, Mode};

pub struct Dispatcher {
    mode: Mode,
    pending: Vec<String>,
    /// Digits accumulated before a sequence starts, vim-style: `1`-`9`
    /// always start/continue a count, `0` only continues one already
    /// started (a bare `0` is the `line_start` motion instead). Applies
    /// to the next resolved action, then resets to 1.
    pending_count: String,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            pending: Vec::new(),
            pending_count: String::new(),
        }
    }

    /// True while a partial key sequence (e.g. the `y` of `y i s`) waits
    /// for more tokens.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Clears any pending partial sequence and returns to Normal mode.
    /// Used for the emergency-cancel path (e.g. switching documents),
    /// independent of whether Esc happens to be bound to anything.
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.pending.clear();
        self.pending_count.clear();
    }

    /// Feeds one key token (e.g. `"g"`, `"G"`, `"Esc"`, `"^"`). Returns the
    /// resolved `Action` and repeat count if a full sequence just
    /// completed. A partial (prefix) match keeps waiting for more
    /// tokens. A dead end (matches no binding and is a prefix of none)
    /// silently resets — this can never be ambiguous because `config.rs`
    /// rejects, at load time, any binding that is a strict prefix of
    /// another in the same mode.
    pub fn feed(&mut self, cfg: &Config, token: &str) -> Option<(Action, usize)> {
        if self.pending.is_empty() {
            if let Some(digit) = token.chars().next().filter(|c| token.len() == 1 && c.is_ascii_digit()) {
                if digit != '0' || !self.pending_count.is_empty() {
                    self.pending_count.push(digit);
                    return None;
                }
                // Bare "0" with no count yet: falls through as the
                // literal "0" token (line_start), not a count digit.
            }
        }

        let Some(bindings) = cfg.keys.get(&self.mode) else {
            self.pending.clear();
            self.pending_count.clear();
            return None;
        };

        self.pending.push(token.to_string());
        let seq = self.pending.join(" ");

        if let Some(&action) = bindings.get(&seq) {
            self.pending.clear();
            let count = self.pending_count.parse().unwrap_or(1).max(1);
            self.pending_count.clear();
            let action = self.apply_mode_transition(action);
            return Some((action, count));
        }

        let is_prefix_of_something = bindings.keys().any(|k| k.len() > seq.len() && k.starts_with(&seq));
        if !is_prefix_of_something {
            self.pending.clear();
            self.pending_count.clear();
        }
        None
    }

    /// Resolves toggles against the current mode into a concrete mode
    /// action, applies it, and returns the action to dispatch.
    /// Toggling the mode you are in drops back to Cursor (Normal, for
    /// `cursor_mode_toggle`).
    fn apply_mode_transition(&mut self, action: Action) -> Action {
        let action = match action {
            Action::CursorModeToggle if self.mode == Mode::Cursor => Action::NormalMode,
            Action::CursorModeToggle => Action::CursorMode,
            Action::VisualModeToggle if self.mode == Mode::Visual => Action::CursorMode,
            Action::VisualModeToggle => Action::VisualMode,
            Action::VisualLineModeToggle if self.mode == Mode::VisualLine => Action::CursorMode,
            Action::VisualLineModeToggle => Action::VisualLineMode,
            other => other,
        };
        self.mode = match action {
            Action::CursorMode => Mode::Cursor,
            Action::VisualMode => Mode::Visual,
            Action::VisualLineMode => Mode::VisualLine,
            Action::NormalMode => Mode::Normal,
            _ => self.mode,
        };
        action
    }
}

/// Whether a plain, unmodified `i` in the current mode should toggle dark
/// mode rather than start a text object (`is`/`ip` in Cursor/Visual
/// modes) — only Normal mode has no other use for a bare `i`.
pub fn dark_mode_shortcut(mode: Mode, modifiers: eframe::egui::Modifiers) -> bool {
    mode == Mode::Normal && !modifiers.any()
}

/// Converts an egui input event into a key token matching `config.rs`'s
/// `KeySeq` vocabulary (space-joined tokens like `"g g"`, `"y i s"`), or
/// `None` for events that aren't a single dispatchable keypress.
/// Printable characters come from `Event::Text` (not `Event::Key`) so
/// shift-adjusted symbols (`^`, `$`, `G`, `V`) and keyboard layout both
/// resolve correctly without the caller tracking shift state by hand.
pub fn token_for_event(event: &eframe::egui::Event) -> Option<String> {
    use eframe::egui::{Event, Key};
    match event {
        Event::Text(text) => {
            let ch = text.chars().next()?;
            if ch.is_control() {
                None
            } else {
                Some(ch.to_string())
            }
        }
        Event::Key { key: Key::Escape, pressed: true, .. } => Some("Esc".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_start_and_line_end_tokens_resolve_in_cursor_mode() {
        // Regression check: a user report ("$ and 0 don't seem to work")
        // turned out to be a rendering/feedback visibility problem, not a
        // dispatcher bug — this pins the dispatcher side down explicitly
        // so a real regression here would be caught by `cargo test`
        // rather than needing another live-window round trip to notice.
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        d.feed(&cfg, "c"); // enter Cursor mode
        assert_eq!(d.feed(&cfg, "0"), Some((Action::LineStart, 1)));
        assert_eq!(d.feed(&cfg, "$"), Some((Action::LineEnd, 1)));
        assert_eq!(d.feed(&cfg, "^"), Some((Action::LineFirstNonBlank, 1)));
    }

    #[test]
    fn single_token_sequence_resolves_immediately() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        assert_eq!(d.feed(&cfg, "j"), Some((Action::ScrollDown, 1)));
    }

    #[test]
    fn multi_token_sequence_waits_then_resolves() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        assert_eq!(d.feed(&cfg, "g"), None, "first token of 'g g' should not resolve yet");
        assert_eq!(d.feed(&cfg, "g"), Some((Action::DocumentStart, 1)));
    }

    #[test]
    fn dead_end_sequence_resets_and_does_not_poison_next_sequence() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        assert_eq!(d.feed(&cfg, "g"), None);
        assert_eq!(d.feed(&cfg, "x"), None, "'g x' is not a sequence, should reset");
        assert_eq!(d.feed(&cfg, "j"), Some((Action::ScrollDown, 1)), "fresh sequence after a dead end should still resolve");
    }

    #[test]
    fn mode_switch_changes_which_bindings_apply() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        assert_eq!(d.mode(), Mode::Normal);
        assert_eq!(d.feed(&cfg, "c"), Some((Action::CursorMode, 1)));
        assert_eq!(d.mode(), Mode::Cursor);
        // "h" is unbound in Normal's default bindings but is
        // character_left in Cursor mode.
        assert_eq!(d.feed(&cfg, "h"), Some((Action::CharacterLeft, 1)));
    }

    #[test]
    fn visual_toggles_resolve_by_current_mode() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        assert_eq!(d.feed(&cfg, "v"), Some((Action::VisualMode, 1)));
        assert_eq!(d.feed(&cfg, "V"), Some((Action::VisualLineMode, 1)));
        assert_eq!(d.feed(&cfg, "v"), Some((Action::VisualMode, 1)));
        assert_eq!(d.feed(&cfg, "v"), Some((Action::CursorMode, 1)));
        assert_eq!(d.mode(), Mode::Cursor);
        assert_eq!(d.feed(&cfg, "V"), Some((Action::VisualLineMode, 1)));
        assert_eq!(d.feed(&cfg, "V"), Some((Action::CursorMode, 1)));
    }

    #[test]
    fn escape_returns_to_normal_mode_from_cursor() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        d.feed(&cfg, "c");
        assert_eq!(d.mode(), Mode::Cursor);
        assert_eq!(d.feed(&cfg, "Esc"), Some((Action::NormalMode, 1)));
        assert_eq!(d.mode(), Mode::Normal);
    }

    #[test]
    fn three_token_sequence_resolves_after_all_three() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        d.feed(&cfg, "c"); // enter Cursor mode
        assert_eq!(d.feed(&cfg, "y"), None);
        assert_eq!(d.feed(&cfg, "i"), None);
        assert_eq!(d.feed(&cfg, "s"), Some((Action::CopySentence, 1)));
    }

    #[test]
    fn reset_clears_pending_and_mode() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        d.feed(&cfg, "c");
        d.feed(&cfg, "y"); // partial "y i s"
        d.reset();
        assert_eq!(d.mode(), Mode::Normal);
        assert_eq!(d.feed(&cfg, "j"), Some((Action::ScrollDown, 1)), "reset should drop the stale partial sequence");
    }

    #[test]
    fn digit_prefix_is_consumed_as_a_count() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        assert_eq!(d.feed(&cfg, "3"), None, "a count digit alone shouldn't resolve");
        assert_eq!(d.feed(&cfg, "j"), Some((Action::ScrollDown, 3)));
        // Multi-digit count.
        assert_eq!(d.feed(&cfg, "1"), None);
        assert_eq!(d.feed(&cfg, "2"), None);
        assert_eq!(d.feed(&cfg, "k"), Some((Action::ScrollUp, 12)));
    }

    #[test]
    fn bare_zero_is_line_start_not_a_count() {
        let cfg = Config::default();
        let mut d = Dispatcher::new();
        d.feed(&cfg, "c"); // enter Cursor mode
        assert_eq!(d.feed(&cfg, "0"), Some((Action::LineStart, 1)), "a leading 0 is the line_start motion, not a count digit");
        // But 0 after a nonzero digit does continue the count ("10").
        assert_eq!(d.feed(&cfg, "1"), None);
        assert_eq!(d.feed(&cfg, "0"), None);
        assert_eq!(d.feed(&cfg, "l"), Some((Action::CharacterRight, 10)));
    }

    #[test]
    fn dark_mode_shortcut_only_works_in_normal_mode() {
        for mode in [Mode::Cursor, Mode::Visual, Mode::VisualLine] {
            assert!(!dark_mode_shortcut(mode, eframe::egui::Modifiers::NONE));
        }
        assert!(dark_mode_shortcut(Mode::Normal, eframe::egui::Modifiers::NONE));
        assert!(!dark_mode_shortcut(Mode::Normal, eframe::egui::Modifiers::CTRL));
    }
}
