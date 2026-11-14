//! The host's side of the Plugins tab (spec 0016, flows 2, 6 and 8): every
//! found plugin as the tab lists it, the settings fields a plugin draws
//! with `set_settings`, and Restart.

use super::manifest::{Field, Right, Subscription};
use super::{Host, HostEvent, State};
use gpui::Context;
use std::path::PathBuf;

/// A found plugin, as the Plugins tab lists it. Every text is the
/// manifest's, cleaned by the title rules (TM-plug-01).
#[derive(Clone, Debug, PartialEq)]
pub struct ListedPlugin {
    pub id: String,
    pub folder: PathBuf,
    pub state: State,
    /// The manifest's, for a usable plugin; the folder's name otherwise.
    pub name: String,
    pub version: String,
    /// The command line on this OS: what enabling it confirms.
    pub run: Vec<String>,
    /// What the manifest declares; the user ticks among these.
    pub rights: Vec<Right>,
    pub events: Vec<Subscription>,
    /// The manifest's settings fields, then those of its last
    /// `set_settings`.
    pub fields: Vec<Field>,
    /// Enabled for a command line the manifest no longer has: it isn't
    /// started until the user confirms the new one (TM-plug-12).
    pub run_changed: bool,
}

impl Host {
    /// Every found plugin, by folder name.
    pub fn plugins(&self) -> Vec<ListedPlugin> {
        self.plugins
            .iter()
            .enumerate()
            .filter_map(|(ix, plugin)| {
                let state = self.state(&plugin.id)?;
                let granted = self.granted.get(&plugin.id);
                let listed = match &plugin.manifest {
                    Ok(manifest) => ListedPlugin {
                        id: plugin.id.clone(),
                        folder: plugin.folder.clone(),
                        state,
                        name: manifest.name.clone(),
                        version: manifest.version.clone(),
                        run: manifest.run.clone(),
                        rights: manifest.rights.clone(),
                        events: manifest.events.clone(),
                        fields: manifest
                            .settings
                            .iter()
                            .chain(&self.plugins[ix].fields)
                            .cloned()
                            .collect(),
                        run_changed: granted
                            .is_some_and(|granted| granted.enabled && granted.run != manifest.run),
                    },
                    Err(_) => ListedPlugin {
                        id: plugin.id.clone(),
                        folder: plugin.folder.clone(),
                        state,
                        name: plugin.id.clone(),
                        version: String::new(),
                        run: Vec::new(),
                        rights: Vec::new(),
                        events: Vec::new(),
                        fields: Vec::new(),
                        run_changed: false,
                    },
                };
                Some(listed)
            })
            .collect()
    }

    /// Restart on the Plugins tab: a running process is killed with what
    /// it started, a stopped plugin takes its events again, and one that
    /// hears `start` gets it (flow 6).
    pub fn restart_plugin(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.index(id).filter(|ix| self.enabled(*ix)) else {
            return;
        };
        let plugin = &mut self.plugins[ix];
        plugin.queue.clear();
        // Dropping the process kills its tree; its late messages carry an
        // old run and are ignored.
        plugin.running = None;
        plugin.stopped = false;
        self.log(ix, "restarted by the user".into(), cx);
        self.enabled_now(ix, cx);
        cx.emit(HostEvent::PanelsChanged);
        cx.notify();
    }

    /// A `set_settings`: its fields are drawn under the manifest's. One
    /// whose key the manifest already has is dropped whole (flow 8).
    pub(super) fn set_fields(&mut self, ix: usize, fields: Vec<Field>, cx: &mut Context<Self>) {
        let taken = self.plugins[ix].manifest.as_ref().is_ok_and(|manifest| {
            fields
                .iter()
                .any(|field| manifest.settings.iter().any(|known| known.key == field.key))
        });
        if taken {
            let dropped = "`set_settings` was dropped: it names a setting of the manifest's";
            self.log(ix, dropped.into(), cx);
            return;
        }
        self.plugins[ix].fields = fields;
        cx.notify();
    }
}
