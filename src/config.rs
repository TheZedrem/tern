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
use futures::channel::mpsc::UnboundedSender;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_saphyr::Spanned;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write as _;
use std::marker::PhantomData;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What "Open config file" writes when there is no config yet: every key at
/// its default, explained. It parses back to exactly [`Config::default`].
pub const TEMPLATE: &str = include_str!("default-config.yaml");

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

    fn configures(&self, id: &str) -> bool {
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

/// `<config_dir>/zed-term`, which holds `config.yaml` and `themes/`, or
/// `None` if the OS has no config directory.
pub fn default_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("zed-term"))
}

/// `<config_dir>/zed-term/config.yaml`.
pub fn default_path() -> Option<PathBuf> {
    default_dir().map(|dir| dir.join("config.yaml"))
}

/// `<config_dir>/zed-term/themes`, where theme files live.
pub fn themes_dir() -> Option<PathBuf> {
    default_dir().map(|dir| dir.join("themes"))
}

/// Reads and parses the config at `path`. A missing file means defaults
/// with no notice; it is never created here (no writes on the boot path).
pub fn load(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded {
            config: Config::default(),
            notices: vec![],
        },
        Err(e) => Loaded {
            config: Config::default(),
            notices: vec![format!(
                "config.yaml could not be read ({e}); using defaults"
            )],
        },
    }
}

pub fn parse(text: &str) -> Loaded {
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    loop {
        let reading = RefCell::new(None);
        let notices = RefCell::new(Vec::new());
        let attempt = Attempt {
            skip: &skipped,
            reading: &reading,
            notices: &notices,
        };
        match serde_saphyr::with_deserializer_from_str(text, |de| Optional(attempt).deserialize(de))
        {
            Ok(config) => {
                failures.extend(notices.into_inner());
                return finish(config, failures);
            }
            Err(error) => match reading.into_inner() {
                Some(part) if !skipped.contains(&part) => {
                    failures.push(format!(
                        "config.yaml{}: {}: {}",
                        at_line(error.location().map(|l| l.line())),
                        part.fallback(),
                        reason(&error)
                    ));
                    skipped.push(part);
                }
                // Not pinned to one part (or failing again while skipped):
                // the text isn't valid YAML.
                _ => {
                    return Loaded {
                        config: Config::default(),
                        notices: vec![format!(
                            "config.yaml{}: {}. Using defaults until it's fixed.",
                            at_line(error.location().map(|l| l.line())),
                            reason(&error)
                        )],
                    };
                }
            },
        }
    }
}

/// Checks what only the whole config can: that `default_profile` names a
/// profile, the implicit `shell` included.
fn finish(config: Config, mut notices: Vec<String>) -> Loaded {
    if let Some(id) = &config.default_profile
        && *id != config.default_profile().id
    {
        notices.push(format!(
            "config.yaml: default_profile `{id}` isn't a profile; new tabs open `{}`.",
            config.default_profile().id
        ));
    }
    Loaded { config, notices }
}

/// " line N", or nothing when the location is unknown.
fn at_line(line: Option<u64>) -> String {
    line.filter(|line| *line > 0)
        .map(|line| format!(" line {line}"))
        .unwrap_or_default()
}

/// The first line of a parse error, without the location the notice
/// already names.
fn reason(error: &serde_saphyr::Error) -> String {
    let rendered = error.to_string();
    let first = rendered.lines().next().unwrap_or("invalid YAML");
    let bare = first
        .strip_prefix("error: ")
        .and_then(|rest| rest.strip_prefix("line "))
        .and_then(|rest| rest.split_once(": "))
        .map_or(first, |(_, message)| message);
    // serde-saphyr suggests one of its own options here.
    bare.split(", set DuplicateKeyPolicy")
        .next()
        .unwrap_or(bare)
        .to_owned()
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

/// Saves `name` as `theme.default` in the config at `path`, creating the
/// config from [`TEMPLATE`] if there is none, and changing nothing else
/// (spec 0005, flow 4). A file that changes while it's being edited is
/// read again once; if it changes again, nothing is written (flow 5).
pub fn save_theme_default(path: &Path, name: &str) -> Result<(), String> {
    ensure_exists(path)?;
    // A linked config is edited where it points, so the link stays.
    let path = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let read =
        |path: &Path| std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
    for _ in 0..2 {
        let text = read(&path)?;
        let edited = with_theme_default(&text, name).ok_or(
            "config.yaml writes `theme` in a way this can't change safely; \
             set `theme.default` there yourself",
        )?;
        if read(&path)? == text {
            return write_replacing(&path, &edited);
        }
    }
    Err("config.yaml kept changing while the theme was saved, so it was left alone".into())
}

/// Writes `text` over `path` through a temporary file, so a reader never
/// sees half of it, and keeps the file's permissions.
fn write_replacing(path: &Path, text: &str) -> Result<(), String> {
    let temporary = path.with_extension("yaml.tmp");
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    std::fs::write(&temporary, text).map_err(fail)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temporary, meta.permissions());
    }
    std::fs::rename(&temporary, path).map_err(fail)
}

/// `text`, a config, with `theme.default` set to `name` and the rest
/// (comments, formatting, other keys) left as it was. `None` when that
/// can't be done safely: the text isn't YAML, `theme` is written inline as
/// `{ … }`, or it isn't a mapping.
pub fn with_theme_default(text: &str, name: &str) -> Option<String> {
    let edited = edit_theme_default(text, name)?;
    // Never hand back a config that reads differently from what was asked.
    let reads_back = serde_saphyr::from_str::<IgnoredAny>(&edited).is_ok()
        && parse(&edited).config.theme.default == name;
    reads_back.then_some(edited)
}

fn edit_theme_default(text: &str, name: &str) -> Option<String> {
    let spots =
        serde_saphyr::with_deserializer_from_str(text, |de| Optional(SpotFinder).deserialize(de))
            .ok()?;
    let value = yaml_scalar(name);
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut edited = text.to_owned();
    if let Some(range) = spots.default {
        edited.replace_range(range, &value);
        return Some(edited);
    }
    match spots.theme {
        Some(ThemeSpot::Inline) => None,
        // A `theme:` block without `default`: it becomes its first line.
        Some(ThemeSpot::Block { key, indent }) => {
            let line_end = text
                .get(key..)?
                .find('\n')
                .map_or(text.len(), |len| key + len);
            let line = text[key..line_end].trim_end_matches('\r');
            // Only `theme:` with at most a comment after it starts a block
            // that takes another line: below `theme: {}` or `theme: ~` it
            // would be broken YAML.
            let after = line.split_once(':')?.1.trim_start();
            if !(after.is_empty() || after.starts_with('#')) {
                return None;
            }
            let at = line_end - usize::from(text[..line_end].ends_with('\r'));
            edited.insert_str(
                at,
                &format!("{newline}{}default: {value}", " ".repeat(indent)),
            );
            Some(edited)
        }
        None => {
            let separator = if text.is_empty() || text.ends_with('\n') {
                ""
            } else {
                newline
            };
            Some(format!(
                "{text}{separator}theme:{newline}  default: {value}{newline}"
            ))
        }
    }
}

/// `text` as a YAML scalar: plain if it reads back as the same string,
/// double-quoted otherwise.
fn yaml_scalar(text: &str) -> String {
    let plain = text.starts_with(|c: char| c.is_ascii_alphabetic())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n"
        );
    if plain {
        text.to_owned()
    } else {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Where `theme` and `theme.default` are in a config's text.
#[derive(Default)]
struct Spots {
    theme: Option<ThemeSpot>,
    /// The byte range of `theme.default`'s value.
    default: Option<std::ops::Range<usize>>,
}

enum ThemeSpot {
    /// `theme:` starts a block: the byte offset of the key, and the indent
    /// its keys have (or would have).
    Block { key: usize, indent: usize },
    /// `theme: { … }`, or a `theme` whose place in the text is unknown: left
    /// for the user to edit.
    Inline,
}

/// Finds the [`Spots`] of a document.
struct SpotFinder;

impl<'de> MapReader<'de> for SpotFinder {
    type Value = Spots;
    const EXPECTING: &'static str = "a mapping of settings";

    fn empty(self) -> Spots {
        Spots::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<Spots, A::Error> {
        let mut spots = Spots::default();
        while let Some(key) = map.next_key::<Spanned<String>>()? {
            if key.value != "theme" {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let at = key.referenced;
            let theme = map.next_value_seed(Optional(ThemeSpotFinder))?;
            spots.default = theme.default;
            spots.theme = Some(match (theme.first_key, at.span().byte_offset()) {
                (_, None) => ThemeSpot::Inline,
                (Some((line, _)), _) if line == at.line() => ThemeSpot::Inline,
                (first_key, Some(offset)) => ThemeSpot::Block {
                    key: offset as usize,
                    indent: first_key
                        .map_or(at.column().saturating_sub(1) as usize + 2, |(_, indent)| {
                            indent
                        }),
                },
            });
        }
        Ok(spots)
    }
}

/// What [`SpotFinder`] learns inside `theme:`.
#[derive(Default)]
struct ThemeKeys {
    /// The line of its first key and that key's indent.
    first_key: Option<(u64, usize)>,
    default: Option<std::ops::Range<usize>>,
}

struct ThemeSpotFinder;

impl<'de> MapReader<'de> for ThemeSpotFinder {
    type Value = ThemeKeys;
    const EXPECTING: &'static str = "`default` and `custom`";

    fn empty(self) -> ThemeKeys {
        ThemeKeys::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<ThemeKeys, A::Error> {
        let mut keys = ThemeKeys::default();
        while let Some(key) = map.next_key::<Spanned<String>>()? {
            keys.first_key.get_or_insert((
                key.referenced.line(),
                key.referenced.column().saturating_sub(1) as usize,
            ));
            if key.value != "default" {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let value = map.next_value::<Spanned<String>>()?.referenced.span();
            keys.default = value
                .byte_offset()
                .zip(value.byte_len())
                .map(|(start, len)| start as usize..(start + len) as usize);
        }
        Ok(keys)
    }
}

/// `themes/*.yaml` and sends `()` on `changes` for each (spec 0005,
/// flow 3). Its parent is watched too, so a config directory created while
/// the app runs is picked up, and so is where linked config files point:
/// call [`Watcher::arm`] after each change.
pub struct Watcher {
    notify: RecommendedWatcher,
    dir: PathBuf,
    armed: bool,
    /// What linked config files point to (into a dotfiles repo, say). Edits
    /// land there, not in the config directory.
    links: Arc<Mutex<Vec<PathBuf>>>,
    /// The link targets' directories that are watched.
    followed: BTreeSet<PathBuf>,
}

impl Watcher {
    pub fn start(dir: PathBuf, changes: UnboundedSender<()>) -> Result<Self, String> {
        let links = Arc::new(Mutex::new(Vec::new()));
        let (watched, linked) = (dir.clone(), Arc::clone(&links));
        let mut notify =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let links = linked.lock().map(|links| links.clone()).unwrap_or_default();
                if event.is_ok_and(|event| affects_config(&event, &watched, &links)) {
                    // An error only means the app is shutting down.
                    let _ = changes.unbounded_send(());
                }
            })
            .map_err(|e| e.to_string())?;
        if let Some(parent) = dir.parent() {
            notify
                .watch(parent, RecursiveMode::NonRecursive)
                .map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut watcher = Self {
            notify,
            dir,
            armed: false,
            links,
            followed: BTreeSet::new(),
        };
        watcher.arm();
        Ok(watcher)
    }

    /// Watches the config directory once it exists, and wherever its linked
    /// files point now.
    pub fn arm(&mut self) {
        if !self.armed && self.dir.is_dir() {
            self.armed = self
                .notify
                .watch(&self.dir, RecursiveMode::Recursive)
                .is_ok();
        }
        self.follow_links();
    }

    /// Watches the directories that `config.yaml` and `themes/*.yaml` link
    /// into, where those are outside the config directory (which is
    /// watched already), and stops watching ones no longer linked into.
    fn follow_links(&mut self) {
        let themes = std::fs::read_dir(self.dir.join("themes"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"));
        let targets: Vec<PathBuf> = std::iter::once(self.dir.join("config.yaml"))
            .chain(themes)
            .filter(|path| {
                std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
            })
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .collect();
        let own = std::fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone());
        let dirs: BTreeSet<PathBuf> = targets
            .iter()
            .filter_map(|target| target.parent())
            .filter(|dir| !dir.starts_with(&own))
            .map(Path::to_path_buf)
            .collect();
        for old in self.followed.difference(&dirs) {
            let _ = self.notify.unwatch(old);
        }
        let new: Vec<PathBuf> = dirs.difference(&self.followed).cloned().collect();
        self.followed.retain(|dir| dirs.contains(dir));
        for dir in new {
            if self.notify.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                self.followed.insert(dir);
            }
        }
        if let Ok(mut links) = self.links.lock() {
            *links = targets;
        }
    }
}

/// Whether `event` touches the config directory, `config.yaml`, a theme
/// file or what one of them links to. Reads don't count, or loading the
/// config would trigger a reload.
fn affects_config(event: &notify::Event, dir: &Path, links: &[PathBuf]) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    let themes = dir.join("themes");
    event.paths.iter().any(|path| {
        path == dir
            || path == &dir.join("config.yaml")
            || links.contains(path)
            || (path.parent() == Some(&themes) && path.extension().is_some_and(|ext| ext == "yaml"))
    })
}

/// A part of the document that falls back on its own when it doesn't fit.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Key(String),
    Profile(String),
    Theme(String),
    Keybinding(String),
}

impl Part {
    /// What happens to it, for the notice.
    fn fallback(&self) -> String {
        match self {
            Part::Key(key) => format!("`{key}` uses its default"),
            Part::Profile(id) => format!("profile `{id}` was skipped"),
            Part::Theme(name) => format!("theme `{name}` was skipped"),
            Part::Keybinding(keys) => format!("keybinding `{keys}` was skipped"),
        }
    }
}

/// One read of the document: parts that failed before are skipped, and the
/// part being read is noted, so a failure can be pinned to it.
#[derive(Clone, Copy)]
struct Attempt<'a> {
    skip: &'a [Part],
    reading: &'a RefCell<Option<Part>>,
    /// Problems that don't stop the read, like unknown keys.
    notices: &'a RefCell<Vec<String>>,
}

/// A map key and its line, for notices.
struct Key {
    name: String,
    line: u64,
}

impl Attempt<'_> {
    /// The next key of a map inside `container` (`None`: the document).
    fn key<'de, A: MapAccess<'de>>(
        &self,
        map: &mut A,
        container: Option<Part>,
    ) -> Result<Option<Key>, A::Error> {
        self.reading.replace(container);
        Ok(map.next_key::<Spanned<String>>()?.map(|key| Key {
            line: key.referenced.line(),
            name: key.value,
        }))
    }

    /// Starts reading `part`'s value; `false` if it failed before and is to
    /// be skipped.
    fn enter(&self, part: Part) -> bool {
        let skipped = self.skip.contains(&part);
        self.reading.replace(Some(part));
        !skipped
    }

    fn notice(&self, notice: String) {
        self.notices.borrow_mut().push(notice);
    }

    /// Unknown keys are a notice, never a fallback (spec 0005).
    fn unknown_key(&self, path: &str, line: u64) {
        self.notice(format!(
            "config.yaml{}: unknown key `{path}`, ignored",
            at_line(Some(line))
        ));
    }
}

/// A map read key by key, from a value that may be missing: an empty file,
/// or a key with nothing after it (`font:`), is [`MapReader::empty`].
trait MapReader<'de> {
    type Value;
    /// What a value of the wrong shape should have been, for the notice.
    const EXPECTING: &'static str;
    fn empty(self) -> Self::Value;
    fn read<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error>;
}

/// Reads a [`MapReader`]'s value with serde.
struct Optional<R>(R);

impl<'de, R: MapReader<'de>> DeserializeSeed<'de> for Optional<R> {
    type Value = R::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<R::Value, D::Error> {
        deserializer.deserialize_option(self)
    }
}

impl<'de, R: MapReader<'de>> Visitor<'de> for Optional<R> {
    type Value = R::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(R::EXPECTING)
    }

    fn visit_none<E>(self) -> Result<R::Value, E> {
        Ok(self.0.empty())
    }

    fn visit_unit<E>(self) -> Result<R::Value, E> {
        Ok(self.0.empty())
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<R::Value, D::Error> {
        deserializer.deserialize_map(self)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<R::Value, A::Error> {
        self.0.read(map)
    }
}

/// The document.
impl<'de> MapReader<'de> for Attempt<'_> {
    type Value = Config;
    const EXPECTING: &'static str = "a mapping of settings";

    fn empty(self) -> Config {
        Config::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<Config, A::Error> {
        let mut config = Config::default();
        while let Some(key) = self.key(&mut map, None)? {
            if !self.enter(Part::Key(key.name.clone())) {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            // A key with no value (`font:`) keeps its default.
            match key.name.as_str() {
                "default_profile" => config.default_profile = map.next_value()?,
                "profiles" => {
                    let entries = map.next_value_seed(Optional(Entries::<RawProfile>::new(
                        self,
                        Part::Key(key.name),
                        Part::Profile,
                    )))?;
                    config.profiles = entries
                        .into_iter()
                        .filter_map(|entry| self.profile(entry))
                        .collect();
                }
                "font" => config.font = map.next_value_seed(Optional(FontReader(self)))?,
                "scrollback" => {
                    if let Some(lines) = map.next_value()? {
                        config.scrollback = lines;
                    }
                }
                "close_on_exit" => {
                    if let Some(policy) = map.next_value()? {
                        config.close_on_exit = policy;
                    }
                }
                "theme" => config.theme = map.next_value_seed(Optional(ThemeReader(self)))?,
                "keybindings" => {
                    let entries = map.next_value_seed(Optional(Entries::<String>::new(
                        self,
                        Part::Key(key.name),
                        Part::Keybinding,
                    )))?;
                    config.keybindings = entries
                        .into_iter()
                        .map(|entry| Keybinding {
                            keys: entry.name,
                            command: entry.value,
                            line: entry.line,
                        })
                        .collect();
                }
                "restore_session" => {
                    if let Some(restore) = map.next_value()? {
                        config.restore_session = restore;
                    }
                }
                "copy_on_select" => {
                    if let Some(copy) = map.next_value()? {
                        config.copy_on_select = copy;
                    }
                }
                "cursor" => {
                    config.cursor = map.next_value_seed(Optional(CursorReader(self)))?;
                }
                unknown => {
                    map.next_value::<IgnoredAny>()?;
                    self.unknown_key(unknown, key.line);
                }
            }
        }
        Ok(config)
    }
}

impl Attempt<'_> {
    /// The profile `entry` describes, or `None` (with a notice) if it can't
    /// open yet.
    fn profile(&self, entry: Entry<RawProfile>) -> Option<Profile> {
        let Entry {
            name: id,
            line,
            value: raw,
        } = entry;
        let at = at_line(Some(line));
        if raw.password.is_some() {
            self.notice(format!(
                "config.yaml{at}: the `password` of profile `{id}` was ignored. \
                 Passwords never go in config.yaml; ssh asks for them when it connects."
            ));
        }
        // The other type's keys can't do what they were written for.
        let foreign: Vec<&str> = match raw.kind {
            ProfileType::Local => [
                ("host", raw.host.is_some()),
                ("user", raw.user.is_some()),
                ("port", raw.port.is_some()),
                ("identity_file", raw.identity_file.is_some()),
                ("group", raw.group.is_some()),
            ]
            .into_iter()
            .filter_map(|(key, set)| set.then_some(key))
            .collect(),
            ProfileType::Ssh => [
                ("command", raw.command.is_some()),
                ("cwd", raw.cwd.is_some()),
                ("env", !raw.env.is_empty()),
            ]
            .into_iter()
            .filter_map(|(key, set)| set.then_some(key))
            .collect(),
        };
        if !foreign.is_empty() {
            let kind = match raw.kind {
                ProfileType::Local => "local",
                ProfileType::Ssh => "ssh",
            };
            self.notice(format!(
                "config.yaml{at}: profile `{id}` is `type: {kind}`, so `{}` was ignored",
                foreign.join("`, `")
            ));
        }
        let kind = match raw.kind {
            ProfileType::Local => ProfileKind::Local(LocalProfile {
                command: raw.command.filter(|command| !command.is_empty()),
                cwd: raw.cwd.as_deref().map(expand_home),
                env: raw.env,
            }),
            ProfileType::Ssh => {
                let host = raw.host.map(|host| host.trim().to_owned());
                let Some(host) = host.filter(|host| !host.is_empty() && !host.starts_with('-'))
                else {
                    self.notice(format!(
                        "config.yaml{at}: profile `{id}` was skipped: \
                         an ssh profile needs a `host`, and one can't start with `-`"
                    ));
                    return None;
                };
                ProfileKind::Ssh(SshProfile {
                    host,
                    user: raw.user,
                    port: raw.port,
                    identity_file: raw.identity_file.as_deref().map(expand_home),
                    group: raw.group.filter(|group| !group.trim().is_empty()),
                })
            }
        };
        Some(Profile {
            id,
            name: raw.name,
            theme: raw.theme,
            kind,
        })
    }
}

/// `type:`, which picks how a profile is read.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProfileType {
    Local,
    Ssh,
}

/// A profile as written. Unknown keys make it fail, so a typo like
/// `comand` can't silently run the login shell instead.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    #[serde(rename = "type")]
    kind: ProfileType,
    name: Option<String>,
    theme: Option<String>,
    // `type: local`.
    command: Option<Vec<String>>,
    cwd: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    // `type: ssh` (spec 0006).
    host: Option<String>,
    user: Option<String>,
    port: Option<NonZeroU16>,
    identity_file: Option<String>,
    group: Option<String>,
    /// Only checked for, never kept: secrets belong in the OS keychain.
    password: Option<IgnoredAny>,
}

/// A named value of a map whose values fail one by one.
struct Entry<T> {
    name: String,
    line: u64,
    value: T,
}

/// A map whose values fail one by one: `profiles` and `theme.custom`. A
/// name given twice keeps its last value, in the first one's place.
struct Entries<'a, T> {
    attempt: Attempt<'a>,
    container: Part,
    part: fn(String) -> Part,
    values: PhantomData<T>,
}

impl<'a, T> Entries<'a, T> {
    fn new(attempt: Attempt<'a>, container: Part, part: fn(String) -> Part) -> Self {
        Self {
            attempt,
            container,
            part,
            values: PhantomData,
        }
    }
}

impl<'de, T: Deserialize<'de>> MapReader<'de> for Entries<'_, T> {
    type Value = Vec<Entry<T>>;
    const EXPECTING: &'static str = "a mapping of names";

    fn empty(self) -> Self::Value {
        Vec::new()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries: Vec<Entry<T>> = Vec::new();
        while let Some(key) = self.attempt.key(&mut map, Some(self.container.clone()))? {
            if !self.attempt.enter((self.part)(key.name.clone())) {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let entry = Entry {
                value: map.next_value()?,
                name: key.name,
                line: key.line,
            };
            match entries.iter_mut().find(|known| known.name == entry.name) {
                Some(known) => *known = entry,
                None => entries.push(entry),
            }
        }
        Ok(entries)
    }
}

/// `font:`, read key by key so an unknown key is only a notice.
struct FontReader<'a>(Attempt<'a>);

impl<'de> MapReader<'de> for FontReader<'_> {
    type Value = Font;
    const EXPECTING: &'static str = "`family`, `size` and `line_height`";

    fn empty(self) -> Font {
        Font::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<Font, A::Error> {
        let attempt = self.0;
        let mut font = Font::default();
        while let Some(key) = attempt.key(&mut map, Some(Part::Key("font".into())))? {
            match key.name.as_str() {
                "family" => font.family = map.next_value()?,
                "size" => {
                    if let Some(size) = map.next_value()? {
                        font.size = size;
                    }
                }
                "line_height" => {
                    if let Some(line_height) = map.next_value()? {
                        font.line_height = line_height;
                    }
                }
                unknown => {
                    map.next_value::<IgnoredAny>()?;
                    attempt.unknown_key(&format!("font.{unknown}"), key.line);
                }
            }
        }
        Ok(font)
    }
}

/// `cursor:`, read key by key so an unknown one is a notice (spec 0005).
struct CursorReader<'a>(Attempt<'a>);

impl<'de> MapReader<'de> for CursorReader<'_> {
    type Value = Cursor;
    const EXPECTING: &'static str = "`style` and `blink`";

    fn empty(self) -> Cursor {
        Cursor::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<Cursor, A::Error> {
        let attempt = self.0;
        let mut cursor = Cursor::default();
        while let Some(key) = attempt.key(&mut map, Some(Part::Key("cursor".into())))? {
            match key.name.as_str() {
                "style" => {
                    if let Some(style) = map.next_value()? {
                        cursor.style = style;
                    }
                }
                "blink" => {
                    if let Some(blink) = map.next_value()? {
                        cursor.blink = blink;
                    }
                }
                unknown => {
                    map.next_value::<IgnoredAny>()?;
                    attempt.unknown_key(&format!("cursor.{unknown}"), key.line);
                }
            }
        }
        Ok(cursor)
    }
}

/// `theme:`, read key by key so `custom` can fail theme by theme.
struct ThemeReader<'a>(Attempt<'a>);

impl<'de> MapReader<'de> for ThemeReader<'_> {
    type Value = ThemeSettings;
    const EXPECTING: &'static str = "`default` and `custom`";

    fn empty(self) -> ThemeSettings {
        ThemeSettings::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<ThemeSettings, A::Error> {
        let attempt = self.0;
        let container = Part::Key("theme".into());
        let mut settings = ThemeSettings::default();
        while let Some(key) = attempt.key(&mut map, Some(container.clone()))? {
            match key.name.as_str() {
                "default" => {
                    if let Some(name) = map.next_value()? {
                        settings.default = name;
                    }
                }
                "custom" => {
                    let entries = map.next_value_seed(Optional(Entries::<ThemeSource>::new(
                        attempt,
                        container.clone(),
                        Part::Theme,
                    )))?;
                    settings.custom = entries
                        .into_iter()
                        .map(|entry| (entry.name, entry.value))
                        .collect();
                }
                unknown => {
                    map.next_value::<IgnoredAny>()?;
                    attempt.unknown_key(&format!("theme.{unknown}"), key.line);
                }
            }
        }
        Ok(settings)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terminal::FontSettings;
    use crate::terminal::pty::Command;
    use crate::theme::{self, Color, DARK, Library};
    use futures::channel::mpsc::{UnboundedReceiver, unbounded};
    use std::fs;
    use std::time::{Duration, Instant};

    /// A new, empty directory in the system's temp dir, removed again on
    /// drop, so no test touches the real config directory. `name` must be
    /// unique among all tests.
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("zed-term-test-{name}-{}", std::process::id()));
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
    fn profile_ids(config: &Config) -> Vec<&str> {
        config
            .profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect()
    }

    #[test]
    fn empty_text_is_defaults_without_notice() {
        for text in ["", "   \n\n"] {
            let loaded = parse(text);
            assert_eq!(loaded.config, Config::default());
            assert!(loaded.notices.is_empty(), "notice for {text:?}");
        }
    }

    #[test]
    fn missing_file_is_defaults_without_notice() {
        let path = std::env::temp_dir().join("zed-term-test-does-not-exist/config.yaml");
        let loaded = load(&path);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.notices.is_empty());
    }

    #[test]
    fn invalid_yaml_gives_defaults_and_one_notice_naming_the_line() {
        // Line 3 has an unterminated flow mapping.
        let text = "scrollback: 5\nfont:\n  family: {oops\n";
        let loaded = parse(text);
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.notices.len(), 1);
        assert!(
            loaded.notices[0].contains("line 3"),
            "notice: {}",
            loaded.notices[0]
        );
    }

    #[test]
    fn wrong_type_gives_defaults_and_notice_naming_the_line() {
        let text = "font:\n  size: large\n";
        let loaded = parse(text);
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.notices.len(), 1);
        assert!(
            loaded.notices[0].contains("line 2"),
            "notice: {}",
            loaded.notices[0]
        );
    }

    #[test]
    fn partial_config_keeps_defaults_for_unspecified_keys() {
        let loaded = parse("scrollback: 500\nfont:\n  size: 16\n");
        assert!(loaded.notices.is_empty());
        let defaults = Config::default();
        assert_eq!(loaded.config.scrollback, 500);
        assert_eq!(loaded.config.font.size, 16.0);
        assert_eq!(loaded.config.font.family, defaults.font.family);
        assert_eq!(loaded.config.font.line_height, defaults.font.line_height);
        assert_eq!(loaded.config.close_on_exit, defaults.close_on_exit);
    }

    #[test]
    fn close_on_exit_parses_all_variants() {
        let cases = [
            ("clean", CloseOnExit::Clean),
            ("always", CloseOnExit::Always),
            ("never", CloseOnExit::Never),
        ];
        for (text, expected) in cases {
            let loaded = parse(&format!("close_on_exit: {text}\n"));
            assert!(loaded.notices.is_empty(), "notice for {text}");
            assert_eq!(loaded.config.close_on_exit, expected);
        }
    }

    /// Spec 0003 §Security & privacy, spec 0005 §Model: the last session is
    /// restored unless `restore_session: false` turns that off.
    #[test]
    fn restore_session_is_on_unless_set_to_false() {
        assert!(parse("").config.restore_session);
        for (text, restore) in [
            ("restore_session: true\n", true),
            ("restore_session: false\n", false),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.restore_session, restore, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0002, flow 4: copy-on-select is a config option, off by
    /// default.
    #[test]
    fn copy_on_select_is_off_unless_set_to_true() {
        assert!(!parse("").config.copy_on_select);
        for (text, copy) in [
            ("copy_on_select: true\n", true),
            ("copy_on_select: false\n", false),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.copy_on_select, copy, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: `cursor` is a style (`block`, `beam` or `underline`) and
    /// whether it blinks, by default a blinking block. A key it leaves out
    /// keeps its default.
    #[test]
    fn the_cursor_takes_each_style_and_blink_and_a_partial_map_keeps_the_defaults() {
        use CursorStyle::{Beam, Block, Underline};
        let default = Cursor {
            style: Block,
            blink: true,
        };
        assert_eq!(parse("").config.cursor, default);
        let cases = [
            ("{ style: beam, blink: false }", Beam, false),
            ("{ style: underline, blink: true }", Underline, true),
            ("{ style: block, blink: false }", Block, false),
            ("{ style: underline }", Underline, true),
            ("{ blink: false }", Block, false),
        ];
        for (cursor, style, blink) in cases {
            let text = format!("cursor: {cursor}\n");
            let Loaded { config, notices } = parse(&text);
            assert_eq!(config.cursor, Cursor { style, blink }, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value that doesn't fit resets its top-level key, here
    /// all of `cursor`, with a notice naming it; the rest still applies.
    #[test]
    fn a_bad_cursor_style_resets_the_cursor_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "cursor:\n",
            "  style: bar\n",
            "  blink: false\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.cursor, Cursor::default());
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("cursor"), "{}", notices[0]);
    }

    /// Spec 0005: "Unknown keys produce a notice, not an error", in every
    /// settings map, so `shape: beam` (alacritty's name for the style)
    /// doesn't silently do nothing. The rest of `cursor` still applies.
    #[test]
    fn an_unknown_key_inside_cursor_is_a_notice_and_the_rest_of_it_applies() {
        let Loaded { config, notices } = parse("cursor:\n  shape: beam\n  blink: false\n");
        let expected = Cursor {
            blink: false,
            ..Cursor::default()
        };
        assert_eq!(config.cursor, expected);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("cursor.shape") && notices[0].contains("line 2"),
            "{}",
            notices[0]
        );
    }

    /// AC1 (spec 0005).
    #[test]
    fn no_config_file_boots_the_login_shell_on_dark_with_the_bundled_font() {
        let root = TempDir::new("no-config");
        let dir = root.path().join("zed-term");
        let Loaded { config, notices } = load(&dir.join("config.yaml"));
        assert!(notices.is_empty(), "{notices:?}");

        let profile = config.default_profile();
        assert_eq!(Command::of_profile(&profile), Command::default_shell());
        assert_eq!(profile.theme, None, "the shell follows theme.default");
        let mut notices = Vec::new();
        let themes = dir.join("themes");
        let library = Library::new(&config.theme.custom, Some(themes.as_path()));
        let name = theme::effective(&config.theme.default, false);
        assert_eq!(library.resolve(name, &mut notices), DARK);
        assert!(notices.is_empty(), "{notices:?}");
        // No family configured: nothing needs to be installed.
        let (font, notice) = FontSettings::checked_with(&config.font, |_| false);
        assert_eq!((font.family.as_ref(), notice), ("Lilex", None));

        assert!(!dir.exists(), "{} was created", dir.display());
    }

    /// AC2 (spec 0005): what "Open config file" writes changes nothing.
    #[test]
    fn the_template_parses_back_to_exactly_the_defaults() {
        let loaded = parse(TEMPLATE);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
    }

    #[test]
    fn ensure_exists_writes_the_template_and_the_directories_above_it() {
        let root = TempDir::new("ensure-new");
        let path = root.path().join("config/zed-term/config.yaml");
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

    /// The template with `theme.default` changed from `dark` to `name`.
    fn template_with_theme(name: &str) -> String {
        TEMPLATE.replacen("  default: dark ", &format!("  default: {name} "), 1)
    }

    /// AC8 (spec 0005): choosing a theme changes only the value of
    /// `theme.default`; comments, spacing and every other key stay.
    #[test]
    fn choosing_a_theme_changes_only_the_value_of_theme_default() {
        let edited = with_theme_default(TEMPLATE, "light").expect("edit the template");
        assert_eq!(edited, template_with_theme("light"));
        let expected = Config {
            theme: ThemeSettings {
                default: "light".into(),
                ..ThemeSettings::default()
            },
            ..Config::default()
        };
        assert_eq!(
            parse(&edited),
            Loaded {
                config: expected,
                notices: vec![]
            }
        );
    }

    #[test]
    fn an_existing_theme_default_has_only_its_value_replaced() {
        let cases = [
            (
                "theme:\n  default: dark  # mine\n  custom: {}\n",
                "theme:\n  default: light  # mine\n  custom: {}\n",
            ),
            ("theme: { default: dark }\n", "theme: { default: light }\n"),
            // Positions count bytes, not characters.
            (
                "# Farben für später → dunkel\ntheme:\n  default: dark\n",
                "# Farben für später → dunkel\ntheme:\n  default: light\n",
            ),
            // Notepad's line endings.
            (
                "theme:\r\n  default: dark\r\n",
                "theme:\r\n  default: light\r\n",
            ),
        ];
        for (text, expected) in cases {
            let edited = with_theme_default(text, "light");
            assert_eq!(edited.as_deref(), Some(expected), "{text:?}");
        }
    }

    /// A quoted value is replaced whole, so the new name reads back
    /// however it's written.
    #[test]
    fn a_quoted_theme_default_is_replaced_whole() {
        for text in [
            "theme:\n  default: \"dark\"\n",
            "theme:\n  default: 'dark'\n",
        ] {
            for name in ["light", "my theme"] {
                let edited = with_theme_default(text, name).expect(text);
                assert_eq!(parse(&edited).config.theme.default, name, "{edited:?}");
            }
        }
    }

    /// The new line goes first in the block, at the block's own indent.
    #[test]
    fn a_theme_block_without_default_gets_it_as_its_first_key() {
        let cases = [
            (
                concat!(
                    "theme:  # colors\n",
                    "    custom:\n",
                    "      paper:\n",
                    "        base: light\n",
                    "scrollback: 500\n",
                ),
                concat!(
                    "theme:  # colors\n",
                    "    default: paper\n",
                    "    custom:\n",
                    "      paper:\n",
                    "        base: light\n",
                    "scrollback: 500\n",
                ),
            ),
            // `theme:` with nothing under it.
            (
                "theme:\nscrollback: 500\n",
                "theme:\n  default: paper\nscrollback: 500\n",
            ),
            // The inserted line ends like the others.
            (
                "scrollback: 500\r\ntheme:\r\n  custom: {}\r\n",
                "scrollback: 500\r\ntheme:\r\n  default: paper\r\n  custom: {}\r\n",
            ),
        ];
        for (text, expected) in cases {
            let edited = with_theme_default(text, "paper").expect(text);
            assert_eq!(edited, expected);
            assert_eq!(parse(&edited).config.theme.default, "paper", "{edited:?}");
        }
    }

    /// The rest of the text stays as it was. A profile's `theme` is not
    /// the `theme` block.
    #[test]
    fn a_config_without_a_theme_block_gets_one_at_the_end() {
        let texts = [
            "",
            "scrollback: 500",
            "# mine\nscrollback: 500\n",
            "profiles:\n  htop:\n    type: local\n    theme: light\n",
        ];
        for text in texts {
            let edited = with_theme_default(text, "paper").expect(text);
            assert!(edited.starts_with(text), "{edited:?}");
            let mut expected = parse(text).config;
            expected.theme.default = "paper".into();
            assert_eq!(parse(&edited).config, expected, "{edited:?}");
        }
    }

    #[test]
    fn a_theme_block_added_to_a_crlf_config_ends_its_lines_in_crlf() {
        let edited = with_theme_default("scrollback: 500\r\n", "paper").expect("edit");
        assert!(!edited.replace("\r\n", "").contains('\n'), "{edited:?}");
    }

    /// Notepad can start the file with a byte order mark.
    #[test]
    fn a_config_with_a_byte_order_mark_is_edited_in_the_right_place() {
        let cases = [
            (
                "\u{feff}theme:\n  default: dark\n",
                "\u{feff}theme:\n  default: light\n",
            ),
            (
                "\u{feff}theme:\n  custom: {}\n",
                "\u{feff}theme:\n  default: light\n  custom: {}\n",
            ),
        ];
        for (text, expected) in cases {
            let edited = with_theme_default(text, "light");
            assert_eq!(edited.as_deref(), Some(expected), "{text:?}");
        }
    }

    /// Names plain YAML would read as something else (a boolean, null, a
    /// number, a comment, a mapping, ...) are quoted.
    #[test]
    fn a_chosen_theme_name_reads_back_exactly() {
        let names = [
            "yes",
            "Off",
            "true",
            "null",
            "~",
            "1.5",
            "0x1f",
            "#1e1e2e",
            "a: b",
            "[x]",
            "*x",
            "\"x\"",
            "back\\slash",
            " padded ",
            "Ünïcode",
        ];
        for text in ["", "theme:\n  default: dark\n"] {
            for name in names {
                let edited = with_theme_default(text, name).expect(name);
                assert_eq!(parse(&edited).config.theme.default, name, "{edited:?}");
            }
        }
    }

    /// Flow 4 (spec 0005) edits one value in place; adding a key to a
    /// `theme: { … }` written inline isn't that, so it's left to the user.
    #[test]
    fn an_inline_theme_without_default_is_left_alone() {
        let text = "theme: { custom: { paper: { base: light } } }\n";
        assert_eq!(with_theme_default(text, "paper"), None);
    }

    /// Whatever `theme` holds, the picker either sets the theme or changes
    /// nothing: a config that no longer parses would lose every setting.
    #[test]
    fn choosing_a_theme_never_leaves_a_config_that_doesnt_parse() {
        for text in ["theme: {}\n", "theme: {\n  custom: {}\n}\n", "theme: ~\n"] {
            if let Some(edited) = with_theme_default(text, "paper") {
                let loaded = parse(&edited);
                assert_eq!(
                    loaded.config.theme.default, "paper",
                    "{edited:?}: {:?}",
                    loaded.notices
                );
            }
        }
    }

    /// Choosing a theme before there is a config (spec 0005, flow 4): the
    /// config is the template with that theme, and nothing else is written.
    #[test]
    fn saving_a_theme_without_a_config_writes_the_template_with_it() {
        let root = TempDir::new("save-theme-new");
        let dir = root.path().join("zed-term");
        let path = dir.join("config.yaml");
        save_theme_default(&path, "light").expect("save the theme");
        let text = fs::read_to_string(&path).expect("read config.yaml");
        assert_eq!(text, template_with_theme("light"));
        let files: Vec<String> = fs::read_dir(&dir)
            .expect("list the config dir")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(files, ["config.yaml"]);
    }

    /// Rather than rewrite what it can't edit in place, the picker leaves
    /// the file as it is.
    #[test]
    fn saving_a_theme_into_a_config_it_cant_edit_leaves_the_file_alone() {
        let root = TempDir::new("save-theme-cant-edit");
        let path = root.path().join("config.yaml");
        for text in ["theme: { custom: {} }\n", "theme: [oops\n"] {
            fs::write(&path, text).expect("write config.yaml");
            assert!(save_theme_default(&path, "light").is_err(), "{text:?}");
            assert_eq!(fs::read_to_string(&path).expect("read it back"), text);
        }
    }

    /// Dotfile managers link config.yaml into a repository: the theme is
    /// saved there, and the link stays a link.
    #[cfg(unix)]
    #[test]
    fn saving_a_theme_into_a_linked_config_edits_the_target_and_keeps_the_link() {
        let root = TempDir::new("save-theme-linked");
        let dotfiles = root.path().join("dotfiles");
        let dir = root.path().join("zed-term");
        fs::create_dir(&dotfiles).expect("create the dotfiles dir");
        fs::create_dir(&dir).expect("create the config dir");
        let target = dotfiles.join("config.yaml");
        let link = dir.join("config.yaml");
        fs::write(&target, "theme:\n  default: dark\n").expect("write the config");
        std::os::unix::fs::symlink(&target, &link).expect("link config.yaml");

        save_theme_default(&link, "light").expect("save the theme");
        let meta = fs::symlink_metadata(&link).expect("stat the link");
        assert!(meta.file_type().is_symlink(), "config.yaml was replaced");
        assert_eq!(fs::read_link(&link).expect("read the link"), target);
        assert_eq!(
            fs::read_to_string(&target).expect("read the config"),
            "theme:\n  default: light\n"
        );
    }

    /// A config kept private stays private.
    #[cfg(unix)]
    #[test]
    fn saving_a_theme_keeps_the_configs_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = TempDir::new("save-theme-mode");
        let path = root.path().join("config.yaml");
        fs::write(&path, "theme:\n  default: dark\n").expect("write config.yaml");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod 600");
        save_theme_default(&path, "light").expect("save the theme");
        let mode = fs::metadata(&path)
            .expect("stat config.yaml")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// The example of spec 0005 §Model, as written there.
    const SPEC_EXAMPLE: &str = r##"
default_profile: shell              # missing → shell, the login shell

profiles:                           # map key = stable profile ID
  shell:
    type: local                     # no command → the user's login shell
    env: { EDITOR: nvim }
  htop:
    name: Htop                      # display name; defaults to the ID
    type: local
    command: [htop]                 # program + args
    cwd: ~/projects                 # optional, default: home
  my-server:                        # spec 0006
    type: ssh
    host: myserver.example.com      # or a Host alias from ~/.ssh/config
    user: myuser                    # optional
    port: 22                        # optional
    identity_file: ~/.ssh/id_ed25519 # optional; omit to let ssh choose
    group: Production               # optional; host picker group, default "Hosts"
    theme: midnight

font: { family: null, size: 13, line_height: 1.2 }   # null → the bundled font
scrollback: 10000
cursor: { style: block, blink: true }
copy_on_select: false
close_on_exit: clean                # always | clean | never
restore_session: true
keybindings: { "ctrl+shift+d": pane.splitRight }

theme:
  default: dark                     # dark | light | system | any theme name
  custom:                           # inline themes
    midnight:
      base: dark                    # missing colors come from here (default: dark)
      background: "#1e1e2e"
      foreground: "#cdd6f4"
      cursor: "#f5c2e7"
"##;

    /// Its `cursor` and `copy_on_select` spell out the defaults.
    #[test]
    fn the_spec_example_loads_every_profile_without_a_notice() {
        let Loaded { config, notices } = parse(SPEC_EXAMPLE);
        let home = dirs::home_dir().expect("home dir");
        let midnight = ThemeSource {
            base: Some("dark".into()),
            background: Some(Color(0x1e1e2e)),
            foreground: Some(Color(0xcdd6f4)),
            cursor: Some(Color(0xf5c2e7)),
            ..ThemeSource::default()
        };
        let expected = Config {
            default_profile: Some("shell".into()),
            profiles: vec![
                Profile {
                    kind: ProfileKind::Local(LocalProfile {
                        env: BTreeMap::from([("EDITOR".into(), "nvim".into())]),
                        ..LocalProfile::default()
                    }),
                    ..Profile::shell()
                },
                Profile {
                    id: "htop".into(),
                    name: Some("Htop".into()),
                    kind: ProfileKind::Local(LocalProfile {
                        command: Some(vec!["htop".into()]),
                        cwd: Some(home.join("projects")),
                        ..LocalProfile::default()
                    }),
                    ..Profile::shell()
                },
                Profile {
                    id: "my-server".into(),
                    theme: Some("midnight".into()),
                    kind: ProfileKind::Ssh(SshProfile {
                        host: "myserver.example.com".into(),
                        user: Some("myuser".into()),
                        port: NonZeroU16::new(22),
                        identity_file: Some(home.join(".ssh/id_ed25519")),
                        group: Some("Production".into()),
                    }),
                    ..Profile::shell()
                },
            ],
            theme: ThemeSettings {
                default: "dark".into(),
                custom: vec![("midnight".into(), midnight)],
            },
            keybindings: vec![binding("ctrl+shift+d", "pane.splitRight", 28)],
            ..Config::default()
        };
        assert_eq!(config, expected);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// A `keybindings:` entry as [`parse`] reads it.
    fn binding(keys: &str, command: &str, line: u64) -> Keybinding {
        Keybinding {
            keys: keys.into(),
            command: command.into(),
            line,
        }
    }

    /// Spec 0004: bindings apply in the order they're written, and each
    /// knows its line for the notice if it can't be bound.
    #[test]
    fn keybindings_load_in_file_order_with_their_lines() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "keybindings:\n",
            "  \"ctrl+shift+n\": tab.new\n",
            "  \"alt+shift+d\": pane.splitRight\n",
            "  ctrl+shift+a: font.increase\n",
        ));
        let expected = [
            binding("ctrl+shift+n", "tab.new", 3),
            binding("alt+shift+d", "pane.splitRight", 4),
            binding("ctrl+shift+a", "font.increase", 5),
        ];
        assert_eq!(config.keybindings, expected);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Like a profile, a keybinding that doesn't fit is skipped on its own
    /// (spec 0004, flow 4).
    #[test]
    fn a_keybinding_whose_command_isnt_text_is_skipped_alone_with_a_notice() {
        let Loaded { config, notices } = parse(concat!(
            "keybindings:\n",
            "  \"ctrl+shift+n\": tab.new\n",
            "  \"alt+shift+d\": [pane.splitRight]\n",
            "  \"ctrl+shift+m\": tab.rename\n",
            "scrollback: 500\n",
        ));
        let expected = [
            binding("ctrl+shift+n", "tab.new", 2),
            binding("ctrl+shift+m", "tab.rename", 4),
        ];
        assert_eq!(config.keybindings, expected);
        assert_eq!(config.scrollback, 500);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("alt+shift+d") && notices[0].contains("line 3"),
            "{}",
            notices[0]
        );
    }

    /// AC4 (spec 0005).
    #[test]
    fn a_profile_with_command_42_is_skipped_and_the_others_load_in_file_order() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  zeta:\n",
            "    type: local\n",
            "    command: [htop]\n",
            "  broken:\n",
            "    type: local\n",
            "    command: 42\n",
            "  alpha:\n",
            "    type: local\n",
        ));
        let ids: Vec<_> = config.profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["zeta", "alpha"]);
        let ProfileKind::Local(zeta) = &config.profiles[0].kind else {
            panic!("zeta is local");
        };
        assert_eq!(zeta.command, Some(vec!["htop".to_owned()]));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("broken") && notices[0].contains("line 7"),
            "{}",
            notices[0]
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

    #[test]
    fn an_unknown_top_level_key_is_a_notice_naming_its_line_not_a_fallback() {
        let loaded = parse("scrollback: 500\ncolour_scheme: solarized\nclose_on_exit: never\n");
        assert_eq!(loaded.config.scrollback, 500);
        assert_eq!(loaded.config.close_on_exit, CloseOnExit::Never);
        assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
        let notice = &loaded.notices[0];
        assert!(
            notice.contains("colour_scheme") && notice.contains("line 2"),
            "{notice}"
        );
    }

    /// A value of the wrong type resets its whole top-level key, `family`
    /// included; the keys before and after it still apply.
    #[test]
    fn a_key_that_doesnt_fit_falls_back_alone_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "font:\n",
            "  family: Fira Code\n",
            "  size: large\n",
            "close_on_exit: never\n",
        ));
        assert_eq!(config.font, Font::default());
        assert_eq!(config.scrollback, 500);
        assert_eq!(config.close_on_exit, CloseOnExit::Never);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("font") && notices[0].contains("line 4"),
            "{}",
            notices[0]
        );
    }

    /// Unknown keys are a notice, never a fallback, in every settings map
    /// (spec 0005: "Unknown keys produce a notice, not an error"). The keys
    /// after one still apply.
    #[test]
    fn an_unknown_key_inside_font_or_theme_is_a_notice_and_the_rest_of_it_applies() {
        let Loaded { config, notices } = parse(concat!(
            "font:\n",
            "  weight: bold\n",
            "  size: 16\n",
            "theme:\n",
            "  colour: red\n",
            "  default: light\n",
            "scrollback: 500\n",
        ));
        let expected = Config {
            font: Font {
                size: 16.0,
                ..Font::default()
            },
            theme: ThemeSettings {
                default: "light".into(),
                ..ThemeSettings::default()
            },
            scrollback: 500,
            ..Config::default()
        };
        assert_eq!(config, expected);
        assert_eq!(notices.len(), 2, "{notices:?}");
        for (path, line) in [("font.weight", "line 2"), ("theme.colour", "line 5")] {
            assert!(
                notices
                    .iter()
                    .any(|notice| notice.contains(path) && notice.contains(line)),
                "no notice for `{path}` on {line}: {notices:?}"
            );
        }
    }

    /// Unlike settings, a profile or theme with a typo like `comand` is
    /// skipped: running it anyway would quietly change what runs or shows.
    #[test]
    fn an_unknown_key_skips_its_profile_or_inline_theme_with_a_notice() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  htop:\n",
            "    type: local\n",
            "    comand: [htop]\n",
            "  shell:\n",
            "    type: local\n",
            "theme:\n",
            "  custom:\n",
            "    midnight:\n",
            "      backgroud: \"#1e1e2e\"\n",
            "    paper:\n",
            "      base: light\n",
        ));
        let ids: Vec<_> = config.profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["shell"]);
        let names: Vec<_> = config
            .theme
            .custom
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(names, ["paper"]);
        assert_eq!(notices.len(), 2, "{notices:?}");
        for (part, line) in [("htop", "line 4"), ("midnight", "line 10")] {
            assert!(
                notices
                    .iter()
                    .any(|notice| notice.contains(part) && notice.contains(line)),
                "no notice for `{part}` on {line}: {notices:?}"
            );
        }
    }

    #[test]
    fn a_bad_inline_theme_is_skipped_alone() {
        let Loaded { config, notices } = parse(concat!(
            "theme:\n",
            "  default: paper\n",
            "  custom:\n",
            "    midnight:\n",
            "      background: red\n",
            "    paper:\n",
            "      base: light\n",
            "      background: \"#ffffff\"\n",
        ));
        assert_eq!(config.theme.default, "paper");
        let names: Vec<_> = config
            .theme
            .custom
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(names, ["paper"]);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("midnight") && notices[0].contains("line 5"),
            "{}",
            notices[0]
        );
    }

    /// The parts that don't fit are found one read at a time; every one of
    /// them must stay skipped while the rest is read again.
    #[test]
    fn several_parts_that_dont_fit_each_fall_back_with_their_own_notice() {
        let Loaded { config, notices } = parse(concat!(
            "font:\n",
            "  size: large\n",
            "profiles:\n",
            "  broken:\n",
            "    type: local\n",
            "    command: 42\n",
            "  htop:\n",
            "    type: local\n",
            "    command: [htop]\n",
            "scrollback: 500\n",
        ));
        assert_eq!(config.font, Font::default());
        let ids: Vec<_> = config.profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["htop"]);
        assert_eq!(config.scrollback, 500);
        assert_eq!(notices.len(), 2, "{notices:?}");
    }

    /// Secrets never go in config.yaml (spec 0005 §Security & privacy).
    #[test]
    fn a_password_in_a_profile_gives_a_notice_and_is_kept_nowhere() {
        let loaded = parse(concat!(
            "profiles:\n",
            "  db:\n",
            "    type: local\n",
            "    command: [psql]\n",
            "    password: hunter2\n",
        ));
        assert!(
            loaded
                .notices
                .iter()
                .any(|notice| notice.contains("password") && notice.contains("db")),
            "{:?}",
            loaded.notices
        );
        assert!(!format!("{loaded:?}").contains("hunter2"), "{loaded:?}");
    }

    /// Options a profile leaves out stay unset, so `~/.ssh/config` decides
    /// them (spec 0006 §Model).
    #[test]
    fn an_ssh_profile_with_only_a_host_leaves_every_option_unset() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod-alias\n",
        ));
        let expected = Profile {
            id: "prod".into(),
            kind: ProfileKind::Ssh(ssh_to("prod-alias")),
            ..Profile::shell()
        };
        assert_eq!(config.profiles, [expected]);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// An empty `group` puts the host in the default group, "Hosts" (spec
    /// 0005 §Model).
    #[test]
    fn an_empty_group_counts_as_none() {
        for group in ["", "\"\"", "\"   \""] {
            let Loaded { config, notices } = parse(&format!(
                "profiles:\n  prod:\n    type: ssh\n    host: prod.example.com\n    group: {group}\n"
            ));
            let kinds: Vec<_> = config.profiles.iter().map(|p| &p.kind).collect();
            let ungrouped = ProfileKind::Ssh(ssh_to("prod.example.com"));
            assert_eq!(kinds, [&ungrouped], "group: {group}");
            assert!(notices.is_empty(), "group: {group}: {notices:?}");
        }
    }

    /// The palette lists profiles in file order (spec 0005), whatever their
    /// type.
    #[test]
    fn ssh_and_local_profiles_load_together_in_file_order() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  zeta:\n",
            "    type: local\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
            "  alpha:\n",
            "    type: local\n",
            "  db:\n",
            "    type: ssh\n",
            "    host: db.example.com\n",
        ));
        let loaded: Vec<_> = config
            .profiles
            .iter()
            .map(|p| (p.id.as_str(), matches!(p.kind, ProfileKind::Ssh(_))))
            .collect();
        let expected = [
            ("zeta", false),
            ("prod", true),
            ("alpha", false),
            ("db", true),
        ];
        assert_eq!(loaded, expected);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// New tabs open an SSH profile only when `default_profile` names it;
    /// at boot it then waits for Enter (spec 0006, flow 6).
    #[test]
    fn default_profile_can_name_an_ssh_profile() {
        let Loaded { config, notices } = parse(concat!(
            "default_profile: prod\n",
            "profiles:\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
        ));
        assert_eq!(config.default_profile().id, "prod");
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Without a host there is nothing to connect to.
    #[test]
    fn an_ssh_profile_without_a_host_is_skipped_with_a_notice_naming_it() {
        // No `host` key, one with no value, an empty one.
        for host in ["", "    host:\n", "    host: \"\"\n"] {
            let Loaded { config, notices } = parse(&format!(
                concat!(
                    "profiles:\n",
                    "  shell:\n",
                    "    type: local\n",
                    "  nowhere:\n",
                    "    type: ssh\n",
                    "    user: alice\n",
                    "{}",
                    "  prod:\n",
                    "    type: ssh\n",
                    "    host: prod.example.com\n",
                ),
                host
            ));
            assert_eq!(profile_ids(&config), ["shell", "prod"], "{host:?}");
            assert_eq!(notices.len(), 1, "{host:?}: {notices:?}");
            assert!(notices[0].contains("nowhere"), "{}", notices[0]);
        }
    }

    /// ssh reads an argument that starts with `-` as an option, so such a
    /// host could set any option, `ProxyCommand` included.
    #[test]
    fn an_ssh_host_that_starts_with_a_dash_skips_its_profile_with_a_notice_naming_it() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  shell:\n",
            "    type: local\n",
            "  sneaky:\n",
            "    type: ssh\n",
            "    host: \"-oProxyCommand=echo pwned\"\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
        ));
        assert_eq!(profile_ids(&config), ["shell", "prod"]);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("sneaky"), "{}", notices[0]);
    }

    /// Whitespace in front can't sneak a dash past the check: whatever is
    /// done to the host on its way, the argument ssh gets never starts
    /// with `-`.
    #[test]
    fn whitespace_before_a_dash_never_gets_a_host_to_ssh_as_an_option() {
        for host in [" -oProxyCommand=echo pwned", "\\t-oProxyCommand=echo pwned"] {
            let Loaded { config, notices } = parse(&format!(
                "profiles:\n  sneaky:\n    type: ssh\n    host: \"{host}\"\n"
            ));
            match config.profiles.as_slice() {
                [profile] => {
                    let args = Command::of_profile(profile).args;
                    let target = args.last().expect("ssh gets a host");
                    assert!(!target.starts_with('-'), "{host:?} reached ssh as {args:?}");
                }
                _ => assert!(
                    notices.iter().any(|notice| notice.contains("sneaky")),
                    "{host:?} was skipped without a notice: {notices:?}"
                ),
            }
        }
    }

    #[test]
    fn an_ssh_port_from_1_to_65535_is_kept() {
        for port in [1, 22, 65535] {
            let Loaded { config, notices } = parse(&format!(
                "profiles:\n  prod:\n    type: ssh\n    host: prod.example.com\n    port: {port}\n"
            ));
            let expected = ProfileKind::Ssh(SshProfile {
                port: NonZeroU16::new(port),
                ..ssh_to("prod.example.com")
            });
            let kinds: Vec<_> = config.profiles.iter().map(|p| &p.kind).collect();
            assert_eq!(kinds, [&expected], "port {port}");
            assert!(notices.is_empty(), "port {port}: {notices:?}");
        }
    }

    /// OpenSSH refuses port 0, and its exit 255 would look like a dropped
    /// connection. A profile that doesn't fit is skipped on its own (spec
    /// 0005).
    #[test]
    fn an_ssh_port_that_isnt_a_number_from_1_to_65535_skips_its_profile_with_a_notice() {
        for port in ["0", "65536", "-1", "twenty-two"] {
            let Loaded { config, notices } = parse(&format!(
                concat!(
                    "profiles:\n",
                    "  shell:\n",
                    "    type: local\n",
                    "  bad-port:\n",
                    "    type: ssh\n",
                    "    host: prod.example.com\n",
                    "    port: {}\n",
                    "  db:\n",
                    "    type: ssh\n",
                    "    host: db.example.com\n",
                ),
                port
            ));
            assert_eq!(profile_ids(&config), ["shell", "db"], "port {port}");
            assert_eq!(notices.len(), 1, "port {port}: {notices:?}");
            assert!(notices[0].contains("bad-port"), "{}", notices[0]);
        }
    }

    /// Spec 0006 §Security: no password in config.yaml, least of all on
    /// the profiles that log in somewhere.
    #[test]
    fn a_password_on_an_ssh_profile_gives_a_notice_and_is_kept_nowhere() {
        let loaded = parse(concat!(
            "profiles:\n",
            "  prod-db:\n",
            "    type: ssh\n",
            "    host: db.example.com\n",
            "    password: hunter2\n",
        ));
        assert!(
            loaded
                .notices
                .iter()
                .any(|notice| notice.contains("password") && notice.contains("prod-db")),
            "{:?}",
            loaded.notices
        );
        assert!(!format!("{loaded:?}").contains("hunter2"), "{loaded:?}");
    }

    /// A profile whose `type` changed keeps loading; what it no longer
    /// uses is named, once, and has no effect.
    #[test]
    fn ssh_keys_on_a_local_profile_give_one_notice_naming_them_and_are_ignored() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  htop:\n",
            "    type: local\n",
            "    command: [htop]\n",
            "    host: prod.example.com\n",
            "    user: alice\n",
            "    port: 2222\n",
            "    identity_file: ~/.ssh/id_ed25519\n",
            "    group: Tools\n",
        ));
        assert_eq!(profile_ids(&config), ["htop"], "{notices:?}");
        let runs = Command {
            program: "htop".into(),
            args: vec![],
            env: BTreeMap::new(),
        };
        assert_eq!(Command::of_profile(&config.profiles[0]), runs);
        assert_eq!(notices.len(), 1, "{notices:?}");
        for name in ["htop", "host", "user", "port", "identity_file", "group"] {
            assert!(notices[0].contains(name), "no `{name}`: {}", notices[0]);
        }
    }

    #[test]
    fn local_keys_on_an_ssh_profile_give_one_notice_naming_them_and_are_ignored() {
        let Loaded { config, notices } = parse(concat!(
            "profiles:\n",
            "  prod:\n",
            "    type: ssh\n",
            "    host: prod.example.com\n",
            "    command: [htop]\n",
            "    cwd: ~/projects\n",
            "    env: { EDITOR: nvim }\n",
        ));
        assert_eq!(profile_ids(&config), ["prod"], "{notices:?}");
        let profile = &config.profiles[0];
        let runs = Command {
            program: "ssh".into(),
            args: vec!["prod.example.com".into()],
            env: BTreeMap::new(),
        };
        assert_eq!(Command::of_profile(profile), runs);
        assert_eq!(profile.cwd(), None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        for name in ["prod", "command", "cwd", "env"] {
            assert!(notices[0].contains(name), "no `{name}`: {}", notices[0]);
        }
    }

    /// A known limitation: both types' keys are read as what they hold
    /// before `type` decides which apply, so a key of the other type whose
    /// value doesn't fit that key makes the profile not fit. Like any such
    /// profile, it's skipped on its own with a notice (spec 0005); not
    /// reading it at all would take a hand-written profile parser.
    #[test]
    fn a_key_of_the_other_type_that_doesnt_fit_skips_its_profile_with_a_notice() {
        let cases = [
            (
                "htop",
                "    type: local\n    command: [htop]\n    port: 70000\n",
            ),
            (
                "prod",
                "    type: ssh\n    host: prod.example.com\n    command: htop\n",
            ),
        ];
        for (id, keys) in cases {
            let Loaded { config, notices } = parse(&format!(
                "profiles:\n  {id}:\n{keys}  shell:\n    type: local\n"
            ));
            assert_eq!(profile_ids(&config), ["shell"], "{id}: {notices:?}");
            assert_eq!(notices.len(), 1, "{id}: {notices:?}");
            assert!(notices[0].contains(id), "{}", notices[0]);
        }
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

    /// Notepad saves with CRLF line endings, and can add a byte order mark.
    #[test]
    fn a_config_saved_on_windows_loads_the_same() {
        let loaded = parse("\u{feff}scrollback: 500\r\nfont:\r\n  size: 16\r\n");
        assert_eq!(loaded.config.scrollback, 500);
        assert_eq!(loaded.config.font.size, 16.0);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
    }

    /// How long a change may take to be reported (spec 0005, AC7).
    const WITHIN: Duration = Duration::from_secs(1);
    /// How long a watcher has to stay quiet to count as not reporting.
    const QUIET: Duration = Duration::from_millis(500);

    /// A watcher on `dir` and the end of the channel it reports on.
    fn watch(dir: &Path) -> (Watcher, UnboundedReceiver<()>) {
        let (changes, reports) = unbounded();
        let watcher = Watcher::start(dir.to_path_buf(), changes).expect("start the watcher");
        (watcher, reports)
    }

    /// Whether a change is reported within `wait`.
    fn reported(reports: &mut UnboundedReceiver<()>, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            if reports.try_recv().is_ok() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Drops the rest of one change's reports: a save is several file
    /// system events.
    fn settle(reports: &mut UnboundedReceiver<()>) {
        while reported(reports, Duration::from_millis(200)) {}
    }

    /// AC7 (spec 0005), the watcher's part, for each way editors save.
    #[test]
    fn saving_config_yaml_is_reported_within_a_second() {
        let root = TempDir::new("watch-config");
        let dir = root.path().join("zed-term");
        fs::create_dir(&dir).expect("create the config dir");
        let (_watcher, mut reports) = watch(&dir);
        let config = dir.join("config.yaml");

        fs::write(&config, "scrollback: 1\n").expect("create config.yaml");
        assert!(reported(&mut reports, WITHIN), "creating config.yaml");
        settle(&mut reports);
        fs::write(&config, "scrollback: 2\n").expect("write config.yaml");
        assert!(reported(&mut reports, WITHIN), "writing config.yaml");
        settle(&mut reports);
        // Write a new version next to it, then rename it over the old one.
        let saved = dir.join("config.yaml.tmp");
        fs::write(&saved, "scrollback: 3\n").expect("write the new version");
        settle(&mut reports);
        fs::rename(&saved, &config).expect("replace config.yaml");
        assert!(reported(&mut reports, WITHIN), "replacing config.yaml");
    }

    #[test]
    fn changing_a_theme_file_is_reported_within_a_second() {
        let root = TempDir::new("watch-themes");
        let dir = root.path().join("zed-term");
        let themes = dir.join("themes");
        fs::create_dir_all(&themes).expect("create the themes dir");
        let (_watcher, mut reports) = watch(&dir);
        let theme = themes.join("midnight.yaml");

        fs::write(&theme, "background: \"#000001\"\n").expect("create a theme");
        assert!(reported(&mut reports, WITHIN), "creating a theme file");
        settle(&mut reports);
        fs::write(&theme, "background: \"#000002\"\n").expect("edit the theme");
        assert!(reported(&mut reports, WITHIN), "editing a theme file");
        settle(&mut reports);
        fs::remove_file(&theme).expect("delete the theme");
        assert!(reported(&mut reports, WITHIN), "deleting a theme file");
    }

    /// A reload reads these files: if that counted as a change, every
    /// reload would cause the next.
    #[test]
    fn reading_the_config_and_its_themes_is_not_a_change() {
        let root = TempDir::new("watch-reads");
        let dir = root.path().join("zed-term");
        let themes = dir.join("themes");
        fs::create_dir_all(&themes).expect("create the themes dir");
        let config = dir.join("config.yaml");
        fs::write(&config, "theme:\n  default: midnight\n").expect("write config.yaml");
        fs::write(themes.join("midnight.yaml"), "background: \"#000001\"\n")
            .expect("write a theme");
        let (_watcher, mut reports) = watch(&dir);

        let loaded = load(&config);
        let library = Library::new(&loaded.config.theme.custom, Some(themes.as_path()));
        let _ = library.resolve(&loaded.config.theme.default, &mut Vec::new());
        assert!(!reported(&mut reports, QUIET), "a read was reported");
        fs::write(&config, "scrollback: 1\n").expect("write config.yaml");
        assert!(reported(&mut reports, WITHIN), "the watcher stopped");
    }

    #[test]
    fn other_files_in_the_config_dir_are_not_a_change() {
        let root = TempDir::new("watch-unrelated");
        let dir = root.path().join("zed-term");
        let themes = dir.join("themes");
        fs::create_dir_all(&themes).expect("create the themes dir");
        let (_watcher, mut reports) = watch(&dir);

        // Notes, the swap file of an editor that has config.yaml open,
        // YAML that is neither the config nor a theme, a theme's readme.
        let unrelated = [
            dir.join("notes.txt"),
            dir.join(".config.yaml.swp"),
            dir.join("other.yaml"),
            themes.join("README.md"),
        ];
        for file in &unrelated {
            fs::write(file, "x\n").expect("write an unrelated file");
        }
        assert!(
            !reported(&mut reports, QUIET),
            "an unrelated file was reported"
        );
        fs::write(dir.join("config.yaml"), "scrollback: 1\n").expect("write config.yaml");
        assert!(reported(&mut reports, WITHIN), "the watcher stopped");
    }

    /// On a fresh system the config dir doesn't exist when zed-term starts.
    #[test]
    fn a_config_dir_created_after_start_is_watched_once_armed() {
        let root = TempDir::new("watch-later");
        let dir = root.path().join("zed-term");
        let (mut watcher, mut reports) = watch(&dir);

        fs::create_dir(&dir).expect("create the config dir");
        assert!(reported(&mut reports, WITHIN), "creating the config dir");
        settle(&mut reports);
        watcher.arm();
        fs::write(dir.join("config.yaml"), "scrollback: 1\n").expect("write config.yaml");
        assert!(reported(&mut reports, WITHIN), "writing config.yaml");
    }

    /// Dotfile managers like GNU Stow link config.yaml to a file in the
    /// dotfiles repository, and editing it writes through the link.
    #[cfg(unix)]
    #[test]
    fn editing_a_linked_config_yaml_is_reported_within_a_second() {
        let root = TempDir::new("watch-linked");
        let dotfiles = root.path().join("dotfiles");
        let dir = root.path().join("zed-term");
        fs::create_dir(&dotfiles).expect("create the dotfiles dir");
        fs::create_dir(&dir).expect("create the config dir");
        fs::write(dotfiles.join("config.yaml"), "scrollback: 1\n").expect("write the config");
        std::os::unix::fs::symlink(dotfiles.join("config.yaml"), dir.join("config.yaml"))
            .expect("link config.yaml");
        let (_watcher, mut reports) = watch(&dir);

        fs::write(dir.join("config.yaml"), "scrollback: 2\n").expect("write config.yaml");
        assert!(reported(&mut reports, WITHIN), "writing through the link");
    }

    /// Linking the dotfiles in while zed-term runs: the reload the new link
    /// causes arms the watcher, which then follows it.
    #[cfg(unix)]
    #[test]
    fn a_config_yaml_linked_after_start_is_followed_once_armed() {
        let root = TempDir::new("watch-linked-later");
        let dotfiles = root.path().join("dotfiles");
        let dir = root.path().join("zed-term");
        fs::create_dir(&dotfiles).expect("create the dotfiles dir");
        fs::create_dir(&dir).expect("create the config dir");
        fs::write(dotfiles.join("config.yaml"), "scrollback: 1\n").expect("write the config");
        let (mut watcher, mut reports) = watch(&dir);

        std::os::unix::fs::symlink(dotfiles.join("config.yaml"), dir.join("config.yaml"))
            .expect("link config.yaml");
        assert!(reported(&mut reports, WITHIN), "linking config.yaml");
        settle(&mut reports);
        watcher.arm();
        fs::write(dir.join("config.yaml"), "scrollback: 2\n").expect("write config.yaml");
        assert!(reported(&mut reports, WITHIN), "writing through the link");
    }

    /// AC7 covers theme files too, and dotfile managers link those the
    /// same way.
    #[cfg(unix)]
    #[test]
    fn editing_a_linked_theme_file_is_reported_within_a_second() {
        let root = TempDir::new("watch-linked-theme");
        let dotfiles = root.path().join("dotfiles");
        let themes = root.path().join("zed-term/themes");
        fs::create_dir(&dotfiles).expect("create the dotfiles dir");
        fs::create_dir_all(&themes).expect("create the themes dir");
        fs::write(dotfiles.join("midnight.yaml"), "background: \"#000001\"\n")
            .expect("write the theme");
        std::os::unix::fs::symlink(dotfiles.join("midnight.yaml"), themes.join("midnight.yaml"))
            .expect("link the theme");
        let (_watcher, mut reports) = watch(&root.path().join("zed-term"));

        fs::write(themes.join("midnight.yaml"), "background: \"#000002\"\n")
            .expect("write the theme");
        assert!(reported(&mut reports, WITHIN), "writing through the link");
    }
}
