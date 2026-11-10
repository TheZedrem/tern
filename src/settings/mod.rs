//! The settings screen (spec 0009): every setting of `config.yaml`, the
//! keybindings and the profiles, changed in place in the file. The file is
//! the state: the screen shows the config as loaded, follows reloads, and
//! asks the workspace to write each change.
//!
//! Every user-facing setting belongs here: a task that adds a config key
//! adds its field in the same change (AGENTS.md, Definition of Done).

pub mod change;
mod pages;
pub mod yaml;

pub use pages::update_check_name;
use pages::{Form, ImportCard};

use crate::commands::OpenConfigFolder;
use crate::config::{ActivePane, Config};
use change::Change;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme, Selectable, Sizable};

/// The screen's key context.
const CONTEXT: &str = "Settings";

/// Every `config.yaml` key the screen has a field for, as dotted paths. A
/// test holds it against the config template, so a new key without a field
/// fails the build (spec 0009, always complete).
#[cfg(test)]
pub const FIELDS: &[&str] = &[
    "default_profile",
    "profiles",
    "font.family",
    "font.size",
    "font.line_height",
    "scrollback",
    "close_on_exit",
    "copy_on_select",
    "restore_session",
    "open_new_tab",
    "open_links",
    "cursor.style",
    "cursor.blink",
    "active_pane.border",
    "active_pane.color",
    "active_pane.dim",
    "keybindings",
    "theme.default",
    "tab_bar",
    "sftp_sidebar",
    "sftp_click",
    "sftp_upload",
    "files_executable",
    "new_tab",
    "update_check",
];

/// Template keys the screen leaves to the file, and why: custom themes are
/// files of their own (spec 0009, out of scope).
#[cfg(test)]
pub const LEFT_TO_THE_FILE: &[&str] = &["theme.custom"];

actions!(tern, [CloseSettings]);

/// Binds Esc, which leaves the screen.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CloseSettings, Some(CONTEXT))]);
}

/// What the screen asks the workspace to do.
pub enum SettingsEvent {
    /// Write this to `config.yaml`.
    Change(Change),
    /// Open a picker in the palette.
    Pick(Pick),
    /// The screen wrote `config.yaml` itself (an import), which may
    /// have created it.
    Written,
    Close,
}

/// The settings picked from a list in the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Font,
    Theme,
    DefaultProfile,
}

/// A setting typed into a field, committed on Enter or on leaving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    FontSize,
    LineHeight,
    Scrollback,
    PaneBorder,
    PaneColor,
    PaneDim,
}

/// A tab of the screen, down its left side as in Tabby (spec 0009).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Appearance,
    Terminal,
    Keyboard,
    Profiles,
    Import,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::General,
        Page::Appearance,
        Page::Terminal,
        Page::Keyboard,
        Page::Profiles,
        Page::Import,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::Terminal => "Terminal",
            Page::Keyboard => "Keyboard",
            Page::Profiles => "Profiles",
            Page::Import => "Import",
        }
    }
}

/// One setting on the screen: its tab, the group heading it sits under,
/// its label, the other words search finds it by, and its control.
struct Row {
    page: Page,
    group: &'static str,
    label: SharedString,
    keywords: String,
    control: AnyElement,
}

impl Row {
    /// A row of the tabs that build theirs from a fixed label.
    fn new(
        page: Page,
        group: &'static str,
        label: &'static str,
        keywords: &str,
        control: AnyElement,
    ) -> Self {
        Row {
            page,
            group,
            label: label.into(),
            keywords: keywords.to_owned(),
            control,
        }
    }
}

/// Whether a setting matches the search `query`: every word of it starts
/// a word of the label, group, tab or keywords, ignoring case (spec 0009,
/// flow 6). An empty query matches everything.
pub fn matches(query: &str, haystack: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let words: Vec<&str> = haystack
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|wanted| words.iter().any(|word| word.starts_with(wanted)))
}

pub struct SettingsView {
    config: Config,
    focus: FocusHandle,
    fields: Vec<(Field, Entity<InputState>)>,
    /// The field being typed into, which a reload leaves alone.
    editing: Option<Field>,
    /// A field's value that isn't valid, and why.
    invalid: Option<(Field, &'static str)>,
    /// The command whose keys the next key combination becomes.
    recording: Option<&'static str>,
    _recorder: Option<Subscription>,
    form: Option<Form>,
    /// The profile whose Delete was pressed once and asks again.
    deleting: Option<String>,
    page: Page,
    /// Searches every tab; while it holds a query, the matches show
    /// instead of a tab.
    search: Entity<InputState>,
    /// The Import tab's cards, once it (or a search) first showed them.
    imports: Vec<ImportCard>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

/// The keyboard lands in the search field, ready to type.
impl Focusable for SettingsView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.focus_handle(cx)
    }
}

impl SettingsView {
    pub fn new(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let fields = [
            Field::FontSize,
            Field::LineHeight,
            Field::Scrollback,
            Field::PaneBorder,
            Field::PaneColor,
            Field::PaneDim,
        ]
        .into_iter()
        .map(|field| {
            let input = cx.new(|cx| {
                let input = InputState::new(window, cx).default_value(field_value(field, &config));
                match field {
                    Field::PaneColor => input.placeholder("theme accent"),
                    _ => input,
                }
            });
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Focus => this.editing = Some(field),
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.editing = None;
                        this.commit(field, cx);
                    }
                    _ => {}
                },
            ));
            (field, input)
        })
        .collect();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        subscriptions.push(cx.subscribe_in(
            &search,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    // A search shows the import cards too: look once.
                    if this.imports.is_empty() {
                        this.look_for_imports(window, cx);
                    }
                    cx.notify();
                }
            },
        ));
        search.update(cx, |search, cx| search.focus(window, cx));
        Self {
            page: Page::General,
            search,
            config,
            focus: cx.focus_handle(),
            fields,
            editing: None,
            invalid: None,
            recording: None,
            _recorder: None,
            form: None,
            deleting: None,
            imports: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Takes a reloaded config: the screen shows what the file says now
    /// (spec 0009, flow 1), except in the field being typed into.
    pub fn set_config(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
        self.config = config;
        for (field, input) in &self.fields {
            if self.editing != Some(*field) {
                let value = field_value(*field, &self.config);
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        cx.notify();
    }

    /// Gives the keyboard to the panel itself when the body changes under
    /// it (another tab, the profile form closing). A focused field that's
    /// no longer drawn leaves the keyboard nowhere, and Esc then reaches no
    /// binding (spec 0009, criterion 8; T-058).
    fn keep_keyboard(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    fn set(&self, change: Change, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::Change(change));
    }

    /// Writes a typed field if it holds a valid value that differs from
    /// the file's.
    fn commit(&mut self, field: Field, cx: &mut Context<Self>) {
        let Some((_, input)) = self.fields.iter().find(|(found, _)| *found == field) else {
            return;
        };
        let typed = input.read(cx).value().trim().to_owned();
        match field_change(field, &typed) {
            Ok(change) => {
                self.invalid = None;
                if typed != field_value(field, &self.config) {
                    self.set(change, cx);
                }
            }
            Err(why) => self.invalid = Some((field, why)),
        }
        cx.notify();
    }
}

/// What the file says for a typed field, as the field shows it.
fn field_value(field: Field, config: &Config) -> String {
    match field {
        Field::FontSize => number(config.font.size),
        Field::LineHeight => number(config.font.line_height),
        Field::Scrollback => config.scrollback.to_string(),
        Field::PaneBorder => config.active_pane.border.to_string(),
        Field::PaneColor => config
            .active_pane
            .color
            .map_or(String::new(), |color| format!("#{:06x}", color.0)),
        Field::PaneDim => config.active_pane.dim.to_string(),
    }
}

/// `active_pane.color` as YAML: quoted, since `#` starts a comment.
fn pane_color_yaml(config: &Config) -> String {
    config
        .active_pane
        .color
        .map_or("null".to_owned(), |color| format!("\"#{:06x}\"", color.0))
}

/// The change a typed field asks for, or why it can't be written.
fn field_change(field: Field, typed: &str) -> Result<Change, &'static str> {
    match field {
        Field::FontSize => {
            let size = typed
                .parse::<f64>()
                .ok()
                .filter(|size| (6.0..=72.0).contains(size))
                .ok_or("A size from 6 to 72.")?;
            Ok(Change::Set {
                path: vec!["font", "size"],
                value: number(size),
                reads: |config| number(config.font.size),
            })
        }
        Field::LineHeight => {
            let height = typed
                .parse::<f64>()
                .ok()
                .filter(|height| (1.0..=3.0).contains(height))
                .ok_or("A multiple of the font size from 1 to 3.")?;
            Ok(Change::Set {
                path: vec!["font", "line_height"],
                value: number(height),
                reads: |config| number(config.font.line_height),
            })
        }
        Field::Scrollback => {
            let lines = typed
                .parse::<u32>()
                .ok()
                .filter(|lines| *lines <= 1_000_000)
                .ok_or("Lines from 0 to 1 000 000.")?;
            Ok(Change::Set {
                path: vec!["scrollback"],
                value: lines.to_string(),
                reads: |config| config.scrollback.to_string(),
            })
        }
        Field::PaneBorder => {
            let width = typed
                .parse::<u32>()
                .ok()
                .filter(|width| *width <= ActivePane::MAX_BORDER)
                .ok_or("Pixels from 0 to 6.")?;
            Ok(Change::Set {
                path: vec!["active_pane", "border"],
                value: width.to_string(),
                reads: |config| config.active_pane.border.to_string(),
            })
        }
        Field::PaneColor => {
            // Empty is the theme's accent.
            let value = if typed.is_empty() {
                "null".to_owned()
            } else {
                let hex = typed
                    .strip_prefix('#')
                    .filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or("A color like #ff8800, or empty for the theme's accent.")?;
                format!("\"#{}\"", hex.to_ascii_lowercase())
            };
            Ok(Change::Set {
                path: vec!["active_pane", "color"],
                value,
                reads: pane_color_yaml,
            })
        }
        Field::PaneDim => {
            let dim = typed
                .parse::<u32>()
                .ok()
                .filter(|dim| *dim <= ActivePane::MAX_DIM)
                .ok_or("Percent from 0 to 50.")?;
            Ok(Change::Set {
                path: vec!["active_pane", "dim"],
                value: dim.to_string(),
                reads: |config| config.active_pane.dim.to_string(),
            })
        }
    }
}

/// A number as YAML: no `.0` on a whole one.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// The setting's change for a value picked from `choices`, as YAML.
fn choice(path: Vec<&'static str>, value: &'static str, reads: fn(&Config) -> String) -> Change {
    Change::Set {
        path,
        value: value.to_owned(),
        reads,
    }
}

impl SettingsView {
    fn heading(title: String, cx: &App) -> gpui::Div {
        div()
            .pt(px(14.))
            .pb(px(2.))
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(cx.theme().foreground)
            .child(title)
    }

    fn row(label: impl Into<SharedString>, control: impl IntoElement) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(div().w(px(170.)).text_sm().child(label.into()))
            .child(control)
    }

    /// Buttons for each of `options`, the current one selected.
    fn options(
        &self,
        id: &'static str,
        options: &[(&'static str, &'static str)],
        current: &str,
        change: fn(&'static str) -> Change,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex()
            .gap(px(4.))
            .children(options.iter().map(|&(value, label)| {
                Button::new((id, value.len() + label.len() * 31))
                    .label(label)
                    .small()
                    .ghost()
                    .selected(current == value)
                    .on_click(cx.listener(move |this, _, _, cx| this.set(change(value), cx)))
            }))
    }

    fn switch(
        &self,
        id: &'static str,
        on: bool,
        change: fn(bool) -> Change,
        cx: &mut Context<Self>,
    ) -> Switch {
        Switch::new(id)
            .checked(on)
            .on_click(cx.listener(move |this, on: &bool, _, cx| this.set(change(*on), cx)))
    }

    fn field(&self, field: Field) -> AnyElement {
        let input = self
            .fields
            .iter()
            .find(|(found, _)| *found == field)
            .map(|(_, input)| input.clone());
        let why = self
            .invalid
            .filter(|(invalid, _)| *invalid == field)
            .map(|(_, why)| why);
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(input.map(|input| div().w(px(120.)).child(Input::new(&input).small())))
            .children(why.map(|why| div().text_xs().child(why)))
            .into_any_element()
    }

    fn picker(
        &self,
        id: &'static str,
        label: String,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .label(label)
            .small()
            .outline()
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::Pick(pick))))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = self.general_rows(cx);
        rows.extend(self.appearance_rows(cx));
        rows.extend(self.terminal_rows(cx));
        rows.extend(self.keybinding_rows(cx));
        rows.extend(self.profile_rows(cx));
        rows.extend(self.import_rows(cx));

        let query = self.search.read(cx).value().trim().to_owned();
        let searching = !query.is_empty();
        let shown: Vec<Row> = rows
            .into_iter()
            .filter(|row| {
                if searching {
                    let haystack = format!(
                        "{} {} {} {}",
                        row.label,
                        row.group,
                        row.page.title(),
                        row.keywords
                    );
                    matches(&query, &haystack)
                } else {
                    row.page == self.page
                }
            })
            .collect();
        let found_nothing = searching && shown.is_empty();
        let mut body: Vec<AnyElement> = Vec::new();
        let mut last_heading = None;
        for row in shown {
            // Grouped under headings; while searching, with their tab.
            let heading = if searching {
                format!("{} · {}", row.page.title(), row.group)
            } else {
                row.group.to_owned()
            };
            if last_heading.as_ref() != Some(&heading) {
                body.push(Self::heading(heading.clone(), cx).into_any_element());
                last_heading = Some(heading);
            }
            body.push(Self::row(row.label, row.control).into_any_element());
        }
        if found_nothing {
            body.push(
                div()
                    .pt(px(14.))
                    .text_sm()
                    .child(format!("No setting matches “{query}”."))
                    .into_any_element(),
            );
        }
        if !searching && self.page == Page::Profiles {
            body.push(Self::profile_buttons(cx).into_any_element());
        }
        // The profile form takes the body while it's open.
        let body: Vec<AnyElement> = match self.render_form(cx) {
            Some(form) => vec![form.into_any_element()],
            None => body,
        };

        let tabs = div()
            .w(px(150.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(8.))
            .border_r_1()
            .border_color(cx.theme().border)
            .children(Page::ALL.into_iter().map(|page| {
                Button::new(("settings-tab", page as usize))
                    .label(page.title())
                    .small()
                    .ghost()
                    .w_full()
                    .selected(!searching && self.page == page)
                    // Where a test clicks; a no-op outside test builds.
                    .debug_selector(move || format!("settings-tab-{}", page.title()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.page = page;
                        this.form = None;
                        if page == Page::Import {
                            this.look_for_imports(window, cx);
                        }
                        this.search
                            .update(cx, |search, cx| search.set_value("", window, cx));
                        this.keep_keyboard(window, cx);
                        cx.notify();
                    }))
            }))
            // The build's version, pinned under the tabs: what to quote in a bug report.
            .child(
                div()
                    .mt_auto()
                    .px(px(8.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(concat!("Tern ", env!("CARGO_PKG_VERSION"))),
            );

        let theme_colors = cx.theme();
        let (border, background) = (theme_colors.border, theme_colors.popover);
        div()
            .id("settings")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseSettings, _, cx| cx.emit(SettingsEvent::Close)))
            .size_full()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(background)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(16.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .w(px(280.))
                            .child(Input::new(&self.search).small().cleanable(true)),
                    )
                    .child(
                        Button::new("settings-open-folder")
                            .label("Open config folder")
                            .small()
                            .ghost()
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenConfigFolder), cx)
                            }),
                    )
                    .child(
                        Button::new("settings-close")
                            .label("Close")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                    ),
            )
            .child(
                div().flex_1().min_h_0().flex().child(tabs).child(
                    div()
                        .id("settings-body")
                        .flex_1()
                        .overflow_y_scroll()
                        .px(px(16.))
                        .pb(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .children(body),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TEMPLATE;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Every top-level key spec 0005's schema names, and its nested keys
    /// as `parent.key` (spec 0005, Model).
    const SPEC_0005_KEYS: &[&str] = &[
        "default_profile",
        "profiles",
        "font.family",
        "font.size",
        "font.line_height",
        "scrollback",
        "cursor.style",
        "cursor.blink",
        // Spec 0003's, documented with spec 0005's.
        "active_pane.border",
        "active_pane.color",
        "active_pane.dim",
        "copy_on_select",
        "open_links",
        "tab_bar",
        // Spec 0008's, documented with spec 0005's.
        "sftp_sidebar",
        "sftp_click",
        "sftp_upload",
        // Spec 0013's, documented with spec 0005's.
        "files_executable",
        "new_tab",
        // Spec 0011's, documented with spec 0005's.
        "update_check",
        "close_on_exit",
        "restore_session",
        "open_new_tab",
        "keybindings",
        "theme.default",
        "theme.custom",
    ];

    /// A plain YAML key: lowercase words joined with `_`.
    fn key_name(text: &str) -> Option<&str> {
        let key = text
            .strip_suffix(':')
            .or_else(|| text.split_once(": ").map(|(key, _)| key))?;
        (!key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_')).then_some(key)
    }

    /// The template's keys as dotted paths, one level deep: the keys
    /// written, plus the ones it documents commented out as `# key:`.
    fn template_keys() -> Vec<String> {
        let mut keys = Vec::new();
        let mut parent: Option<&str> = None;
        for line in TEMPLATE.lines() {
            let (nested, rest) = match line.strip_prefix("  ") {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let rest = rest.strip_prefix("# ").unwrap_or(rest);
            let Some(key) = key_name(rest.split(" #").next().unwrap_or(rest).trim_end()) else {
                if !nested && !line.starts_with('#') && !line.is_empty() {
                    parent = None;
                }
                continue;
            };
            if nested {
                if let Some(parent) = parent {
                    keys.push(format!("{parent}.{key}"));
                }
            } else {
                let block = rest.trim_end().ends_with(':') && !line.starts_with('#');
                parent = block.then_some(key);
                if !block {
                    keys.push(key.to_owned());
                }
            }
        }
        // A commented `# profiles:` or `# keybindings:` is a key of its own.
        for line in TEMPLATE.lines() {
            if let Some(key) = line
                .strip_prefix("# ")
                .and_then(|rest| rest.strip_suffix(':'))
                && key_name(&format!("{key}:")).is_some()
            {
                keys.push(key.to_owned());
            }
        }
        keys
    }

    /// Spec 0014 Scope, flow 7: Import is the sixth tab, after Profiles,
    /// and a search for it by name finds it.
    #[test]
    fn the_import_tab_follows_profiles_and_is_found_by_search() {
        assert_eq!(Page::ALL[4..], [Page::Profiles, Page::Import]);
        for query in ["import", "Imp"] {
            assert!(matches(query, Page::Import.title()), "{query}");
        }
    }

    /// Spec 0009, always complete: every key the config template writes or
    /// documents has a field on the screen, or is left to the file on
    /// purpose.
    #[test]
    fn every_template_key_has_a_field_or_is_left_to_the_file() {
        let keys = template_keys();
        for expected in [
            "default_profile",
            "profiles",
            "keybindings",
            "font.size",
            "theme.custom",
        ] {
            assert!(
                keys.iter().any(|key| key == expected),
                "{expected} not found in {keys:?}"
            );
        }
        let missing: Vec<&String> = keys
            .iter()
            .filter(|key| {
                !FIELDS.contains(&key.as_str()) && !LEFT_TO_THE_FILE.contains(&key.as_str())
            })
            .collect();
        assert!(missing.is_empty(), "no settings field for {missing:?}");
    }

    /// Spec 0009, always complete: every field names a key spec 0005 knows,
    /// so the list can't drift from the config it edits.
    #[test]
    fn every_field_names_a_spec_0005_key() {
        for field in FIELDS.iter().chain(LEFT_TO_THE_FILE) {
            assert!(
                SPEC_0005_KEYS.contains(field),
                "{field} isn't a spec 0005 key"
            );
        }
    }

    /// Spec 0003, flow 3: the Appearance fields take a border of 0–6 px,
    /// a `#rrggbb` color or nothing for the theme's accent, and a dim of
    /// 0–50%; anything else isn't written and the field says why.
    #[test]
    fn the_active_pane_fields_take_only_values_in_their_ranges() {
        let cases = [
            (
                Field::PaneBorder,
                &["0", "2", "6"][..],
                &["7", "-1", "2.5", "", "wide"][..],
            ),
            (
                Field::PaneColor,
                &["", "#ff8800", "#FF8800"],
                &["ff8800", "#fff", "#ff88001", "red", "#gg0000"],
            ),
            (
                Field::PaneDim,
                &["0", "15", "50"],
                &["51", "-5", "", "half"],
            ),
        ];
        for (field, valid, invalid) in cases {
            for typed in valid {
                assert!(field_change(field, typed).is_ok(), "{field:?} {typed:?}");
            }
            for typed in invalid {
                assert!(field_change(field, typed).is_err(), "{field:?} {typed:?}");
            }
        }
    }

    /// `text` with what `field` would write for `typed`, checked as the
    /// screen does before it saves.
    fn committed(text: &str, field: Field, typed: &str) -> String {
        let change = field_change(field, typed).expect("a valid value");
        change.edited(text).expect("an edit that reads back")
    }

    /// Spec 0003, AC12, spec 0009 in-place edits: border 4 and color
    /// `#ff0000` set on the template read back as set, each rewriting only
    /// its own line and keeping that line's comment. An empty color writes
    /// `null` again, the theme's accent.
    #[test]
    fn committed_active_pane_fields_edit_the_template_in_place_and_read_back() {
        let edited = committed(TEMPLATE, Field::PaneBorder, "4");
        let edited = committed(&edited, Field::PaneColor, "#FF0000");
        let edited = committed(&edited, Field::PaneDim, "0");
        let (before, after): (Vec<&str>, Vec<&str>) =
            (TEMPLATE.lines().collect(), edited.lines().collect());
        assert_eq!(before.len(), after.len(), "{edited}");
        let changed: Vec<(&str, &str)> = before
            .into_iter()
            .zip(after)
            .filter(|(old, new)| old != new)
            .collect();
        assert_eq!(changed.len(), 3, "{changed:?}");
        for (key, value) in [("border", "4"), ("color", "\"#ff0000\""), ("dim", "0")] {
            let line = format!("  {key}: {value} ");
            assert!(
                changed
                    .iter()
                    .any(|(_, new)| new.starts_with(&line) && new.contains(" # ")),
                "no `{line}` with its comment in {changed:?}"
            );
        }
        let loaded = crate::config::parse(&edited);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        let expected = ActivePane {
            border: 4,
            color: Some(crate::theme::Color(0xff0000)),
            dim: 0,
        };
        assert_eq!(loaded.config.active_pane, expected);

        let reset = committed(&edited, Field::PaneColor, "");
        assert_eq!(crate::config::parse(&reset).config.active_pane.color, None);
    }

    /// Spec 0009: a field shows what the file says, so committing it
    /// unchanged writes the file as it was; an unset color shows empty.
    #[test]
    fn the_active_pane_fields_show_the_files_values() {
        let config = Config::default();
        assert_eq!(field_value(Field::PaneColor, &config), "");
        for field in [Field::PaneBorder, Field::PaneColor, Field::PaneDim] {
            let shown = field_value(field, &config);
            assert_eq!(committed(TEMPLATE, field, &shown), TEMPLATE, "{field:?}");
        }
        let edited = committed(TEMPLATE, Field::PaneColor, "#ff8800");
        let config = crate::config::parse(&edited).config;
        assert_eq!(field_value(Field::PaneColor, &config), "#ff8800");
    }

    /// The settings screen open in a test window, as the workspace shows
    /// it: inside the workspace's key context, with the app's keymap, and
    /// with the keyboard on the screen. `closed` counts the times it asked
    /// to be closed.
    pub(in crate::settings) struct Screen<'a> {
        pub view: Entity<SettingsView>,
        pub closed: Rc<Cell<usize>>,
        pub cx: &'a mut VisualTestContext,
    }

    /// Stands in for the workspace: draws the screen and counts its
    /// `Close` events.
    struct Host {
        view: Entity<SettingsView>,
        _events: Subscription,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context(crate::commands::keymap::CONTEXT)
                .size_full()
                .child(self.view.clone())
        }
    }

    /// Opens the screen with the default config, focused like the
    /// workspace's `open_settings` does.
    pub(in crate::settings) fn open_screen(cx: &mut TestAppContext) -> Screen<'_> {
        cx.update(|cx| {
            crate::layout::init(cx);
            crate::settings::init(cx);
            gpui_component::init(cx);
            crate::commands::keymap::keep_library(cx);
            crate::commands::keymap::bind(&[], Config::default().new_tab, cx);
        });
        let closed = Rc::new(Cell::new(0));
        let opened = Rc::new(RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let (closed, opened) = (closed.clone(), opened.clone());
            move |window, cx| {
                let host = cx.new(|cx| {
                    let view = cx.new(|cx| SettingsView::new(Config::default(), window, cx));
                    let events = cx.subscribe(&view, move |_, _, event, _| {
                        if let SettingsEvent::Close = event {
                            closed.set(closed.get() + 1);
                        }
                    });
                    Host {
                        view,
                        _events: events,
                    }
                });
                *opened.borrow_mut() = Some(host.read(cx).view.clone());
                gpui_component::Root::new(host, window, cx)
            }
        });
        let view = opened.take().expect("the window built the screen");
        cx.update(|window, cx| {
            let handle = view.focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();
        Screen { view, closed, cx }
    }

    impl Screen<'_> {
        /// Clicks the element drawn with `selector` in its middle.
        pub fn click(&mut self, selector: &'static str) {
            self.cx.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = self
                .cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} isn't drawn"));
            self.cx.simulate_click(bounds.center(), Modifiers::none());
            self.cx.run_until_parked();
        }

        /// Gives the keyboard to `input`, as a click or Tab into it does.
        pub fn focus(&mut self, input: &Entity<InputState>) {
            self.cx
                .update(|window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
            self.cx.run_until_parked();
        }

        pub fn is_focused(&mut self, input: &Entity<InputState>) -> bool {
            self.cx
                .update(|window, cx| input.focus_handle(cx).is_focused(window))
        }
    }

    /// Spec 0009, flow 1 and criterion 8: Esc right after opening, with the
    /// keyboard in the search field (flow 6), leaves the screen.
    #[gpui::test]
    fn esc_right_after_opening_closes_the_screen(cx: &mut TestAppContext) {
        let mut screen = open_screen(cx);
        let search = screen
            .cx
            .update(|_, cx| screen.view.read(cx).search.clone());
        assert!(screen.is_focused(&search), "the screen opens in its search");
        screen.cx.simulate_keystrokes("escape");
        assert_eq!(screen.closed.get(), 1);
    }

    /// Spec 0009, flows 1 and 6, criterion 8: Esc leaves the screen while
    /// the search field holds a query too.
    #[gpui::test]
    fn esc_after_typing_a_search_closes_the_screen(cx: &mut TestAppContext) {
        let screen = open_screen(cx);
        screen.cx.simulate_keystrokes("f o n t");
        let query = screen
            .cx
            .update(|_, cx| screen.view.read(cx).search.read(cx).value().to_string());
        assert_eq!(query, "font", "the keys reached the search field");
        screen.cx.simulate_keystrokes("escape");
        assert_eq!(screen.closed.get(), 1);
    }

    /// Spec 0009, criterion 8: with a field on one tab holding the keyboard,
    /// a click on another tab's button hides that field, and Esc still
    /// leaves the screen.
    #[gpui::test]
    fn esc_after_switching_tabs_from_a_focused_field_closes_the_screen(cx: &mut TestAppContext) {
        let mut screen = open_screen(cx);
        screen.click("settings-tab-Appearance");
        let font_size = screen.cx.update(|_, cx| {
            let view = screen.view.read(cx);
            assert_eq!(view.page, Page::Appearance, "the tab click went through");
            view.fields
                .iter()
                .find(|(field, _)| *field == Field::FontSize)
                .map(|(_, input)| input.clone())
                .expect("a font size field")
        });
        screen.focus(&font_size);
        assert!(screen.is_focused(&font_size));

        screen.click("settings-tab-General");
        let page = screen.cx.update(|_, cx| screen.view.read(cx).page);
        assert_eq!(page, Page::General, "the tab click went through");
        screen.cx.simulate_keystrokes("escape");
        assert_eq!(screen.closed.get(), 1);
    }
}
