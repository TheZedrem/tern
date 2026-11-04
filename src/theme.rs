//! Themes (spec 0005), resolved to complete palettes at load so the
//! window's clear color is known before the window exists. A theme name is
//! looked up in `theme.custom`, then in `themes/<name>.yaml`, then among the
//! built-ins; the first match wins. A theme may be partial: the colors it
//! leaves out come from its `base`, `dark` unless it names another.
//!
//! Colors are `0xRRGGBB`. The same palette drives the terminal and, through
//! [`apply_to_widgets`], `gpui-component`'s widgets, so the two never
//! disagree (architecture §Key decisions).

use gpui::{App, Hsla, Rgba};
use serde::{Deserialize, Deserializer};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
    /// Widget highlights, like the marker where a dragged tab lands.
    pub accent: u32,
    /// The 16 ANSI colors: black, red, green, yellow, blue, magenta, cyan,
    /// white, then their bright variants in the same order.
    pub ansi: [u32; 16],
}

pub const DARK: Theme = Theme {
    background: 0x1b1d23,
    foreground: 0xd4d7dd,
    cursor: 0xe5e9f0,
    selection: 0x3d4350,
    accent: 0x61afef,
    ansi: [
        0x2a2e36, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xc8ccd4, //
        0x5c6370, 0xef8891, 0xb0d68f, 0xf0d197, 0x82c1f5, 0xd79aea, 0x7ccbd5, 0xffffff,
    ],
};

pub const LIGHT: Theme = Theme {
    background: 0xfafafa,
    foreground: 0x383a42,
    cursor: 0x526fff,
    selection: 0xd5dae4,
    accent: 0x4078f2,
    ansi: [
        0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7, //
        0x696c77, 0xca1243, 0x3e953a, 0x986801, 0x2f5ecf, 0x8a2b8c, 0x0a6d8f, 0xffffff,
    ],
};

/// The built-in theme `name`. They are the roots every partial theme
/// resolves to, so they live in code and can't fail to load.
pub fn built_in(name: &str) -> Option<Theme> {
    match name {
        "dark" => Some(DARK),
        "light" => Some(LIGHT),
        _ => None,
    }
}

/// `system` stands for `light` or `dark`, following the OS; every other
/// name stands for itself.
pub fn effective(name: &str, light: bool) -> &str {
    match name {
        "system" if light => "light",
        "system" => "dark",
        name => name,
    }
}

/// A `"#rrggbb"` color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.strip_prefix('#')
            .filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .map(Color)
            .ok_or_else(|| {
                serde::de::Error::custom(format!("`{text}` isn't a color like \"#1e1e2e\""))
            })
    }
}

/// A theme as written, in `theme.custom` or in a theme file. Every color is
/// optional; missing ones come from `base`.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThemeSource {
    pub base: Option<String>,
    pub background: Option<Color>,
    pub foreground: Option<Color>,
    pub cursor: Option<Color>,
    pub selection: Option<Color>,
    pub accent: Option<Color>,
    pub black: Option<Color>,
    pub red: Option<Color>,
    pub green: Option<Color>,
    pub yellow: Option<Color>,
    pub blue: Option<Color>,
    pub magenta: Option<Color>,
    pub cyan: Option<Color>,
    pub white: Option<Color>,
    pub bright_black: Option<Color>,
    pub bright_red: Option<Color>,
    pub bright_green: Option<Color>,
    pub bright_yellow: Option<Color>,
    pub bright_blue: Option<Color>,
    pub bright_magenta: Option<Color>,
    pub bright_cyan: Option<Color>,
    pub bright_white: Option<Color>,
}

impl ThemeSource {
    /// `base` with this theme's colors on top.
    fn over(&self, base: Theme) -> Theme {
        let pick = |color: Option<Color>, fallback: u32| color.map_or(fallback, |c| c.0);
        let ansi = [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
            self.bright_black,
            self.bright_red,
            self.bright_green,
            self.bright_yellow,
            self.bright_blue,
            self.bright_magenta,
            self.bright_cyan,
            self.bright_white,
        ];
        Theme {
            background: pick(self.background, base.background),
            foreground: pick(self.foreground, base.foreground),
            cursor: pick(self.cursor, base.cursor),
            selection: pick(self.selection, base.selection),
            accent: pick(self.accent, base.accent),
            ansi: std::array::from_fn(|ix| pick(ansi[ix], base.ansi[ix])),
        }
    }
}

/// How many `base` steps a theme may take; more means a loop.
const MAX_BASE_DEPTH: usize = 8;

/// Where a theme was found, in lookup order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Inline,
    File,
    BuiltIn,
}

/// Theme lookup for one config: its inline themes, then the theme files in
/// `dir`, then the built-ins. Files are read when a name needs them, so
/// boot reads only the themes its panes use (spec 0005).
pub struct Library<'a> {
    custom: &'a [(String, ThemeSource)],
    dir: Option<&'a Path>,
}

impl<'a> Library<'a> {
    pub fn new(custom: &'a [(String, ThemeSource)], dir: Option<&'a Path>) -> Self {
        Self { custom, dir }
    }

    /// The complete palette of theme `name`. An unknown name is `dark`,
    /// with a notice.
    pub fn resolve(&self, name: &str, notices: &mut Vec<String>) -> Theme {
        self.find_or_dark(name, Source::Inline, 0, notices, || {
            format!("Theme `{name}` doesn't exist; using `dark`.")
        })
    }

    /// [`Self::find`], else `dark` with the `missing` notice. A theme file
    /// that exists but doesn't parse has said so already.
    fn find_or_dark(
        &self,
        name: &str,
        from: Source,
        depth: usize,
        notices: &mut Vec<String>,
        missing: impl FnOnce() -> String,
    ) -> Theme {
        let known = notices.len();
        self.find(name, from, depth, notices).unwrap_or_else(|| {
            if notices.len() == known {
                notices.push(missing());
            }
            DARK
        })
    }

    /// Looks `name` up from `from` down the lookup order.
    fn find(
        &self,
        name: &str,
        from: Source,
        depth: usize,
        notices: &mut Vec<String>,
    ) -> Option<Theme> {
        if from <= Source::Inline
            && let Some((_, source)) = self.custom.iter().find(|(custom, _)| custom == name)
        {
            return Some(self.layer(name, Source::Inline, source, depth, notices));
        }
        if from <= Source::File
            && let Some(source) = self.file(name, notices)
        {
            return Some(self.layer(name, Source::File, &source, depth, notices));
        }
        built_in(name)
    }

    /// `source`'s colors over its resolved base.
    fn layer(
        &self,
        name: &str,
        found: Source,
        source: &ThemeSource,
        depth: usize,
        notices: &mut Vec<String>,
    ) -> Theme {
        if depth >= MAX_BASE_DEPTH {
            notices.push(format!(
                "Theme `{name}`: its `base` chain loops; the rest comes from `dark`."
            ));
            return source.over(DARK);
        }
        let base = source.base.as_deref().unwrap_or("dark");
        // A theme named like its base builds on the next source down, so
        // `theme.custom.dark` can adjust the built-in `dark`.
        let from = match (base == name, found) {
            (true, Source::Inline) => Source::File,
            (true, _) => Source::BuiltIn,
            (false, _) => Source::Inline,
        };
        let base_theme = self.find_or_dark(base, from, depth + 1, notices, || {
            format!("Theme `{name}`: base `{base}` doesn't exist; the rest comes from `dark`.")
        });
        source.over(base_theme)
    }

    /// `themes/<name>.yaml`, if there is one and it parses.
    fn file(&self, name: &str, notices: &mut Vec<String>) -> Option<ThemeSource> {
        let dir = self.dir.filter(|_| is_file_name(name))?;
        let file = format!("{name}.yaml");
        match std::fs::read_to_string(dir.join(&file)) {
            Ok(text) => match serde_saphyr::from_str::<Option<ThemeSource>>(&text) {
                Ok(source) => Some(source.unwrap_or_default()),
                Err(error) => {
                    let first = error.to_string();
                    let reason = first.lines().next().unwrap_or("invalid YAML");
                    notices.push(format!("themes/{file} was skipped: {reason}"));
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                notices.push(format!("themes/{file} could not be read: {error}"));
                None
            }
        }
    }
}

/// Whether `name` can be a theme file's name: no path separators or dots
/// at the start, so a theme name never reaches outside `themes/`.
fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `0xRRGGBB` as a GPUI color.
pub fn hsla(rgb: u32) -> Hsla {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.0,
        g: ((rgb >> 8) & 0xff) as f32 / 255.0,
        b: (rgb & 0xff) as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

/// Writes `theme` into `gpui-component`'s global theme, on top of its
/// light or dark mode (whichever the background is closer to), so widgets
/// (toasts, the tab bar, the palette) match the terminal instead of using
/// the library's own colors. The active tab shares the terminal's
/// background, so it reads as part of the pane below it.
pub fn apply_to_widgets(theme: &Theme, cx: &mut App) {
    use gpui_component::{Theme as WidgetTheme, ThemeMode};

    let background = hsla(theme.background);
    let foreground = hsla(theme.foreground);
    let mode = if background.l > 0.5 {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    WidgetTheme::change(mode, None, cx);
    let raised = background.blend(foreground.opacity(0.06));
    let colors = &mut WidgetTheme::global_mut(cx).colors;
    colors.background = background;
    colors.foreground = foreground;
    colors.popover = raised;
    colors.popover_foreground = foreground;
    colors.muted = raised;
    colors.muted_foreground = foreground.opacity(0.7);
    colors.border = background.blend(foreground.opacity(0.15));
    colors.tab_bar = raised;
    colors.tab = raised;
    colors.tab_foreground = foreground.opacity(0.7);
    colors.tab_active = background;
    colors.tab_active_foreground = foreground;
    colors.title_bar = raised;
    colors.selection = hsla(theme.selection);
    colors.drag_border = hsla(theme.accent);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, tests::TempDir};
    use std::fs;

    /// Theme `name` looked up in `custom` and the theme files in `dir`, and
    /// the notices that gave.
    fn resolve(
        custom: &[(String, ThemeSource)],
        dir: Option<&Path>,
        name: &str,
    ) -> (Theme, Vec<String>) {
        let mut notices = Vec::new();
        let theme = Library::new(custom, dir).resolve(name, &mut notices);
        (theme, notices)
    }

    /// An inline theme `name` that sets `base` and `background` only.
    fn with_background(name: &str, base: Option<&str>, rgb: u32) -> (String, ThemeSource) {
        let source = ThemeSource {
            base: base.map(str::to_owned),
            background: Some(Color(rgb)),
            ..ThemeSource::default()
        };
        (name.to_owned(), source)
    }

    /// AC5 (spec 0005).
    #[test]
    fn an_inline_theme_with_three_colors_takes_the_rest_from_dark() {
        let loaded = config::parse(concat!(
            "theme:\n",
            "  default: midnight\n",
            "  custom:\n",
            "    midnight:\n",
            "      background: \"#1e1e2e\"\n",
            "      foreground: \"#cdd6f4\"\n",
            "      cursor: \"#f5c2e7\"\n",
        ));
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        let settings = &loaded.config.theme;
        let (midnight, notices) = resolve(&settings.custom, None, &settings.default);
        let expected = Theme {
            background: 0x1e1e2e,
            foreground: 0xcdd6f4,
            cursor: 0xf5c2e7,
            ..DARK
        };
        assert_eq!(midnight, expected);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// A base is looked up like any theme name, so it can be partial too.
    #[test]
    fn a_partial_theme_takes_the_missing_colors_from_its_base() {
        let themes = TempDir::new("theme-base");
        fs::write(
            themes.path().join("paper.yaml"),
            "base: light\nforeground: \"#000002\"\n",
        )
        .expect("write paper.yaml");
        let custom = [with_background("note", Some("paper"), 0x000001)];
        let (note, notices) = resolve(&custom, Some(themes.path()), "note");
        let expected = Theme {
            background: 0x000001,
            foreground: 0x000002,
            ..LIGHT
        };
        assert_eq!(note, expected);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// AC6 (spec 0005).
    #[test]
    fn inline_beats_a_file_beats_the_built_in_of_the_same_name() {
        let themes = TempDir::new("theme-precedence");
        let file = themes.path().join("light.yaml");
        fs::write(&file, "background: \"#000002\"\n").expect("write light.yaml");
        let inline = config::parse(concat!(
            "theme:\n",
            "  custom:\n",
            "    light:\n",
            "      background: \"#000001\"\n",
        ))
        .config
        .theme
        .custom;

        let (theme, notices) = resolve(&inline, Some(themes.path()), "light");
        assert_eq!(theme.background, 0x000001, "inline");
        assert!(notices.is_empty(), "{notices:?}");
        // Removed from config.yaml: the file wins.
        let (theme, notices) = resolve(&[], Some(themes.path()), "light");
        assert_eq!(theme.background, 0x000002, "file");
        assert!(notices.is_empty(), "{notices:?}");
        // The file removed too: the built-in.
        fs::remove_file(&file).expect("remove light.yaml");
        let (theme, notices) = resolve(&[], Some(themes.path()), "light");
        assert_eq!(theme, LIGHT);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// So `theme.custom.dark` adjusts the `dark` below it instead of
    /// looping on itself.
    #[test]
    fn a_theme_named_like_its_base_builds_on_the_next_source_down() {
        let themes = TempDir::new("theme-same-name");
        fs::write(themes.path().join("dark.yaml"), "foreground: \"#000002\"\n")
            .expect("write dark.yaml");
        let custom = [with_background("dark", None, 0x000001)];
        let (dark, notices) = resolve(&custom, Some(themes.path()), "dark");
        let expected = Theme {
            background: 0x000001,
            foreground: 0x000002,
            ..DARK
        };
        assert_eq!(dark, expected);
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn an_unknown_theme_or_base_is_dark_with_a_notice() {
        let themes = TempDir::new("theme-unknown");
        let (theme, notices) = resolve(&[], Some(themes.path()), "nope");
        assert_eq!(theme, DARK);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("nope"), "{}", notices[0]);

        let custom = [with_background("midnight", Some("nope"), 0x000001)];
        let (theme, notices) = resolve(&custom, Some(themes.path()), "midnight");
        let expected = Theme {
            background: 0x000001,
            ..DARK
        };
        assert_eq!(theme, expected);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("nope"), "{}", notices[0]);
    }

    /// Whether as the theme's name or as a base, a name never reads a file
    /// outside `themes/`.
    #[test]
    fn theme_names_never_reach_outside_the_themes_dir() {
        let root = TempDir::new("theme-outside");
        let themes = root.path().join("themes");
        fs::create_dir(&themes).expect("create the themes dir");
        fs::write(
            root.path().join("outside.yaml"),
            "background: \"#ff0000\"\n",
        )
        .expect("write outside.yaml");
        let absolute = root.path().join("outside");
        let absolute = absolute.to_str().expect("a UTF-8 temp dir");
        for name in ["../outside", "..\\outside", absolute] {
            let (theme, _) = resolve(&[], Some(themes.as_path()), name);
            assert_eq!(theme, DARK, "{name}");
            let custom = [(
                "inside".to_owned(),
                ThemeSource {
                    base: Some(name.to_owned()),
                    ..ThemeSource::default()
                },
            )];
            let (theme, _) = resolve(&custom, Some(themes.as_path()), "inside");
            assert_eq!(theme, DARK, "base {name}");
        }
    }

    /// A broken file is no match, so the lookup goes on below it.
    #[test]
    fn a_theme_file_that_doesnt_parse_is_skipped_with_a_notice() {
        let themes = TempDir::new("theme-broken");
        fs::write(themes.path().join("midnight.yaml"), "background: [oops\n")
            .expect("write midnight.yaml");
        fs::write(themes.path().join("light.yaml"), "background: red\n").expect("write light.yaml");

        let (theme, notices) = resolve(&[], Some(themes.path()), "midnight");
        assert_eq!(theme, DARK);
        assert!(
            notices
                .iter()
                .any(|notice| notice.contains("midnight.yaml")),
            "{notices:?}"
        );
        let (theme, notices) = resolve(&[], Some(themes.path()), "light");
        assert_eq!(theme, LIGHT);
        assert!(
            notices.iter().any(|notice| notice.contains("light.yaml")),
            "{notices:?}"
        );
        // Theme files are strict: a typo in a key skips the file.
        fs::write(themes.path().join("paper.yaml"), "backgroud: \"#ffffff\"\n")
            .expect("write paper.yaml");
        let (theme, notices) = resolve(&[], Some(themes.path()), "paper");
        assert_eq!(theme, DARK);
        assert!(
            notices.iter().any(|notice| notice.contains("paper.yaml")),
            "{notices:?}"
        );
    }

    /// The file's own notice says what's wrong with it; "doesn't exist" on
    /// top of it would be wrong.
    #[test]
    fn a_broken_theme_file_gives_only_its_own_notice() {
        let themes = TempDir::new("theme-broken-notice");
        fs::write(themes.path().join("midnight.yaml"), "background: [oops\n")
            .expect("write midnight.yaml");
        let custom = [with_background("note", Some("midnight"), 0x000001)];
        // By its own name, and as the base of another theme.
        for name in ["midnight", "note"] {
            let (_, notices) = resolve(&custom, Some(themes.path()), name);
            assert_eq!(notices.len(), 1, "{name}: {notices:?}");
            assert!(
                notices[0].contains("themes/midnight.yaml"),
                "{name}: {}",
                notices[0]
            );
        }
    }

    #[test]
    fn a_base_chain_that_loops_ends_with_a_notice() {
        let custom = [
            with_background("a", Some("b"), 0x000001),
            (
                "b".to_owned(),
                ThemeSource {
                    base: Some("a".into()),
                    ..ThemeSource::default()
                },
            ),
        ];
        let (theme, notices) = resolve(&custom, None, "a");
        let expected = Theme {
            background: 0x000001,
            ..DARK
        };
        assert_eq!(theme, expected);
        assert_eq!(notices.len(), 1, "{notices:?}");
    }

    #[test]
    fn every_color_key_sets_its_own_color() {
        let keys = [
            "background",
            "foreground",
            "cursor",
            "selection",
            "accent",
            "black",
            "red",
            "green",
            "yellow",
            "blue",
            "magenta",
            "cyan",
            "white",
            "bright_black",
            "bright_red",
            "bright_green",
            "bright_yellow",
            "bright_blue",
            "bright_magenta",
            "bright_cyan",
            "bright_white",
        ];
        // Each key gets its position as its color: 1 for background, ...
        let text: String = keys
            .iter()
            .zip(1..)
            .map(|(key, rgb)| format!("{key}: \"#{rgb:06x}\"\n"))
            .collect();
        let themes = TempDir::new("theme-keys");
        fs::write(themes.path().join("all.yaml"), text).expect("write all.yaml");

        let (theme, notices) = resolve(&[], Some(themes.path()), "all");
        let expected = Theme {
            background: 1,
            foreground: 2,
            cursor: 3,
            selection: 4,
            accent: 5,
            ansi: std::array::from_fn(|ix| ix as u32 + 6),
        };
        assert_eq!(theme, expected);
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn colors_are_a_hash_and_six_hex_digits() {
        let color = |text: &str| serde_saphyr::from_str::<Color>(text).ok();
        assert_eq!(color("\"#1e1e2e\""), Some(Color(0x1e1e2e)));
        assert_eq!(color("\"#ABCDEF\""), Some(Color(0xabcdef)));
        for bad in [
            "\"1e1e2e\"",
            "\"#fff\"",
            "\"#1e1e2g\"",
            "\"#1e1e2e0\"",
            "\"#+1e1e2\"",
            "red",
        ] {
            assert_eq!(color(bad), None, "{bad}");
        }
    }

    /// AC9 (spec 0005): boot passes `false`, so the first frame is `dark`
    /// until the OS has been asked.
    #[test]
    fn system_is_dark_unless_the_os_is_light() {
        assert_eq!(effective("system", false), "dark");
        assert_eq!(effective("system", true), "light");
        for name in ["dark", "light", "midnight"] {
            assert_eq!(effective(name, false), name);
            assert_eq!(effective(name, true), name);
        }
    }
}
