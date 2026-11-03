//! Themes, resolved in Rust only so the frontend always gets a complete
//! palette and the window background is known before the webview exists
//! (spec 0005). T-001 ships the built-in `dark`; lookup, files and partial
//! themes arrive with T-005.
//!
//! Field names serialize to xterm.js's `ITheme` keys, so the frontend passes
//! the palette straight through.

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub background: &'static str,
    pub foreground: &'static str,
    pub cursor: &'static str,
    pub selection_background: &'static str,
    pub black: &'static str,
    pub red: &'static str,
    pub green: &'static str,
    pub yellow: &'static str,
    pub blue: &'static str,
    pub magenta: &'static str,
    pub cyan: &'static str,
    pub white: &'static str,
    pub bright_black: &'static str,
    pub bright_red: &'static str,
    pub bright_green: &'static str,
    pub bright_yellow: &'static str,
    pub bright_blue: &'static str,
    pub bright_magenta: &'static str,
    pub bright_cyan: &'static str,
    pub bright_white: &'static str,
}

pub const DARK: Theme = Theme {
    background: "#1b1d23",
    foreground: "#d4d7dd",
    cursor: "#e5e9f0",
    selection_background: "#3d4350",
    black: "#1b1d23",
    red: "#e06c75",
    green: "#98c379",
    yellow: "#e5c07b",
    blue: "#61afef",
    magenta: "#c678dd",
    cyan: "#56b6c2",
    white: "#c8ccd4",
    bright_black: "#5c6370",
    bright_red: "#ef8891",
    bright_green: "#b0d68f",
    bright_yellow: "#f0d197",
    bright_blue: "#82c1f5",
    bright_magenta: "#d79aea",
    bright_cyan: "#7ccbd5",
    bright_white: "#ffffff",
};

/// Parses `#rrggbb` into its channels.
pub fn hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}
