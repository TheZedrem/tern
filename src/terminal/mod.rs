//! The terminal pane (spec 0002): one program in one PTY, drawn by
//! [`element::TerminalElement`] and fed by the keyboard through [`input`].

mod colors;
mod element;
pub mod grid;
pub mod input;
pub mod pty;

use crate::config::{self, CloseOnExit, Profile, ProfileKind};
use crate::ssh;
use crate::theme::{self, Theme};
use crate::timing::Timing;
use alacritty_terminal::event::{Event, Notify, OnResize};
use alacritty_terminal::event_loop::{Msg, Notifier};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::color::COUNT as COLOR_SLOTS;
use alacritty_terminal::term::{Config as TermConfig, TermMode};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use futures::StreamExt;
use gpui::{
    App, Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, NoAction, ParentElement, Pixels, Render, SharedString, Size, Styled,
    Window, div, font, px, size,
};
use pty::{EventProxy, GridSize, Terminal};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

/// Longest tab title taken from a program (spec 0003).
const MAX_TITLE_CHARS: usize = 200;

/// The embedded default font (spec 0005).
pub const DEFAULT_FONT_FAMILY: &str = "Lilex";

/// A pane's key context. Its bindings outrank those of the widget
/// library's `Root`, which wraps every window.
const KEY_CONTEXT: &str = "Terminal";

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
    ]);
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
    /// An SSH connection that drops waits for Enter to reconnect (spec
    /// 0006, flow 5).
    pub reconnects: bool,
}

impl From<&Profile> for PaneProfile {
    fn from(profile: &Profile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.display_name().to_owned(),
            theme: profile.theme.clone(),
            reconnects: matches!(profile.kind, ProfileKind::Ssh(_)),
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
    /// With its program started, or the reason it couldn't start.
    Started(Result<pty::Session, String>),
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
        };
        match start {
            Start::Started(session) => pane.attach(session, cx),
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
                self.state = State::Failed(failure(&self.launch.command, &error));
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
    fn start(&mut self, window: &Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.settings.font.estimated_cell(),
        );
        let session = pty::spawn(
            &self.launch.command,
            grid,
            self.launch.cwd.as_deref(),
            self.settings.scrollback,
            Arc::clone(&self.launch.timing),
        );
        self.attach(session, cx);
        cx.notify();
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
    /// connection dropped: the old session stays in the scrollback.
    fn rerun(&mut self, cx: &mut Context<Self>) {
        let State::Running(running) = &mut self.state else {
            return;
        };
        let started = pty::rerun(
            &self.launch.command,
            running.size,
            self.launch.cwd.as_deref(),
            &running.term,
            &running.proxy,
        );
        match started {
            Ok((notifier, child_pid)) => {
                running.notifier = notifier;
                running.child_pid = child_pid;
                self.exited = false;
                self.reconnect = false;
            }
            Err(error) => {
                let notice = format!("\r\n{}", failure(&self.launch.command, &error));
                let mut parser = Processor::<StdSyncHandler>::new();
                parser.advance(&mut *running.term.lock(), notice.as_bytes());
            }
        }
        cx.notify();
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn profile(&self) -> &PaneProfile {
        &self.profile
    }

    /// Takes the settings of a reloaded config: font, theme and scrollback
    /// apply at once, `close_on_exit` at the next exit.
    pub fn apply(&mut self, settings: PaneSettings, cx: &mut Context<Self>) {
        if let State::Running(running) = &self.state
            && settings.scrollback != self.settings.scrollback
        {
            running.term.lock().set_options(TermConfig {
                scrolling_history: settings.scrollback,
                ..TermConfig::default()
            });
        }
        self.settings = settings;
        cx.notify();
    }

    /// The program's working directory, so a split starts in the same place
    /// (spec 0003, flow 1). Read from `/proc` on Linux; elsewhere unknown
    /// until OSC 7 tracking lands (T-002).
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
            // Replies to queries (device attributes, cursor position, ...).
            Event::PtyWrite(text) => running.notifier.notify(text.into_bytes()),
            Event::ColorRequest(slot, format) if slot < COLOR_SLOTS => {
                let color = running.term.lock().colors()[slot]
                    .unwrap_or_else(|| colors::default_for_slot(slot, &self.settings.theme));
                running.notifier.notify(format(color).into_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                running
                    .notifier
                    .notify(format(running.size.window_size()).into_bytes());
            }
            Event::Title(title) => {
                self.title = Some(clean_title(&title)).filter(|title| !title.is_empty());
                cx.emit(PaneEvent::TitleChanged);
            }
            Event::ResetTitle => {
                self.title = None;
                cx.emit(PaneEvent::TitleChanged);
            }
            // Bell and clipboard requests arrive with T-002.
            _ => {}
        }
    }

    fn child_exited(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        self.exited = true;
        let after = after_exit(self.profile.reconnects, self.settings.close_on_exit, code);
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
                self.rerun(cx);
                cx.stop_propagation();
            }
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
        cx.stop_propagation();
    }

    /// Resizes the terminal and the PTY when the grid that fits changed.
    fn resize(&mut self, size: GridSize) {
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

        let content = match &self.state {
            State::Running(running) => element::TerminalElement::new(
                cx.entity(),
                running.term.clone(),
                self.settings.font.clone(),
                self.settings.theme,
                self.focus.is_focused(window),
            )
            .into_any_element(),
            State::Failed(message) => self
                .message(message, self.settings.theme.ansi[1])
                .into_any_element(),
            State::Waiting(prompt) => self
                .message(prompt, self.settings.theme.foreground)
                .into_any_element(),
        };

        div()
            .size_full()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pane, _, window, cx| {
                    window.focus(&pane.focus, cx);
                    cx.emit(PaneEvent::Focused);
                }),
            )
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::config::tests::ssh_to;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::Term;

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
        assert!(PaneProfile::from(&ssh).reconnects);
        assert!(!PaneProfile::from(&Profile::shell()).reconnects);
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
        let invisible = ['\u{061C}', '\u{200B}', '\u{200E}', '\u{200F}', '\u{FEFF}']
            .into_iter()
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2060}'..='\u{2064}')
            .chain('\u{2066}'..='\u{2069}')
            // The first tag character, tag "A", and the closing cancel tag.
            .chain(['\u{E0000}', '\u{E0041}', '\u{E007F}']);
        for c in invisible {
            assert_title(&format!("before{c}after"), &["before", "after"]);
        }
    }

    /// The zero-width joiner and non-joiner are part of the text: emoji
    /// sequences and scripts like Persian need them.
    #[test]
    fn titles_keep_the_joiners_emoji_and_scripts_need() {
        // 👨‍💻: man, zero-width joiner, laptop.
        let technologist = "\u{1F468}\u{200D}\u{1F4BB}";
        // Persian "mikhaham" (I want), a zero-width non-joiner after "mi".
        let persian = "\u{0645}\u{06CC}\u{200C}\u{062E}\u{0648}\u{0627}\u{0647}\u{0645}";
        let both = format!("{technologist} {persian}");
        for title in [technologist, persian, both.as_str()] {
            assert_eq!(clean_title(title), title);
        }
    }
}
