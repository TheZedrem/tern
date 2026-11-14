//! The window's content (spec 0003): the tab bar, which doubles as the
//! title bar when the compositor draws no frame, over the active tab's
//! split tree, which tabs and panes are dragged into and out of (spec
//! 0007). The layout itself is plain data in [`tabs`] and [`tree`]; this
//! module owns the panes and draws them.

pub mod answers;
mod chrome;
mod closing;
mod files;
mod forwards;
mod panes;
mod plugins;
mod profiles;
mod reload;
mod requests;
mod session;
mod sidebar;
mod splits;
pub mod tabs;
pub mod tree;
mod updates;

use crate::commands::{
    self, CloseTab, CopySelection, DecreaseFontSize, FocusDown, FocusLeft, FocusRight, FocusUp,
    GoToTab, IncreaseFontSize, NewTab, NextTab, Paste, PluginCommand, PreviousTab, RenameTab,
    ResetFontSize, Search, SplitDown, SplitDownWithProfile, SplitRight, SplitRightWithProfile,
    ZoomPane,
};
use crate::config::{self, Config, SidebarSide, TabBarSide};
use crate::files::browser;
use crate::session::SavedSidebar;
use crate::session::{SavedFiles, SavedLeaf, Session, WindowSize};
use crate::sftp;
use crate::terminal::{FONT_SIZES, FontSettings, Launch, PaneProfile, Start, TerminalPane};
use crate::theme::{self, Theme};
use crate::timing::{Mark, Timing};
use chrome::{RENAME_CONTEXT, Rename};
use closing::Closing;
use files::{FilesLeaf, PaneViews};
use gpui::{
    Anchor, App, Context, Decorations, Entity, Focusable, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Pixels, Render, Size, Styled, Subscription, Task, Window, actions,
    canvas, div, px,
};
use gpui_component::notification::Notification;
use gpui_component::{WindowExt, window_border};
use profiles::OpenPaletteView;
use reload::SettingsScreen;
pub use session::restored_profile;
use session::{restored_files, window_size};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tabs::Tabs;
use tree::{Axis, Direction, LeafId, Rect};

/// Key context of the workspace, where the commands are bound.
const CONTEXT: &str = commands::keymap::CONTEXT;
/// How long boot waits for the first frame with output before starting
/// what waits for it anyway.
const SILENT_BOOT_WAIT: Duration = Duration::from_secs(1);

actions!(tern, [CancelRename, CloseWindow]);

/// Binds the keys of the workspace's own widgets. The commands' keys are
/// [`commands::keymap`]'s.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelRename,
        Some(RENAME_CONTEXT),
    )]);
    browser::init(cx);
    crate::sidebar::init(cx);
    answers::init(cx);
    crate::forwards::view::init(cx);
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
    /// The sidebar as the session saved it (spec 0018); closed without.
    sidebar: Option<SavedSidebar>,
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
            sidebar: None,
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
        let mut boot = Self::parked(session)?;
        let shown = boot.tabs.active();
        let rects = shown.root.rects(Rect::UNIT);
        let zoomed = shown.zoomed;
        for leaf in session.tabs[boot.tabs.active_index()].root.leaves() {
            boot.pending.remove(&leaf.id);
            if let Some(saved) = &leaf.files {
                boot.files.push((leaf.id, saved.clone()));
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
            boot.panes.push((
                leaf.id,
                start(leaf, (rect.width * area.0, rect.height * area.1)),
            ));
        }
        Some(boot)
    }

    /// The tabs of `session`, none of them started, behind a new last tab
    /// of `pane` that is shown (spec 0003, flow 6, `open_new_tab`). `None`
    /// for a session without tabs.
    pub fn restore_behind(session: &Session, pane: impl FnOnce() -> BootPane) -> Option<Self> {
        let mut boot = Self::parked(session)?;
        let leaf = boot.tabs.new_tab();
        boot.panes.push((leaf, pane()));
        Some(boot)
    }

    /// The tabs of `session` with every leaf waiting for its tab.
    fn parked(session: &Session) -> Option<Self> {
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
        let pending = session
            .tabs
            .iter()
            .flat_map(|saved| saved.root.leaves())
            .map(|leaf| {
                // Shown in the tab bar until it starts: the title rules
                // apply to a saved title too.
                let title = leaf.title.as_deref().map(crate::terminal::clean_title);
                let leaf = SavedLeaf {
                    title: title.filter(|title| !title.is_empty()),
                    ..leaf.clone()
                };
                (leaf.id, leaf)
            })
            .collect();
        Some(Self {
            tabs,
            panes: Vec::new(),
            files: Vec::new(),
            pending,
            bad_session: None,
            sidebar: session.sidebar.clone(),
        })
    }
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
    /// What the sidebar's Files panel draws for each SSH pane it was
    /// shown for (spec 0008, spec 0018). Only the active pane's shows.
    sftp: HashMap<LeafId, PaneViews>,
    /// The files panes, by leaf (spec 0013); every other leaf is a
    /// terminal pane.
    files: HashMap<LeafId, FilesLeaf>,
    /// The window's sidebar, from its first use on (spec 0018).
    sidebar: Option<sidebar::Hosted>,
    /// The sidebar as the session saved it: what the first frame leaves
    /// room for, and what the sidebar starts from.
    sidebar_boot: SavedSidebar,
    /// The app's port forwards, from the first one on (spec 0017).
    forwards: Option<Entity<crate::forwards::registry::Registry>>,
    /// The plugin host, from the first frame on (spec 0016).
    plugins: Option<plugins::Plugins>,
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
            sidebar,
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
            window_size: window_size(window).usable(),
            save_task: None,
            save_failed: false,
            bad_session,
            update_timer: None,
            update_asked: false,
            sftp: HashMap::new(),
            files: HashMap::new(),
            sidebar: None,
            sidebar_boot: sidebar.unwrap_or_else(sidebar::closed),
            forwards: None,
            plugins: None,
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
                // Minimizing reports a size too; keep the last real one.
                if let Some(size) = window_size(window).usable() {
                    this.window_size = Some(size);
                    this.save_soon(window, cx);
                }
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
        self.restore_sidebar(window, cx);
        self.follow_update_check(window, cx);
        // Edit copies a crashed run left (spec 0008, Model), off the UI
        // thread and never before the first frame.
        if !self.bench {
            cx.background_executor()
                .spawn(async { sftp::copies::sweep() })
                .detach();
            self.start_plugins(window, cx);
        }
        cx.notify();
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
            "pane.portForwards" => self
                .panes
                .get(&tab.focused)
                .is_some_and(|pane| pane.read(cx).ssh_target().is_some()),
            "forwards.stop" => self
                .forwards
                .as_ref()
                .is_some_and(|forwards| !forwards.read(cx).all().is_empty()),
            "font.increase" => size < *FONT_SIZES.end(),
            "font.decrease" => size > *FONT_SIZES.start(),
            "font.reset" => self.zoom != 0.0,
            "plugin.allowRequest" | "plugin.denyRequest" => self.has_focused_request(),
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

    /// The shown tab's focused pane, which copy, paste and search act on.
    fn focused_pane(&self) -> Option<Entity<TerminalPane>> {
        self.panes.get(&self.tabs.active().focused).cloned()
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

    /// After the shown tab or its layout changed: starts what hadn't run
    /// yet, moves the keyboard there and saves soon.
    fn show_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_pending(window, cx);
        self.focus_active(window, cx);
        self.save_soon(window, cx);
        cx.notify();
    }

    /// A tab running `default_profile` (spec 0005).
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.new_tab();
        let profile = self.config.default_profile();
        self.spawn_pane(leaf, profile, None, window, cx);
        self.show_active(window, cx);
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
}

/// Shows `notice` as a toast in the window's bottom-right corner
/// (architecture §Cross-cutting rules).
pub fn show_notice(notice: String, window: &mut Window, cx: &mut App) {
    let notice = Notification::new()
        .message(notice)
        .placement(Anchor::BottomRight);
    window.push_notification(notice, cx);
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
        self.sync_sidebar(window, cx);
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
        // The sidebar sits inside a tab list on its side (spec 0018).
        let sidebar = self.render_sidebar(window, cx);
        let (sidebar_left, sidebar_right) = match self.config.app_sidebar {
            SidebarSide::Left => (sidebar, None),
            SidebarSide::Right => (None, sidebar),
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
            .on_action(cx.listener(Self::open_plugins_folder))
            .on_action(cx.listener(Self::allow_plugin_request))
            .on_action(cx.listener(Self::deny_plugin_request))
            .on_action(cx.listener(|this, command: &PluginCommand, window, cx| {
                this.run_plugin_command(command, window, cx);
            }))
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
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::focus_sidebar))
            .on_action(cx.listener(Self::show_port_forwards))
            .on_action(cx.listener(Self::stop_forward))
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
                    .children(sidebar_left)
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
                    .children(sidebar_right)
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
    use crate::config::{Profile, SHELL};
    use crate::session::tests::{files_leaf, leaf, shell_ssh_shell_split, split, tab};
    use crate::session::{SavedNode, SavedTab};
    use crate::terminal::pty::Command;
    use gpui::AppContext;
    use std::collections::BTreeMap;
    use std::time::Instant;

    /// A pane for `saved` that starts no program: it waits for Enter.
    fn idle_pane(saved: &SavedLeaf) -> BootPane {
        idle_pane_of(&Profile::shell(), saved)
    }

    /// A pane of `profile` for `saved` that starts no program: it waits
    /// for Enter, which a test never presses in it. An SSH profile's pane
    /// names its host as a real one does, and connects to nothing.
    pub(super) fn idle_pane_of(profile: &Profile, saved: &SavedLeaf) -> BootPane {
        let command = match profile.kind {
            config::ProfileKind::Ssh(_) => Command::of_profile(profile),
            config::ProfileKind::Local(_) => Command::bench(),
        };
        BootPane {
            start: Start::OnEnter(String::new()),
            launch: Launch {
                command,
                cwd: saved.cwd.clone(),
                timing: Arc::new(Timing::new(Instant::now())),
            },
            profile: profile.into(),
            theme: theme::DARK,
            title: saved.title.clone(),
        }
    }

    /// A workspace showing `boot` in a test window, with the app's keymap
    /// for `config` and its first frame drawn. It saves no session. What
    /// runs after the first frame starts real threads, which GPUI's test
    /// scheduler rejects, so a test calls the step it needs itself.
    pub(super) fn open_workspace(
        boot: Boot,
        config: Config,
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            init(cx);
            gpui_component::init(cx);
            commands::keymap::keep_library(cx);
            commands::keymap::bind(&config.keybindings, config.new_tab, cx);
        });
        let opened = Rc::new(std::cell::RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let opened = opened.clone();
            move |window, cx| {
                let font = FontSettings::from_config(&config.font);
                let workspace = cx.new(|cx| {
                    Workspace::new(
                        boot,
                        config,
                        font,
                        theme::DARK,
                        Vec::new(),
                        None,
                        Arc::new(Timing::new(Instant::now())),
                        false,
                        window,
                        cx,
                    )
                });
                *opened.borrow_mut() = Some(workspace.clone());
                gpui_component::Root::new(workspace, window, cx)
            }
        });
        let workspace = opened.take().expect("the window built the workspace");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        (workspace, cx)
    }

    /// Three tabs, the middle one shown: a shell in `~/notes`; one pane a
    /// quarter wide beside two stacked; an SSH host's beside a shell. The
    /// `~/notes` tab is a plain shell, so a real boot would reuse it (spec
    /// 0003, Reuse); tests of `restore_behind` call it directly.
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

    /// AC11 (spec 0003), flow 6, `open_new_tab: true`: three saved tabs
    /// boot as four, the new last one shown. Its pane is the only one
    /// started; every saved pane, the last shown tab's too, waits for its
    /// tab, and none shares the new pane's ID.
    #[test]
    fn a_new_tab_behind_the_session_is_shown_and_the_only_one_started() {
        let session = three_tabs();
        let mut fresh = 0;
        let boot = Boot::restore_behind(&session, || {
            fresh += 1;
            BootPane {
                start: Start::OnEnter(String::new()),
                launch: Launch {
                    command: Command::bench(),
                    cwd: None,
                    timing: Arc::new(Timing::new(Instant::now())),
                },
                profile: (&Profile::shell()).into(),
                theme: theme::DARK,
                title: None,
            }
        })
        .expect("a session with tabs");
        assert_eq!(fresh, 1);
        assert_eq!(boot.tabs.tabs().len(), 4);
        assert_eq!(boot.tabs.active_index(), 3);
        let new_leaf = boot.tabs.active().focused;
        assert_eq!(boot.tabs.active().root.leaves(), [new_leaf]);
        let boot_panes: Vec<LeafId> = boot.panes.iter().map(|(leaf, _)| *leaf).collect();
        assert_eq!(boot_panes, [new_leaf]);
        assert!(boot.files.is_empty());
        let waiting: HashMap<LeafId, SavedLeaf> = session
            .tabs
            .iter()
            .flat_map(|saved| saved.root.leaves())
            .map(|saved| (saved.id, saved.clone()))
            .collect();
        assert_eq!(boot.pending, waiting);
        assert!(!waiting.contains_key(&new_leaf));
    }

    /// Boots `session` the way `main` does with `open_new_tab: true` and the
    /// local `shell` as `default_profile` (spec 0003, flow 6): a restored
    /// plain shell is shown and started, else a new tab is added behind
    /// the session. Returns the boot and the IDs of the panes started.
    fn boot_with_new_tab(mut session: Session) -> (Boot, Vec<LeafId>) {
        let mut started = Vec::new();
        let boot = if session.focus_plain_shell(&Profile::shell()) {
            Boot::restore(&session, (1200.0, 800.0), |saved, _| {
                started.push(saved.id);
                idle_pane(saved)
            })
        } else {
            Boot::restore_behind(&session, || {
                idle_pane(&SavedLeaf {
                    id: 0,
                    profile: SHELL.into(),
                    cwd: None,
                    title: None,
                    files: None,
                })
            })
            .inspect(|boot| started.extend(boot.panes.iter().map(|(leaf, _)| *leaf)))
        }
        .expect("a session with tabs");
        (boot, started)
    }

    /// What quitting right after `boot` saves: its tabs as they booted,
    /// each pane as restored, a new tab's as a shell of `default_profile`.
    fn quit(boot: &Boot, before: &Session) -> Session {
        let saved: HashMap<LeafId, SavedLeaf> = before
            .tabs
            .iter()
            .flat_map(|saved| saved.root.leaves())
            .map(|saved| (saved.id, saved.clone()))
            .collect();
        let tabs = boot
            .tabs
            .tabs()
            .iter()
            .map(|shown| SavedTab {
                name: shown.name.clone(),
                root: SavedNode::of(&shown.root, &mut |id| {
                    saved.get(&id).cloned().unwrap_or(SavedLeaf {
                        id,
                        profile: SHELL.into(),
                        cwd: None,
                        title: None,
                        files: None,
                    })
                }),
                focused: shown.focused,
                zoomed: shown.zoomed,
            })
            .collect();
        Session::new(None, tabs, boot.tabs.active_index())
    }

    /// AC13 (spec 0003), flow 6, Reuse: with `open_new_tab: true` and saved
    /// tabs [shell in `/tmp`, SSH, shell in `~/src`, split of two shells],
    /// the same four tabs boot with the third shown. Its pane is the only
    /// one started, in `~/src` with its saved title; every other pane,
    /// the last shown SSH tab's too, waits for its tab.
    #[test]
    fn a_restored_plain_shell_is_shown_and_the_only_one_started() {
        let (boot, started) = boot_with_new_tab(shell_ssh_shell_split());
        assert_eq!(boot.tabs.tabs().len(), 4);
        assert_eq!(boot.tabs.active_index(), 2);
        assert_eq!(started, [3]);
        let [(3, pane)] = &boot.panes[..] else {
            panic!("only the ~/src shell is a boot pane");
        };
        assert_eq!(pane.launch.cwd, Some(PathBuf::from("/home/me/src")));
        assert_eq!(pane.title.as_deref(), Some("git log"));
        assert!(boot.files.is_empty());
        let mut waiting: Vec<LeafId> = boot.pending.keys().copied().collect();
        waiting.sort();
        assert_eq!(waiting, [1, 2, 4, 5]);
        let layouts: Vec<Vec<LeafId>> = boot
            .tabs
            .tabs()
            .iter()
            .map(|shown| shown.root.leaves())
            .collect();
        assert_eq!(layouts, [vec![1], vec![2], vec![3], vec![4, 5]]);
    }

    /// AC13 (spec 0003): quitting and relaunching five times with
    /// `open_new_tab: true` leaves the same four tabs, each launch showing
    /// the `~/src` shell and starting only it. Quitting is modelled as
    /// saving the tabs as booted; the save itself needs a window.
    #[test]
    fn relaunching_onto_a_plain_shell_adds_no_tabs() {
        let mut session = shell_ssh_shell_split();
        for launch in 1..=5 {
            let (boot, started) = boot_with_new_tab(session.clone());
            assert_eq!(boot.tabs.tabs().len(), 4, "launch {launch}");
            assert_eq!(boot.tabs.active_index(), 2, "launch {launch}");
            assert_eq!(started, [3], "launch {launch}");
            session = quit(&boot, &session);
        }
        assert_eq!(session.tabs, shell_ssh_shell_split().tabs);
    }

    /// AC11 and AC13 (spec 0003): with no plain shell to reuse, the first
    /// launch adds one tab; that tab is a plain shell, so every later
    /// launch focuses it instead of adding another.
    #[test]
    fn the_tab_added_once_is_reused_on_every_later_launch() {
        let three_tabs = vec![
            tab(leaf(1, "ssh:prod", None, Some("me@prod: ~")), 1),
            tab(
                split(
                    Axis::Row,
                    0.5,
                    leaf(2, "shell", Some("/tmp"), None),
                    leaf(3, "shell", None, None),
                ),
                2,
            ),
            tab(files_leaf(4, None, Some("/home/me")), 4),
        ];
        let mut session = Session::new(None, three_tabs, 1);
        assert_eq!(session.plain_shell(SHELL), None);
        for launch in 1..=5 {
            let (boot, started) = boot_with_new_tab(session.clone());
            assert_eq!(boot.tabs.tabs().len(), 4, "launch {launch}");
            assert_eq!(boot.tabs.active_index(), 3, "launch {launch}");
            assert_eq!(started, [boot.tabs.active().focused], "launch {launch}");
            session = quit(&boot, &session);
        }
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
        crate::session::save(&path, &Session::new(Some(window), Vec::new(), 0)).expect("save");
        let loaded = crate::session::load(&path)
            .expect("no error, so nothing to keep as .bad")
            .expect("a saved session");
        let boot = Boot::restore(&loaded, (900.0, 600.0), |_, _| {
            panic!("a session without tabs started a pane")
        });
        assert!(boot.is_none(), "a session without tabs was restored");
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

    /// Spec 0003, flow 6 (T-063): a restored tab whose saved focus is its
    /// files pane gives that pane the keyboard and the active border. What
    /// runs after the first frame starts real threads (the Local listing,
    /// the config watcher), which GPUI's test scheduler rejects, so that
    /// step is checked in the running app instead.
    #[gpui::test]
    fn a_restored_tab_focused_on_its_files_pane_keeps_that_focus(cx: &mut gpui::TestAppContext) {
        let shown = tab(
            split(
                Axis::Row,
                0.5,
                leaf(1, "shell", None, None),
                files_leaf(2, None, None),
            ),
            2,
        );
        let session = Session::new(None, vec![shown], 0);
        let boot = Boot::restore(&session, (1200.0, 800.0), |saved, _| idle_pane(saved))
            .expect("a session with tabs");
        let (workspace, cx) = open_workspace(boot, Config::default(), cx);
        cx.update(|window, cx| {
            let workspace = workspace.read(cx);
            assert_eq!(workspace.tabs.active().focused, 2, "the border's pane");
            let files = workspace.files[&2].pane.read(cx).focus_handle(cx);
            assert!(
                files.is_some_and(|handle| handle.contains_focused(window, cx)),
                "the keyboard is in the files pane"
            );
        });
    }
}
