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
use crate::config::Config;
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
    "open_links",
    "cursor.style",
    "cursor.blink",
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
        let fields = [Field::FontSize, Field::LineHeight, Field::Scrollback]
            .into_iter()
            .map(|field| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx).default_value(field_value(field, &config))
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
            |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    // A search shows the import cards too: look once.
                    if this.imports.is_empty() {
                        this.look_for_imports(cx);
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
    }
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.page = page;
                        this.form = None;
                        if page == Page::Import {
                            this.look_for_imports(cx);
                        }
                        this.search
                            .update(cx, |search, cx| search.set_value("", window, cx));
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
}
