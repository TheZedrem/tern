//! The window's content (spec 0003): the tab bar, which doubles as the
//! title bar when the compositor draws no frame, over the active tab's
//! split tree, which tabs and panes are dragged into and out of (spec
//! 0007). The layout itself is plain data in [`tabs`] and [`tree`]; this
//! module owns the panes and draws them.

pub mod tabs;
pub mod tree;
mod updates;

use crate::commands::palette::{Entry, Palette, PaletteEvent};
use crate::commands::{
    self, ClosePane, CloseTab, ConnectToHost, CopySelection, DecreaseFontSize, FocusDown,
    FocusLeft, FocusRight, FocusUp, ForgetPassword, GoToTab, IncreaseFontSize, NewTab,
    NewTabWithProfile, NextTab, OpenConfig, OpenConfigFolder, OpenPalette, OpenSettings, Paste,
    PreviousTab, RenameTab, ResetFontSize, Search, SelectTheme, SplitDown, SplitDownWithProfile,
    SplitRight, SplitRightWithProfile, ToggleSftp, ZoomPane,
};
use crate::commands::{OpenFiles, OpenFilesInNewTab, SwitchFilesTarget};
use crate::config::{self, Config, Profile, ProfileKind, SftpSide, TabBarSide};
use crate::files::browser::{self, Browser, BrowserEvent, BrowserSettings, Source};
use crate::files::local;
use crate::files::pane::{FilesEvent, FilesPane, Target};
use crate::secrets;
use crate::session::{self, SavedFiles, SavedLeaf, SavedNode, SavedTab, Session, WindowSize};
use crate::settings::{self, Pick, SettingsEvent, SettingsView, change::Change};
use crate::sftp;
use crate::ssh;
use crate::terminal::pty::{self, Command, GridSize};
use crate::terminal::{
    FONT_SIZES, FontSettings, Launch, PaneEvent, PaneProfile, PaneSettings, Start, TerminalPane,
    connect_prompt,
};
use crate::theme::{self, Theme};
use crate::timing::{Mark, Timing};
use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, AnyElement, App, AppContext, Bounds, ClickEvent, Context, Decorations, Div,
    DragMoveEvent, ElementId, Entity, Focusable, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, ParentElement, Pixels, Point, Render, SharedString, Size, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, WindowAppearance, actions,
    canvas, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::notification::Notification;
use gpui_component::tab::{Tab, TabBar};
// Component icons only: `gpui_kit_assets::IconName` names the whole Lucide
// catalog, but the registered `Assets` bundle holds just these, and anything
// else renders blank.
use gpui_component::{ActiveTheme, IconName, Sizable, WindowExt, window_border};
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tabs::{Closed, Dragged, TabId, Tabs};
use tree::{Axis, Direction, LeafId, Node, Rect};

/// Key context of the workspace, where the commands are bound.
const CONTEXT: &str = commands::keymap::CONTEXT;
/// Key context of the tab-rename field.
const RENAME_CONTEXT: &str = "TabRename";
/// Thickness of a split divider, in logical pixels.
const DIVIDER: f32 = 4.0;
/// The smallest pane a divider may leave (spec 0003).
const MIN_PANE_CELLS: (f32, f32) = (10.0, 3.0);
/// The split picker's group of local profiles (spec 0007, flow 6).
const LOCAL_GROUP: &str = "Local";
/// How long config changes must settle before a reload: editors save in
/// bursts (spec 0005, flow 3).
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
/// How long boot waits for the first frame with output before starting
/// what waits for it anyway.
const SILENT_BOOT_WAIT: Duration = Duration::from_secs(1);
/// How long the layout must stay put before the session is saved (spec
/// 0003, flow 5).
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// The SFTP sidebar's width until it's dragged, and the least it can be
/// dragged to; the most is half the window (spec 0008, Model).
const SFTP_WIDTH: f32 = 300.0;
const SFTP_MIN_WIDTH: f32 = 200.0;

actions!(tern, [CancelRename, CloseWindow]);

/// What a close is about to remove, kept while it waits on SFTP uploads
/// (spec 0008, flow 3).
#[derive(Clone, Copy, PartialEq)]
enum Closing {
    Pane(LeafId),
    Tab(TabId),
    Window,
}

/// Binds the keys of the workspace's own widgets. The commands' keys are
/// [`commands::keymap`]'s.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelRename,
        Some(RENAME_CONTEXT),
    )]);
    browser::init(cx);
}

/// Dragging the SFTP sidebar's inner edge to resize it (spec 0008).
#[derive(Clone)]
struct DraggedSftpEdge;

/// An SSH pane's SFTP sidebar, and whether it's open. It outlives being
/// closed: the session goes on until the pane closes (spec 0008, Model).
struct Sidebar {
    view: Entity<Browser>,
    open: bool,
    _events: Subscription,
}

/// Dragging a tab to a new place in the bar, or onto a side of a pane
/// (spec 0007).
#[derive(Clone)]
struct DraggedTab {
    index: usize,
    id: TabId,
    title: SharedString,
}

/// Dragging a pane of a split tab by its grab handle (spec 0007).
#[derive(Clone)]
struct DraggedPane {
    leaf: LeafId,
    title: SharedString,
}

/// Dragging the divider of the split at this path.
#[derive(Clone)]
struct DraggedDivider(Vec<bool>);

/// What follows the mouse while a tab is dragged.
struct TabGhost(SharedString);

impl Render for TabGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme.drag_border)
            .bg(theme.tab_active)
            .text_color(theme.tab_active_foreground)
            .text_size(px(12.))
            .shadow_md()
            .child(self.0.clone())
    }
}

/// A divider drag shows nothing but the moving divider itself.
struct DividerGhost;

impl Render for DividerGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A tab being renamed: its ID and the text field.
struct Rename {
    tab: TabId,
    input: Entity<InputState>,
}

/// A files pane of the split tree (spec 0013), and its events.
struct FilesLeaf {
    pane: Entity<FilesPane>,
    _events: Subscription,
}

/// A pane of the window's first frame, its program started before the
/// window existed (spec 0001).
pub struct BootPane {
    pub start: Start,
    pub launch: Launch,
    pub profile: PaneProfile,
    pub theme: Theme,
    /// A restored pane's last title.
    pub title: Option<String>,
}

/// What the window starts with (spec 0003): a fresh tab of one pane, or a
/// restored session whose shown tab's panes started with the window.
pub struct Boot {
    tabs: Tabs,
    panes: Vec<(LeafId, BootPane)>,
    /// The shown tab's files panes, which start nothing before the first
    /// frame (spec 0013, Startup impact).
    files: Vec<(LeafId, SavedFiles)>,
    /// The other tabs' panes, started when their tab is first shown.
    pending: HashMap<LeafId, SavedLeaf>,
    /// A session that couldn't be restored, to set aside after the first
    /// frame (spec 0003, flow 7).
    bad_session: Option<PathBuf>,
}

impl Boot {
    /// One tab of `pane`.
    pub fn fresh(pane: BootPane) -> Self {
        let (tabs, leaf) = Tabs::new();
        Self {
            tabs,
            panes: vec![(leaf, pane)],
            files: Vec::new(),
            pending: HashMap::new(),
            bad_session: None,
        }
    }

    /// This boot, with the session file at `bad`, if any, to be set aside
    /// as `session.json.bad` once the first frame is on screen.
    pub fn setting_aside(self, bad: Option<PathBuf>) -> Self {
        Self {
            bad_session: bad,
            ..self
        }
    }

    /// The tabs of `session` (spec 0003, flow 6). `start` makes a pane for
    /// each terminal leaf of the shown tab, given its share of the window's
    /// `area` in pixels; its files leaves wait for the window, the other
    /// tabs' leaves for their tab. `None` for a session without tabs.
    pub fn restore(
        session: &Session,
        area: (f32, f32),
        mut start: impl FnMut(&SavedLeaf, (f32, f32)) -> BootPane,
    ) -> Option<Self> {
        let tabs = session
            .tabs
            .iter()
            .map(|saved| tabs::Tab {
                id: 0,
                name: saved.name.clone(),
                root: saved.root.tree(),
                focused: saved.focused,
                zoomed: saved.zoomed,
            })
            .collect();
        let tabs = Tabs::restore(tabs, session.active)?;
        let shown = tabs.active_index();
        let rects = tabs.active().root.rects(Rect::UNIT);
        let mut panes = Vec::new();
        let mut files = Vec::new();
        let mut pending = HashMap::new();
        let zoomed = tabs.active().zoomed;
        for (ix, saved) in session.tabs.iter().enumerate() {
            for leaf in saved.root.leaves() {
                if ix != shown {
                    // Shown in the tab bar until it starts: the title rules
                    // apply to a saved title too.
                    let title = leaf.title.as_deref().map(crate::terminal::clean_title);
                    let leaf = SavedLeaf {
                        title: title.filter(|title| !title.is_empty()),
                        ..leaf.clone()
                    };
                    pending.insert(leaf.id, leaf);
                    continue;
                }
                if let Some(saved) = &leaf.files {
                    files.push((leaf.id, saved.clone()));
                    continue;
                }
                let rect = match zoomed {
                    // A zoomed pane fills the window.
                    Some(zoomed) if zoomed == leaf.id => Rect::UNIT,
                    _ => rects
                        .iter()
                        .find(|(id, _)| *id == leaf.id)
                        .map_or(Rect::UNIT, |(_, rect)| *rect),
                };
                panes.push((
                    leaf.id,
                    start(leaf, (rect.width * area.0, rect.height * area.1)),
                ));
            }
        }
        Some(Self {
            tabs,
            panes,
            files,
            pending,
            bad_session: None,
        })
    }
}

/// What a restored files pane shows (spec 0013, Saved): its SSH profile
/// if that's still one, disconnected, else Local at home, with a notice.
pub fn restored_files(
    config: &Config,
    saved: &SavedFiles,
    notices: &mut Vec<String>,
) -> (Target, Option<String>) {
    match &saved.target {
        None => (Target::Local, saved.folder.clone()),
        Some(id) => match config.profile(id).map(|profile| profile.kind) {
            Some(ProfileKind::Ssh(_)) => (Target::Ssh(id.clone()), saved.folder.clone()),
            _ => {
                notices.push(format!(
                    "A restored files pane's host `{id}` no longer exists, so it shows Local."
                ));
                (Target::Local, None)
            }
        },
    }
}

/// The profile a restored pane names, or `default_profile`, with a notice,
/// if that profile no longer exists (spec 0003, flow 6).
pub fn restored_profile(config: &Config, id: &str, notices: &mut Vec<String>) -> Profile {
    config.profile(id).unwrap_or_else(|| {
        let instead = config.default_profile();
        notices.push(format!(
            "A restored pane's profile `{id}` no longer exists, so it opened `{}`.",
            instead.display_name()
        ));
        instead
    })
}

/// The settings screen while it's open (spec 0009).
struct SettingsScreen {
    view: Entity<SettingsView>,
    _events: Subscription,
}

/// The palette while it's open.
struct OpenPaletteView {
    view: Entity<Palette>,
    _events: Subscription,
    /// The default theme as saved, while the theme picker previews others.
    saved_theme: Option<Theme>,
}

pub struct Workspace {
    tabs: Tabs,
    panes: HashMap<LeafId, Entity<TerminalPane>>,
    pane_subscriptions: HashMap<LeafId, Subscription>,
    config: Config,
    /// The configured font, its family checked (spec 0005).
    configured_font: FontSettings,
    /// Pixels the font-size commands added to the configured size. Not
    /// saved: a restart or `font.reset` goes back to the config.
    zoom: f32,
    /// What every pane draws with: the configured font, zoomed.
    font: FontSettings,
    /// `theme.default`, resolved.
    theme: Theme,
    palette: Option<OpenPaletteView>,
    /// The settings screen while it's open (spec 0009).
    settings: Option<SettingsScreen>,
    /// The themes profiles name, resolved so far, by the name as written.
    themes: HashMap<String, Theme>,
    /// The OS is in light mode, which `system` follows. Dark until the
    /// first frame is on screen (spec 0005).
    light: bool,
    /// Kept alive so config edits keep arriving.
    watcher: Option<config::Watcher>,
    /// What waits for the first frame has run.
    booted: bool,
    timing: Arc<Timing>,
    bench: bool,
    /// Boot notices, shown once the first frame is on screen.
    notices: Vec<String>,
    /// TTFP for the dev badge, once the first frame was drawn.
    ttfp: Option<f64>,
    /// "There is no keychain" was said; it's said once (spec 0006, flow 8).
    keychain_missing: bool,
    rename: Option<Rename>,
    rename_subscription: Option<Subscription>,
    /// A mouse press on the title bar's empty area that may become a move.
    title_bar_pressed: bool,
    /// The tab content's size at the last layout, for the split-room check.
    content_size: Rc<Cell<Option<Size<Pixels>>>>,
    /// The pane side a dragged tab or pane would land on now (spec 0007).
    /// Drawn only while a drag is on.
    drop_target: Option<(LeafId, Direction)>,
    /// Restored panes whose tab hasn't been shown yet (spec 0003, flow 6).
    pending: HashMap<LeafId, SavedLeaf>,
    /// Where the session is saved; `None` in bench runs, and once the last
    /// save is done.
    session_path: Option<PathBuf>,
    window_size: Option<WindowSize>,
    /// The next save, waiting for the layout to settle.
    save_task: Option<Task<()>>,
    /// A failed save was reported; the next failure isn't, until a save
    /// works again.
    save_failed: bool,
    /// A session file that couldn't be restored, set aside before anything
    /// is saved over it (spec 0003, flow 7).
    bad_session: Option<PathBuf>,
    /// The daily update check while `update_check: daily` (spec 0011).
    update_timer: Option<Task<()>>,
    /// This run asked whether to check for updates; it asks once.
    update_asked: bool,
    /// The SFTP sidebars of SSH panes, by pane (spec 0008). Only the
    /// active pane's shows, if it's open.
    sftp: HashMap<LeafId, Sidebar>,
    /// The files panes, by leaf (spec 0013); every other leaf is a
    /// terminal pane.
    files: HashMap<LeafId, FilesLeaf>,
    /// The sidebars' width, one for the window, until Tern quits.
    sftp_width: f32,
    /// A close the user chose "Upload and close" for, waiting on the
    /// uploads (spec 0008, flow 3).
    closing: Option<Closing>,
    /// The window may close: nothing to ask, or the user answered.
    window_may_close: bool,
}

impl Workspace {
    /// A workspace showing `boot`, which saves its session to
    /// `session_path` (spec 0003).
    #[allow(clippy::too_many_arguments, reason = "one call, from the boot path")]
    pub fn new(
        boot: Boot,
        config: Config,
        font: FontSettings,
        theme: Theme,
        notices: Vec<String>,
        session_path: Option<PathBuf>,
        timing: Arc<Timing>,
        bench: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Boot {
            tabs,
            panes,
            files,
            pending,
            bad_session,
        } = boot;
        let themes = panes
            .iter()
            .filter_map(|(_, pane)| Some((pane.profile.theme.clone()?, pane.theme)))
            .collect();
        let mut workspace = Self {
            tabs,
            panes: HashMap::new(),
            pane_subscriptions: HashMap::new(),
            config,
            configured_font: font.clone(),
            zoom: 0.0,
            font,
            theme,
            palette: None,
            settings: None,
            themes,
            light: false,
            watcher: None,
            booted: false,
            timing,
            bench,
            notices,
            ttfp: None,
            keychain_missing: false,
            rename: None,
            rename_subscription: None,
            title_bar_pressed: false,
            content_size: Rc::default(),
            drop_target: None,
            pending,
            session_path,
            window_size: Some(window_size(window)),
            save_task: None,
            save_failed: false,
            bad_session,
            update_timer: None,
            update_asked: false,
            sftp: HashMap::new(),
            files: HashMap::new(),
            sftp_width: SFTP_WIDTH,
            closing: None,
            window_may_close: false,
        };
        // The compositor's close asks first too, if SFTP edits would go.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |workspace, cx| {
                if workspace.window_may_close {
                    return true;
                }
                let edits = workspace.edits_of(Closing::Window, cx);
                if edits.is_empty() {
                    return true;
                }
                workspace.ask_close(Closing::Window, edits, window, cx);
                false
            })
            .unwrap_or(true)
        });
        for (leaf, pane) in panes {
            let BootPane {
                start,
                launch,
                profile,
                theme,
                title,
            } = pane;
            workspace.add_pane(leaf, start, launch, profile, theme, window, cx);
            workspace.restore_title(leaf, title.as_deref(), cx);
        }
        for (leaf, saved) in files {
            let mut notices = Vec::new();
            let (target, folder) = restored_files(&workspace.config, &saved, &mut notices);
            workspace.notices.extend(notices);
            workspace.add_files(leaf, target, folder, false, window, cx);
        }
        workspace.focus_active(window, cx);
        if workspace.session_path.is_some() {
            cx.observe_window_bounds(window, |this, window, cx| {
                this.window_size = Some(window_size(window));
                this.save_soon(window, cx);
            })
            .detach();
            // Closing the window releases the workspace: the last chance
            // to save (spec 0003, flow 5).
            cx.on_release(|this, cx| this.save_on_exit(cx)).detach();
        }
        if !bench {
            // A program that prints nothing never makes the first frame
            // with output; what waits for that frame starts anyway.
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(SILENT_BOOT_WAIT).await;
                let _ = this.update_in(cx, |this, window, cx| this.after_first_frame(window, cx));
            })
            .detach();
        }
        workspace
    }

    #[allow(clippy::too_many_arguments, reason = "a pane's parts, used once each")]
    fn add_pane(
        &mut self,
        leaf: LeafId,
        start: Start,
        launch: Launch,
        profile: PaneProfile,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (settings, bench) = (self.pane_settings(theme), self.bench);
        let pane =
            cx.new(|cx| TerminalPane::new(start, launch, profile, settings, bench, window, cx));
        let subscription = cx.subscribe_in(&pane, window, move |this, _, event, window, cx| {
            this.pane_event(leaf, event, window, cx);
        });
        self.panes.insert(leaf, pane);
        self.pane_subscriptions.insert(leaf, subscription);
    }

    /// Puts a files pane on `target` at `folder` in `leaf` (spec 0013).
    /// Without `connect` it waits: see [`FilesPane::new`].
    fn add_files(
        &mut self,
        leaf: LeafId,
        target: Target,
        folder: Option<String>,
        connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let config = &self.config;
        let pane = cx.new(|cx| FilesPane::new(target, folder, connect, config, window, cx));
        let events = cx.subscribe_in(&pane, window, move |this, _, event, window, cx| {
            this.files_event(leaf, event, window, cx);
        });
        self.files.insert(
            leaf,
            FilesLeaf {
                pane,
                _events: events,
            },
        );
    }

    fn files_event(
        &mut self,
        leaf: LeafId,
        event: &FilesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A tab with a terminal waits for its output (spec 0001).
            FilesEvent::FirstFrame => {
                if self.panes.is_empty() {
                    self.first_frame(window, cx);
                }
            }
            FilesEvent::Focused => {
                if self.tabs.focus(leaf) {
                    cx.notify();
                }
            }
            FilesEvent::TitleChanged => {
                self.save_soon(window, cx);
                cx.notify();
            }
            FilesEvent::DialogChanged => {
                if leaf == self.tabs.active().focused
                    && self.palette.is_none()
                    && self.rename.is_none()
                    && self.settings.is_none()
                {
                    self.focus_active(window, cx);
                }
                cx.notify();
            }
            FilesEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
            FilesEvent::NoKeychain => {
                if !std::mem::replace(&mut self.keychain_missing, true) {
                    show_notice(secrets::UNAVAILABLE.into(), window, cx);
                }
            }
            FilesEvent::Settled => self.finish_closing(window, cx),
            FilesEvent::Run(file) => self.run_file(leaf, file, window, cx),
        }
    }

    /// Splits files pane `leaf` to the right with a local pane running
    /// `file` in its folder, so its output and exit show (spec 0013, flow
    /// 4). Without room, it says so and runs nothing.
    fn run_file(&mut self, leaf: LeafId, file: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.active().root.contains(leaf) {
            return;
        }
        self.tabs.focus(leaf);
        if !self.has_room(Axis::Row, window) {
            show_notice(
                "There's no room beside the files pane to run it".into(),
                window,
                cx,
            );
            return;
        }
        let name = file.file_name().map_or_else(
            || file.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let launch = Launch {
            command: local::run_command(file),
            cwd: file.parent().map(Path::to_path_buf),
            timing: Arc::clone(&self.timing),
        };
        let theme = self.theme;
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.font.estimated_cell(),
        );
        // Saved as the default profile in the file's folder: a restore
        // opens a shell there, never runs the file again.
        let profile = PaneProfile {
            id: self.config.default_profile().id,
            name,
            theme: None,
            ssh: false,
            stays: true,
        };
        let new = self.tabs.split(Axis::Row);
        self.add_pane(new, Start::Now(grid), launch, profile, theme, window, cx);
        self.show_active(window, cx);
    }

    /// Opens a files pane (spec 0013, flow 1): the tab's own if it has one;
    /// else beside the focused pane, on its target: an SSH pane's host, a
    /// local pane's folder. Without room, it says so.
    fn open_files(&mut self, _: &OpenFiles, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tabs.active();
        if let Some(leaf) = tab
            .root
            .leaves()
            .into_iter()
            .find(|leaf| self.files.contains_key(leaf))
        {
            self.tabs.focus(leaf);
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let focused = tab.focused;
        let (target, folder) = match self.panes.get(&focused) {
            Some(pane) => {
                let pane = pane.read(cx);
                let profile = pane.profile();
                if profile.ssh {
                    (Target::Ssh(profile.id.clone()), None)
                } else {
                    let folder = pane.cwd().and_then(|cwd| cwd.to_str().map(str::to_owned));
                    (Target::Local, folder)
                }
            }
            None => return self.open_files_in_new_tab(&OpenFilesInNewTab, window, cx),
        };
        if !self.has_room(Axis::Row, window) {
            show_notice("There's no room for a files pane here".into(), window, cx);
            return;
        }
        let leaf = self.tabs.split(Axis::Row);
        self.add_files(leaf, target, folder, true, window, cx);
        self.show_active(window, cx);
    }

    /// Lists the focused files pane's targets in the palette, so the
    /// keyboard switches them as the header's dropdown does (spec 0013,
    /// flow 2).
    fn switch_files_target(
        &mut self,
        _: &SwitchFilesTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(files) = self.files.get(&self.tabs.active().focused) else {
            return;
        };
        let entries = files
            .pane
            .read(cx)
            .choices()
            .into_iter()
            .map(|choice| Entry::Files {
                host: ssh::Host {
                    id: String::new(),
                    label: choice.label.to_string(),
                    group: choice.group,
                    destination: String::new(),
                },
                pick: choice.pick,
                current: choice.current,
            })
            .collect();
        self.show_palette(None, window, cx, |window, cx| {
            Palette::new(entries, "Show Local or a host", None, window, cx)
        });
    }

    /// A tab holding one files pane on Local at home (spec 0013, flow 1).
    fn open_files_in_new_tab(
        &mut self,
        _: &OpenFilesInNewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaf = self.tabs.new_tab();
        self.add_files(leaf, Target::Local, None, true, window, cx);
        self.show_active(window, cx);
    }

    /// Shows a restored pane's last title until its program sets one.
    fn restore_title(&self, leaf: LeafId, title: Option<&str>, cx: &mut Context<Self>) {
        if let (Some(pane), Some(_)) = (self.panes.get(&leaf), title) {
            pane.update(cx, |pane, _| pane.set_title(title));
        }
    }

    /// Starts the shown tab's restored panes that haven't run yet: its tab
    /// is shown for the first time, or was dragged into view (spec 0003,
    /// flow 6; spec 0007, flow 7). SSH panes wait for Enter.
    fn start_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for leaf in self.tabs.active().root.leaves() {
            let Some(saved) = self.pending.remove(&leaf) else {
                continue;
            };
            // Shown now, after the first frame: Local lists its folder.
            if let Some(files) = &saved.files {
                let mut notices = Vec::new();
                let (target, folder) = restored_files(&self.config, files, &mut notices);
                for notice in notices {
                    show_notice(notice, window, cx);
                }
                self.add_files(leaf, target, folder, false, window, cx);
                if let Some(files) = self.files.get(&leaf) {
                    files.pane.update(cx, |pane, cx| pane.start(window, cx));
                }
                continue;
            }
            let mut notices = Vec::new();
            let profile = restored_profile(&self.config, &saved.profile, &mut notices);
            for notice in notices {
                show_notice(notice, window, cx);
            }
            let cwd = saved
                .cwd
                .clone()
                .or_else(|| profile.cwd().map(Path::to_path_buf));
            if matches!(profile.kind, ProfileKind::Ssh(_)) {
                let launch = Launch {
                    command: Command::of_profile(&profile),
                    cwd,
                    timing: Arc::clone(&self.timing),
                };
                let theme = self.theme_for(profile.theme.as_deref(), window, cx);
                let start = Start::OnEnter(connect_prompt(profile.display_name()));
                self.add_pane(leaf, start, launch, (&profile).into(), theme, window, cx);
            } else {
                self.spawn_pane(leaf, profile, cwd, window, cx);
            }
            self.restore_title(leaf, saved.title.as_deref(), cx);
        }
    }

    /// Saves the session once the layout has stayed put for a second (spec
    /// 0003, flow 5), writing off the UI thread. Not in bench runs, nor with
    /// `restore_session: false`.
    fn save_soon(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(path) = self.session_path.clone() else {
            return;
        };
        if !self.config.restore_session {
            return;
        }
        // Replacing the waiting save cancels it.
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let Ok(session) = this.update(cx, |this, cx| {
                this.set_aside_bad_session();
                this.snapshot(cx)
            }) else {
                return;
            };
            let saved = cx
                .background_spawn(async move { session::save(&path, &session) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match saved {
                Ok(()) => this.save_failed = false,
                Err(error) if !std::mem::replace(&mut this.save_failed, true) => {
                    show_notice(
                        format!("The session couldn't be saved: {error}"),
                        window,
                        cx,
                    );
                }
                Err(_) => {}
            });
        }));
    }

    /// The last save, as the window closes: the session as it stands
    /// (spec 0003, flow 5). Then every program stops, so no shell outlives
    /// the window (spec 0002, flow 10).
    fn save_on_exit(&mut self, cx: &mut App) {
        let session = self.snapshot(cx);
        self.save_last(session);
        for pane in self.panes.values() {
            pane.update(cx, |pane, _| pane.shutdown());
        }
    }

    /// Writes `session` as the last save, or deletes the saved one with
    /// `restore_session: false`. Once only, right away: nothing runs after.
    fn save_last(&mut self, session: Session) {
        let Some(path) = self.session_path.take() else {
            return;
        };
        self.save_task = None;
        self.set_aside_bad_session();
        let result = if self.config.restore_session {
            session::save(&path, &session)
        } else {
            session::delete(&path)
        };
        if let Err(error) = result {
            eprintln!("tern: the session couldn't be saved: {error}");
        }
    }

    /// The session as it stands: every tab's layout with each pane's
    /// profile, directory and title (spec 0003). Never what a pane shows.
    fn snapshot(&self, cx: &App) -> Session {
        let tabs = self
            .tabs
            .tabs()
            .iter()
            .map(|tab| SavedTab {
                name: tab.name.clone(),
                root: SavedNode::of(&tab.root, &mut |leaf| self.saved_leaf(leaf, cx)),
                focused: tab.focused,
                zoomed: tab.zoomed,
            })
            .collect();
        Session::new(self.window_size, tabs, self.tabs.active_index())
    }

    fn saved_leaf(&self, leaf: LeafId, cx: &App) -> SavedLeaf {
        if let Some(saved) = self.pending.get(&leaf) {
            return saved.clone();
        }
        if let Some(files) = self.files.get(&leaf) {
            let (target, folder) = files.pane.read(cx).saved(cx);
            return SavedLeaf {
                id: leaf,
                profile: self.config.default_profile().id,
                cwd: None,
                title: None,
                files: Some(SavedFiles { target, folder }),
            };
        }
        let (profile, cwd, title) = match self.panes.get(&leaf) {
            Some(pane) => {
                let pane = pane.read(cx);
                let title = pane.title().map(str::to_owned);
                (pane.profile().id.clone(), pane.cwd(), title)
            }
            None => (self.config.default_profile().id, None, None),
        };
        SavedLeaf {
            id: leaf,
            profile,
            cwd,
            title,
            files: None,
        }
    }

    /// What a pane with `theme` takes from the current config.
    fn pane_settings(&self, theme: Theme) -> PaneSettings {
        PaneSettings {
            font: self.font.clone(),
            theme,
            close_on_exit: self.config.close_on_exit,
            scrollback: self.config.scrollback as usize,
            copy_on_select: self.config.copy_on_select,
            cursor: self.config.cursor,
            open_links: self.config.open_links,
        }
    }

    /// Starts `profile` for `leaf` in `cwd`, else the profile's own cwd,
    /// else the home directory (spec 0005, flow 2).
    fn spawn_pane(
        &mut self,
        leaf: LeafId,
        profile: Profile,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launch = Launch {
            command: Command::of_profile(&profile),
            cwd: cwd.or_else(|| profile.cwd().map(Path::to_path_buf)),
            timing: Arc::clone(&self.timing),
        };
        let theme = self.theme_for(profile.theme.as_deref(), window, cx);
        let viewport = window.viewport_size();
        let grid = GridSize::fit(
            (f32::from(viewport.width), f32::from(viewport.height)),
            self.font.estimated_cell(),
        );
        let profile = PaneProfile::from(&profile);
        self.add_pane(leaf, Start::Now(grid), launch, profile, theme, window, cx);
    }

    /// The palette of a pane whose profile names theme `name` (`None`:
    /// `theme.default`). A name met for the first time is resolved now,
    /// which reads its theme file if it has one: a small read, next to the
    /// fork of the pane's program.
    fn theme_for(
        &mut self,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Theme {
        let Some(name) = name else {
            return self.theme;
        };
        if let Some(theme) = self.themes.get(name) {
            return *theme;
        }
        let mut notices = Vec::new();
        let dir = config::themes_dir();
        let library = theme::Library::new(&self.config.theme.custom, dir.as_deref());
        let theme = library.resolve(theme::effective(name, self.light), &mut notices);
        self.themes.insert(name.to_owned(), theme);
        for notice in notices {
            show_notice(notice, window, cx);
        }
        theme
    }

    fn pane_event(
        &mut self,
        leaf: LeafId,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::FirstFrame => self.first_frame(window, cx),
            PaneEvent::Focused => {
                if self.tabs.focus(leaf) {
                    cx.notify();
                }
            }
            PaneEvent::TitleChanged => {
                self.save_soon(window, cx);
                cx.notify();
            }
            PaneEvent::Exited => self.close(Closing::Pane(leaf), window, cx),
            // The keyboard follows a prompt dialog of the focused pane in and
            // out, unless the palette or a rename has it.
            PaneEvent::DialogChanged => {
                if leaf == self.tabs.active().focused
                    && self.palette.is_none()
                    && self.rename.is_none()
                    && self.settings.is_none()
                {
                    self.focus_active(window, cx);
                }
                cx.notify();
            }
            PaneEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
            PaneEvent::NoKeychain => {
                if !std::mem::replace(&mut self.keychain_missing, true) {
                    show_notice(secrets::UNAVAILABLE.into(), window, cx);
                }
            }
        }
    }

    /// Records and prints TTFP (spec 0001), then runs what waits for the
    /// first frame. In bench mode, quits instead. Later panes' first frames
    /// change nothing: the marks are set once.
    fn first_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.timing.mark(Mark::Rendered) {
            return;
        }
        let report = self.timing.report();
        if let Ok(json) = serde_json::to_string(&report) {
            println!("{json}");
        }
        if self.bench {
            cx.quit();
            return;
        }
        self.ttfp = report.rendered;
        self.after_first_frame(window, cx);
    }

    /// What waits for the first frame (spec 0001): the boot notices,
    /// config reloads and the update check (spec 0011). Runs once, at the
    /// first frame with output, or [`SILENT_BOOT_WAIT`] after boot if the
    /// program prints nothing.
    fn after_first_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.booted, true) {
            return;
        }
        for notice in std::mem::take(&mut self.notices) {
            show_notice(notice, window, cx);
        }
        self.set_aside_bad_session();
        // A restored files pane lists Local now (spec 0013, Startup
        // impact).
        for files in self.files.values() {
            files.pane.update(cx, |pane, cx| pane.start(window, cx));
        }
        self.start_reloads(window, cx);
        self.follow_update_check(window, cx);
        // Edit copies a crashed run left (spec 0008, Model), off the UI
        // thread and never before the first frame.
        if !self.bench {
            cx.background_executor()
                .spawn(async { sftp::copies::sweep() })
                .detach();
        }
        cx.notify();
    }

    /// Renames a session that couldn't be restored to `session.json.bad`,
    /// once: after the first frame, or before a save that comes first.
    fn set_aside_bad_session(&mut self) {
        if let Some(path) = self.bad_session.take()
            && let Err(error) = session::keep_bad(&path)
        {
            eprintln!("tern: the unrestorable session couldn't be set aside: {error}");
        }
    }

    /// Reloads the config whenever `config.yaml` or a theme file changes,
    /// and whenever the OS switches between light and dark while a theme in
    /// use is `system` (spec 0005). Starts after the first frame.
    fn start_reloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (reload, mut reloads) = unbounded();
        if let Some(dir) = config::default_dir() {
            match config::Watcher::start(dir, reload.clone()) {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(error) => show_notice(
                    format!("Config changes apply after a restart: {error}"),
                    window,
                    cx,
                ),
            }
        }
        self.light = is_light(window.appearance());
        if self.light && self.uses_system() {
            let _ = reload.unbounded_send(());
        }
        cx.observe_window_appearance(window, move |this, window, _| {
            let light = is_light(window.appearance());
            if light != this.light {
                this.light = light;
                if this.uses_system() {
                    let _ = reload.unbounded_send(());
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            while reloads.next().await.is_some() {
                cx.background_executor().timer(RELOAD_DEBOUNCE).await;
                while reloads.try_recv().is_ok() {}
                let Ok(job) = this.update(cx, |this, _| this.reload_job()) else {
                    break;
                };
                let reloaded = cx.background_spawn(async move { job.run() }).await;
                let applied = this.update_in(cx, |this, window, cx| {
                    this.apply_reload(reloaded, window, cx);
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Whether `system` decides any theme on screen.
    fn uses_system(&self) -> bool {
        self.config.theme.default == "system" || self.themes.contains_key("system")
    }

    /// What a reload reads: the config file, then every theme the new
    /// config's profiles and the open panes name.
    fn reload_job(&self) -> ReloadJob {
        ReloadJob {
            light: self.light,
            pane_themes: self.themes.keys().cloned().collect(),
        }
    }

    /// Swaps in a reloaded config: font, themes, scrollback and
    /// `close_on_exit` change in every open pane, while profiles apply to
    /// panes opened from now on (spec 0005, flow 3).
    fn apply_reload(&mut self, reloaded: Reloaded, window: &mut Window, cx: &mut Context<Self>) {
        let Reloaded {
            config,
            theme,
            themes,
            mut notices,
        } = reloaded;
        if let Some(watcher) = &mut self.watcher {
            watcher.arm();
        }
        let (font, font_notice) = FontSettings::checked(&config.font, cx);
        notices.extend(font_notice);
        notices.extend(commands::keymap::bind(
            &config.keybindings,
            config.new_tab,
            cx,
        ));
        self.config = config;
        self.configured_font = font;
        self.font = self.configured_font.zoomed(self.zoom);
        self.themes = themes;
        // While the theme picker previews, the new theme waits for its Esc.
        match self
            .palette
            .as_mut()
            .and_then(|open| open.saved_theme.as_mut())
        {
            Some(saved) => *saved = theme,
            None => self.theme = theme,
        }
        self.apply_settings(cx);
        if let Some(screen) = &self.settings {
            let config = self.config.clone();
            screen
                .view
                .update(cx, |view, cx| view.set_config(config, window, cx));
        }
        for notice in notices {
            show_notice(notice, window, cx);
        }
        self.follow_update_check(window, cx);
    }

    /// Hands the current font, themes and config to the widgets and every
    /// pane.
    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        theme::apply_to_widgets(&self.theme, cx);
        let settings = BrowserSettings::of(&self.config);
        for sidebar in self.sftp.values() {
            sidebar
                .view
                .update(cx, |view, _| view.set_settings(settings));
        }
        for files in self.files.values() {
            let config = &self.config;
            files
                .pane
                .update(cx, |pane, cx| pane.set_config(config, cx));
        }
        for pane in self.panes.values() {
            let name = pane.read(cx).profile().theme.clone();
            let theme = name
                .and_then(|name| self.themes.get(&name).copied())
                .unwrap_or(self.theme);
            let settings = self.pane_settings(theme);
            pane.update(cx, |pane, cx| pane.apply(settings, cx));
        }
        cx.notify();
    }

    /// Opens the command palette over the focused pane, or closes it if
    /// it's open (spec 0004).
    fn open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(false, window, cx);
            return;
        }
        let commands = commands::COMMANDS.iter().filter_map(|command| {
            Some(Entry::Command {
                command,
                title: command.title?,
                enabled: self.can_run(command.id, window, cx),
            })
        });
        let profiles = self.profiles().into_iter().map(|profile| Entry::Profile {
            title: format!("Open profile: {}", profile.display_name()),
            id: profile.id,
        });
        let entries = commands.chain(profiles).collect();
        self.show_palette(None, window, cx, |window, cx| {
            Palette::new(entries, "Run a command or open a profile", None, window, cx)
        });
    }

    /// Opens the host picker to connect to a host (spec 0006, flow 1).
    fn connect_to_host(&mut self, _: &ConnectToHost, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_host(Vec::new(), Entry::Host, "Connect to a host", window, cx);
    }

    /// Opens the host picker to forget what's saved for a host (spec 0006,
    /// flow 7).
    fn forget_password(&mut self, _: &ForgetPassword, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_host(
            Vec::new(),
            Entry::ForgetHost,
            "Forget the password saved for a host",
            window,
            cx,
        );
    }

    /// Opens the host picker, with the local profiles first, to split the
    /// focused pane along `axis` with the one picked (spec 0007, flow 6).
    /// Without room for a split it doesn't open.
    fn pick_split_profile(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_room(axis, window) {
            return;
        }
        let placeholder = match axis {
            Axis::Row => "Split right with a profile or host",
            Axis::Column => "Split down with a profile or host",
        };
        let entry = move |host| Entry::SplitWith { host, axis };
        self.pick_host(self.local_hosts(), entry, placeholder, window, cx);
    }

    /// Asks which profile or host a new tab opens, the default profile
    /// first so Enter opens it (spec 0004, `Ctrl+Shift+T`).
    fn new_tab_with_profile(
        &mut self,
        _: &NewTabWithProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = self.local_hosts();
        self.pick_host(
            local,
            Entry::Host,
            "Open a profile or host in a new tab",
            window,
            cx,
        );
    }

    /// The local profiles as picker lines in group "Local", the default
    /// profile first.
    fn local_hosts(&self) -> Vec<ssh::Host> {
        let default = self.config.default_profile().id;
        let mut local: Vec<ssh::Host> = self
            .profiles()
            .into_iter()
            .filter(|profile| matches!(profile.kind, ProfileKind::Local(_)))
            .map(|profile| ssh::Host {
                label: profile.display_name().to_owned(),
                group: LOCAL_GROUP.into(),
                destination: String::new(),
                id: profile.id,
            })
            .collect();
        local.sort_by_key(|host| host.id != default);
        local
    }

    /// Opens profile or host `id` in a new tab.
    fn open_profile_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.config.profile_or_default(id);
        let leaf = self.tabs.new_tab();
        self.spawn_pane(leaf, profile, None, window, cx);
        self.save_soon(window, cx);
    }

    /// Splits the focused pane along `axis` with profile or host `id`.
    fn split_with(&mut self, id: &str, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.config.profile_or_default(id);
        self.split(axis, Some(profile), window, cx);
    }

    /// Opens the host picker over `first`, then the SSH profiles and the
    /// hosts of `~/.ssh/config`, which is read each time, off the UI thread,
    /// so its edits show. `entry` says what picking a host does. A picker
    /// over local profiles that would list only the default profile runs it
    /// without asking (spec 0004).
    fn pick_host(
        &mut self,
        first: Vec<ssh::Host>,
        entry: impl Fn(ssh::Host) -> Entry + 'static,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profiles = self.config.profiles.clone();
        let only_default = first.len() == 1;
        let found = cx.background_spawn(async move {
            let path = ssh::config_path().unwrap_or_default();
            ssh::hosts(&profiles, &path)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (hosts, notices) = found.await;
            let _ = this.update_in(cx, |this, window, cx| {
                for notice in notices {
                    show_notice(notice, window, cx);
                }
                let mut entries: Vec<Entry> = first.into_iter().chain(hosts).map(entry).collect();
                if only_default && entries.len() == 1 {
                    match entries.pop() {
                        Some(Entry::Host(host)) => this.open_profile_tab(&host.id, window, cx),
                        Some(Entry::SplitWith { host, axis }) => {
                            this.split_with(&host.id, axis, window, cx)
                        }
                        _ => {}
                    }
                    return;
                }
                this.show_palette(None, window, cx, |window, cx| {
                    Palette::new(entries, placeholder, None, window, cx).with_empty(
                        "No hosts yet: add `type: ssh` profiles to config.yaml, \
                         or hosts to ~/.ssh/config",
                    )
                });
            });
        })
        .detach();
    }

    /// Forgets the password and key passphrases saved for host `id`, with
    /// a notice saying what was forgotten. Only this asks ssh where the host
    /// connects, besides connecting (TM-askpass-20), and it runs off the UI
    /// thread.
    fn forget_host(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.config.profile(id) else {
            return;
        };
        let ProfileKind::Ssh(remote) = &profile.kind else {
            return;
        };
        let args = ssh::command(remote).args;
        let label = profile.display_name().to_owned();
        let forgotten = secrets::off_thread(move || ssh::forget(&args));
        cx.spawn_in(window, async move |this, cx| {
            let notice = match forgotten.await {
                Ok(Ok(keys)) if keys.is_empty() => format!("Nothing was saved for {label}."),
                Ok(Ok(keys)) => {
                    let described: Vec<String> = keys.iter().map(secrets::Key::describe).collect();
                    format!("Forgot {}.", described.join(", "))
                }
                Ok(Err(error)) => format!("Nothing was forgotten for {label}: {error}"),
                Err(_) => format!("Nothing was forgotten for {label}."),
            };
            let _ = this.update_in(cx, |_, window, cx| show_notice(notice, window, cx));
        })
        .detach();
    }

    /// The profiles the palette offers: `shell` and the configured ones
    /// (spec 0005).
    fn profiles(&self) -> Vec<Profile> {
        self.config.all_profiles()
    }

    /// Opens the theme picker: moving through it previews each theme, Enter
    /// saves one as `theme.default`, Esc goes back (spec 0005, flow 4).
    fn select_theme(&mut self, _: &SelectTheme, window: &mut Window, cx: &mut Context<Self>) {
        let dir = config::themes_dir();
        let mut notices = Vec::new();
        let names = theme::list(&self.config.theme.custom, dir.as_deref(), &mut notices);
        for notice in notices {
            show_notice(notice, window, cx);
        }
        let current = self.config.theme.default.clone();
        let highlight = names.iter().position(|name| *name == current);
        let entries = names
            .into_iter()
            .map(|name| Entry::Theme {
                current: name == current,
                name,
            })
            .collect();
        self.show_palette(Some(self.theme), window, cx, |window, cx| {
            Palette::new(entries, "Select a theme", highlight, window, cx)
        });
    }

    /// Shows a palette, replacing one that's open: a command in the
    /// palette can open the theme picker.
    /// `saved_theme` is the theme to go back to when the palette previews
    /// themes.
    fn show_palette(
        &mut self,
        saved_theme: Option<Theme>,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(&mut Window, &mut Context<Palette>) -> Palette,
    ) {
        if let Some(saved) = self.palette.take().and_then(|open| open.saved_theme) {
            self.theme = saved;
            self.apply_settings(cx);
        }
        let view = cx.new(|cx| build(window, cx));
        let events = cx.subscribe_in(&view, window, Self::palette_event);
        self.palette = Some(OpenPaletteView {
            view,
            _events: events,
            saved_theme,
        });
        cx.notify();
    }

    fn palette_event(
        &mut self,
        palette: &Entity<Palette>,
        event: &PaletteEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .palette
            .as_ref()
            .is_none_or(|open| open.view != *palette)
        {
            return;
        }
        match event {
            PaletteEvent::OpenProfile(id) => self.open_profile_tab(id, window, cx),
            PaletteEvent::ForgetHost(id) => self.forget_host(id, window, cx),
            PaletteEvent::SplitWith { id, axis } => self.split_with(id, *axis, window, cx),
            PaletteEvent::PreviewTheme(name) => {
                self.theme = self.theme_for(Some(name), window, cx);
                self.apply_settings(cx);
            }
            PaletteEvent::Change(change) => self.write_change(change.clone(), window, cx),
            PaletteEvent::Files(pick) => {
                let focused = self.tabs.active().focused;
                if let Some(files) = self.files.get(&focused) {
                    let pick = pick.clone();
                    files
                        .pane
                        .update(cx, |pane, cx| pane.pick(pick, window, cx));
                }
            }
            PaletteEvent::ChooseTheme(name) => {
                if let Some(open) = &mut self.palette {
                    open.saved_theme = None;
                }
                self.save_theme(name.clone(), window, cx);
            }
            PaletteEvent::Closed { cancelled } => self.close_palette(*cancelled, window, cx),
        }
    }

    /// Closes the palette and gives the focus back to the active pane.
    /// Leaving the theme picker without choosing restores the saved theme.
    fn close_palette(&mut self, cancelled: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(saved) = self.palette.take().and_then(|open| open.saved_theme)
            && cancelled
        {
            self.theme = saved;
            self.apply_settings(cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Writes `name` as `theme.default` off the UI thread. The watcher
    /// then reloads the config, which applies it for good.
    fn save_theme(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = config::default_path() else {
            show_notice("This system has no config directory.".into(), window, cx);
            return;
        };
        let saved = cx.background_spawn(async move { config::save_theme_default(&path, &name) });
        cx.spawn_in(window, async move |this, cx| {
            let result = saved.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Saving may have created the config directory.
                if let Some(watcher) = &mut this.watcher {
                    watcher.arm();
                }
                if let Err(error) = result {
                    show_notice(format!("The theme wasn't saved: {error}"), window, cx);
                }
            });
        })
        .detach();
    }

    /// Whether command `id` would do anything now; the palette dims those
    /// that wouldn't (spec 0004, flow 3).
    fn can_run(&self, id: &str, window: &Window, cx: &App) -> bool {
        let tab = self.tabs.active();
        let tabs = self.tabs.tabs().len();
        let neighbor =
            |direction| tab.zoomed.is_none() && tab.root.neighbor(tab.focused, direction).is_some();
        let size = f32::from(self.font.size);
        match id {
            "tab.next" | "tab.previous" => tabs > 1,
            "pane.zoom" => tab.root.leaves().len() > 1,
            "pane.focusLeft" => neighbor(Direction::Left),
            "pane.focusRight" => neighbor(Direction::Right),
            "pane.focusUp" => neighbor(Direction::Up),
            "pane.focusDown" => neighbor(Direction::Down),
            "pane.copy" => self
                .panes
                .get(&tab.focused)
                .is_some_and(|pane| pane.read(cx).has_selection()),
            "pane.splitRight" | "pane.splitRightWithProfile" => self.has_room(Axis::Row, window),
            "pane.splitDown" | "pane.splitDownWithProfile" => self.has_room(Axis::Column, window),
            "files.switchTarget" => self.files.contains_key(&tab.focused),
            "font.increase" => size < *FONT_SIZES.end(),
            "font.decrease" => size > *FONT_SIZES.start(),
            "font.reset" => self.zoom != 0.0,
            id => id
                .strip_prefix("tab.goTo")
                .and_then(|n| n.parse::<usize>().ok())
                .is_none_or(|n| n <= tabs),
        }
    }

    /// Grows or shrinks every pane's font by `step` pixels; `None` goes back
    /// to the configured size (spec 0004). Not saved in the config.
    fn zoom_font(&mut self, step: Option<f32>, cx: &mut Context<Self>) {
        self.zoom = step.map_or(0.0, |step| self.zoom + step);
        self.font = self.configured_font.zoomed(self.zoom);
        // At the size limits the step didn't take effect; neither does it
        // count.
        self.zoom = f32::from(self.font.size) - f32::from(self.configured_font.size);
        self.apply_settings(cx);
    }

    /// Opens `config.yaml` with the OS's default handler, first writing the
    /// commented defaults if there is no config yet (spec 0005).
    fn open_config(&mut self, _: &OpenConfig, window: &mut Window, cx: &mut Context<Self>) {
        self.with_config_file(App::open_with_system, window, cx);
    }

    /// Shows `config.yaml` in the OS's file manager, creating it first like
    /// "Open config file" does (spec 0009).
    fn open_config_folder(
        &mut self,
        _: &OpenConfigFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_config_file(App::reveal_path, window, cx);
    }

    /// Makes sure `config.yaml` exists, off the UI thread, then hands its
    /// path to `open`.
    fn with_config_file(
        &mut self,
        open: fn(&App, &std::path::Path),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = config::default_path() else {
            show_notice("This system has no config directory.".into(), window, cx);
            return;
        };
        let created = cx.background_spawn({
            let path = path.clone();
            async move { config::ensure_exists(&path) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = created.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    if let Some(watcher) = &mut this.watcher {
                        watcher.arm();
                    }
                    open(cx, &path);
                }
                Err(error) => show_notice(
                    format!("config.yaml could not be created: {error}"),
                    window,
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Opens the settings screen over the window (spec 0009, flow 1).
    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_none() {
            let config = self.config.clone();
            let view = cx.new(|cx| SettingsView::new(config, window, cx));
            let events = cx.subscribe_in(&view, window, Self::settings_event);
            self.settings = Some(SettingsScreen {
                view,
                _events: events,
            });
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn settings_event(
        &mut self,
        _: &Entity<SettingsView>,
        event: &SettingsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SettingsEvent::Change(change) => self.write_change(change.clone(), window, cx),
            SettingsEvent::Pick(pick) => self.pick_setting(*pick, window, cx),
            SettingsEvent::Close => {
                self.settings = None;
                self.focus_active(window, cx);
                cx.notify();
            }
        }
    }

    /// Writes `change` into `config.yaml` off the UI thread; the watcher's
    /// reload applies it (spec 0009, flow 2). A failure says why.
    fn write_change(&mut self, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = config::default_path() else {
            show_notice("This system has no config directory.".into(), window, cx);
            return;
        };
        let saved = cx
            .background_spawn(async move { config::save_edit(&path, |text| change.edited(text)) });
        cx.spawn_in(window, async move |this, cx| {
            let result = saved.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Saving may have created the config directory.
                if let Some(watcher) = &mut this.watcher {
                    watcher.arm();
                }
                if let Err(error) = result {
                    show_notice(error, window, cx);
                }
            });
        })
        .detach();
    }

    /// Opens the palette as a picker for a setting the screen lists
    /// (spec 0009): fonts and default profiles here, themes as the theme
    /// picker does.
    fn pick_setting(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let (entries, placeholder): (Vec<Entry>, &str) = match pick {
            Pick::Theme => return self.select_theme(&SelectTheme, window, cx),
            Pick::Font => {
                let current = self.config.font.family.clone();
                let mut names = cx.text_system().all_font_names();
                names.sort();
                names.dedup();
                let bundled = Entry::Setting {
                    title: format!("{} (bundled)", crate::terminal::DEFAULT_FONT_FAMILY),
                    current: current.is_none(),
                    change: font_family(None),
                };
                let installed = names.into_iter().map(|name| Entry::Setting {
                    current: current.as_deref() == Some(name.as_str()),
                    change: font_family(Some(&name)),
                    title: name,
                });
                (
                    std::iter::once(bundled).chain(installed).collect(),
                    "Pick a font",
                )
            }
            Pick::DefaultProfile => {
                let current = self.config.default_profile().id;
                let builtin_shell = !self.config.configures(config::SHELL);
                let entries = self
                    .profiles()
                    .into_iter()
                    .map(|profile| Entry::Setting {
                        title: if builtin_shell && profile.id == config::SHELL {
                            format!("{} (built-in)", profile.display_name())
                        } else {
                            profile.display_name().to_owned()
                        },
                        current: profile.id == current,
                        change: Change::Set {
                            path: vec!["default_profile"],
                            value: settings::yaml::scalar(&profile.id),
                            reads: |config| settings::yaml::scalar(&config.default_profile().id),
                        },
                    })
                    .collect();
                (entries, "Pick the profile new tabs open")
            }
        };
        self.show_palette(None, window, cx, |window, cx| {
            Palette::new(entries, placeholder, None, window, cx)
        });
    }

    /// The settings screen, centered over the tab content.
    fn render_settings(&self) -> Option<impl IntoElement> {
        let screen = self.settings.as_ref()?;
        Some(
            div()
                .id("settings-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.3))
                // The panes below get no clicks, which would take the
                // keyboard from the screen's fields.
                .occlude()
                .child(
                    div()
                        .w(gpui::relative(0.9))
                        .max_w(px(760.))
                        .h(gpui::relative(0.9))
                        .child(screen.view.clone()),
                ),
        )
    }

    /// The shown tab's focused pane, which copy, paste and search act on.
    fn focused_pane(&self) -> Option<Entity<TerminalPane>> {
        self.panes.get(&self.tabs.active().focused).cloned()
    }

    /// Opens or closes the active pane's SFTP sidebar, starting its session
    /// the first time (spec 0008, flow 1). A local pane has none.
    fn toggle_sftp(&mut self, _: &ToggleSftp, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.active().focused;
        if let Some(sidebar) = self.sftp.get_mut(&leaf) {
            sidebar.open = !sidebar.open;
            if !sidebar.open {
                self.focus_active(window, cx);
            }
            cx.notify();
            return;
        }
        let pane = self.panes.get(&leaf).cloned();
        let Some(pane) = pane.filter(|pane| pane.read(cx).ssh_target().is_some()) else {
            show_notice("SFTP needs an SSH pane".into(), window, cx);
            return;
        };
        let settings = BrowserSettings::of(&self.config);
        let source = Source::Pane(pane.downgrade());
        let view = cx.new(|cx| Browser::new(source, None, true, settings, window, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                BrowserEvent::Close => {
                    if let Some(sidebar) = this.sftp.get_mut(&leaf) {
                        sidebar.open = false;
                    }
                    this.focus_active(window, cx);
                    cx.notify();
                }
                BrowserEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
                BrowserEvent::Settled => this.finish_closing(window, cx),
                // A remote file never runs.
                BrowserEvent::Run(_) => {}
            },
        );
        self.sftp.insert(
            leaf,
            Sidebar {
                view,
                open: true,
                _events: events,
            },
        );
        cx.notify();
    }

    /// The panes `closing` would close.
    fn closing_leaves(&self, closing: Closing) -> Vec<LeafId> {
        match closing {
            Closing::Pane(leaf) => vec![leaf],
            Closing::Tab(id) => self
                .tabs
                .tabs()
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.root.leaves())
                .unwrap_or_default(),
            Closing::Window => self
                .tabs
                .tabs()
                .iter()
                .flat_map(|tab| tab.root.leaves())
                .collect(),
        }
    }

    /// The SFTP edits `closing` would delete: each one's name, and whether
    /// its changes aren't on the host.
    fn edits_of(&self, closing: Closing, cx: &App) -> Vec<(SharedString, bool)> {
        let leaves = self.closing_leaves(closing);
        let sidebars = leaves
            .iter()
            .filter_map(|leaf| self.sftp.get(leaf))
            .flat_map(|sidebar| sidebar.view.read(cx).open_edits());
        let files = leaves
            .iter()
            .filter_map(|leaf| self.files.get(leaf))
            .flat_map(|files| files.pane.read(cx).open_edits(cx));
        sidebars.chain(files).collect()
    }

    /// Closes a pane, a tab or the window, asking first when that would
    /// delete SFTP edits (spec 0008, flow 3).
    fn close(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        let edits = self.edits_of(closing, cx);
        if edits.is_empty() {
            self.close_now(closing, window, cx);
        } else {
            self.ask_close(closing, edits, window, cx);
        }
    }

    fn close_now(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        match closing {
            Closing::Pane(leaf) => self.close_pane(leaf, window, cx),
            Closing::Tab(id) => {
                if let Some(ix) = self.tabs.tabs().iter().position(|tab| tab.id == id) {
                    self.close_tab(ix, window, cx);
                }
            }
            Closing::Window => {
                self.window_may_close = true;
                window.remove_window();
            }
        }
    }

    /// The question before a close deletes SFTP edits: it names them, calls
    /// out the ones whose changes aren't on the host, and offers to upload
    /// those first.
    fn ask_close(
        &mut self,
        closing: Closing,
        edits: Vec<(SharedString, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = cx.entity().downgrade();
        let unsent = edits.iter().any(|(_, unsent)| *unsent);
        let what = match closing {
            Closing::Pane(_) => "this pane",
            Closing::Tab(_) => "this tab",
            Closing::Window => "this window",
        };
        let lines: Vec<SharedString> = edits
            .iter()
            .map(|(name, unsent)| match unsent {
                true => format!("{name}: changed, not uploaded").into(),
                false => name.clone(),
            })
            .collect();
        window.open_dialog(cx, move |dialog, _, _| {
            let answer = |id: &'static str, label: &'static str, upload: Option<bool>| {
                let workspace = workspace.clone();
                Button::new(id)
                    .label(label)
                    .small()
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = workspace.update(cx, |workspace, cx| {
                            workspace.answer_close(closing, upload, window, cx);
                        });
                    })
            };
            dialog
                .title(format!("Close {what}?"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .text_sm()
                        .child("Its SFTP edit copies are deleted when it closes:")
                        .children(
                            lines
                                .iter()
                                .map(|line| div().pl(px(8.)).child(line.clone())),
                        ),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(6.))
                        .child(answer("close-cancel", "Cancel", None))
                        .child(answer("close-anyway", "Close", Some(false)).danger())
                        .when(unsent, |footer| {
                            footer.child(
                                answer("close-upload", "Upload and close", Some(true)).primary(),
                            )
                        }),
                )
        });
    }

    /// The answer to [`Self::ask_close`]: `None` cancels, `Some(true)`
    /// uploads the unsent edits and closes once they're on the host.
    fn answer_close(
        &mut self,
        closing: Closing,
        upload: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match upload {
            None => {}
            Some(false) => self.close_now(closing, window, cx),
            Some(true) => {
                self.closing = Some(closing);
                for leaf in self.closing_leaves(closing) {
                    if let Some(sidebar) = self.sftp.get(&leaf) {
                        sidebar.view.update(cx, |view, cx| view.upload_unsent(cx));
                    }
                    if let Some(files) = self.files.get(&leaf) {
                        files.pane.update(cx, |pane, cx| pane.upload_unsent(cx));
                    }
                }
                self.finish_closing(window, cx);
            }
        }
    }

    /// Goes on with a close that waited on uploads once none is running:
    /// it closes if the host has every change, else it stays open and says
    /// so, and the sidebar says why.
    fn finish_closing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(closing) = self.closing else {
            return;
        };
        let mut sent = true;
        for leaf in self.closing_leaves(closing) {
            let settled = match (self.sftp.get(&leaf), self.files.get(&leaf)) {
                (Some(sidebar), _) => sidebar.view.read(cx).settled(),
                (_, Some(files)) => files.pane.read(cx).settled(cx),
                (None, None) => continue,
            };
            match settled {
                None => return,
                Some(all) => sent &= all,
            }
        }
        self.closing = None;
        if sent {
            self.close_now(closing, window, cx);
        } else {
            let notice = "Not closed: an SFTP edit didn't upload; its file browser says why";
            show_notice(notice.into(), window, cx);
        }
    }

    /// The active pane's SFTP sidebar if it's open, with the inner edge it
    /// is resized by (spec 0008, Model).
    fn render_sftp(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sidebar = self
            .sftp
            .get(&self.tabs.active().focused)
            .filter(|sidebar| sidebar.open)?;
        let theme = cx.theme();
        let (background, border) = (theme.tab_bar, theme.border);
        let edge = div()
            .id("sftp-edge")
            .w(px(DIVIDER))
            .h_full()
            .flex_shrink_0()
            .bg(border)
            .cursor_col_resize()
            .on_drag(DraggedSftpEdge, |_, _, _, cx| cx.new(|_| DividerGhost));
        let body = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(sidebar.view.clone());
        let side = self.config.sftp_sidebar;
        Some(
            div()
                .id("sftp")
                .w(px(self.sftp_width))
                .h_full()
                .flex_shrink_0()
                .flex()
                .bg(background)
                .map(|sidebar| match side {
                    SftpSide::Left => sidebar.child(body).child(edge),
                    SftpSide::Right => sidebar.child(edge).child(body),
                })
                .on_drag_move(cx.listener(
                    |this, event: &DragMoveEvent<DraggedSftpEdge>, window, cx| {
                        this.drag_sftp_edge(event, window, cx);
                    },
                ))
                .into_any_element(),
        )
    }

    /// Sets the SFTP sidebar's width from the mouse: from its outer edge,
    /// which stays put, between [`SFTP_MIN_WIDTH`] and half the window.
    fn drag_sftp_edge(
        &mut self,
        event: &DragMoveEvent<DraggedSftpEdge>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (bounds, x) = (event.bounds, event.event.position.x);
        let width = match self.config.sftp_sidebar {
            SftpSide::Left => x - bounds.left(),
            SftpSide::Right => bounds.right() - x,
        };
        let most = (f32::from(window.viewport_size().width) / 2.0).max(SFTP_MIN_WIDTH);
        self.sftp_width = f32::from(width).clamp(SFTP_MIN_WIDTH, most);
        cx.notify();
    }

    /// Moves keyboard focus to the active tab's focused pane.
    /// While the settings screen is open, the keyboard is its.
    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(screen) = &self.settings {
            let handle = screen.view.focus_handle(cx);
            window.focus(&handle, cx);
            return;
        }
        let focused = self.tabs.active().focused;
        if let Some(pane) = self.panes.get(&focused) {
            let handle = pane.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        } else if let Some(handle) = self
            .files
            .get(&focused)
            .and_then(|files| files.pane.read(cx).focus_handle(cx))
        {
            window.focus(&handle, cx);
        }
    }

    /// Stops a pane's program and forgets the pane, or a restored pane that
    /// never started.
    fn stop_pane(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        self.pending.remove(&leaf);
        // Its SFTP session ends with it, and its ssh with the session; a
        // files pane's sessions end the same way.
        self.sftp.remove(&leaf);
        self.files.remove(&leaf);
        self.pane_subscriptions.remove(&leaf);
        if let Some(pane) = self.panes.remove(&leaf) {
            pane.update(cx, |pane, _| pane.shutdown());
        }
    }

    fn after_close(&mut self, closed: Closed, window: &mut Window, cx: &mut Context<Self>) {
        if closed == Closed::Last {
            // Nothing is left to restore: the normal case, not a corrupt
            // file (spec 0003, flow 7).
            self.save_last(Session::new(self.window_size, Vec::new(), 0));
            cx.quit();
            return;
        }
        self.show_active(window, cx);
    }

    /// After the shown tab or its layout changed: starts what hadn't run
    /// yet, moves the keyboard there and saves soon.
    fn show_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_pending(window, cx);
        self.focus_active(window, cx);
        self.save_soon(window, cx);
        cx.notify();
    }

    fn close_pane(&mut self, leaf: LeafId, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_pane(leaf, cx);
        let closed = self.tabs.close_pane(leaf);
        self.after_close(closed, window, cx);
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (leaves, closed) = self.tabs.close_tab(ix);
        for leaf in leaves {
            self.stop_pane(leaf, cx);
        }
        self.after_close(closed, window, cx);
    }

    /// The tab's title: the user's name, else its focused pane's (spec
    /// 0003).
    fn tab_title(&self, tab: &tabs::Tab, cx: &App) -> SharedString {
        match &tab.name {
            Some(name) => name.clone().into(),
            None => self.pane_title(tab.focused, cx),
        }
    }

    /// A pane's title: its program's, else the name of its profile. A
    /// restored pane that hasn't started shows what it was saved with.
    fn pane_title(&self, leaf: LeafId, cx: &App) -> SharedString {
        if let Some(files) = self.files.get(&leaf) {
            return files.pane.read(cx).title(cx).into();
        }
        let Some(pane) = self.panes.get(&leaf) else {
            let Some(saved) = self.pending.get(&leaf) else {
                return SharedString::default();
            };
            if saved.files.is_some() {
                return "Files".into();
            }
            let profile = self.config.profile(&saved.profile);
            let name = profile
                .as_ref()
                .map_or(&*saved.profile, Profile::display_name);
            return saved.title.as_deref().unwrap_or(name).to_owned().into();
        };
        let pane = pane.read(cx);
        pane.title()
            .unwrap_or(&pane.profile().name)
            .to_owned()
            .into()
    }

    /// A tab running `default_profile` (spec 0005).
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.new_tab();
        let profile = self.config.default_profile();
        self.spawn_pane(leaf, profile, None, window, cx);
        self.show_active(window, cx);
    }

    fn close_focused_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.active().focused;
        self.close(Closing::Pane(leaf), window, cx);
    }

    /// Closes the tab at `ix`, asking first if SFTP edits would go.
    fn ask_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.tabs().get(ix).map(|tab| tab.id) {
            self.close(Closing::Tab(id), window, cx);
        }
    }

    /// Splits the focused pane, unless that would leave either half below
    /// the minimum pane size (spec 0003); then the shortcut does nothing.
    /// The new pane opens `picked` (spec 0007, flow 6), else the same
    /// profile as configured now, in the same directory, unless `picked`
    /// sets one of its own.
    fn split(
        &mut self,
        axis: Axis,
        picked: Option<Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_room(axis, window) {
            return;
        }
        let source = self.tabs.active().focused;
        let (source_profile, source_cwd) = match self.panes.get(&source) {
            Some(pane) => {
                let pane = pane.read(cx);
                (Some(pane.profile().id.clone()), pane.cwd())
            }
            // Beside a files pane, a shell starts in the local folder shown.
            None => (
                None,
                self.files
                    .get(&source)
                    .and_then(|files| files.pane.read(cx).local_folder(cx)),
            ),
        };
        let (profile, cwd) = match picked {
            Some(profile) => {
                let cwd = source_cwd.filter(|_| profile.cwd().is_none());
                (profile, cwd)
            }
            None => {
                let profile = match source_profile {
                    Some(id) => self.config.profile_or_default(&id),
                    None => self.config.default_profile(),
                };
                (profile, source_cwd)
            }
        };
        let leaf = self.tabs.split(axis);
        self.spawn_pane(leaf, profile, cwd, window, cx);
        self.show_active(window, cx);
    }

    /// Whether the focused pane has room for a split along `axis` (spec
    /// 0003). Before the first layout there is nothing to measure yet.
    fn has_room(&self, axis: Axis, window: &Window) -> bool {
        self.content_size.get().is_none_or(|size| {
            let area = (f32::from(size.width), f32::from(size.height));
            self.tabs
                .can_split(axis, area, self.pane_min(window), DIVIDER)
        })
    }

    /// Marks the side of pane `leaf` that `dragged` would land on, with the
    /// pointer at `position`: the nearest one, if it has room (spec 0007).
    /// Every pane hears every move of a drag, so a pane only clears the mark
    /// when it's its own.
    fn drag_over_pane(
        &mut self,
        leaf: LeafId,
        dragged: Dragged,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let side = bounds.contains(&position).then(|| {
            let x = (position.x - bounds.left()) / bounds.size.width;
            let y = (position.y - bounds.top()) / bounds.size.height;
            tree::nearest_side(x, y)
        });
        let target = side
            .filter(|&side| {
                self.content_size.get().is_some_and(|size| {
                    let area = (f32::from(size.width), f32::from(size.height));
                    let pane_min = self.pane_min(window);
                    self.tabs
                        .can_drop(dragged, leaf, side, area, pane_min, DIVIDER)
                })
            })
            .map(|side| (leaf, side));
        let ours = self.drop_target.is_some_and(|(marked, _)| marked == leaf);
        if (target.is_some() || ours) && self.drop_target != target {
            self.drop_target = target;
            cx.notify();
        }
    }

    /// Drops `dragged` on the side of pane `leaf` its drag marked (spec
    /// 0007, flows 3 and 4). Without a mark, nothing moves.
    fn drop_on_pane(
        &mut self,
        leaf: LeafId,
        dragged: Dragged,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target, side)) = self
            .drop_target
            .take()
            .filter(|(marked, _)| *marked == leaf)
        else {
            return;
        };
        if self.tabs.drop_in_split(dragged, target, side) {
            self.show_active(window, cx);
        }
        cx.notify();
    }

    /// Makes a pane dropped on the tab bar a tab of its own at `at` (spec
    /// 0007, flow 5).
    fn pane_to_tab(
        &mut self,
        leaf: LeafId,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if self.tabs.pane_to_tab(leaf, at) {
            self.show_active(window, cx);
        }
    }

    fn focus_toward(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.focus_direction(direction).is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn switch_tab(
        &mut self,
        change: impl FnOnce(&mut Tabs),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        change(&mut self.tabs);
        self.show_active(window, cx);
    }

    /// Opens the rename field over tab `ix`, holding its current title, all
    /// selected so typing replaces it.
    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.tabs().get(ix) else {
            return;
        };
        let (id, title) = (tab.id, self.tab_title(tab, cx));
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        self.rename_subscription = Some(cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let name = input.read(cx).value().to_string();
                    this.finish_rename(Some(name), window, cx);
                }
                InputEvent::Blur => this.finish_rename(None, window, cx),
                _ => {}
            },
        ));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.rename = Some(Rename { tab: id, input });
        cx.notify();
    }

    /// Ends a rename; `Some` saves the name (blank resets it), `None` keeps
    /// the old one.
    fn finish_rename(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        if let (Some(name), Some(ix)) = (name, self.tabs.index_of(rename.tab)) {
            self.tabs.rename(ix, Some(name));
        }
        self.show_active(window, cx);
    }

    /// Sets a divider from the mouse, keeping both sides at least
    /// `MIN_PANE_CELLS` (spec 0003).
    fn drag_divider(
        &mut self,
        path: &[bool],
        axis: Axis,
        event: &DragMoveEvent<DraggedDivider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.drag(cx).0 != path {
            return;
        }
        // A drag that ended outside the window can leave its drop mark.
        self.drop_target = None;
        let bounds = event.bounds;
        let position = event.event.position;
        let (offset, length) = match axis {
            Axis::Row => (position.x - bounds.left(), bounds.size.width),
            Axis::Column => (position.y - bounds.top(), bounds.size.height),
        };
        let (offset, length) = (f32::from(offset), f32::from(length));
        let pane_min = self.pane_min(window);
        let Some(Node::Split {
            ratio,
            first,
            second,
            ..
        }) = self.tabs.active_mut().root.split_at_mut(path)
        else {
            return;
        };
        let (first_min, second_min) = {
            let (a, b) = (
                first.min_size(pane_min, DIVIDER),
                second.min_size(pane_min, DIVIDER),
            );
            match axis {
                Axis::Row => (a.0, b.0),
                Axis::Column => (a.1, b.1),
            }
        };
        let wanted = (offset - DIVIDER / 2.0) / (length - DIVIDER);
        *ratio = tree::clamp_ratio(wanted, length, first_min, second_min, DIVIDER);
        self.save_soon(window, cx);
        cx.notify();
    }

    /// The smallest pane in pixels: `MIN_PANE_CELLS` of the current font,
    /// plus the grid's padding.
    fn pane_min(&self, window: &Window) -> (f32, f32) {
        let cell = self.font.cell_size(window);
        (
            MIN_PANE_CELLS.0 * f32::from(cell.width) + 2.0 * pty::PADDING,
            MIN_PANE_CELLS.1 * f32::from(cell.height) + 2.0 * pty::PADDING,
        )
    }

    /// A pane, which tabs and panes are dropped on (spec 0007). In a split
    /// tab that isn't zoomed, it has the grab handle it's dragged by.
    fn render_leaf(&self, leaf: LeafId, cx: &mut Context<Self>) -> AnyElement {
        let pane = match (self.panes.get(&leaf), self.files.get(&leaf)) {
            (Some(pane), _) => pane.clone().into_any_element(),
            (None, Some(files)) => files.pane.clone().into_any_element(),
            (None, None) => return div().size_full().into_any_element(),
        };
        let tab = self.tabs.active();
        let group = SharedString::from(format!("pane-{leaf}"));
        let handle = (tab.zoomed.is_none() && matches!(tab.root, Node::Split { .. })).then(|| {
            let title = self.pane_title(leaf, cx);
            grab_handle(leaf, &group, self.theme)
                .on_drag(DraggedPane { leaf, title }, |pane, _, _, cx| {
                    cx.new(|_| TabGhost(pane.title.clone()))
                })
        });
        let mark = self
            .drop_target
            .filter(|(marked, _)| *marked == leaf && cx.has_active_drag())
            .map(|(_, side)| {
                let theme = cx.theme();
                div()
                    .absolute()
                    .bg(theme.drop_target)
                    .border_2()
                    .border_color(theme.drag_border)
                    .map(|half| match side {
                        Direction::Left => half.top_0().left_0().h_full().w_1_2(),
                        Direction::Right => half.top_0().right_0().h_full().w_1_2(),
                        Direction::Up => half.top_0().left_0().w_full().h_1_2(),
                        Direction::Down => half.bottom_0().left_0().w_full().h_1_2(),
                    })
            });
        div()
            .group(group)
            .relative()
            .size_full()
            .child(pane)
            .children(handle)
            .children(mark)
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<DraggedTab>, window, cx| {
                    let dragged = Dragged::Tab(event.drag(cx).id);
                    this.drag_over_pane(
                        leaf,
                        dragged,
                        event.bounds,
                        event.event.position,
                        window,
                        cx,
                    );
                },
            ))
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<DraggedPane>, window, cx| {
                    let dragged = Dragged::Pane(event.drag(cx).leaf);
                    this.drag_over_pane(
                        leaf,
                        dragged,
                        event.bounds,
                        event.event.position,
                        window,
                        cx,
                    );
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.drop_on_pane(leaf, Dragged::Tab(dragged.id), window, cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, cx| {
                this.drop_on_pane(leaf, Dragged::Pane(dragged.leaf), window, cx);
            }))
            .into_any_element()
    }

    fn render_node(
        &self,
        node: &Node,
        path: Vec<bool>,
        pane_min: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (axis, ratio, first, second) = match node {
            Node::Leaf(leaf) => return self.render_leaf(*leaf, cx),
            Node::Split {
                axis,
                ratio,
                first,
                second,
            } => (*axis, ratio, first, second),
        };
        let key: String = path.iter().map(|&b| if b { '1' } else { '0' }).collect();
        // Both halves share the space left after the divider by weight, so
        // the first gets exactly `ratio` of it. Their minimum sizes keep a
        // shrinking window from squeezing a pane below the minimum.
        let half = |share: f32, node: &Node, child: AnyElement| {
            let (min_width, min_height) = node.min_size(pane_min, DIVIDER);
            let mut half = div()
                .flex_basis(px(0.))
                .overflow_hidden()
                .map(|half| match axis {
                    Axis::Row => half.min_w(px(min_width)).min_h_0(),
                    Axis::Column => half.min_h(px(min_height)).min_w_0(),
                })
                .child(child);
            half.style().flex_grow = Some(share);
            half.style().flex_shrink = Some(1.);
            half
        };
        let child_path = |second_child: bool| {
            let mut child = path.clone();
            child.push(second_child);
            child
        };
        let first = half(
            *ratio,
            first,
            self.render_node(first, child_path(false), pane_min, cx),
        );
        let second = half(
            1.0 - *ratio,
            second,
            self.render_node(second, child_path(true), pane_min, cx),
        );
        let divider = div()
            .id(ElementId::Name(format!("divider-{key}").into()))
            .flex_shrink_0()
            .bg(theme::hsla(self.theme.foreground).opacity(0.12))
            .map(|divider| match axis {
                Axis::Row => divider.w(px(DIVIDER)).h_full().cursor_col_resize(),
                Axis::Column => divider.h(px(DIVIDER)).w_full().cursor_row_resize(),
            })
            .on_drag(DraggedDivider(path.clone()), |_, _, _, cx| {
                cx.new(|_| DividerGhost)
            });
        let container = div()
            .id(ElementId::Name(format!("split-{key}").into()))
            .size_full()
            .flex()
            .map(|container: gpui::Stateful<Div>| match axis {
                Axis::Row => container.flex_row(),
                Axis::Column => container.flex_col(),
            })
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<DraggedDivider>, window, cx| {
                    this.drag_divider(&path, axis, event, window, cx);
                },
            ));
        container
            .child(first)
            .child(divider)
            .child(second)
            .into_any_element()
    }

    fn render_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> TabBar {
        let renaming = self.rename.as_ref().map(|rename| rename.tab);
        let settings = Self::settings_button("settings-button", cx);
        let tabs = self.tabs.tabs().iter().enumerate().map(|(ix, tab)| {
            let title = self.tab_title(tab, cx);
            let is_renaming = renaming == Some(tab.id);
            let label = match &self.rename {
                Some(rename) if is_renaming => div()
                    .key_context(RENAME_CONTEXT)
                    .on_action(cx.listener(|this, _: &CancelRename, window, cx| {
                        this.finish_rename(None, window, cx);
                    }))
                    .w(px(160.))
                    .child(Input::new(&rename.input).xsmall())
                    .into_any_element(),
                _ => div()
                    .max_w(px(220.))
                    .truncate()
                    .child(title.clone())
                    .into_any_element(),
            };
            let close = Button::new(("close-tab", tab.id))
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.ask_close_tab(ix, window, cx);
                }));
            Tab::new()
                .child(label)
                .suffix(close)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(ix, window, cx);
                    } else {
                        this.switch_tab(|tabs| tabs.activate(ix), window, cx);
                    }
                }))
                // The whole tab is the drag handle, except while its rename
                // field needs the mouse for selecting text.
                .when(!is_renaming, |this| {
                    let dragged = DraggedTab {
                        index: ix,
                        id: tab.id,
                        title,
                    };
                    this.on_drag(dragged, |tab, _, _, cx| {
                        cx.new(|_| TabGhost(tab.title.clone()))
                    })
                })
                // Mark the side the dragged tab lands on (`Tabs::move_tab`
                // puts it at this tab's index). Tabs have a clear 1px border
                // on each side, so the marker takes both and the width holds.
                .drag_over::<DraggedTab>(move |style, dragged, _, cx| {
                    let marker = cx.theme().drag_border;
                    match dragged.index.cmp(&ix) {
                        Ordering::Less => style.border_l_0().border_r_2().border_color(marker),
                        Ordering::Greater => style.border_l_2().border_r_0().border_color(marker),
                        Ordering::Equal => style,
                    }
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                    this.tabs.move_tab(dragged.index, ix);
                    this.save_soon(window, cx);
                    cx.notify();
                }))
                // A pane dropped on a tab becomes the tab after it (spec
                // 0007, flow 5).
                .drag_over::<DraggedPane>(|style, _, _, cx| {
                    let marker = cx.theme().drag_border;
                    style.border_l_0().border_r_2().border_color(marker)
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, cx| {
                    this.pane_to_tab(dragged.leaf, ix + 1, window, cx);
                }))
        });

        // Only a bar on top doubles as the title bar; one at the bottom
        // leaves that to `render_title_bar`.
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. })
            && self.config.tab_bar == TabBarSide::Top;
        let new_tab = Button::new("new-tab")
            .icon(IconName::Plus)
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)));
        let suffix = div()
            .flex()
            .items_center()
            .child(new_tab)
            .child(settings)
            .when(client_decorated, |suffix| {
                suffix.child(Self::window_controls(window))
            });
        TabBar::new("tabs")
            .selected_index(self.tabs.active_index())
            .children(tabs)
            .suffix(suffix)
            .last_empty_space(self.title_bar_space(client_decorated, cx))
    }

    /// The tabs as a list down the left or right side, `tab_bar: left` or
    /// `right` (spec 0003): the same clicks, renames, drags and drops as the
    /// bar, the drop markers above and below instead of beside.
    fn render_side_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bar, active, active_text, text, border) = (
            theme.tab_bar,
            theme.tab_active,
            theme.tab_active_foreground,
            theme.tab_foreground,
            theme.border,
        );
        let renaming = self.rename.as_ref().map(|rename| rename.tab);
        let shown = self.tabs.active_index();
        let tabs = self.tabs.tabs().iter().enumerate().map(|(ix, tab)| {
            let title = self.tab_title(tab, cx);
            let is_renaming = renaming == Some(tab.id);
            let label = match &self.rename {
                Some(rename) if is_renaming => div()
                    .key_context(RENAME_CONTEXT)
                    .on_action(cx.listener(|this, _: &CancelRename, window, cx| {
                        this.finish_rename(None, window, cx);
                    }))
                    .flex_1()
                    .child(Input::new(&rename.input).xsmall())
                    .into_any_element(),
                _ => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(title.clone())
                    .into_any_element(),
            };
            let close = Button::new(("close-side-tab", tab.id))
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.ask_close_tab(ix, window, cx);
                }));
            div()
                .id(("side-tab", tab.id))
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(4.))
                .border_y_2()
                .border_color(gpui::transparent_black())
                .text_sm()
                .map(|row| match ix == shown {
                    true => row.bg(active).text_color(active_text),
                    false => row.text_color(text),
                })
                .child(label)
                .child(close)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(ix, window, cx);
                    } else {
                        this.switch_tab(|tabs| tabs.activate(ix), window, cx);
                    }
                }))
                .when(!is_renaming, |row| {
                    let dragged = DraggedTab {
                        index: ix,
                        id: tab.id,
                        title,
                    };
                    row.on_drag(dragged, |tab, _, _, cx| {
                        cx.new(|_| TabGhost(tab.title.clone()))
                    })
                })
                .drag_over::<DraggedTab>(move |style, dragged, _, cx| {
                    let marker = cx.theme().drag_border;
                    match dragged.index.cmp(&ix) {
                        Ordering::Less => style.border_color(marker).border_t_0(),
                        Ordering::Greater => style.border_color(marker).border_b_0(),
                        Ordering::Equal => style,
                    }
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                    this.tabs.move_tab(dragged.index, ix);
                    this.save_soon(window, cx);
                    cx.notify();
                }))
                .drag_over::<DraggedPane>(|style, _, _, cx| {
                    style.border_color(cx.theme().drag_border).border_t_0()
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, cx| {
                    this.pane_to_tab(dragged.leaf, ix + 1, window, cx);
                }))
        });
        let new_tab = Button::new("new-side-tab")
            .icon(IconName::Plus)
            .label("New tab")
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)));
        // Below the tabs, a dropped pane becomes the last tab (spec 0007).
        let rest = div()
            .id("side-tabs-rest")
            .flex_1()
            .min_h(px(24.))
            .drag_over::<DraggedPane>(|style, _, _, cx| {
                style.border_t_2().border_color(cx.theme().drag_border)
            })
            .on_drop(cx.listener(|this, dragged: &DraggedPane, window, cx| {
                this.pane_to_tab(dragged.leaf, usize::MAX, window, cx);
            }));
        let _ = window;
        div()
            .id("side-tabs")
            .w(px(200.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(6.))
            .bg(bar)
            .map(|list| match self.config.tab_bar {
                TabBarSide::Right => list.border_l_1(),
                _ => list.border_r_1(),
            })
            .border_color(border)
            .overflow_y_scroll()
            .children(tabs)
            .child(new_tab)
            .child(rest)
            .child(Self::settings_button("side-settings-button", cx))
    }

    /// The gear in the tab list's corner that opens the settings screen
    /// (spec 0009, flow 1).
    fn settings_button(id: &'static str, cx: &mut Context<Self>) -> Button {
        Button::new(id)
            .icon(IconName::Settings)
            .ghost()
            .small()
            .tooltip("Settings")
            .on_click(
                cx.listener(|this, _, window, cx| this.open_settings(&OpenSettings, window, cx)),
            )
    }

    /// With the tabs down the side, what's left on top where the compositor
    /// draws no frame: the title bar's free space and the window controls
    /// (spec 0003, flow 8).
    fn render_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .h(px(34.))
            .bg(cx.theme().tab_bar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.title_bar_space(true, cx))
            .child(Self::window_controls(window))
    }

    /// The tab bar's free space, where a dropped pane becomes the last tab
    /// (spec 0007, flow 5). When the compositor draws no frame it acts as
    /// the title bar: drag to move, double-click to maximize (spec 0003,
    /// flow 8). A press only becomes a move once the mouse moves, so the
    /// double click still registers.
    fn title_bar_space(&self, client_decorated: bool, cx: &mut Context<Self>) -> AnyElement {
        let space = div()
            .id("title-bar-space")
            .flex_1()
            .h_full()
            .min_w(px(24.))
            .drag_over::<DraggedPane>(|style, _, _, cx| {
                style.border_l_2().border_color(cx.theme().drag_border)
            })
            .on_drop(cx.listener(|this, dragged: &DraggedPane, window, cx| {
                this.pane_to_tab(dragged.leaf, usize::MAX, window, cx);
            }));
        if !client_decorated {
            return space.into_any_element();
        }
        space
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_bar_pressed = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_bar_pressed = false),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if std::mem::take(&mut this.title_bar_pressed) {
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            })
            .into_any_element()
    }

    /// Minimize, maximize and close, for compositors that draw no frame.
    fn window_controls(window: &Window) -> impl IntoElement {
        let supported = window.window_controls();
        let maximize_icon = if window.is_maximized() {
            IconName::WindowRestore
        } else {
            IconName::WindowMaximize
        };
        div()
            .flex()
            .items_center()
            .when(supported.minimize, |controls| {
                controls.child(
                    Button::new("minimize-window")
                        .icon(IconName::WindowMinimize)
                        .ghost()
                        .small()
                        .on_click(|_, window, _| window.minimize_window()),
                )
            })
            .when(supported.maximize, |controls| {
                controls.child(
                    Button::new("maximize-window")
                        .icon(maximize_icon)
                        .ghost()
                        .small()
                        .on_click(|_, window, _| window.zoom_window()),
                )
            })
            .child(
                Button::new("close-window")
                    .icon(IconName::WindowClose)
                    .ghost()
                    .small()
                    // Through the workspace, which asks first if SFTP edits
                    // would go (spec 0008, flow 3).
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(CloseWindow), cx)),
            )
    }
}

/// The half of a reload that reads files, run on the background executor.
struct ReloadJob {
    /// The OS is in light mode, so `system` reads as `light`.
    light: bool,
    /// The theme names open panes use.
    pane_themes: Vec<String>,
}

/// A reloaded config with every theme it needs resolved.
struct Reloaded {
    config: Config,
    /// `theme.default`.
    theme: Theme,
    themes: HashMap<String, Theme>,
    notices: Vec<String>,
}

impl ReloadJob {
    fn run(self) -> Reloaded {
        let config::Loaded {
            config,
            mut notices,
        } = config::default_path()
            .map(|path| config::load(&path))
            .unwrap_or_else(|| config::Loaded {
                config: Config::default(),
                notices: vec![],
            });
        let (theme, themes) = {
            let dir = config::themes_dir();
            let library = theme::Library::new(&config.theme.custom, dir.as_deref());
            let mut resolve =
                |name: &str| library.resolve(theme::effective(name, self.light), &mut notices);
            let theme = resolve(&config.theme.default);
            let names: BTreeSet<String> = config
                .profiles
                .iter()
                .filter_map(|profile| profile.theme.clone())
                .chain(self.pane_themes)
                .collect();
            let themes = names
                .into_iter()
                .map(|name| {
                    let theme = resolve(&name);
                    (name, theme)
                })
                .collect();
            (theme, themes)
        };
        // Several names can lead to the same broken theme.
        let mut seen = std::collections::HashSet::new();
        notices.retain(|notice| seen.insert(notice.clone()));
        Reloaded {
            config,
            theme,
            themes,
            notices,
        }
    }
}

/// The 6-dot handle a pane of a split tab is dragged by, in the pane's top
/// right corner, shown while the pointer is over the pane's `group` (spec
/// 0007, flow 1). It keeps the mouse from the terminal below, which ends
/// the pane's hover too, so it also shows while hovered itself. It's
/// faded out rather than hidden: a hidden element can't be hovered.
fn grab_handle(leaf: LeafId, group: &SharedString, theme: Theme) -> Stateful<Div> {
    let dot = || {
        div()
            .size(px(3.))
            .rounded_full()
            .bg(theme::hsla(theme.foreground).opacity(0.7))
    };
    let row = || div().flex().gap(px(3.)).child(dot()).child(dot());
    div()
        .id(("pane-handle", leaf))
        .absolute()
        .top(px(4.))
        .right(px(4.))
        .p(px(4.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .rounded(px(4.))
        .bg(theme::hsla(theme.background).opacity(0.85))
        .cursor_grab()
        .occlude()
        .opacity(0.)
        .hover(|style| style.opacity(1.))
        .group_hover(group.clone(), |style| style.opacity(1.))
        .child(row())
        .child(row())
        .child(row())
}

/// Shows `notice` as a toast in the window's bottom-right corner
/// (architecture §Cross-cutting rules).
pub fn show_notice(notice: String, window: &mut Window, cx: &mut App) {
    let notice = Notification::new()
        .message(notice)
        .placement(Anchor::BottomRight);
    window.push_notification(notice, cx);
}

fn is_light(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Light | WindowAppearance::VibrantLight
    )
}

/// The widest the palette gets, in logical pixels.
const PALETTE_WIDTH: f32 = 560.0;

impl Workspace {
    /// The open palette, centered over the focused pane near its top (spec
    /// 0004, flow 2). A click outside it leaves it, like Esc.
    fn render_palette(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let open = self.palette.as_ref()?;
        let size = self.content_size.get()?;
        let tab = self.tabs.active();
        let pane = match tab.zoomed {
            Some(_) => Rect::UNIT,
            None => tab
                .root
                .rects(Rect::UNIT)
                .into_iter()
                .find(|(leaf, _)| *leaf == tab.focused)
                .map_or(Rect::UNIT, |(_, rect)| rect),
        };
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let pane_width = pane.width * width;
        let palette_width = PALETTE_WIDTH.min(pane_width - 16.0).max(240.0);
        let left = (pane.x * width + (pane_width - palette_width) / 2.0).max(0.0);
        let top = pane.y * height + (pane.height * height * 0.12).min(80.0);
        let (border, background) = (cx.theme().border, cx.theme().popover);
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(palette_width))
                .rounded(px(8.))
                .border_1()
                .border_color(border)
                .bg(background)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.close_palette(true, window, cx);
                }))
                .child(open.view.clone()),
        )
    }
}

/// The change that sets `font.family`; `None` is the bundled font.
fn font_family(name: Option<&str>) -> Change {
    Change::Set {
        path: vec!["font", "family"],
        value: name.map_or_else(|| "null".to_owned(), settings::yaml::scalar),
        reads: |config| {
            config
                .font
                .family
                .as_deref()
                .map_or_else(|| "null".to_owned(), settings::yaml::scalar)
        },
    }
}

/// The window's size in logical pixels, as the session keeps it.
fn window_size(window: &Window) -> WindowSize {
    let size = window.window_bounds().get_bounds().size;
    WindowSize {
        width: f32::from(size.width),
        height: f32::from(size.height),
    }
}

/// An invisible element covering its parent that records the parent's size
/// at every layout.
fn measure_size(size: Rc<Cell<Option<Size<Pixels>>>>) -> impl IntoElement {
    canvas(
        move |bounds, _, _| size.set(Some(bounds.size)),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tabs.active();
        let content = match tab.zoomed {
            Some(leaf) => self.render_leaf(leaf, cx),
            None => self.render_node(&tab.root, Vec::new(), self.pane_min(window), cx),
        };
        // The dev badge (spec 0001): every debug run shows its own TTFP.
        // Bottom left: window controls sit top right, notices bottom right.
        let badge = self.ttfp.filter(|_| cfg!(debug_assertions)).map(|ms| {
            div()
                .absolute()
                .bottom(px(8.))
                .left(px(12.))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(gpui::black().opacity(0.6))
                .text_size(px(12.))
                .text_color(gpui::white())
                .child(format!("TTFP {ms:.0} ms"))
        });
        // The tabs on top as a bar, or down the left as a list with only the
        // title bar left on top, where the compositor draws no frame.
        // Elsewhere than on top, the tabs leave the title bar on top where
        // the compositor draws no frame (spec 0003, flow 8).
        let side = self.config.tab_bar;
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let top = match side {
            TabBarSide::Top => Some(self.render_tabs(window, cx).into_any_element()),
            _ => client_decorated.then(|| self.render_title_bar(window, cx).into_any_element()),
        };
        let bottom =
            (side == TabBarSide::Bottom).then(|| self.render_tabs(window, cx).into_any_element());
        let list = side
            .vertical()
            .then(|| self.render_side_tabs(window, cx).into_any_element());
        let (left, right) = match side {
            TabBarSide::Right => (None, list),
            _ => (list, None),
        };
        // The SFTP sidebar sits inside a tab list on its side (spec 0008).
        let sftp = self.render_sftp(cx);
        let (sftp_left, sftp_right) = match self.config.sftp_sidebar {
            SftpSide::Left => (sftp, None),
            SftpSide::Right => (None, sftp),
        };
        let workspace = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::hsla(self.theme.background))
            .text_color(theme::hsla(self.theme.foreground))
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::new_tab_with_profile))
            .on_action(cx.listener(Self::close_focused_pane))
            .on_action(cx.listener(Self::open_config))
            .on_action(cx.listener(Self::open_config_folder))
            .on_action(cx.listener(Self::check_for_updates))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                this.switch_tab(Tabs::next_tab, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                this.switch_tab(Tabs::previous_tab, window, cx);
            }))
            .on_action(cx.listener(|this, action: &GoToTab, window, cx| {
                let ix = action.0;
                this.switch_tab(|tabs| tabs.activate(ix), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                this.split(Axis::Row, None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                this.split(Axis::Column, None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitRightWithProfile, window, cx| {
                this.pick_split_profile(Axis::Row, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitDownWithProfile, window, cx| {
                this.pick_split_profile(Axis::Column, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusLeft, window, cx| {
                this.focus_toward(Direction::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusRight, window, cx| {
                this.focus_toward(Direction::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusUp, window, cx| {
                this.focus_toward(Direction::Up, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusDown, window, cx| {
                this.focus_toward(Direction::Down, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomPane, window, cx| {
                this.tabs.toggle_zoom();
                this.save_soon(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| {
                if let Some(pane) = this.focused_pane() {
                    pane.update(cx, |pane, cx| pane.copy(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                if let Some(pane) = this.focused_pane() {
                    pane.update(cx, |pane, cx| pane.paste(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Search, window, cx| {
                if let Some(pane) = this.focused_pane() {
                    pane.update(cx, |pane, cx| pane.open_search(window, cx));
                }
            }))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::select_theme))
            .on_action(cx.listener(Self::connect_to_host))
            .on_action(cx.listener(Self::forget_password))
            .on_action(cx.listener(Self::toggle_sftp))
            .on_action(cx.listener(Self::open_files))
            .on_action(cx.listener(Self::open_files_in_new_tab))
            .on_action(cx.listener(Self::switch_files_target))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.close(Closing::Window, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.ask_close_tab(this.tabs.active_index(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                this.start_rename(this.tabs.active_index(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, _, cx| {
                this.zoom_font(Some(1.0), cx);
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, _, cx| {
                this.zoom_font(Some(-1.0), cx);
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, _, cx| {
                this.zoom_font(None, cx);
            }))
            .children(top)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(left)
                    .children(sftp_left)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .relative()
                            .child(content)
                            .child(measure_size(Rc::clone(&self.content_size)))
                            .children(self.render_settings())
                            .children(self.render_palette(cx)),
                    )
                    .children(sftp_right)
                    .children(right),
            )
            .children(bottom)
            .children(badge);
        // Shadow and resize edges when the compositor draws no frame;
        // nothing otherwise.
        window_border().child(workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use crate::session::tests::{files_leaf, leaf, split, tab};
    use std::collections::BTreeMap;
    use std::time::Instant;

    /// A pane for `saved` that starts no program: it waits for Enter.
    fn idle_pane(saved: &SavedLeaf) -> BootPane {
        BootPane {
            start: Start::OnEnter(String::new()),
            launch: Launch {
                command: Command::bench(),
                cwd: saved.cwd.clone(),
                timing: Arc::new(Timing::new(Instant::now())),
            },
            profile: (&Profile::shell()).into(),
            theme: theme::DARK,
            title: saved.title.clone(),
        }
    }

    /// Three tabs, the middle one shown: a shell in `~/notes`; one pane a
    /// quarter wide beside two stacked; an SSH host's beside a shell.
    fn three_tabs() -> Session {
        let notes = tab(
            leaf(1, "shell", Some("/home/me/notes"), Some("vim todo.md")),
            1,
        );
        let stacked = split(
            Axis::Column,
            0.5,
            leaf(3, "shell", Some("/tmp"), None),
            leaf(4, "htop", None, Some("htop")),
        );
        let shown = tab(
            split(Axis::Row, 0.25, leaf(2, "shell", None, None), stacked),
            2,
        );
        let servers = tab(
            split(
                Axis::Row,
                0.5,
                leaf(5, "ssh:prod", None, Some("me@prod: ~")),
                leaf(6, "shell", Some("/srv"), None),
            ),
            6,
        );
        let window = WindowSize {
            width: 1200.0,
            height: 800.0,
        };
        Session::new(Some(window), vec![notes, shown, servers], 1)
    }

    /// AC3 and AC4 (spec 0003), flow 6: at boot only the shown tab's panes
    /// start. The other tabs' panes wait, with the profile, cwd and title
    /// they were saved with, until their tab is first shown.
    #[test]
    fn at_boot_only_the_shown_tabs_panes_start_and_the_others_wait_as_saved() {
        let session = three_tabs();
        let mut started = Vec::new();
        let boot = Boot::restore(&session, (1200.0, 800.0), |saved, _| {
            started.push(saved.id);
            idle_pane(saved)
        })
        .expect("a session with tabs");
        started.sort();
        assert_eq!(started, [2, 3, 4]);
        assert_eq!(boot.tabs.active_index(), 1);
        let mut boot_panes: Vec<LeafId> = boot.panes.iter().map(|(leaf, _)| *leaf).collect();
        boot_panes.sort();
        assert_eq!(boot_panes, [2, 3, 4]);
        let waiting: HashMap<LeafId, SavedLeaf> = [&session.tabs[0], &session.tabs[2]]
            .into_iter()
            .flat_map(|saved| saved.root.leaves())
            .map(|saved| (saved.id, saved.clone()))
            .collect();
        assert_eq!(boot.pending, waiting);
    }

    /// Spec 0003, flow 6, and spec 0001, flow 4: each pane of the shown tab
    /// starts at its share of the saved window, so its program draws for
    /// the size it's shown at.
    #[test]
    fn each_pane_started_at_boot_gets_its_share_of_the_window() {
        let mut areas = BTreeMap::new();
        Boot::restore(&three_tabs(), (1200.0, 800.0), |saved, area| {
            areas.insert(saved.id, area);
            idle_pane(saved)
        })
        .expect("a session with tabs");
        // A quarter of the width, beside two sharing the rest's height.
        let expected = BTreeMap::from([
            (2, (300.0, 800.0)),
            (3, (900.0, 400.0)),
            (4, (900.0, 400.0)),
        ]);
        assert_eq!(areas, expected);
    }

    /// AC6 (spec 0003), flow 7: quitting by closing the last tab saves a
    /// session without tabs. That's the normal case, not a corrupt file, so
    /// nothing is kept as `.bad`, and the relaunch boots one fresh tab.
    #[test]
    fn a_session_without_tabs_loads_without_error_and_boots_one_fresh_tab() {
        let root = TempDir::new("session-no-tabs");
        let path = root.path().join("session.json");
        let window = WindowSize {
            width: 900.0,
            height: 600.0,
        };
        session::save(&path, &Session::new(Some(window), Vec::new(), 0)).expect("save");
        let loaded = session::load(&path)
            .expect("no error, so nothing to keep as .bad")
            .expect("a saved session");
        let boot = Boot::restore(&loaded, (900.0, 600.0), |_, _| {
            panic!("a session without tabs started a pane")
        });
        assert!(boot.is_none(), "a session without tabs was restored");
    }

    /// Spec 0003, flow 6: a restored pane whose profile no longer exists
    /// opens `default_profile`, with one notice naming the profile that's
    /// gone. A pane whose profile is still there opens it without one.
    #[test]
    fn a_restored_pane_whose_profile_is_gone_opens_default_profile_with_a_notice() {
        let config = config::parse(concat!(
            "default_profile: htop\n",
            "profiles:\n",
            "  htop:\n",
            "    type: local\n",
            "    command: [htop]\n",
            "  vim:\n",
            "    type: local\n",
            "    command: [vim]\n",
        ))
        .config;
        let mut notices = Vec::new();
        let opened = restored_profile(&config, "old-project", &mut notices);
        assert_eq!(opened.id, "htop");
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("old-project"), "{}", notices[0]);
        notices.clear();
        assert_eq!(restored_profile(&config, "vim", &mut notices).id, "vim");
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Spec 0013, Startup impact and AC9: a files pane in the shown tab
    /// starts nothing before the first frame; it waits for the window with
    /// what it saved, beside the tab's terminal, which starts. A files
    /// pane in another tab waits for its tab like any pane.
    #[test]
    fn at_boot_a_files_pane_starts_nothing_and_waits_as_saved() {
        let shown = tab(
            split(
                Axis::Row,
                0.5,
                leaf(1, "shell", Some("/tmp"), None),
                files_leaf(2, None, Some("/home/me/notes")),
            ),
            2,
        );
        let other = tab(files_leaf(3, Some("prod"), Some("/srv")), 3);
        let session = Session::new(None, vec![shown, other], 0);
        let mut started = Vec::new();
        let boot = Boot::restore(&session, (1200.0, 800.0), |saved, _| {
            started.push(saved.id);
            idle_pane(saved)
        })
        .expect("a session with tabs");
        assert_eq!(started, [1]);
        let boot_panes: Vec<LeafId> = boot.panes.iter().map(|(leaf, _)| *leaf).collect();
        assert_eq!(boot_panes, [1]);
        let local = SavedFiles {
            target: None,
            folder: Some("/home/me/notes".into()),
        };
        assert_eq!(boot.files, [(2, local)]);
        let waiting: Vec<LeafId> = boot.pending.keys().copied().collect();
        assert_eq!(waiting, [3]);
        assert_eq!(
            boot.pending[&3].files,
            Some(SavedFiles {
                target: Some("prod".into()),
                folder: Some("/srv".into()),
            })
        );
    }

    /// Spec 0013, Saved: a restored files pane shows Local at its folder,
    /// or its SSH profile at its folder (disconnected, so no connection);
    /// a profile that no longer exists shows Local at home, with one
    /// notice naming it.
    #[test]
    fn a_restored_files_pane_whose_host_is_gone_shows_local_at_home_with_a_notice() {
        let config = config::parse(concat!(
            "profiles:\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
        ))
        .config;
        let saved = |target: Option<&str>| SavedFiles {
            target: target.map(str::to_owned),
            folder: Some("/srv".into()),
        };
        let mut notices = Vec::new();
        assert_eq!(
            restored_files(&config, &saved(None), &mut notices),
            (Target::Local, Some("/srv".into()))
        );
        assert_eq!(
            restored_files(&config, &saved(Some("prod")), &mut notices),
            (Target::Ssh("prod".into()), Some("/srv".into()))
        );
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(
            restored_files(&config, &saved(Some("old-host")), &mut notices),
            (Target::Local, None)
        );
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("old-host"), "{}", notices[0]);
    }
}
