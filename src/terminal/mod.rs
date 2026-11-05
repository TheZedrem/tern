//! The terminal pane (spec 0002): one program in one PTY, drawn by
//! [`element::TerminalElement`] and fed by the keyboard through [`input`].

mod colors;
mod element;
pub mod grid;
pub mod input;
pub mod link;
mod prompt;
pub mod pty;
pub mod search;
pub mod select;

use crate::config::{self, CloseOnExit, Profile, ProfileKind};
use crate::secrets::{self, Key};
use crate::ssh::askpass::{self, Prompt, PromptKind};
use crate::ssh::{self, target::Target};
use crate::theme::{self, Theme};
use crate::timing::Timing;
use alacritty_terminal::event::{Event, Notify, OnResize};
use alacritty_terminal::event_loop::{Msg, Notifier};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::color::{COUNT as COLOR_SLOTS, Colors};
use alacritty_terminal::term::search::{Match, RegexSearch};
use alacritty_terminal::term::{Config as TermConfig, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor, StdSyncHandler};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::oneshot;
use futures::future::{Either, select};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, BackgroundExecutor, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NoAction, ParentElement, Pixels, Point, Render, ScrollEvent,
    SharedString, Size, StatefulInteractiveElement, Styled, Subscription, Task, Window, actions,
    div, font, px, size,
};
use gpui::{Bounds, EntityInputHandler, UTF16Selection};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Selectable, Sizable};
use prompt::{Answer, PromptDialog, Remember};
use pty::{EventProxy, GridSize, Terminal};
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

/// Longest tab title taken from a program (spec 0003).
const MAX_TITLE_CHARS: usize = 200;
/// The most of a prompt a dialog shows (TM-askpass-08).
const MAX_PROMPT_BYTES: usize = 4096;
/// How long a keychain call may take, an unlock prompt included, before it
/// counts as nothing saved (TM-askpass-17).
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `ssh -G` may take before prompts are answered without it, so
/// nothing is filled in.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

/// The embedded default font (spec 0005).
pub const DEFAULT_FONT_FAMILY: &str = "Lilex";

/// A pane's key context. Its bindings outrank those of the widget
/// library's `Root`, which wraps every window.
const KEY_CONTEXT: &str = "Terminal";
/// The search bar's key context.
const SEARCH_CONTEXT: &str = "TerminalSearch";
/// How long the cursor stays on, then off, while it blinks.
const BLINK: Duration = Duration::from_millis(530);

actions!(tern, [CloseSearch]);

const LILEX: [&[u8]; 4] = [
    include_bytes!("../../assets/fonts/lilex/Lilex-Regular.ttf"),
    include_bytes!("../../assets/fonts/lilex/Lilex-Bold.ttf"),
    include_bytes!("../../assets/fonts/lilex/Lilex-Italic.ttf"),
    include_bytes!("../../assets/fonts/lilex/Lilex-BoldItalic.ttf"),
];

/// Registers the embedded font and the pane's key bindings. Runs once at
/// startup, before the first window opens.
pub fn init(cx: &mut App) -> Result<(), String> {
    // `Root` binds these above every pane (focus cycling, copy). `NoAction`
    // in the pane's own context outranks them, so the keys reach the shell.
    // Re-check `Root`'s bindings on every gpui-component bump.
    cx.bind_keys([
        KeyBinding::new("tab", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-c", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CloseSearch, Some(SEARCH_CONTEXT)),
    ]);
    prompt::init(cx);
    cx.text_system()
        .add_fonts(LILEX.iter().map(|face| Cow::Borrowed(*face)).collect())
        .map_err(|e| format!("{e:#}"))
}

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

/// What a pane takes from the config. A reload replaces it while the pane
/// runs (spec 0005, flow 3).
#[derive(Clone, Debug, PartialEq)]
pub struct PaneSettings {
    pub font: FontSettings,
    pub theme: Theme,
    pub close_on_exit: CloseOnExit,
    pub scrollback: usize,
    pub copy_on_select: bool,
    pub cursor: config::Cursor,
    pub open_links: config::OpenLinks,
}

impl PaneSettings {
    /// What the terminal takes from these: scrollback and the cursor a
    /// program starts with; OSC 52 stays off (TM-escape-04, -05).
    fn term_config(&self) -> TermConfig {
        let shape = match self.cursor.style {
            config::CursorStyle::Block => CursorShape::Block,
            config::CursorStyle::Beam => CursorShape::Beam,
            config::CursorStyle::Underline => CursorShape::Underline,
        };
        TermConfig {
            scrolling_history: self.scrollback,
            default_cursor_style: CursorStyle {
                shape,
                blinking: self.cursor.blink,
            },
            osc52: Osc52::Disabled,
            ..TermConfig::default()
        }
    }
}

/// The profile a pane was started from, as it was then: profile edits
/// apply to new panes only (spec 0005).
#[derive(Clone, Debug, PartialEq)]
pub struct PaneProfile {
    /// For splits, which open the same profile (spec 0003).
    pub id: String,
    /// The tab title until the program sets one.
    pub name: String,
    /// The pane's theme; `None` follows `theme.default`.
    pub theme: Option<String>,
    /// An SSH connection (spec 0006): its prompts are answered in the app,
    /// and when it drops it waits for Enter to reconnect.
    pub ssh: bool,
}

impl From<&Profile> for PaneProfile {
    fn from(profile: &Profile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.display_name().to_owned(),
            theme: profile.theme.clone(),
            ssh: matches!(profile.kind, ProfileKind::Ssh(_)),
        }
    }
}

/// What starts a pane's program: at first, at Enter in a waiting pane, and
/// again when a dropped connection reconnects.
pub struct Launch {
    pub command: pty::Command,
    pub cwd: Option<PathBuf>,
    pub timing: Arc<Timing>,
}

/// How a pane begins.
pub enum Start {
    /// With its program started before the window existed, or the reason it
    /// couldn't start: the boot pane (spec 0001).
    Started(Result<pty::Session, String>),
    /// Starting its program now, in a grid of this size.
    Now(GridSize),
    /// Showing this until Enter starts the program: an SSH tab opened at
    /// boot or restored never connects by itself (spec 0006, flow 6).
    OnEnter(String),
}

/// What a waiting SSH pane shows (spec 0006, flow 6).
pub fn connect_prompt(label: &str) -> String {
    format!("Press Enter to connect to {label}")
}

/// Why `command` didn't start, as its pane says it. A missing `ssh` comes
/// with how to install it (spec 0006, flow 8).
fn failure(command: &pty::Command, error: &str) -> String {
    if command.program == ssh::PROGRAM && !ssh::installed() {
        return ssh::not_installed();
    }
    format!("Could not start {}: {error}", command.describe())
}

/// The notice written into the grid when an SSH connection drops.
const RECONNECT_NOTICE: &str = "\r\n\x1b[2m[connection closed: press Enter to reconnect]\x1b[0m";

/// What a pane tells its owner.
pub enum PaneEvent {
    /// The first frame with the pane's output, or its spawn error, was
    /// drawn. This is the TTFP moment (spec 0001).
    FirstFrame,
    /// The user clicked into the pane.
    Focused,
    /// The program set or reset its title.
    TitleChanged,
    /// The program exited and `close_on_exit` says the pane closes.
    Exited,
    /// A prompt dialog opened or closed, which moves the keyboard focus.
    DialogChanged,
    /// Something for a notice, like a password filled in from the keychain.
    Notice(String),
    /// There is no keychain; the workspace says so once (spec 0006, flow 8).
    NoKeychain,
}

/// A program's title as a tab shows it: control characters dropped, runs
/// of whitespace collapsed, at most 200 characters (spec 0003).
pub fn clean_title(title: &str) -> String {
    title
        .split(|c: char| c.is_whitespace() || c.is_control() || is_invisible_format(c))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

/// ssh's prompt as a dialog shows it (spec 0006, flow 4): the title rules,
/// but with line breaks kept and tabs as spaces, capped at 4 KiB.
/// Host-key questions span several lines.
pub fn clean_prompt(prompt: &str) -> String {
    let mut cleaned = String::new();
    for c in prompt.chars() {
        let c = match c {
            '\n' => '\n',
            '\t' => ' ',
            c if c.is_control() || is_invisible_format(c) => continue,
            c => c,
        };
        if cleaned.len() + c.len_utf8() > MAX_PROMPT_BYTES {
            break;
        }
        cleaned.push(c);
    }
    cleaned
}

/// Invisible characters that could hide text or reorder what a tab shows:
/// zero-width space, direction marks and overrides, invisible operators,
/// byte-order mark, tag characters. They are treated like controls. The
/// zero-width joiner and non-joiner stay: emoji sequences and several
/// scripts need them.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// What goes back to the program for `event`: replies to its queries,
/// which are fixed strings and numbers only (TM-escape-06). Clipboard
/// writes and reads (OSC 52) get nothing [load-bearing] (TM-escape-04,
/// -05), and neither does anything else.
fn reply(event: Event, colors: &Colors, theme: &Theme, size: GridSize) -> Option<String> {
    match event {
        // Device attributes, status, cursor position, modes.
        Event::PtyWrite(text) => Some(text),
        Event::ColorRequest(slot, format) if slot < COLOR_SLOTS => {
            let color = colors[slot].unwrap_or_else(|| colors::default_for_slot(slot, theme));
            Some(format(color))
        }
        Event::TextAreaSizeRequest(format) => Some(format(size.window_size())),
        _ => None,
    }
}

/// Whether a pane closes after its program exited with `code` (`None`:
/// killed by a signal) under `policy` (spec 0002, flow 8).
pub fn closes_on_exit(policy: CloseOnExit, code: Option<i32>) -> bool {
    match policy {
        CloseOnExit::Always => true,
        CloseOnExit::Clean => code == Some(0),
        CloseOnExit::Never => false,
    }
}

/// What a pane does once its program exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterExit {
    Close,
    /// Shows the exit code and stays.
    Stay,
    /// Says the connection closed and waits for Enter to reconnect.
    Reconnect,
}

/// What a pane does after its program exited with `code`. A dropped SSH
/// connection (exit 255) stays open whatever `policy` says (spec 0006,
/// flow 5); everything else follows `policy`.
pub fn after_exit(reconnects: bool, policy: CloseOnExit, code: Option<i32>) -> AfterExit {
    if reconnects && code == Some(ssh::CONNECTION_LOST) {
        AfterExit::Reconnect
    } else if closes_on_exit(policy, code) {
        AfterExit::Close
    } else {
        AfterExit::Stay
    }
}

/// The notice written into the grid when the program exits.
pub fn exit_notice(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("\r\n\x1b[2m[process exited with code {code}]\x1b[0m"),
        None => "\r\n\x1b[2m[process was killed by a signal]\x1b[0m".to_owned(),
    }
}

enum State {
    Running(Running),
    /// The program waits for Enter; the pane shows this.
    Waiting(String),
    /// The program couldn't start; the pane shows why.
    Failed(String),
}

struct Running {
    term: Terminal,
    /// What feeds the pane's events, kept for a program started again.
    proxy: EventProxy,
    notifier: Notifier,
    size: GridSize,
    child_pid: Option<u32>,
}

pub struct TerminalPane {
    focus: FocusHandle,
    state: State,
    launch: Launch,
    profile: PaneProfile,
    settings: PaneSettings,
    /// Bench panes never close on exit; the bench quits after the first frame.
    bench: bool,
    exited: bool,
    /// The SSH connection dropped and Enter reconnects (spec 0006, flow 5).
    reconnect: bool,
    has_output: bool,
    first_frame_scheduled: bool,
    /// The title the program set (OSC 0/2), already cleaned.
    title: Option<String>,
    /// Answers the running SSH connection's prompts (spec 0006, flow 4).
    /// Dropping it ends them, and their helpers exit 1.
    askpass: Option<Task<()>>,
    /// The dialog over the pane while ssh waits on a prompt.
    dialog: Option<Entity<PromptDialog>>,
    /// Where the grid was drawn last, its origin and cell size, for
    /// turning mouse positions into cells.
    geometry: Option<(Point<Pixels>, Size<Pixels>)>,
    /// A selection is being dragged out with the left button.
    selecting: bool,
    /// Wheel movement short of a whole line, carried to the next event.
    scroll_remainder: f32,
    search: Option<SearchBar>,
    /// Finds URLs in the output, built on first hover.
    urls: Option<RegexSearch>,
    /// The link under the pointer, underlined, its target shown.
    hovered_link: Option<link::Link>,
    /// A link a plain click pressed, and the cell it pressed: it opens if
    /// the button comes up there (`open_links: click`).
    pressed_link: Option<(link::Link, alacritty_terminal::index::Point)>,
    /// What an input method is composing, shown at the cursor until it's
    /// committed (spec 0002, flow 2).
    preedit: Option<String>,
    /// The cursor is in the visible half of its blink.
    cursor_shown: bool,
    /// Blinks the cursor while the pane has the keyboard.
    blink: Option<Task<()>>,
}

/// The search bar while it's open (spec 0002, flow 6).
struct SearchBar {
    input: Entity<InputState>,
    case_sensitive: bool,
    regex: bool,
    /// The query, compiled; `None` while it's empty or not a valid regex.
    compiled: Option<RegexSearch>,
    /// The match the view is on.
    current: Option<Match>,
    _events: Subscription,
}

impl EventEmitter<PaneEvent> for TerminalPane {}

impl TerminalPane {
    /// A pane that begins as `start` says, showing the error if the program
    /// couldn't start (spec 0002, flow 9).
    pub fn new(
        start: Start,
        launch: Launch,
        profile: PaneProfile,
        settings: PaneSettings,
        bench: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            focus: cx.focus_handle(),
            state: State::Failed(String::new()),
            launch,
            profile,
            settings,
            bench,
            exited: false,
            reconnect: false,
            has_output: false,
            first_frame_scheduled: false,
            title: None,
            askpass: None,
            dialog: None,
            geometry: None,
            selecting: false,
            scroll_remainder: 0.0,
            search: None,
            urls: None,
            hovered_link: None,
            pressed_link: None,
            preedit: None,
            cursor_shown: true,
            blink: None,
        };
        match start {
            Start::Started(session) => pane.attach(session, cx),
            Start::Now(grid) => pane.spawn(grid, window, cx),
            Start::OnEnter(prompt) => pane.state = State::Waiting(prompt),
        }
        pane
    }

    /// Shows `session`'s program, or why it couldn't start, and forwards its
    /// events to the pane.
    fn attach(&mut self, session: Result<pty::Session, String>, cx: &mut Context<Self>) {
        let session = match session {
            Ok(session) => session,
            Err(error) => {
                let message = failure(&self.launch.command, &error);
                self.notice(message.clone(), cx);
                self.state = State::Failed(message);
                self.end_prompts(cx);
                return;
            }
        };
        let mut events = session.events;
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let mut batch = vec![event];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                let alive = this.update(cx, |pane, cx| {
                    for event in batch {
                        pane.handle_event(event, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        // The cursor the config asks for, from the first frame on.
        session.term.lock().set_options(self.settings.term_config());
        self.state = State::Running(Running {
            term: session.term,
            proxy: session.proxy,
            notifier: session.notifier,
            size: session.size,
            child_pid: session.child_pid,
        });
    }

    /// Starts the program of a waiting pane, sized to the window until the
    /// next frame measures the pane.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.settings.font.estimated_cell(),
        );
        self.spawn(grid, window, cx);
        cx.notify();
    }

    /// Starts the program in a new PTY of `grid`.
    fn spawn(&mut self, grid: GridSize, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.connection_command(window, cx);
        let session = pty::spawn(
            &command,
            grid,
            self.launch.cwd.as_deref(),
            self.settings.scrollback,
            Arc::clone(&self.launch.timing),
        );
        self.attach(session, cx);
    }

    /// The command to start now. An SSH connection's gets the askpass
    /// environment of a new token, and the pane answers its prompts until
    /// the next start (spec 0006, flow 2). Without an askpass server, ssh
    /// asks in the terminal, with a notice saying why.
    fn connection_command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> pty::Command {
        let mut command = self.launch.command.clone();
        self.end_prompts(cx);
        if self.profile.ssh {
            match askpass::connect(cx) {
                Ok(connection) => {
                    command.env.extend(connection.env);
                    let args = command.args.clone();
                    self.askpass = Some(self.answer_prompts(connection.prompts, args, window, cx));
                }
                Err(error) => self.notice(format!("ssh asks in the terminal: {error}"), cx),
            }
        }
        command
    }

    /// Shows `notice` as a toast. Deferred, so one raised while the pane is
    /// being created reaches the workspace's subscription.
    fn notice(&self, notice: String, cx: &mut Context<Self>) {
        let pane = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = pane.update(cx, |_, cx| cx.emit(PaneEvent::Notice(notice)));
        });
    }

    /// Answers the prompts of the connection whose ssh runs with `args`, one
    /// at a time (spec 0006, flow 4). The connection's own password and
    /// passphrase prompts are filled in from the keychain once each, with a
    /// notice; everything else gets a dialog.
    fn answer_prompts(
        &self,
        mut prompts: UnboundedReceiver<Prompt>,
        args: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let executor = cx.background_executor().clone();
        let target = secrets::off_thread(move || Target::resolve(&args));
        cx.spawn_in(window, async move |this, cx| {
            let target = within(target, RESOLVE_TIMEOUT, &executor)
                .await
                .and_then(Result::ok)
                .flatten();
            let mut offers = Offers::default();
            while let Some(mut prompt) = prompts.next().await {
                let key = target
                    .as_ref()
                    .filter(|_| prompt.kind == PromptKind::Secret)
                    .and_then(|target| target.secret_for(&prompt.text));
                if let Some(key) = key.as_ref().filter(|key| offers.ask(key)) {
                    match lookup(key, &executor).await {
                        Lookup::Found(secret) => {
                            prompt.answer(&secret);
                            offers.filled.insert(key.clone());
                            let notice = filled_notice(key);
                            let _ = this.update(cx, |_, cx| cx.emit(PaneEvent::Notice(notice)));
                            continue;
                        }
                        Lookup::Nothing => {}
                        Lookup::Failed(error) => {
                            let notice = format!("Couldn't read the keychain: {error}");
                            let _ = this.update(cx, |_, cx| cx.emit(PaneEvent::Notice(notice)));
                        }
                        Lookup::NoKeychain => {
                            offers.no_keychain = true;
                            let _ = this.update(cx, |_, cx| cx.emit(PaneEvent::NoKeychain));
                        }
                    }
                }
                let remember = offers.remember(key.clone());
                let Ok(answered) = this.update_in(cx, |pane, window, cx| {
                    pane.open_dialog(&prompt, remember, window, cx)
                }) else {
                    break;
                };
                let answer = match select(answered, pin!(prompt.ended())).await {
                    Either::Left((Ok(answer), _)) => Some(answer),
                    _ => None,
                };
                let _ = this.update(cx, |pane, cx| pane.close_dialog(cx));
                match answer {
                    Some(Answer::Typed { text, save }) => {
                        prompt.answer(&text);
                        if let Some(key) = key.filter(|_| save) {
                            let _ = this.update(cx, |pane, cx| pane.save(key, text, cx));
                        }
                    }
                    Some(Answer::Yes) => prompt.answer("yes"),
                    // Declined, or ssh is done with it: dropping it ends the
                    // helper.
                    None => {}
                }
            }
        })
    }

    /// Opens the dialog for `prompt` over the pane. The receiver gets the
    /// answer, or fails when the dialog is left without one.
    fn open_dialog(
        &mut self,
        prompt: &Prompt,
        remember: Option<Remember>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Answer> {
        let (answer, answered) = oneshot::channel();
        let connection = SharedString::from(self.profile.name.clone());
        self.dialog = Some(cx.new(|cx| {
            PromptDialog::new(
                connection,
                &prompt.text,
                prompt.kind,
                remember,
                answer,
                window,
                cx,
            )
        }));
        cx.emit(PaneEvent::DialogChanged);
        cx.notify();
        answered
    }

    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        if self.dialog.take().is_some() {
            cx.emit(PaneEvent::DialogChanged);
            cx.notify();
        }
    }

    /// Ends the connection's prompts; helpers still waiting exit 1.
    fn end_prompts(&mut self, cx: &mut Context<Self>) {
        self.askpass = None;
        self.close_dialog(cx);
    }

    /// Saves what was typed under `key`, off the UI thread. Only a failure
    /// makes a notice.
    fn save(&self, key: Key, secret: Zeroizing<String>, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let described = key.describe();
        let saving = secrets::off_thread(move || secrets::set(&key, &secret));
        cx.spawn(async move |this, cx| {
            let error = match within(saving, KEYCHAIN_TIMEOUT, &executor).await {
                Some(Ok(Ok(()))) => return,
                Some(Ok(Err(error))) => error,
                _ => "the keychain didn't answer".to_owned(),
            };
            let notice = format!("Couldn't save {described}: {error}");
            let _ = this.update(cx, |_, cx| cx.emit(PaneEvent::Notice(notice)));
        })
        .detach();
    }

    /// Text where the grid would be, in the pane's font: a waiting pane's
    /// prompt, or why its program couldn't start.
    fn message(&self, text: &str, color: u32) -> gpui::Div {
        div()
            .p(px(pty::PADDING))
            .font_family(self.settings.font.family.clone())
            .text_size(self.settings.font.size)
            .text_color(theme::hsla(color))
            .child(text.to_owned())
    }

    /// Starts the program again in the same terminal, after its SSH
    /// connection dropped: the old session stays in the scrollback. The new
    /// connection gets a token of its own.
    fn rerun(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state, State::Running(_)) {
            return;
        }
        let command = self.connection_command(window, cx);
        let State::Running(running) = &mut self.state else {
            return;
        };
        let started = pty::rerun(
            &command,
            running.size,
            self.launch.cwd.as_deref(),
            &running.term,
            &running.proxy,
        );
        let failed = match started {
            Ok((notifier, child_pid)) => {
                running.notifier = notifier;
                running.child_pid = child_pid;
                self.exited = false;
                self.reconnect = false;
                false
            }
            Err(error) => {
                let notice = format!("\r\n{}", failure(&command, &error));
                let mut parser = Processor::<StdSyncHandler>::new();
                parser.advance(&mut *running.term.lock(), notice.as_bytes());
                true
            }
        };
        if failed {
            self.end_prompts(cx);
        }
        cx.notify();
    }

    /// What takes the keyboard when the pane is focused: its prompt dialog
    /// while one is open, else the grid.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.dialog {
            Some(dialog) => dialog.read(cx).focus_target(cx),
            None => self.focus.clone(),
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Shows `title`, by the title rules, until the program sets its own: a
    /// restored pane's last one (spec 0003).
    pub fn set_title(&mut self, title: Option<&str>) {
        self.title = title.map(clean_title).filter(|title| !title.is_empty());
    }

    pub fn profile(&self) -> &PaneProfile {
        &self.profile
    }

    /// Takes the settings of a reloaded config: font, theme and scrollback
    /// apply at once, `close_on_exit` at the next exit.
    pub fn apply(&mut self, settings: PaneSettings, cx: &mut Context<Self>) {
        if let State::Running(running) = &self.state
            && (settings.scrollback != self.settings.scrollback
                || settings.cursor != self.settings.cursor)
        {
            running.term.lock().set_options(settings.term_config());
        }
        self.settings = settings;
        self.blink = None;
        cx.notify();
    }

    /// The program's working directory, so a split starts in the same place
    /// (spec 0003, flow 1). Read from `/proc` on Linux; elsewhere unknown.
    // TODO(OQ-018): prefer the cwd the shell reports with OSC 7 (T-022).
    pub fn cwd(&self) -> Option<PathBuf> {
        let State::Running(running) = &self.state else {
            return None;
        };
        let pid = running.child_pid?;
        if cfg!(target_os = "linux") {
            std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
        } else {
            None
        }
    }

    /// Stops the program: the event loop exits, which hangs up the PTY.
    pub fn shutdown(&mut self) {
        if let State::Running(running) = &self.state {
            // An error means the event loop already ended with the program.
            let _ = running.notifier.0.send(Msg::Shutdown);
        }
    }

    fn handle_event(&mut self, event: Event, cx: &mut Context<Self>) {
        if let Event::ChildExit(status) = event {
            self.child_exited(status.code(), cx);
            return;
        }
        let State::Running(running) = &mut self.state else {
            return;
        };
        match event {
            Event::Wakeup => {
                self.has_output = true;
                cx.notify();
            }
            Event::Title(title) => {
                self.title = Some(clean_title(&title)).filter(|title| !title.is_empty());
                cx.emit(PaneEvent::TitleChanged);
            }
            Event::ResetTitle => {
                self.title = None;
                cx.emit(PaneEvent::TitleChanged);
            }
            event => {
                let reply = {
                    let term = running.term.lock();
                    reply(event, term.colors(), &self.settings.theme, running.size)
                };
                if let Some(reply) = reply {
                    running.notifier.notify(reply.into_bytes());
                }
            }
        }
    }

    fn child_exited(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        self.exited = true;
        // The connection's token dies with its ssh (TM-askpass-03).
        self.end_prompts(cx);
        let after = after_exit(self.profile.ssh, self.settings.close_on_exit, code);
        self.reconnect = after == AfterExit::Reconnect;
        let notice = if self.reconnect {
            RECONNECT_NOTICE.to_owned()
        } else {
            exit_notice(code)
        };
        if let State::Running(running) = &self.state {
            let mut parser = Processor::<StdSyncHandler>::new();
            parser.advance(&mut *running.term.lock(), notice.as_bytes());
        }
        cx.notify();
        if !self.bench && after == AfterExit::Close {
            cx.emit(PaneEvent::Exited);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // While ssh waits on a dialog the grid takes no keys: a password
        // typed into the PTY would reach the remote shell after login.
        if self.dialog.is_some() {
            return;
        }
        let enter = event.keystroke.key == "enter" && !event.keystroke.modifiers.modified();
        let running = match &self.state {
            State::Waiting(_) => {
                if enter {
                    self.start(window, cx);
                }
                cx.stop_propagation();
                return;
            }
            State::Failed(_) => return,
            State::Running(running) => running,
        };
        if self.exited {
            if enter && self.reconnect {
                self.rerun(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let paging = match keystroke.key.as_str() {
            "pageup" => Some(Scroll::PageUp),
            "pagedown" => Some(Scroll::PageDown),
            _ => None,
        };
        // Shift+PageUp/PageDown page through the scrollback (spec 0002,
        // flow 5); without Shift the keys go to the program.
        if let Some(page) =
            paging.filter(|_| modifiers.shift && modifiers.number_of_modifiers() == 1)
        {
            running.term.lock().scroll_display(page);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let bytes = {
            let mut term = running.term.lock();
            let modes = input::KeyModes {
                app_cursor: term.mode().contains(TermMode::APP_CURSOR),
            };
            let Some(bytes) = input::to_bytes(&event.keystroke, modes) else {
                return;
            };
            // Typing snaps the view back to the bottom (spec 0002, flow 5).
            term.scroll_display(Scroll::Bottom);
            bytes
        };
        running.notifier.notify(bytes);
        self.keep_cursor_shown();
        cx.stop_propagation();
    }

    /// Typing shows the cursor and starts its blink over.
    fn keep_cursor_shown(&mut self) {
        self.cursor_shown = true;
        self.blink = None;
    }

    /// Blinks the cursor while the pane has the keyboard and its style
    /// blinks, by `cursor.blink` or by the program's choice.
    fn blink(&mut self, blinking: bool, cx: &mut Context<Self>) {
        if !blinking {
            self.blink = None;
            self.cursor_shown = true;
            return;
        }
        if self.blink.is_some() {
            return;
        }
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let blinked = this.update(cx, |pane, cx| {
                    pane.cursor_shown = !pane.cursor_shown;
                    cx.notify();
                });
                if blinked.is_err() {
                    break;
                }
            }
        }));
    }

    /// Writes what an input method committed to the program, as is.
    fn type_text(&mut self, text: &str) {
        // As for keys: nothing reaches a program waiting on a dialog.
        if self.exited || self.dialog.is_some() {
            return;
        }
        let State::Running(running) = &self.state else {
            return;
        };
        running.term.lock().scroll_display(Scroll::Bottom);
        running.notifier.notify(text.as_bytes().to_vec());
        self.keep_cursor_shown();
    }

    /// Where the cursor's cell is on screen, for the input method's
    /// candidate window.
    fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let (origin, cell) = self.geometry?;
        let State::Running(running) = &self.state else {
            return None;
        };
        let term = running.term.lock();
        let point = term.grid().cursor.point;
        let row = point.line.0 + term.grid().display_offset() as i32;
        let at = origin
            + gpui::point(
                px(pty::PADDING) + cell.width * point.column.0 as f32,
                px(pty::PADDING) + cell.height * row.max(0) as f32,
            );
        Some(Bounds::new(at, cell))
    }

    /// Takes the grid's place on screen from the element: its origin and cell
    /// size, and the grid that fits, which resizes the terminal and the PTY
    /// when it changed.
    fn lay_out(&mut self, origin: Point<Pixels>, cell: Size<Pixels>, size: GridSize) {
        self.geometry = Some((origin, cell));
        let State::Running(running) = &mut self.state else {
            return;
        };
        if running.size == size {
            return;
        }
        running.size = size;
        running.term.lock().resize(size);
        running.notifier.on_resize(size.window_size());
    }

    /// The cell under `position` in `term`, and which half of it.
    fn cell_at<T>(
        &self,
        position: Point<Pixels>,
        term: &Term<T>,
        size: GridSize,
    ) -> Option<(
        alacritty_terminal::index::Point,
        alacritty_terminal::index::Side,
    )> {
        let (origin, cell) = self.geometry?;
        Some(select::cell_at(
            (f32::from(position.x), f32::from(position.y)),
            (f32::from(origin.x), f32::from(origin.y)),
            (f32::from(cell.width), f32::from(cell.height)),
            size,
            term.grid().display_offset(),
        ))
    }

    /// Starts a selection under the left button: dragging selects from
    /// there, a double click a word, a triple click a line (spec 0002,
    /// flow 4). Over a link, a click opens it as `open_links` says (flow
    /// 7).
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ctrl = if cfg!(target_os = "macos") {
            event.modifiers.platform
        } else {
            event.modifiers.control
        };
        let others = event.modifiers.number_of_modifiers() > u8::from(ctrl);
        let focused = self.focus.is_focused(window);
        let press = link::press(
            self.settings.open_links,
            ctrl,
            others,
            event.click_count,
            focused,
        );
        let mut armed = None;
        if press != link::Press::Select
            && let Some(link) = self.link_under(event.position)
        {
            if press == link::Press::Open {
                // GPUI's URL opener, never a shell (TM-escape-03).
                cx.open_url(&link.target);
                return;
            }
            armed = Some(link);
        }
        let State::Running(running) = &self.state else {
            return;
        };
        let mut term = running.term.lock();
        let Some((point, side)) = self.cell_at(event.position, &term, running.size) else {
            return;
        };
        let kind = match event.click_count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(kind, point, side));
        drop(term);
        self.pressed_link = armed.map(|link| (link, point));
        self.selecting = true;
        cx.notify();
    }

    /// Drags a selection out, or with no button down, tracks the link under
    /// the pointer.
    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button.is_none() {
            let link = self.link_under(event.position);
            if self.hovered_link != link {
                self.hovered_link = link;
                cx.notify();
            }
            return;
        }
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let State::Running(running) = &self.state else {
            return;
        };
        let mut term = running.term.lock();
        let Some((point, side)) = self.cell_at(event.position, &term, running.size) else {
            return;
        };
        if let Some(selection) = &mut term.selection {
            selection.update(point, side);
        }
        drop(term);
        // Leaving the pressed cell makes the click a drag, which selects.
        if self
            .pressed_link
            .as_ref()
            .is_some_and(|(_, pressed)| *pressed != point)
        {
            self.pressed_link = None;
        }
        cx.notify();
    }

    /// The link Tern would open under `position` (spec 0002, flow 7).
    fn link_under(&mut self, position: Point<Pixels>) -> Option<link::Link> {
        let State::Running(running) = &self.state else {
            return None;
        };
        let term = running.term.lock();
        let (point, _) = self.cell_at(position, &term, running.size)?;
        let urls = self.urls.get_or_insert_with(link::detector);
        link::at(&term, point, urls)
    }

    /// The pointer left the pane: no link is hovered.
    fn hover_ended(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if !hovered && self.hovered_link.take().is_some() {
            cx.notify();
        }
    }

    /// Ends a selection: a click that selected nothing clears it; anything
    /// selected becomes the primary selection on Linux, and the clipboard
    /// with `copy_on_select`.
    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.selecting) {
            return;
        }
        // A click on a link, with `open_links: click` (spec 0002, flow 7).
        if let Some((link, _)) = self.pressed_link.take() {
            if let State::Running(running) = &self.state {
                running.term.lock().selection = None;
            }
            cx.open_url(&link.target);
            cx.notify();
            return;
        }
        let Some(text) = self.selected_text() else {
            if let State::Running(running) = &self.state {
                running.term.lock().selection = None;
            }
            cx.notify();
            return;
        };
        #[cfg(target_os = "linux")]
        cx.write_to_primary(ClipboardItem::new_string(text.clone()));
        if self.settings.copy_on_select {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// The button came up outside the pane: that was a drag, never a click
    /// on a link.
    fn mouse_up_out(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.pressed_link = None;
        self.mouse_up(event, window, cx);
    }

    /// Middle-click pastes the primary selection (Linux).
    fn middle_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if let Some(text) = cx.read_from_primary().and_then(|item| item.text()) {
            self.paste_text(&text);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = cx;
    }

    /// The selection as copy puts it on the clipboard, if anything is
    /// selected.
    fn selected_text(&self) -> Option<String> {
        let State::Running(running) = &self.state else {
            return None;
        };
        let text = running.term.lock().selection_to_string()?;
        Some(select::copy_text(&text)).filter(|text| !text.is_empty())
    }

    /// Whether anything is selected, so "Copy" has something to copy.
    pub fn has_selection(&self) -> bool {
        self.selected_text().is_some()
    }

    /// Copies the selection to the clipboard (spec 0002, flow 4).
    pub fn copy(&self, cx: &mut App) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Pastes the clipboard into the program (spec 0002, flow 4).
    pub fn paste(&self, cx: &mut App) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text);
        }
    }

    /// Sends `text` to the program as a paste (TM-escape-07). Nothing
    /// reaches a program that has exited, or one waiting on a prompt
    /// dialog: a pasted password would go to the remote shell.
    fn paste_text(&self, text: &str) {
        if self.exited || self.dialog.is_some() {
            return;
        }
        let State::Running(running) = &self.state else {
            return;
        };
        let bytes = {
            let mut term = running.term.lock();
            let bracketed = term.mode().contains(TermMode::BRACKETED_PASTE);
            term.scroll_display(Scroll::Bottom);
            select::paste_bytes(text, bracketed)
        };
        running.notifier.notify(bytes);
    }

    /// Scrolls through the scrollback, a line per wheel notch (spec 0002,
    /// flow 5). Where a full-screen program (less, vim, man) asked for it,
    /// the wheel sends arrow keys instead, so it scrolls itself.
    fn scroll_wheel(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((_, cell)) = self.geometry else {
            return;
        };
        let State::Running(running) = &self.state else {
            return;
        };
        let lines = f32::from(event.delta.pixel_delta(cell.height).y) / f32::from(cell.height)
            + self.scroll_remainder;
        self.scroll_remainder = lines.fract();
        let lines = lines.trunc() as i32;
        if lines == 0 {
            return;
        }
        let mut term = running.term.lock();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let key = match (lines > 0, app_cursor) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            drop(term);
            let keys = key.repeat(lines.unsigned_abs() as usize);
            running.notifier.notify(keys.into_bytes());
        } else {
            term.scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// Opens the search bar, or selects its query if it's open (spec 0002,
    /// flow 6).
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state, State::Running(_)) {
            return;
        }
        if let Some(bar) = &self.search {
            bar.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let events = cx.subscribe_in(&input, window, |pane, _, event: &InputEvent, _, cx| {
            match event {
                InputEvent::Change => pane.search_changed(cx),
                // Enter goes up to older output, Shift+Enter back down.
                InputEvent::PressEnter { shift, .. } => pane.search_step(!shift, cx),
                _ => {}
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.search = Some(SearchBar {
            input,
            case_sensitive: false,
            regex: false,
            compiled: None,
            current: None,
            _events: events,
        });
        cx.notify();
    }

    /// Closes the search bar; the keyboard goes back to the program.
    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Searches anew for the query as it is now, from the newest output up.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let Some(bar) = &mut self.search else {
            return;
        };
        let query = bar.input.read(cx).value().to_string();
        bar.compiled = search::compile(&query, bar.case_sensitive, bar.regex);
        bar.current = None;
        self.search_step(true, cx);
    }

    /// Moves to the next match, up to older output or down to newer, and
    /// scrolls it into view.
    fn search_step(&mut self, up: bool, cx: &mut Context<Self>) {
        let (Some(bar), State::Running(running)) = (&mut self.search, &self.state) else {
            return;
        };
        let Some(compiled) = &mut bar.compiled else {
            cx.notify();
            return;
        };
        let mut term = running.term.lock();
        bar.current = search::step(&term, compiled, bar.current.as_ref(), up);
        if let Some(found) = &bar.current {
            term.scroll_to_point(*found.start());
        }
        cx.notify();
    }

    fn toggle_search_option(&mut self, regex: bool, cx: &mut Context<Self>) {
        if let Some(bar) = &mut self.search {
            if regex {
                bar.regex = !bar.regex;
            } else {
                bar.case_sensitive = !bar.case_sensitive;
            }
            self.search_changed(cx);
        }
    }

    /// The search highlights on screen now.
    fn marks(&mut self) -> grid::Marks {
        let link = self.hovered_link.as_ref().map(|link| link.range.clone());
        let (Some(bar), State::Running(running)) = (&mut self.search, &self.state) else {
            return grid::Marks {
                link,
                ..Default::default()
            };
        };
        let Some(compiled) = &mut bar.compiled else {
            return grid::Marks {
                link,
                ..Default::default()
            };
        };
        let term = running.term.lock();
        grid::Marks {
            matches: search::on_screen(&term, compiled),
            current: bar.current.clone(),
            link,
            ..Default::default()
        }
    }

    /// Where the hovered link goes, over the pane's bottom left corner
    /// (TM-escape-02).
    fn render_link_hint(&self, cx: &App) -> Option<impl IntoElement> {
        let link = self.hovered_link.as_ref()?;
        let theme = cx.theme();
        Some(
            div()
                .absolute()
                .bottom(px(4.))
                .left(px(4.))
                .max_w(gpui::relative(0.9))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .text_size(px(11.))
                .truncate()
                .child(link::hint(&link.target)),
        )
    }

    /// The search bar over the pane's top right corner.
    fn render_search(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let bar = self.search.as_ref()?;
        let found = bar.current.is_some() || bar.compiled.is_none();
        let theme = cx.theme();
        let toggle = |id: &'static str, label: &'static str, on: bool, regex: bool| {
            Button::new(id)
                .label(label)
                .ghost()
                .xsmall()
                .selected(on)
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.toggle_search_option(regex, cx);
                }))
        };
        let step = |id: &'static str, label: &'static str, up: bool| {
            Button::new(id)
                .label(label)
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |pane, _, _, cx| pane.search_step(up, cx)))
        };
        Some(
            div()
                .key_context(SEARCH_CONTEXT)
                .on_action(cx.listener(|pane, _: &CloseSearch, window, cx| {
                    pane.close_search(window, cx);
                }))
                .absolute()
                .top(px(6.))
                .right(px(6.))
                .w(px(340.))
                .p(px(4.))
                .flex()
                .items_center()
                .gap(px(2.))
                .rounded(px(6.))
                .border_1()
                .border_color(if found { theme.border } else { theme.danger })
                .bg(theme.popover)
                .shadow_md()
                .child(div().flex_1().child(Input::new(&bar.input).xsmall()))
                .child(toggle("search-case", "Aa", bar.case_sensitive, false))
                .child(toggle("search-regex", ".*", bar.regex, true))
                .child(step("search-up", "↑", true))
                .child(step("search-down", "↓", false)),
        )
    }
}

/// Input methods (spec 0002, flow 2): what they compose shows at the
/// cursor, and what they commit goes to the program as is. The grid's text
/// isn't theirs to read, so there is nothing to select or replace.
impl EntityInputHandler for TerminalPane {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let preedit = self.preedit.as_ref()?;
        Some(0..preedit.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.preedit.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit = None;
        self.type_text(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preedit = Some(text.to_owned()).filter(|text| !text.is_empty());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds()
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text_only = matches!(self.state, State::Failed(_) | State::Waiting(_));
        if (self.has_output || text_only) && !self.first_frame_scheduled {
            self.first_frame_scheduled = true;
            let pane = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = pane.update(cx, |_, cx| cx.emit(PaneEvent::FirstFrame));
            });
        }

        let focused = self.focus.is_focused(window);
        let blinking = match &self.state {
            State::Running(running) => focused && running.term.lock().cursor_style().blinking,
            _ => false,
        };
        self.blink(blinking, cx);
        let mut marks = self.marks();
        marks.hide_cursor = !self.cursor_shown;
        marks.preedit = self.preedit.clone();
        let content = match &self.state {
            State::Running(running) => element::TerminalElement::new(
                cx.entity(),
                running.term.clone(),
                self.settings.font.clone(),
                self.settings.theme,
                focused,
                marks,
                self.focus.clone(),
            )
            .into_any_element(),
            State::Failed(message) => self
                .message(message, self.settings.theme.ansi[1])
                .into_any_element(),
            State::Waiting(prompt) => self
                .message(prompt, self.settings.theme.foreground)
                .into_any_element(),
        };

        // The dialog sits beside the grid, not in it, so keys typed into the
        // dialog never pass the grid's key handler.
        let grid = div()
            .id("grid")
            .size_full()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .map(|grid| match self.hovered_link {
                Some(_) => grid.cursor_pointer(),
                None => grid.cursor_text(),
            })
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::middle_click))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up_out))
            .on_scroll(cx.listener(Self::scroll_wheel))
            .on_hover(cx.listener(Self::hover_ended))
            .child(content);
        div()
            .size_full()
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, _, window, cx| {
                    window.focus(&pane.focus_handle(cx), cx);
                    cx.emit(PaneEvent::Focused);
                }),
            )
            .child(grid)
            .children(self.render_link_hint(cx))
            .children(self.render_search(cx))
            .children(self.dialog.clone())
    }
}

/// What a connection's prompts got from the keychain so far (spec 0006,
/// flow 4).
#[derive(Default)]
struct Offers {
    /// Asked for once each: a second prompt for one gets a dialog, never
    /// another fill, so a wrong saved password can't loop (AC5).
    asked: HashSet<Key>,
    /// Filled in from the keychain. A second prompt for one means the saved
    /// secret failed.
    filled: HashSet<Key>,
    /// There is no keychain: nothing is asked or offered to remember.
    no_keychain: bool,
}

impl Offers {
    /// Whether to ask the keychain for `key` now: once per connection, and
    /// only with a keychain.
    fn ask(&mut self, key: &Key) -> bool {
        !self.no_keychain && self.asked.insert(key.clone())
    }

    /// The keychain option of the dialog for a prompt answered by `key`, if
    /// any: after the saved secret failed, it offers to replace it.
    fn remember(&self, key: Option<Key>) -> Option<Remember> {
        let key = key.filter(|_| !self.no_keychain)?;
        Some(Remember {
            replace: self.filled.contains(&key),
            key,
        })
    }
}

/// What the keychain holds for a prompt.
enum Lookup {
    Found(Zeroizing<String>),
    Nothing,
    NoKeychain,
    /// It couldn't answer: asked like nothing saved, with this notice.
    Failed(String),
}

/// Asks the keychain for `key` on a thread of its own. No answer within
/// [`KEYCHAIN_TIMEOUT`] counts as nothing saved (TM-askpass-17), with a
/// notice saying so.
async fn lookup(key: &Key, executor: &BackgroundExecutor) -> Lookup {
    let key = key.clone();
    let found = secrets::off_thread(move || secrets::get(&key));
    match within(found, KEYCHAIN_TIMEOUT, executor).await {
        Some(Ok(Ok(Some(secret)))) => Lookup::Found(secret),
        Some(Ok(Ok(None))) => Lookup::Nothing,
        Some(Ok(Err(secrets::GetError::Unavailable))) => Lookup::NoKeychain,
        Some(Ok(Err(secrets::GetError::Failed(error)))) => Lookup::Failed(error),
        Some(Err(_)) => Lookup::Failed("the keychain lookup couldn't start".into()),
        None => Lookup::Failed("the keychain didn't answer".into()),
    }
}

/// `future`'s output, unless it takes longer than `limit`.
async fn within<T>(
    future: impl Future<Output = T>,
    limit: Duration,
    executor: &BackgroundExecutor,
) -> Option<T> {
    match select(pin!(future), executor.timer(limit)).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(_) => None,
    }
}

/// The notice when the keychain answered a prompt: it names what it used,
/// never the secret (TM-askpass-10). It can't say the secret worked: a
/// wrong one gets the replace dialog right after.
fn filled_notice(key: &Key) -> String {
    match key {
        Key::Password(target) => format!("Used the password saved for {target}"),
        Key::Passphrase(file) => format!("Used the passphrase saved for {file}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::config::tests::ssh_to;
    use alacritty_terminal::event::{EventListener, VoidListener};
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::{ClipboardType, Term};
    use alacritty_terminal::vte::ansi::Rgb;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A `cols`×`rows` terminal that has parsed `bytes`, as a pane's event
    /// loop would. Shared by the tests of the terminal module.
    pub(super) fn term_with(cols: u16, rows: u16, bytes: &[u8]) -> Term<VoidListener> {
        let size = GridSize {
            cols,
            rows,
            cell_width: 8,
            cell_height: 16,
        };
        let config = alacritty_terminal::term::Config::default();
        let mut term = Term::new(config, &size, VoidListener);
        Processor::<StdSyncHandler>::new().advance(&mut term, bytes);
        term
    }

    /// The visible rows as text, without trailing blanks.
    pub(super) fn screen<T>(term: &Term<T>) -> Vec<String> {
        let grid = term.grid();
        (0..grid.screen_lines())
            .map(|line| {
                let row = &grid[Line(line as i32)];
                let text: String = (0..grid.columns()).map(|col| row[Column(col)].c).collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    #[test]
    fn close_on_exit_policies() {
        // Exit codes 0 and 3, then killed by a signal.
        let cases = [
            (CloseOnExit::Always, [true, true, true]),
            (CloseOnExit::Clean, [true, false, false]),
            (CloseOnExit::Never, [false, false, false]),
        ];
        for (policy, expected) in cases {
            for (code, closes) in [Some(0), Some(3), None].into_iter().zip(expected) {
                assert_eq!(closes_on_exit(policy, code), closes, "{policy:?} {code:?}");
            }
        }
    }

    #[test]
    fn default_config_closes_on_exit_0_and_stays_open_on_exit_3() {
        let policy = Config::default().close_on_exit;
        assert!(closes_on_exit(policy, Some(0)));
        assert!(!closes_on_exit(policy, Some(3)));
    }

    #[test]
    fn exit_notice_shows_the_code_on_the_line_after_the_output() {
        let mut bytes = b"hi".to_vec();
        bytes.extend_from_slice(exit_notice(Some(3)).as_bytes());
        let term = term_with(40, 3, &bytes);
        assert_eq!(screen(&term)[..2], ["hi", "[process exited with code 3]"]);
    }

    /// Spec 0006, flow 5: ssh exits 255 when the connection failed or
    /// dropped.
    #[test]
    fn an_ssh_exit_255_reconnects_whatever_close_on_exit_says() {
        for policy in [CloseOnExit::Always, CloseOnExit::Clean, CloseOnExit::Never] {
            let after = after_exit(true, policy, Some(255));
            assert_eq!(after, AfterExit::Reconnect, "{policy:?}");
        }
    }

    /// The remote shell's `exit`, a failed remote command, a signal: the
    /// pane does what `close_on_exit` says, as for any program.
    #[test]
    fn every_other_ssh_exit_follows_close_on_exit() {
        use AfterExit::{Close, Stay};
        // Exit codes 0 and 1, then killed by a signal.
        let cases = [
            (CloseOnExit::Always, [Close, Close, Close]),
            (CloseOnExit::Clean, [Close, Stay, Stay]),
            (CloseOnExit::Never, [Stay, Stay, Stay]),
        ];
        for (policy, expected) in cases {
            for (code, after) in [Some(0), Some(1), None].into_iter().zip(expected) {
                assert_eq!(after_exit(true, policy, code), after, "{policy:?} {code:?}");
            }
        }
    }

    /// Only ssh's 255 means a dropped connection; for a local program it's
    /// an exit code like any other.
    #[test]
    fn a_local_pane_follows_close_on_exit_even_on_exit_255() {
        use AfterExit::{Close, Stay};
        // Exit codes 0 and 255, then killed by a signal.
        let cases = [
            (CloseOnExit::Always, [Close, Close, Close]),
            (CloseOnExit::Clean, [Close, Stay, Stay]),
            (CloseOnExit::Never, [Stay, Stay, Stay]),
        ];
        for (policy, expected) in cases {
            for (code, after) in [Some(0), Some(255), None].into_iter().zip(expected) {
                assert_eq!(
                    after_exit(false, policy, code),
                    after,
                    "{policy:?} {code:?}"
                );
            }
        }
    }

    /// Spec 0006, flow 5.
    #[test]
    fn a_dropped_connection_shows_the_reconnect_hint_on_the_line_after_the_output() {
        let mut bytes = b"Connection to prod closed.".to_vec();
        bytes.extend_from_slice(RECONNECT_NOTICE.as_bytes());
        let term = term_with(60, 3, &bytes);
        let expected = [
            "Connection to prod closed.",
            "[connection closed: press Enter to reconnect]",
        ];
        assert_eq!(screen(&term)[..2], expected);
    }

    #[test]
    fn panes_of_an_ssh_profile_reconnect_and_local_ones_dont() {
        let ssh = Profile {
            kind: ProfileKind::Ssh(ssh_to("prod.example.com")),
            ..Profile::shell()
        };
        assert!(PaneProfile::from(&ssh).ssh);
        assert!(!PaneProfile::from(&Profile::shell()).ssh);
    }

    /// Spec 0006, flow 6: a restored SSH tab names the host it waits for.
    #[test]
    fn a_waiting_ssh_pane_asks_for_enter_naming_its_host() {
        assert_eq!(
            connect_prompt("Production DB"),
            "Press Enter to connect to Production DB"
        );
    }

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

    /// Keeps the events a terminal raises, which a pane hears of through
    /// its event proxy.
    #[derive(Clone, Default)]
    struct Recorder(Rc<RefCell<Vec<Event>>>);

    impl EventListener for Recorder {
        fn send_event(&self, event: Event) {
            self.0.borrow_mut().push(event);
        }
    }

    /// 80×24 cells of 8×16 pixels.
    const GRID: GridSize = GridSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };

    /// What a pane takes from the default config.
    fn default_settings() -> PaneSettings {
        let config = Config::default();
        PaneSettings {
            font: FontSettings::from_config(&config.font),
            theme: theme::DARK,
            close_on_exit: config.close_on_exit,
            scrollback: config.scrollback as usize,
            copy_on_select: config.copy_on_select,
            cursor: config.cursor,
            open_links: config.open_links,
        }
    }

    /// A pane's terminal under `settings` once its program printed `bytes`,
    /// and the events that raised.
    fn printed(settings: &PaneSettings, bytes: &[u8]) -> (Term<Recorder>, Vec<Event>) {
        let recorder = Recorder::default();
        let mut term = Term::new(settings.term_config(), &GRID, recorder.clone());
        Processor::<StdSyncHandler>::new().advance(&mut term, bytes);
        let events = recorder.0.take();
        (term, events)
    }

    /// What a pane under the default config sends back for `bytes` from
    /// its program.
    fn replies(bytes: &[u8]) -> Vec<String> {
        let (term, events) = printed(&default_settings(), bytes);
        events
            .into_iter()
            .filter_map(|event| reply(event, term.colors(), &theme::DARK, GRID))
            .collect()
    }

    /// TM-escape-04, -05 [load-bearing]: output can neither write the
    /// clipboard with OSC 52 nor read it. The pane's terminal raises no
    /// clipboard event for either, and one raised anyway gets no reply.
    #[test]
    fn osc_52_can_neither_write_nor_read_the_clipboard() {
        // "rm -rf ~" to the clipboard and to the primary selection, then
        // reads of both, ended by BEL and by ST.
        let requests: [&[u8]; 4] = [
            b"\x1b]52;c;cm0gLXJmIH4=\x07",
            b"\x1b]52;p;cm0gLXJmIH4=\x1b\\",
            b"\x1b]52;c;?\x07",
            b"\x1b]52;p;?\x1b\\",
        ];
        for request in requests {
            let (_, events) = printed(&default_settings(), request);
            let clipboard = events
                .iter()
                .any(|event| matches!(event, Event::ClipboardStore(..) | Event::ClipboardLoad(..)));
            let request = String::from_utf8_lossy(request);
            assert!(!clipboard, "{} raised {events:?}", request.escape_debug());
        }
        let colors = Colors::default();
        for kind in [ClipboardType::Clipboard, ClipboardType::Selection] {
            let store = Event::ClipboardStore(kind, "rm -rf ~".into());
            assert_eq!(reply(store, &colors, &theme::DARK, GRID), None);
            let load =
                Event::ClipboardLoad(kind, Arc::new(|text: &str| format!("\x1b]52;c;{text}")));
            assert_eq!(reply(load, &colors, &theme::DARK, GRID), None);
        }
    }

    /// TM-escape-06, -11: replies are fixed strings and numbers. What
    /// alacritty_terminal writes back itself (device attributes, status,
    /// cursor position, modes) passes as it is, colors go back as the
    /// theme's `rgb:` values and the text area as its size in pixels. A
    /// title report, which would echo text the program chose, gets nothing.
    #[test]
    fn replies_are_fixed_strings_and_numbers_never_terminal_text() {
        let attributes = Event::PtyWrite("\x1b[?6c".into());
        let passed = reply(attributes, &Colors::default(), &theme::DARK, GRID);
        assert_eq!(passed.as_deref(), Some("\x1b[?6c"));
        // The foreground, the background and palette color 1, each asked
        // for and answered the way xterm does.
        let queries = [
            ("10", theme::DARK.foreground),
            ("11", theme::DARK.background),
            ("4;1", theme::DARK.ansi[1]),
        ];
        for (code, value) in queries {
            let query = format!("\x1b]{code};?\x07");
            let Rgb { r, g, b } = colors::rgb(value);
            let answer =
                format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x07");
            assert_eq!(replies(query.as_bytes()), [answer], "OSC {code}");
        }
        // 24 rows of 16 pixels by 80 columns of 8.
        assert_eq!(replies(b"\x1b[14t"), ["\x1b[4;384;640t"]);
        // The title and icon label reports, after a title was set.
        let reports = replies(b"\x1b]2;rm -rf ~\x07\x1b[21t\x1b[20t");
        assert!(reports.is_empty(), "{reports:?}");
    }

    /// Spec 0005, `cursor`: the configured style and blink are the cursor
    /// a program starts with. A program may change it, as vim does in
    /// insert mode, and its reset goes back to the configured one.
    #[test]
    fn the_configured_cursor_is_the_one_a_program_starts_with_and_resets_to() {
        let shapes = [
            (config::CursorStyle::Block, CursorShape::Block),
            (config::CursorStyle::Beam, CursorShape::Beam),
            (config::CursorStyle::Underline, CursorShape::Underline),
        ];
        for (style, shape) in shapes {
            for blink in [true, false] {
                let settings = PaneSettings {
                    cursor: config::Cursor { style, blink },
                    ..default_settings()
                };
                let configured = CursorStyle {
                    shape,
                    blinking: blink,
                };
                let (mut term, _) = printed(&settings, b"");
                assert_eq!(term.cursor_style(), configured, "{style:?}, blink {blink}");
                // vim's insert mode, a blinking bar (DECSCUSR 5), then the
                // reset as it leaves (DECSCUSR 0).
                let mut parser = Processor::<StdSyncHandler>::new();
                parser.advance(&mut term, b"\x1b[5 q");
                let bar = CursorStyle {
                    shape: CursorShape::Beam,
                    blinking: true,
                };
                assert_eq!(term.cursor_style(), bar);
                parser.advance(&mut term, b"\x1b[0 q");
                assert_eq!(term.cursor_style(), configured, "{style:?}, blink {blink}");
            }
        }
    }

    /// `raw` cleans to `words`, joined by single spaces or by nothing: a
    /// control character may be dropped or become a space, but never stays.
    fn assert_title(raw: &str, words: &[&str]) {
        let title = clean_title(raw);
        assert!(
            title == words.join(" ") || title == words.concat(),
            "{raw:?} became {title:?}"
        );
    }

    #[test]
    fn titles_lose_their_control_characters() {
        assert_title("bell\x07here", &["bell", "here"]);
        assert_title("esc\x1b[31mred", &["esc", "[31mred"]);
        assert_title("one\ntwo\r\nthree", &["one", "two", "three"]);
        assert_title("tab\there", &["tab", "here"]);
        // NUL, DEL and the C1 control CSI.
        assert_title("nul\0del\x7fcsi\u{9b}end", &["nul", "del", "csi", "end"]);
    }

    #[test]
    fn title_whitespace_runs_collapse_to_one_space() {
        assert_eq!(
            clean_title("cargo   build \t\n  --release"),
            "cargo build --release"
        );
        assert_eq!(clean_title("vim ~/notes.md"), "vim ~/notes.md");
    }

    /// AC9 (spec 0003): a 5 000-character title is capped at 200
    /// characters, on one line.
    #[test]
    fn a_5000_character_title_is_capped_at_200_characters_on_one_line() {
        assert_eq!(clean_title(&"x".repeat(5000)).chars().count(), 200);
        // Characters, not bytes: two- and four-byte characters stay whole.
        assert_eq!(clean_title(&"ä🦀".repeat(2500)), "ä🦀".repeat(100));
        let lines = clean_title(&"line\n".repeat(1000));
        assert_eq!(lines.chars().count(), 200);
        assert!(!lines.contains(['\n', '\r']), "{lines:?}");
        // The other Unicode line breaks: CR, VT, FF, NEL, LS and PS.
        for separator in ["\r", "\u{0b}", "\u{0c}", "\u{85}", "\u{2028}", "\u{2029}"] {
            assert_title(&format!("first{separator}second"), &["first", "second"]);
        }
    }

    /// The characters that hide or reorder text: Arabic letter mark,
    /// zero-width space, direction marks, embeddings, overrides and
    /// isolates, invisible operators, byte-order mark, and the first tag
    /// character, tag "A" and the closing cancel tag.
    fn invisible_characters() -> impl Iterator<Item = char> {
        ['\u{061C}', '\u{200B}', '\u{200E}', '\u{200F}', '\u{FEFF}']
            .into_iter()
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2060}'..='\u{2064}')
            .chain('\u{2066}'..='\u{2069}')
            .chain(['\u{E0000}', '\u{E0041}', '\u{E007F}'])
    }

    /// 👨‍💻 (man, zero-width joiner, laptop), and Persian "mikhaham" (I
    /// want), with a zero-width non-joiner after "mi".
    const JOINED: [&str; 2] = [
        "\u{1F468}\u{200D}\u{1F4BB}",
        "\u{0645}\u{06CC}\u{200C}\u{062E}\u{0648}\u{0627}\u{0647}\u{0645}",
    ];

    /// A title can't reorder or hide text: a right-to-left override would
    /// show `invoice\u{202E}fdp.exe` as "invoiceexe.pdf", zero-width
    /// characters make different text look the same, and tag characters
    /// carry text nobody sees.
    #[test]
    fn titles_lose_characters_that_hide_or_reorder_text() {
        assert_title("invoice\u{202E}fdp.exe", &["invoice", "fdp.exe"]);
        assert_title("ad\u{200B}min\u{FEFF}", &["ad", "min"]);
        // "rm" spelled in tag characters.
        assert_title("build\u{E0072}\u{E006D}", &["build"]);
        for c in invisible_characters() {
            assert_title(&format!("before{c}after"), &["before", "after"]);
        }
    }

    /// The zero-width joiner and non-joiner are part of the text: emoji
    /// sequences and scripts like Persian need them.
    #[test]
    fn titles_keep_the_joiners_emoji_and_scripts_need() {
        let [technologist, persian] = JOINED;
        let both = format!("{technologist} {persian}");
        for title in [technologist, persian, both.as_str()] {
            assert_eq!(clean_title(title), title);
        }
    }

    /// Spec 0006, flow 4: ssh's host-key question spans several lines, and
    /// its dialog shows them as lines. A tab is a space.
    #[test]
    fn prompts_keep_their_line_breaks_and_tabs_become_spaces() {
        let question = "The authenticity of host 'prod (192.0.2.10)' can't be established.\n\
                        ED25519 key fingerprint is: SHA256:ufr9ck+x5zXLjNm4HvDvXiRs6L/6qjQoTXCJCukwfy8\n\
                        This key is not known by any other names.\n\
                        Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(clean_prompt(question), question);
        assert_eq!(clean_prompt("first\r\nsecond"), "first\nsecond");
        assert_eq!(clean_prompt("Verification\tcode: "), "Verification code: ");
    }

    /// TM-askpass-08: a server's prompt text can't steer its dialog: the
    /// control characters are dropped, line breaks aside.
    #[test]
    fn prompts_lose_their_control_characters() {
        let cases = [
            ("bell\x07here", "bellhere"),
            ("esc\x1b[31mred", "esc[31mred"),
            ("back\x08space", "backspace"),
            // NUL, DEL and the C1 control CSI.
            ("nul\0del\x7fcsi\u{9b}end", "nuldelcsiend"),
        ];
        for (raw, cleaned) in cases {
            assert_eq!(clean_prompt(raw), cleaned, "{raw:?}");
        }
    }

    /// TM-askpass-08, by the title rules of spec 0003: a prompt can't hide
    /// or reorder what it says, say to make its dialog name another host
    /// than the one asking. The joiners emoji and scripts need stay.
    #[test]
    fn prompts_lose_the_characters_that_hide_or_reorder_text() {
        for c in invisible_characters() {
            let prompt = format!("alice@prod{c}'s password: ");
            assert_eq!(
                clean_prompt(&prompt),
                "alice@prod's password: ",
                "{prompt:?}"
            );
        }
        for text in JOINED {
            assert_eq!(clean_prompt(text), text);
        }
    }

    /// TM-askpass-08: a dialog shows 4 KiB of a prompt at most, cut between
    /// two characters, never through one. What's dropped doesn't count.
    #[test]
    fn a_prompt_is_capped_at_4_kib_between_characters() {
        assert_eq!(clean_prompt(&"x".repeat(5000)), "x".repeat(4096));
        // 4 KiB are 1 024 four-byte crabs.
        assert_eq!(clean_prompt(&"🦀".repeat(2000)), "🦀".repeat(1024));
        // One byte ahead, the crab that would cross 4 KiB is left out whole.
        assert_eq!(
            clean_prompt(&format!("x{}", "🦀".repeat(2000))),
            format!("x{}", "🦀".repeat(1023))
        );
        assert_eq!(
            clean_prompt(&format!("{}ok", "\u{200B}".repeat(5000))),
            "ok"
        );
    }

    /// TM-askpass-10: each fill from the keychain says what it used, in the
    /// words of spec 0006 for a password. It names, and has, no secret.
    #[test]
    fn a_fill_notice_names_the_target_or_key_file_it_used() {
        assert_eq!(
            filled_notice(&Key::Password("alice@prod.example.com:22".into())),
            "Used the password saved for alice@prod.example.com:22"
        );
        let file = "/home/alice/.ssh/id_ed25519";
        let notice = filled_notice(&Key::Passphrase(file.into()));
        assert!(
            notice.contains(file) && notice.contains("passphrase"),
            "{notice}"
        );
    }

    /// The connection's password and its key's passphrase, as
    /// `Target::secret_for` names them.
    fn password() -> Key {
        Key::Password("alice@prod.example.com:22".into())
    }

    fn passphrase() -> Key {
        Key::Passphrase("/home/alice/.ssh/id_ed25519".into())
    }

    /// What the keychain of an `Offers` test holds for any key it's asked.
    #[derive(Clone, Copy)]
    enum Keychain {
        Saved,
        Empty,
        /// There is none: no Secret Service is running.
        Missing,
    }

    /// What became of a prompt.
    #[derive(Debug, PartialEq)]
    enum Outcome {
        /// Filled in from the keychain, with no dialog.
        Filled,
        /// A dialog. `asked`: whether the keychain was asked first.
        /// `remember`: its keychain option, as the key and whether it
        /// replaces what's saved.
        Dialog {
            asked: bool,
            remember: Option<(Key, bool)>,
        },
    }

    /// A prompt for `key` (`None`: no secret of the connection's own) taken
    /// through `offers` as `answer_prompts` takes it: the keychain is asked
    /// only when `offers` says so, what it has fills the prompt, and every
    /// prompt not filled gets a dialog.
    fn take(offers: &mut Offers, key: Option<&Key>, keychain: Keychain) -> Outcome {
        let asked = key.filter(|key| offers.ask(key));
        match (asked, keychain) {
            (Some(key), Keychain::Saved) => {
                offers.filled.insert(key.clone());
                return Outcome::Filled;
            }
            (Some(_), Keychain::Missing) => offers.no_keychain = true,
            _ => {}
        }
        let remember = offers.remember(key.cloned());
        Outcome::Dialog {
            asked: asked.is_some(),
            remember: remember.map(|remember| (remember.key, remember.replace)),
        }
    }

    /// AC5 (spec 0006, flow 4): the saved password is filled in once. When
    /// it was wrong, ssh asks again, and that prompt gets a dialog instead
    /// of the keychain, so there is no retry loop, and the dialog offers to
    /// replace what's saved. The key's passphrase is a secret of its own,
    /// still asked for once.
    #[test]
    fn a_wrong_saved_password_is_filled_once_then_its_dialog_offers_to_replace_it() {
        let mut offers = Offers::default();
        let filled = take(&mut offers, Some(&password()), Keychain::Saved);
        assert_eq!(filled, Outcome::Filled);
        let replace = Outcome::Dialog {
            asked: false,
            remember: Some((password(), true)),
        };
        // ssh asks up to `NumberOfPasswordPrompts` (3) times.
        for attempt in 2..=3 {
            let again = take(&mut offers, Some(&password()), Keychain::Saved);
            assert_eq!(again, replace, "attempt {attempt}");
        }
        let unlocked = take(&mut offers, Some(&passphrase()), Keychain::Saved);
        assert_eq!(unlocked, Outcome::Filled);
    }

    /// Spec 0006, flow 4: with nothing saved, the connection's own password
    /// prompt gets a dialog offering "Remember in keychain", a first save
    /// rather than a replacement, and so does ssh's next prompt for it,
    /// without asking the keychain again.
    #[test]
    fn with_nothing_saved_the_dialog_offers_to_remember_the_password() {
        let mut offers = Offers::default();
        let first = take(&mut offers, Some(&password()), Keychain::Empty);
        let remember = Some((password(), false));
        assert_eq!(
            first,
            Outcome::Dialog {
                asked: true,
                remember: remember.clone()
            }
        );
        let again = take(&mut offers, Some(&password()), Keychain::Empty);
        assert_eq!(
            again,
            Outcome::Dialog {
                asked: false,
                remember
            }
        );
    }

    /// Spec 0006, flow 4: a prompt that asks for no secret of the
    /// connection's own, like a one-time code or a jump host's password,
    /// gets a dialog without the keychain being asked, and nothing is
    /// offered to remember.
    #[test]
    fn a_prompt_for_no_secret_of_the_connections_own_gets_a_dialog_with_nothing_to_remember() {
        let mut offers = Offers::default();
        let outcome = take(&mut offers, None, Keychain::Saved);
        let bare = Outcome::Dialog {
            asked: false,
            remember: None,
        };
        assert_eq!(outcome, bare);
    }

    /// Spec 0006, flow 8: once a lookup finds no keychain, nothing more is
    /// asked of it, and no dialog offers to remember: there's nowhere to
    /// save. Connecting still works, through the dialogs.
    #[test]
    fn without_a_keychain_nothing_more_is_asked_and_nothing_is_offered_to_remember() {
        let mut offers = Offers::default();
        let first = take(&mut offers, Some(&password()), Keychain::Missing);
        assert_eq!(
            first,
            Outcome::Dialog {
                asked: true,
                remember: None
            }
        );
        let bare = Outcome::Dialog {
            asked: false,
            remember: None,
        };
        for key in [passphrase(), password()] {
            let outcome = take(&mut offers, Some(&key), Keychain::Missing);
            assert_eq!(outcome, bare, "{key:?}");
        }
    }
}
