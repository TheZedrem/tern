//! The workspace's side of plugins (spec 0016): it starts the host after
//! the first frame, hands it the reloaded config and the panes that open
//! and close, and shows its notices. The host owns the plugins; the
//! workspace only hosts it.

use super::tree::LeafId;
use super::{Workspace, show_notice};
use crate::plugins::{self, Host, HostEvent, PaneInfo};
use gpui::{AppContext, Context, Window};

impl Workspace {
    /// Looks for plugins (flow 1). Called after the first frame; the host
    /// reads the folders off the UI thread.
    pub(super) fn start_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let granted = self.config.plugins.clone();
        let host = cx.new(|cx| Host::new(plugins::root(), granted, cx));
        cx.subscribe_in(&host, window, |_, host, event, window, cx| match event {
            HostEvent::Notice(notice) => show_notice(notice.clone(), window, cx),
            // TODO(T-068, T-069): carry the actions out.
            HostEvent::Action { plugin, action } => {
                host.update(cx, |host, cx| host.unhandled(plugin, action, cx));
            }
        })
        .detach();
        host.update(cx, |host, cx| host.discover(cx));
        self.plugins = Some(host);
    }

    /// Hands a reloaded config's `plugins:` to the host (flow 2).
    pub(super) fn plugins_reloaded(&mut self, cx: &mut Context<Self>) {
        if let Some(host) = &self.plugins {
            let granted = self.config.plugins.clone();
            host.update(cx, |host, cx| host.set_granted(granted, cx));
        }
    }

    /// Tells the plugins that hear it that terminal pane `leaf` opened.
    // TODO(OQ-037): panes that were open before a plugin was found or
    // enabled are not told to it.
    pub(super) fn plugins_pane_opened(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        let (Some(host), Some(pane)) = (&self.plugins, self.panes.get(&leaf)) else {
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
        host.update(cx, |host, cx| host.pane_opened(leaf, &info, cx));
    }

    pub(super) fn plugins_pane_closed(&mut self, leaf: LeafId, cx: &mut Context<Self>) {
        if let Some(host) = &self.plugins {
            host.update(cx, |host, cx| host.pane_closed(leaf, cx));
        }
    }
}
