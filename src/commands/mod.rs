//! Commands (spec 0004): every app action, with the stable ID config binds
//! keys to and the title the palette shows. The actions themselves are
//! GPUI actions, so a typo in a default binding is a compile error.

pub mod fuzzy;
pub mod keymap;
pub mod palette;

use gpui::{Action, actions};

actions!(
    tern,
    [
        OpenPalette,
        NewTab,
        ClosePane,
        CloseTab,
        RenameTab,
        NextTab,
        PreviousTab,
        SplitRight,
        SplitDown,
        /// Picks a profile or host to split the focused pane with, on the
        /// right (spec 0007, flow 6).
        SplitRightWithProfile,
        /// Picks a profile or host to split the focused pane with, below.
        SplitDownWithProfile,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        ZoomPane,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        SelectTheme,
        /// Opens `config.yaml` in the OS's editor, creating it first if it's
        /// missing (spec 0005).
        OpenConfig,
        /// Shows `config.yaml` in the OS's file manager (spec 0009).
        OpenConfigFolder,
        /// Opens the host picker (spec 0006).
        ConnectToHost,
        /// Opens the host picker to forget what's saved for a host (spec
        /// 0006, flow 7).
        ForgetPassword,
        /// Copies the focused pane's selection (spec 0002, flow 4).
        CopySelection,
        /// Pastes the clipboard into the focused pane's program.
        Paste,
        /// Opens the focused pane's search bar (spec 0002, flow 6).
        Search,
        /// Opens the settings screen (spec 0009).
        OpenSettings,
        /// Picks a profile or host to open in a new tab (spec 0004).
        NewTabWithProfile,
        /// Asks GitHub for a newer release now, whatever `update_check`
        /// says (spec 0011, flow 6).
        CheckForUpdates,
        /// Opens or closes the focused SSH pane's SFTP sidebar (spec 0008,
        /// flow 1).
        ToggleSftp,
    ]
);

/// Activates tab N, counted from 0.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tern, no_json)]
pub struct GoToTab(pub usize);

/// An app command.
pub struct AppCommand {
    /// What `keybindings:` in config names it by, like `pane.splitRight`.
    pub id: &'static str,
    /// What the palette shows. `None` keeps it out of the palette.
    pub title: Option<&'static str>,
    pub action: fn() -> Box<dyn Action>,
}

/// Every app command, in the order the palette lists them.
pub const COMMANDS: &[AppCommand] = &[
    command("palette.open", None, || Box::new(OpenPalette)),
    command("tab.new", Some("New tab"), || Box::new(NewTab)),
    command(
        "tab.newWithProfile",
        Some("New tab with profile…"),
        || Box::new(NewTabWithProfile),
    ),
    command("pane.splitRight", Some("Split right"), || {
        Box::new(SplitRight)
    }),
    command("pane.splitDown", Some("Split down"), || Box::new(SplitDown)),
    command(
        "pane.splitRightWithProfile",
        Some("Split right with profile…"),
        || Box::new(SplitRightWithProfile),
    ),
    command(
        "pane.splitDownWithProfile",
        Some("Split down with profile…"),
        || Box::new(SplitDownWithProfile),
    ),
    command("pane.copy", Some("Copy"), || Box::new(CopySelection)),
    command("pane.paste", Some("Paste"), || Box::new(Paste)),
    command("pane.search", Some("Search"), || Box::new(Search)),
    command("pane.close", Some("Close pane"), || Box::new(ClosePane)),
    command("tab.close", Some("Close tab"), || Box::new(CloseTab)),
    command("tab.rename", Some("Rename tab"), || Box::new(RenameTab)),
    command("tab.next", Some("Next tab"), || Box::new(NextTab)),
    command("tab.previous", Some("Previous tab"), || {
        Box::new(PreviousTab)
    }),
    command("pane.zoom", Some("Zoom pane"), || Box::new(ZoomPane)),
    command("pane.focusLeft", Some("Focus pane on the left"), || {
        Box::new(FocusLeft)
    }),
    command("pane.focusRight", Some("Focus pane on the right"), || {
        Box::new(FocusRight)
    }),
    command("pane.focusUp", Some("Focus pane above"), || {
        Box::new(FocusUp)
    }),
    command("pane.focusDown", Some("Focus pane below"), || {
        Box::new(FocusDown)
    }),
    command("font.increase", Some("Increase font size"), || {
        Box::new(IncreaseFontSize)
    }),
    command("font.decrease", Some("Decrease font size"), || {
        Box::new(DecreaseFontSize)
    }),
    command("font.reset", Some("Reset font size"), || {
        Box::new(ResetFontSize)
    }),
    command("host.connect", Some("Connect to host…"), || {
        Box::new(ConnectToHost)
    }),
    command("host.forget", Some("Forget saved password…"), || {
        Box::new(ForgetPassword)
    }),
    command("sftp.toggle", Some("Toggle SFTP sidebar"), || {
        Box::new(ToggleSftp)
    }),
    command("theme.select", Some("Select theme"), || {
        Box::new(SelectTheme)
    }),
    command("settings.open", Some("Open settings"), || {
        Box::new(OpenSettings)
    }),
    command("config.open", Some("Open config file"), || {
        Box::new(OpenConfig)
    }),
    command("config.openFolder", Some("Open config folder"), || {
        Box::new(OpenConfigFolder)
    }),
    command("app.checkForUpdates", Some("Check for updates"), || {
        Box::new(CheckForUpdates)
    }),
    command("tab.goTo1", Some("Go to tab 1"), || Box::new(GoToTab(0))),
    command("tab.goTo2", Some("Go to tab 2"), || Box::new(GoToTab(1))),
    command("tab.goTo3", Some("Go to tab 3"), || Box::new(GoToTab(2))),
    command("tab.goTo4", Some("Go to tab 4"), || Box::new(GoToTab(3))),
    command("tab.goTo5", Some("Go to tab 5"), || Box::new(GoToTab(4))),
    command("tab.goTo6", Some("Go to tab 6"), || Box::new(GoToTab(5))),
    command("tab.goTo7", Some("Go to tab 7"), || Box::new(GoToTab(6))),
    command("tab.goTo8", Some("Go to tab 8"), || Box::new(GoToTab(7))),
    command("tab.goTo9", Some("Go to tab 9"), || Box::new(GoToTab(8))),
];

const fn command(
    id: &'static str,
    title: Option<&'static str>,
    action: fn() -> Box<dyn Action>,
) -> AppCommand {
    AppCommand { id, title, action }
}

/// The command config calls `id`.
pub fn by_id(id: &str) -> Option<&'static AppCommand> {
    COMMANDS.iter().find(|command| command.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Config binds commands by ID, so two commands can't share one.
    #[test]
    fn every_command_has_its_own_id() {
        let ids: BTreeSet<&str> = COMMANDS.iter().map(|command| command.id).collect();
        assert_eq!(ids.len(), COMMANDS.len());
    }

    /// AC7 (spec 0004): a command without a title isn't in the palette,
    /// and only opening the palette itself may stay out of it.
    #[test]
    fn every_command_but_opening_the_palette_has_a_palette_title() {
        let untitled: Vec<&str> = COMMANDS
            .iter()
            .filter(|command| command.title.is_none() && command.id != "palette.open")
            .map(|command| command.id)
            .collect();
        assert!(untitled.is_empty(), "not in the palette: {untitled:?}");
    }

    /// AC7 (spec 0004): the Phase 1 actions of spec 0002 (copy, paste,
    /// search), spec 0003 (tabs and splits), spec 0004 (font size) and
    /// spec 0005 (theme picker, config file) are in the palette and
    /// bindable by ID.
    #[test]
    fn every_phase_1_action_is_in_the_palette_and_bindable_by_its_id() {
        let tabs = [
            "tab.new",
            "tab.close",
            "tab.rename",
            "tab.next",
            "tab.previous",
            "tab.goTo1",
            "tab.goTo2",
            "tab.goTo3",
            "tab.goTo4",
            "tab.goTo5",
            "tab.goTo6",
            "tab.goTo7",
            "tab.goTo8",
            "tab.goTo9",
        ];
        let panes = [
            "pane.splitRight",
            "pane.splitDown",
            "pane.close",
            "pane.zoom",
            "pane.focusLeft",
            "pane.focusRight",
            "pane.focusUp",
            "pane.focusDown",
            "pane.copy",
            "pane.paste",
            "pane.search",
        ];
        let settings = [
            "font.increase",
            "font.decrease",
            "font.reset",
            "theme.select",
            "config.open",
        ];
        for id in tabs.into_iter().chain(panes).chain(settings) {
            let command = by_id(id).unwrap_or_else(|| panic!("no command has the ID `{id}`"));
            assert!(command.title.is_some(), "`{id}` isn't in the palette");
        }
    }

    /// Spec 0006, flow 1: "Connect to host…" is in the palette, bindable
    /// as `host.connect`, and opens the host picker.
    #[test]
    fn connecting_to_a_host_is_in_the_palette_and_bindable_as_host_connect() {
        let command = by_id("host.connect").expect("no command has the ID `host.connect`");
        assert!(
            command
                .title
                .is_some_and(|title| title.starts_with("Connect to host")),
            "titled {:?}",
            command.title
        );
        let action = (command.action)();
        assert!(
            action.as_any().downcast_ref::<ConnectToHost>().is_some(),
            "runs {}",
            action.name()
        );
    }

    /// Spec 0006, flow 7: "Forget saved password…" is in the palette,
    /// bindable as `host.forget`, and opens the host picker to forget with.
    #[test]
    fn forgetting_a_saved_password_is_in_the_palette_and_bindable_as_host_forget() {
        let command = by_id("host.forget").expect("no command has the ID `host.forget`");
        assert_eq!(command.title, Some("Forget saved password…"));
        let action = (command.action)();
        assert!(
            action.as_any().downcast_ref::<ForgetPassword>().is_some(),
            "runs {}",
            action.name()
        );
    }

    /// Spec 0007, flow 6: "Split right with profile…" and "Split down with
    /// profile…" are in the palette and bindable as
    /// `pane.splitRightWithProfile` and `pane.splitDownWithProfile`.
    #[test]
    fn splitting_with_a_profile_is_in_the_palette_and_bindable_by_its_id() {
        let cases: [(&str, &str, &dyn Action); 2] = [
            (
                "pane.splitRightWithProfile",
                "Split right with profile…",
                &SplitRightWithProfile,
            ),
            (
                "pane.splitDownWithProfile",
                "Split down with profile…",
                &SplitDownWithProfile,
            ),
        ];
        for (id, title, runs) in cases {
            let command = by_id(id).unwrap_or_else(|| panic!("no command has the ID `{id}`"));
            assert_eq!(command.title, Some(title));
            let action = (command.action)();
            assert!(action.partial_eq(runs), "`{id}` runs {}", action.name());
        }
    }

    /// Spec 0011, flow 6 and AC8: "Check for updates" is in the palette,
    /// bindable as `app.checkForUpdates`, and runs the check.
    #[test]
    fn checking_for_updates_is_in_the_palette_and_bindable_as_app_check_for_updates() {
        let command =
            by_id("app.checkForUpdates").expect("no command has the ID `app.checkForUpdates`");
        assert_eq!(command.title, Some("Check for updates"));
        let action = (command.action)();
        assert!(
            action.partial_eq(&CheckForUpdates),
            "runs {}",
            action.name()
        );
    }

    /// Spec 0008, flow 1: "Toggle SFTP sidebar" is in the palette,
    /// bindable as `sftp.toggle`, and toggles the sidebar.
    #[test]
    fn toggling_the_sftp_sidebar_is_in_the_palette_and_bindable_as_sftp_toggle() {
        let command = by_id("sftp.toggle").expect("no command has the ID `sftp.toggle`");
        assert_eq!(command.title, Some("Toggle SFTP sidebar"));
        let action = (command.action)();
        assert!(action.partial_eq(&ToggleSftp), "runs {}", action.name());
    }
}
