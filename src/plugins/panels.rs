//! The host's side of plugin panels (spec 0016, Model; spec 0018, flows 5
//! and 6): which panels the sidebar lists, and what the user does in them
//! as events for their plugin.

use super::manifest::Right;
use super::protocol::Event;
use super::{Host, HostEvent};
use gpui::Context;

/// A panel of an enabled, usable plugin, as the sidebar lists it. Every
/// text is the manifest's, cleaned by the title rules, never an action's
/// (TM-plug-01).
#[derive(Clone, Debug, PartialEq)]
pub struct ListedPanel {
    pub plugin: String,
    /// The plugin's name, for the header's `Plugin <name>` line.
    pub name: String,
    pub panel: String,
    pub title: String,
    /// The name of one of Tern's own icons, if the manifest gave one.
    pub icon: Option<String>,
    /// The plugin crashed or couldn't start: the panel offers Restart.
    pub stopped: bool,
    /// The rights in force; a command button without its right is drawn
    /// disabled.
    pub rights: Vec<Right>,
}

impl Host {
    /// The panels of every enabled, usable plugin, each plugin's in its
    /// manifest's order.
    pub fn panels(&self) -> Vec<ListedPanel> {
        let mut listed = Vec::new();
        for (ix, plugin) in self.plugins.iter().enumerate() {
            let Ok(manifest) = &plugin.manifest else {
                continue;
            };
            if !self.enabled(ix) {
                continue;
            }
            let rights: Vec<Right> = manifest
                .rights
                .iter()
                .copied()
                .filter(|right| self.holds(ix, *right))
                .collect();
            listed.extend(manifest.panels.iter().map(|panel| ListedPanel {
                plugin: plugin.id.clone(),
                name: manifest.name.clone(),
                panel: panel.id.clone(),
                title: panel.title.clone(),
                icon: panel.icon.clone(),
                stopped: plugin.stopped,
                rights: rights.clone(),
            }));
        }
        listed
    }

    /// `panel` of `plugin` is on screen now.
    pub fn panel_opened(&mut self, plugin: &str, panel: &str, cx: &mut Context<Self>) {
        let panel = panel.to_owned();
        self.tell(plugin, Event::PanelOpened { panel }, cx);
    }

    /// `panel` of `plugin` left the screen.
    pub fn panel_closed(&mut self, plugin: &str, panel: &str, cx: &mut Context<Self>) {
        let panel = panel.to_owned();
        self.tell(plugin, Event::PanelClosed { panel }, cx);
    }

    /// The user clicked the row or button `id` in `panel`: an act of
    /// theirs on the plugin.
    pub fn panel_action(&mut self, plugin: &str, panel: &str, id: &str, cx: &mut Context<Self>) {
        self.user_acted(plugin);
        let (panel, id) = (panel.to_owned(), id.to_owned());
        self.tell(plugin, Event::PanelAction { panel, id }, cx);
    }

    /// The user submitted `text` in `panel`'s input field: an act of
    /// theirs on the plugin.
    pub fn panel_input(&mut self, plugin: &str, panel: &str, text: &str, cx: &mut Context<Self>) {
        self.user_acted(plugin);
        let (panel, text) = (panel.to_owned(), text.to_owned());
        self.tell(plugin, Event::PanelInput { panel, text }, cx);
    }

    /// Restart on a stopped plugin's `panel` (spec 0016, flow 6): its
    /// events are taken again, and this panel's `panel_opened` is sent
    /// once more, which starts it.
    pub fn restart(&mut self, plugin: &str, panel: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.index(plugin) else {
            return;
        };
        if !std::mem::take(&mut self.plugins[ix].stopped) {
            return;
        }
        self.user_acted(plugin);
        self.panel_opened(plugin, panel, cx);
        cx.emit(HostEvent::PanelsChanged);
        cx.notify();
    }

    /// Says in `plugin`'s log why something it asked for wasn't done.
    pub fn dropped(&mut self, plugin: &str, why: String, cx: &mut Context<Self>) {
        if let Some(ix) = self.index(plugin) {
            self.log(ix, why, cx);
        }
    }

    /// The user ran one of `plugin`'s commands or acted in one of its
    /// panels, which lets its next `open_panel` through (spec 0018,
    /// flow 6).
    pub(super) fn user_acted(&mut self, plugin: &str) {
        if let Some(ix) = self.index(plugin) {
            self.plugins[ix].acted = true;
            self.plugins[ix].denied = false;
        }
    }

    fn tell(&mut self, plugin: &str, event: Event, cx: &mut Context<Self>) {
        if let Some(ix) = self.index(plugin).filter(|ix| self.enabled(*ix)) {
            self.enqueue(ix, event, cx);
        }
    }
}
