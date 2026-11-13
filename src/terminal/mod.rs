//! The terminal pane (spec 0002): one program in one PTY, drawn by
//! [`element::TerminalElement`] and fed by the keyboard through [`input`].

mod answers;
mod colors;
mod element;
mod exit;
mod font;
pub mod grid;
pub mod input;
mod launch;
pub mod link;
mod mouse;
pub mod osc7;
pub mod prompt;
pub mod pty;
pub mod search;
pub mod select;

use crate::config::{self, CloseOnExit, Profile, ProfileKind};
use crate::theme::{self, Theme};
use alacritty_terminal::event::{Event, Notify, OnResize};
use alacritty_terminal::event_loop::{Msg, Notifier};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::color::{COUNT as COLOR_SLOTS, Colors};
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config as TermConfig, Osc52, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, NoAction, ParentElement, Pixels, Point, Render, SharedString, Size,
    StatefulInteractiveElement, Styled, Task, Window, actions, div, px,
};
use prompt::PromptDialog;
use pty::{EventProxy, GridSize, Terminal};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

pub use answers::{PromptHost, PromptNotice, answer_prompts};
pub use exit::{clean_prompt, clean_title, is_invisible_format};
pub use font::{DEFAULT_FONT_FAMILY, FONT_SIZES, FontSettings};
pub use launch::{Launch, Start, connect_prompt};
use search::SearchBar;

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
    /// and when it ends and the pane stays, Enter reconnects.
    pub ssh: bool,
    /// A program run from a files pane (spec 0013, flow 4): it stays open
    /// after it exits, whatever `close_on_exit` says, so its output and
    /// exit show.
    pub stays: bool,
}

impl From<&Profile> for PaneProfile {
    fn from(profile: &Profile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.display_name().to_owned(),
            theme: profile.theme.clone(),
            ssh: matches!(profile.kind, ProfileKind::Ssh(_)),
            stays: false,
        }
    }
}

/// What a pane tells its owner.
pub enum PaneEvent {
    /// The first frame with the pane's output, or its spawn error, was
    /// drawn. This is the TTFP moment (spec 0001).
    FirstFrame,
    /// The user clicked into the pane.
    Focused,
    /// The program set or reset its title.
    TitleChanged,
    /// A local pane's shell reported a new cwd (OSC 7), which the session
    /// saves.
    CwdChanged,
    /// The program exited and `close_on_exit` says the pane closes.
    Exited,
    /// A prompt dialog opened or closed, which moves the keyboard focus.
    DialogChanged,
    /// Something for a notice, like a password filled in from the keychain.
    Notice(String),
    /// There is no keychain; the workspace says so once (spec 0006, flow 8).
    NoKeychain,
    /// A prompt of a port forward's ssh opened or closed (spec 0017).
    ForwardPrompt { id: crate::forwards::Id, open: bool },
    /// The user declined a port forward's prompt: it fails as cancelled.
    ForwardDeclined(crate::forwards::Id),
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
    /// The cwd the shell reports with OSC 7.
    reported: std::sync::Arc<osc7::Reported>,
}

/// Whose ssh a prompt dialog answers: the pane's terminal connection, its
/// SFTP connection (spec 0008), or a port forward added from it (spec
/// 0017, flow 4).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Asker {
    Terminal,
    Sftp,
    Forward(crate::forwards::Id),
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
    /// The SSH connection ended, the pane stayed, and Enter reconnects
    /// (spec 0006, flow 5).
    reconnect: bool,
    has_output: bool,
    first_frame_scheduled: bool,
    /// The title the program set (OSC 0/2), already cleaned.
    title: Option<String>,
    /// Answers the running SSH connection's prompts (spec 0006, flow 4).
    /// Dropping it ends them, and their helpers exit 1.
    askpass: Option<Task<()>>,
    /// Answers the prompts of the pane's SFTP connection, which has a token
    /// of its own (spec 0008, Model).
    sftp_askpass: Option<Task<()>>,
    /// Answers the prompts of the port forwards started from this pane,
    /// each with a token of its own, and names them in the dialog (spec
    /// 0017, flow 4). Closing the pane ends them.
    forward_askpass: HashMap<crate::forwards::Id, (SharedString, Task<()>)>,
    /// The dialog over the pane while an ssh waits on a prompt, and whose.
    dialog: Option<(Asker, Entity<PromptDialog>)>,
    /// The terminal's and the SFTP connection's prompts take turns at the
    /// one dialog.
    dialog_turn: Rc<futures::lock::Mutex<()>>,
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
            sftp_askpass: None,
            forward_askpass: HashMap::new(),
            dialog: None,
            dialog_turn: Rc::default(),
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

    /// Shows `notice` as a toast. Deferred, so one raised while the pane is
    /// being created reaches the workspace's subscription.
    fn notice(&self, notice: String, cx: &mut Context<Self>) {
        let pane = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = pane.update(cx, |_, cx| cx.emit(PaneEvent::Notice(notice)));
        });
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

    /// What takes the keyboard when the pane is focused: its prompt dialog
    /// while one is open, else the grid.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.dialog {
            Some((_, dialog)) => dialog.read(cx).focus_target(cx),
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
    /// (spec 0003, flow 1): the one a local pane's shell last reported with
    /// OSC 7, else `/proc` on Linux; elsewhere unknown (spec 0002, cwd).
    pub fn cwd(&self) -> Option<PathBuf> {
        let State::Running(running) = &self.state else {
            return None;
        };
        if let Some(reported) = running.reported.local() {
            return Some(reported);
        }
        let pid = running.child_pid?;
        if cfg!(target_os = "linux") {
            std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
        } else {
            None
        }
    }

    /// Where a browser of this pane's local disk starts (spec 0018, Files
    /// panel): [`Self::cwd`], else the folder the pane was started in.
    pub fn local_folder(&self) -> Option<PathBuf> {
        self.cwd().or_else(|| self.launch.cwd.clone())
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
                if running.reported.take_changed() {
                    cx.emit(PaneEvent::CwdChanged);
                }
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
            .on_mouse_down(MouseButton::Right, cx.listener(Self::right_click))
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
            .children(self.dialog.as_ref().map(|(_, dialog)| dialog.clone()))
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
    use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

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
    fn panes_of_an_ssh_profile_reconnect_and_local_ones_dont() {
        let ssh = Profile {
            kind: ProfileKind::Ssh(ssh_to("prod.example.com")),
            ..Profile::shell()
        };
        assert!(PaneProfile::from(&ssh).ssh);
        assert!(!PaneProfile::from(&Profile::shell()).ssh);
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
}
