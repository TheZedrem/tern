//! The settings screen (spec 0009): every setting of `config.yaml`, the
//! keybindings and the profiles, changed in place in the file. The file is
//! the state: the screen shows the config as loaded, follows reloads, and
//! asks the workspace to write each change.
//!
//! Every user-facing setting belongs here: a task that adds a config key
//! adds its field in the same change (AGENTS.md, Definition of Done).

pub mod change;
pub mod yaml;

use crate::commands::{self, AppCommand, OpenConfigFolder, keymap};
use crate::config::{
    CloseOnExit, Config, CursorStyle, FilesExecutable, NewTab, OpenLinks, Profile, ProfileKind,
    SftpClick, SftpSide, TabBarSide, UpdateCheck, UploadMode,
};
use crate::import::{self, Outcome, Source};
use crate::layout::show_notice;
use change::{Change, ProfileForm};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, Keystroke, ParentElement, PathPromptOptions,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, actions, div,
    px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme, Disableable, Selectable, Sizable};
use std::path::PathBuf;

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

/// An import source's card on the Import tab (spec 0014, Model).
struct ImportCard {
    source: Source,
    /// The file it reads: the chosen one, else the default place. `None`
    /// where the app doesn't run and nothing was chosen.
    file: Option<PathBuf>,
    state: CardState,
    /// What the last import from this card did.
    imported: Option<Outcome>,
}

enum CardState {
    Looking,
    /// No file to read.
    Missing,
    /// What importing the file would do.
    Ready(Outcome),
    Importing,
    Failed(String),
}

/// The profile being added or edited.
struct Form {
    /// `None` for a new profile, which gets its ID when it's saved.
    id: Option<String>,
    ssh: bool,
    env: Vec<(String, String)>,
    inputs: Vec<(&'static str, Entity<InputState>)>,
    problem: Option<&'static str>,
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

    /// Takes the next key combination as `command`'s keys; Esc cancels
    /// (spec 0009, flow 3). It's caught before any binding runs it.
    fn record(&mut self, command: &'static str, cx: &mut Context<Self>) {
        self.recording = Some(command);
        let this = cx.entity().downgrade();
        self._recorder = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = event.keystroke.clone();
            if keystroke.is_modifier_key() {
                return;
            }
            cx.stop_propagation();
            let _ = this.update(cx, |this, cx| {
                let Some(command) = this.recording.take() else {
                    return;
                };
                this._recorder = None;
                let cancel = keystroke.key == "escape" && !keystroke.modifiers.modified();
                if !cancel && let Some(keys) = config_keys(&keystroke) {
                    this.set(
                        Change::Bind {
                            command,
                            keys: Some(keys),
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Opens the profile form, empty or filled from `profile`.
    fn open_form(
        &mut self,
        ssh: bool,
        profile: Option<&Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = dirs::home_dir();
        let form = profile.map_or_else(
            || ProfileForm {
                ssh,
                ..ProfileForm::default()
            },
            |profile| ProfileForm::of(profile, home.as_deref()),
        );
        let fields: &[(&'static str, &str)] = if form.ssh {
            &[
                ("Name", &form.name),
                ("Host", &form.host),
                ("User", &form.user),
                ("Port", &form.port),
                ("Key file", &form.identity_file),
                ("Group", &form.group),
                ("Theme", &form.theme),
            ]
        } else {
            &[
                ("Name", &form.name),
                ("Command", &form.command),
                ("Directory", &form.cwd),
                ("Theme", &form.theme),
            ]
        };
        let inputs = fields
            .iter()
            .map(|(label, value)| {
                let value = value.to_string();
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
                (*label, input)
            })
            .collect();
        self.form = Some(Form {
            id: profile.map(|profile| profile.id.clone()),
            ssh: form.ssh,
            env: form.env,
            inputs,
            problem: None,
        });
        cx.notify();
    }

    /// Asks the OS's folder picker for the profile form's directory and
    /// writes the pick into its field; cancelling leaves the field alone,
    /// and a picker that can't open says why.
    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let failed = match picked.await {
                Ok(Ok(Some(paths))) => Ok(paths),
                Ok(Ok(None)) => return,
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err("it closed without an answer".to_owned()),
            };
            let paths = match failed {
                Ok(paths) => paths,
                Err(error) => {
                    let notice = format!("The folder picker didn't open: {error}");
                    let _ = cx.update(|window, cx| show_notice(notice, window, cx));
                    return;
                }
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let value = change::home_relative(&path, dirs::home_dir().as_deref());
            let _ = this.update_in(cx, |this, window, cx| {
                let input = this.form.as_ref().and_then(|form| {
                    form.inputs
                        .iter()
                        .find(|(label, _)| *label == "Directory")
                        .map(|(_, input)| input.clone())
                });
                if let Some(input) = input {
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }
            });
        })
        .detach();
    }

    /// Saves the form as a profile (spec 0009, flow 4), unless it isn't
    /// complete.
    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let value = |label: &str| {
            form.inputs
                .iter()
                .find(|(found, _)| *found == label)
                .map(|(_, input)| input.read(cx).value().trim().to_owned())
                .unwrap_or_default()
        };
        let profile = ProfileForm {
            name: value("Name"),
            ssh: form.ssh,
            command: value("Command"),
            cwd: value("Directory"),
            host: value("Host"),
            user: value("User"),
            port: value("Port"),
            identity_file: value("Key file"),
            group: value("Group"),
            theme: value("Theme"),
            env: form.env.clone(),
        };
        if let Some(problem) = profile.problem() {
            form.problem = Some(problem);
            cx.notify();
            return;
        }
        let id = form.id.clone().unwrap_or_else(|| {
            let mut taken: Vec<String> =
                self.config.profiles.iter().map(|p| p.id.clone()).collect();
            taken.push("shell".into());
            let name = if profile.name.is_empty() {
                &profile.host
            } else {
                &profile.name
            };
            change::new_id(name, &taken)
        });
        self.form = None;
        self.set(
            Change::Profile {
                id,
                profile: Some(Box::new(profile)),
            },
            cx,
        );
        cx.notify();
    }

    /// Looks at every import source's file afresh (spec 0014, flow 1):
    /// the default place, or the file chosen on its card.
    fn look_for_imports(&mut self, cx: &mut Context<Self>) {
        if self.imports.is_empty() {
            self.imports = Source::ALL
                .into_iter()
                .map(|source| ImportCard {
                    source,
                    file: source.default_path(),
                    state: CardState::Looking,
                    imported: None,
                })
                .collect();
        }
        for source in Source::ALL {
            self.import_from(source, None, false, cx);
        }
    }

    /// Plans the import of `source`'s card or, with `write`, runs it, from
    /// `file` if given, on the background executor; the card shows the
    /// outcome. An import plans again after writing, so the card shows
    /// what's left (spec 0014, flow 4).
    fn import_from(
        &mut self,
        source: Source,
        file: Option<PathBuf>,
        write: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.imports.iter_mut().find(|card| card.source == source) else {
            return;
        };
        if let Some(file) = file {
            card.file = Some(file);
            card.imported = None;
        }
        let Some(file) = card.file.clone() else {
            card.state = CardState::Missing;
            cx.notify();
            return;
        };
        card.state = if write {
            CardState::Importing
        } else {
            CardState::Looking
        };
        cx.notify();
        let work = cx.background_spawn(async move {
            if !file.exists() {
                return (None, None);
            }
            let imported = write.then(|| import::outcome(&file, false));
            let planned = match &imported {
                Some(Err(error)) => Err(error.clone()),
                _ => import::outcome(&file, true),
            };
            (imported, Some(planned))
        });
        cx.spawn(async move |this, cx| {
            let (imported, planned) = work.await;
            let _ = this.update(cx, |this, cx| {
                if imported.as_ref().is_some_and(Result::is_ok) {
                    cx.emit(SettingsEvent::Written);
                }
                let Some(card) = this.imports.iter_mut().find(|card| card.source == source) else {
                    return;
                };
                if let Some(Ok(outcome)) = imported {
                    card.imported = Some(outcome);
                }
                card.state = match planned {
                    None => CardState::Missing,
                    Some(Ok(outcome)) => CardState::Ready(outcome),
                    Some(Err(error)) => CardState::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks the OS's file picker for a file to import on `source`'s card
    /// (spec 0014, flow 5); cancelling changes nothing.
    fn choose_import(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    let notice = format!("The file picker didn't open: {error:#}");
                    let _ = cx.update(|window, cx| show_notice(notice, window, cx));
                    None
                }
            };
            if let Some(path) = path {
                let _ = this.update(cx, |this, cx| {
                    this.import_from(source, Some(path), false, cx)
                });
            }
        })
        .detach();
    }

    fn delete_profile(&mut self, id: String, cx: &mut Context<Self>) {
        if self.deleting.as_ref() == Some(&id) {
            self.deleting = None;
            self.set(Change::Profile { id, profile: None }, cx);
        } else {
            self.deleting = Some(id);
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

/// `new_tab` as the file writes it.
fn new_tab_name(config: &Config) -> &'static str {
    match config.new_tab {
        NewTab::Ask => "ask",
        NewTab::Default => "default",
    }
}

/// `update_check` as the file writes it; empty while it's not chosen,
/// which is never written (spec 0011).
pub fn update_check_name(config: &Config) -> &'static str {
    match config.update_check {
        Some(UpdateCheck::Daily) => "daily",
        Some(UpdateCheck::Off) => "off",
        None => "",
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

/// `sftp_sidebar` as the file writes it.
fn sftp_side(config: &Config) -> &'static str {
    match config.sftp_sidebar {
        SftpSide::Left => "left",
        SftpSide::Right => "right",
    }
}

/// `sftp_click` as the file writes it.
fn sftp_click_name(config: &Config) -> &'static str {
    match config.sftp_click {
        SftpClick::Edit => "edit",
        SftpClick::Download => "download",
    }
}

/// `sftp_upload` as the file writes it.
fn sftp_upload_name(config: &Config) -> &'static str {
    match config.sftp_upload {
        UploadMode::Manual => "manual",
        UploadMode::Save => "save",
        UploadMode::Focus => "focus",
    }
}

/// `files_executable` as the file writes it.
fn files_executable_name(config: &Config) -> &'static str {
    match config.files_executable {
        FilesExecutable::Ask => "ask",
        FilesExecutable::Run => "run",
        FilesExecutable::Edit => "edit",
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

/// A key combination as `keybindings:` writes it (spec 0004), like
/// `ctrl+shift+k`; `None` for a modifier alone.
pub fn config_keys(keystroke: &Keystroke) -> Option<String> {
    if keystroke.is_modifier_key() || keystroke.key.is_empty() {
        return None;
    }
    let modifiers = keystroke.modifiers;
    let platform = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "super"
    };
    let mut parts: Vec<&str> = [
        (modifiers.control, "ctrl"),
        (modifiers.alt, "alt"),
        (modifiers.shift, "shift"),
        (modifiers.platform, platform),
    ]
    .into_iter()
    .filter_map(|(held, name)| held.then_some(name))
    .collect();
    parts.push(&keystroke.key);
    Some(parts.join("+"))
}

/// GPUI's spelling of keys, like `ctrl-shift-d`, as config writes them.
fn shown_keys(keys: &str) -> String {
    Keystroke::parse_with_os(keys, keymap::OS)
        .ok()
        .and_then(|keystroke| config_keys(&keystroke))
        .unwrap_or_else(|| keys.to_owned())
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

    fn keybinding_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let (planned, _) = keymap::plan_for(&self.config.keybindings, self.config.new_tab);
        let own: Vec<&str> = self
            .config
            .keybindings
            .iter()
            .map(|binding| binding.command.as_str())
            .collect();
        let rows = commands::COMMANDS
            .iter()
            .filter_map(|command: &'static AppCommand| Some((command, command.title?)))
            .map(|(command, title)| {
                let keys: Vec<String> = planned
                    .iter()
                    .filter(|(_, bound)| bound.id == command.id)
                    .map(|(keys, _)| shown_keys(keys))
                    .collect();
                let recording = self.recording == Some(command.id);
                let shown = if recording {
                    "Press keys… (Esc cancels)".to_owned()
                } else if keys.is_empty() {
                    "—".to_owned()
                } else {
                    keys.join(", ")
                };
                let id = command.id;
                let control = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(200.)).text_sm().child(shown))
                    .child(
                        Button::new(("record", id.len() * 131 + title.len()))
                            .label("Record")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| this.record(id, cx))),
                    )
                    .when(own.contains(&id), |row| {
                        row.child(
                            Button::new(("reset", id.len() * 131 + title.len()))
                                .label("Reset")
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set(
                                        Change::Bind {
                                            command: id,
                                            keys: None,
                                        },
                                        cx,
                                    );
                                })),
                        )
                    });
                Row {
                    page: Page::Keyboard,
                    group: "Keybindings",
                    label: title.into(),
                    keywords: format!("{id} {} shortcut hotkey key", keys.join(" ")),
                    control: control.into_any_element(),
                }
            });
        rows.collect()
    }

    /// The profile form while one is added or edited; it takes the
    /// screen's body.
    fn render_form(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let form = self.form.as_ref()?;
        let title = match (&form.id, form.ssh) {
            (Some(id), _) => format!("Edit {id}"),
            (None, true) => "New SSH profile".to_owned(),
            (None, false) => "New local profile".to_owned(),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(Self::heading(title, cx))
                .children(form.inputs.iter().map(|(label, input)| {
                    let field = div()
                        .w(px(320.))
                        .flex()
                        .gap(px(6.))
                        .child(div().flex_1().child(Input::new(input).small()));
                    let field = match *label {
                        "Directory" => field.child(
                            Button::new("profile-browse-cwd")
                                .label("Browse…")
                                .small()
                                .ghost()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                                ),
                        ),
                        _ => field,
                    };
                    Self::row(*label, field)
                }))
                .children(form.problem.map(|problem| div().text_xs().child(problem)))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            Button::new("profile-save")
                                .label("Save")
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| this.save_form(cx))),
                        )
                        .child(
                            Button::new("profile-cancel")
                                .label("Cancel")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.form = None;
                                    cx.notify();
                                })),
                        ),
                ),
        )
    }

    fn profile_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        self.config
            .profiles
            .iter()
            .enumerate()
            .map(|(ix, profile)| {
                let what = match &profile.kind {
                    ProfileKind::Local(local) => local
                        .command
                        .as_ref()
                        .map_or("login shell".to_owned(), |command| command.join(" ")),
                    ProfileKind::Ssh(ssh) => format!("ssh {}", ssh.host),
                };
                let (edit, delete) = (profile.clone(), profile.id.clone());
                let asking = self.deleting.as_ref() == Some(&profile.id);
                let (group, keywords) = match &profile.kind {
                    ProfileKind::Local(_) => ("Local profiles", format!("{} {what}", profile.id)),
                    ProfileKind::Ssh(ssh) => (
                        "SSH profiles",
                        format!(
                            "{} {what} {} {} host server",
                            profile.id,
                            ssh.user.as_deref().unwrap_or_default(),
                            ssh.group.as_deref().unwrap_or_default()
                        ),
                    ),
                };
                let control = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(200.)).text_sm().truncate().child(what))
                    .child(
                        Button::new(("profile-edit", ix))
                            .label("Edit")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let ssh = matches!(edit.kind, ProfileKind::Ssh(_));
                                this.open_form(ssh, Some(&edit), window, cx);
                            })),
                    )
                    .child(
                        Button::new(("profile-delete", ix))
                            .label(if asking { "Delete it?" } else { "Delete" })
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_profile(delete.clone(), cx);
                            })),
                    );
                Row {
                    page: Page::Profiles,
                    group,
                    label: profile.display_name().to_owned().into(),
                    keywords,
                    control: control.into_any_element(),
                }
            })
            .collect()
    }

    /// Below the profile list: adding one.
    fn profile_buttons(cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap(px(6.))
            .pt(px(10.))
            .child(
                Button::new("profile-add-local")
                    .label("Add local profile")
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_form(false, None, window, cx);
                    })),
            )
            .child(
                Button::new("profile-add-ssh")
                    .label("Add SSH profile")
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_form(true, None, window, cx);
                    })),
            )
    }
}

impl SettingsView {
    /// The Import tab: a row per source, with what its file holds and the
    /// buttons to import it (spec 0014).
    fn import_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let muted = cx.theme().muted_foreground;
        self.imports
            .iter()
            .map(|card| {
                let source = card.source;
                let title = source.title();
                let (lines, can_import) = card_lines(card);
                let file = card
                    .file
                    .as_ref()
                    .filter(|_| !matches!(card.state, CardState::Missing))
                    .map(|file| file.display().to_string());
                let control = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .py(px(4.))
                    .children(lines.into_iter().map(|line| div().text_sm().child(line)))
                    .children(
                        file.map(|file| div().text_xs().text_color(muted).truncate().child(file)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                Button::new(("import-run", source as usize))
                                    .label("Import")
                                    .small()
                                    .primary()
                                    .disabled(!can_import)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.import_from(source, None, true, cx);
                                    })),
                            )
                            .child(
                                Button::new(("import-choose", source as usize))
                                    .label("Choose file…")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose_import(source, window, cx);
                                    })),
                            ),
                    );
                Row {
                    page: Page::Import,
                    group: "Import SSH hosts",
                    label: title.into(),
                    keywords: format!("import migrate hosts sessions ssh {}", title.to_lowercase()),
                    control: control.into_any_element(),
                }
            })
            .collect()
    }
}

/// What an import card says, line by line, and whether its Import button
/// has anything to do (spec 0014, flows 2 to 6).
fn card_lines(card: &ImportCard) -> (Vec<String>, bool) {
    let title = card.source.title();
    let mut lines = Vec::new();
    if let Some(done) = &card.imported {
        lines.push(format!("Imported {}.", hosts(done.new)));
    }
    let mut can_import = false;
    match &card.state {
        CardState::Looking => lines.push("Looking…".into()),
        CardState::Importing => lines.push("Importing…".into()),
        CardState::Missing => lines.push(match &card.file {
            Some(file) => format!("Not found at {}", file.display()),
            None => format!("{title} runs on Windows: choose an exported .mxtsessions file."),
        }),
        CardState::Failed(error) => lines.push(error.clone()),
        CardState::Ready(outcome) => {
            if outcome.format != title {
                lines.push(format!("This is a {} file.", outcome.format));
            }
            if outcome.new == 0 {
                lines.push("Everything is already in Tern.".into());
            } else {
                can_import = true;
                let groups = match outcome.groups {
                    0 => String::new(),
                    1 => " in 1 group".into(),
                    n => format!(" in {n} groups"),
                };
                let new = match outcome.new {
                    1 => "1 new host".to_owned(),
                    n => format!("{n} new hosts"),
                };
                lines.push(format!("{new}{groups}."));
            }
            let mut also = Vec::new();
            if outcome.duplicates > 0 {
                also.push(format!("{} already there", outcome.duplicates));
            }
            if outcome.skipped > 0 {
                also.push(format!(
                    "{} skipped (not SSH, or behind a jump host or proxy)",
                    outcome.skipped
                ));
            }
            if !also.is_empty() {
                lines.push(also.join("; "));
            }
        }
    }
    (lines, can_import)
}

/// `n hosts`, or `1 host`.
fn hosts(n: usize) -> String {
    if n == 1 {
        "1 host".to_owned()
    } else {
        format!("{n} hosts")
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let close = match config.close_on_exit {
            CloseOnExit::Always => "always",
            CloseOnExit::Clean => "clean",
            CloseOnExit::Never => "never",
        };
        let links = match config.open_links {
            OpenLinks::Click => "click",
            OpenLinks::CtrlClick => "ctrl_click",
        };
        let default_profile = config
            .default_profile
            .clone()
            .unwrap_or_else(|| "shell".to_owned());

        let theme_name = config.theme.default.clone();
        let row = |page, group, label: &'static str, keywords: &str, control: AnyElement| Row {
            page,
            group,
            label: label.into(),
            keywords: keywords.to_owned(),
            control,
        };
        let mut rows = vec![
            row(
                Page::General,
                "Startup",
                "New tabs open",
                "default profile shell startup",
                self.picker(
                    "pick-default-profile",
                    default_profile,
                    Pick::DefaultProfile,
                    cx,
                )
                .into_any_element(),
            ),
            row(
                Page::General,
                "Startup",
                "Restore the last session",
                "restore session tabs splits reopen relaunch startup",
                self.switch(
                    "restore-session",
                    config.restore_session,
                    |on| Change::Set {
                        path: vec!["restore_session"],
                        value: on.to_string(),
                        reads: |config| config.restore_session.to_string(),
                    },
                    cx,
                )
                .into_any_element(),
            ),
            row(
                Page::General,
                "Startup",
                if cfg!(target_os = "macos") {
                    "Cmd+T opens"
                } else {
                    "Ctrl+Shift+T opens"
                },
                "new tab key shortcut profile ask default picker swap",
                self.options(
                    "new-tab",
                    &[
                        ("ask", "A profile picker"),
                        ("default", "The default profile"),
                    ],
                    new_tab_name(config),
                    |value| {
                        choice(vec!["new_tab"], value, |config| {
                            new_tab_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            row(
                Page::General,
                "Updates",
                "Check GitHub for new releases",
                "update upgrade new version release github check",
                self.options(
                    "update-check",
                    &[("daily", "Check daily"), ("off", "Don't check")],
                    update_check_name(config),
                    |value| {
                        choice(vec!["update_check"], value, |config| {
                            update_check_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .when(config.update_check.is_none(), |options| {
                    options.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Not chosen yet"),
                    )
                })
                .into_any_element(),
            ),
            row(
                Page::General,
                "SFTP",
                "Double-clicking a remote file",
                "sftp remote file click open edit download double",
                self.options(
                    "sftp-click",
                    &[("edit", "Edits it"), ("download", "Downloads it")],
                    sftp_click_name(config),
                    |value| {
                        choice(vec!["sftp_click"], value, |config| {
                            sftp_click_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            row(
                Page::General,
                "SFTP",
                "Edits upload",
                "sftp edit upload save focus manual remote",
                self.options(
                    "sftp-upload",
                    &[
                        ("manual", "On a click"),
                        ("save", "On save"),
                        ("focus", "On focus"),
                    ],
                    sftp_upload_name(config),
                    |value| {
                        choice(vec!["sftp_upload"], value, |config| {
                            sftp_upload_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            row(
                Page::General,
                "Files",
                "Double-clicking a local program",
                "files pane local executable program script run edit ask double click",
                self.options(
                    "files-executable",
                    &[
                        ("ask", "Asks"),
                        ("run", "Runs it"),
                        ("edit", "Opens it as text"),
                    ],
                    files_executable_name(config),
                    |value| {
                        choice(vec!["files_executable"], value, |config| {
                            files_executable_name(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
            row(
                Page::Appearance,
                "Font",
                "Font family",
                "typeface font face",
                self.picker("pick-font", family, Pick::Font, cx)
                    .into_any_element(),
            ),
            row(
                Page::Appearance,
                "Font",
                "Font size",
                "text zoom font",
                self.field(Field::FontSize),
            ),
            row(
                Page::Appearance,
                "Font",
                "Line height",
                "spacing font",
                self.field(Field::LineHeight),
            ),
            row(
                Page::Appearance,
                "Colors",
                "Theme",
                "colors dark light system color scheme",
                self.picker("pick-theme", theme_name, Pick::Theme, cx)
                    .into_any_element(),
            ),
            row(
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
            row(
                Page::Appearance,
                "SFTP",
                "SFTP sidebar",
                "sftp files sidebar panel left right side remote browser",
                self.options(
                    "sftp-sidebar",
                    &[("left", "Left"), ("right", "Right")],
                    sftp_side(config),
                    |value| {
                        choice(vec!["sftp_sidebar"], value, |config| {
                            sftp_side(config).to_owned()
                        })
                    },
                    cx,
                )
                .into_any_element(),
            ),
        ];
        rows.push(row(
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
        rows.push(row(
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
        rows.push(row(
            Page::Terminal,
            "Scrollback",
            "Scrollback lines",
            "history buffer lines scrollback",
            self.field(Field::Scrollback),
        ));
        rows.push(row(
            Page::Terminal,
            "Panes",
            "Close a pane on exit",
            "exit code quit close shell ends",
            self.options(
                "close-on-exit",
                &[
                    ("always", "Always"),
                    ("clean", "On exit code 0"),
                    ("never", "Never"),
                ],
                close,
                |value| {
                    choice(vec!["close_on_exit"], value, |config| {
                        match config.close_on_exit {
                            CloseOnExit::Always => "always",
                            CloseOnExit::Clean => "clean",
                            CloseOnExit::Never => "never",
                        }
                        .to_owned()
                    })
                },
                cx,
            )
            .into_any_element(),
        ));
        rows.push(row(
            Page::Terminal,
            "Mouse",
            "Open links with",
            "url hyperlink link click ctrl browser open",
            self.options(
                "open-links",
                &[
                    ("click", "A click"),
                    (
                        "ctrl_click",
                        if cfg!(target_os = "macos") {
                            "Cmd+click"
                        } else {
                            "Ctrl+click"
                        },
                    ),
                ],
                links,
                |value| {
                    choice(vec!["open_links"], value, |config| {
                        match config.open_links {
                            OpenLinks::Click => "click",
                            OpenLinks::CtrlClick => "ctrl_click",
                        }
                        .to_owned()
                    })
                },
                cx,
            )
            .into_any_element(),
        ));
        rows.push(row(
            Page::Terminal,
            "Mouse",
            "Copy on select",
            "clipboard selection copy mouse",
            self.switch(
                "copy-on-select",
                config.copy_on_select,
                |on| Change::Set {
                    path: vec!["copy_on_select"],
                    value: on.to_string(),
                    reads: |config| config.copy_on_select.to_string(),
                },
                cx,
            )
            .into_any_element(),
        ));
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

    fn keystroke(text: &str) -> Keystroke {
        Keystroke::parse_with_os(text, keymap::OS).unwrap()
    }

    /// Spec 0009, flow 3 and AC 3: a recorded combination is written in
    /// config's spelling.
    /// An import card of `source` reading `file`, in `state`.
    fn card(source: Source, file: Option<&str>, state: CardState) -> ImportCard {
        ImportCard {
            source,
            file: file.map(PathBuf::from),
            state,
            imported: None,
        }
    }

    fn ready(format: &'static str, new: usize, groups: usize) -> CardState {
        CardState::Ready(Outcome {
            format,
            new,
            groups,
            duplicates: 1,
            skipped: 2,
        })
    }

    /// Spec 0014 flow 3: a found file says how many new hosts in how many
    /// groups, with the duplicates and skips, and can be imported.
    #[test]
    fn a_card_with_new_hosts_counts_them_and_offers_import() {
        let (lines, can_import) = card_lines(&card(
            Source::MobaXterm,
            Some("MobaXterm.ini"),
            ready("MobaXterm", 3, 2),
        ));
        assert!(can_import);
        let text = lines.join("\n");
        assert_eq!(lines[0], "3 new hosts in 2 groups.", "{text}");
        assert!(text.contains("1 already there"), "{text}");
        assert!(text.contains("2 skipped"), "{text}");
        assert!(!text.contains("This is a"), "{text}");
    }

    /// Spec 0014 AC3, flow 3: with nothing new the button is disabled and
    /// the card says everything is already in Tern.
    #[test]
    fn a_card_with_nothing_new_disables_import() {
        for source in Source::ALL {
            let (lines, can_import) =
                card_lines(&card(source, Some("f"), ready(source.title(), 0, 0)));
            assert!(!can_import);
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains("Everything is already in Tern")),
                "{lines:?}"
            );
        }
    }

    /// Spec 0014 AC3, flow 4: after an import the card says what it did,
    /// next to the up-to-date state.
    #[test]
    fn after_an_import_the_card_says_how_many_were_imported() {
        let mut done = card(Source::Tabby, Some("config.yaml"), ready("Tabby", 0, 0));
        done.imported = Some(Outcome {
            format: "Tabby",
            new: 4,
            groups: 1,
            duplicates: 0,
            skipped: 0,
        });
        let (lines, can_import) = card_lines(&done);
        assert!(!can_import);
        assert!(
            lines.iter().any(|l| l.contains("Imported 4 hosts")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Everything is already in Tern")),
            "{lines:?}"
        );
    }

    /// Spec 0014 AC6: a Tabby file on the MobaXterm card says it's Tabby.
    #[test]
    fn a_card_says_when_its_file_is_the_other_format() {
        let (lines, can_import) = card_lines(&card(
            Source::MobaXterm,
            Some("config.yaml"),
            ready("Tabby", 2, 1),
        ));
        assert!(can_import);
        assert!(lines.iter().any(|l| l.contains("Tabby file")), "{lines:?}");
    }

    /// Spec 0014 AC5, flow 2: no file at the default place names the
    /// place; MobaXterm where it doesn't run asks for an export instead,
    /// and neither can import.
    #[test]
    fn a_missing_file_names_where_it_was_looked_for() {
        let path = PathBuf::from("somewhere").join("tabby").join("config.yaml");
        let mut missing = card(Source::Tabby, None, CardState::Missing);
        missing.file = Some(path.clone());
        let (lines, can_import) = card_lines(&missing);
        assert!(!can_import);
        assert_eq!(lines, [format!("Not found at {}", path.display())]);

        let (lines, can_import) = card_lines(&card(Source::MobaXterm, None, CardState::Missing));
        assert!(!can_import);
        let text = lines.join("\n");
        assert!(text.contains(".mxtsessions"), "{text}");
        assert!(text.contains("Windows"), "{text}");
    }

    /// Spec 0014 AC7, flow 6: a failure shows its error on one line and
    /// the card can't import; neither can a card still looking.
    #[test]
    fn a_failed_card_shows_its_error_and_cannot_import() {
        let error = "x.ini is over 10 MB";
        let (lines, can_import) = card_lines(&card(
            Source::MobaXterm,
            Some("x.ini"),
            CardState::Failed(error.into()),
        ));
        assert!(!can_import);
        assert_eq!(lines, [error]);
        for state in [CardState::Looking, CardState::Importing] {
            assert!(!card_lines(&card(Source::Tabby, Some("f"), state)).1);
        }
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

    #[test]
    fn a_recorded_combination_is_written_in_config_spelling() {
        assert_eq!(
            config_keys(&keystroke("ctrl-shift-k")).as_deref(),
            Some("ctrl+shift+k")
        );
        assert_eq!(
            config_keys(&keystroke("alt-shift-d")).as_deref(),
            Some("alt+shift+d")
        );
    }

    /// Spec 0009, flow 3: a modifier held alone isn't a combination to
    /// record yet.
    #[test]
    fn a_lone_modifier_is_no_combination() {
        for modifier in ["shift", "control", "alt"] {
            assert_eq!(config_keys(&keystroke(modifier)), None, "{modifier}");
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

    /// Spec 0011, AC10: the Updates field shows `daily` or `off` as set,
    /// and neither while the question is still open.
    #[test]
    fn the_updates_field_shows_neither_choice_while_unset() {
        let name = |update_check| {
            update_check_name(&Config {
                update_check,
                ..Config::default()
            })
        };
        assert_eq!(name(None), "");
        assert_eq!(name(Some(UpdateCheck::Daily)), "daily");
        assert_eq!(name(Some(UpdateCheck::Off)), "off");
        assert!(FIELDS.contains(&"update_check"));
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
