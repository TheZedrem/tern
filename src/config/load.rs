//! The lenient reader: [`parse`] and [`load`] (spec 0005).

use super::{
    ActivePane, Config, Cursor, Font, Keybinding, Loaded, LocalProfile, PluginSettings, Profile,
    ProfileKind, SshProfile, ThemeSettings, expand_home,
};
use crate::theme::ThemeSource;
use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_saphyr::Spanned;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU16;
use std::path::Path;

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

/// A part of the document that falls back on its own when it doesn't fit.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Key(String),
    Profile(String),
    Theme(String),
    Keybinding(String),
    Plugin(String),
}

impl Part {
    /// What happens to it, for the notice.
    fn fallback(&self) -> String {
        match self {
            Part::Key(key) => format!("`{key}` uses its default"),
            Part::Profile(id) => format!("profile `{id}` was skipped"),
            Part::Theme(name) => format!("theme `{name}` was skipped"),
            Part::Keybinding(keys) => format!("keybinding `{keys}` was skipped"),
            Part::Plugin(id) => format!("plugin `{id}` stays disabled"),
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
pub(super) trait MapReader<'de> {
    type Value;
    /// What a value of the wrong shape should have been, for the notice.
    const EXPECTING: &'static str;
    fn empty(self) -> Self::Value;
    fn read<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error>;
}

/// Reads a [`MapReader`]'s value with serde.
pub(super) struct Optional<R>(pub(super) R);

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
        // `app_sidebar` wins over its old name wherever the two stand.
        let mut app_sidebar = false;
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
                "open_new_tab" => {
                    if let Some(open) = map.next_value()? {
                        config.open_new_tab = open;
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
                "active_pane" => {
                    config.active_pane = map.next_value_seed(Optional(ActivePaneReader(self)))?;
                }
                "open_links" => {
                    if let Some(open) = map.next_value()? {
                        config.open_links = open;
                    }
                }
                "tab_bar" => {
                    if let Some(side) = map.next_value()? {
                        config.tab_bar = side;
                    }
                }
                "app_sidebar" => {
                    if let Some(side) = map.next_value()? {
                        config.app_sidebar = side;
                        app_sidebar = true;
                    }
                }
                // The setting's name before spec 0018: read without a
                // notice, so existing configs keep their side.
                "sftp_sidebar" => {
                    if let Some(side) = map.next_value()?
                        && !app_sidebar
                    {
                        config.app_sidebar = side;
                    }
                }
                "sftp_click" => {
                    if let Some(click) = map.next_value()? {
                        config.sftp_click = click;
                    }
                }
                "sftp_upload" => {
                    if let Some(mode) = map.next_value()? {
                        config.sftp_upload = mode;
                    }
                }
                "files_executable" => {
                    if let Some(executable) = map.next_value()? {
                        config.files_executable = executable;
                    }
                }
                "new_tab" => {
                    if let Some(new_tab) = map.next_value()? {
                        config.new_tab = new_tab;
                    }
                }
                "update_check" => config.update_check = map.next_value()?,
                "plugins" => {
                    let entries = map.next_value_seed(Optional(Entries::<PluginSettings>::new(
                        self,
                        Part::Key(key.name),
                        Part::Plugin,
                    )))?;
                    config.plugins = entries
                        .into_iter()
                        .map(|entry| (entry.name, entry.value))
                        .collect();
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

/// `active_pane:`, read key by key so an unknown one is a notice (spec
/// 0003, flow 3). `color: null` is the theme's accent.
struct ActivePaneReader<'a>(Attempt<'a>);

impl<'de> MapReader<'de> for ActivePaneReader<'_> {
    type Value = ActivePane;
    const EXPECTING: &'static str = "`border`, `color` and `dim`";

    fn empty(self) -> ActivePane {
        ActivePane::default()
    }

    fn read<A: MapAccess<'de>>(self, mut map: A) -> Result<ActivePane, A::Error> {
        let attempt = self.0;
        let mut active = ActivePane::default();
        while let Some(key) = attempt.key(&mut map, Some(Part::Key("active_pane".into())))? {
            match key.name.as_str() {
                "border" => {
                    if let Some(border) = map.next_value()? {
                        active.border = border;
                    }
                }
                "color" => active.color = map.next_value()?,
                "dim" => {
                    if let Some(dim) = map.next_value()? {
                        active.dim = dim;
                    }
                }
                unknown => {
                    map.next_value::<IgnoredAny>()?;
                    attempt.unknown_key(&format!("active_pane.{unknown}"), key.line);
                }
            }
        }
        Ok(active)
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
mod tests {
    use super::*;
    use crate::config::tests::{TempDir, profile_ids, ssh_to};
    use crate::config::{
        CloseOnExit, CursorStyle, FilesExecutable, NewTab, OpenLinks, SftpClick, SidebarSide,
        TEMPLATE, UpdateCheck, UploadMode,
    };
    use crate::terminal::FontSettings;
    use crate::terminal::pty::Command;
    use crate::theme::{self, Color, DARK, Library};

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
        let path = std::env::temp_dir().join("tern-test-does-not-exist/config.yaml");
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

    /// Spec 0003, flow 6, AC11: boot opens a fresh tab behind the restored
    /// ones unless `open_new_tab: false` turns that off.
    #[test]
    fn open_new_tab_is_on_unless_set_to_false() {
        assert!(parse("").config.open_new_tab);
        for (text, open) in [
            ("open_new_tab: true\n", true),
            ("open_new_tab: false\n", false),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.open_new_tab, open, "{text}");
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

    /// Spec 0003, flow 3, spec 0005 §Model: `active_pane` is by default a
    /// 2 px border in the theme's accent (`color: null`) and the other
    /// panes dimmed 30%. Each key sets its own part, a `"#rrggbb"` color
    /// included, and a key left out keeps its default.
    #[test]
    fn active_pane_takes_each_key_and_a_partial_map_keeps_the_defaults() {
        let default = ActivePane {
            border: 2,
            color: None,
            dim: 30,
        };
        assert_eq!(parse("").config.active_pane, default);
        let red = Some(Color(0xff0000));
        let cases = [
            ("{ border: 4, color: \"#ff0000\", dim: 40 }", 4, red, 40),
            ("{ border: 0, dim: 0 }", 0, None, 0),
            ("{ color: \"#FF0000\" }", 2, red, 30),
            ("{ color: null }", 2, None, 30),
            ("{ border: 5 }", 5, None, 30),
            ("{ dim: 50 }", 2, None, 50),
            ("{}", 2, None, 30),
        ];
        for (active_pane, border, color, dim) in cases {
            let text = format!("active_pane: {active_pane}\n");
            let Loaded { config, notices } = parse(&text);
            let expected = ActivePane { border, color, dim };
            assert_eq!(config.active_pane, expected, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0003, flow 3: the ranges are clamped where the marking is
    /// drawn, as `font.size` is (spec 0005), so a value past them is no
    /// mistake and gets no notice.
    #[test]
    fn an_active_pane_value_past_its_range_loads_without_a_notice() {
        let Loaded { config, notices } = parse("active_pane: { border: 40, dim: 90 }\n");
        assert_eq!(
            (config.active_pane.border, config.active_pane.dim),
            (40, 90)
        );
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Spec 0005: "Unknown keys produce a notice, not an error", in
    /// `active_pane` too; the rest of it still applies.
    #[test]
    fn an_unknown_key_inside_active_pane_is_a_notice_and_the_rest_of_it_applies() {
        let Loaded { config, notices } =
            parse("active_pane:\n  border: 4\n  opacity: 30\n  dim: 0\n");
        let expected = ActivePane {
            border: 4,
            dim: 0,
            ..ActivePane::default()
        };
        assert_eq!(config.active_pane, expected);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("active_pane.opacity") && notices[0].contains("line 3"),
            "{}",
            notices[0]
        );
    }

    /// Spec 0005: a value that doesn't fit resets its top-level key, here
    /// all of `active_pane`, with a notice naming it; the rest still
    /// applies. The good key beside the bad one resets too.
    #[test]
    fn a_bad_active_pane_value_resets_active_pane_with_a_notice_and_the_rest_applies() {
        for (bad, good) in [
            ("color: red", "border: 4"),
            ("color: \"#ff00\"", "border: 4"),
            ("border: wide", "dim: 30"),
            ("border: -1", "dim: 30"),
            ("border: 2.5", "dim: 30"),
            ("dim: -5", "border: 4"),
        ] {
            let text =
                format!("scrollback: 500\nactive_pane:\n  {good}\n  {bad}\ncopy_on_select: true\n");
            let Loaded { config, notices } = parse(&text);
            assert_eq!(config.active_pane, ActivePane::default(), "{bad}");
            assert_eq!(config.scrollback, 500, "{bad}");
            assert!(config.copy_on_select, "{bad}");
            assert_eq!(notices.len(), 1, "{bad}: {notices:?}");
            assert!(notices[0].contains("active_pane"), "{bad}: {}", notices[0]);
        }
    }

    /// Spec 0005 §Model, spec 0002 flow 7: links open on Ctrl+click
    /// unless `open_links: click` makes a plain click do.
    #[test]
    fn open_links_is_ctrl_click_unless_set_to_click() {
        assert_eq!(parse("").config.open_links, OpenLinks::CtrlClick);
        for (text, open_links) in [
            ("open_links: click\n", OpenLinks::Click),
            ("open_links: ctrl_click\n", OpenLinks::CtrlClick),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.open_links, open_links, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value `open_links` doesn't take, like `ctrl-click`,
    /// falls back to `ctrl_click` with a notice naming the key; the rest of
    /// the file still applies.
    #[test]
    fn a_bad_open_links_falls_back_to_ctrl_click_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "open_links: ctrl-click\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.open_links, OpenLinks::CtrlClick);
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("open_links"), "{}", notices[0]);
    }

    /// Spec 0005 §Model, spec 0004: `Ctrl+Shift+T` asks which profile
    /// unless `new_tab: default` makes it open the default one.
    #[test]
    fn new_tab_is_ask_unless_set_to_default() {
        assert_eq!(parse("").config.new_tab, NewTab::Ask);
        for (text, new_tab) in [
            ("new_tab: ask\n", NewTab::Ask),
            ("new_tab: default\n", NewTab::Default),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.new_tab, new_tab, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value `new_tab` doesn't take, like `profile`, falls
    /// back to `ask` with a notice naming the key; the rest of the file
    /// still applies.
    #[test]
    fn a_bad_new_tab_falls_back_to_ask_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "new_tab: profile\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.new_tab, NewTab::Ask);
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("new_tab"), "{}", notices[0]);
    }

    /// Spec 0018, Migration: under its old name, `sftp_sidebar`, the
    /// sidebar still sits on the right unless `left` moves it.
    #[test]
    fn sftp_sidebar_is_right_unless_set_to_left() {
        assert_eq!(parse("").config.app_sidebar, SidebarSide::Right);
        for (text, side) in [
            ("sftp_sidebar: left\n", SidebarSide::Left),
            ("sftp_sidebar: right\n", SidebarSide::Right),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.app_sidebar, side, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value `sftp_sidebar` doesn't take, like `split` (a
    /// later mode), falls back to `right` with a notice naming the key;
    /// the rest of the file still applies.
    #[test]
    fn a_bad_sftp_sidebar_falls_back_to_right_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "sftp_sidebar: split\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.app_sidebar, SidebarSide::Right);
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("sftp_sidebar"), "{}", notices[0]);
    }

    /// Spec 0018, Module surface: the sidebar sits on the right unless
    /// `app_sidebar: left` moves it.
    #[test]
    fn app_sidebar_is_right_unless_set_to_left() {
        for (text, side) in [
            ("app_sidebar: left\n", SidebarSide::Left),
            ("app_sidebar: right\n", SidebarSide::Right),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.app_sidebar, side, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0018, Migration [load-bearing] and AC4: a config that still
    /// says `sftp_sidebar: left` keeps its sidebar on the left, among its
    /// other settings, and reading the old name gives no notice.
    #[test]
    fn a_config_with_only_sftp_sidebar_keeps_its_side_without_a_notice() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "sftp_sidebar: left   # where the SFTP sidebar opens\n",
            "tab_bar: left\n",
        ));
        assert_eq!(config.app_sidebar, SidebarSide::Left);
        assert_eq!(config.scrollback, 500);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Spec 0018, Migration [load-bearing]: in a file that has both names,
    /// `app_sidebar` wins whichever comes first, and neither line gives a
    /// notice.
    #[test]
    fn app_sidebar_wins_over_sftp_sidebar_in_either_order() {
        let cases = [
            (
                "app_sidebar: left\nsftp_sidebar: right\n",
                SidebarSide::Left,
            ),
            (
                "sftp_sidebar: right\napp_sidebar: left\n",
                SidebarSide::Left,
            ),
            (
                "app_sidebar: right\nsftp_sidebar: left\n",
                SidebarSide::Right,
            ),
            (
                "sftp_sidebar: left\napp_sidebar: right\n",
                SidebarSide::Right,
            ),
        ];
        for (text, side) in cases {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.app_sidebar, side, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value `app_sidebar` doesn't take falls back to `right`
    /// with a notice naming the key; the rest of the file still applies.
    #[test]
    fn a_bad_app_sidebar_falls_back_to_right_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "app_sidebar: top\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.app_sidebar, SidebarSide::Right);
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("app_sidebar"), "{}", notices[0]);
    }

    /// Spec 0018, AC4 and Module surface: the template "Open config file"
    /// writes has the `app_sidebar` line, set to the default, in place of
    /// the old `sftp_sidebar` one, and loads without a notice.
    #[test]
    fn the_template_has_app_sidebar_and_no_sftp_sidebar() {
        let lines: Vec<&str> = TEMPLATE
            .lines()
            .filter(|line| line.starts_with("app_sidebar:"))
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("app_sidebar: right"), "{}", lines[0]);
        assert!(!TEMPLATE.contains("sftp_sidebar"), "the old name is left");
        let Loaded { config, notices } = parse(TEMPLATE);
        assert_eq!(config.app_sidebar, SidebarSide::Right);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Spec 0008 §Settings: a click in the SFTP sidebar edits a file
    /// unless `sftp_click: download` says otherwise.
    #[test]
    fn sftp_click_is_edit_unless_set_to_download() {
        assert_eq!(parse("").config.sftp_click, SftpClick::Edit);
        for (text, click) in [
            ("sftp_click: download\n", SftpClick::Download),
            ("sftp_click: edit\n", SftpClick::Edit),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.sftp_click, click, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value `sftp_click` doesn't take falls back to `edit`
    /// with a notice naming the key; the rest of the file still applies.
    #[test]
    fn a_bad_sftp_click_falls_back_to_edit_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "sftp_click: run\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.sftp_click, SftpClick::Edit);
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("sftp_click"), "{}", notices[0]);
    }

    /// Spec 0008 §Settings: edits upload on a click unless `sftp_upload`
    /// says `save` or `focus`.
    #[test]
    fn sftp_upload_is_manual_unless_set_to_save_or_focus() {
        assert_eq!(parse("").config.sftp_upload, UploadMode::Manual);
        for (text, mode) in [
            ("sftp_upload: save\n", UploadMode::Save),
            ("sftp_upload: focus\n", UploadMode::Focus),
            ("sftp_upload: manual\n", UploadMode::Manual),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.sftp_upload, mode, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value `sftp_upload` doesn't take falls back to
    /// `manual` with a notice naming the key; the rest still applies.
    #[test]
    fn a_bad_sftp_upload_falls_back_to_manual_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "sftp_upload: always\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.sftp_upload, UploadMode::Manual);
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("sftp_upload"), "{}", notices[0]);
    }

    /// Spec 0013 §Settings: a runnable local file asks unless
    /// `files_executable` says `run` or `edit`.
    #[test]
    fn files_executable_asks_unless_set_to_run_or_edit() {
        assert_eq!(parse("").config.files_executable, FilesExecutable::Ask);
        for (text, executable) in [
            ("files_executable: run\n", FilesExecutable::Run),
            ("files_executable: edit\n", FilesExecutable::Edit),
            ("files_executable: ask\n", FilesExecutable::Ask),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.files_executable, executable, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0005: a value `files_executable` doesn't take falls back to
    /// `ask`, never to running, with a notice naming the key; the rest
    /// still applies.
    #[test]
    fn a_bad_files_executable_falls_back_to_ask_with_a_notice_and_the_rest_applies() {
        let Loaded { config, notices } = parse(concat!(
            "scrollback: 500\n",
            "files_executable: always\n",
            "copy_on_select: true\n",
        ));
        assert_eq!(config.files_executable, FilesExecutable::Ask);
        assert_eq!(config.scrollback, 500);
        assert!(config.copy_on_select);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("files_executable"), "{}", notices[0]);
    }

    /// Spec 0011 §Model: `update_check` is unset unless it's `daily` or
    /// `off`; a key with no value is unset too, without a notice.
    #[test]
    fn update_check_is_unset_unless_daily_or_off() {
        assert_eq!(parse("").config.update_check, None);
        for (text, update_check) in [
            ("update_check:\n", None),
            ("update_check: daily\n", Some(UpdateCheck::Daily)),
            ("update_check: off\n", Some(UpdateCheck::Off)),
        ] {
            let Loaded { config, notices } = parse(text);
            assert_eq!(config.update_check, update_check, "{text}");
            assert!(notices.is_empty(), "{text}: {notices:?}");
        }
    }

    /// Spec 0011 §Model, spec 0005: any other value, `ask` included, is a
    /// config error that leaves the key unset, so the question still
    /// appears; the notice names the key and the rest of the file applies.
    #[test]
    fn a_bad_update_check_is_unset_with_a_notice_and_the_rest_applies() {
        for value in ["ask", "weekly", "Daily", "true", "1", "[daily]"] {
            let text = format!("scrollback: 500\nupdate_check: {value}\ncopy_on_select: true\n");
            let Loaded { config, notices } = parse(&text);
            assert_eq!(config.update_check, None, "{value}");
            assert_eq!(config.scrollback, 500, "{value}");
            assert!(config.copy_on_select, "{value}");
            assert_eq!(notices.len(), 1, "{value}: {notices:?}");
            assert!(notices[0].contains("update_check"), "{}", notices[0]);
        }
    }

    /// Spec 0011 §Model: the template documents `update_check` only as a
    /// comment, since a written value would answer the question for the
    /// user.
    #[test]
    fn the_template_leaves_update_check_unset() {
        assert_eq!(parse(TEMPLATE).config.update_check, None);
        assert!(
            TEMPLATE
                .lines()
                .any(|line| line.starts_with("# update_check:")),
            "update_check isn't documented"
        );
        assert!(
            !TEMPLATE
                .lines()
                .any(|line| line.trim_start().starts_with("update_check")),
            "update_check is written"
        );
    }

    /// AC1 (spec 0005).
    #[test]
    fn no_config_file_boots_the_login_shell_on_dark_with_the_bundled_font() {
        let root = TempDir::new("no-config");
        let dir = root.path().join("tern");
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
open_links: ctrl_click              # ctrl_click | click (spec 0002, flow 7)
tab_bar: top                        # top | bottom | left | right (spec 0003)
active_pane: { border: 2, color: null, dim: 30 }  # in a split: border px 0–6, color null → theme accent, dim % 0–50 (spec 0003)
sftp_sidebar: right                # left | right (spec 0008)
sftp_click: edit                    # edit | download (spec 0008)
sftp_upload: manual                 # manual | save | focus (spec 0008)
files_executable: ask               # ask | run | edit (spec 0013)
new_tab: ask                    # what Ctrl+Shift+T opens: ask | default (spec 0004)
close_on_exit: clean                # always | clean | never
restore_session: true
open_new_tab: true                  # a fresh tab on every launch, or a restored local shell focused (spec 0003)
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

    /// Its `cursor`, `active_pane`, `copy_on_select` and `open_links` spell
    /// out the defaults.
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
            keybindings: vec![binding("ctrl+shift+d", "pane.splitRight", 37)],
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

    /// Notepad saves with CRLF line endings, and can add a byte order mark.
    #[test]
    fn a_config_saved_on_windows_loads_the_same() {
        let loaded = parse("\u{feff}scrollback: 500\r\nfont:\r\n  size: 16\r\n");
        assert_eq!(loaded.config.scrollback, 500);
        assert_eq!(loaded.config.font.size, 16.0);
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
    }

    /// The `plugins:` block of spec 0016 §Model, as written there.
    const SPEC_PLUGINS: &str = r#"
plugins:
  docker:
    enabled: true
    run: [./tern-docker]          # the command line the user confirmed
    rights: [pane_info, terminal_write]
    events: [pane_opened, pane_closed]
    settings: { socket: /var/run/docker.sock }
"#;

    /// The entry [`SPEC_PLUGINS`] grants `docker`.
    fn spec_docker() -> PluginSettings {
        use crate::config::Setting;
        use crate::plugins::{Right, Subscription};
        PluginSettings {
            enabled: true,
            run: vec!["./tern-docker".into()],
            rights: vec![Right::PaneInfo, Right::TerminalWrite],
            events: vec![Subscription::PaneOpened, Subscription::PaneClosed],
            settings: BTreeMap::from([(
                "socket".into(),
                Setting::Text("/var/run/docker.sock".into()),
            )]),
        }
    }

    /// Whether plugin `id` is enabled by `config`. A plugin that isn't
    /// named is found and never started.
    fn plugin_enabled(config: &Config, id: &str) -> bool {
        config.plugins.get(id).is_some_and(|plugin| plugin.enabled)
    }

    /// Spec 0016 §Model: enabling, the confirmed `run`, each right, each
    /// event and the plugin's settings are stored under `plugins.<id>`.
    #[test]
    fn the_spec_plugins_block_grants_what_it_says() {
        let Loaded { config, notices } = parse(SPEC_PLUGINS);
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(
            config.plugins,
            BTreeMap::from([("docker".into(), spec_docker())])
        );
        assert_eq!(parse("").config.plugins, BTreeMap::new());
    }

    /// Spec 0016 §Model: every right and every listed event can be
    /// ticked, and a setting is text, a bool or a number (flow 8).
    #[test]
    fn every_right_event_and_kind_of_setting_loads() {
        use crate::config::Setting;
        use crate::plugins::{Right, Subscription};
        let Loaded { config, notices } = parse(concat!(
            "plugins:\n",
            "  all:\n",
            "    enabled: true\n",
            "    run: [python, main.py, --flag]\n",
            "    rights: [pane_info, terminal_write, terminal_read, open_tab]\n",
            "    events: [start, pane_opened, pane_closed]\n",
            "    settings: { name: web, tls: true, retries: 3, ratio: 0.5 }\n",
        ));
        assert!(notices.is_empty(), "{notices:?}");
        let all = &config.plugins["all"];
        assert!(all.enabled);
        assert_eq!(all.run, ["python", "main.py", "--flag"]);
        assert_eq!(
            all.rights,
            [
                Right::PaneInfo,
                Right::TerminalWrite,
                Right::TerminalRead,
                Right::OpenTab
            ]
        );
        assert_eq!(
            all.events,
            [
                Subscription::Start,
                Subscription::PaneOpened,
                Subscription::PaneClosed
            ]
        );
        let expected = BTreeMap::from([
            ("name".to_owned(), Setting::Text("web".into())),
            ("tls".to_owned(), Setting::Bool(true)),
            ("retries".to_owned(), Setting::Number(3.0)),
            ("ratio".to_owned(), Setting::Number(0.5)),
        ]);
        assert_eq!(all.settings, expected);
    }

    /// Spec 0016 §Model, "Disabled until enabled": only `enabled: true`
    /// enables, and nothing is granted that isn't written.
    #[test]
    fn a_plugin_is_disabled_and_holds_nothing_unless_its_entry_says_so() {
        let Loaded { config, notices } = parse(concat!(
            "plugins:\n",
            "  silent:\n",
            "    run: [./silent]\n",
            "  off:\n",
            "    enabled: false\n",
            "    run: [./off]\n",
            "    rights: [terminal_write]\n",
            "  bare:\n",
            "    enabled: true\n",
        ));
        assert!(notices.is_empty(), "{notices:?}");
        assert!(!plugin_enabled(&config, "silent"));
        assert!(!plugin_enabled(&config, "off"));
        assert!(!plugin_enabled(&config, "unnamed"));
        let bare = &config.plugins["bare"];
        assert!(bare.enabled);
        assert!(bare.run.is_empty(), "no command line was confirmed");
        assert!(bare.rights.is_empty() && bare.events.is_empty());
        assert!(bare.settings.is_empty());
    }

    /// Spec 0005 and spec 0016 §Model: an entry Tern can't read as written
    /// (a right, an event or a key it doesn't know, a value of the wrong
    /// type) leaves that plugin disabled, never running with something
    /// other than what was written. The notice names it, and the other
    /// plugins and the rest of the file still apply.
    #[test]
    fn a_plugin_entry_that_doesnt_fit_stays_disabled_alone_with_a_notice() {
        let bad = [
            "rights: [pane_info, root]",
            "rights: [Terminal_Write]",
            "events: [pane_opened, keystroke]",
            "events: [command]",
            "enable: true",
            "sandbox: off",
            "run: ./tern-broken --flag",
            "enabled: yes please",
            "rights: terminal_write",
            "settings: [socket]",
        ];
        for line in bad {
            // Enabled and confirmed, but for the line that doesn't fit.
            let key = line.split(':').next();
            let rest: String = ["enabled: true", "run: [./tern-broken]"]
                .into_iter()
                .filter(|kept| kept.split(':').next() != key)
                .map(|kept| format!("    {kept}\n"))
                .collect();
            let text = format!(
                "scrollback: 500\n\
                 plugins:\n  \
                   broken:\n\
                     {rest}    \
                     {line}\n  \
                   docker:\n    \
                     enabled: true\n    \
                     run: [./tern-docker]\n    \
                     rights: [pane_info, terminal_write]\n    \
                     events: [pane_opened, pane_closed]\n    \
                     settings: {{ socket: /var/run/docker.sock }}\n\
                 copy_on_select: true\n"
            );
            let Loaded { config, notices } = parse(&text);
            assert!(!plugin_enabled(&config, "broken"), "{line}: enabled");
            assert_eq!(config.plugins.get("docker"), Some(&spec_docker()), "{line}");
            assert_eq!(config.scrollback, 500, "{line}");
            assert!(config.copy_on_select, "{line}");
            assert_eq!(notices.len(), 1, "{line}: {notices:?}");
            assert!(notices[0].contains("broken"), "{line}: {}", notices[0]);
            assert!(!notices[0].contains("docker"), "{line}: {}", notices[0]);
        }
    }

    /// Spec 0005: a `plugins` that isn't a map of entries falls back as a
    /// whole, with a notice naming it; the rest still applies.
    #[test]
    fn a_plugins_key_that_isnt_a_map_enables_nothing_and_the_rest_applies() {
        for value in ["[docker]", "docker", "true"] {
            let text = format!("scrollback: 500\nplugins: {value}\ncopy_on_select: true\n");
            let Loaded { config, notices } = parse(&text);
            assert_eq!(config.plugins, BTreeMap::new(), "{value}");
            assert_eq!(config.scrollback, 500, "{value}");
            assert!(config.copy_on_select, "{value}");
            assert_eq!(notices.len(), 1, "{value}: {notices:?}");
            assert!(notices[0].contains("plugins"), "{}", notices[0]);
        }
        let Loaded { config, notices } = parse("plugins:\nscrollback: 500\n");
        assert_eq!(config.plugins, BTreeMap::new());
        assert_eq!(config.scrollback, 500);
        assert!(notices.is_empty(), "{notices:?}");
    }

    /// Definition of Done: `plugins:` has its template lines, commented
    /// out, since a written entry would enable a plugin nobody confirmed.
    /// Uncommented, the example is an entry Tern reads as written.
    #[test]
    fn the_template_documents_plugins_as_a_comment_that_loads_when_uncommented() {
        assert_eq!(parse(TEMPLATE).config.plugins, BTreeMap::new());
        let start = TEMPLATE
            .lines()
            .position(|line| line == "# plugins:")
            .expect("`plugins:` isn't documented");
        let block: Vec<&str> = TEMPLATE
            .lines()
            .skip(start)
            .take_while(|line| line.starts_with('#'))
            .map(|line| line.strip_prefix("# ").unwrap_or("").trim_end())
            .collect();
        let Loaded { config, notices } = parse(&block.join("\n"));
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(config.plugins.len(), 1, "{block:?}");
        let example = config.plugins.values().next().expect("one example");
        assert!(example.enabled);
        assert!(!example.run.is_empty(), "the example confirms no `run`");
        assert!(!example.rights.is_empty() && !example.events.is_empty());
    }
}
