//! What the settings screen changes in `config.yaml` (spec 0009): in-place
//! edits that are written only if the file then reads back as asked, with
//! no new config notice.

use super::yaml;
use crate::commands;
use crate::config::{self, Config, PluginSettings, Profile, ProfileKind, Setting};
use std::path::Path;

#[derive(Clone, Debug)]
pub enum Change {
    /// The scalar at `path`, like `font.size`, set to `value`, already
    /// YAML. `reads` is how the loaded config says that setting, in the
    /// same YAML, so the edit can be checked.
    Set {
        path: Vec<&'static str>,
        value: String,
        reads: fn(&Config) -> String,
    },
    /// The top-level scalar `to` set like [`Change::Set`], and the line of
    /// `from`, the name it had before, removed: a setting that was renamed
    /// is written under its new name only.
    Renamed {
        from: &'static str,
        to: &'static str,
        value: String,
        reads: fn(&Config) -> String,
    },
    /// `keys`, in config's spelling like `ctrl+shift+k`, run `command`, and
    /// no other key the user set does; `None` gives it its default keys
    /// back (spec 0004).
    Bind {
        command: &'static str,
        keys: Option<String>,
    },
    /// Profile `id` written as `profile`, added or rewritten; `None`
    /// deletes it.
    Profile {
        id: String,
        profile: Option<Box<ProfileForm>>,
    },
    /// Plugin `id`'s entry under `plugins:` written whole as `entry`
    /// (spec 0016, flows 2 and 8).
    Plugin { id: String, entry: PluginSettings },
}

impl Change {
    /// What a notice calls it.
    pub fn describe(&self) -> String {
        match self {
            Change::Set { path, .. } => path.join("."),
            Change::Renamed { to, .. } => (*to).to_owned(),
            Change::Bind { command, .. } => format!("keybindings (for {command})"),
            Change::Profile { id, .. } => format!("profiles.{id}"),
            Change::Plugin { id, .. } => format!("plugins.{id}"),
        }
    }

    /// `text` with the change, or `None` if it can't be made in place.
    pub fn apply(&self, text: &str) -> Option<String> {
        match self {
            Change::Set { path, value, .. } => yaml::set_scalar(text, path, value),
            Change::Renamed {
                from, to, value, ..
            } => yaml::set_scalar(&yaml::remove(text, &[from])?, &[to], value),
            Change::Bind { command, keys } => {
                let text = yaml::remove_where(text, "keybindings", |_, bound| {
                    // An entry under the command's old name is its key too.
                    commands::current_id(bound) == *command
                })?;
                match keys {
                    Some(keys) => yaml::upsert(
                        &text,
                        &["keybindings", keys],
                        &[format!("{}: {command}", yaml::key_text(keys))],
                    ),
                    None => Some(text),
                }
            }
            Change::Profile { id, profile } => match profile {
                Some(form) => yaml::upsert(text, &["profiles", id], &form.lines(id)),
                None => yaml::remove(text, &["profiles", id]),
            },
            Change::Plugin { id, entry } => {
                yaml::upsert(text, &["plugins", id], &plugin_lines(id, entry))
            }
        }
    }

    /// Whether `after` is `before` with this change, and nothing broke:
    /// no notice the file didn't have.
    pub fn holds(&self, before: &config::Loaded, after: &config::Loaded) -> bool {
        if after.notices.len() > before.notices.len() {
            return false;
        }
        let config = &after.config;
        match self {
            Change::Set { value, reads, .. } | Change::Renamed { value, reads, .. } => {
                reads(config) == *value
            }
            Change::Bind { command, keys } => {
                let bound: Vec<&str> = config
                    .keybindings
                    .iter()
                    .filter(|binding| commands::current_id(&binding.command) == *command)
                    .map(|binding| binding.keys.as_str())
                    .collect();
                match keys {
                    Some(keys) => bound == [keys.as_str()],
                    None => bound.is_empty(),
                }
            }
            Change::Profile { id, profile } => {
                config.profiles.iter().any(|found| found.id == *id) == profile.is_some()
            }
            Change::Plugin { id, entry } => config.plugins.get(id) == Some(entry),
        }
    }

    /// `text` with the change, checked to read back as asked.
    pub fn edited(&self, text: &str) -> Result<String, String> {
        let refused = || {
            format!(
                "config.yaml writes {} in a way the settings can't change safely; \
                 change it there yourself",
                self.describe()
            )
        };
        let edited = self.apply(text).ok_or_else(refused)?;
        if self.holds(&config::parse(text), &config::parse(&edited)) {
            Ok(edited)
        } else {
            Err(refused())
        }
    }
}

/// Plugin `id`'s entry as YAML lines for [`yaml::upsert`], flow style
/// inside, so it stays a short block.
fn plugin_lines(id: &str, entry: &PluginSettings) -> Vec<String> {
    let list = |items: Vec<String>| format!("[{}]", items.join(", "));
    let run = entry.run.iter().map(|word| yaml::scalar(word)).collect();
    let rights = entry
        .rights
        .iter()
        .map(|right| right.name().to_owned())
        .collect();
    let events = entry
        .events
        .iter()
        .map(|event| event.name().to_owned())
        .collect();
    let settings: Vec<String> = entry
        .settings
        .iter()
        .map(|(key, value)| {
            let value = match value {
                Setting::Bool(on) => on.to_string(),
                Setting::Number(number) => super::number(*number),
                Setting::Text(text) => yaml::scalar(text),
            };
            format!("{}: {value}", yaml::key_text(key))
        })
        .collect();
    vec![
        format!("{}:", yaml::key_text(id)),
        format!("  enabled: {}", entry.enabled),
        format!("  run: {}", list(run)),
        format!("  rights: {}", list(rights)),
        format!("  events: {}", list(events)),
        match settings.is_empty() {
            true => "  settings: {}".to_owned(),
            false => format!("  settings: {{ {} }}", settings.join(", ")),
        },
    ]
}

/// A profile as the form edits it: every field as typed (spec 0005, 0006).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileForm {
    pub name: String,
    pub ssh: bool,
    /// Program and arguments, separated by spaces; empty: the login shell.
    pub command: String,
    pub cwd: String,
    pub host: String,
    pub user: String,
    pub port: String,
    pub identity_file: String,
    pub group: String,
    pub theme: String,
    /// Kept from the profile being edited; the form doesn't show it.
    pub env: Vec<(String, String)>,
}

/// `path` as a form shows it: under the home folder, written with `~` and
/// `/` after it, so Windows doesn't get `~/.ssh\id_ed25519`.
pub fn home_relative(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => {
            let parts: Vec<_> = rest.iter().map(|part| part.to_string_lossy()).collect();
            format!("~/{}", parts.join("/"))
        }
        None => path.display().to_string(),
    }
}

impl ProfileForm {
    /// The form for `profile`, paths under the home folder written with `~`.
    pub fn of(profile: &Profile, home: Option<&Path>) -> Self {
        let path = |path: &Path| home_relative(path, home);
        let mut form = Self {
            name: profile.name.clone().unwrap_or_default(),
            theme: profile.theme.clone().unwrap_or_default(),
            ..Self::default()
        };
        match &profile.kind {
            ProfileKind::Local(local) => {
                form.command = local
                    .command
                    .as_ref()
                    .map(|c| c.join(" "))
                    .unwrap_or_default();
                form.cwd = local.cwd.as_deref().map(path).unwrap_or_default();
                form.env = local.env.clone().into_iter().collect();
            }
            ProfileKind::Ssh(ssh) => {
                form.ssh = true;
                form.host = ssh.host.clone();
                form.user = ssh.user.clone().unwrap_or_default();
                form.port = ssh.port.map(|port| port.to_string()).unwrap_or_default();
                form.identity_file = ssh.identity_file.as_deref().map(path).unwrap_or_default();
                form.group = ssh.group.clone().unwrap_or_default();
            }
        }
        form
    }

    /// Why the form can't be saved yet, if anything stops it.
    pub fn problem(&self) -> Option<&'static str> {
        if self.ssh && self.host.trim().is_empty() {
            return Some("An SSH profile needs a host.");
        }
        let port = self.port.trim();
        if self.ssh && !port.is_empty() && port.parse::<u16>().map_or(true, |port| port == 0) {
            return Some("The port is a number from 1 to 65535.");
        }
        None
    }

    /// The profile's entry under `profiles:`, block style.
    pub fn lines(&self, id: &str) -> Vec<String> {
        let mut lines = vec![format!("{}:", yaml::key_text(id))];
        let field = |key: &str, value: &str, text: fn(&str) -> String| {
            let value = value.trim();
            (!value.is_empty()).then(|| format!("  {key}: {}", text(value)))
        };
        lines.extend(field("name", &self.name, yaml::scalar));
        lines.extend(field(
            "type",
            if self.ssh { "ssh" } else { "local" },
            str::to_owned,
        ));
        if self.ssh {
            lines.extend(field("host", &self.host, yaml::scalar));
            lines.extend(field("user", &self.user, yaml::scalar));
            lines.extend(field("port", &self.port, str::to_owned));
            lines.extend(field("identity_file", &self.identity_file, yaml::scalar));
            lines.extend(field("group", &self.group, yaml::scalar));
        } else {
            let words: Vec<String> = self.command.split_whitespace().map(yaml::scalar).collect();
            if !words.is_empty() {
                lines.push(format!("  command: [{}]", words.join(", ")));
            }
            lines.extend(field("cwd", &self.cwd, yaml::scalar));
            if !self.env.is_empty() {
                let pairs: Vec<String> = self
                    .env
                    .iter()
                    .map(|(key, value)| format!("{}: {}", yaml::scalar(key), yaml::scalar(value)))
                    .collect();
                lines.push(format!("  env: {{ {} }}", pairs.join(", ")));
            }
        }
        lines.extend(field("theme", &self.theme, yaml::scalar));
        lines
    }
}

/// An ID for a new profile named `name`, unlike any in `taken`: lowercase,
/// words joined with `-`.
pub fn new_id(name: &str, taken: &[String]) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let base = if words.is_empty() {
        "profile".to_owned()
    } else {
        words.join("-")
    };
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LocalProfile, SidebarSide, SshProfile, TEMPLATE, parse};
    use std::path::PathBuf;

    /// A config with two profiles, each with a comment, and keys around them.
    const TWO_PROFILES: &str = "\
# my hosts
profiles:
  htop:
    name: Htop   # top
    type: local
    command: [htop]
  shell:
    type: local
    env: { EDITOR: nvim }
font:
  size: 13 # pt
";

    /// The lines `before` and `after` don't share at their start and end:
    /// what was taken out, and what was put in.
    fn changed(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
        let (old, new): (Vec<&str>, Vec<&str>) =
            (before.lines().collect(), after.lines().collect());
        let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(new[head..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let own = |lines: &[&str]| lines.iter().map(|l| (*l).to_owned()).collect();
        (
            own(&old[head..old.len() - tail]),
            own(&new[head..new.len() - tail]),
        )
    }

    fn profile(text: &str, id: &str) -> Profile {
        let loaded = parse(text);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        loaded
            .config
            .profiles
            .into_iter()
            .find(|p| p.id == id)
            .unwrap()
    }

    fn ssh_form() -> ProfileForm {
        ProfileForm {
            name: "Web".into(),
            ssh: true,
            host: "web.example.com".into(),
            user: "deploy".into(),
            port: "2222".into(),
            group: "Production".into(),
            ..ProfileForm::default()
        }
    }

    fn bind(keys: Option<&str>) -> Change {
        Change::Bind {
            command: "pane.splitRight",
            keys: keys.map(str::to_owned),
        }
    }

    /// Spec 0009, AC 1: a font size set on the template reads back as set.
    #[test]
    fn setting_font_size_on_the_template_reads_back() {
        let change = Change::Set {
            path: vec!["font", "size"],
            value: "14".into(),
            reads: |config| config.font.size.to_string(),
        };
        let edited = change.edited(TEMPLATE).unwrap();
        assert_eq!(parse(&edited).config.font.size, 14.0);
    }

    /// The settings screen's write of `update_check: value` (spec 0011).
    fn update_check(value: &str) -> Change {
        Change::Set {
            path: vec!["update_check"],
            value: value.into(),
            reads: |config| super::super::update_check_name(config).to_owned(),
        }
    }

    /// Spec 0011, AC1 and AC10: answering on the template writes
    /// `update_check` and changes no other line.
    #[test]
    fn answering_the_update_question_on_the_template_adds_only_its_line() {
        for value in ["daily", "off"] {
            let edited = update_check(value).edited(TEMPLATE).unwrap();
            let (gone, added) = changed(TEMPLATE, &edited);
            assert!(gone.is_empty(), "{value}: {gone:?}");
            let added: Vec<&String> = added.iter().filter(|line| !line.is_empty()).collect();
            assert_eq!(added, [&format!("update_check: {value}")], "{value}");
            assert_eq!(
                super::super::update_check_name(&parse(&edited).config),
                value
            );
        }
    }

    /// Spec 0011, AC1: a config without the key gets it, and nothing else
    /// changes.
    #[test]
    fn answering_the_update_question_keeps_the_rest_of_the_file() {
        let edited = update_check("daily").edited(TWO_PROFILES).unwrap();
        let (gone, added) = changed(TWO_PROFILES, &edited);
        assert!(gone.is_empty(), "{gone:?}");
        let added: Vec<&String> = added.iter().filter(|line| !line.is_empty()).collect();
        assert_eq!(added, ["update_check: daily"]);
        let loaded = parse(&edited);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert_eq!(loaded.config.profiles, parse(TWO_PROFILES).config.profiles);
    }

    /// Spec 0011 §Model, settings screen: switching between `daily` and
    /// `off` rewrites that line alone.
    #[test]
    fn switching_update_check_changes_only_its_line() {
        let text = "scrollback: 500\nupdate_check: daily\nfont:\n  size: 13 # pt\n";
        let edited = update_check("off").edited(text).unwrap();
        let (gone, added) = changed(text, &edited);
        assert_eq!(gone, ["update_check: daily"]);
        assert_eq!(added.len(), 1, "{added:?}");
        assert!(added[0].starts_with("update_check: off"), "{added:?}");
        assert_eq!(
            parse(&edited).config.update_check,
            Some(crate::config::UpdateCheck::Off)
        );
    }

    /// Spec 0011 §Model, AC10: nothing writes an `ask` value or any other
    /// than `daily` or `off`; such a write doesn't read back and is
    /// refused.
    #[test]
    fn an_update_check_other_than_daily_or_off_is_never_written() {
        for value in ["ask", "weekly", "\"\""] {
            assert!(update_check(value).edited(TEMPLATE).is_err(), "{value}");
        }
    }

    /// What the settings screen writes when the sidebar's side is picked
    /// (spec 0018, Migration): `app_sidebar`, and its old name's line gone.
    fn sidebar_side(value: &str) -> Change {
        Change::Renamed {
            from: "sftp_sidebar",
            to: "app_sidebar",
            value: value.into(),
            reads: |config| side_name(config).to_owned(),
        }
    }

    /// `app_sidebar` as the file writes it.
    fn side_name(config: &Config) -> &'static str {
        match config.app_sidebar {
            SidebarSide::Left => "left",
            SidebarSide::Right => "right",
        }
    }

    /// Spec 0018, Migration [load-bearing] and AC4: on a config that holds
    /// only the old `sftp_sidebar: left`, changing the side leaves
    /// `app_sidebar` and no `sftp_sidebar` line. Every other line, with
    /// its comment, stays as it was, and the file reads back as picked
    /// without a notice.
    #[test]
    fn changing_the_side_writes_app_sidebar_and_removes_the_sftp_sidebar_line() {
        let text = "\
# my settings
scrollback: 500   # lines
sftp_sidebar: left # where the files open
font:
  size: 13 # pt
";
        for value in ["right", "left"] {
            let edited = sidebar_side(value).edited(text).unwrap();
            assert!(!edited.contains("sftp_sidebar"), "{value}:\n{edited}");
            let written: Vec<&str> = edited
                .lines()
                .filter(|line| line.starts_with("app_sidebar:"))
                .collect();
            assert_eq!(written.len(), 1, "{value}:\n{edited}");
            assert!(
                written[0].starts_with(&format!("app_sidebar: {value}")),
                "{value}: {}",
                written[0]
            );
            for kept in [
                "# my settings",
                "scrollback: 500   # lines",
                "font:",
                "  size: 13 # pt",
            ] {
                assert!(
                    edited.lines().any(|line| line == kept),
                    "{value}: `{kept}` is gone from\n{edited}"
                );
            }
            let loaded = parse(&edited);
            assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
            assert_eq!(side_name(&loaded.config), value);
            assert_eq!(loaded.config.scrollback, 500);
            assert_eq!(loaded.config.font.size, 13.0);
        }
    }

    /// Spec 0018, Migration: in a file that has both names, changing the
    /// side rewrites `app_sidebar` and takes the stale `sftp_sidebar` line
    /// out, so one name is left.
    #[test]
    fn changing_the_side_of_a_config_with_both_names_leaves_app_sidebar_only() {
        let text = "sftp_sidebar: left\nscrollback: 500\napp_sidebar: left # mine\n";
        let edited = sidebar_side("right").edited(text).unwrap();
        assert!(!edited.contains("sftp_sidebar"), "{edited}");
        let written: Vec<&str> = edited
            .lines()
            .filter(|line| line.starts_with("app_sidebar:"))
            .collect();
        assert_eq!(written.len(), 1, "{edited}");
        assert!(written[0].starts_with("app_sidebar: right"), "{edited}");
        assert!(edited.contains("scrollback: 500\n"), "{edited}");
        assert_eq!(parse(&edited).config.app_sidebar, SidebarSide::Right);
    }

    /// Spec 0018, Module surface, and spec 0009 in-place edits: on the
    /// template, changing the side rewrites the `app_sidebar` line alone
    /// and keeps its comment.
    #[test]
    fn changing_the_side_on_the_template_rewrites_only_the_app_sidebar_line() {
        let edited = sidebar_side("left").edited(TEMPLATE).unwrap();
        let (gone, added) = changed(TEMPLATE, &edited);
        assert_eq!(gone.len(), 1, "{gone:?}");
        assert!(gone[0].starts_with("app_sidebar: right"), "{gone:?}");
        assert_eq!(added.len(), 1, "{added:?}");
        assert!(added[0].starts_with("app_sidebar: left "), "{added:?}");
        assert!(added[0].contains(" # "), "the comment is gone: {added:?}");
        assert!(!edited.contains("sftp_sidebar"), "{edited}");
        assert_eq!(parse(&edited).config.app_sidebar, SidebarSide::Left);
    }

    /// Spec 0018, Module surface (`sftp.toggle` stays an accepted name for
    /// `sidebar.toggle` in `keybindings:`), with spec 0009, flow 3
    /// ("Record" takes the next key combination as the command's key): in
    /// a config that binds the sidebar under its old name, recording a new
    /// key for "Toggle sidebar" leaves that key as the only one the config
    /// gives it.
    #[test]
    fn recording_a_key_for_the_sidebar_replaces_a_binding_under_its_old_name() {
        use crate::commands::keymap::plan;

        let text = "keybindings:\n  ctrl+shift+y: sftp.toggle\n  ctrl+shift+n: tab.new\n";
        let record = Change::Bind {
            command: "sidebar.toggle",
            keys: Some("ctrl+shift+k".into()),
        };
        let edited = record.edited(text).unwrap();
        let loaded = parse(&edited);
        let (planned, notices) = plan(&loaded.config.keybindings);
        assert!(notices.is_empty(), "{notices:?}");
        let keys: Vec<&str> = planned
            .iter()
            .filter(|(_, command)| command.id == "sidebar.toggle")
            .map(|(keys, _)| keys.as_str())
            .collect();
        assert_eq!(keys, ["ctrl-shift-k"], "{edited}");
    }

    /// Spec 0009, in-place edits and AC 6: a key in flow style isn't
    /// written, and the notice names the key to edit by hand.
    #[test]
    fn a_flow_style_font_size_is_refused_with_a_notice_naming_it() {
        let change = Change::Set {
            path: vec!["font", "size"],
            value: "14".into(),
            reads: |config| config.font.size.to_string(),
        };
        let notice = change.edited("font: { size: 13 }\n").unwrap_err();
        assert!(notice.contains("font.size"), "{notice}");
    }

    /// Spec 0009, in-place edits: flow-style keybindings aren't rewritten,
    /// and the notice names `keybindings`, the key to edit by hand.
    #[test]
    fn flow_style_keybindings_are_refused_with_a_notice_naming_the_key() {
        let text = "keybindings: { \"ctrl+shift+j\": pane.splitRight }\n";
        let notice = bind(Some("ctrl+shift+k")).edited(text).unwrap_err();
        assert!(notice.contains("keybindings"), "{notice}");
    }

    /// Spec 0009, flow 3 and AC 3: recording a key replaces the command's
    /// old binding in an existing `keybindings:` block, and leaves the
    /// others.
    #[test]
    fn recording_a_key_replaces_the_commands_old_binding() {
        let text = "keybindings:\n  \"ctrl+shift+j\": pane.splitRight\n  ctrl+shift+n: tab.new\n";
        let edited = bind(Some("ctrl+shift+k")).edited(text).unwrap();
        let bound: Vec<(String, String)> = parse(&edited)
            .config
            .keybindings
            .into_iter()
            .map(|b| (b.keys, b.command))
            .collect();
        assert_eq!(bound.len(), 2, "{bound:?}");
        assert!(bound.contains(&("ctrl+shift+k".into(), "pane.splitRight".into())));
        assert!(bound.contains(&("ctrl+shift+n".into(), "tab.new".into())));
    }

    /// Spec 0009, flow 3: on the template, whose `keybindings` is only a
    /// comment, recording a key adds a block and keeps the comment.
    #[test]
    fn recording_a_key_on_the_template_adds_a_keybindings_block() {
        let edited = bind(Some("ctrl+shift+k")).edited(TEMPLATE).unwrap();
        let (gone, added) = changed(TEMPLATE, &edited);
        assert!(gone.is_empty(), "{gone:?}");
        assert!(added.iter().any(|line| line == "keybindings:"), "{added:?}");
        let bindings = parse(&edited).config.keybindings;
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            (bindings[0].keys.as_str(), bindings[0].command.as_str()),
            ("ctrl+shift+k", "pane.splitRight")
        );
    }

    /// Spec 0009, flow 3: "Reset" removes the command's entry, and only it.
    #[test]
    fn resetting_a_key_removes_its_entry() {
        let text =
            "keybindings:\n  ctrl+shift+k: pane.splitRight # mine\n  ctrl+shift+n: tab.new\n";
        let edited = bind(None).edited(text).unwrap();
        assert_eq!(edited, "keybindings:\n  ctrl+shift+n: tab.new\n");
    }

    /// Spec 0009, flow 4 and AC 4: an added SSH profile goes in as a block
    /// under `profiles:`, and deleting it gives back the file as it was.
    #[test]
    fn an_added_profile_deleted_again_leaves_the_file_as_it_was() {
        let add = Change::Profile {
            id: "web".into(),
            profile: Some(Box::new(ssh_form())),
        };
        let added = add.edited(TWO_PROFILES).unwrap();
        let (gone, new) = changed(TWO_PROFILES, &added);
        assert!(gone.is_empty(), "{gone:?}");
        assert!(new.iter().all(|line| line.starts_with("  ")), "{new:?}");
        let ProfileKind::Ssh(ssh) = profile(&added, "web").kind else {
            panic!("not an SSH profile");
        };
        assert_eq!(ssh.host, "web.example.com");

        let delete = Change::Profile {
            id: "web".into(),
            profile: None,
        };
        assert_eq!(delete.edited(&added).unwrap(), TWO_PROFILES);
    }

    /// Spec 0018, A host's menu, and AC6: Delete… in the hosts panel
    /// removes that SSH profile's block, its comments with it, and keeps
    /// every other line and comment of the file, the other profiles too.
    #[test]
    fn deleting_an_ssh_profile_keeps_every_other_line_and_comment() {
        let text = "\
# my hosts
profiles:
  prod:
    name: Prod   # the live one
    type: ssh
    host: prod.example.com
    user: me
  stage:   # for testing
    type: ssh
    host: stage.example.com
font:
  size: 13 # pt
";
        let delete = Change::Profile {
            id: "prod".into(),
            profile: None,
        };
        let edited = delete.edited(text).unwrap();
        let (gone, added) = changed(text, &edited);
        assert_eq!(
            gone,
            [
                "  prod:",
                "    name: Prod   # the live one",
                "    type: ssh",
                "    host: prod.example.com",
                "    user: me",
            ]
        );
        assert!(added.is_empty(), "{added:?}");
        let loaded = parse(&edited);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        let ids: Vec<&str> = loaded
            .config
            .profiles
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids, ["stage"]);
    }

    /// Spec 0009, flow 4: editing a profile rewrites its block only.
    #[test]
    fn editing_a_profile_changes_only_its_lines() {
        let mut form = ProfileForm::of(&profile(TWO_PROFILES, "htop"), None);
        form.name = "Top".into();
        let edit = Change::Profile {
            id: "htop".into(),
            profile: Some(Box::new(form)),
        };
        let edited = edit.edited(TWO_PROFILES).unwrap();
        let (gone, _) = changed(TWO_PROFILES, &edited);
        assert_eq!(gone, ["    name: Htop   # top"]);
        assert_eq!(profile(&edited, "htop").name.as_deref(), Some("Top"));
        assert!(edited.contains("    env: { EDITOR: nvim }\nfont:\n  size: 13 # pt\n"));
    }

    /// Spec 0009, flow 4: a local profile's `env`, which the form doesn't
    /// show, survives an edit.
    #[test]
    fn a_local_profiles_env_survives_an_edit() {
        let mut form = ProfileForm::of(&profile(TWO_PROFILES, "shell"), None);
        form.cwd = "~/projects".into();
        let edit = Change::Profile {
            id: "shell".into(),
            profile: Some(Box::new(form)),
        };
        let edited = edit.edited(TWO_PROFILES).unwrap();
        let ProfileKind::Local(local) = profile(&edited, "shell").kind else {
            panic!("not a local profile");
        };
        assert_eq!(local.env.get("EDITOR").map(String::as_str), Some("nvim"));
    }

    /// Spec 0009, no secrets and AC 7: an SSH profile's block holds host,
    /// user, port and group, and never a password.
    #[test]
    fn an_ssh_profiles_lines_hold_its_fields_and_no_password() {
        let lines = ssh_form().lines("web");
        for field in [
            "  host: web.example.com",
            "  user: deploy",
            "  port: 2222",
            "  group: Production",
        ] {
            assert!(
                lines.iter().any(|line| line == field),
                "{field} in {lines:?}"
            );
        }
        assert!(
            !lines.iter().any(|line| line.contains("password")),
            "{lines:?}"
        );
    }

    /// Spec 0009, flow 2: an SSH profile without a host or with a port
    /// outside 1–65535 can't be saved, and the form says why.
    #[test]
    fn a_profile_without_a_host_or_with_a_bad_port_has_a_problem() {
        let no_host = ProfileForm {
            host: "  ".into(),
            ..ssh_form()
        };
        assert!(no_host.problem().is_some());
        for port in ["0", "70000", "abc", "-1"] {
            let form = ProfileForm {
                port: port.into(),
                ..ssh_form()
            };
            assert!(form.problem().is_some(), "port {port}");
        }
        for port in ["", "1", "65535"] {
            let form = ProfileForm {
                port: port.into(),
                ..ssh_form()
            };
            assert_eq!(form.problem(), None, "port {port}");
        }
        assert_eq!(ProfileForm::default().problem(), None);
    }

    /// Spec 0009, flow 4: a new profile gets a lowercase, dashed ID unlike
    /// any taken one.
    #[test]
    fn new_ids_are_lowercase_dashed_and_unique() {
        assert_eq!(new_id("My Prod Server", &[]), "my-prod-server");
        let taken = ["web".to_owned(), "web-2".to_owned()];
        assert_eq!(new_id("Web", &taken), "web-3");
        assert!(!new_id("", &[]).is_empty());
    }

    /// Spec 0009, platform notes: paths are written as typed, so a path
    /// under the home folder shows with `~`, and one outside it in full.
    #[test]
    fn the_form_writes_paths_under_home_with_a_tilde() {
        let home = PathBuf::from("/home/someone");
        let local = Profile {
            id: "work".into(),
            name: None,
            theme: None,
            kind: ProfileKind::Local(LocalProfile {
                cwd: Some(home.join("projects")),
                ..LocalProfile::default()
            }),
        };
        assert_eq!(ProfileForm::of(&local, Some(&home)).cwd, "~/projects");
        let ssh = Profile {
            kind: ProfileKind::Ssh(SshProfile {
                host: "web".into(),
                user: None,
                port: None,
                identity_file: Some(home.join(".ssh/id_ed25519")),
                group: None,
            }),
            ..local
        };
        assert_eq!(
            ProfileForm::of(&ssh, Some(&home)).identity_file,
            "~/.ssh/id_ed25519"
        );
        let elsewhere = Profile {
            kind: ProfileKind::Local(LocalProfile {
                cwd: Some(PathBuf::from("/srv/www")),
                ..LocalProfile::default()
            }),
            ..ssh
        };
        assert_eq!(ProfileForm::of(&elsewhere, Some(&home)).cwd, "/srv/www");
    }

    /// An enabled plugin entry with every kind of setting, including texts
    /// that a careless writer would let YAML read as something else.
    fn plugin_entry() -> PluginSettings {
        use crate::plugins::{Right, Subscription};
        let settings = [
            ("debug", Setting::Bool(true)),
            ("quiet", Setting::Bool(false)),
            ("port", Setting::Number(8080.0)),
            ("ratio", Setting::Number(0.25)),
            ("offset", Setting::Number(-3.5)),
            ("version", Setting::Text("8080".into())),
            ("float_text", Setting::Text("1.5".into())),
            ("bool_text", Setting::Text("true".into())),
            ("empty", Setting::Text(String::new())),
            ("greeting", Setting::Text("say \"hi\" to C:\\Users".into())),
            ("socket", Setting::Text("unix: /var/run/docker.sock".into())),
            ("hash", Setting::Text("a # not a comment".into())),
        ];
        PluginSettings {
            enabled: true,
            run: vec!["./tern-docker".into(), "--verbose".into(), "a b".into()],
            rights: vec![Right::PaneInfo, Right::TerminalWrite],
            events: vec![Subscription::Start, Subscription::PaneClosed],
            settings: settings
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        }
    }

    fn plugin(id: &str, entry: PluginSettings) -> Change {
        Change::Plugin {
            id: id.into(),
            entry,
        }
    }

    /// The `plugins:` entry of `id` in `text`, which must load cleanly.
    fn loaded_plugin(text: &str, id: &str) -> Option<PluginSettings> {
        let loaded = parse(text);
        assert!(loaded.notices.is_empty(), "{:?}\n{text}", loaded.notices);
        loaded.config.plugins.get(id).cloned()
    }

    /// Spec 0016 flows 2 and 8, spec 0009 in-place edits: a plugin's entry
    /// written into an empty file reads back as the same entry, every
    /// setting with its type.
    #[test]
    fn a_plugin_entry_written_into_an_empty_file_reads_back_the_same() {
        let entry = plugin_entry();
        let edited = plugin("docker", entry.clone()).edited("").unwrap();
        assert_eq!(loaded_plugin(&edited, "docker"), Some(entry), "{edited}");
    }

    /// A disabled entry with nothing ticked or set is written too, and
    /// reads back as such.
    #[test]
    fn an_empty_plugin_entry_reads_back_the_same() {
        let edited = plugin("bare", PluginSettings::default())
            .edited(TEMPLATE)
            .unwrap();
        assert_eq!(
            loaded_plugin(&edited, "bare"),
            Some(PluginSettings::default())
        );
    }

    /// Spec 0009 in-place edits: writing one plugin into a file that has
    /// others and comments adds its lines and keeps every other line.
    #[test]
    fn writing_a_plugin_keeps_the_other_plugins_and_comments() {
        let before = "\
# my setup
plugins:
  # the one I use daily
  aaa:
    enabled: true # yes
    run: [./aaa]
    rights: [pane_info]
    events: []
    settings: { level: 3 }
  zzz:
    enabled: false
font:
  size: 13 # pt
";
        let entry = plugin_entry();
        let edited = plugin("docker", entry.clone()).edited(before).unwrap();
        let (gone, _) = changed(before, &edited);
        assert!(gone.is_empty(), "lines lost: {gone:?}\n{edited}");
        for line in before.lines() {
            assert!(edited.contains(line), "`{line}` lost:\n{edited}");
        }
        assert_eq!(loaded_plugin(&edited, "docker"), Some(entry));
        let before_config = parse(before).config;
        let after_config = parse(&edited).config;
        for id in ["aaa", "zzz"] {
            assert_eq!(after_config.plugins.get(id), before_config.plugins.get(id));
        }
        assert_eq!(after_config.font.size, 13.0);
    }

    /// Flows 2 and 8: a plugin's entry is written whole, replacing what
    /// the file had for it (a right unticked, an event added, a setting
    /// gone) and leaving its neighbours alone.
    #[test]
    fn writing_a_plugin_replaces_its_existing_entry_whole() {
        let before = "\
plugins:
  docker:
    enabled: false
    run: [./old]
    rights: [open_tab, terminal_read]
    events: [pane_opened]
    settings: { stale: gone, port: 1 }
  other:
    enabled: true
    run: [./other]
theme:
  default: dark
";
        let entry = plugin_entry();
        let edited = plugin("docker", entry.clone()).edited(before).unwrap();
        assert_eq!(loaded_plugin(&edited, "docker"), Some(entry), "{edited}");
        assert!(!edited.contains("stale"), "{edited}");
        assert!(!edited.contains("./old"), "{edited}");
        assert_eq!(
            parse(&edited).config.plugins.get("other"),
            parse(before).config.plugins.get("other")
        );
        assert!(edited.ends_with("theme:\n  default: dark\n"), "{edited}");

        // Disabling writes `enabled: false` and keeps the rest of the entry.
        let disabled = PluginSettings {
            enabled: false,
            ..plugin_entry()
        };
        let edited = plugin("docker", disabled.clone()).edited(&edited).unwrap();
        assert_eq!(loaded_plugin(&edited, "docker"), Some(disabled));
    }

    /// Spec 0009, criterion 9: a pick under home on Windows is written
    /// with `/` after the `~`, not `~/.ssh\id_ed25519`.
    #[cfg(windows)]
    #[test]
    fn a_windows_path_under_home_is_written_with_slashes() {
        let home = Path::new(r"C:\Users\me");
        let picked = Path::new(r"C:\Users\me\.ssh\id_ed25519");
        assert_eq!(home_relative(picked, Some(home)), "~/.ssh/id_ed25519");
    }
}
