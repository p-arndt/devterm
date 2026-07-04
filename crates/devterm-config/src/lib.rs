//! Configuration for DevTerm.
//!
//! Owns `config.toml` (font, size, theme, shell, scrollback) with hot-reload, the
//! keybinding schema (default keymap + tmux preset), themes, and project layout files
//! (`devterm.yml`, M2). Pure schema + validation; the file watcher lives in the app.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font_family: String::new(),
            font_size: 15.0,
            scrollback_lines: 10_000,
            shell_program: String::new(),
            shell_args: Vec::new(),
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
    fn default_path_ends_with_expected_components() {
        let path = Config::default_path();
        assert!(path.ends_with("DevTerm/config.toml") || path.ends_with("DevTerm\\config.toml"));
    }
}
