//! The entry's context menu, by a right click or by Ctrl+Enter (spec
//! 0013, Keys).

use super::{
    Browser, DeleteSelected, DownloadSelected, DownloadSelectedTo, EditPermissions, EditSelected,
    OpenSelected, RenameSelected,
};
use crate::files::Entry;
use gpui::{Context, DismissEvent, Entity, FocusHandle, Focusable, Subscription, Window};
use gpui_component::menu::PopupMenu;

/// The context menu Ctrl+Enter opens on row `ix`, as a right click
/// would (spec 0013, Keys).
pub(super) struct KeyMenu {
    pub(super) ix: usize,
    pub(super) menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

/// Whether the browser offers Permissions… on `entry`: its target said
/// what it has, and it isn't a link, which has none of its own.
pub(super) fn has_permissions(entry: &Entry) -> bool {
    !entry.link && (entry.perms.mode.is_some() || entry.perms.readonly.is_some())
}

impl Browser {
    /// The selected entry's menu: what can be done with it on this
    /// target, each as the action its key runs, so the menu shows the key.
    pub(super) fn entry_menu(
        menu: PopupMenu,
        focus: FocusHandle,
        folder: bool,
        local: bool,
        permissions: bool,
    ) -> PopupMenu {
        let menu = menu.action_context(focus);
        let menu = if folder || local {
            menu.menu("Open", Box::new(OpenSelected))
        } else {
            menu.menu("Edit", Box::new(EditSelected))
                .menu("Download", Box::new(DownloadSelected))
                .menu("Download to…", Box::new(DownloadSelectedTo))
        };
        let menu = menu
            .menu("Rename", Box::new(RenameSelected))
            .menu("Delete", Box::new(DeleteSelected));
        if permissions {
            menu.menu("Permissions…", Box::new(EditPermissions))
        } else {
            menu
        }
    }

    /// Ctrl+Enter: the selected entry's context menu, at its row, with the
    /// keyboard in it. Its actions go to the browser, as a right click's.
    pub(super) fn show_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ix, folder, permissions)) = self.selected.and_then(|ix| {
            let (entry, _) = self.entry(ix)?;
            Some((ix, entry.is_folder(), has_permissions(entry)))
        }) else {
            return;
        };
        let (focus, local) = (self.focus.clone(), self.is_local());
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            Self::entry_menu(menu, focus, folder, local, permissions)
        });
        let dismissed =
            cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
                // An action may have moved the keyboard, like Rename into its
                // field; only a menu left without one gives it back.
                if menu.focus_handle(cx).is_focused(window) || window.focused(cx).is_none() {
                    window.focus(&this.focus, cx);
                }
                this.key_menu = None;
                cx.notify();
            });
        menu.focus_handle(cx).focus(window, cx);
        self.key_menu = Some(KeyMenu {
            ix,
            menu,
            _dismissed: dismissed,
        });
        cx.notify();
    }
}
