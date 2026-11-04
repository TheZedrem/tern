//! Terminal colors to RGB: named and 256-color indices resolve through the
//! theme, unless the program overrode that slot with OSC 4/10/11; true color
//! passes through unchanged.

use crate::theme::Theme;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// `0xRRGGBB` as an RGB triple.
pub fn rgb(color: u32) -> Rgb {
    Rgb {
        r: (color >> 16) as u8,
        g: (color >> 8) as u8,
        b: color as u8,
    }
}

/// Two thirds of the brightness, for SGR 2 (dim) and the `Dim*` named colors.
pub fn dim(color: Rgb) -> Rgb {
    let scale = |v: u8| (u16::from(v) * 2 / 3) as u8;
    Rgb {
        r: scale(color.r),
        g: scale(color.g),
        b: scale(color.b),
    }
}

/// The default color of xterm's 256-color palette entry `index`: the
/// theme's 16 ANSI colors, then the 6×6×6 color cube, then 24 grays.
pub fn indexed(index: u8, theme: &Theme) -> Rgb {
    match index {
        0..=15 => rgb(theme.ansi[usize::from(index)]),
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            Rgb {
                r: level(i / 36),
                g: level((i / 6) % 6),
                b: level(i % 6),
            }
        }
        232..=255 => {
            let v = 8 + 10 * (index - 232);
            Rgb { r: v, g: v, b: v }
        }
    }
}

/// The theme's color for palette slot `slot`: 0–255 are the indexed colors,
/// 256 and up are the named ones (`NamedColor as usize`).
pub fn default_for_slot(slot: usize, theme: &Theme) -> Rgb {
    let foreground = rgb(theme.foreground);
    match slot {
        0..=255 => indexed(slot as u8, theme),
        s if s == NamedColor::Background as usize => rgb(theme.background),
        s if s == NamedColor::Cursor as usize => rgb(theme.cursor),
        s if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&s) => {
            dim(rgb(theme.ansi[s - NamedColor::DimBlack as usize]))
        }
        s if s == NamedColor::DimForeground as usize => dim(foreground),
        // Foreground, BrightForeground and anything unknown.
        _ => foreground,
    }
}

/// Resolves a cell color, honoring palette overrides set by the program.
pub fn resolve(color: Color, overrides: &Colors, theme: &Theme) -> Rgb {
    let slot = match color {
        Color::Spec(rgb) => return rgb,
        Color::Indexed(index) => usize::from(index),
        Color::Named(named) => named as usize,
    };
    overrides[slot].unwrap_or_else(|| default_for_slot(slot, theme))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    fn rgb_of(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    fn named(color: NamedColor) -> Rgb {
        resolve(Color::Named(color), &Colors::default(), &DARK)
    }

    #[test]
    fn hex_colors_split_into_channels() {
        assert_eq!(rgb(0x1b1d23), rgb_of(0x1b, 0x1d, 0x23));
    }

    #[test]
    fn indices_0_to_15_are_the_theme_ansi_colors() {
        for (index, &color) in (0u8..).zip(&DARK.ansi) {
            assert_eq!(indexed(index, &DARK), rgb(color), "index {index}");
        }
    }

    #[test]
    fn indices_16_to_231_are_the_xterm_color_cube() {
        // Index 16 + 36r + 6g + b; each channel steps through these levels.
        let levels = [0, 95, 135, 175, 215, 255];
        for (step, level) in (0u8..).zip(levels) {
            assert_eq!(indexed(16 + 36 * step, &DARK), rgb_of(level, 0, 0));
            assert_eq!(indexed(16 + 6 * step, &DARK), rgb_of(0, level, 0));
            assert_eq!(indexed(16 + step, &DARK), rgb_of(0, 0, level));
        }
        // xterm's #5f87af and #ff8700.
        assert_eq!(indexed(67, &DARK), rgb_of(95, 135, 175));
        assert_eq!(indexed(208, &DARK), rgb_of(255, 135, 0));
    }

    #[test]
    fn indices_232_to_255_are_24_grays_from_8_in_steps_of_10() {
        for (step, index) in (0u8..).zip(232..=255) {
            let level = 8 + 10 * step;
            let gray = rgb_of(level, level, level);
            assert_eq!(indexed(index, &DARK), gray, "index {index}");
        }
    }

    #[test]
    fn named_colors_come_from_the_theme() {
        assert_eq!(named(NamedColor::Foreground), rgb(DARK.foreground));
        assert_eq!(named(NamedColor::Background), rgb(DARK.background));
        assert_eq!(named(NamedColor::Cursor), rgb(DARK.cursor));
        assert_eq!(named(NamedColor::Red), rgb(DARK.ansi[1]));
        assert_eq!(named(NamedColor::BrightBlue), rgb(DARK.ansi[12]));
    }

    #[test]
    fn dim_named_colors_are_darker_than_their_theme_color() {
        let darker = |dim: Rgb, full: Rgb| {
            dim != full && dim.r <= full.r && dim.g <= full.g && dim.b <= full.b
        };
        let fg = rgb(DARK.foreground);
        assert!(darker(named(NamedColor::DimRed), rgb(DARK.ansi[1])));
        assert!(darker(named(NamedColor::DimWhite), rgb(DARK.ansi[7])));
        assert!(darker(named(NamedColor::DimForeground), fg));
    }

    #[test]
    fn colors_set_by_the_program_win_over_the_theme() {
        let set = rgb_of(1, 2, 3);
        let mut overrides = Colors::default();
        overrides[1] = Some(set); // OSC 4;1
        overrides[100] = Some(set); // OSC 4;100
        overrides[NamedColor::Foreground] = Some(set); // OSC 10
        overrides[NamedColor::Background] = Some(set); // OSC 11
        let resolved = |color| resolve(color, &overrides, &DARK);

        assert_eq!(resolved(Color::Indexed(1)), set);
        assert_eq!(resolved(Color::Named(NamedColor::Red)), set);
        assert_eq!(resolved(Color::Indexed(100)), set);
        assert_eq!(resolved(Color::Named(NamedColor::Foreground)), set);
        assert_eq!(resolved(Color::Named(NamedColor::Background)), set);
        // Slots the program left alone still come from the theme.
        assert_eq!(resolved(Color::Indexed(2)), rgb(DARK.ansi[2]));
    }

    #[test]
    fn true_color_passes_through_unchanged() {
        let color = rgb_of(0x12, 0x34, 0x56);
        let resolved = resolve(Color::Spec(color), &Colors::default(), &DARK);
        assert_eq!(resolved, color);
    }
}
