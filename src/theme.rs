//! Themes, resolved to complete palettes at load so the window's clear
//! color is known before the window exists (spec 0005). The built-in `dark`
//! lives here; lookup, files and partial themes arrive with T-005.
//!
//! Colors are `0xRRGGBB`. The same palette drives the terminal and, through
//! [`apply_to_widgets`], `gpui-component`'s widgets, so the two never
//! disagree (architecture §Key decisions).

use gpui::{App, Hsla, Rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
    /// The 16 ANSI colors: black, red, green, yellow, blue, magenta, cyan,
    /// white, then their bright variants in the same order.
    pub ansi: [u32; 16],
}

pub const DARK: Theme = Theme {
    background: 0x1b1d23,
    foreground: 0xd4d7dd,
    cursor: 0xe5e9f0,
    selection: 0x3d4350,
    ansi: [
        0x2a2e36, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xc8ccd4, //
        0x5c6370, 0xef8891, 0xb0d68f, 0xf0d197, 0x82c1f5, 0xd79aea, 0x7ccbd5, 0xffffff,
    ],
};

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

/// Writes `theme` into `gpui-component`'s global theme, on top of its dark
/// mode, so widgets (toasts, the tab bar, the palette) match the terminal
/// instead of using the library's own colors. The active tab shares the
/// terminal's background, so it reads as part of the pane below it.
pub fn apply_to_widgets(theme: &Theme, cx: &mut App) {
    use gpui_component::{Theme as WidgetTheme, ThemeMode};

    WidgetTheme::change(ThemeMode::Dark, None, cx);
    let background = hsla(theme.background);
    let foreground = hsla(theme.foreground);
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
    colors.drag_border = hsla(theme.cursor);
}
