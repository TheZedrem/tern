//! A pane's font: family, size and line height from the config (spec
//! 0005), and the cell size they make.

use crate::config;
use gpui::{App, Pixels, SharedString, Size, Window, font, px, size};

/// The embedded default font (spec 0005).
pub const DEFAULT_FONT_FAMILY: &str = "Lilex";

/// The font sizes a pane takes, in pixels: a `font.size` of 0 would
/// otherwise size the grid at billions of cells and never reach the first
/// frame (spec 0005).
pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 4.0..=200.0;

/// Font family, size and line height of a pane's grid.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSettings {
    pub family: SharedString,
    pub size: Pixels,
    /// Multiple of the font size.
    pub line_height: f32,
}

impl FontSettings {
    /// Out-of-range sizes are clamped to [`FONT_SIZES`]. YAML's `.nan`,
    /// which `clamp` lets through, means the default.
    pub fn from_config(font: &config::Font) -> Self {
        let defaults = config::Font::default();
        let number = |value: f64, default: f64| if value.is_nan() { default } else { value };
        Self {
            family: font
                .family
                .clone()
                .unwrap_or_else(|| DEFAULT_FONT_FAMILY.to_owned())
                .into(),
            size: px(clamp_size(number(font.size, defaults.size) as f32)),
            line_height: (number(font.line_height, defaults.line_height) as f32).clamp(0.5, 4.0),
        }
    }

    /// This font, `delta` pixels bigger (smaller when negative), within
    /// [`FONT_SIZES`].
    pub fn zoomed(&self, delta: f32) -> Self {
        Self {
            size: px(clamp_size(f32::from(self.size) + delta)),
            ..self.clone()
        }
    }

    /// `font` with its family checked against the installed fonts: one that
    /// isn't installed falls back to the bundled font, with a notice (spec
    /// 0005). GPUI on its own would quietly substitute some other font.
    pub fn checked(font: &config::Font, cx: &App) -> (Self, Option<String>) {
        if font.family.is_none() {
            return (Self::from_config(font), None);
        }
        let installed = cx.text_system().all_font_names();
        Self::checked_with(font, |family| {
            installed
                .binary_search_by(|name| name.as_str().cmp(family))
                .is_ok()
        })
    }

    /// [`Self::checked`], given whether a family is installed.
    pub fn checked_with(
        font: &config::Font,
        installed: impl Fn(&str) -> bool,
    ) -> (Self, Option<String>) {
        let mut settings = Self::from_config(font);
        match &font.family {
            Some(family) if !installed(family) => {
                settings.family = DEFAULT_FONT_FAMILY.into();
                let notice = format!(
                    "Font `{family}` isn't installed; using the bundled {DEFAULT_FONT_FAMILY}."
                );
                (settings, Some(notice))
            }
            _ => (settings, None),
        }
    }

    /// A cell size good enough to size the PTY before the window and its
    /// fonts exist. The first frame measures the real one and resizes.
    pub fn estimated_cell(&self) -> (f32, f32) {
        let font_size = f32::from(self.size);
        (font_size * 0.6, (font_size * self.line_height).round())
    }

    /// The cell size of this font: the advance of `m` by the line height.
    pub fn cell_size(&self, window: &Window) -> Size<Pixels> {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font(self.family.clone()));
        let width = text_system
            .advance(font_id, self.size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(self.size * 0.6);
        size(width, (self.size * self.line_height).round())
    }
}

fn clamp_size(size: f32) -> f32 {
    size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_lilex_is_the_default_font() {
        let default = FontSettings::from_config(&config::Font::default());
        assert_eq!(default.family, "Lilex");
        let configured = FontSettings::from_config(&config::Font {
            family: Some("JetBrains Mono".into()),
            ..config::Font::default()
        });
        assert_eq!(configured.family, "JetBrains Mono");
    }

    /// AC10 (spec 0005).
    #[test]
    fn a_font_family_that_isnt_installed_falls_back_to_lilex_with_a_notice_naming_it() {
        let font = config::Font {
            family: Some("Nonexistent Mono".into()),
            size: 16.0,
            ..config::Font::default()
        };
        let (settings, notice) = FontSettings::checked_with(&font, |name| name == "JetBrains Mono");
        assert_eq!(settings.family, "Lilex");
        assert_eq!(settings.size, px(16.0), "only the family falls back");
        let notice = notice.expect("a notice");
        assert!(notice.contains("Nonexistent Mono"), "{notice}");
    }

    #[test]
    fn an_installed_font_family_is_kept_without_a_notice() {
        let font = config::Font {
            family: Some("JetBrains Mono".into()),
            ..config::Font::default()
        };
        let (settings, notice) = FontSettings::checked_with(&font, |name| name == "JetBrains Mono");
        assert_eq!(settings.family, "JetBrains Mono");
        assert_eq!(notice, None);
    }

    /// So a nonsense size can't make a grid of billions of cells (spec 0005).
    #[test]
    fn font_size_and_line_height_are_clamped_to_their_ranges() {
        let cases = [
            ((0.0, 0.0), (4.0, 0.5)),
            ((-5.0, -1.0), (4.0, 0.5)),
            ((1e12, 100.0), (200.0, 4.0)),
            ((f64::INFINITY, f64::INFINITY), (200.0, 4.0)),
            ((13.0, 1.2), (13.0, 1.2)),
        ];
        for ((size, line_height), (clamped_size, clamped_line_height)) in cases {
            let font = config::Font {
                family: None,
                size,
                line_height,
            };
            let settings = FontSettings::from_config(&font);
            assert_eq!(
                (settings.size, settings.line_height),
                (px(clamped_size), clamped_line_height),
                "size {size}, line height {line_height}"
            );
        }
    }

    /// The font-size commands (spec 0004) step the font; nothing else of
    /// it changes.
    #[test]
    fn zooming_changes_only_the_size_by_the_step() {
        let font = FontSettings::from_config(&config::Font {
            family: Some("JetBrains Mono".into()),
            size: 13.0,
            line_height: 1.4,
        });
        let bigger = FontSettings {
            size: px(15.0),
            ..font.clone()
        };
        assert_eq!(font.zoomed(2.0), bigger);
        let smaller = FontSettings {
            size: px(12.0),
            ..font.clone()
        };
        assert_eq!(font.zoomed(-1.0), smaller);
    }

    /// Spec 0005 keeps font sizes within 4–200; zooming can't leave that.
    #[test]
    fn zooming_stops_at_the_font_size_limits() {
        let font = FontSettings::from_config(&config::Font::default());
        assert_eq!(font.zoomed(-1000.0).size, px(4.0));
        assert_eq!(font.zoomed(1000.0).size, px(200.0));
    }

    /// `.nan` is a YAML float, and `clamp` would pass NaN through.
    #[test]
    fn a_nan_font_size_from_the_config_means_the_default() {
        let loaded = config::parse("font:\n  size: .nan\n  line_height: .nan\n");
        let settings = FontSettings::from_config(&loaded.config.font);
        let defaults = FontSettings::from_config(&config::Font::default());
        assert_eq!(
            (settings.size, settings.line_height),
            (defaults.size, defaults.line_height)
        );
    }
}
