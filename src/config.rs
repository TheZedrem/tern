//! `config.yaml` loading (spec 0005, boot-path subset for T-010).
//!
//! A broken config never blocks boot: any read or parse failure yields the
//! defaults plus a notice naming the line. Per-key fallback, profiles and
//! hot reload arrive with T-005.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub font: Font,
    pub scrollback: u32,
    pub close_on_exit: CloseOnExit,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Font {
    /// `None` means the font bundled in the binary (spec 0005).
    pub family: Option<String>,
    pub size: f64,
    pub line_height: f64,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloseOnExit {
    Always,
    Clean,
    Never,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: Font::default(),
            scrollback: 10_000,
            close_on_exit: CloseOnExit::Clean,
        }
    }
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: None,
            size: 13.0,
            line_height: 1.2,
        }
    }
}

/// A config plus the notices produced while loading it.
#[derive(Debug, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub notices: Vec<String>,
}

/// `<config_dir>/zed-term/config.yaml`, or `None` if the OS has no config dir.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("zed-term").join("config.yaml"))
}

/// Reads and parses the config at `path`. A missing file means defaults
/// with no notice; it is never created here (no writes on the boot path).
pub fn load(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded {
            config: Config::default(),
            notices: vec![],
        },
        Err(e) => Loaded {
            config: Config::default(),
            notices: vec![format!("config.yaml could not be read ({e}); using defaults")],
        },
    }
}

pub fn parse(text: &str) -> Loaded {
    if text.trim().is_empty() {
        return Loaded {
            config: Config::default(),
            notices: vec![],
        };
    }
    match serde_saphyr::from_str::<Config>(text) {
        Ok(config) => Loaded {
            config,
            notices: vec![],
        },
        Err(e) => {
            let at = e
                .location()
                .map(|l| format!(" at line {}", l.line()))
                .unwrap_or_default();
            // The rendered error can span several lines (with a source
            // snippet); the notice keeps only the first.
            let rendered = e.to_string();
            let message = rendered.lines().next().unwrap_or("invalid YAML");
            Loaded {
                config: Config::default(),
                notices: vec![format!(
                    "config.yaml{at}: {message}. Using defaults until it's fixed."
                )],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_defaults_without_notice() {
        for text in ["", "   \n\n"] {
            let loaded = parse(text);
            assert_eq!(loaded.config, Config::default());
            assert!(loaded.notices.is_empty(), "notice for {text:?}");
        }
    }

    #[test]
    fn missing_file_is_defaults_without_notice() {
        let path = std::env::temp_dir().join("zed-term-test-does-not-exist/config.yaml");
        let loaded = load(&path);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.notices.is_empty());
    }

    #[test]
    fn invalid_yaml_gives_defaults_and_one_notice_naming_the_line() {
        // Line 3 has an unterminated flow mapping.
        let text = "scrollback: 5\nfont:\n  family: {oops\n";
        let loaded = parse(text);
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.notices.len(), 1);
        assert!(
            loaded.notices[0].contains("line 3"),
            "notice: {}",
            loaded.notices[0]
        );
    }

    #[test]
    fn wrong_type_gives_defaults_and_notice_naming_the_line() {
        let text = "font:\n  size: large\n";
        let loaded = parse(text);
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.notices.len(), 1);
        assert!(
            loaded.notices[0].contains("line 2"),
            "notice: {}",
            loaded.notices[0]
        );
    }

    #[test]
    fn partial_config_keeps_defaults_for_unspecified_keys() {
        let loaded = parse("scrollback: 500\nfont:\n  size: 16\n");
        assert!(loaded.notices.is_empty());
        let defaults = Config::default();
        assert_eq!(loaded.config.scrollback, 500);
        assert_eq!(loaded.config.font.size, 16.0);
        assert_eq!(loaded.config.font.family, defaults.font.family);
        assert_eq!(loaded.config.font.line_height, defaults.font.line_height);
        assert_eq!(loaded.config.close_on_exit, defaults.close_on_exit);
    }

    #[test]
    fn close_on_exit_parses_all_variants() {
        let cases = [
            ("clean", CloseOnExit::Clean),
            ("always", CloseOnExit::Always),
            ("never", CloseOnExit::Never),
        ];
        for (text, expected) in cases {
            let loaded = parse(&format!("close_on_exit: {text}\n"));
            assert!(loaded.notices.is_empty(), "notice for {text}");
            assert_eq!(loaded.config.close_on_exit, expected);
        }
    }
}
