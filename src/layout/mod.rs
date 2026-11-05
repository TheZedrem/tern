//! The window's content (spec 0003): the tab bar, which doubles as the
//! title bar when the compositor draws no frame, over the active tab's
//! split tree, which tabs and panes are dragged into and out of (spec
//! 0007). The layout itself is plain data in [`tabs`] and [`tree`]; this
//! module owns the panes and draws them.

pub mod tabs;
pub mod tree;

use crate::commands::palette::{Entry, Palette, PaletteEvent};
use crate::commands::{
    self, ClosePane, CloseTab, ConnectToHost, DecreaseFontSize, FocusDown, FocusLeft, FocusRight,
    FocusUp, ForgetPassword, GoToTab, IncreaseFontSize, NewTab, NextTab, OpenConfig, OpenPalette,
    PreviousTab, RenameTab, ResetFontSize, SelectTheme, SplitDown, SplitDownWithProfile,
    SplitRight, SplitRightWithProfile, ZoomPane,
};
use crate::config::{self, Config, Profile, ProfileKind};
use crate::secrets;
use crate::ssh;
use crate::terminal::pty::{self, Command, GridSize};
use crate::terminal::{
    FONT_SIZES, FontSettings, Launch, PaneEvent, PaneProfile, PaneSettings, Start, TerminalPane,
};
use crate::theme::{self, Theme};
use crate::timing::{Mark, Timing};
use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, AnyElement, App, AppContext, Bounds, ClickEvent, Context, Decorations, Div,
    DragMoveEvent, ElementId, Entity, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, Pixels, Point, Render, SharedString, Size, Stateful, StatefulInteractiveElement,
    Styled, Subscription, Window, WindowAppearance, actions, canvas, div, px,
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

actions!(zed_term, [CancelRename]);

/// Binds the keys of the workspace's own widgets. The commands' keys are
/// [`commands::keymap`]'s.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelRename,
        Some(RENAME_CONTEXT),
    )]);
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

/// The first pane, its program started before the window existed (spec
/// 0001).
pub struct BootPane {
    pub start: Start,
    pub launch: Launch,
    pub profile: PaneProfile,
    pub theme: Theme,
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
}

impl Workspace {
    /// A workspace with one tab, showing `boot`.
    #[allow(clippy::too_many_arguments, reason = "one call, from the boot path")]
    pub fn new(
        boot: BootPane,
        config: Config,
        font: FontSettings,
        theme: Theme,
        notices: Vec<String>,
        timing: Arc<Timing>,
        bench: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tabs, leaf) = Tabs::new();
        let themes = boot
            .profile
            .theme
            .iter()
            .map(|name| (name.clone(), boot.theme))
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
        };
        let BootPane {
            start,
            launch,
            profile,
            theme,
        } = boot;
        workspace.add_pane(leaf, start, launch, profile, theme, window, cx);
        workspace.focus_active(window, cx);
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

    /// What a pane with `theme` takes from the current config.
    fn pane_settings(&self, theme: Theme) -> PaneSettings {
        PaneSettings {
            font: self.font.clone(),
            theme,
            close_on_exit: self.config.close_on_exit,
            scrollback: self.config.scrollback as usize,
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
            PaneEvent::TitleChanged => cx.notify(),
            PaneEvent::Exited => self.close_pane(leaf, window, cx),
            // The keyboard follows a prompt dialog of the focused pane in and
            // out, unless the palette or a rename has it.
            PaneEvent::DialogChanged => {
                if leaf == self.tabs.active().focused
                    && self.palette.is_none()
                    && self.rename.is_none()
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

    /// What waits for the first frame (spec 0001): the boot notices and
    /// config reloads. Runs once, at the first frame with output, or
    /// [`SILENT_BOOT_WAIT`] after boot if the program prints nothing.
    fn after_first_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.booted, true) {
            return;
        }
        for notice in std::mem::take(&mut self.notices) {
            show_notice(notice, window, cx);
        }
        self.start_reloads(window, cx);
        cx.notify();
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
        notices.extend(commands::keymap::bind(&config.keybindings, cx));
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
        for notice in notices {
            show_notice(notice, window, cx);
        }
    }

    /// Hands the current font, themes and config to the widgets and every
    /// pane.
    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        theme::apply_to_widgets(&self.theme, cx);
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
                enabled: self.can_run(command.id, window),
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
        let local = self
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
        let placeholder = match axis {
            Axis::Row => "Split right with a profile or host",
            Axis::Column => "Split down with a profile or host",
        };
        let entry = move |host| Entry::SplitWith { host, axis };
        self.pick_host(local, entry, placeholder, window, cx);
    }

    /// Opens the host picker over `first`, then the SSH profiles and the
    /// hosts of `~/.ssh/config`, which is read each time, off the UI thread,
    /// so its edits show. `entry` says what picking a host does.
    fn pick_host(
        &mut self,
        first: Vec<ssh::Host>,
        entry: impl Fn(ssh::Host) -> Entry + 'static,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profiles = self.config.profiles.clone();
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
                let entries = first.into_iter().chain(hosts).map(entry).collect();
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
        let names = theme::list(&self.config.theme.custom, dir.as_deref());
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
            PaletteEvent::OpenProfile(id) => {
                let profile = self.config.profile_or_default(id);
                let leaf = self.tabs.new_tab();
                self.spawn_pane(leaf, profile, None, window, cx);
            }
            PaletteEvent::ForgetHost(id) => self.forget_host(id, window, cx),
            PaletteEvent::SplitWith { id, axis } => {
                let profile = self.config.profile_or_default(id);
                self.split(*axis, Some(profile), window, cx);
            }
            PaletteEvent::PreviewTheme(name) => {
                self.theme = self.theme_for(Some(name), window, cx);
                self.apply_settings(cx);
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
    fn can_run(&self, id: &str, window: &Window) -> bool {
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
            "pane.splitRight" | "pane.splitRightWithProfile" => self.has_room(Axis::Row, window),
            "pane.splitDown" | "pane.splitDownWithProfile" => self.has_room(Axis::Column, window),
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
                    cx.open_with_system(&path);
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

    /// Moves keyboard focus to the active tab's focused pane.
    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.panes.get(&self.tabs.active().focused) {
            let handle = pane.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// Stops a pane's program and forgets the pane.
    fn stop_pane(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        self.pane_subscriptions.remove(&leaf);
        if let Some(pane) = self.panes.remove(&leaf) {
            pane.update(cx, |pane, _| pane.shutdown());
        }
    }

    fn after_close(&mut self, closed: Closed, window: &mut Window, cx: &mut Context<Self>) {
        if closed == Closed::Last {
            cx.quit();
            return;
        }
        self.focus_active(window, cx);
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

    /// A pane's title: its program's, else the name of its profile.
    fn pane_title(&self, leaf: LeafId, cx: &App) -> SharedString {
        let Some(pane) = self.panes.get(&leaf) else {
            return SharedString::default();
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
        self.focus_active(window, cx);
        cx.notify();
    }

    fn close_focused_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.active().focused;
        self.close_pane(leaf, window, cx);
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
            None => (None, None),
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
        self.focus_active(window, cx);
        cx.notify();
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
            self.focus_active(window, cx);
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
            self.focus_active(window, cx);
            cx.notify();
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
        self.focus_active(window, cx);
        cx.notify();
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
        self.focus_active(window, cx);
        cx.notify();
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
        let Some(pane) = self.panes.get(&leaf) else {
            return div().size_full().into_any_element();
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
            .child(pane.clone())
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
                    this.close_tab(ix, window, cx);
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
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                    this.tabs.move_tab(dragged.index, ix);
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

        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let new_tab = Button::new("new-tab")
            .icon(IconName::Plus)
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx)));
        let suffix = div()
            .flex()
            .items_center()
            .child(new_tab)
            .when(client_decorated, |suffix| {
                suffix.child(Self::window_controls(window))
            });
        TabBar::new("tabs")
            .selected_index(self.tabs.active_index())
            .children(tabs)
            .suffix(suffix)
            .last_empty_space(self.title_bar_space(client_decorated, cx))
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
                    .on_click(|_, window, _| window.remove_window()),
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
fn show_notice(notice: String, window: &mut Window, cx: &mut App) {
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
        let workspace = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::hsla(self.theme.background))
            .text_color(theme::hsla(self.theme.foreground))
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_focused_pane))
            .on_action(cx.listener(Self::open_config))
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
            .on_action(cx.listener(|this, _: &ZoomPane, _, cx| {
                this.tabs.toggle_zoom();
                cx.notify();
            }))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::select_theme))
            .on_action(cx.listener(Self::connect_to_host))
            .on_action(cx.listener(Self::forget_password))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.close_tab(this.tabs.active_index(), window, cx);
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
            .child(self.render_tabs(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(content)
                    .child(measure_size(Rc::clone(&self.content_size)))
                    .children(self.render_palette(cx)),
            )
            .children(badge);
        // Shadow and resize edges when the compositor draws no frame;
        // nothing otherwise.
        window_border().child(workspace)
    }
}
