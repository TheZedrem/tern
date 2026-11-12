//! The Appearance tab: font, colors, where the bars sit, the cursor, how
//! the focused pane stands out.

use crate::config::{Config, CursorStyle, SidebarSide, TabBarSide};
use crate::settings::change::Change;
use crate::settings::{Field, Page, Pick, Row, SettingsView, choice};
use gpui::{Context, IntoElement};

impl SettingsView {
    /// The Appearance tab's rows.
    pub(in crate::settings) fn appearance_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let config = &self.config;
        let family = config
            .font
            .family
            .clone()
            .unwrap_or_else(|| format!("{} (bundled)", crate::terminal::DEFAULT_FONT_FAMILY));
        let cursor_style = match config.cursor.style {
            CursorStyle::Block => "block",
            CursorStyle::Beam => "beam",
            CursorStyle::Underline => "underline",
        };
        let theme_name = config.theme.default.clone();
        let mut rows = vec![
            Row::new(
                Page::Appearance,
                "Font",
                "Font family",
                "typeface font face",
                self.picker("pick-font", family, Pick::Font, cx)
                    .into_any_element(),
            ),
            Row::new(
                Page::Appearance,
                "Font",
                "Font size",
                "text zoom font",
                self.field(Field::FontSize),
            ),
            Row::new(
                Page::Appearance,
                "Font",
                "Line height",
                "spacing font",
                self.field(Field::LineHeight),
            ),
            Row::new(
                Page::Appearance,
                "Colors",
                "Theme",
                "colors dark light system color scheme",
                self.picker("pick-theme", theme_name, Pick::Theme, cx)
                    .into_any_element(),
            ),
            Row::new(
                Page::Appearance,
                "Tabs",
                "Tab bar",
                "tabs vertical sidebar left right top bottom position side",
                self.options(
                    "tab-bar",
                    &[
                        ("top", "Top"),
                        ("bottom", "Bottom"),
                        ("left", "Left"),
                        ("right", "Right"),
                    ],
                    tab_bar_side(config),
                    |value| {
                        choice(vec!["tab_bar"], value, |config| {
                            tab_bar_side(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            Row::new(
                Page::Appearance,
                "Sidebar",
                "Sidebar",
                "sidebar sftp files hosts panel left right side remote browser",
                self.options(
                    "app-sidebar",
                    &[("left", "Left"), ("right", "Right")],
                    sftp_side(config),
                    |value| {
                        // The setting's old name goes with the first change.
                        Change::Renamed {
                            from: "sftp_sidebar",
                            to: "app_sidebar",
                            value: value.to_owned(),
                            reads: |config| sftp_side(config).to_owned(),
                        }
                    },
                    cx,
                )
                .into_any_element(),
            ),
        ];
        rows.push(Row::new(
            Page::Appearance,
            "Cursor",
            "Cursor style",
            "caret block beam underline cursor",
            self.options(
                "cursor-style",
                &[
                    ("block", "Block"),
                    ("beam", "Beam"),
                    ("underline", "Underline"),
                ],
                cursor_style,
                |value| {
                    choice(vec!["cursor", "style"], value, |config| {
                        match config.cursor.style {
                            CursorStyle::Block => "block",
                            CursorStyle::Beam => "beam",
                            CursorStyle::Underline => "underline",
                        }
                        .to_owned()
                    })
                },
                cx,
            )
            .into_any_element(),
        ));
        rows.push(Row::new(
            Page::Appearance,
            "Cursor",
            "Blinking cursor",
            "caret blink cursor",
            self.switch(
                "cursor-blink",
                config.cursor.blink,
                |on| Change::Set {
                    path: vec!["cursor", "blink"],
                    value: on.to_string(),
                    reads: |config| config.cursor.blink.to_string(),
                },
                cx,
            )
            .into_any_element(),
        ));
        rows.extend([
            Row::new(
                Page::Appearance,
                "Panes",
                "Active pane border",
                "focus focused split highlight outline width pixels",
                self.field(Field::PaneBorder),
            ),
            Row::new(
                Page::Appearance,
                "Panes",
                "Active pane border color",
                "focus focused split highlight outline accent colour hex",
                self.field(Field::PaneColor),
            ),
            Row::new(
                Page::Appearance,
                "Panes",
                "Dim inactive panes",
                "focus focused split unfocused fade dim opacity percent",
                self.field(Field::PaneDim),
            ),
        ]);
        rows
    }
}

/// `tab_bar` as the file writes it.
fn tab_bar_side(config: &Config) -> &'static str {
    match config.tab_bar {
        TabBarSide::Top => "top",
        TabBarSide::Bottom => "bottom",
        TabBarSide::Left => "left",
        TabBarSide::Right => "right",
    }
}

/// `app_sidebar` as the file writes it.
fn sftp_side(config: &Config) -> &'static str {
    match config.app_sidebar {
        SidebarSide::Left => "left",
        SidebarSide::Right => "right",
    }
}
