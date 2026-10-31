//! Config loading and validation (see PLAN.md "Config file — one file, keys
//! + appearance").
//!
//! The app must always come up runnable: if the user's config file is
//! missing, unparsable, or invalid, [`load_or_default`] falls back to
//! [`Config::default`] (which always binds `q`/`Esc` to `close`) rather
//! than failing to start.

use eframe::egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

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
    /// Config-facing toggles. `Dispatcher::feed` resolves each to one of
    /// the concrete mode actions below, so callers never see these.
    CursorModeToggle,
    VisualModeToggle,
    VisualLineModeToggle,
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
        ("cursor_mode_toggle", Action::CursorModeToggle),
        ("visual_mode_toggle", Action::VisualModeToggle),
        ("visual_line_mode_toggle", Action::VisualLineModeToggle),
        ("document_mode", Action::NormalMode),
        // Old names, kept as aliases.
        ("cursor_mode", Action::CursorModeToggle),
        ("visual_mode", Action::VisualModeToggle),
        ("visual_line_mode", Action::VisualLineModeToggle),
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
        ("yank_sentence", Action::CopySentence),
        ("yank_paragraph", Action::CopyParagraph),
        ("copy_sentence", Action::CopySentence),
        ("copy_paragraph", Action::CopyParagraph),
        ("select_sentence", Action::SelectSentence),
        ("select_paragraph", Action::SelectParagraph),
        ("yank", Action::Yank),
        ("search_next", Action::SearchNext),
        ("search_prev", Action::SearchPrev),
    ];

    /// Canonical (non-alias) names for error messages: the first name
    /// listed for each action.
    fn valid_names() -> String {
        let mut seen = Vec::new();
        let mut names = Vec::new();
        for (name, action) in Self::ALL {
            if !seen.contains(action) {
                seen.push(*action);
                names.push(*name);
            }
        }
        names.join(", ")
    }

    fn name(self) -> &'static str {
        Self::ALL.iter().find(|(_, a)| *a == self).map_or("?", |(n, _)| n)
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub canvas_light: String,
    pub canvas_dark: String,
    pub page_gap: u32,
    pub gutter_width: u32,
    pub selection_light: String,
    pub selection_dark: String,
    pub cursor_light: String,
    pub cursor_dark: String,
    /// Fill for every `/`/`?` search match except the focused one —
    /// matches a typical "Search" highlight group (a soft yellow),
    /// distinct from `selection`/`cursor` so all three can be visible
    /// at once without being confused for each other.
    pub search_light: String,
    pub search_dark: String,
    /// Fill (and outline) for the one match `n`/`N` last jumped to —
    /// a typical "CurSearch"/"IncSearch" highlight group: more opaque
    /// and a different hue than `search` so the current match stands
    /// out among the others.
    pub search_current_light: String,
    pub search_current_dark: String,
    /// Brief flash over the yanked text.
    pub yank_highlight_light: String,
    pub yank_highlight_dark: String,
    /// Old single-color keys: when set, `convert` copies them into both
    /// the `_light` and `_dark` variants. Never serialized.
    #[serde(skip_serializing)]
    selection: Option<String>,
    #[serde(skip_serializing)]
    cursor: Option<String>,
    #[serde(skip_serializing)]
    search: Option<String>,
    #[serde(skip_serializing)]
    search_current: Option<String>,
    #[serde(skip_serializing)]
    yank_highlight: Option<String>,
    /// How long the yank flash stays visible, in milliseconds.
    pub yank_highlight_ms: u32,
    /// Gutter and tab text sizes derive from this; the gutter is one
    /// point smaller.
    pub font_size: f32,
    /// Path to a TTF/OTF file used before the built-in monospace font.
    /// Empty = built-in only.
    pub font_file: String,
    pub gutter_rel_light: String,
    pub gutter_rel_dark: String,
    pub gutter_current_light: String,
    pub gutter_current_dark: String,
    pub tab_bar_light: String,
    pub tab_bar_dark: String,
    pub tab_active_light: String,
    pub tab_active_dark: String,
    pub tab_inactive_light: String,
    pub tab_inactive_dark: String,
    pub finder_bg_light: String,
    pub finder_bg_dark: String,
    pub finder_fg_light: String,
    pub finder_fg_dark: String,
    pub finder_selected_bg_light: String,
    pub finder_selected_bg_dark: String,
    /// Color of the characters that matched the finder query.
    pub finder_match_light: String,
    pub finder_match_dark: String,
    pub finder_border_light: String,
    pub finder_border_dark: String,
    pub finder_width: u32,
    /// Visible result rows.
    pub finder_rows: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            canvas_light: "#ededed".to_string(),
            canvas_dark: "#282828".to_string(),
            page_gap: 12,
            gutter_width: 48,
            selection_light: "#2563eb33".to_string(),
            selection_dark: "#60a5fa40".to_string(),
            cursor_light: "#2563eb99".to_string(),
            cursor_dark: "#60a5fa99".to_string(),
            search_light: "#eab30840".to_string(),
            search_dark: "#fbbf244d".to_string(),
            search_current_light: "#ff8c00a6".to_string(),
            search_current_dark: "#ff8c00a6".to_string(),
            yank_highlight_light: "#16a34a4d".to_string(),
            yank_highlight_dark: "#22c55e59".to_string(),
            selection: None,
            cursor: None,
            search: None,
            search_current: None,
            yank_highlight: None,
            yank_highlight_ms: 150,
            font_size: 14.0,
            font_file: String::new(),
            gutter_rel_light: "#c8c8c8".to_string(),
            gutter_rel_dark: "#2a2a2a".to_string(),
            gutter_current_light: "#dc2626".to_string(),
            gutter_current_dark: "#ef4444".to_string(),
            tab_bar_light: "#ededed".to_string(),
            tab_bar_dark: "#282828".to_string(),
            tab_active_light: "#dc2626".to_string(),
            tab_active_dark: "#ef4444".to_string(),
            tab_inactive_light: "#c8c8c8".to_string(),
            tab_inactive_dark: "#2a2a2a".to_string(),
            finder_bg_light: "#ffffff".to_string(),
            finder_bg_dark: "#333333".to_string(),
            finder_fg_light: "#333333".to_string(),
            finder_fg_dark: "#eeeeee".to_string(),
            finder_selected_bg_light: "#2563eb33".to_string(),
            finder_selected_bg_dark: "#60a5fa40".to_string(),
            finder_match_light: "#dc2626".to_string(),
            finder_match_dark: "#ef4444".to_string(),
            finder_border_light: "#c8c8c8".to_string(),
            finder_border_dark: "#555555".to_string(),
            finder_width: 640,
            finder_rows: 12,
        }
    }
}

/// Where the `Ctrl+F` finder looks for PDFs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Finder {
    /// Root directories to scan; a leading `~` is the home directory.
    pub dirs: Vec<String>,
    pub max_depth: usize,
    /// Key that opens the finder, e.g. `"ctrl+f"`.
    pub open_key: String,
    /// Key that opens the native file dialog, e.g. `"ctrl+shift+f"`.
    pub dialog_key: String,
    /// Key that closes the finder, e.g. `"ctrl+c"` or `"esc"`.
    pub close_key: String,
}

impl Default for Finder {
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
            max_depth: 4,
            open_key: "ctrl+f".to_string(),
            dialog_key: "ctrl+shift+f".to_string(),
            close_key: "ctrl+c".to_string(),
        }
    }
}

/// A key plus the exact modifiers that must be held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl Chord {
    /// Parses `"esc"`, `"f"`, `"ctrl+c"`, `"alt+shift+f"`, `"cmd+ctrl+j"`: any
    /// order and combination of `ctrl`, `alt`, `shift` and `cmd` (the Mac
    /// Command key), then any egui key name. `control`, `option`, `command`
    /// and `super` are accepted aliases.
    fn parse(raw: &str) -> Option<Chord> {
        let raw = raw.trim().to_ascii_lowercase();
        let mut modifiers = Modifiers::NONE;
        let mut name = raw.as_str();
        while let Some((prefix, rest)) = name.split_once('+') {
            match prefix {
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" | "option" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "cmd" | "command" | "super" => modifiers.mac_cmd = true,
                _ => return None,
            }
            name = rest;
        }
        let key = if name == "esc" { Key::Escape } else { Key::from_name(name)? };
        Some(Chord { key, modifiers })
    }

    pub fn matches(&self, key: Key, modifiers: Modifiers) -> bool {
        let (held, wanted) = (modifiers, self.modifiers);
        key == self.key && held.ctrl == wanted.ctrl && held.alt == wanted.alt && held.shift == wanted.shift && held.mac_cmd == wanted.mac_cmd
    }
}

impl Finder {
    pub fn open_chord(&self) -> Chord {
        Chord::parse(&self.open_key).expect("open_key validated at load")
    }

    pub fn dialog_chord(&self) -> Chord {
        Chord::parse(&self.dialog_key).expect("dialog_key validated at load")
    }

    pub fn close_chord(&self) -> Chord {
        Chord::parse(&self.close_key).expect("close_key validated at load")
    }

    /// `dirs` with `~` expanded. Missing directories are kept; the scan skips them.
    pub fn roots(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
        self.dirs.iter().map(|dir| expand_home(dir, home.as_deref())).collect()
    }
}

fn expand_home(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(path),
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

#[derive(Debug, Clone)]
pub struct Config {
    pub appearance: Appearance,
    pub finder: Finder,
    tables: KeyTables,
    /// mode -> key sequence -> action
    pub keys: HashMap<Mode, HashMap<String, Action>>,
}

/// The four `[keys.*]` tables as written in the config file.
/// `document` is Normal mode. Cursor mode is `motion` + `cursor`;
/// Visual and VisualLine are `motion` + `selection`. A mode's own table
/// wins over `motion` for the same sequence.
#[derive(Debug, Clone)]
struct KeyTables {
    document: HashMap<String, Action>,
    motion: HashMap<String, Action>,
    cursor: HashMap<String, Action>,
    selection: HashMap<String, Action>,
}

fn table(pairs: &[(&str, Action)]) -> HashMap<String, Action> {
    pairs.iter().map(|(k, a)| (k.to_string(), *a)).collect()
}

impl Default for KeyTables {
    fn default() -> Self {
        use Action::*;
        Self {
            // `/`/`?` aren't bindings: typing a search query is free text
            // handled directly in `main.rs`'s event loop.
            document: table(&[
                ("j", ScrollDown),
                ("k", ScrollUp),
                ("g g", DocumentStart),
                ("G", DocumentEnd),
                ("c", CursorModeToggle),
                ("v", VisualModeToggle),
                ("V", VisualLineModeToggle),
                ("n", SearchNext),
                ("N", SearchPrev),
                ("y i s", CopySentence),
                ("y i p", CopyParagraph),
                ("q", Close),
                ("Esc", Close),
            ]),
            // j/k move the text cursor a visual line here, unlike
            // `document` where they scroll.
            motion: table(&[
                ("h", CharacterLeft),
                ("l", CharacterRight),
                ("j", LineDown),
                ("k", LineUp),
                ("w", WordForward),
                ("b", WordBackward),
                ("e", WordEnd),
                ("W", BigWordForward),
                ("B", BigWordBackward),
                ("E", BigWordEnd),
                ("0", LineStart),
                ("^", LineFirstNonBlank),
                ("$", LineEnd),
                ("n", SearchNext),
                ("N", SearchPrev),
                ("v", VisualModeToggle),
                ("V", VisualLineModeToggle),
                ("Esc", NormalMode),
            ]),
            // `y` is unbound here, so `y i s` is a legal sequence.
            cursor: table(&[("y i s", CopySentence), ("y i p", CopyParagraph)]),
            // `y` is bound here, so no `y ...` sequence may exist.
            selection: table(&[
                ("y", Yank),
                ("i s", SelectSentence),
                ("i p", SelectParagraph),
            ]),
        }
    }
}

impl KeyTables {
    /// Per-mode bindings after merging, with prefix validation.
    fn resolve(&self) -> Result<HashMap<Mode, HashMap<String, Action>>, ConfigError> {
        let merged = |own: &HashMap<String, Action>| {
            let mut m = self.motion.clone();
            m.extend(own.iter().map(|(k, a)| (k.clone(), *a)));
            m
        };
        let mut keys = HashMap::new();
        keys.insert(Mode::Normal, self.document.clone());
        keys.insert(Mode::Cursor, merged(&self.cursor));
        keys.insert(Mode::Visual, merged(&self.selection));
        keys.insert(Mode::VisualLine, merged(&self.selection));
        for (mode, bindings) in &keys {
            check_prefixes(mode_label(*mode), bindings)?;
        }
        Ok(keys)
    }
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "document",
        Mode::Cursor => "cursor",
        Mode::Visual => "visual",
        Mode::VisualLine => "visual_line",
    }
}

fn check_prefixes(label: &str, bindings: &HashMap<String, Action>) -> Result<(), ConfigError> {
    let seqs: Vec<KeySeq> = bindings.keys().map(|k| KeySeq::parse(k)).collect();
    for shorter in &seqs {
        if let Some(longer) = seqs.iter().find(|s| shorter.is_prefix_of(s)) {
            return Err(ConfigError::AmbiguousPrefix {
                mode: label.to_string(),
                shorter: shorter.to_string(),
                longer: longer.to_string(),
            });
        }
    }
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        let tables = KeyTables::default();
        Self {
            appearance: Appearance::default(),
            finder: Finder::default(),
            keys: tables.resolve().expect("default key tables are valid"),
            tables,
        }
    }
}

impl Config {
    /// The resolved config as TOML that loads back to the same config.
    pub fn to_toml(&self) -> String {
        let mut out = String::from("[appearance]\n");
        out.push_str(&toml::to_string(&self.appearance).expect("appearance serializes"));
        out.push_str("\n[finder]\n");
        out.push_str(&toml::to_string(&self.finder).expect("finder serializes"));
        for (name, table) in [
            ("document", &self.tables.document),
            ("motion", &self.tables.motion),
            ("cursor", &self.tables.cursor),
            ("selection", &self.tables.selection),
        ] {
            out.push_str(&format!("\n[keys.{name}]\n"));
            let mut lines: Vec<_> = table.iter().map(|(seq, a)| format!("\"{seq}\" = \"{}\"\n", a.name())).collect();
            lines.sort();
            out.extend(lines);
        }
        out
    }

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
    appearance: Appearance,
    #[serde(default)]
    finder: Finder,
    #[serde(default)]
    keys: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    UnknownMode { mode: String },
    UnknownAction { mode: String, sequence: String, action: String },
    AmbiguousPrefix { mode: String, shorter: String, longer: String },
    NoEmergencyClose,
    InvalidFinderKey { name: &'static str, key: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "could not read config file: {e}"),
            ConfigError::Parse(e) => write!(f, "could not parse config TOML: {e}"),
            ConfigError::UnknownMode { mode } => {
                write!(f, "unknown key mode [keys.{mode}] (expected one of: document, motion, cursor, selection)")
            }
            ConfigError::UnknownAction { mode, sequence, action } => write!(
                f,
                "[keys.{mode}] \"{sequence}\" = \"{action}\": unknown action \"{action}\" (valid actions: {})",
                Action::valid_names()
            ),
            ConfigError::AmbiguousPrefix { mode, shorter, longer } => write!(
                f,
                "[keys.{mode}]: \"{shorter}\" is a prefix of \"{longer}\" — ambiguous key sequence"
            ),
            ConfigError::InvalidFinderKey { name, key } => {
                write!(f, "[finder] {name} \"{key}\" is not a key (examples: \"ctrl+f\", \"ctrl+shift+f\", \"esc\")")
            }
            ConfigError::NoEmergencyClose => write!(
                f,
                "config binds no key to close/cancel in any mode — refusing to load a config with no way to quit"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

fn convert(mut raw: RawConfig) -> Result<Config, ConfigError> {
    let finder = &raw.finder;
    for (name, key) in [("open_key", &finder.open_key), ("dialog_key", &finder.dialog_key), ("close_key", &finder.close_key)] {
        if Chord::parse(key).is_none() {
            return Err(ConfigError::InvalidFinderKey { name, key: key.clone() });
        }
    }
    let ap = &mut raw.appearance;
    for (old, light, dark) in [
        (ap.selection.take(), &mut ap.selection_light, &mut ap.selection_dark),
        (ap.cursor.take(), &mut ap.cursor_light, &mut ap.cursor_dark),
        (ap.search.take(), &mut ap.search_light, &mut ap.search_dark),
        (ap.search_current.take(), &mut ap.search_current_light, &mut ap.search_current_dark),
        (ap.yank_highlight.take(), &mut ap.yank_highlight_light, &mut ap.yank_highlight_dark),
    ] {
        if let Some(color) = old {
            *light = color.clone();
            *dark = color;
        }
    }
    let mut tables = KeyTables::default();
    let parse_table = |name: &str, bindings: &HashMap<String, String>| {
        bindings
            .iter()
            .map(|(seq, action_name)| {
                Action::from_name(action_name)
                    .map(|a| (seq.clone(), a))
                    .ok_or_else(|| ConfigError::UnknownAction {
                        mode: name.to_string(),
                        sequence: seq.clone(),
                        action: action_name.clone(),
                    })
            })
            .collect::<Result<HashMap<_, _>, _>>()
    };

    for (name, bindings) in &raw.keys {
        let parsed = parse_table(name, bindings)?;
        match name.as_str() {
            "document" | "normal" => tables.document = parsed,
            "motion" => tables.motion = parsed,
            "cursor" => tables.cursor = parsed,
            "selection" => tables.selection = parsed,
            "visual" | "visual_line" => {}
            _ => return Err(ConfigError::UnknownMode { mode: name.clone() }),
        }
    }

    // Old [keys.visual] / [keys.visual_line] both map to `selection`.
    if let Some(bindings) = raw.keys.get("visual").or_else(|| raw.keys.get("visual_line")) {
        tables.selection = parse_table("visual", bindings)?;
        if let (Some(v), Some(l)) = (raw.keys.get("visual"), raw.keys.get("visual_line")) {
            if v != l {
                eprintln!("ggpdf: [keys.visual] and [keys.visual_line] differ; both are now [keys.selection], using [keys.visual]");
            }
        }
    }

    let keys = tables.resolve()?;

    let config = Config { appearance: raw.appearance, finder: raw.finder, tables, keys };

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
    fn example_config_matches_defaults() {
        let raw: RawConfig = toml::from_str(include_str!("../config.example.toml")).unwrap();
        let config = convert(raw).unwrap();
        let defaults = Config::default();
        assert_eq!(config.keys, defaults.keys);
        assert_eq!(format!("{:?}", config.appearance), format!("{:?}", defaults.appearance));
        assert_eq!(format!("{:?}", config.finder), format!("{:?}", defaults.finder));
    }

    #[test]
    fn finder_defaults_to_no_dirs() {
        let finder = Config::default().finder;
        assert!(finder.dirs.is_empty());
        assert_eq!(finder.max_depth, 4);
    }

    #[test]
    fn finder_section_overrides_defaults() {
        let raw: RawConfig = toml::from_str("[finder]\ndirs = [\"~/papers\"]\nmax_depth = 2").unwrap();
        let finder = convert(raw).unwrap().finder;
        assert_eq!(finder.dirs, ["~/papers"]);
        assert_eq!(finder.max_depth, 2);
    }

    #[test]
    fn finder_close_key_defaults_to_ctrl_c_and_accepts_esc() {
        assert_eq!(Config::default().finder.close_chord(), Chord { key: Key::C, modifiers: Modifiers::CTRL });
        let raw: RawConfig = toml::from_str("[finder]\nclose_key = \"Esc\"").unwrap();
        assert_eq!(convert(raw).unwrap().finder.close_chord(), Chord { key: Key::Escape, modifiers: Modifiers::NONE });
    }

    #[test]
    fn finder_keys_accept_any_modifier_combination_in_any_order() {
        let parse = |raw: &str| Chord::parse(raw).unwrap();
        assert!(parse("alt+f").matches(Key::F, Modifiers::ALT));
        assert!(!parse("alt+f").matches(Key::F, Modifiers::ALT | Modifiers::SHIFT));
        assert_eq!(parse("shift+alt+f"), parse("alt+shift+f"));
        assert_eq!(parse("option+f"), parse("alt+f"));
        let cmd_ctrl = Modifiers { mac_cmd: true, ctrl: true, ..Modifiers::NONE };
        assert_eq!(parse("cmd+ctrl+j"), Chord { key: Key::J, modifiers: cmd_ctrl });
        assert_eq!(parse("f"), Chord { key: Key::F, modifiers: Modifiers::NONE });
    }

    #[test]
    fn finder_open_keys_default_to_ctrl_f_and_ctrl_shift_f() {
        let finder = Config::default().finder;
        assert_eq!(finder.open_chord(), Chord { key: Key::F, modifiers: Modifiers::CTRL });
        assert_eq!(finder.dialog_chord(), Chord { key: Key::F, modifiers: Modifiers::CTRL | Modifiers::SHIFT });
    }

    #[test]
    fn invalid_finder_close_key_is_rejected() {
        let raw: RawConfig = toml::from_str("[finder]\nclose_key = \"ctrl+nope\"").unwrap();
        assert!(matches!(convert(raw).unwrap_err(), ConfigError::InvalidFinderKey { .. }));
    }

    #[test]
    fn expand_home_replaces_only_a_leading_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(expand_home("~", Some(home)), PathBuf::from("/home/me"));
        assert_eq!(expand_home("~/papers", Some(home)), PathBuf::from("/home/me/papers"));
        assert_eq!(expand_home("~other/papers", Some(home)), PathBuf::from("~other/papers"));
        assert_eq!(expand_home("/abs/~/papers", Some(home)), PathBuf::from("/abs/~/papers"));
        assert_eq!(expand_home("~/papers", None), PathBuf::from("~/papers"));
    }

    #[test]
    fn cursor_mode_can_enter_visual_modes() {
        // Regression check: v/V were only bound in Normal mode, so
        // starting to mark from an already-navigated cursor position was
        // impossible without first returning all the way to Normal.
        let config = Config::default();
        let cursor = &config.keys[&Mode::Cursor];
        assert_eq!(cursor.get("v"), Some(&Action::VisualModeToggle));
        assert_eq!(cursor.get("V"), Some(&Action::VisualLineModeToggle));
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
    fn mode_table_wins_over_motion() {
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.cursor]
            "w" = "word_end"
            "#,
        )
        .unwrap();
        let config = convert(raw).unwrap();
        assert_eq!(config.keys[&Mode::Cursor]["w"], Action::WordEnd);
        assert_eq!(config.keys[&Mode::Visual]["w"], Action::WordForward);
    }

    #[test]
    fn document_does_not_inherit_motion() {
        let config = Config::default();
        assert!(!config.keys[&Mode::Normal].contains_key("w"));
        assert_eq!(config.keys[&Mode::Cursor]["w"], Action::WordForward);
    }

    #[test]
    fn prefix_conflict_across_motion_and_mode_table_is_rejected() {
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.motion]
            "y" = "yank"
            "#,
        )
        .unwrap();
        assert!(matches!(convert(raw).unwrap_err(), ConfigError::AmbiguousPrefix { .. }));
    }

    #[test]
    fn old_visual_table_maps_to_selection() {
        let raw: RawConfig = toml::from_str(
            r#"
            [keys.visual]
            "x" = "yank"
            "#,
        )
        .unwrap();
        let config = convert(raw).unwrap();
        assert_eq!(config.keys[&Mode::VisualLine]["x"], Action::Yank);
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
    fn printed_config_loads_back_identically() {
        let mut config = Config::default();
        config.tables.cursor.insert("x".to_string(), Action::Yank);
        config.keys = config.tables.resolve().unwrap();
        let reloaded = convert(toml::from_str(&config.to_toml()).unwrap()).unwrap();
        assert_eq!(reloaded.keys, config.keys);
        assert_eq!(format!("{:?}", reloaded.appearance), format!("{:?}", config.appearance));
    }

    #[test]
    fn old_single_color_key_sets_both_themes() {
        let raw: RawConfig = toml::from_str("[appearance]\nselection = \"#112233\"\nsearch_dark = \"#445566\"").unwrap();
        let ap = convert(raw).unwrap().appearance;
        assert_eq!((ap.selection_light.as_str(), ap.selection_dark.as_str()), ("#112233", "#112233"));
        assert_eq!(ap.search_dark, "#445566");
        assert_eq!(ap.search_light, Appearance::default().search_light);
    }

    #[test]
    fn unknown_appearance_key_is_rejected() {
        assert!(toml::from_str::<RawConfig>("[appearance]\npage_gapp = 1").is_err());
    }

    #[test]
    fn unknown_action_error_lists_valid_names() {
        let err = ConfigError::UnknownAction { mode: "cursor".into(), sequence: "x".into(), action: "nope".into() };
        assert!(err.to_string().contains("yank_sentence"));
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
