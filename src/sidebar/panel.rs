//! What the sidebar hosts (spec 0018, Model): a panel, registered by the
//! built-in panels and, with T-069, by plugins in the same way.

use gpui::{AnyView, App, SharedString, Window};
use gpui_component::IconName;
use std::rc::Rc;

/// Which panel: what the session saves and what `open_panel` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelId {
    Files,
    Hosts,
    /// The active SSH pane's port forwards; listed only while an SSH pane
    /// is active.
    Forwards,
    // Registered by T-069.
    #[allow(dead_code, reason = "plugin panels are registered by T-069")]
    Plugin {
        plugin: String,
        panel: String,
    },
}

impl PanelId {
    /// As `session.json` writes it.
    pub fn saved(&self) -> String {
        match self {
            PanelId::Files => "files".into(),
            PanelId::Hosts => "hosts".into(),
            PanelId::Forwards => "forwards".into(),
            PanelId::Plugin { plugin, panel } => format!("plugin.{plugin}.{panel}"),
        }
    }

    /// The panel a relaunch opens on for a saved ID. A plugin's comes back
    /// as Files, like anything unknown: showing it would start the plugin
    /// (spec 0018, flow 7). Port forwards stays only on an SSH pane; the
    /// workspace sees to that.
    pub fn restored(saved: &str) -> Self {
        match saved {
            "hosts" => PanelId::Hosts,
            "forwards" => PanelId::Forwards,
            _ => PanelId::Files,
        }
    }
}

/// Puts the keyboard into a panel.
pub type Focus = Rc<dyn Fn(&mut Window, &mut App)>;

/// One panel of the sidebar. The sidebar draws the strip button and the
/// header from this, and `view` as the body.
pub struct Panel {
    pub id: PanelId,
    pub title: SharedString,
    pub icon: IconName,
    /// The owning plugin's name, as the host knows it, never as an action
    /// says it: the header's `Plugin <name>` line (TM-plug-01). `None` for
    /// a built-in panel.
    pub plugin: Option<SharedString>,
    pub view: AnyView,
    /// Puts the keyboard into the panel, for `sidebar.focus`.
    pub focus: Focus,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 0018, flow 7: a saved plugin panel comes back as Files, so no
    /// plugin starts at launch; the built-in ones come back as themselves.
    #[test]
    fn a_saved_plugin_panel_is_restored_as_files() {
        let plugin = PanelId::Plugin {
            plugin: "docker".into(),
            panel: "containers".into(),
        };
        assert_eq!(plugin.saved(), "plugin.docker.containers");
        assert_eq!(PanelId::restored(&plugin.saved()), PanelId::Files);
        for id in [PanelId::Files, PanelId::Hosts] {
            assert_eq!(PanelId::restored(&id.saved()), id);
        }
        assert_eq!(PanelId::restored("nonsense"), PanelId::Files);
    }

    /// Spec 0018, Port forwards panel and Saved (T-084): the panel's ID is
    /// saved as `forwards` and read back as Port forwards; whether it
    /// stays is the active pane's business (flow 7). The other built-in
    /// panels keep their saved names.
    #[test]
    fn port_forwards_is_saved_as_forwards_and_restored_as_itself() {
        assert_eq!(PanelId::Forwards.saved(), "forwards");
        assert_eq!(PanelId::restored("forwards"), PanelId::Forwards);
        assert_eq!(PanelId::Files.saved(), "files");
        assert_eq!(PanelId::Hosts.saved(), "hosts");
        // Only the exact ID is Port forwards.
        for other in ["Forwards", "forward", "plugin.forwards.forwards", ""] {
            assert_eq!(PanelId::restored(other), PanelId::Files, "{other:?}");
        }
    }
}
