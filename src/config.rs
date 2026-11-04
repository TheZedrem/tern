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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What "Open config file" writes when there is no config yet: every key at
/// its default, explained. It parses back to exactly [`Config::default`].
pub const TEMPLATE: &str = include_str!("default-config.yaml");

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// The profile new tabs open, as written; see [`Config::default_profile`].
    pub default_profile: Option<String>,
    /// In file order. Empty means the one implicit `shell` profile.
    pub profiles: Vec<Profile>,
    pub font: Font,
    pub scrollback: u32,
    pub close_on_exit: CloseOnExit,
    pub theme: ThemeSettings,
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

/// A `type: local` profile: what a pane runs. `type: ssh` profiles are
/// skipped at load until they can connect (T-012).
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: Option<String>,
    /// Program and arguments. `None` runs the user's login shell.
    pub command: Option<Vec<String>>,
    /// With `~` expanded. A directory that doesn't exist falls back to home
    /// when the pane starts.
    pub cwd: Option<PathBuf>,
    /// Set on top of the inherited environment.
    pub env: BTreeMap<String, String>,
    /// The theme of this profile's panes; `None` follows `theme.default`.
    pub theme: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_profile: None,
            profiles: Vec::new(),
            font: Font::default(),
            scrollback: 10_000,
            close_on_exit: CloseOnExit::Clean,
            theme: ThemeSettings::default(),
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
    /// The profile new tabs open: `default_profile` if it names a profile,
    /// else the first one, else the implicit login shell.
    pub fn default_profile(&self) -> Profile {
        self.default_profile
            .as_deref()
            .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
            .or_else(|| self.profiles.first())
            .cloned()
            .unwrap_or_else(Profile::shell)
    }

    /// Profile `id` as configured now, or the default profile if it's gone
    /// (spec 0003: a missing profile opens `default_profile`).
    pub fn profile_or_default(&self, id: &str) -> Profile {
        self.profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
            .unwrap_or_else(|| self.default_profile())
    }
}

impl Profile {
    /// The only profile of a config that configures none.
    pub fn shell() -> Self {
        Self {
            id: "shell".into(),
            name: None,
            command: None,
            cwd: None,
            env: BTreeMap::new(),
            theme: None,
        }
    }

    /// The name tab titles and the palette show: `name`, else the ID.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
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

/// Checks what only the whole config can: that `default_profile` exists,
/// counting the implicit `shell` of a config without profiles.
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

/// Watches the config directory for edits to `config.yaml` and
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
}

impl Part {
    /// What happens to it, for the notice.
    fn fallback(&self) -> String {
        match self {
            Part::Key(key) => format!("`{key}` uses its default"),
            Part::Profile(id) => format!("profile `{id}` was skipped"),
            Part::Theme(name) => format!("theme `{name}` was skipped"),
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
                // Read by the tasks that build them: cursor and
                // copy_on_select with T-002, keybindings with T-004,
                // restore_session with T-006.
                "cursor" | "copy_on_select" | "keybindings" | "restore_session" => {
                    map.next_value::<IgnoredAny>()?;
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
        match raw.kind {
            ProfileKind::Ssh => {
                self.notice(format!(
                    "config.yaml{at}: profile `{id}` was skipped: SSH profiles aren't supported yet"
                ));
                None
            }
            ProfileKind::Local => Some(Profile {
                id,
                name: raw.name,
                command: raw.command.filter(|command| !command.is_empty()),
                cwd: raw.cwd.as_deref().map(expand_home),
                env: raw.env,
                theme: raw.theme,
            }),
        }
    }
}

/// `type:`, which picks how a profile is read.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProfileKind {
    Local,
    Ssh,
}

/// A profile as written. Unknown keys make it fail, so a typo like
/// `comand` can't silently run the login shell instead.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    #[serde(rename = "type")]
    kind: ProfileKind,
    name: Option<String>,
    command: Option<Vec<String>>,
    cwd: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    theme: Option<String>,
    /// Only checked for, never kept: secrets belong in the OS keychain.
    password: Option<IgnoredAny>,
    // The connection of a `type: ssh` profile (spec 0006), read with T-012.
    #[serde(rename = "host")]
    _host: Option<IgnoredAny>,
    #[serde(rename = "user")]
    _user: Option<IgnoredAny>,
    #[serde(rename = "port")]
    _port: Option<IgnoredAny>,
    #[serde(rename = "identity_file")]
    _identity_file: Option<IgnoredAny>,
    #[serde(rename = "group")]
    _group: Option<IgnoredAny>,
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

    /// The example of spec 0005 §Model, as written there.
    const SPEC_EXAMPLE: &str = r##"
default_profile: shell              # missing → first profile in file order

profiles:                           # map key = stable profile ID
  shell:
    type: local                     # no command → the user's login shell
    env: { EDITOR: nvim }
  htop:
    name: Htop                      # display name; defaults to the ID
    type: local
    command: [htop]                 # program + args
    cwd: ~/projects                 # optional, default: home
  my-server:                        # spec 0006; skipped with a notice until T-012
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

    /// The keys later tasks read (cursor, keybindings, ...) pass silently.
    #[test]
    fn the_spec_example_loads_with_one_notice_for_its_ssh_profile() {
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
                    env: BTreeMap::from([("EDITOR".into(), "nvim".into())]),
                    ..Profile::shell()
                },
                Profile {
                    id: "htop".into(),
                    name: Some("Htop".into()),
                    command: Some(vec!["htop".into()]),
                    cwd: Some(home.join("projects")),
                    ..Profile::shell()
                },
            ],
            theme: ThemeSettings {
                default: "dark".into(),
                custom: vec![("midnight".into(), midnight)],
            },
            ..Config::default()
        };
        assert_eq!(config, expected);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("my-server"), "{}", notices[0]);
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
        assert_eq!(config.profiles[0].command, Some(vec!["htop".to_owned()]));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("broken") && notices[0].contains("line 7"),
            "{}",
            notices[0]
        );
    }

    #[test]
    fn new_tabs_open_default_profile_else_the_first_profile_in_file_order() {
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
        assert_eq!(unnamed.config.default_profile().id, "zeta");
        assert!(named.notices.is_empty(), "{:?}", named.notices);
        assert!(unnamed.notices.is_empty(), "{:?}", unnamed.notices);
    }

    #[test]
    fn an_unknown_default_profile_opens_the_first_profile_with_a_notice() {
        let loaded = parse(concat!(
            "default_profile: nope\n",
            "profiles:\n",
            "  zeta:\n",
            "    type: local\n",
            "  alpha:\n",
            "    type: local\n",
        ));
        assert_eq!(loaded.config.default_profile().id, "zeta");
        assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
        assert!(loaded.notices[0].contains("nope"), "{}", loaded.notices[0]);
    }

    /// With no profiles configured, `shell` is the implicit login-shell
    /// profile (spec 0005 §Model), so naming it is no mistake. The template
    /// suggests exactly this line.
    #[test]
    fn default_profile_shell_names_the_implicit_profile_without_a_notice() {
        let loaded = parse("default_profile: shell\n");
        assert_eq!(loaded.config.default_profile().id, "shell");
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
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
