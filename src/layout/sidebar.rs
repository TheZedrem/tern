//! The workspace's side of the sidebar (spec 0018): making it and its
//! built-in panels on first use, the commands, where it's drawn, what the
//! session saves, and what the Files and Port forwards panels show for
//! the active pane. The sidebar and the panels themselves are
//! `crate::sidebar`'s.

use super::{Workspace, show_notice};
use crate::commands::{FocusSidebar, ShowPortForwards, ToggleSidebar};
use crate::session::SavedSidebar;
use crate::sidebar::hosts::{HostsEvent, HostsPanel};
use crate::sidebar::pane::{Body, PanePanel, PanePanelEvent};
use crate::sidebar::{self, Panel, PanelId, Sidebar, SidebarEvent};
use crate::terminal::TerminalPane;
use gpui::{AnyElement, App, AppContext, Context, Entity, Focusable, IntoElement, Window};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{IconName, Sizable};
use std::rc::Rc;

/// The sidebar and its built-in panels.
#[derive(Clone)]
pub(super) struct Hosted {
    sidebar: Entity<Sidebar>,
    files: Entity<PanePanel>,
    /// Registered only while the active pane is an SSH pane.
    forwards: Entity<PanePanel>,
    hosts: Entity<HostsPanel>,
}

/// The sidebar of a window that has no session to restore: closed.
pub(super) fn closed() -> SavedSidebar {
    SavedSidebar {
        open: false,
        width: sidebar::WIDTH,
        panel: PanelId::Files.saved(),
    }
}

impl Workspace {
    /// The sidebar, made now if this is its first use: after the first
    /// frame if the session saved it open, else with the first command
    /// that needs it (spec 0018, Startup impact). The built-in panels
    /// register here the way a plugin's will.
    fn hosted(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Hosted {
        if let Some(hosted) = &self.sidebar {
            return hosted.clone();
        }
        let saved = &self.sidebar_boot;
        let (open, width) = (saved.open, saved.usable_width().unwrap_or(sidebar::WIDTH));
        let shown = PanelId::restored(&saved.panel);
        let side = self.config.app_sidebar;
        let sidebar = cx.new(|cx| Sidebar::new(open, width, shown.clone(), side, cx));
        let files = cx.new(PanePanel::new);
        let forwards = cx.new(PanePanel::new);
        let profiles = self.config.profiles.clone();
        let hosts = cx.new(|cx| HostsPanel::new(profiles, window, cx));
        let (files_focus, hosts_focus) = (files.clone(), hosts.clone());
        sidebar.update(cx, |sidebar, cx| {
            sidebar.register(
                Panel {
                    id: PanelId::Files,
                    title: "Files".into(),
                    icon: IconName::Folder,
                    plugin: None,
                    view: files.clone().into(),
                    focus: Rc::new(move |window, cx| {
                        files_focus.update(cx, |panel, cx| panel.focus(window, cx));
                    }),
                },
                cx,
            );
            sidebar.register(
                Panel {
                    id: PanelId::Hosts,
                    title: "SSH hosts".into(),
                    icon: IconName::Globe,
                    plugin: None,
                    view: hosts.clone().into(),
                    focus: Rc::new(move |window, cx| {
                        hosts_focus.update(cx, |panel, cx| panel.focus(window, cx));
                    }),
                },
                cx,
            );
        });
        cx.subscribe_in(&sidebar, window, Self::sidebar_event)
            .detach();
        cx.subscribe_in(&files, window, |this, _, event, window, cx| match event {
            PanePanelEvent::Connect => this.show_files_of_active(window, cx),
        })
        .detach();
        cx.subscribe_in(&hosts, window, |this, _, event, window, cx| match event {
            HostsEvent::Open(id) => {
                this.open_profile_tab(id, window, cx);
                this.show_active(window, cx);
            }
            HostsEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
        })
        .detach();
        // A sidebar restored on the hosts lists them; Files connects
        // nothing at launch (spec 0018, flow 7).
        if open && shown == PanelId::Hosts {
            hosts.update(cx, |hosts, cx| hosts.reload(cx));
        }
        let hosted = Hosted {
            sidebar,
            files,
            forwards,
            hosts,
        };
        self.sidebar = Some(hosted.clone());
        hosted
    }

    /// Makes a sidebar the session saved open, once the first frame is on
    /// screen with its empty column.
    pub(super) fn restore_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_boot.open {
            self.hosted(window, cx);
            cx.notify();
        }
    }

    fn sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::Shown { panel, by_user } => match panel {
                // Opening the sidebar on Files, or switching to it, is the
                // user asking for the active pane's files. A panel that
                // comes up any other way connects nothing.
                PanelId::Files if *by_user => self.show_files_of_active(window, cx),
                PanelId::Hosts => {
                    if let Some(hosted) = &self.sidebar {
                        hosted.hosts.update(cx, |hosts, cx| hosts.reload(cx));
                    }
                }
                // TODO(T-069): `panel_opened` for a plugin's panel.
                _ => {}
            },
            // TODO(T-069): `panel_closed` for a plugin's panel.
            SidebarEvent::Hidden(_) => {}
            SidebarEvent::Changed => {
                self.save_soon(window, cx);
                cx.notify();
            }
            SidebarEvent::Leave => self.focus_active(window, cx),
        }
    }

    /// Opens the sidebar on the panel it showed last, or closes it (spec
    /// 0018, flow 1).
    pub(super) fn toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hosted = self.hosted(window, cx);
        hosted
            .sidebar
            .update(cx, |sidebar, cx| sidebar.toggle(window, cx));
    }

    /// Puts the keyboard in the sidebar's shown panel, opening it if it's
    /// closed; pressed with the keyboard inside, gives it back to the
    /// active pane (spec 0018, flow 2).
    pub(super) fn focus_sidebar(
        &mut self,
        _: &FocusSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hosted = self.hosted(window, cx);
        if hosted.sidebar.read(cx).has_focus(window, cx) {
            self.focus_active(window, cx);
            return;
        }
        hosted.sidebar.update(cx, |sidebar, cx| sidebar.open(cx));
        // After what opening it started (the active pane's files), so the
        // keyboard lands in what the panel shows then.
        cx.defer_in(window, move |this, window, cx| {
            this.sync_sidebar(window, cx);
            hosted
                .sidebar
                .update(cx, |sidebar, cx| sidebar.focus(window, cx));
        });
    }

    /// Opens the sidebar on the active pane's Port forwards panel, with
    /// the keyboard in the add row (spec 0017, flow 1). A local pane has
    /// no host.
    pub(super) fn show_port_forwards(
        &mut self,
        _: &ShowPortForwards,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaf = self.tabs.active().focused;
        if self.active_ssh_pane(cx).is_none() {
            show_notice("Port forwards need an SSH pane".into(), window, cx);
            return;
        }
        let hosted = self.hosted(window, cx);
        // Registers the panel, shows it, then hands it this pane's view.
        self.sync_sidebar(window, cx);
        hosted
            .sidebar
            .update(cx, |sidebar, cx| sidebar.show(&PanelId::Forwards, cx));
        self.sync_sidebar(window, cx);
        if let Some(forwards) = self.sftp.get(&leaf).and_then(|s| s.forwards.as_ref()) {
            forwards
                .view
                .update(cx, |view, cx| view.focus_add(window, cx));
        }
    }

    /// The active pane with its `user@host`, if it's an SSH pane.
    fn active_ssh_pane(&self, cx: &App) -> Option<(Entity<TerminalPane>, String)> {
        let pane = self.panes.get(&self.tabs.active().focused)?;
        let host = pane.read(cx).ssh_target()?;
        Some((pane.clone(), host))
    }

    /// Starts the active SSH pane's SFTP session if it has none (spec
    /// 0008, Model): only ever by the user's own act on the sidebar,
    /// never by a pane switch or at launch (spec 0018, Files panel).
    fn show_files_of_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let leaf = self.tabs.active().focused;
        let Some((pane, _)) = self.active_ssh_pane(cx) else {
            return;
        };
        if self
            .sftp
            .get(&leaf)
            .is_none_or(|views| views.files.is_none())
        {
            let files = self.sidebar_files(leaf, &pane, window, cx);
            self.sftp.entry(leaf).or_default().files = Some(files);
        }
        cx.notify();
    }

    /// Makes the sidebar follow the active pane (spec 0018, flow 8): Port
    /// forwards is listed only while that pane is an SSH pane, and the
    /// panel on screen is handed that pane's own view, never another's
    /// (TM-sftp-15). Called before every draw, so a pane switch shows in
    /// the same frame; a panel redraws only when what it's handed
    /// differs.
    pub(super) fn sync_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hosted) = self.sidebar.clone() else {
            return;
        };
        let ssh = self.active_ssh_pane(cx);
        self.list_forwards(&hosted, ssh.is_some(), cx);
        let sidebar = hosted.sidebar.read(cx);
        let (files, forwards) = (
            sidebar.shows(&PanelId::Files),
            sidebar.shows(&PanelId::Forwards),
        );
        if files {
            let body = self.files_body(ssh.as_ref(), window, cx);
            hosted
                .files
                .update(cx, |panel, cx| panel.set_body(body, cx));
        }
        if forwards && let Some((pane, _)) = &ssh {
            let body = self.forwards_body(pane, window, cx);
            hosted
                .forwards
                .update(cx, |panel, cx| panel.set_body(body, cx));
        }
    }

    /// Registers the Port forwards panel while an SSH pane is active and
    /// removes it while none is; removing it while it's shown shows Files
    /// (spec 0018, Port forwards panel).
    fn list_forwards(&mut self, hosted: &Hosted, ssh: bool, cx: &mut Context<Self>) {
        let sidebar = hosted.sidebar.read(cx);
        let listed = sidebar.has(&PanelId::Forwards);
        // A sidebar restored on Port forwards names it before it's listed.
        let named = *sidebar.shown() == PanelId::Forwards;
        if ssh && !listed {
            let (view, focus) = (hosted.forwards.clone(), hosted.forwards.clone());
            let panel = Panel {
                id: PanelId::Forwards,
                title: "Port forwards".into(),
                icon: IconName::Network,
                plugin: None,
                view: view.into(),
                focus: Rc::new(move |window, cx| {
                    focus.update(cx, |panel, cx| panel.focus(window, cx));
                }),
            };
            hosted
                .sidebar
                .update(cx, |sidebar, cx| sidebar.register(panel, cx));
        } else if !ssh && (listed || named) {
            hosted
                .sidebar
                .update(cx, |sidebar, cx| sidebar.remove(&PanelId::Forwards, cx));
        }
    }

    /// What the Files panel draws for the active pane: an SSH pane's
    /// session, or the offer to start it; a local pane's Local browser,
    /// made on first sight since it connects nothing.
    fn files_body(
        &mut self,
        ssh: Option<&(Entity<TerminalPane>, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Body {
        let leaf = self.tabs.active().focused;
        let made = |this: &Self, cx: &App| {
            let files = this.sftp.get(&leaf)?.files.as_ref()?;
            Some(Body::View {
                view: files.view.clone().into(),
                focus: files.view.read(cx).focus_handle(cx),
            })
        };
        if let Some(body) = made(self, cx) {
            return body;
        }
        if let Some((_, host)) = ssh {
            return Body::Offer(host.clone().into());
        }
        let Some(pane) = self.panes.get(&leaf).cloned() else {
            return Body::Nothing("This files pane has its own file browser.".into());
        };
        let files = self.sidebar_files(leaf, &pane, window, cx);
        self.sftp.entry(leaf).or_default().files = Some(files);
        made(self, cx).unwrap_or(Body::Nothing("".into()))
    }

    /// What the Port forwards panel draws for SSH pane `pane`: its
    /// host's forwards, made on first sight since listing them connects
    /// nothing.
    fn forwards_body(
        &mut self,
        pane: &Entity<TerminalPane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Body {
        let leaf = self.tabs.active().focused;
        if self
            .sftp
            .get(&leaf)
            .is_none_or(|views| views.forwards.is_none())
            && let Some(view) = self.sidebar_forwards(pane, window, cx)
        {
            self.sftp.entry(leaf).or_default().forwards = Some(view);
        }
        match self.sftp.get(&leaf).and_then(|v| v.forwards.as_ref()) {
            Some(forwards) => Body::View {
                view: forwards.view.clone().into(),
                focus: forwards.view.read(cx).focus_handle(cx),
            },
            None => Body::Nothing("Port forwards need an SSH pane.".into()),
        }
    }

    /// Hands a reloaded config to the sidebar: its side, and the hosts.
    pub(super) fn sidebar_reloaded(&mut self, cx: &mut Context<Self>) {
        let Some(hosted) = &self.sidebar else {
            return;
        };
        let side = self.config.app_sidebar;
        hosted
            .sidebar
            .update(cx, |sidebar, cx| sidebar.set_side(side, cx));
        let profiles = self.config.profiles.clone();
        hosted
            .hosts
            .update(cx, |hosts, cx| hosts.set_profiles(profiles, cx));
    }

    /// What the session saves of the sidebar (spec 0018, Saved).
    pub(super) fn saved_sidebar(&self, cx: &App) -> SavedSidebar {
        match &self.sidebar {
            Some(hosted) => {
                let sidebar = hosted.sidebar.read(cx);
                SavedSidebar {
                    open: sidebar.is_open(),
                    width: sidebar.width(),
                    panel: sidebar.shown().saved(),
                }
            }
            None => self.sidebar_boot.clone(),
        }
    }

    /// The sidebar if it's open and the window has room for it beside the
    /// panes' minimum (spec 0018, flow 9). Before the sidebar exists, a
    /// session that saved it open gets an empty column of its width, so
    /// the first frame lays the panes out where they stay.
    pub(super) fn render_sidebar(&self, window: &Window, cx: &App) -> Option<AnyElement> {
        let saved = self.saved_sidebar(cx);
        if !saved.open {
            return None;
        }
        let window_width = f32::from(window.viewport_size().width);
        let width = sidebar::fitted(saved.usable_width().unwrap_or(sidebar::WIDTH), window_width);
        if window_width - width < self.pane_min(window).0 {
            return None;
        }
        Some(match &self.sidebar {
            Some(hosted) => hosted.sidebar.clone().into_any_element(),
            None => sidebar::placeholder(width, cx).into_any_element(),
        })
    }

    /// The button beside the settings gear that shows or hides the
    /// sidebar.
    pub(super) fn sidebar_button(id: &'static str, cx: &mut Context<Self>) -> Button {
        Button::new(id)
            .icon(IconName::PanelLeft)
            .ghost()
            .small()
            .tooltip("Toggle sidebar")
            .on_click(
                cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx)),
            )
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::commands::{FocusLeft, FocusRight};
    use crate::config::tests::TempDir;
    use crate::config::{self, Config};
    use crate::layout::tests::{idle_pane_of, open_workspace};
    use crate::layout::tree::{Axis, LeafId};
    use crate::layout::{Boot, BootPane};
    use crate::session::Session;
    use crate::session::tests::{files_leaf, leaf, split, tab};
    use crate::sidebar::pane::tests::Drawn;
    use gpui::{Action, TestAppContext, VisualTestContext, px, size};
    use std::path::{Path, PathBuf};

    /// A config with two SSH hosts beside the local shell.
    pub(in crate::layout) fn two_hosts() -> Config {
        let loaded = config::parse(concat!(
            "profiles:\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
            "    user: me\n",
            "  stage:\n",
            "    type: ssh\n",
            "    host: stage.example.com\n",
            "    user: deploy\n",
        ));
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        loaded.config
    }

    const PROD: &str = "me@prod.example.com";
    const STAGE: &str = "deploy@stage.example.com";

    /// Three tabs with the first shown: a local shell (pane 1), `prod`
    /// (pane 2) and `stage` (pane 3). The SSH panes wait for Enter, as
    /// restored ones do, so showing them connects nothing.
    pub(in crate::layout) fn local_and_two_hosts() -> Session {
        let tabs = vec![
            tab(leaf(1, "shell", None, None), 1),
            tab(leaf(2, "prod", None, None), 2),
            tab(leaf(3, "stage", None, None), 3),
        ];
        Session::new(None, tabs, 0)
    }

    /// One tab: `prod` (pane 2), waiting for Enter.
    fn one_host() -> Session {
        Session::new(None, vec![tab(leaf(2, "prod", None, None), 2)], 0)
    }

    /// One tab of two local shells side by side, pane 1 (focused) starting
    /// in `first` and pane 4 in `second`; then `prod` (pane 2), waiting
    /// for Enter.
    fn two_shells_and_a_host(first: &Path, second: &Path) -> Session {
        let shell = |id, dir: &Path| leaf(id, "shell", dir.to_str(), None);
        let shells = split(Axis::Row, 0.5, shell(1, first), shell(4, second));
        let tabs = vec![tab(shells, 1), tab(leaf(2, "prod", None, None), 2)];
        Session::new(None, tabs, 0)
    }

    /// One tab of a local shell (pane 1, focused) beside a files pane on
    /// Local at `folder` (pane 4), which waits and lists nothing; then
    /// `prod` (pane 2).
    fn a_shell_beside_a_files_pane(folder: &Path) -> Session {
        let panes = split(
            Axis::Row,
            0.5,
            leaf(1, "shell", None, None),
            files_leaf(4, None, folder.to_str()),
        );
        let tabs = vec![tab(panes, 1), tab(leaf(2, "prod", None, None), 2)];
        Session::new(None, tabs, 0)
    }

    /// The strip's built-in panels without Port forwards, and with it.
    const BUILT_IN: [PanelId; 2] = [PanelId::Files, PanelId::Hosts];
    const WITH_FORWARDS: [PanelId; 3] = [PanelId::Files, PanelId::Hosts, PanelId::Forwards];

    /// The file browser the sidebar holds for a pane.
    #[derive(Debug)]
    struct Browsed {
        /// As the Files panel draws it.
        view: Drawn,
        local: bool,
        /// What its header names: `Local` or `user@host`.
        target: String,
        /// The folder it shows, or will once it has listed it.
        folder: Option<PathBuf>,
    }

    fn saved(open: bool, width: f32, panel: &str) -> SavedSidebar {
        SavedSidebar {
            open,
            width,
            panel: panel.into(),
        }
    }

    /// `session` booted with [`two_hosts`]: every pane of the shown tab
    /// waits for Enter and runs nothing.
    pub(in crate::layout) fn boot(session: &Session) -> Boot {
        let config = two_hosts();
        Boot::restore(session, (1200.0, 800.0), |saved, _| -> BootPane {
            let profile = config.profile(&saved.profile).expect("a known profile");
            idle_pane_of(&profile, saved)
        })
        .expect("a session with tabs")
    }

    /// A workspace in a test window, driven with keys and commands like
    /// the running app.
    pub(in crate::layout) struct Rig<'a> {
        pub workspace: Entity<Workspace>,
        pub cx: &'a mut VisualTestContext,
    }

    pub(in crate::layout) fn rig<'a>(session: &Session, cx: &'a mut TestAppContext) -> Rig<'a> {
        // A local pane's Local browser lists its folder on a thread of its
        // own, which GPUI's test scheduler rejects unless parking is
        // allowed. No test waits for the listing.
        cx.executor().allow_parking();
        let (workspace, cx) = open_workspace(boot(session), two_hosts(), cx);
        Rig { workspace, cx }
    }

    impl Rig<'_> {
        pub fn draw(&mut self) {
            self.cx.run_until_parked();
            self.cx.update(|window, cx| window.draw(cx).clear(cx));
            self.cx.run_until_parked();
        }

        /// Presses `keys`, with the app's default keymap.
        pub fn keys(&mut self, keys: &str) {
            self.draw();
            self.cx.simulate_keystrokes(keys);
            self.draw();
        }

        /// Runs a command the way the palette does.
        fn run(&mut self, action: impl Action) {
            self.draw();
            self.cx.dispatch_action(action);
            self.draw();
        }

        /// What the session would save of the sidebar.
        fn saved(&mut self) -> SavedSidebar {
            self.workspace
                .read_with(self.cx, |workspace, cx| workspace.saved_sidebar(cx))
        }

        fn window_width(&mut self) -> f32 {
            self.cx
                .update(|window, _| f32::from(window.viewport_size().width))
        }

        /// The width the panes are laid out in, as last drawn.
        fn panes_width(&mut self) -> f32 {
            self.draw();
            self.workspace.read_with(self.cx, |workspace, _| {
                let size = workspace.content_size.get().expect("the panes were drawn");
                f32::from(size.width)
            })
        }

        /// The focused pane of the shown tab.
        fn active(&mut self) -> LeafId {
            self.workspace
                .read_with(self.cx, |workspace, _| workspace.tabs.active().focused)
        }

        /// Whether typing goes to pane `leaf`'s program.
        fn pane_has_the_keyboard(&mut self, leaf: LeafId) -> bool {
            let workspace = self.workspace.clone();
            self.cx.update(|window, cx| {
                let pane = workspace.read(cx).panes[&leaf].read(cx);
                pane.focus_handle(cx).is_focused(window)
            })
        }

        fn sidebar_has_the_keyboard(&mut self) -> bool {
            let workspace = self.workspace.clone();
            self.cx.update(|window, cx| {
                let hosted = workspace.read(cx).sidebar.as_ref();
                hosted.is_some_and(|hosted| hosted.sidebar.read(cx).has_focus(window, cx))
            })
        }

        /// What the Files panel draws, as of the last frame.
        fn files_panel(&mut self) -> Drawn {
            self.draw();
            self.workspace.read_with(self.cx, |workspace, cx| {
                let hosted = workspace.sidebar.as_ref().expect("the sidebar was used");
                hosted.files.read(cx).drawn()
            })
        }

        /// What the Port forwards panel draws, as of the last frame.
        fn forwards_panel(&mut self) -> Drawn {
            self.draw();
            self.workspace.read_with(self.cx, |workspace, cx| {
                let hosted = workspace.sidebar.as_ref().expect("the sidebar was used");
                hosted.forwards.read(cx).drawn()
            })
        }

        /// The strip's panels in its order, as of the last frame.
        fn strip(&mut self) -> Vec<PanelId> {
            self.draw();
            self.workspace.read_with(self.cx, |workspace, cx| {
                let hosted = workspace.sidebar.as_ref().expect("the sidebar was used");
                hosted.sidebar.read(cx).order()
            })
        }

        /// The file browser the sidebar holds for pane `leaf`, if any.
        fn browser(&mut self, leaf: LeafId) -> Option<Browsed> {
            self.workspace.read_with(self.cx, |workspace, cx| {
                let files = workspace.sftp.get(&leaf)?.files.as_ref()?;
                let browser = files.view.read(cx);
                Some(Browsed {
                    view: Drawn::View(files.view.entity_id()),
                    local: browser.is_local(),
                    target: browser.target().to_string(),
                    folder: browser.folder().map(PathBuf::from),
                })
            })
        }

        /// Pane `leaf`'s Local browser, as the Files panel draws it. It
        /// names Local, and no host.
        fn local_browser(&mut self, leaf: LeafId) -> Drawn {
            let browser = self.browser(leaf).expect("a browser for the local pane");
            assert!(browser.local, "pane {leaf}: {browser:?}");
            assert_eq!(browser.target, "Local", "pane {leaf}");
            browser.view
        }

        /// Whether typing goes to the listing of pane `leaf`'s browser in
        /// the sidebar.
        fn browser_has_the_keyboard(&mut self, leaf: LeafId) -> bool {
            let workspace = self.workspace.clone();
            self.cx.update(|window, cx| {
                let views = workspace.read(cx).sftp.get(&leaf);
                let files = views.and_then(|views| views.files.as_ref());
                files.is_some_and(|files| files.view.read(cx).focus_handle(cx).is_focused(window))
            })
        }

        /// The panes an SFTP session was started for: each one is an `ssh`
        /// process. A Local browser is none.
        fn sessions(&mut self) -> Vec<LeafId> {
            self.workspace.read_with(self.cx, |workspace, cx| {
                let mut leaves: Vec<LeafId> = workspace
                    .sftp
                    .iter()
                    .filter(|(_, views)| {
                        let files = views.files.as_ref();
                        files.is_some_and(|files| !files.view.read(cx).is_local())
                    })
                    .map(|(leaf, _)| *leaf)
                    .collect();
                leaves.sort();
                leaves
            })
        }

        /// The Port forwards view made for pane `leaf`, if any.
        fn forwards_of(&mut self, leaf: LeafId) -> Option<Drawn> {
            self.workspace.read_with(self.cx, |workspace, _| {
                let views = workspace.sftp.get(&leaf)?;
                let forwards = views.forwards.as_ref()?;
                Some(Drawn::View(forwards.view.entity_id()))
            })
        }

        /// Makes the sidebar a session saved open, as the workspace does
        /// once the first frame is on screen.
        fn after_first_frame(&mut self) {
            self.workspace.update_in(self.cx, |workspace, window, cx| {
                workspace.restore_sidebar(window, cx);
            });
            self.draw();
        }
    }

    /// Spec 0018, AC1, flow 1 and Show and hide: with a shell focused,
    /// `Ctrl+Shift+S` shows the sidebar on Files the first time, 260 px
    /// wide, and the panes give up that width; pressed again it hides it
    /// and they take the width back. The keyboard stays in the shell the
    /// whole time, and `Ctrl+R` with the sidebar open isn't the sidebar's.
    #[gpui::test]
    fn ctrl_shift_s_shows_the_sidebar_on_files_and_hides_it_with_the_shell_keeping_the_keyboard(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        let window = rig.window_width();
        assert_eq!(rig.saved(), closed());
        assert_eq!(rig.panes_width(), window);

        rig.keys("ctrl-shift-s");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert_eq!(rig.panes_width(), window - 260.0);
        assert!(rig.pane_has_the_keyboard(1), "the toggle took the keyboard");
        let local = rig.local_browser(1);
        assert_eq!(rig.files_panel(), local);

        rig.keys("ctrl-r");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert!(rig.pane_has_the_keyboard(1), "Ctrl+R left the shell");

        rig.keys("ctrl-shift-s");
        assert_eq!(rig.saved(), saved(false, 260.0, "files"));
        assert_eq!(rig.panes_width(), window);
        assert!(rig.pane_has_the_keyboard(1));
        assert_eq!(
            rig.sessions(),
            Vec::<LeafId>::new(),
            "a local pane got an SFTP session"
        );
    }

    /// Spec 0018, Model ("One per window, not per pane") and AC2: a closed
    /// sidebar stays closed and an open one stays open, on Files, while
    /// the user switches tabs, and the keyboard goes with the panes.
    #[gpui::test]
    fn switching_tabs_never_opens_closes_or_switches_the_sidebar(cx: &mut TestAppContext) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        for (keys, leaf) in [("alt-2", 2), ("alt-3", 3), ("alt-1", 1)] {
            rig.keys(keys);
            assert_eq!(rig.active(), leaf, "{keys}");
            assert_eq!(rig.saved(), closed(), "{keys} opened the sidebar");
        }

        rig.keys("ctrl-shift-s");
        let window = rig.window_width();
        for (keys, leaf) in [("alt-2", 2), ("alt-3", 3), ("alt-1", 1), ("alt-3", 3)] {
            rig.keys(keys);
            assert_eq!(rig.active(), leaf, "{keys}");
            assert_eq!(rig.saved(), saved(true, 260.0, "files"), "{keys}");
            assert_eq!(rig.panes_width(), window - 260.0, "{keys}");
            assert!(rig.pane_has_the_keyboard(leaf), "{keys}");
        }
    }

    /// Spec 0018, Files panel [load-bearing], flow 8 and AC2: with Files
    /// shown, switching to an SSH pane that has no session draws that
    /// pane's `user@host` with "Show files" in the next frame and starts
    /// no SFTP session, so no `ssh` process. Each pane gets its own host,
    /// never the one drawn before it, and a local pane its Local browser,
    /// the same one each time (T-049).
    #[gpui::test]
    fn an_ssh_pane_without_a_session_is_offered_show_files_and_connects_nothing(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("ctrl-shift-s");
        let local = rig.local_browser(1);
        assert_eq!(rig.files_panel(), local);

        rig.keys("alt-2");
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));
        rig.keys("alt-3");
        assert_eq!(rig.files_panel(), Drawn::Offer(STAGE.into()));
        rig.keys("alt-2");
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));
        rig.keys("alt-1");
        assert_eq!(rig.files_panel(), local, "not the local pane's browser");
        assert_eq!(rig.local_browser(1), local, "the pane got a new browser");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        for ssh in [2, 3] {
            assert!(rig.browser(ssh).is_none(), "pane {ssh} got a browser");
        }
        assert_eq!(
            rig.sessions(),
            Vec::<LeafId>::new(),
            "a pane switch started an SFTP session"
        );
    }

    /// Spec 0018, Module surface, Port forwards panel and AC4 (T-084),
    /// with spec 0017, flow 1: `pane.portForwards` on an SSH pane opens
    /// the sidebar on the Port forwards panel, drawing that pane's host's
    /// forwards, with the keyboard in it. Showing it connects nothing,
    /// and Files isn't asked for the pane's files by it.
    #[gpui::test]
    fn show_port_forwards_opens_the_sidebar_on_the_port_forwards_panel_and_connects_nothing(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("alt-2");
        rig.run(ShowPortForwards);
        assert_eq!(rig.saved(), saved(true, 260.0, "forwards"));
        assert_eq!(rig.strip(), WITH_FORWARDS);
        let prod = rig.forwards_of(2).expect("prod's port forwards");
        assert_eq!(rig.forwards_panel(), prod);
        assert!(rig.sidebar_has_the_keyboard(), "not in the add row");
        assert_eq!(rig.active(), 2);
        assert!(
            rig.browser(2).is_none(),
            "port forwards made a file browser"
        );
        assert_eq!(
            rig.sessions(),
            Vec::<LeafId>::new(),
            "port forwards started an SFTP session"
        );
    }

    /// Spec 0018, AC4 (T-084): with the sidebar already open on Files for
    /// an SSH pane, `pane.portForwards` switches it to Port forwards, at
    /// the width it has, and the pane's files still aren't connected to.
    #[gpui::test]
    fn show_port_forwards_switches_an_open_sidebar_from_files_to_port_forwards(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("ctrl-shift-s");
        rig.keys("alt-2");
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));

        rig.run(ShowPortForwards);
        assert_eq!(rig.saved(), saved(true, 260.0, "forwards"));
        let prod = rig.forwards_of(2).expect("prod's port forwards");
        assert_eq!(rig.forwards_panel(), prod);
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
    }

    /// Spec 0018, Order [load-bearing], Port forwards panel and AC4
    /// (T-084): with an SSH pane active the strip reads Files, SSH hosts,
    /// Port forwards; with a local pane active it has no Port forwards
    /// button. The panel comes and goes with the active pane (flow 5)
    /// without the shown panel changing, and nothing connects.
    #[gpui::test]
    fn the_strip_lists_port_forwards_only_while_an_ssh_pane_is_active(cx: &mut TestAppContext) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("ctrl-shift-s");
        assert_eq!(rig.strip(), BUILT_IN);

        for (keys, strip) in [
            ("alt-2", &WITH_FORWARDS[..]),
            ("alt-3", &WITH_FORWARDS[..]),
            ("alt-1", &BUILT_IN[..]),
            ("alt-3", &WITH_FORWARDS[..]),
        ] {
            rig.keys(keys);
            assert_eq!(rig.strip(), strip, "{keys}");
            assert_eq!(rig.saved(), saved(true, 260.0, "files"), "{keys}");
        }
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
    }

    /// Spec 0018, Port forwards panel [load-bearing], flows 5 and 8, AC4
    /// (T-084): switching to a local pane while Port forwards is shown
    /// shows Files, with the sidebar still open and that pane's Local
    /// browser in it, and the strip loses the button. Back on the SSH
    /// pane the button returns, but the panel doesn't come back by
    /// itself: Files stays, offering "Show files", and nothing connects.
    #[gpui::test]
    fn leaving_the_ssh_pane_with_port_forwards_shown_shows_files_and_it_does_not_come_back(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("alt-2");
        rig.run(ShowPortForwards);
        rig.keys("escape");
        assert_eq!(rig.saved(), saved(true, 260.0, "forwards"));
        let window = rig.window_width();

        rig.keys("alt-1");
        assert_eq!(rig.active(), 1);
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert_eq!(rig.panes_width(), window - 260.0, "the sidebar closed");
        assert_eq!(rig.strip(), BUILT_IN);
        let local = rig.local_browser(1);
        assert_eq!(rig.files_panel(), local);
        assert!(rig.pane_has_the_keyboard(1));

        rig.keys("alt-2");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"), "it came back");
        assert_eq!(rig.strip(), WITH_FORWARDS);
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
    }

    /// Spec 0018, Port forwards panel and flow 8 (TM-fwd-07): with Port
    /// forwards shown, switching to another SSH pane keeps the panel and
    /// draws that pane's own forwards in the next frame, never the ones
    /// drawn before. Each pane keeps its view: switching back shows the
    /// first pane's again, also after a local pane took the panel away
    /// in between.
    #[gpui::test]
    fn each_ssh_pane_gets_its_own_port_forwards_and_never_another_panes(cx: &mut TestAppContext) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("alt-2");
        rig.run(ShowPortForwards);
        rig.keys("escape");
        let prod = rig.forwards_of(2).expect("prod's port forwards");
        assert_eq!(rig.forwards_panel(), prod);

        rig.keys("alt-3");
        assert_eq!(rig.saved(), saved(true, 260.0, "forwards"));
        let stage = rig.forwards_panel();
        assert!(matches!(stage, Drawn::View(_)), "{stage:?}");
        assert_ne!(stage, prod, "stage shows prod's forwards");
        assert_eq!(Some(&stage), rig.forwards_of(3).as_ref());

        rig.keys("alt-2");
        assert_eq!(rig.saved(), saved(true, 260.0, "forwards"));
        assert_eq!(rig.forwards_panel(), prod, "prod got another view");

        // A local pane takes the panel away; asked for again on stage,
        // it draws stage's forwards, not prod's, which it drew last.
        rig.keys("alt-1");
        rig.keys("alt-3");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        rig.run(ShowPortForwards);
        assert_eq!(rig.saved(), saved(true, 260.0, "forwards"));
        assert_eq!(rig.forwards_panel(), stage);
        assert_eq!(rig.forwards_of(2), Some(prod), "prod lost its view");
        assert_eq!(
            rig.sessions(),
            Vec::<LeafId>::new(),
            "port forwards started an SFTP session"
        );
    }

    /// Spec 0017, flow 1, as spec 0018 moved it: on a local pane there is
    /// no host, so `pane.portForwards` leaves the sidebar closed. With
    /// the sidebar open on Files it stays on Files, and Port forwards
    /// isn't listed.
    #[gpui::test]
    fn show_port_forwards_on_a_local_pane_opens_nothing(cx: &mut TestAppContext) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.run(ShowPortForwards);
        assert_eq!(rig.saved(), closed());
        assert!(rig.pane_has_the_keyboard(1));

        rig.keys("ctrl-shift-s");
        rig.run(ShowPortForwards);
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert_eq!(rig.strip(), BUILT_IN);
        assert!(rig.pane_has_the_keyboard(1));
        assert_eq!(rig.forwards_of(1), None);
    }

    /// Spec 0018, flow 2, flow 1 and AC6: `sidebar.focus` opens a closed
    /// sidebar and puts the keyboard in its panel, without changing which
    /// pane is active. Esc gives the keyboard back to that pane and
    /// leaves the sidebar open; so does `sidebar.focus` pressed with the
    /// keyboard inside. Hiding the sidebar with the keyboard inside gives
    /// it back too.
    #[gpui::test]
    fn sidebar_focus_takes_the_keyboard_and_esc_gives_it_back_with_the_sidebar_open(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.run(FocusSidebar);
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert!(rig.sidebar_has_the_keyboard(), "not in the sidebar");
        assert!(rig.browser_has_the_keyboard(1), "not in the listing");
        assert!(!rig.pane_has_the_keyboard(1));
        assert_eq!(rig.active(), 1);

        rig.keys("escape");
        assert!(rig.pane_has_the_keyboard(1), "Esc didn't give it back");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"), "Esc closed it");

        rig.run(FocusSidebar);
        assert!(rig.sidebar_has_the_keyboard());
        rig.run(FocusSidebar);
        assert!(rig.pane_has_the_keyboard(1), "a second press kept it");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));

        rig.run(FocusSidebar);
        assert!(rig.sidebar_has_the_keyboard());
        rig.keys("ctrl-shift-s");
        assert_eq!(rig.saved(), saved(false, 260.0, "files"));
        assert!(rig.pane_has_the_keyboard(1), "hiding it kept the keyboard");
    }

    /// Spec 0018, AC6: Esc in the Port forwards panel, where
    /// `pane.portForwards` puts the keyboard (spec 0017, flow 1), returns
    /// typing to the pane and leaves the sidebar open on that panel.
    #[gpui::test]
    fn esc_in_the_port_forwards_panel_gives_the_keyboard_back_with_the_sidebar_open(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("alt-2");
        rig.run(ShowPortForwards);
        assert!(rig.sidebar_has_the_keyboard(), "not in the add row");

        rig.keys("escape");
        assert!(rig.pane_has_the_keyboard(2), "Esc didn't give it back");
        assert_eq!(rig.saved(), saved(true, 260.0, "forwards"));
        let prod = rig.forwards_of(2).expect("prod's port forwards");
        assert_eq!(rig.forwards_panel(), prod);
    }

    /// Spec 0018, AC6 and "The keyboard stays with the shell" (T-049):
    /// in the Files panel's Local browser, where `sidebar.focus` puts
    /// the keyboard, Esc with nothing to answer returns typing to the
    /// local pane and leaves the sidebar open on Files with that browser.
    #[gpui::test]
    fn esc_in_the_local_browser_gives_the_keyboard_back_with_the_sidebar_open(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("ctrl-shift-s");
        let local = rig.local_browser(1);
        assert!(
            rig.pane_has_the_keyboard(1),
            "showing Local took the keyboard"
        );

        rig.run(FocusSidebar);
        assert!(rig.browser_has_the_keyboard(1), "not in the listing");
        assert!(!rig.pane_has_the_keyboard(1));

        rig.keys("escape");
        assert!(rig.pane_has_the_keyboard(1), "Esc didn't give it back");
        assert!(!rig.sidebar_has_the_keyboard());
        assert_eq!(rig.saved(), saved(true, 260.0, "files"), "Esc closed it");
        assert_eq!(rig.files_panel(), local);
    }

    /// Spec 0018, Saved and AC3, AC7: without a session to restore (none
    /// saved yet, or `restore_session: false`, where boot reads none) and
    /// with one saved before the sidebar existed, the sidebar starts
    /// closed, 260 px wide, and opens on Files.
    #[gpui::test]
    fn without_a_saved_sidebar_it_starts_closed_and_opens_on_files(cx: &mut TestAppContext) {
        let session = local_and_two_hosts();
        assert_eq!(session.sidebar, None);
        let mut rig = rig(&session, cx);
        assert_eq!(rig.saved(), saved(false, 260.0, "files"));
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window);
        rig.after_first_frame();
        assert_eq!(rig.panes_width(), window, "it opened after the first frame");

        rig.keys("ctrl-shift-s");
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
    }

    /// Spec 0018, Saved: a fresh window, as boot opens without a session,
    /// has the sidebar closed on Files.
    #[gpui::test]
    fn a_fresh_window_has_the_sidebar_closed_on_files(cx: &mut TestAppContext) {
        let pane = idle_pane_of(
            &config::Profile::shell(),
            &crate::session::SavedLeaf {
                id: 0,
                profile: config::SHELL.into(),
                cwd: None,
                title: None,
                files: None,
            },
        );
        let (workspace, cx) = open_workspace(Boot::fresh(pane), Config::default(), cx);
        let mut rig = Rig { workspace, cx };
        assert_eq!(rig.saved(), saved(false, 260.0, "files"));
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window);
    }

    /// Spec 0018, Startup impact, flow 7 and AC7: a sidebar saved open at
    /// 320 px on an SSH pane's files has its width taken from the panes on
    /// the first frame, before the sidebar itself exists, and nothing
    /// resizes them when it is made after that frame. It comes back on
    /// Files with the pane's `user@host` and "Show files": the relaunch
    /// starts no SFTP session, and the keyboard is in the pane.
    #[gpui::test]
    fn a_sidebar_saved_open_has_its_width_on_the_first_frame_and_connects_nothing(
        cx: &mut TestAppContext,
    ) {
        let session = one_host().with_sidebar(saved(true, 320.0, "files"));
        let mut rig = rig(&session, cx);
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window - 320.0, "the first frame");
        let made = rig
            .workspace
            .read_with(rig.cx, |workspace, _| workspace.sidebar.is_some());
        assert!(!made, "the sidebar was made before the first frame");

        rig.after_first_frame();
        assert_eq!(rig.panes_width(), window - 320.0, "the panes were resized");
        assert_eq!(rig.saved(), saved(true, 320.0, "files"));
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));
        assert_eq!(
            rig.sessions(),
            Vec::<LeafId>::new(),
            "the relaunch started an SFTP session"
        );
        assert!(rig.pane_has_the_keyboard(2));
    }

    /// Spec 0018, flow 7 and AC10: a session that was saved on a plugin's
    /// panel comes back on Files, open as it was, so no plugin is started
    /// by the relaunch; and Files connects nothing either.
    #[gpui::test]
    fn a_sidebar_saved_on_a_plugins_panel_comes_back_on_files(cx: &mut TestAppContext) {
        let session = one_host().with_sidebar(saved(true, 300.0, "plugin.docker.containers"));
        let mut rig = rig(&session, cx);
        rig.after_first_frame();
        assert_eq!(rig.saved(), saved(true, 300.0, "files"));
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
    }

    /// Spec 0018, flow 7 and Saved: a closed sidebar that showed a
    /// plugin's panel last opens on Files after a relaunch, at the width
    /// it was saved with.
    #[gpui::test]
    fn a_closed_sidebar_saved_on_a_plugins_panel_opens_on_files(cx: &mut TestAppContext) {
        let session =
            local_and_two_hosts().with_sidebar(saved(false, 300.0, "plugin.docker.containers"));
        let mut rig = rig(&session, cx);
        rig.after_first_frame();
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window, "a closed sidebar took room");

        rig.keys("ctrl-shift-s");
        assert_eq!(rig.saved(), saved(true, 300.0, "files"));
        assert_eq!(rig.panes_width(), window - 300.0);
    }

    /// Spec 0018, Width and Saved: a saved width no sidebar can have is
    /// not used, on the first frame or after it: the sidebar is 260 px
    /// wide.
    #[gpui::test]
    fn a_saved_width_of_zero_gives_the_default_width(cx: &mut TestAppContext) {
        let session = one_host().with_sidebar(saved(true, 0.0, "files"));
        let mut rig = rig(&session, cx);
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window - 260.0, "the first frame");
        rig.after_first_frame();
        assert_eq!(rig.panes_width(), window - 260.0);
    }

    /// Spec 0018, Width: a saved width over half the window is drawn at
    /// half the window, on the first frame and after it, and one under
    /// 200 px at 200 px.
    #[gpui::test]
    fn a_saved_width_past_the_limits_is_drawn_within_them(cx: &mut TestAppContext) {
        let session = one_host().with_sidebar(saved(true, 50_000.0, "files"));
        let mut rig = rig(&session, cx);
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window / 2.0, "the first frame");
        rig.after_first_frame();
        assert_eq!(rig.panes_width(), window / 2.0);
    }

    /// Spec 0018, Width: a saved width under 200 px is drawn at 200 px.
    #[gpui::test]
    fn a_saved_width_under_200_px_is_drawn_at_200_px(cx: &mut TestAppContext) {
        let session = one_host().with_sidebar(saved(true, 80.0, "files"));
        let mut rig = rig(&session, cx);
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window - 200.0, "the first frame");
        rig.after_first_frame();
        assert_eq!(rig.panes_width(), window - 200.0);
    }

    /// Spec 0018, flow 9 and Width: in a window too narrow for 200 px
    /// beside the panes' minimum the sidebar isn't drawn, and the panes
    /// have the whole width. Its flag stays, so it returns when the
    /// window grows, clamped to half of a window that is narrower than
    /// before.
    #[gpui::test]
    fn in_a_window_too_narrow_the_sidebar_is_not_drawn_and_returns_when_it_grows(
        cx: &mut TestAppContext,
    ) {
        let mut rig = rig(&local_and_two_hosts(), cx);
        rig.keys("ctrl-shift-s");
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window - 260.0);

        rig.cx.simulate_resize(size(px(250.0), px(600.0)));
        assert_eq!(rig.window_width(), 250.0, "the test window wasn't resized");
        assert_eq!(rig.panes_width(), 250.0, "the sidebar is drawn");
        assert!(rig.saved().open, "the flag was dropped");

        rig.cx.simulate_resize(size(px(480.0), px(600.0)));
        assert_eq!(rig.panes_width(), 240.0, "not clamped to half the window");
        rig.cx.simulate_resize(size(px(1200.0), px(600.0)));
        assert_eq!(rig.panes_width(), 1200.0 - 260.0);
        assert!(rig.pane_has_the_keyboard(1));
    }

    /// Spec 0018, flow 7 (T-084): a sidebar saved open on Port forwards
    /// comes back on Port forwards when the active pane is an SSH pane,
    /// drawing that pane's forwards. The relaunch connects nothing and
    /// leaves the keyboard in the pane.
    #[gpui::test]
    fn a_sidebar_saved_on_port_forwards_comes_back_on_it_with_an_ssh_pane_active(
        cx: &mut TestAppContext,
    ) {
        let session = one_host().with_sidebar(saved(true, 300.0, "forwards"));
        let mut rig = rig(&session, cx);
        rig.after_first_frame();
        assert_eq!(rig.saved(), saved(true, 300.0, "forwards"));
        assert_eq!(rig.strip(), WITH_FORWARDS);
        let prod = rig.forwards_of(2).expect("prod's port forwards");
        assert_eq!(rig.forwards_panel(), prod);
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
        assert!(rig.pane_has_the_keyboard(2));
    }

    /// Spec 0018, flow 7 (T-084): a sidebar saved open on Port forwards
    /// comes back on Files when the active pane is a local one, open as
    /// it was and showing that pane's Local browser, with no Port
    /// forwards button. Switching to an SSH pane afterwards doesn't bring
    /// Port forwards back by itself.
    #[gpui::test]
    fn a_sidebar_saved_on_port_forwards_comes_back_on_files_with_a_local_pane_active(
        cx: &mut TestAppContext,
    ) {
        let session = local_and_two_hosts().with_sidebar(saved(true, 300.0, "forwards"));
        let mut rig = rig(&session, cx);
        rig.after_first_frame();
        assert_eq!(rig.saved(), saved(true, 300.0, "files"));
        assert_eq!(rig.strip(), BUILT_IN);
        let local = rig.local_browser(1);
        assert_eq!(rig.files_panel(), local);
        assert!(rig.pane_has_the_keyboard(1));

        rig.keys("alt-2");
        assert_eq!(rig.saved(), saved(true, 300.0, "files"), "it came back");
        assert_eq!(rig.strip(), WITH_FORWARDS);
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
    }

    /// Spec 0018, flow 7 and Saved (T-084): a closed sidebar that showed
    /// Port forwards last opens on Port forwards after a relaunch when
    /// the active pane is an SSH pane, and connects nothing.
    #[gpui::test]
    fn a_closed_sidebar_saved_on_port_forwards_opens_on_it_on_an_ssh_pane(cx: &mut TestAppContext) {
        let session = one_host().with_sidebar(saved(false, 300.0, "forwards"));
        let mut rig = rig(&session, cx);
        rig.after_first_frame();
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window, "a closed sidebar took room");

        rig.keys("ctrl-shift-s");
        assert_eq!(rig.saved(), saved(true, 300.0, "forwards"));
        assert_eq!(rig.strip(), WITH_FORWARDS);
        let prod = rig.forwards_of(2).expect("prod's port forwards");
        assert_eq!(rig.forwards_panel(), prod);
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
        assert!(rig.pane_has_the_keyboard(2));
    }

    /// Spec 0018, flow 7 and Saved (T-084): a closed sidebar that showed
    /// Port forwards last opens on Files after a relaunch when the active
    /// pane is a local one.
    #[gpui::test]
    fn a_closed_sidebar_saved_on_port_forwards_opens_on_files_on_a_local_pane(
        cx: &mut TestAppContext,
    ) {
        let session = local_and_two_hosts().with_sidebar(saved(false, 300.0, "forwards"));
        let mut rig = rig(&session, cx);
        rig.after_first_frame();

        rig.keys("ctrl-shift-s");
        assert_eq!(rig.saved(), saved(true, 300.0, "files"));
        assert_eq!(rig.strip(), BUILT_IN);
        let local = rig.local_browser(1);
        assert_eq!(rig.files_panel(), local);
        assert!(rig.pane_has_the_keyboard(1));
    }

    /// Spec 0018, Files panel, *Local pane* (T-049) and flow 7: a sidebar
    /// saved open on Files with a local pane active shows the Local
    /// target once the sidebar is made after the first frame, with no
    /// act of the user's, since nothing connects: no `ssh` is started
    /// and the keyboard stays in the pane.
    #[gpui::test]
    fn a_sidebar_saved_open_on_files_shows_local_for_a_local_pane_at_once(cx: &mut TestAppContext) {
        let session = local_and_two_hosts().with_sidebar(saved(true, 320.0, "files"));
        let mut rig = rig(&session, cx);
        let window = rig.window_width();
        assert_eq!(rig.panes_width(), window - 320.0, "the first frame");

        rig.after_first_frame();
        assert_eq!(rig.panes_width(), window - 320.0, "the panes were resized");
        assert_eq!(rig.saved(), saved(true, 320.0, "files"));
        let local = rig.local_browser(1);
        assert_eq!(rig.files_panel(), local);
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
        assert!(rig.pane_has_the_keyboard(1));
    }

    /// Spec 0018, Files panel [load-bearing] (T-049), flow 8 and AC2:
    /// each local pane has its own Local browser, shown in the frame
    /// after the pane is focused and kept while the pane lives:
    /// switching back to a pane shows the browser it had, also after an
    /// SSH pane's offer was drawn in between. The sidebar stays open on
    /// Files, the keyboard goes with the panes, and no `ssh` is started.
    #[gpui::test]
    fn each_local_pane_keeps_its_own_local_browser_across_switches(cx: &mut TestAppContext) {
        let first_dir = TempDir::new("sidebar-two-shells-first");
        let second_dir = TempDir::new("sidebar-two-shells-second");
        let session = two_shells_and_a_host(first_dir.path(), second_dir.path());
        let mut rig = rig(&session, cx);
        rig.keys("ctrl-shift-s");
        let first = rig.local_browser(1);
        assert_eq!(rig.files_panel(), first);
        assert!(rig.browser(4).is_none(), "a pane not shown got a browser");

        rig.run(FocusRight);
        assert_eq!(rig.active(), 4);
        let second = rig.local_browser(4);
        assert_ne!(second, first, "two panes share a browser");
        assert_eq!(rig.files_panel(), second);
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert!(rig.pane_has_the_keyboard(4));

        rig.run(FocusLeft);
        assert_eq!(rig.active(), 1);
        assert_eq!(rig.files_panel(), first, "pane 1 lost its browser");

        rig.keys("alt-2");
        assert_eq!(rig.files_panel(), Drawn::Offer(PROD.into()));
        rig.keys("alt-1");
        assert_eq!(rig.files_panel(), first);
        rig.run(FocusRight);
        assert_eq!(rig.files_panel(), second, "pane 4 lost its browser");
        assert_eq!(rig.local_browser(1), first);
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
    }

    /// Spec 0018, Files panel, *Local pane* (T-049) and AC2: the Local
    /// browser starts in the folder the pane's shell last reported, else
    /// in the pane's start folder. These panes' shells have reported
    /// nothing, so each browser shows its own pane's start folder.
    #[gpui::test]
    fn a_local_panes_browser_starts_in_the_panes_start_folder(cx: &mut TestAppContext) {
        let first_dir = TempDir::new("sidebar-start-folder-first");
        let second_dir = TempDir::new("sidebar-start-folder-second");
        let session = two_shells_and_a_host(first_dir.path(), second_dir.path());
        let mut rig = rig(&session, cx);
        rig.keys("ctrl-shift-s");
        rig.run(FocusRight);
        rig.draw();

        let same = |folder: &Option<PathBuf>, dir: &Path| {
            let canonical = |path: &Path| std::fs::canonicalize(path).ok();
            folder.as_deref().and_then(canonical) == canonical(dir)
        };
        for (leaf, dir) in [(1, first_dir.path()), (4, second_dir.path())] {
            let browser = rig.browser(leaf).expect("the pane's Local browser");
            assert!(
                same(&browser.folder, dir),
                "pane {leaf} shows {:?}, not its start folder {dir:?}",
                browser.folder
            );
        }
    }

    /// Spec 0018, Files panel, *Files pane*: a files pane has its own
    /// browser, so with one active the panel only says so: it draws no
    /// browser and no "Show files", makes none for that pane, and lists
    /// no Port forwards. Back on the local pane beside it, that pane's
    /// Local browser is shown again.
    #[gpui::test]
    fn with_a_files_pane_active_the_files_panel_only_says_it_has_its_own_browser(
        cx: &mut TestAppContext,
    ) {
        let folder = TempDir::new("sidebar-files-pane");
        let mut rig = rig(&a_shell_beside_a_files_pane(folder.path()), cx);
        rig.keys("ctrl-shift-s");
        let local = rig.local_browser(1);
        assert_eq!(rig.files_panel(), local);

        rig.run(FocusRight);
        assert_eq!(rig.active(), 4);
        let drawn = rig.files_panel();
        let Drawn::Nothing(why) = &drawn else {
            panic!("a files pane is drawn {drawn:?}");
        };
        assert!(why.contains("own file browser"), "{why:?}");
        assert!(
            rig.browser(4).is_none(),
            "the files pane got a second browser"
        );
        assert_eq!(rig.saved(), saved(true, 260.0, "files"));
        assert_eq!(rig.strip(), BUILT_IN);

        rig.run(FocusLeft);
        assert_eq!(rig.active(), 1);
        assert_eq!(rig.files_panel(), local);
        assert_eq!(rig.sessions(), Vec::<LeafId>::new());
    }
}
