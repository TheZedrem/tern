//! In-place edits of `config.yaml` that keep its comments and layout.

use super::load::{MapReader, Optional};
use super::{ensure_exists, parse};
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess};
use serde_saphyr::Spanned;
use std::path::Path;

/// Saves `name` as `theme.default` in the config at `path`, creating the
/// config from [`TEMPLATE`] if there is none, and changing nothing else
/// (spec 0005, flow 4). A file that changes while it's being edited is
/// read again once; if it changes again, nothing is written (flow 5).
pub fn save_theme_default(path: &Path, name: &str) -> Result<(), String> {
    save_edit(path, |text| {
        with_theme_default(text, name).ok_or_else(|| {
            "config.yaml writes `theme` in a way this can't change safely; \
             set `theme.default` there yourself"
                .to_owned()
        })
    })
}

/// Rewrites the config at `path` with `edit`, creating it from
/// [`TEMPLATE`] first if there is none (spec 0005, flow 4; spec 0009). A
/// file that changes while it's being edited is read again once; if it
/// changes again, nothing is written (spec 0005, flow 5).
pub fn save_edit(path: &Path, edit: impl Fn(&str) -> Result<String, String>) -> Result<(), String> {
    ensure_exists(path)?;
    // A linked config is edited where it points, so the link stays.
    let path = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let read =
        |path: &Path| std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
    for _ in 0..2 {
        let text = read(&path)?;
        let edited = edit(&text)?;
        if read(&path)? == text {
            return write_replacing(&path, &edited);
        }
    }
    Err("config.yaml kept changing while the setting was saved, so it was left alone".into())
}

/// Writes `text` over `path` through a temporary file, so a reader never
/// sees half of it, and keeps the file's permissions.
fn write_replacing(path: &Path, text: &str) -> Result<(), String> {
    let temporary = path.with_extension("yaml.tmp");
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    std::fs::write(&temporary, text).map_err(fail)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temporary, meta.permissions());
    }
    std::fs::rename(&temporary, path).map_err(fail)
}

/// `text`, a config, with `theme.default` set to `name` and the rest
/// (comments, formatting, other keys) left as it was. `None` when that
/// can't be done safely: the text isn't YAML, `theme` is written inline as
/// `{ … }`, or it isn't a mapping.
pub fn with_theme_default(text: &str, name: &str) -> Option<String> {
    let edited = edit_theme_default(text, name)?;
    // Never hand back a config that reads differently from what was asked.
    let reads_back = serde_saphyr::from_str::<IgnoredAny>(&edited).is_ok()
        && parse(&edited).config.theme.default == name;
    reads_back.then_some(edited)
}

fn edit_theme_default(text: &str, name: &str) -> Option<String> {
    let spots =
        serde_saphyr::with_deserializer_from_str(text, |de| Optional(SpotFinder).deserialize(de))
            .ok()?;
    let value = yaml_scalar(name);
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut edited = text.to_owned();
    if let Some(range) = spots.default {
        edited.replace_range(range, &value);
        return Some(edited);
    }
    match spots.theme {
        Some(ThemeSpot::Inline) => None,
        // A `theme:` block without `default`: it becomes its first line.
        Some(ThemeSpot::Block { key, indent }) => {
            let line_end = text
                .get(key..)?
                .find('\n')
                .map_or(text.len(), |len| key + len);
            let line = text[key..line_end].trim_end_matches('\r');
            // Only `theme:` with at most a comment after it starts a block
            // that takes another line: below `theme: {}` or `theme: ~` it
            // would be broken YAML.
            let after = line.split_once(':')?.1.trim_start();
            if !(after.is_empty() || after.starts_with('#')) {
                return None;
            }
            let at = line_end - usize::from(text[..line_end].ends_with('\r'));
            edited.insert_str(
                at,
                &format!("{newline}{}default: {value}", " ".repeat(indent)),
            );
            Some(edited)
        }
        None => {
            let separator = if text.is_empty() || text.ends_with('\n') {
                ""
            } else {
                newline
            };
            Some(format!(
                "{text}{separator}theme:{newline}  default: {value}{newline}"
            ))
        }
    }
}

/// `text` as a YAML scalar: plain if it reads back as the same string,
/// double-quoted otherwise.
fn yaml_scalar(text: &str) -> String {
    let plain = text.starts_with(|c: char| c.is_ascii_alphabetic())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n"
        );
    if plain {
        text.to_owned()
    } else {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Where `theme` and `theme.default` are in a config's text.
#[derive(Default)]
struct Spots {
    theme: Option<ThemeSpot>,
    /// The byte range of `theme.default`'s value.
    default: Option<std::ops::Range<usize>>,
}

enum ThemeSpot {
    /// `theme:` starts a block: the byte offset of the key, and the indent
    /// its keys have (or would have).
    Block { key: usize, indent: usize },
    /// `theme: { … }`, or a `theme` whose place in the text is unknown: left
    /// for the user to edit.
    Inline,
}

/// Finds the [`Spots`] of a document.
struct SpotFinder;

impl<'de> MapReader<'de> for SpotFinder {
    type Value = Spots;
    const EXPECTING: &'static str = "a mapping of settings";

    fn empty(self) -> Spots {
        Spots::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<Spots, A::Error> {
        let mut spots = Spots::default();
        while let Some(key) = map.next_key::<Spanned<String>>()? {
            if key.value != "theme" {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let at = key.referenced;
            let theme = map.next_value_seed(Optional(ThemeSpotFinder))?;
            spots.default = theme.default;
            spots.theme = Some(match (theme.first_key, at.span().byte_offset()) {
                (_, None) => ThemeSpot::Inline,
                (Some((line, _)), _) if line == at.line() => ThemeSpot::Inline,
                (first_key, Some(offset)) => ThemeSpot::Block {
                    key: offset as usize,
                    indent: first_key
                        .map_or(at.column().saturating_sub(1) as usize + 2, |(_, indent)| {
                            indent
                        }),
                },
            });
        }
        Ok(spots)
    }
}

/// What [`SpotFinder`] learns inside `theme:`.
#[derive(Default)]
struct ThemeKeys {
    /// The line of its first key and that key's indent.
    first_key: Option<(u64, usize)>,
    default: Option<std::ops::Range<usize>>,
}

struct ThemeSpotFinder;

impl<'de> MapReader<'de> for ThemeSpotFinder {
    type Value = ThemeKeys;
    const EXPECTING: &'static str = "`default` and `custom`";

    fn empty(self) -> ThemeKeys {
        ThemeKeys::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<ThemeKeys, A::Error> {
        let mut keys = ThemeKeys::default();
        while let Some(key) = map.next_key::<Spanned<String>>()? {
            keys.first_key.get_or_insert((
                key.referenced.line(),
                key.referenced.column().saturating_sub(1) as usize,
            ));
            if key.value != "default" {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let value = map.next_value::<Spanned<String>>()?.referenced.span();
            keys.default = value
                .byte_offset()
                .zip(value.byte_len())
                .map(|(start, len)| start as usize..(start + len) as usize);
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use crate::config::{Config, Loaded, TEMPLATE, ThemeSettings};
    use std::fs;

    /// The template with `theme.default` changed from `dark` to `name`.
    fn template_with_theme(name: &str) -> String {
        TEMPLATE.replacen("  default: dark ", &format!("  default: {name} "), 1)
    }

    /// AC8 (spec 0005): choosing a theme changes only the value of
    /// `theme.default`; comments, spacing and every other key stay.
    #[test]
    fn choosing_a_theme_changes_only_the_value_of_theme_default() {
        let edited = with_theme_default(TEMPLATE, "light").expect("edit the template");
        assert_eq!(edited, template_with_theme("light"));
        let expected = Config {
            theme: ThemeSettings {
                default: "light".into(),
                ..ThemeSettings::default()
            },
            ..Config::default()
        };
        assert_eq!(
            parse(&edited),
            Loaded {
                config: expected,
                notices: vec![]
            }
        );
    }

    #[test]
    fn an_existing_theme_default_has_only_its_value_replaced() {
        let cases = [
            (
                "theme:\n  default: dark  # mine\n  custom: {}\n",
                "theme:\n  default: light  # mine\n  custom: {}\n",
            ),
            ("theme: { default: dark }\n", "theme: { default: light }\n"),
            // Positions count bytes, not characters.
            (
                "# Farben für später → dunkel\ntheme:\n  default: dark\n",
                "# Farben für später → dunkel\ntheme:\n  default: light\n",
            ),
            // Notepad's line endings.
            (
                "theme:\r\n  default: dark\r\n",
                "theme:\r\n  default: light\r\n",
            ),
        ];
        for (text, expected) in cases {
            let edited = with_theme_default(text, "light");
            assert_eq!(edited.as_deref(), Some(expected), "{text:?}");
        }
    }

    /// A quoted value is replaced whole, so the new name reads back
    /// however it's written.
    #[test]
    fn a_quoted_theme_default_is_replaced_whole() {
        for text in [
            "theme:\n  default: \"dark\"\n",
            "theme:\n  default: 'dark'\n",
        ] {
            for name in ["light", "my theme"] {
                let edited = with_theme_default(text, name).expect(text);
                assert_eq!(parse(&edited).config.theme.default, name, "{edited:?}");
            }
        }
    }

    /// The new line goes first in the block, at the block's own indent.
    #[test]
    fn a_theme_block_without_default_gets_it_as_its_first_key() {
        let cases = [
            (
                concat!(
                    "theme:  # colors\n",
                    "    custom:\n",
                    "      paper:\n",
                    "        base: light\n",
                    "scrollback: 500\n",
                ),
                concat!(
                    "theme:  # colors\n",
                    "    default: paper\n",
                    "    custom:\n",
                    "      paper:\n",
                    "        base: light\n",
                    "scrollback: 500\n",
                ),
            ),
            // `theme:` with nothing under it.
            (
                "theme:\nscrollback: 500\n",
                "theme:\n  default: paper\nscrollback: 500\n",
            ),
            // The inserted line ends like the others.
            (
                "scrollback: 500\r\ntheme:\r\n  custom: {}\r\n",
                "scrollback: 500\r\ntheme:\r\n  default: paper\r\n  custom: {}\r\n",
            ),
        ];
        for (text, expected) in cases {
            let edited = with_theme_default(text, "paper").expect(text);
            assert_eq!(edited, expected);
            assert_eq!(parse(&edited).config.theme.default, "paper", "{edited:?}");
        }
    }

    /// The rest of the text stays as it was. A profile's `theme` is not
    /// the `theme` block.
    #[test]
    fn a_config_without_a_theme_block_gets_one_at_the_end() {
        let texts = [
            "",
            "scrollback: 500",
            "# mine\nscrollback: 500\n",
            "profiles:\n  htop:\n    type: local\n    theme: light\n",
        ];
        for text in texts {
            let edited = with_theme_default(text, "paper").expect(text);
            assert!(edited.starts_with(text), "{edited:?}");
            let mut expected = parse(text).config;
            expected.theme.default = "paper".into();
            assert_eq!(parse(&edited).config, expected, "{edited:?}");
        }
    }

    #[test]
    fn a_theme_block_added_to_a_crlf_config_ends_its_lines_in_crlf() {
        let edited = with_theme_default("scrollback: 500\r\n", "paper").expect("edit");
        assert!(!edited.replace("\r\n", "").contains('\n'), "{edited:?}");
    }

    /// Notepad can start the file with a byte order mark.
    #[test]
    fn a_config_with_a_byte_order_mark_is_edited_in_the_right_place() {
        let cases = [
            (
                "\u{feff}theme:\n  default: dark\n",
                "\u{feff}theme:\n  default: light\n",
            ),
            (
                "\u{feff}theme:\n  custom: {}\n",
                "\u{feff}theme:\n  default: light\n  custom: {}\n",
            ),
        ];
        for (text, expected) in cases {
            let edited = with_theme_default(text, "light");
            assert_eq!(edited.as_deref(), Some(expected), "{text:?}");
        }
    }

    /// Names plain YAML would read as something else (a boolean, null, a
    /// number, a comment, a mapping, ...) are quoted.
    #[test]
    fn a_chosen_theme_name_reads_back_exactly() {
        let names = [
            "yes",
            "Off",
            "true",
            "null",
            "~",
            "1.5",
            "0x1f",
            "#1e1e2e",
            "a: b",
            "[x]",
            "*x",
            "\"x\"",
            "back\\slash",
            " padded ",
            "Ünïcode",
        ];
        for text in ["", "theme:\n  default: dark\n"] {
            for name in names {
                let edited = with_theme_default(text, name).expect(name);
                assert_eq!(parse(&edited).config.theme.default, name, "{edited:?}");
            }
        }
    }

    /// Flow 4 (spec 0005) edits one value in place; adding a key to a
    /// `theme: { … }` written inline isn't that, so it's left to the user.
    #[test]
    fn an_inline_theme_without_default_is_left_alone() {
        let text = "theme: { custom: { paper: { base: light } } }\n";
        assert_eq!(with_theme_default(text, "paper"), None);
    }

    /// Whatever `theme` holds, the picker either sets the theme or changes
    /// nothing: a config that no longer parses would lose every setting.
    #[test]
    fn choosing_a_theme_never_leaves_a_config_that_doesnt_parse() {
        for text in ["theme: {}\n", "theme: {\n  custom: {}\n}\n", "theme: ~\n"] {
            if let Some(edited) = with_theme_default(text, "paper") {
                let loaded = parse(&edited);
                assert_eq!(
                    loaded.config.theme.default, "paper",
                    "{edited:?}: {:?}",
                    loaded.notices
                );
            }
        }
    }

    /// Choosing a theme before there is a config (spec 0005, flow 4): the
    /// config is the template with that theme, and nothing else is written.
    #[test]
    fn saving_a_theme_without_a_config_writes_the_template_with_it() {
        let root = TempDir::new("save-theme-new");
        let dir = root.path().join("tern");
        let path = dir.join("config.yaml");
        save_theme_default(&path, "light").expect("save the theme");
        let text = fs::read_to_string(&path).expect("read config.yaml");
        assert_eq!(text, template_with_theme("light"));
        let files: Vec<String> = fs::read_dir(&dir)
            .expect("list the config dir")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(files, ["config.yaml"]);
    }

    /// Rather than rewrite what it can't edit in place, the picker leaves
    /// the file as it is.
    #[test]
    fn saving_a_theme_into_a_config_it_cant_edit_leaves_the_file_alone() {
        let root = TempDir::new("save-theme-cant-edit");
        let path = root.path().join("config.yaml");
        for text in ["theme: { custom: {} }\n", "theme: [oops\n"] {
            fs::write(&path, text).expect("write config.yaml");
            assert!(save_theme_default(&path, "light").is_err(), "{text:?}");
            assert_eq!(fs::read_to_string(&path).expect("read it back"), text);
        }
    }

    /// Dotfile managers link config.yaml into a repository: the theme is
    /// saved there, and the link stays a link.
    #[cfg(unix)]
    #[test]
    fn saving_a_theme_into_a_linked_config_edits_the_target_and_keeps_the_link() {
        let root = TempDir::new("save-theme-linked");
        let dotfiles = root.path().join("dotfiles");
        let dir = root.path().join("tern");
        fs::create_dir(&dotfiles).expect("create the dotfiles dir");
        fs::create_dir(&dir).expect("create the config dir");
        let target = dotfiles.join("config.yaml");
        let link = dir.join("config.yaml");
        fs::write(&target, "theme:\n  default: dark\n").expect("write the config");
        std::os::unix::fs::symlink(&target, &link).expect("link config.yaml");

        save_theme_default(&link, "light").expect("save the theme");
        let meta = fs::symlink_metadata(&link).expect("stat the link");
        assert!(meta.file_type().is_symlink(), "config.yaml was replaced");
        assert_eq!(fs::read_link(&link).expect("read the link"), target);
        assert_eq!(
            fs::read_to_string(&target).expect("read the config"),
            "theme:\n  default: light\n"
        );
    }

    /// A config kept private stays private.
    #[cfg(unix)]
    #[test]
    fn saving_a_theme_keeps_the_configs_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = TempDir::new("save-theme-mode");
        let path = root.path().join("config.yaml");
        fs::write(&path, "theme:\n  default: dark\n").expect("write config.yaml");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod 600");
        save_theme_default(&path, "light").expect("save the theme");
        let mode = fs::metadata(&path)
            .expect("stat config.yaml")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
