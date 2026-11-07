//! Key bindings (spec 0004): each command's default keys, with
//! `keybindings:` from config on top. A command the config binds loses its
//! default keys, so they reach the shell again, and a key the config binds
//! leaves the command it was the default of.

use super::{AppCommand, by_id};
use crate::config::{Keybinding, NewTab};
use gpui::{App, Global, KeyBinding, KeyBindingContextPredicate, Keystroke, OperatingSystem};
use std::collections::BTreeSet;
use std::rc::Rc;

/// The key context app commands are bound in: the workspace's.
pub const CONTEXT: &str = "Workspace";

/// The OS whose key names keystrokes are read with.
pub const OS: OperatingSystem = if cfg!(target_os = "macos") {
    OperatingSystem::Mac
} else if cfg!(windows) {
    OperatingSystem::Windows
} else {
    OperatingSystem::Linux
};

/// Keys a GPUI keystroke can name besides single characters.
const NAMED_KEYS: &[&str] = &[
    "enter",
    "tab",
    "space",
    "backspace",
    "delete",
    "escape",
    "insert",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
];

/// The bindings that aren't ours to change: the widget library's and the
/// terminal's. Kept once, before [`bind`] first runs, because [`bind`]
/// starts again from an empty keymap.
struct Library(Vec<KeyBinding>);

impl Global for Library {}

/// Keeps the bindings made so far as the ones [`bind`] leaves alone.
pub fn keep_library(cx: &mut App) {
    let bindings = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(Library(bindings));
}

/// Binds every command as [`plan`] says, replacing what the previous call
/// bound, so a reloaded config applies at once. Returns [`plan`]'s notices.
pub fn bind(user: &[Keybinding], new_tab: NewTab, cx: &mut App) -> Vec<String> {
    let context = KeyBindingContextPredicate::parse(CONTEXT).ok().map(Rc::new);
    let (planned, mut notices) = plan_for(user, new_tab);
    let mut bindings = Vec::with_capacity(planned.len());
    for (keys, command) in planned {
        match KeyBinding::load(&keys, (command.action)(), context.clone(), false, None, cx) {
            Ok(binding) => bindings.push(binding),
            Err(error) => notices.push(format!("`{keys}` couldn't be bound: {error}")),
        }
    }
    let library = cx
        .try_global::<Library>()
        .map(|library| library.0.clone())
        .unwrap_or_default();
    cx.clear_key_bindings();
    cx.bind_keys(library);
    cx.bind_keys(bindings);
    notices
}

/// The bindings to make, in order, as GPUI keys and the command each runs:
/// the defaults, then `user`'s own, plus a notice per entry of `user` that
/// was skipped (spec 0004, flow 4). A default goes when `user` binds its
/// command (its key reaches the shell again) or takes its key, so the
/// palette never shows a key that runs something else.
#[cfg(test)]
pub fn plan(user: &[Keybinding]) -> (Vec<(String, &'static AppCommand)>, Vec<String>) {
    plan_for(user, NewTab::Ask)
}

/// [`plan`], with the two new-tab keys as `new_tab` says.
pub fn plan_for(
    user: &[Keybinding],
    new_tab: NewTab,
) -> (Vec<(String, &'static AppCommand)>, Vec<String>) {
    let mut notices = Vec::new();
    let mut own = Vec::new();
    for entry in user {
        let checked = by_id(&entry.command)
            .ok_or_else(|| format!("there is no command `{}`", entry.command))
            .and_then(|command| Ok((gpui_keys(&entry.keys)?, command)));
        match checked {
            Ok(binding) => own.push(binding),
            Err(reason) => notices.push(format!(
                "config.yaml line {}: keybinding `{}` was skipped: {reason}",
                entry.line, entry.keys
            )),
        }
    }
    let rebound: BTreeSet<&str> = own.iter().map(|(_, command)| command.id).collect();
    // Compared as keystrokes: `shift+ctrl+d` takes `ctrl-shift-d`.
    let keystroke = |keys: &str| Keystroke::parse_with_os(keys, OS).ok();
    let taken: Vec<Keystroke> = own.iter().filter_map(|(keys, _)| keystroke(keys)).collect();
    let mut planned: Vec<(String, &'static AppCommand)> = defaults_for(new_tab)
        .into_iter()
        .filter(|(keys, id)| {
            !rebound.contains(id) && keystroke(keys).is_some_and(|keys| !taken.contains(&keys))
        })
        .filter_map(|(keys, id)| Some((keys, by_id(id)?)))
        .collect();
    planned.extend(own);
    (planned, notices)
}

/// The default keys, as GPUI writes them, with the command each runs. App
/// shortcuts are `Ctrl+Shift` (`Cmd` on macOS), except go-to-tab and the
/// font size, where no `Shift` combination works on every layout (OQ-014,
/// OQ-015).
#[cfg(test)]
pub fn defaults() -> Vec<(String, &'static str)> {
    defaults_for(NewTab::Ask)
}

/// [`defaults`], with the new-tab keys as `new_tab` says: `Ctrl+Shift+T`
/// does what it names, `Ctrl+Alt+Shift+T` (`Cmd+Alt+T` on macOS) the other
/// (spec 0004; the owner's choice).
pub fn defaults_for(new_tab: NewTab) -> Vec<(String, &'static str)> {
    let (app, tab_digit, font) = if cfg!(target_os = "macos") {
        ("cmd", "cmd", "cmd")
    } else {
        ("ctrl-shift", "alt", "ctrl")
    };
    let (ask, open) = ("tab.newWithProfile", "tab.new");
    let (new_tab_key, other_key) = match new_tab {
        NewTab::Ask => (ask, open),
        NewTab::Default => (open, ask),
    };
    let other = if cfg!(target_os = "macos") {
        "cmd-alt-t"
    } else {
        "ctrl-alt-shift-t"
    };
    let mut keys = vec![
        (format!("{app}-p"), "palette.open"),
        (format!("{app}-h"), "host.connect"),
        (format!("{app}-t"), new_tab_key),
        (other.to_owned(), other_key),
        (format!("{app}-w"), "pane.close"),
        (format!("{app}-d"), "pane.splitRight"),
        (format!("{app}-e"), "pane.splitDown"),
        (format!("{app}-z"), "pane.zoom"),
        (format!("{app}-c"), "pane.copy"),
        (format!("{app}-v"), "pane.paste"),
        (format!("{app}-f"), "pane.search"),
        (format!("{app}-s"), "sftp.toggle"),
        (format!("{app}-b"), "files.open"),
        (format!("{app}-o"), "files.switchTarget"),
        ("ctrl-tab".into(), "tab.next"),
        ("ctrl-shift-tab".into(), "tab.previous"),
        ("alt-left".into(), "pane.focusLeft"),
        ("alt-right".into(), "pane.focusRight"),
        ("alt-up".into(), "pane.focusUp"),
        ("alt-down".into(), "pane.focusDown"),
        (format!("{font}-="), "font.increase"),
        // For layouts with a key of its own for `+`, like German.
        (format!("{font}-+"), "font.increase"),
        (format!("{font}--"), "font.decrease"),
        (format!("{font}-0"), "font.reset"),
    ];
    let tabs = [
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
    keys.extend(
        (1..)
            .zip(tabs)
            .map(|(n, id)| (format!("{tab_digit}-{n}"), id)),
    );
    keys
}

/// Config's `ctrl+shift+d` as GPUI writes it: `ctrl-shift-d`. The key is
/// what follows the last `+`, so `ctrl++` is the `+` key.
pub fn gpui_keys(keys: &str) -> Result<String, String> {
    let keys = keys.trim().to_lowercase();
    if keys.contains(char::is_whitespace) {
        return Err("only single keystrokes can be bound, not sequences".into());
    }
    let (modifiers, key) = match keys.strip_suffix("++") {
        Some(modifiers) => (modifiers, "+"),
        None if keys == "+" => ("", "+"),
        None => keys.rsplit_once('+').unwrap_or(("", keys.as_str())),
    };
    let modifiers: Vec<&str> = modifiers
        .split('+')
        .filter(|modifier| !modifier.is_empty())
        .map(|modifier| match modifier {
            "control" => Ok("ctrl"),
            "option" => Ok("alt"),
            "ctrl" | "alt" | "shift" | "super" | "cmd" | "win" | "fn" => Ok(modifier),
            unknown => Err(format!("`{unknown}` isn't a modifier key")),
        })
        .collect::<Result<_, _>>()?;
    let single = key.chars().count() == 1;
    let function_key = key
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n));
    if !single && !function_key && !NAMED_KEYS.contains(&key) {
        return Err(format!("`{key}` isn't a key"));
    }
    // GPUI on Linux sees Shift plus a digit or symbol only as the symbol it
    // types (OQ-014), so such a binding would never fire there.
    if cfg!(target_os = "linux")
        && single
        && modifiers.contains(&"shift")
        && !key.chars().all(char::is_alphabetic)
    {
        return Err(format!(
            "Shift with `{key}` arrives as the symbol it types; bind that symbol instead, like `ctrl+!`"
        ));
    }
    Ok(modifiers
        .into_iter()
        .chain([key])
        .collect::<Vec<_>>()
        .join("-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use gpui::{Keystroke, Modifiers, OperatingSystem};

    /// A `keybindings:` entry of config.yaml.
    fn entry(keys: &str, command: &str, line: u64) -> Keybinding {
        Keybinding {
            keys: keys.into(),
            command: command.into(),
            line,
        }
    }

    /// What GPUI reads `keys`, written its way, as.
    fn keystroke(keys: &str) -> Keystroke {
        Keystroke::parse_with_os(keys, OperatingSystem::Linux)
            .unwrap_or_else(|e| panic!("GPUI can't read `{keys}`: {e}"))
    }

    /// `keys` as GPUI writes the keystroke it reads, so two spellings of
    /// one keystroke compare equal.
    fn canonical(keys: &str) -> String {
        keystroke(keys).unparse(OperatingSystem::Linux)
    }

    /// Config's `keys` converted, as GPUI reads the result.
    fn converted(keys: &str) -> Result<String, String> {
        gpui_keys(keys).map(|keys| canonical(&keys))
    }

    /// The default keys of command `id`.
    fn default_keys(id: &str) -> Vec<String> {
        defaults()
            .into_iter()
            .filter(|(_, command)| *command == id)
            .map(|(keys, _)| keys)
            .collect()
    }

    /// The keys `planned` binds command `id` to.
    fn keys_of(planned: &[(String, &AppCommand)], id: &str) -> Vec<String> {
        planned
            .iter()
            .filter(|(_, command)| command.id == id)
            .map(|(keys, _)| canonical(keys))
            .collect()
    }

    /// The command `key` runs: GPUI runs the last binding of a key.
    fn runs(planned: &[(String, &'static AppCommand)], key: &str) -> Option<&'static AppCommand> {
        let key = canonical(key);
        planned
            .iter()
            .rev()
            .find(|(keys, _)| canonical(keys) == key)
            .map(|(_, command)| *command)
    }

    /// `planned` as keys and command IDs, so two plans can be compared.
    fn ids(planned: &[(String, &AppCommand)]) -> Vec<(String, &'static str)> {
        planned
            .iter()
            .map(|(keys, command)| (keys.clone(), command.id))
            .collect()
    }

    /// AC3 (spec 0004): the new key runs the command, and the old one runs
    /// nothing, so it reaches the shell.
    #[test]
    fn rebinding_split_right_to_alt_shift_d_frees_its_default_key() {
        let old = default_keys("pane.splitRight");
        assert!(!old.is_empty(), "pane.splitRight has no default key");
        let (planned, notices) = plan(&[entry("alt+shift+d", "pane.splitRight", 1)]);
        assert_eq!(keys_of(&planned, "pane.splitRight"), ["alt-shift-d"]);
        for key in &old {
            assert!(
                runs(&planned, key).is_none(),
                "`{key}` still runs a command"
            );
        }
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Every default key of a command the user binds goes back to the
    /// shell, not just the first.
    #[test]
    fn a_command_the_user_binds_loses_all_its_default_keys() {
        let old = default_keys("font.increase");
        assert!(old.len() > 1, "font.increase has one default key: {old:?}");
        let (planned, _) = plan(&[entry("alt+shift+i", "font.increase", 1)]);
        for key in &old {
            assert!(
                runs(&planned, key).is_none(),
                "`{key}` still runs a command"
            );
        }
    }

    /// AC2 (spec 0004): a key the user binds to another command leaves the
    /// command it was the default of, so the palette never shows it there.
    #[test]
    fn a_key_the_user_takes_leaves_the_command_it_was_the_default_of() {
        let cases = [
            // Alt+Left focuses the pane on the left by default.
            ("alt+left", "tab.previous", "pane.focusLeft", "alt-left"),
            // The same keystroke, whatever order its modifiers are written in.
            ("shift+ctrl+d", "tab.new", "pane.splitRight", "ctrl-shift-d"),
        ];
        for (keys, command, default_of, default) in cases {
            let (planned, notices) = plan(&[entry(keys, command, 1)]);
            assert!(
                !keys_of(&planned, default_of).contains(&canonical(default)),
                "`{keys}` left `{default}` on {default_of}"
            );
            let now = runs(&planned, default).map(|command| command.id);
            assert_eq!(now, Some(command), "{keys}");
            assert!(notices.is_empty(), "{notices:?}");
        }
    }

    /// AC4 (spec 0004), from config.yaml: one notice, naming the entry's
    /// line and keys, and the other bindings as if it weren't there.
    #[test]
    fn a_binding_for_tab_explode_gives_one_notice_and_the_rest_still_bind() {
        let loaded = config::parse(concat!(
            "keybindings:\n",
            "  \"alt+shift+d\": pane.splitRight\n",
            "  \"ctrl+shift+x\": tab.explode\n",
            "  \"alt+shift+n\": tab.new\n",
        ));
        let (planned, notices) = plan(&loaded.config.keybindings);
        let all: Vec<&String> = loaded.notices.iter().chain(&notices).collect();
        assert_eq!(all.len(), 1, "{all:?}");
        assert!(
            all[0].contains("line 3") && all[0].contains("ctrl+shift+x"),
            "{}",
            all[0]
        );
        let others: Vec<Keybinding> = loaded
            .config
            .keybindings
            .iter()
            .filter(|binding| binding.command != "tab.explode")
            .cloned()
            .collect();
        assert_eq!(ids(&planned), ids(&plan(&others).0));
        assert_eq!(keys_of(&planned, "tab.new"), ["alt-shift-n"]);
    }

    /// Flow 4 (spec 0004): an entry whose keys can't be bound is ignored,
    /// so its command keeps its default keys.
    #[test]
    fn an_entry_with_keys_that_cant_be_bound_is_ignored_with_a_notice() {
        let (planned, notices) = plan(&[entry("hyper+e", "pane.splitDown", 7)]);
        assert_eq!(ids(&planned), ids(&plan(&[]).0));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("line 7") && notices[0].contains("hyper+e"),
            "{}",
            notices[0]
        );
    }

    /// AC5 (spec 0004): reverse search, back a word and undo reach
    /// readline with the default config.
    #[test]
    fn the_default_config_binds_none_of_readlines_keys() {
        let (planned, _) = plan(&[]);
        for key in ["ctrl-r", "alt-b", "ctrl-/", "ctrl-_"] {
            assert!(runs(&planned, key).is_none(), "`{key}` runs a command");
        }
    }

    /// Spec 0004 [load-bearing]: Ctrl or Alt plus a letter, digit or
    /// punctuation key without Shift belongs to the shell. The exceptions
    /// are go-to-tab on Alt+1–9 (OQ-014) and the font size on Ctrl+=,
    /// Ctrl++, Ctrl+- and Ctrl+0 (OQ-015).
    #[test]
    fn the_defaults_take_no_shell_key_but_go_to_tab_and_the_font_size() {
        let exceptions: Vec<String> = (1..=9)
            .map(|n| format!("alt-{n}"))
            .chain(["ctrl-=", "ctrl-+", "ctrl--", "ctrl-0"].map(String::from))
            .map(|keys| canonical(&keys))
            .collect();
        for (keys, id) in defaults() {
            let Keystroke { modifiers, key, .. } = keystroke(&keys);
            let Modifiers {
                control,
                alt,
                shift,
                platform,
                ..
            } = modifiers;
            let shells = (control || alt) && !shift && !platform && key.chars().count() == 1;
            assert!(
                !shells || exceptions.contains(&canonical(&keys)),
                "`{keys}` ({id}) belongs to the shell"
            );
        }
    }

    /// The default keys of spec 0004 §Model on Linux and Windows.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_defaults_are_the_spec_table() {
        let table = [
            ("ctrl-shift-p", "palette.open"),
            ("ctrl-shift-h", "host.connect"),
            ("ctrl-shift-t", "tab.newWithProfile"),
            ("ctrl-alt-shift-t", "tab.new"),
            ("ctrl-shift-w", "pane.close"),
            ("ctrl-tab", "tab.next"),
            ("ctrl-shift-tab", "tab.previous"),
            ("alt-1", "tab.goTo1"),
            ("alt-2", "tab.goTo2"),
            ("alt-3", "tab.goTo3"),
            ("alt-4", "tab.goTo4"),
            ("alt-5", "tab.goTo5"),
            ("alt-6", "tab.goTo6"),
            ("alt-7", "tab.goTo7"),
            ("alt-8", "tab.goTo8"),
            ("alt-9", "tab.goTo9"),
            ("ctrl-shift-d", "pane.splitRight"),
            ("ctrl-shift-e", "pane.splitDown"),
            ("alt-left", "pane.focusLeft"),
            ("alt-up", "pane.focusUp"),
            ("alt-right", "pane.focusRight"),
            ("alt-down", "pane.focusDown"),
            ("ctrl-shift-z", "pane.zoom"),
            ("ctrl-shift-c", "pane.copy"),
            ("ctrl-shift-v", "pane.paste"),
            ("ctrl-shift-f", "pane.search"),
            ("ctrl-shift-s", "sftp.toggle"),
            ("ctrl-shift-b", "files.open"),
            ("ctrl-shift-o", "files.switchTarget"),
            ("ctrl-=", "font.increase"),
            ("ctrl-+", "font.increase"),
            ("ctrl--", "font.decrease"),
            ("ctrl-0", "font.reset"),
        ];
        let expected: BTreeSet<(String, &str)> = table
            .into_iter()
            .map(|(keys, id)| (canonical(keys), id))
            .collect();
        let actual: BTreeSet<(String, &str)> = defaults()
            .into_iter()
            .map(|(keys, id)| (canonical(&keys), id))
            .collect();
        assert_eq!(actual, expected);
    }

    /// The command `key` runs in `defaults`, if any.
    fn default_of(defaults: &[(String, &'static str)], key: &str) -> Option<&'static str> {
        let key = canonical(key);
        defaults
            .iter()
            .find(|(keys, _)| canonical(keys) == key)
            .map(|(_, id)| *id)
    }

    /// Spec 0004 §Model, spec 0005 (`new_tab: ask`, the default):
    /// `Ctrl+Shift+T` asks which profile, `Ctrl+Alt+Shift+T` opens the
    /// default one, in the defaults and in the plan for an empty config.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn with_new_tab_ask_ctrl_shift_t_asks_and_ctrl_alt_shift_t_opens_the_default() {
        let defaults = defaults_for(NewTab::Ask);
        assert_eq!(
            default_of(&defaults, "ctrl-shift-t"),
            Some("tab.newWithProfile")
        );
        assert_eq!(default_of(&defaults, "ctrl-alt-shift-t"), Some("tab.new"));
        let (planned, notices) = plan_for(&[], NewTab::Ask);
        let ran = |key| runs(&planned, key).map(|command| command.id);
        assert_eq!(ran("ctrl-shift-t"), Some("tab.newWithProfile"));
        assert_eq!(ran("ctrl-alt-shift-t"), Some("tab.new"));
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Spec 0004 §Model, spec 0005: `new_tab: default` swaps the two keys,
    /// so `Ctrl+Shift+T` opens the default profile and `Ctrl+Alt+Shift+T`
    /// asks, in the defaults and in the plan for an empty config.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn with_new_tab_default_the_two_new_tab_keys_are_swapped() {
        let defaults = defaults_for(NewTab::Default);
        assert_eq!(default_of(&defaults, "ctrl-shift-t"), Some("tab.new"));
        assert_eq!(
            default_of(&defaults, "ctrl-alt-shift-t"),
            Some("tab.newWithProfile")
        );
        let (planned, notices) = plan_for(&[], NewTab::Default);
        let ran = |key| runs(&planned, key).map(|command| command.id);
        assert_eq!(ran("ctrl-shift-t"), Some("tab.new"));
        assert_eq!(ran("ctrl-alt-shift-t"), Some("tab.newWithProfile"));
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Spec 0004: a command the config binds loses its default keys, so a
    /// user key for `tab.new` frees whichever new-tab key `new_tab` gave
    /// it, and the other key still asks which profile.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_user_key_for_tab_new_frees_whichever_new_tab_key_it_had() {
        for (new_tab, freed, asks) in [
            (NewTab::Ask, "ctrl-alt-shift-t", "ctrl-shift-t"),
            (NewTab::Default, "ctrl-shift-t", "ctrl-alt-shift-t"),
        ] {
            let (planned, notices) = plan_for(&[entry("alt+shift+n", "tab.new", 1)], new_tab);
            assert_eq!(keys_of(&planned, "tab.new"), ["alt-shift-n"], "{new_tab:?}");
            assert!(
                runs(&planned, freed).is_none(),
                "{new_tab:?}: `{freed}` still runs a command"
            );
            let ran = runs(&planned, asks).map(|command| command.id);
            assert_eq!(ran, Some("tab.newWithProfile"), "{new_tab:?}");
            assert!(notices.is_empty(), "{notices:?}");
        }
    }

    /// AC6 (spec 0004): Alt plus a digit matches the digit as typed, on any
    /// layout (OQ-014), and goes to that tab. macOS uses Cmd instead.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn alt_and_a_digit_goes_to_that_tab() {
        use crate::commands::GoToTab;

        let (planned, _) = plan(&[]);
        for n in 1..=9 {
            let command = runs(&planned, &format!("alt-{n}"))
                .unwrap_or_else(|| panic!("alt-{n} runs nothing"));
            assert_eq!(command.id, format!("tab.goTo{n}"));
            let action = (command.action)();
            let tab = action.as_any().downcast_ref::<GoToTab>();
            assert_eq!(tab, Some(&GoToTab(n - 1)), "alt-{n}");
        }
    }

    /// AC7 (spec 0004): a default for a command that doesn't exist would
    /// be dropped without a word.
    #[test]
    fn every_default_runs_an_existing_command() {
        for (keys, id) in defaults() {
            assert!(
                by_id(id).is_some(),
                "`{keys}` runs `{id}`, which isn't a command"
            );
        }
    }

    /// The example in the template "Open config file" writes binds as shown
    /// once uncommented.
    #[test]
    fn the_templates_example_keybindings_bind_without_a_notice() {
        let example: String = config::TEMPLATE
            .lines()
            .skip_while(|line| *line != "# keybindings:")
            .take_while(|line| line.starts_with('#'))
            .map(|line| format!("{}\n", line.strip_prefix("# ").unwrap_or(line)))
            .collect();
        let loaded = config::parse(&example);
        assert!(
            !loaded.config.keybindings.is_empty(),
            "no example in:\n{example}"
        );
        let (_, notices) = plan(&loaded.config.keybindings);
        let all: Vec<&String> = loaded.notices.iter().chain(&notices).collect();
        assert!(all.is_empty(), "{all:?}");
    }

    #[test]
    fn config_keys_convert_to_gpui_keystrokes() {
        let cases = [
            ("ctrl+shift+d", "ctrl-shift-d"),
            ("alt+left", "alt-left"),
            ("ctrl+tab", "ctrl-tab"),
            ("shift+pageup", "shift-pageup"),
            ("ctrl+space", "ctrl-space"),
            ("ctrl+f12", "ctrl-f12"),
            ("f5", "f5"),
            ("ctrl+=", "ctrl-="),
            ("ctrl+-", "ctrl--"),
            ("ctrl+/", "ctrl-/"),
        ];
        for (keys, gpui) in cases {
            assert_eq!(converted(keys), Ok(canonical(gpui)), "{keys}");
        }
    }

    #[test]
    fn config_keys_ignore_case() {
        let cases = [
            ("Ctrl+Shift+D", "ctrl-shift-d"),
            ("ctrl+D", "ctrl-d"),
            ("ALT+PageUp", "alt-pageup"),
            ("F5", "f5"),
        ];
        for (keys, gpui) in cases {
            assert_eq!(converted(keys), Ok(canonical(gpui)), "{keys}");
        }
    }

    /// The key is what follows the last `+`, so `ctrl++` is Ctrl and the
    /// `+` key.
    #[test]
    fn a_plus_after_the_last_plus_is_the_plus_key() {
        let cases = [
            ("ctrl++", "ctrl-+"),
            ("ctrl+alt++", "ctrl-alt-+"),
            ("+", "+"),
        ];
        for (keys, gpui) in cases {
            assert_eq!(converted(keys), Ok(canonical(gpui)), "{keys}");
        }
    }

    /// `control` and `option` are other names for `ctrl` and `alt`.
    #[test]
    fn every_modifier_converts_and_so_do_the_aliases() {
        let cases = [
            ("ctrl+d", "ctrl-d"),
            ("control+d", "ctrl-d"),
            ("alt+d", "alt-d"),
            ("option+d", "alt-d"),
            ("shift+f5", "shift-f5"),
            ("super+d", "super-d"),
            ("cmd+d", "cmd-d"),
            ("win+d", "win-d"),
            ("fn+f1", "fn-f1"),
        ];
        for (keys, gpui) in cases {
            assert_eq!(converted(keys), Ok(canonical(gpui)), "{keys}");
        }
    }

    /// Each is skipped with a notice rather than bound as something else
    /// (spec 0004, flow 4).
    #[test]
    fn keys_that_arent_one_keystroke_are_rejected() {
        let cases = [
            ("hyper+d", "an unknown modifier"),
            ("ctrl+foo", "an unknown key"),
            ("ctrl-shift-d", "GPUI's spelling instead of config's"),
            ("ctrl+", "no key after the modifiers"),
            ("", "nothing"),
            ("ctrl+k ctrl+c", "a sequence"),
        ];
        for (keys, what) in cases {
            let result = gpui_keys(keys);
            assert!(result.is_err(), "{what}: `{keys}` gave {result:?}");
        }
    }

    /// GPUI on Linux delivers Shift plus a digit or symbol as the symbol
    /// the layout types (OQ-014), so such a binding could never run.
    #[cfg(target_os = "linux")]
    #[test]
    fn shift_with_a_digit_or_symbol_is_rejected_on_linux() {
        for keys in [
            "ctrl+shift+2",
            "alt+shift+1",
            "ctrl+shift+=",
            "ctrl+shift+-",
            "ctrl+shift++",
        ] {
            let result = gpui_keys(keys);
            assert!(result.is_err(), "`{keys}` gave {result:?}");
        }
        // With a letter, a named key or a function key it arrives as written.
        let cases = [
            ("alt+shift+d", "alt-shift-d"),
            ("ctrl+shift+tab", "ctrl-shift-tab"),
            ("shift+f5", "shift-f5"),
        ];
        for (keys, gpui) in cases {
            assert_eq!(converted(keys), Ok(canonical(gpui)), "{keys}");
        }
    }
}
