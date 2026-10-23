//! Config loading and validation (see PLAN.md "Config file — one file, keys
//! + appearance").
//!
//! The app must always come up runnable: if the user's config file is
//! missing, unparsable, or invalid, [`load_or_default`] falls back to
//! [`Config::default`] (which always binds `q`/`Esc` to `close`) rather
//! than failing to start.

use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

/// Known input actions. Config key sequences must map to one of these.
///
/// This list is intentionally not exhaustive yet — it covers the actions
/// named in PLAN.md's example config and "Port order" section. Extend it
/// as `input.rs` grows real motions; an unrecognized action name in the
/// config is a load error, not a silent no-op, so this list and the
/// input dispatcher must stay in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ScrollDown,
    ScrollUp,
    DocumentStart,
    DocumentEnd,
    CursorMode,
    VisualMode,
    VisualLineMode,
    NormalMode,
    Close,
    Cancel,
    CharacterLeft,
    CharacterRight,
    WordForward,
    WordBackward,
    WordEnd,
    BigWordForward,
    BigWordBackward,
    BigWordEnd,
    LineStart,
    LineFirstNonBlank,
    LineEnd,
    LineDown,
    LineUp,
    CopySentence,
    CopyParagraph,
    SelectSentence,
    SelectParagraph,
    Yank,
    SearchNext,
    SearchPrev,
}

impl Action {
    const ALL: &'static [(&'static str, Action)] = &[
        ("scroll_down", Action::ScrollDown),
        ("scroll_up", Action::ScrollUp),
        ("document_start", Action::DocumentStart),
        ("document_end", Action::DocumentEnd),
        ("cursor_mode", Action::CursorMode),
        ("visual_mode", Action::VisualMode),
        ("visual_line_mode", Action::VisualLineMode),
        ("normal_mode", Action::NormalMode),
        ("close", Action::Close),
        ("cancel", Action::Cancel),
        ("character_left", Action::CharacterLeft),
        ("character_right", Action::CharacterRight),
        ("word_forward", Action::WordForward),
        ("word_backward", Action::WordBackward),
        ("word_end", Action::WordEnd),
        ("big_word_forward", Action::BigWordForward),
        ("big_word_backward", Action::BigWordBackward),
        ("big_word_end", Action::BigWordEnd),
        ("line_start", Action::LineStart),
        ("line_first_non_blank", Action::LineFirstNonBlank),
        ("line_end", Action::LineEnd),
        ("line_down", Action::LineDown),
        ("line_up", Action::LineUp),
        ("copy_sentence", Action::CopySentence),
        ("copy_paragraph", Action::CopyParagraph),
        ("select_sentence", Action::SelectSentence),
        ("select_paragraph", Action::SelectParagraph),
        ("yank", Action::Yank),
        ("search_next", Action::SearchNext),
        ("search_prev", Action::SearchPrev),
    ];

    fn from_name(name: &str) -> Option<Action> {
        Self::ALL.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
    }
}

/// A key sequence split into tokens, e.g. `"y i s"` -> `["y", "i", "s"]`.
/// A bare single key like `"j"` is a one-token sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct KeySeq(Vec<String>);

impl KeySeq {
    fn parse(raw: &str) -> KeySeq {
        KeySeq(raw.split_whitespace().map(str::to_string).collect())
    }

    fn is_prefix_of(&self, other: &KeySeq) -> bool {
        self.0.len() < other.0.len() && other.0.starts_with(&self.0)
    }
}

impl fmt::Display for KeySeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.join(" "))
    }
}

#[derive(Debug, Clone)]
pub struct Appearance {
    pub canvas_light: String,
    pub canvas_dark: String,
    pub page_gap: u32,
    pub gutter_width: u32,
    pub selection: String,
    pub cursor: String,
    /// Fill for every `/`/`?` search match except the focused one —
    /// matches a typical "Search" highlight group (a soft yellow),
    /// distinct from `selection`/`cursor` so all three can be visible
    /// at once without being confused for each other.
    pub search: String,
    /// Fill (and outline) for the one match `n`/`N` last jumped to —
    /// a typical "CurSearch"/"IncSearch" highlight group: more opaque
    /// and a different hue than `search` so the current match stands
    /// out among the others.
    pub search_current: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            canvas_light: "#ededed".to_string(),
            canvas_dark: "#282828".to_string(),
            page_gap: 12,
            gutter_width: 48,
            selection: "#4285f459".to_string(),
            cursor: "#787878a6".to_string(),
            search: "#ffd70059".to_string(),
            search_current: "#ff8c00a6".to_string(),
        }
    }
}

/// Input mode a key sequence is bound in. Mirrors "Port order" item 3
/// (normal / cursor / visual / visual-line modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Normal,
    Cursor,
    Visual,
    VisualLine,
}

impl Mode {
    fn from_toml_key(key: &str) -> Option<Mode> {
        match key {
            "normal" => Some(Mode::Normal),
            "cursor" => Some(Mode::Cursor),
            "visual" => Some(Mode::Visual),
            "visual_line" => Some(Mode::VisualLine),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub appearance: Appearance,
    /// mode -> key sequence -> action
    pub keys: HashMap<Mode, HashMap<String, Action>>,
}

impl Default for Config {
    fn default() -> Self {
        let mut normal = HashMap::new();
        normal.insert("j".to_string(), Action::ScrollDown);
        normal.insert("k".to_string(), Action::ScrollUp);
        normal.insert("g g".to_string(), Action::DocumentStart);
        normal.insert("G".to_string(), Action::DocumentEnd);
        normal.insert("c".to_string(), Action::CursorMode);
        normal.insert("v".to_string(), Action::VisualMode);
        normal.insert("V".to_string(), Action::VisualLineMode);
        normal.insert("q".to_string(), Action::Close);
        normal.insert("Esc".to_string(), Action::Close);
        // `/`/`?` themselves aren't config-driven bindings — typing a
        // search query is free text, not a vim-style sequence lookup —
        // but `n`/`N` repeating the last one are ordinary motions, same
        // as every other key here. See `main.rs`'s direct `/`/`?`
        // handling in its event loop.
        normal.insert("n".to_string(), Action::SearchNext);
        normal.insert("N".to_string(), Action::SearchPrev);

        let mut cursor = HashMap::new();
        cursor.insert("h".to_string(), Action::CharacterLeft);
        cursor.insert("l".to_string(), Action::CharacterRight);
        cursor.insert("w".to_string(), Action::WordForward);
        cursor.insert("b".to_string(), Action::WordBackward);
        cursor.insert("e".to_string(), Action::WordEnd);
        cursor.insert("W".to_string(), Action::BigWordForward);
        cursor.insert("B".to_string(), Action::BigWordBackward);
        cursor.insert("E".to_string(), Action::BigWordEnd);
        cursor.insert("0".to_string(), Action::LineStart);
        cursor.insert("^".to_string(), Action::LineFirstNonBlank);
        cursor.insert("$".to_string(), Action::LineEnd);
        // Cursor mode's j/k move the text cursor a visual line, distinct
        // from Normal mode's j/k which scroll the document.
        cursor.insert("j".to_string(), Action::LineDown);
        cursor.insert("k".to_string(), Action::LineUp);
        cursor.insert("y i s".to_string(), Action::CopySentence);
        cursor.insert("y i p".to_string(), Action::CopyParagraph);
        // v/V start marking from the cursor's current position, matching
        // visual.js (where v/V are recognized in any active state, not
        // just from a separate "Normal" state) — without these, marking
        // was only reachable by first returning all the way to Normal
        // mode, losing the cursor's navigated-to position in the process.
        cursor.insert("v".to_string(), Action::VisualMode);
        cursor.insert("V".to_string(), Action::VisualLineMode);
        cursor.insert("n".to_string(), Action::SearchNext);
        cursor.insert("N".to_string(), Action::SearchPrev);
        cursor.insert("Esc".to_string(), Action::NormalMode);

        // Visual/VisualLine inherit every Cursor-mode motion (h/j/k/l/
        // word motions/line motions) so the mark can actually be
        // extended — the old defaults only bound Esc and y here, so
        // every motion key silently dead-ended once marking started.
        // Inheritance also includes n/N: same-page jumps extend the mark.
        let mut visual = cursor.clone();
        visual.insert("v".to_string(), Action::CursorMode); // toggle off: deselect, stay in Cursor mode
        visual.insert("V".to_string(), Action::VisualLineMode); // switch to linewise
        visual.insert("y".to_string(), Action::Yank);
        // Plain is/ip (no leading y) replace the mark with the text
        // object without yanking — matches visual.js, where "i" only
        // starts this sequence while already marking (`this.marking`);
        // "y i s"/"y i p" above cover the yank-directly case, reachable
        // from Cursor mode too since not gated on marking.
        visual.insert("i s".to_string(), Action::SelectSentence);
        visual.insert("i p".to_string(), Action::SelectParagraph);

        let mut visual_line = cursor.clone();
        visual_line.insert("V".to_string(), Action::CursorMode); // toggle off
        visual_line.insert("v".to_string(), Action::VisualMode); // switch to charwise
        visual_line.insert("y".to_string(), Action::Yank);
        visual_line.insert("i s".to_string(), Action::SelectSentence);
        visual_line.insert("i p".to_string(), Action::SelectParagraph);

        let mut keys = HashMap::new();
        keys.insert(Mode::Normal, normal);
        keys.insert(Mode::Cursor, cursor);
        keys.insert(Mode::Visual, visual);
        keys.insert(Mode::VisualLine, visual_line);

        Self {
            appearance: Appearance::default(),
            keys,
        }
    }
}

impl Config {
    /// Emergency close/cancel must stay reachable even in a from-scratch
    /// config; this is true of [`Config::default`] by construction, but
    /// also double-checked by [`validate`] on any user-supplied config.
    fn has_emergency_close(&self) -> bool {
        self.keys.values().any(|mode_keys| {
            mode_keys
                .iter()
                .any(|(_, action)| matches!(action, Action::Close | Action::Cancel))
        })
    }
}

/// Raw TOML shape, deserialized then converted+validated into [`Config`].
#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    appearance: RawAppearance,
    #[serde(default)]
    keys: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct RawAppearance {
    canvas_light: Option<String>,
    canvas_dark: Option<String>,
    page_gap: Option<u32>,
    gutter_width: Option<u32>,
    selection: Option<String>,
    cursor: Option<String>,
    search: Option<String>,
    search_current: Option<String>,
}

impl Default for RawAppearance {
    fn default() -> Self {
        Self {
            canvas_light: None,
            canvas_dark: None,
            page_gap: None,
            gutter_width: None,
            selection: None,
            cursor: None,
            search: None,
            search_current: None,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    UnknownMode { mode: String },
    UnknownAction { mode: String, sequence: String, action: String },
    AmbiguousPrefix { mode: String, shorter: String, longer: String },
    NoEmergencyClose,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "could not read config file: {e}"),
            ConfigError::Parse(e) => write!(f, "could not parse config TOML: {e}"),
            ConfigError::UnknownMode { mode } => {
                write!(f, "unknown key mode [keys.{mode}] (expected one of: normal, cursor, visual, visual_line)")
            }
            ConfigError::UnknownAction { mode, sequence, action } => write!(
                f,
                "[keys.{mode}] \"{sequence}\" = \"{action}\": unknown action \"{action}\""
            ),
            ConfigError::AmbiguousPrefix { mode, shorter, longer } => write!(
                f,
                "[keys.{mode}]: \"{shorter}\" is a prefix of \"{longer}\" — ambiguous key sequence"
            ),
            ConfigError::NoEmergencyClose => write!(
                f,
                "config binds no key to close/cancel in any mode — refusing to load a config with no way to quit"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

fn convert(raw: RawConfig) -> Result<Config, ConfigError> {
    let defaults = Appearance::default();
    let appearance = Appearance {
        canvas_light: raw.appearance.canvas_light.unwrap_or(defaults.canvas_light),
        canvas_dark: raw.appearance.canvas_dark.unwrap_or(defaults.canvas_dark),
        page_gap: raw.appearance.page_gap.unwrap_or(defaults.page_gap),
        gutter_width: raw.appearance.gutter_width.unwrap_or(defaults.gutter_width),
        selection: raw.appearance.selection.unwrap_or(defaults.selection),
        cursor: raw.appearance.cursor.unwrap_or(defaults.cursor),
        search: raw.appearance.search.unwrap_or(defaults.search),
        search_current: raw.appearance.search_current.unwrap_or(defaults.search_current),
    };

    let mut keys: HashMap<Mode, HashMap<String, Action>> = HashMap::new();

    for (mode_name, bindings) in &raw.keys {
        let mode = Mode::from_toml_key(mode_name).ok_or_else(|| ConfigError::UnknownMode {
            mode: mode_name.clone(),
        })?;

        let mut mode_keys = HashMap::new();
        let mut seqs: Vec<KeySeq> = Vec::new();

        for (seq_str, action_name) in bindings {
            let action = Action::from_name(action_name).ok_or_else(|| ConfigError::UnknownAction {
                mode: mode_name.clone(),
                sequence: seq_str.clone(),
                action: action_name.clone(),
            })?;

            let seq = KeySeq::parse(seq_str);
            for existing in &seqs {
                if existing.is_prefix_of(&seq) {
                    return Err(ConfigError::AmbiguousPrefix {
                        mode: mode_name.clone(),
                        shorter: existing.to_string(),
                        longer: seq.to_string(),
                    });
                }
                if seq.is_prefix_of(existing) {
                    return Err(ConfigError::AmbiguousPrefix {
                        mode: mode_name.clone(),
                        shorter: seq.to_string(),
                        longer: existing.to_string(),
                    });
                }
            }
            seqs.push(seq);

            mode_keys.insert(seq_str.clone(), action);
        }

        keys.insert(mode, mode_keys);
    }

    // Any mode the user didn't touch keeps the built-in bindings for that
    // mode, so a config that only overrides [keys.normal] doesn't lose
    // cursor/visual motions entirely.
    let defaults = Config::default();
    for (mode, default_keys) in defaults.keys {
        keys.entry(mode).or_insert(default_keys);
    }

    let config = Config { appearance, keys };

    if !config.has_emergency_close() {
        return Err(ConfigError::NoEmergencyClose);
    }

    Ok(config)
}

fn xdg_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("ggpdf").join("config.toml"));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/ggpdf/config.toml"))
}

/// Candidate config paths, checked in order — the first one that exists
/// on disk wins. `~/.config/ggpdf/config.toml` (or `$XDG_CONFIG_HOME`) is
/// checked first on every platform, including macOS/Windows, so a user
/// who already keeps dotfiles there doesn't need a second location; the
/// platform-native path (PLAN.md's Config file section) is the fallback.
fn config_path_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(xdg) = xdg_config_path() {
        candidates.push(xdg);
    }

    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join("Library/Application Support/ggpdf/config.toml"));
    }

    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("ggpdf").join("config.toml"));
    }

    candidates
}

fn load_from_path(path: &std::path::Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
    let raw: RawConfig = toml::from_str(&text).map_err(ConfigError::Parse)?;
    convert(raw)
}

/// Loads the user's config if present and valid; otherwise falls back to
/// [`Config::default`] and reports why on stderr, naming the config path
/// and the offending key per PLAN.md's requirement. Never fails: the
/// returned [`Config`] is always runnable with `q`/`Esc` bound to close.
pub fn load_or_default() -> Config {
    let Some(path) = config_path_candidates().into_iter().find(|p| p.exists()) else {
        return Config::default();
    };

    match load_from_path(&path) {
        Ok(config) => {
            eprintln!("ggpdf: loaded config from {}", path.display());
            config
        }
        Err(e) => {
            eprintln!("ggpdf: error loading config {}: {e}", path.display());
            eprintln!("ggpdf: falling back to built-in defaults");
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_modes_inherit_cursor_motions() {
        // Regression check: the original defaults only bound Esc and y
        // in Visual/VisualLine, so every motion key dead-ended once
        // marking started and the mark could never be extended (found
        // live: "visual mode no visual indicator can be seen").
        let config = Config::default();
        for mode in [Mode::Visual, Mode::VisualLine] {
            let bindings = &config.keys[&mode];
            assert_eq!(bindings.get("l"), Some(&Action::CharacterRight), "{mode:?} should have character motions");
            assert_eq!(bindings.get("w"), Some(&Action::WordForward), "{mode:?} should have word motions");
            assert_eq!(bindings.get("$"), Some(&Action::LineEnd), "{mode:?} should have line motions");
        }
    }

    #[test]
    fn cursor_mode_can_enter_visual_modes() {
        // Regression check: v/V were only bound in Normal mode, so
        // starting to mark from an already-navigated cursor position was
        // impossible without first returning all the way to Normal.
        let config = Config::default();
        let cursor = &config.keys[&Mode::Cursor];
        assert_eq!(cursor.get("v"), Some(&Action::VisualMode));
        assert_eq!(cursor.get("V"), Some(&Action::VisualLineMode));
    }

    #[test]
    fn search_next_and_prev_are_bound_in_normal_and_cursor() {
        let config = Config::default();
        for mode in [Mode::Normal, Mode::Cursor] {
            let bindings = &config.keys[&mode];
            assert_eq!(bindings.get("n"), Some(&Action::SearchNext), "{mode:?}");
            assert_eq!(bindings.get("N"), Some(&Action::SearchPrev), "{mode:?}");
        }
    }

    #[test]
    fn default_config_has_emergency_close() {
        assert!(Config::default().has_emergency_close());
    }

    #[test]
    fn empty_config_loads_as_defaults() {
        let raw: RawConfig = toml::from_str("").unwrap();
        let config = convert(raw).unwrap();
        assert!(config.has_emergency_close());
        assert_eq!(config.appearance.page_gap, 12);
    }

    #[test]
    fn overrides_appearance_and_keeps_other_defaults() {
        let raw: RawConfig = toml::from_str(
            r#"
            [appearance]
            page_gap = 20
            "#,
        )
        .unwrap();
        let config = convert(raw).unwrap();
        assert_eq!(config.appearance.page_gap, 20);
        assert_eq!(config.appearance.gutter_width, 48);
    }

    #[test]
    fn unknown_action_is_rejected() {
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.normal]
            "x" = "not_a_real_action"
            "#,
        )
        .unwrap();
        let err = convert(raw).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownAction { .. }));
    }

    #[test]
    fn unknown_mode_is_rejected() {
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.bogus]
            "x" = "close"
            "#,
        )
        .unwrap();
        let err = convert(raw).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownMode { .. }));
    }

    #[test]
    fn ambiguous_prefix_is_rejected() {
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.normal]
            "g" = "document_start"
            "g g" = "document_start"
            "#,
        )
        .unwrap();
        let err = convert(raw).unwrap_err();
        assert!(matches!(err, ConfigError::AmbiguousPrefix { .. }));
    }

    #[test]
    fn non_prefix_sequences_sharing_a_token_are_fine() {
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.cursor]
            "y i s" = "copy_sentence"
            "y i p" = "copy_paragraph"
            "#,
        )
        .unwrap();
        let config = convert(raw).unwrap();
        assert_eq!(
            config.keys[&Mode::Cursor]["y i s"],
            Action::CopySentence
        );
    }

    #[test]
    fn untouched_modes_keep_default_close_binding() {
        // Only overrides [keys.visual]; [keys.normal] (which binds q/Esc
        // to close) is left at its built-in default, so this loads fine.
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.visual]
            "x" = "yank"
            "#,
        )
        .unwrap();
        let config = convert(raw).unwrap();
        assert!(config.has_emergency_close());
    }
}
