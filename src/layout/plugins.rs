//! The workspace's side of plugins (spec 0016): it starts the host after
//! the first frame, hands it the reloaded config and the panes that open
//! and close, shows its notices, and registers its plugins' panels with
//! the sidebar (spec 0018, flows 5 and 6). The host owns the plugins and
//! each panel its contents; the workspace only hosts them.

use super::tree::LeafId;
use super::{Workspace, show_notice};
use crate::commands::OpenPluginsFolder;
use crate::plugins::{self, Action, Host, HostEvent, PaneInfo};
use crate::sidebar::plugin::{self as panel_view, PluginPanel, PluginPanelEvent};
use crate::sidebar::{Panel, PanelId};
use gpui::{AppContext, Context, Entity, Window};
use std::rc::Rc;

/// The plugin host and the panels of its plugins. A panel lives from the
/// moment its plugin is found enabled until it is disabled, whether or not
/// the sidebar exists yet, so what a plugin sent is there when it's shown.
pub(super) struct Plugins {
    pub(super) host: Entity<Host>,
    panels: Vec<(PanelId, Entity<PluginPanel>)>,
    /// Requests waiting on a pane's bar (spec 0016, Confirmation).
    pub(super) requests: Vec<super::requests::Request>,
}

impl Plugins {
    fn panel(&self, plugin: &str, panel: &str) -> Option<(&PanelId, &Entity<PluginPanel>)> {
        self.panels
            .iter()
            .find(|(id, _)| matches!(id, PanelId::Plugin { plugin: p, panel: n } if p == plugin && n == panel))
            .map(|(id, view)| (id, view))
    }
}

impl Workspace {
    /// The plugin host, once plugins were looked for after the first
    /// frame.
    pub(super) fn plugin_host(&self) -> Option<Entity<Host>> {
        self.plugins.as_ref().map(|plugins| plugins.host.clone())
    }

    /// Opens the plugins folder in the OS's file manager, creating it off
    /// the UI thread first; it starts and enables nothing (spec 0016,
    /// Open plugins folder).
    pub(super) fn open_plugins_folder(
        &mut self,
        _: &OpenPluginsFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = plugins::root() else {
            show_notice("This system has no data directory.".into(), window, cx);
            return;
        };
        let created = cx.background_spawn({
            let root = root.clone();
            async move { std::fs::create_dir_all(&root) }
        });
        cx.spawn_in(window, async move |_, cx| {
            let result = created.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => cx.open_with_system(&root),
                Err(error) => {
                    let notice = format!("Couldn't create {}: {error}", root.display());
                    show_notice(notice, window, cx);
                }
            });
        })
        .detach();
    }

    /// Looks for plugins (flow 1). Called after the first frame; the host
    /// reads the folders off the UI thread.
    pub(super) fn start_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let granted = self.config.plugins.clone();
        let host = cx.new(|cx| Host::new(plugins::root(), granted, cx));
        cx.subscribe_in(&host, window, |this, _, event, window, cx| match event {
            HostEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
            HostEvent::PanelsChanged => this.sync_plugin_panels(window, cx),
            HostEvent::Action { plugin, action } => this.plugin_action(plugin, action, window, cx),
        })
        .detach();
        host.update(cx, |host, cx| host.discover(cx));
        self.plugins = Some(Plugins {
            host,
            panels: Vec::new(),
            requests: Vec::new(),
        });
    }

    /// Hands a reloaded config's `plugins:` to the host (flow 2).
    pub(super) fn plugins_reloaded(&mut self, cx: &mut Context<Self>) {
        if let Some(plugins) = &self.plugins {
            let granted = self.config.plugins.clone();
            plugins
                .host
                .update(cx, |host, cx| host.set_granted(granted, cx));
        }
    }

    /// Tells the plugins that hear it that terminal pane `leaf` opened.
    // TODO(OQ-037): panes that were open before a plugin was found or
    // enabled are not told to it.
    pub(super) fn plugins_pane_opened(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        let (Some(plugins), Some(pane)) = (&self.plugins, self.panes.get(&leaf)) else {
            return;
        };
        let pane = pane.read(cx);
        let profile = pane.profile();
        let info = PaneInfo {
            title: pane.title().unwrap_or(&profile.name).to_owned(),
            cwd: pane.cwd().map(|cwd| cwd.display().to_string()),
            profile: profile.id.clone(),
            host: pane.ssh_target(),
        };
        plugins
            .host
            .update(cx, |host, cx| host.pane_opened(leaf, &info, cx));
    }

    pub(super) fn plugins_pane_closed(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        self.drop_requests_of(leaf, cx);
        if let Some(plugins) = &self.plugins {
            plugins
                .host
                .update(cx, |host, cx| host.pane_closed(leaf, cx));
        }
    }

    /// Makes the panels follow what the host lists (spec 0018, flow 5): a
    /// panel of a plugin that is enabled and usable now is made and
    /// registered, one whose plugin is disabled is removed, and each is
    /// told whether its plugin has stopped. Registered with the sidebar
    /// exactly as the built-in panels are, once the sidebar exists.
    pub(super) fn sync_plugin_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plugins) = &mut self.plugins else {
            return;
        };
        let listed = plugins.host.read(cx).panels();
        let sidebar = self.sidebar.as_ref().map(|hosted| hosted.sidebar.clone());
        let font = self.font.family.clone();
        let mut kept = Vec::with_capacity(listed.len());
        for listed in &listed {
            let id = PanelId::Plugin {
                plugin: listed.plugin.clone(),
                panel: listed.panel.clone(),
            };
            let known = plugins.panels.iter().position(|(known, _)| *known == id);
            let view = match known {
                Some(at) => plugins.panels.swap_remove(at).1,
                None => {
                    let view = cx.new(|cx| PluginPanel::new(font.clone(), window, cx));
                    let (plugin, panel) = (listed.plugin.clone(), listed.panel.clone());
                    cx.subscribe_in(&view, window, move |this, _, event, window, cx| {
                        this.plugin_panel_event(&plugin, &panel, event, window, cx);
                    })
                    .detach();
                    view
                }
            };
            view.update(cx, |view, cx| view.set_state(listed, font.clone(), cx));
            if let Some(sidebar) = sidebar
                .as_ref()
                .filter(|sidebar| !sidebar.read(cx).has(&id))
            {
                let focus = view.clone();
                let panel = Panel {
                    id: id.clone(),
                    title: listed.title.clone().into(),
                    icon: panel_view::icon(listed.icon.as_deref(), cx),
                    plugin: Some(listed.name.clone().into()),
                    view: view.clone().into(),
                    focus: Rc::new(move |window, cx| {
                        focus.update(cx, |panel, cx| panel.focus(window, cx));
                    }),
                };
                sidebar.update(cx, |sidebar, cx| sidebar.register(panel, cx));
            }
            kept.push((id, view));
        }
        // What is left belongs to plugins that are no longer listed.
        for (id, _) in std::mem::replace(&mut plugins.panels, kept) {
            if let Some(sidebar) = &sidebar {
                sidebar.update(cx, |sidebar, cx| sidebar.remove(&id, cx));
            }
        }
    }

    /// Carries out what a plugin asked for (spec 0016, flow 4). The host
    /// already checked its rights and that the panel is the plugin's own.
    fn plugin_action(
        &mut self,
        plugin: &str,
        action: &Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(plugins) = &self.plugins else {
            return;
        };
        let host = plugins.host.clone();
        let view = |panel: &str| plugins.panel(plugin, panel).map(|(_, view)| view.clone());
        match action {
            Action::SetPanel { panel, items } => {
                if let Some(view) = view(panel) {
                    let items = items.clone();
                    view.update(cx, |view, cx| view.set_items(items, window, cx));
                }
            }
            Action::AppendLog { panel, text } => {
                if let Some(view) = view(panel) {
                    view.update(cx, |view, cx| view.append_log(text, cx));
                }
            }
            Action::OpenPanel { panel } => {
                let Some(id) = plugins.panel(plugin, panel).map(|(id, _)| id.clone()) else {
                    return;
                };
                self.open_plugin_panel(plugin, id, window, cx);
            }
            Action::Notify { text } => host.update(cx, |host, cx| host.notify(plugin, text, cx)),
            Action::FillCommand { .. }
            | Action::RunCommand { .. }
            | Action::ReadOutput { .. }
            | Action::OpenTab { .. } => self.plugin_request(plugin, action, window, cx),
            // The host answers these itself.
            Action::Ready | Action::SetSettings { .. } => {}
        }
    }

    /// `open_panel` (spec 0018, flow 6): opens the sidebar on the plugin's
    /// panel `id` without moving the keyboard. The host let it through
    /// because the user acted on the plugin; it is still dropped while the
    /// keyboard is in another panel, so no panel is pulled away under
    /// typing.
    fn open_plugin_panel(
        &mut self,
        plugin: &str,
        id: PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sidebar = self.hosted(window, cx).sidebar;
        let typing_elsewhere = {
            let sidebar = sidebar.read(cx);
            sidebar.has_focus(window, cx) && *sidebar.shown() != id
        };
        if typing_elsewhere {
            if let Some(plugins) = &self.plugins {
                let why = "`open_panel` was dropped: the keyboard is in another panel".to_owned();
                plugins
                    .host
                    .update(cx, |host, cx| host.dropped(plugin, why, cx));
            }
            return;
        }
        sidebar.update(cx, |sidebar, cx| sidebar.show_unasked(&id, cx));
    }

    /// What the user did in `panel` of `plugin`, told to the plugin; a
    /// command button's command is carried out too (spec 0016, Command
    /// buttons).
    fn plugin_panel_event(
        &mut self,
        plugin: &str,
        panel: &str,
        event: &PluginPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.plugin_host() else {
            return;
        };
        host.update(cx, |host, cx| match event {
            PluginPanelEvent::Action(id) | PluginPanelEvent::Command { id, .. } => {
                host.panel_action(plugin, panel, id, cx)
            }
            PluginPanelEvent::Input(text) => host.panel_input(plugin, panel, text, cx),
            PluginPanelEvent::Restart => host.restart(plugin, panel, cx),
        });
        if let PluginPanelEvent::Command { command, .. } = event {
            self.plugin_button(plugin, command.clone(), window, cx);
        }
    }

    /// A plugin's panel came on screen (`shown`) or left it: its plugin is
    /// told (spec 0018, Module surface).
    pub(super) fn plugin_panel_shown(&mut self, id: &PanelId, shown: bool, cx: &mut Context<Self>) {
        let (Some(plugins), PanelId::Plugin { plugin, panel }) = (&self.plugins, id) else {
            return;
        };
        plugins.host.update(cx, |host, cx| {
            if shown {
                host.panel_opened(plugin, panel, cx);
            } else {
                host.panel_closed(plugin, panel, cx);
            }
        });
    }
}

/// Spec 0018, flows 5 and 6, AC9 and AC10; spec 0016, Events: the
/// workspace between the plugin host, the panels and the sidebar, against
/// real plugin processes in a folder of the test's own.
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::commands::FocusSidebar;
    use crate::config::PluginSettings;
    use crate::layout::sidebar::tests::{Rig, local_and_two_hosts, rig};
    use crate::plugins::panel::Item;
    use crate::plugins::tests::{
        Lab, grant, names, pids, run, settle, wait_for_events, wait_until,
    };
    use gpui::TestAppContext;
    use std::collections::BTreeMap;

    fn panel(plugin: &str, panel: &str) -> PanelId {
        PanelId::Plugin {
            plugin: plugin.into(),
            panel: panel.into(),
        }
    }

    fn granted(plugins: &[(&str, PluginSettings)]) -> BTreeMap<String, PluginSettings> {
        let named = plugins
            .iter()
            .map(|(id, plugin)| ((*id).to_owned(), plugin.clone()));
        named.collect()
    }

    /// Gives the workspace a host for the lab's plugins and waits for
    /// their panels. `start_plugins` can't be used: it reads the user's
    /// own plugins folder. So the host's events are handed to the
    /// workspace here, the way `start_plugins` hands them over.
    pub(in crate::layout) fn start(
        rig: &mut Rig<'_>,
        lab: &Lab,
        plugins: &[(&str, PluginSettings)],
    ) -> Entity<Host> {
        lab.ends_with(rig.cx);
        let (root, granted) = (lab.root(), granted(plugins));
        let host = rig.workspace.update_in(rig.cx, |workspace, window, cx| {
            let host = cx.new(|cx| Host::new(Some(root), granted, cx));
            cx.subscribe_in(&host, window, |this, _, event, window, cx| match event {
                HostEvent::Notice(_) => {}
                HostEvent::PanelsChanged => this.sync_plugin_panels(window, cx),
                HostEvent::Action { plugin, action } => {
                    this.plugin_action(plugin, action, window, cx);
                }
            })
            .detach();
            host.update(cx, |host, cx| host.discover(cx));
            workspace.plugins = Some(Plugins {
                host: host.clone(),
                panels: Vec::new(),
                requests: Vec::new(),
            });
            host
        });
        wait_until(rig.cx, "the plugins' panels", |cx| {
            host.read_with(cx, |host, _| !host.panels().is_empty())
        });
        rig.draw();
        host
    }

    /// Shows panel `id` in the sidebar, as a strip button does.
    fn show(rig: &mut Rig<'_>, id: &PanelId) {
        rig.workspace.update_in(rig.cx, |workspace, window, cx| {
            let sidebar = workspace.hosted(window, cx).sidebar;
            sidebar.update(cx, |sidebar, cx| sidebar.show(id, cx));
        });
        rig.draw();
    }

    /// The view of `plugin`'s `panel`.
    pub(in crate::layout) fn view(
        rig: &mut Rig<'_>,
        plugin: &str,
        panel: &str,
    ) -> Entity<PluginPanel> {
        rig.workspace.read_with(rig.cx, |workspace, _| {
            let plugins = workspace.plugins.as_ref().expect("the host was started");
            let (_, view) = plugins.panel(plugin, panel).expect("the panel was made");
            view.clone()
        })
    }

    /// Whether the sidebar ignores clicks right now.
    fn sidebar_ignores_clicks(rig: &mut Rig<'_>) -> bool {
        rig.workspace.read_with(rig.cx, |workspace, cx| {
            let hosted = workspace.sidebar.as_ref().expect("the sidebar was used");
            hosted.sidebar.read(cx).ignores_clicks()
        })
    }

    /// Carries out `action` of `plugin`, as when the host hands it over.
    fn carry_out(rig: &mut Rig<'_>, plugin: &str, action: Action) {
        rig.workspace.update_in(rig.cx, |workspace, window, cx| {
            workspace.plugin_action(plugin, &action, window, cx);
        });
        rig.draw();
    }

    /// Spec 0018, flow 5, Order and AC9: plugins' panels "appear through
    /// the same registration the built-in panels use": the strip reads
    /// Files, SSH hosts, then the plugins' by plugin name ignoring case
    /// (the name, not the folder), each plugin's in manifest order.
    /// Module surface: a plugin's panel coming on screen and leaving it
    /// are its `panel_opened` and `panel_closed`, also when the sidebar
    /// closes and opens; listing a panel starts no plugin, and the
    /// keyboard stays in the shell. A disabled plugin's panels leave, and
    /// "if the shown panel leaves, Files is shown".
    #[gpui::test]
    fn a_plugins_panels_are_in_the_strip_and_showing_one_tells_its_plugin(cx: &mut TestAppContext) {
        let lab = Lab::new("workspace-panels");
        let argv = serde_json::to_string(&run("stay")).expect("an argv is JSON");
        let manifest = |id: &str, name: &str, panels: &str| {
            format!("api: 1\nid: {id}\nname: {name}\nrun: {argv}\npanels: [{panels}]\n")
        };
        let two = "{ id: two, title: Two }, { id: one, title: One }";
        let alpha = lab.folder("zz", &manifest("zz", "alpha", two));
        let zeta = lab.folder("aa", &manifest("aa", "Zeta", "{ id: only, title: Only }"));
        let mut rig = rig(&local_and_two_hosts(), cx);
        let enabled = || grant("stay", &[], &[]);
        let host = start(&mut rig, &lab, &[("zz", enabled()), ("aa", enabled())]);

        rig.keys("ctrl-shift-s");
        assert_eq!(
            rig.strip(),
            [
                PanelId::Files,
                PanelId::Hosts,
                panel("zz", "two"),
                panel("zz", "one"),
                panel("aa", "only"),
            ]
        );
        settle(rig.cx);
        for folder in [&alpha, &zeta] {
            assert_eq!(pids(folder), [""; 0], "a listed panel started its plugin");
        }

        show(&mut rig, &panel("zz", "two"));
        show(&mut rig, &panel("zz", "one"));
        rig.keys("ctrl-shift-s");
        rig.keys("ctrl-shift-s");
        let got = wait_for_events(rig.cx, &alpha, 5);
        let told: Vec<(&str, &str)> = names(&got)
            .into_iter()
            .zip(
                got.iter()
                    .map(|event| event["panel"].as_str().unwrap_or("")),
            )
            .collect();
        assert_eq!(
            told,
            [
                ("panel_opened", "two"),
                ("panel_closed", "two"),
                ("panel_opened", "one"),
                ("panel_closed", "one"),
                ("panel_opened", "one"),
            ]
        );
        assert!(rig.pane_has_the_keyboard(1), "a panel took the keyboard");
        settle(rig.cx);
        assert_eq!(pids(&alpha).len(), 1);
        assert_eq!(pids(&zeta), [""; 0], "another plugin's panel started it");

        let disabled = PluginSettings {
            enabled: false,
            ..enabled()
        };
        let now = granted(&[("zz", disabled), ("aa", enabled())]);
        host.update(rig.cx, |host, cx| host.set_granted(now, cx));
        assert_eq!(
            rig.strip(),
            [PanelId::Files, PanelId::Hosts, panel("aa", "only")]
        );
        let saved = rig.saved();
        assert!(saved.open, "a panel leaving closed the sidebar");
        assert_eq!(saved.panel, "files");
    }

    /// Spec 0018, flow 6 [load-bearing] and AC10: after the user acted in
    /// one of a plugin's panels with the sidebar closed, its `open_panel`
    /// "opens the sidebar if needed and shows the panel, without focus",
    /// and the sidebar ignores clicks for the 0.5 s after. The click
    /// reaches the plugin as `panel_action`, and the panel coming up as
    /// `panel_opened`.
    #[gpui::test]
    fn an_open_panel_after_the_users_act_shows_the_panel_without_the_keyboard(
        cx: &mut TestAppContext,
    ) {
        let lab = Lab::new("workspace-open-panel");
        let panels = "panels: [{ id: containers, title: Containers }, { id: logs, title: Logs }]\n";
        let docker = lab.plugin("docker", "stay", panels);
        let ask = "{\"action\":\"open_panel\",\"panel\":\"containers\"}\n";
        std::fs::write(docker.join("say"), ask).expect("write say");
        let mut rig = rig(&local_and_two_hosts(), cx);
        start(&mut rig, &lab, &[("docker", grant("stay", &[], &[]))]);
        assert!(!rig.saved().open);

        // A click on a button of its panel `logs`.
        let logs = view(&mut rig, "docker", "logs");
        logs.update(rig.cx, |_, cx| {
            cx.emit(PluginPanelEvent::Action("refresh".into()));
        });
        wait_until(rig.cx, "the plugin's panel to be shown", |cx| {
            rig.workspace
                .read_with(cx, |workspace, cx| workspace.saved_sidebar(cx).open)
        });
        rig.draw();
        assert_eq!(rig.saved().panel, "plugin.docker.containers");
        assert!(
            rig.pane_has_the_keyboard(1),
            "`open_panel` took the keyboard"
        );
        assert!(!rig.sidebar_has_the_keyboard());
        assert!(sidebar_ignores_clicks(&mut rig), "a click right after it");

        let got = wait_for_events(rig.cx, &docker, 2);
        assert_eq!(names(&got), ["panel_action", "panel_opened"]);
        assert_eq!(got[0]["panel"], "logs");
        assert_eq!(got[0]["id"], "refresh");
        assert_eq!(got[1]["panel"], "containers");
    }

    /// Spec 0018, flow 6 [load-bearing]: "while the keyboard is in
    /// another panel, it is dropped with a line in the plugin's log", so
    /// a plugin can't "pull a panel away under typing"; with the keyboard
    /// back in the shell it is carried out. Spec 0016, Actions: a
    /// `set_panel` fills the panel it names and no other.
    #[gpui::test]
    fn an_open_panel_while_the_keyboard_is_in_another_panel_is_dropped(cx: &mut TestAppContext) {
        let lab = Lab::new("workspace-typing");
        let panels = "panels: [{ id: containers, title: Containers }, { id: logs, title: Logs }]\n";
        lab.plugin("docker", "stay", panels);
        let mut rig = rig(&local_and_two_hosts(), cx);
        let host = start(&mut rig, &lab, &[("docker", grant("stay", &[], &[]))]);
        let open = || Action::OpenPanel {
            panel: "containers".into(),
        };
        let log = |rig: &mut Rig<'_>| -> Vec<String> {
            host.read_with(rig.cx, |host, _| {
                host.log_of("docker").map(str::to_owned).collect()
            })
        };

        rig.run(FocusSidebar);
        assert!(rig.sidebar_has_the_keyboard(), "not in the Files panel");
        carry_out(&mut rig, "docker", open());
        assert_eq!(rig.saved().panel, "files", "pulled away under typing");
        assert!(rig.sidebar_has_the_keyboard());
        assert!(!sidebar_ignores_clicks(&mut rig), "nothing came up");
        let said = log(&mut rig);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("open_panel"), "{said:?}");

        rig.keys("escape");
        assert!(rig.pane_has_the_keyboard(1));
        carry_out(&mut rig, "docker", open());
        assert_eq!(rig.saved().panel, "plugin.docker.containers");
        assert!(
            rig.pane_has_the_keyboard(1),
            "`open_panel` took the keyboard"
        );
        assert_eq!(log(&mut rig).len(), 1, "carried out, yet logged as dropped");

        let (containers, logs) = (
            view(&mut rig, "docker", "containers"),
            view(&mut rig, "docker", "logs"),
        );
        let fill = Action::SetPanel {
            panel: "logs".into(),
            items: vec![Item::Text {
                text: "pulling".into(),
            }],
        };
        carry_out(&mut rig, "docker", fill);
        let changed = |rig: &mut Rig<'_>, view: &Entity<PluginPanel>| {
            view.read_with(rig.cx, |view, _| view.ignores_clicks())
        };
        assert!(
            changed(&mut rig, &logs),
            "the `set_panel` didn't reach its panel"
        );
        assert!(!changed(&mut rig, &containers), "it filled another panel");
        assert!(
            rig.pane_has_the_keyboard(1),
            "`set_panel` took the keyboard"
        );
    }
}
