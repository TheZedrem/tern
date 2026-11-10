//! `config.yaml` (spec 0005): settings, profiles and inline themes.
//!
//! Loading never fails and never writes. Text that isn't YAML gives the
//! defaults and a notice naming the line. In valid YAML, an unknown setting
//! is a notice, and a value of the wrong type makes its top-level key, its
//! profile or its inline theme fall back on its own, with a notice, while
//! the rest of the file still applies. serde-saphyr can't resume a document
//! after an error, so [`parse`] notes the part that failed and reads the
//! document again without it.

use crate::theme::ThemeSource;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};

mod edit;
mod load;
mod watch;

pub use edit::{save_edit, save_theme_default};
pub use load::{load, parse};
pub use watch::Watcher;

/// What "Open config file" writes when there is no config yet: every key at
/// its default, explained. It parses back to exactly [`Config::default`].
pub const TEMPLATE: &str = include_str!("../default-config.yaml");

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// The profile new tabs open, as written; see [`Config::default_profile`].
    pub default_profile: Option<String>,
    /// In file order, without the implicit `shell`; see
    /// [`Config::all_profiles`].
    pub profiles: Vec<Profile>,
    pub font: Font,
    pub scrollback: u32,
    pub close_on_exit: CloseOnExit,
    pub theme: ThemeSettings,
    /// In file order; checked against the commands when they're bound.
    pub keybindings: Vec<Keybinding>,
    /// Relaunch with the tabs of the last run; `false` deletes the saved
    /// session on the next quit (spec 0003).
    pub restore_session: bool,
    /// Copy a selection to the clipboard as soon as it's made (spec 0002,
    /// flow 4).
    pub copy_on_select: bool,
    pub cursor: Cursor,
    /// What opens a link (spec 0002, flow 7).
    pub open_links: OpenLinks,
    /// Which side of the window the tabs are on (spec 0003).
    pub tab_bar: TabBarSide,
    /// Which side of the window an SSH pane's SFTP sidebar is on (spec
    /// 0008).
    pub sftp_sidebar: SftpSide,
    /// What Enter or a double click does to a file in the SFTP sidebar.
    pub sftp_click: SftpClick,
    /// When an SFTP edit uploads, until the edit says otherwise.
    pub sftp_upload: UploadMode,
    /// What Enter or a double click does to a runnable local file in a
    /// files pane (spec 0013, flow 4).
    pub files_executable: FilesExecutable,
    /// What `Ctrl+Shift+T` opens; `Ctrl+Alt+Shift+T` opens the other
    /// (spec 0004).
    pub new_tab: NewTab,
    /// Whether to look for a new release on GitHub (spec 0011). `None`:
    /// not chosen yet, so Tern asks; nothing ever writes that as a value.
    pub update_check: Option<UpdateCheck>,
}

/// The two answers to "check GitHub for new releases?" (spec 0011).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheck {
    Daily,
    Off,
}

/// What the new-tab key opens: asks which profile, or the default one
/// right away (spec 0004).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NewTab {
    Ask,
    Default,
}

/// Which side of the window the tabs are on: a bar on top or at the
/// bottom, a list down the left or the right (spec 0003).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabBarSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl TabBarSide {
    /// A list down a side, rather than a bar.
    pub fn vertical(self) -> bool {
        matches!(self, TabBarSide::Left | TabBarSide::Right)
    }
}

/// Which side of the window the SFTP sidebar is on (spec 0008). Inside the
/// tab list when the tabs are on the same side.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SftpSide {
    Left,
    Right,
}

/// What Enter or a double click does to a file in the SFTP sidebar (spec
/// 0008, flow 2).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SftpClick {
    Edit,
    Download,
}

/// When an SFTP edit's saves go back to the host (spec 0008, Model):
/// only on the user's click, on each save, or when Tern's window gains
/// focus.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UploadMode {
    Manual,
    Save,
    Focus,
}

/// What Enter or a double click does to a runnable local file (spec 0013,
/// flow 4): ask whether to run it or open it as text, or do one of them.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilesExecutable {
    Ask,
    Run,
    Edit,
}

/// What opens a link: Ctrl+click, the owner's default, or a plain click
/// (Cmd+click on macOS), as most terminals do.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenLinks {
    Click,
    CtrlClick,
}

/// The cursor a pane starts with; a program may change it, as vim does in
/// insert mode (spec 0002).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub style: CursorStyle,
    pub blink: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            style: CursorStyle::Block,
            blink: true,
        }
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    Block,
    Beam,
    Underline,
}

/// One `keybindings:` entry as written (spec 0004).
#[derive(Clone, Debug, PartialEq)]
pub struct Keybinding {
    /// Like `ctrl+shift+d`.
    pub keys: String,
    /// A command ID, like `pane.splitRight`.
    pub command: String,
    pub line: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// An installed font's exact family name. `None` means the font bundled
    /// in the binary.
    pub family: Option<String>,
    pub size: f64,
    pub line_height: f64,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloseOnExit {
    Always,
    Clean,
    Never,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeSettings {
    /// `dark`, `light`, `system` or any theme name.
    pub default: String,
    /// `theme.custom`, in file order: the first place a theme name is
    /// looked up.
    pub custom: Vec<(String, ThemeSource)>,
}

/// A profile: what a pane runs (spec 0005).
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: Option<String>,
    /// The theme of this profile's panes; `None` follows `theme.default`.
    pub theme: Option<String>,
    pub kind: ProfileKind,
}

/// A profile's `type`, with the keys that only it has.
#[derive(Clone, Debug, PartialEq)]
pub enum ProfileKind {
    Local(LocalProfile),
    Ssh(SshProfile),
}

/// `type: local`: a program on this machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalProfile {
    /// Program and arguments. `None` runs the user's login shell.
    pub command: Option<Vec<String>>,
    /// With `~` expanded. A directory that doesn't exist falls back to home
    /// when the pane starts.
    pub cwd: Option<PathBuf>,
    /// Set on top of the inherited environment.
    pub env: BTreeMap<String, String>,
}

/// `type: ssh`: a host the system `ssh` connects to (spec 0006). Only the
/// options set here go on its command line, so `~/.ssh/config` still
/// applies to the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct SshProfile {
    /// A host name or a `Host` alias from `~/.ssh/config`.
    pub host: String,
    pub user: Option<String>,
    /// 1–65535: OpenSSH refuses port 0, and that refusal would look like a
    /// dropped connection.
    pub port: Option<NonZeroU16>,
    /// With `~` expanded.
    pub identity_file: Option<PathBuf>,
    /// The host picker's group; `None` is "Hosts".
    pub group: Option<String>,
}

/// What the ID of a host from `~/.ssh/config` starts with: `ssh:<alias>`.
/// Splits and session restore find the host again by it (spec 0006).
pub const ALIAS_PREFIX: &str = "ssh:";

impl Default for Config {
    fn default() -> Self {
        Self {
            default_profile: None,
            profiles: Vec::new(),
            font: Font::default(),
            scrollback: 10_000,
            close_on_exit: CloseOnExit::Clean,
            theme: ThemeSettings::default(),
            keybindings: Vec::new(),
            restore_session: true,
            copy_on_select: false,
            cursor: Cursor::default(),
            open_links: OpenLinks::CtrlClick,
            tab_bar: TabBarSide::Top,
            sftp_sidebar: SftpSide::Right,
            sftp_click: SftpClick::Edit,
            sftp_upload: UploadMode::Manual,
            files_executable: FilesExecutable::Ask,
            new_tab: NewTab::Ask,
            update_check: None,
        }
    }
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: None,
            size: 13.0,
            line_height: 1.2,
        }
    }
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            default: "dark".into(),
            custom: Vec::new(),
        }
    }
}

impl Config {
    /// Every profile: the implicit `shell` unless one is configured with
    /// that ID, then the configured ones in file order (spec 0005).
    pub fn all_profiles(&self) -> Vec<Profile> {
        let implicit = self.profile(SHELL).filter(|_| !self.configures(SHELL));
        implicit
            .into_iter()
            .chain(self.profiles.iter().cloned())
            .collect()
    }

    /// Profile `id`, counting the implicit `shell`.
    pub fn profile(&self, id: &str) -> Option<Profile> {
        self.profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
            .or_else(|| (id == SHELL).then(Profile::shell))
            .or_else(|| id.strip_prefix(ALIAS_PREFIX).and_then(Profile::ssh_alias))
    }

    /// Whether the file has a profile with this ID. `shell` is built-in
    /// exactly when it doesn't.
    pub fn configures(&self, id: &str) -> bool {
        self.profiles.iter().any(|profile| profile.id == id)
    }

    /// The profile new tabs open: `default_profile` if it names a profile,
    /// else `shell`.
    pub fn default_profile(&self) -> Profile {
        self.default_profile
            .as_deref()
            .and_then(|id| self.profile(id))
            .or_else(|| self.profile(SHELL))
            .unwrap_or_else(Profile::shell)
    }

    /// Profile `id` as configured now, or the default profile if it's gone
    /// (spec 0003: a missing profile opens `default_profile`).
    pub fn profile_or_default(&self, id: &str) -> Profile {
        self.profile(id).unwrap_or_else(|| self.default_profile())
    }
}

/// The ID of the profile that is always there: the user's login shell,
/// unless a configured profile takes the ID (spec 0005).
pub const SHELL: &str = "shell";

impl Profile {
    /// The implicit `shell` profile: the user's login shell.
    pub fn shell() -> Self {
        Self {
            id: SHELL.into(),
            name: None,
            theme: None,
            kind: ProfileKind::Local(LocalProfile::default()),
        }
    }

    /// The profile of a `Host` alias from `~/.ssh/config`: plain
    /// `ssh <alias>`, so the alias's own settings apply. `None` for an
    /// alias ssh would read as an option.
    pub fn ssh_alias(alias: &str) -> Option<Self> {
        (!alias.is_empty() && !alias.starts_with('-')).then(|| Self {
            id: format!("{ALIAS_PREFIX}{alias}"),
            name: Some(alias.to_owned()),
            theme: None,
            kind: ProfileKind::Ssh(SshProfile {
                host: alias.to_owned(),
                user: None,
                port: None,
                identity_file: None,
                group: None,
            }),
        })
    }

    /// The name tab titles and the palette show: `name`, else the ID.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// The directory the profile starts in, for a local profile that sets
    /// one.
    pub fn cwd(&self) -> Option<&Path> {
        match &self.kind {
            ProfileKind::Local(local) => local.cwd.as_deref(),
            ProfileKind::Ssh(_) => None,
        }
    }
}

/// A config plus the notices produced while loading it.
#[derive(Debug, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub notices: Vec<String>,
}

/// `<config_dir>/tern`, which holds `config.yaml` and `themes/`, or
/// `None` if the OS has no config directory.
pub fn default_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("tern"))
}

/// `<config_dir>/tern/config.yaml`.
pub fn default_path() -> Option<PathBuf> {
    default_dir().map(|dir| dir.join("config.yaml"))
}

/// `<config_dir>/tern/themes`, where theme files live.
pub fn themes_dir() -> Option<PathBuf> {
    default_dir().map(|dir| dir.join("themes"))
}

/// `~` and `~/…` (`~\…` on Windows too) start at the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    let rest = path.strip_prefix('~').filter(|rest| {
        rest.is_empty() || rest.starts_with('/') || (cfg!(windows) && rest.starts_with('\\'))
    });
    match (rest, dirs::home_dir()) {
        (Some(""), Some(home)) => home,
        (Some(rest), Some(home)) => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// Writes [`TEMPLATE`] to `path` unless a file is already there, creating
/// the directory as needed, so "Open config file" always has a file to
/// open (spec 0005). Never overwrites, even a file that appears meanwhile.
pub fn ensure_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file
            .write_all(TEMPLATE.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terminal::pty::Command;
    use std::fs;

    /// A new, empty directory in the system's temp dir, removed again on
    /// drop, so no test touches the real config directory. `name` must be
    /// unique among all tests.
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("tern-test-{name}-{}", std::process::id()));
            // Left behind by an earlier run that was killed.
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create a temp dir");
            Self(path)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// An ssh profile's connection to `host` with no option set. Shared
    /// with the tests of the modules that run one.
    pub(crate) fn ssh_to(host: &str) -> SshProfile {
        SshProfile {
            host: host.into(),
            user: None,
            port: None,
            identity_file: None,
            group: None,
        }
    }

    /// The IDs of `config`'s profiles, in order.
    pub(super) fn profile_ids(config: &Config) -> Vec<&str> {
        config
            .profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect()
    }

    #[test]
    fn ensure_exists_writes_the_template_and_the_directories_above_it() {
        let root = TempDir::new("ensure-new");
        let path = root.path().join("config/tern/config.yaml");
        ensure_exists(&path).expect("create config.yaml");
        assert_eq!(fs::read_to_string(&path).expect("read it back"), TEMPLATE);
    }

    #[test]
    fn ensure_exists_never_overwrites_a_config() {
        let root = TempDir::new("ensure-existing");
        let path = root.path().join("config.yaml");
        fs::write(&path, "scrollback: 5\n").expect("write a config");
        ensure_exists(&path).expect("keep config.yaml");
        assert_eq!(
            fs::read_to_string(&path).expect("read it back"),
            "scrollback: 5\n"
        );
    }

    #[test]
    fn new_tabs_open_default_profile_else_the_implicit_shell() {
        let profiles = concat!(
            "profiles:\n",
            "  zeta:\n",
            "    type: local\n",
            "  alpha:\n",
            "    type: local\n",
        );
        let named = parse(&format!("default_profile: alpha\n{profiles}"));
        assert_eq!(named.config.default_profile().id, "alpha");
        let unnamed = parse(profiles);
        assert_eq!(unnamed.config.default_profile(), Profile::shell());
        assert!(named.notices.is_empty(), "{:?}", named.notices);
        assert!(unnamed.notices.is_empty(), "{:?}", unnamed.notices);
    }

    /// Spec 0005, flow 1.
    #[test]
    fn an_unknown_default_profile_opens_the_shell_with_a_notice() {
        let loaded = parse(concat!(
            "default_profile: nope\n",
            "profiles:\n",
            "  zeta:\n",
            "    type: local\n",
            "  alpha:\n",
            "    type: local\n",
        ));
        assert_eq!(loaded.config.default_profile(), Profile::shell());
        assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
        assert!(loaded.notices[0].contains("nope"), "{}", loaded.notices[0]);
    }

    /// `shell` is always a profile (spec 0005 §Model), so naming it is no
    /// mistake, with or without other profiles. The template suggests
    /// exactly this line.
    #[test]
    fn default_profile_shell_names_the_implicit_profile_without_a_notice() {
        let htop = "profiles:\n  htop:\n    type: local\n    command: [htop]\n";
        for profiles in ["", htop] {
            let Loaded { config, notices } = parse(&format!("default_profile: shell\n{profiles}"));
            assert_eq!(config.default_profile(), Profile::shell(), "{profiles:?}");
            assert!(notices.is_empty(), "{profiles:?}: {notices:?}");
        }
    }

    /// SSH hosts don't take over new tabs (spec 0005 §Model): those open
    /// the login shell unless `default_profile` names another profile.
    #[test]
    fn with_only_ssh_profiles_new_tabs_still_open_the_login_shell() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
            "  db:\n",
            "    type: ssh\n",
            "    host: db.example.com\n",
        ));
        let tab = config.default_profile();
        assert_eq!(tab, Profile::shell());
        assert_eq!(Command::of_profile(&tab), Command::default_shell());
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// What the palette lists (spec 0005 §Model).
    #[test]
    fn all_profiles_are_the_implicit_shell_then_the_configured_ones_in_file_order() {
        assert_eq!(parse("").config.all_profiles(), [Profile::shell()]);
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  zeta:\n",
            "    type: local\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
            "  alpha:\n",
            "    type: local\n",
        ));
        let all = config.all_profiles();
        let ids: Vec<_> = all.iter().map(|profile| profile.id.as_str()).collect();
        assert_eq!(ids, ["shell", "zeta", "prod", "alpha"]);
        assert_eq!(all[0], Profile::shell());
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// A configured `shell` takes the login shell's place for new tabs,
    /// and the palette lists it where the file has it.
    #[test]
    fn a_configured_shell_replaces_the_implicit_one_at_its_place_in_file_order() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  zeta:\n",
            "    type: local\n",
            "  shell:\n",
            "    type: local\n",
            "    command: [fish]\n",
            "  alpha:\n",
            "    type: local\n",
        ));
        let fish = Profile {
            kind: ProfileKind::Local(LocalProfile {
                command: Some(vec!["fish".into()]),
                ..LocalProfile::default()
            }),
            ..Profile::shell()
        };
        let all = config.all_profiles();
        let ids: Vec<_> = all.iter().map(|profile| profile.id.as_str()).collect();
        assert_eq!(ids, ["zeta", "shell", "alpha"]);
        assert_eq!(all[1], fish);
        assert_eq!(config.default_profile(), fish);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// `shell` is a profile whether it's configured or not; no other ID is.
    #[test]
    fn profile_shell_is_the_configured_shell_else_the_login_shell() {
        let htop = "  htop:\n    type: local\n    command: [htop]\n";
        let implicit = parse(&format!("profiles:\n{htop}")).config;
        assert_eq!(implicit.profile("shell"), Some(Profile::shell()));
        assert_eq!(implicit.profile("nope"), None);
        let fish = "  shell:\n    type: local\n    command: [fish]\n";
        let configured = parse(&format!("profiles:\n{htop}{fish}")).config;
        let shell = configured.profile("shell").expect("the configured shell");
        assert_eq!(Command::of_profile(&shell).program, "fish");
    }

    /// A restored tab whose profile is gone opens `default_profile`, else
    /// the shell (spec 0003). A `shell` tab's profile is never gone.
    #[test]
    fn a_profile_thats_gone_opens_default_profile_else_the_shell() {
        let htop = "profiles:\n  htop:\n    type: local\n    command: [htop]\n";
        let named = parse(&format!("default_profile: htop\n{htop}")).config;
        assert_eq!(named.profile_or_default("gone").id, "htop");
        assert_eq!(named.profile_or_default("shell"), Profile::shell());
        let unnamed = parse(htop).config;
        assert_eq!(unnamed.profile_or_default("gone"), Profile::shell());
    }

    /// A host from `~/.ssh/config` opens by its ID, `ssh:<alias>`, as
    /// plain `ssh <alias>` (spec 0006 §Model). Splits and restored tabs
    /// find it again by that ID.
    #[test]
    fn an_ssh_alias_id_is_an_ssh_profile_for_that_alias_with_no_options() {
        let profile = Config::default()
            .profile("ssh:web")
            .expect("`ssh:web` is a profile");
        assert_eq!(profile.id, "ssh:web");
        assert_eq!(profile.kind, ProfileKind::Ssh(ssh_to("web")));
    }

    /// ssh reads an argument that starts with `-` as an option.
    #[test]
    fn an_ssh_alias_id_whose_alias_starts_with_a_dash_is_no_profile() {
        let id = "ssh:-oProxyCommand=echo pwned";
        assert_eq!(Config::default().profile(id), None);
    }

    /// Like `shell`, an `ssh:` ID is the configured profile if there is one.
    #[test]
    fn a_configured_profile_wins_over_an_ssh_alias_id() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  \"ssh:web\":\n",
            "    type: ssh\n",
            "    host: web.example.com\n",
            "    user: alice\n",
        ));
        let configured = ProfileKind::Ssh(SshProfile {
            user: Some("alice".into()),
            ..ssh_to("web.example.com")
        });
        let kind = config.profile("ssh:web").map(|profile| profile.kind);
        assert_eq!(kind, Some(configured));
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// An SSH pane's own program starts at home; the directory that
    /// matters is on the remote side.
    #[test]
    fn only_a_local_profile_has_a_cwd() {
        let projects = PathBuf::from("/srv/projects");
        let local = Profile {
            kind: ProfileKind::Local(LocalProfile {
                cwd: Some(projects.clone()),
                ..LocalProfile::default()
            }),
            ..Profile::shell()
        };
        assert_eq!(local.cwd(), Some(projects.as_path()));
        assert_eq!(Profile::shell().cwd(), None);
        let ssh = Profile {
            kind: ProfileKind::Ssh(ssh_to("prod.example.com")),
            ..Profile::shell()
        };
        assert_eq!(ssh.cwd(), None);
    }

    #[test]
    fn a_profiles_display_name_defaults_to_its_id() {
        let htop = Profile {
            id: "htop".into(),
            ..Profile::shell()
        };
        assert_eq!(htop.display_name(), "htop");
        let named = Profile {
            name: Some("Htop".into()),
            ..htop
        };
        assert_eq!(named.display_name(), "Htop");
    }

    #[test]
    fn a_tilde_means_home_only_at_the_start_of_a_path() {
        let home = dirs::home_dir().expect("home dir");
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/projects"), home.join("projects"));
        if cfg!(windows) {
            assert_eq!(expand_home("~\\projects"), home.join("projects"));
        }
        for literal in ["/srv/projects", "projects/~", "~alice/projects"] {
            assert_eq!(expand_home(literal), PathBuf::from(literal));
        }
    }
}
