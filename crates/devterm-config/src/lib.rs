//! Configuration for DevTerm.
//!
//! Owns `config.toml` (font, size, theme, shell, scrollback) with hot-reload, the
//! keybinding schema (default keymap + tmux preset), themes, and project layout files
//! (`devterm.yml`, M2). Pure schema + validation; the file watcher lives in the app.

#![forbid(unsafe_code)]

pub mod action;
pub mod color;
pub mod cursor;
pub mod keybinding;
pub mod shell;
pub mod theme;

pub use action::Action;
pub use color::Color;
pub use cursor::{CursorConfig, CursorShapePref};
pub use keybinding::{KeyChord, KeyCode, KeymapPreset, Mods, Named, default_keymap, tmux_preset};
pub use shell::{ResolvedShell, ShellChoice};
pub use theme::Theme;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// User configuration, deserialized from `config.toml`.
///
/// Every field uses `#[serde(default)]` so a partial file merges onto the
/// [`Default`] values.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Preferred font family; `""` lets the renderer pick its default.
    pub font_family: String,
    /// Cell font size in px at scale factor 1.0.
    pub font_size: f32,
    /// Number of scrollback lines of history to keep.
    pub scrollback_lines: usize,
    /// Shell executable; `""` falls back to `PtyCommandSpec::default_shell`.
    pub shell_program: String,
    /// Extra arguments passed to the shell.
    pub shell_args: Vec<String>,
    /// Which built-in keymap to start from before applying `keybindings`.
    pub keymap_preset: KeymapPreset,
    /// Friendly shell preset; consulted when `shell_program` is empty.
    pub shell: ShellChoice,
    /// Named built-in theme to use as the base palette (see [`Theme::builtin`]);
    /// `None` uses the default palette. An inline `[theme]` table overrides slots
    /// on top of this base. See [`Config::resolve_theme`].
    pub theme_name: Option<String>,
    /// Line-spacing multiplier applied to the cell height. `1.0` is single-spaced;
    /// a sane range is roughly `0.8..2.0`.
    pub line_height: f32,
    // Table-valued fields must come last: the TOML serializer rejects a scalar
    // field emitted after a `[table]` header.
    /// Colour theme overlay (partial `[theme]` tables override slots on top of the
    /// base selected by `theme_name`). See [`Config::resolve_theme`].
    pub theme: Theme,
    /// Cursor appearance (shape preference + blink), from `[cursor]`.
    pub cursor: CursorConfig,
    /// User keybinding overrides: chord string -> action string, from `[keybindings]`.
    pub keybindings: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font_family: String::new(),
            font_size: 15.0,
            scrollback_lines: 10_000,
            shell_program: String::new(),
            shell_args: Vec::new(),
            theme_name: None,
            line_height: 1.0,
            theme: Theme::default(),
            cursor: CursorConfig::default(),
            keymap_preset: KeymapPreset::default(),
            keybindings: BTreeMap::new(),
            shell: ShellChoice::default(),
        }
    }
}

impl Config {
    /// Load from a TOML path; on missing file returns [`Default`]; on parse error
    /// returns the error.
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default());
            }
            Err(err) => return Err(err.into()),
        };
        let config = toml::from_str(&text)?;
        Ok(config)
    }

    /// Resolve the effective keybindings.
    ///
    /// Starts from [`Config::keymap_preset`] (Default or Tmux), then applies the
    /// user overrides in [`Config::keybindings`]: each key is parsed as a
    /// [`KeyChord`] and each value as an [`Action`]. Entries that fail to parse are
    /// silently skipped (the `log` crate is not a dependency of this crate).
    ///
    /// A user override is authoritative for both the chord and the action it names:
    /// - the chord it binds takes precedence over any preset use of that chord, and
    /// - the action it binds is defined *entirely* by the user's entries — the preset
    ///   binding for that action is dropped, so rebinding an action to a new chord
    ///   *moves* it rather than adding a second chord. To give an action several
    ///   chords, list each of them.
    pub fn resolve_keybindings(&self) -> Vec<(KeyChord, Action)> {
        let user: Vec<(KeyChord, Action)> = self
            .keybindings
            .iter()
            .filter_map(|(chord_str, action_str)| {
                Some((chord_str.parse().ok()?, action_str.parse().ok()?))
            })
            .collect();

        let mut bindings: Vec<(KeyChord, Action)> = Vec::new();
        for (chord, action) in self.keymap_preset.bindings() {
            // Drop a preset binding when the user redefines that action (their entries
            // fully specify its chords) or claims that chord for something else.
            let action_rebound = user.iter().any(|(_, a)| *a == action);
            let chord_claimed = user.iter().any(|(c, _)| *c == chord);
            if !action_rebound && !chord_claimed {
                bindings.push((chord, action));
            }
        }
        bindings.extend(user);
        bindings
    }

    /// Resolve the effective colour theme.
    ///
    /// Precedence (lowest to highest):
    /// 1. The default palette ([`Theme::default`]).
    /// 2. If [`Config::theme_name`] names a built-in ([`Theme::builtin`]), that
    ///    palette becomes the base instead. An unknown name falls back to the
    ///    default.
    /// 3. Any slot the user set in the inline `[theme]` table overrides the base.
    ///    "Set" is detected by comparing each slot against [`Theme::default`], so
    ///    an inline slot whose value equals the default does not override a named
    ///    base — an accepted limitation of the merge-onto-default representation.
    pub fn resolve_theme(&self) -> Theme {
        let base = self
            .theme_name
            .as_deref()
            .and_then(Theme::builtin)
            .unwrap_or_default();
        let default = Theme::default();
        let mut resolved = base;

        for i in 0..16 {
            if self.theme.ansi[i] != default.ansi[i] {
                resolved.ansi[i] = self.theme.ansi[i];
            }
        }
        if self.theme.foreground != default.foreground {
            resolved.foreground = self.theme.foreground;
        }
        if self.theme.background != default.background {
            resolved.background = self.theme.background;
        }
        if self.theme.cursor != default.cursor {
            resolved.cursor = self.theme.cursor;
        }

        resolved
    }

    /// Serialize to a TOML file, creating the parent directory if needed.
    ///
    /// Writes the full effective config (every field, defaults included), so any
    /// comments or hand-formatting in an existing file are replaced. Used by the
    /// inline settings overlay to persist edits; the file watcher then hot-reloads.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// The default config file path (`%APPDATA%\DevTerm\config.toml`).
    pub fn default_path() -> PathBuf {
        let mut path = match std::env::var("APPDATA") {
            Ok(appdata) if !appdata.is_empty() => PathBuf::from(appdata),
            _ => PathBuf::new(),
        };
        path.push("DevTerm");
        path.push("config.toml");
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_toml() {
        let config = Config::default();
        let text = toml::to_string(&config).expect("serialize default config");
        let parsed: Config = toml::from_str(&text).expect("deserialize default config");

        assert_eq!(parsed.font_family, config.font_family);
        assert_eq!(parsed.font_size, config.font_size);
        assert_eq!(parsed.scrollback_lines, config.scrollback_lines);
        assert_eq!(parsed.shell_program, config.shell_program);
        assert_eq!(parsed.shell_args, config.shell_args);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let mut path = std::env::temp_dir();
        path.push("devterm-config-does-not-exist-04cdd40b.toml");
        // Ensure the file really is absent.
        let _ = std::fs::remove_file(&path);

        let config = Config::load(&path).expect("missing file yields default");
        let default = Config::default();
        assert_eq!(config.font_size, default.font_size);
        assert_eq!(config.scrollback_lines, default.scrollback_lines);
        assert_eq!(config.font_family, default.font_family);
    }

    #[test]
    fn partial_file_merges_onto_defaults() {
        let text = "font_size = 20.0\n";
        let config: Config = toml::from_str(text).expect("parse partial config");
        assert_eq!(config.font_size, 20.0);
        // Untouched fields keep their defaults.
        assert_eq!(config.scrollback_lines, 10_000);
    }

    #[test]
    fn parse_error_is_reported() {
        let mut path = std::env::temp_dir();
        path.push("devterm-config-invalid-04cdd40b.toml");
        std::fs::write(&path, "font_size = = broken").expect("write invalid config");

        let result = Config::load(&path);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err(), "invalid TOML should surface an error");
    }

    #[test]
    fn resolve_keybindings_defaults_to_preset() {
        let config = Config::default();
        let bindings = config.resolve_keybindings();
        assert_eq!(bindings, default_keymap());
    }

    #[test]
    fn resolve_keybindings_applies_override() {
        let mut config = Config::default();
        // Rebind the default Copy chord (ctrl+shift+c) to Quit.
        config
            .keybindings
            .insert("ctrl+shift+c".to_owned(), "quit".to_owned());
        let bindings = config.resolve_keybindings();

        let chord: KeyChord = "ctrl+shift+c".parse().unwrap();
        let action = bindings.iter().find(|(c, _)| *c == chord).map(|(_, a)| *a);
        assert_eq!(action, Some(Action::Quit));
        // Only the one binding for that chord exists (replaced, not appended).
        assert_eq!(bindings.iter().filter(|(c, _)| *c == chord).count(), 1);
    }

    #[test]
    fn resolve_keybindings_binds_new_chord() {
        let mut config = Config::default();
        config
            .keybindings
            .insert("logo+f1".to_owned(), "paste".to_owned());
        let bindings = config.resolve_keybindings();
        let chord: KeyChord = "logo+f1".parse().unwrap();
        assert!(
            bindings
                .iter()
                .any(|(c, a)| *c == chord && *a == Action::Paste)
        );
    }

    #[test]
    fn resolve_keybindings_rebind_moves_action() {
        // Rebinding an action to a new chord moves it: the preset chord stops working
        // and the action ends up bound only to the new chord.
        let mut config = Config::default();
        config
            .keybindings
            .insert("logo+f1".to_owned(), "paste".to_owned());
        let bindings = config.resolve_keybindings();

        let old: KeyChord = "ctrl+shift+v".parse().unwrap(); // default Paste chord
        assert!(
            !bindings.iter().any(|(c, _)| *c == old),
            "the preset Paste chord should be gone after a rebind"
        );
        // Paste is bound exactly once, to the new chord.
        let paste_chords: Vec<_> = bindings
            .iter()
            .filter(|(_, a)| *a == Action::Paste)
            .collect();
        assert_eq!(paste_chords.len(), 1);
        assert_eq!(paste_chords[0].0, "logo+f1".parse().unwrap());
    }

    #[test]
    fn resolve_keybindings_multiple_chords_per_action() {
        // Listing several chords for one action keeps them all.
        let mut config = Config::default();
        config
            .keybindings
            .insert("logo+f1".to_owned(), "paste".to_owned());
        config
            .keybindings
            .insert("logo+f2".to_owned(), "paste".to_owned());
        let bindings = config.resolve_keybindings();
        let count = bindings.iter().filter(|(_, a)| *a == Action::Paste).count();
        assert_eq!(count, 2);
    }

    #[test]
    fn resolve_keybindings_skips_unparseable() {
        let mut config = Config::default();
        config
            .keybindings
            .insert("not a chord".to_owned(), "quit".to_owned());
        config
            .keybindings
            .insert("ctrl+shift+z".to_owned(), "not-an-action".to_owned());
        // Should not panic and should equal the untouched preset.
        assert_eq!(config.resolve_keybindings(), default_keymap());
    }

    #[test]
    fn tmux_preset_selectable_via_config() {
        let config = Config {
            keymap_preset: KeymapPreset::Tmux,
            ..Config::default()
        };
        assert_eq!(config.resolve_keybindings(), tmux_preset());
    }

    #[test]
    fn new_fields_round_trip_and_merge() {
        let text = r##"
keymap_preset = "tmux"
shell = "git-bash"

[theme]
cursor = "#ff0000"

[keybindings]
"ctrl+shift+q" = "close-pane"
"##;
        let config: Config = toml::from_str(text).expect("parse config with new fields");
        assert_eq!(config.keymap_preset, KeymapPreset::Tmux);
        assert_eq!(config.shell, ShellChoice::GitBash);
        assert_eq!(config.theme.cursor, Color::new(0xff, 0x00, 0x00));
        // Untouched theme field keeps its default.
        assert_eq!(config.theme.foreground, Color::new(0xd0, 0xd0, 0xd0));
        assert_eq!(
            config.keybindings.get("ctrl+shift+q").map(String::as_str),
            Some("close-pane")
        );
    }

    #[test]
    fn defaults_for_new_scalar_and_table_fields() {
        let config = Config::default();
        assert_eq!(config.theme_name, None);
        assert_eq!(config.line_height, 1.0);
        assert_eq!(config.cursor.shape, CursorShapePref::Default);
        assert!(!config.cursor.blink);
    }

    #[test]
    fn resolve_theme_named_base_yields_builtin() {
        let config = Config {
            theme_name: Some("gruvbox-dark".to_owned()),
            ..Config::default()
        };
        let resolved = config.resolve_theme();
        assert_eq!(resolved, Theme::builtin("gruvbox-dark").unwrap());
        assert_ne!(resolved, Theme::default());
    }

    #[test]
    fn resolve_theme_defaults_without_name() {
        assert_eq!(Config::default().resolve_theme(), Theme::default());
    }

    #[test]
    fn resolve_theme_unknown_name_falls_back_to_default() {
        let config = Config {
            theme_name: Some("does-not-exist".to_owned()),
            ..Config::default()
        };
        assert_eq!(config.resolve_theme(), Theme::default());
    }

    #[test]
    fn resolve_theme_inline_slot_overrides_named_base() {
        let text = r##"
theme_name = "gruvbox-dark"

[theme]
cursor = "#ff0000"
"##;
        let config: Config = toml::from_str(text).expect("parse config with named base + overlay");
        let resolved = config.resolve_theme();
        // The overridden slot wins.
        assert_eq!(resolved.cursor, Color::new(0xff, 0x00, 0x00));
        // Untouched slots come from the gruvbox base, not the default.
        let gruvbox = Theme::builtin("gruvbox-dark").unwrap();
        assert_eq!(resolved.foreground, gruvbox.foreground);
        assert_eq!(resolved.background, gruvbox.background);
        assert_eq!(resolved.ansi, gruvbox.ansi);
    }

    #[test]
    fn line_height_and_cursor_round_trip_and_merge() {
        let text = r##"
line_height = 1.4

[cursor]
shape = "beam"
blink = true
"##;
        let config: Config = toml::from_str(text).expect("parse line_height + cursor");
        assert_eq!(config.line_height, 1.4);
        assert_eq!(config.cursor.shape, CursorShapePref::Beam);
        assert!(config.cursor.blink);

        // Full round-trip through TOML preserves the values.
        let out = toml::to_string(&config).expect("serialize config");
        let back: Config = toml::from_str(&out).expect("re-parse config");
        assert_eq!(back.line_height, 1.4);
        assert_eq!(back.cursor.shape, CursorShapePref::Beam);
        assert!(back.cursor.blink);
    }

    #[test]
    fn partial_cursor_table_merges_onto_default() {
        // Only `shape` set: `blink` keeps its default (false).
        let text = "[cursor]\nshape = \"block\"\n";
        let config: Config = toml::from_str(text).expect("parse partial cursor table");
        assert_eq!(config.cursor.shape, CursorShapePref::Block);
        assert!(!config.cursor.blink);
        // Other config fields keep their defaults.
        assert_eq!(config.line_height, 1.0);
    }

    #[test]
    fn default_path_ends_with_expected_components() {
        let path = Config::default_path();
        assert!(path.ends_with("DevTerm/config.toml") || path.ends_with("DevTerm\\config.toml"));
    }
}
