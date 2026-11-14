//! A plugin's `plugin.yaml` and the search for plugin folders (spec 0016,
//! Model and flow 1). Everything a manifest says is untrusted text: it is
//! checked and cleaned here, before anything shows or runs it
//! (TM-plug-01, -13, -15).

use crate::terminal::clean_title;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The protocol version this Tern speaks.
pub const API: u32 = 1;
/// A larger manifest is unusable without being parsed (TM-plug-13).
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// The most characters of a name or title that are shown (TM-plug-01).
const MAX_NAME_CHARS: usize = 64;
/// The longest ID of a plugin, command, panel or setting.
const MAX_ID_CHARS: usize = 64;

/// What a plugin may ask Tern for, once the user ticked it.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Right {
    PaneInfo,
    TerminalWrite,
    TerminalRead,
    OpenTab,
}

impl Right {
    /// Its name in a manifest and in the config.
    pub fn name(self) -> &'static str {
        match self {
            Right::PaneInfo => "pane_info",
            Right::TerminalWrite => "terminal_write",
            Right::TerminalRead => "terminal_read",
            Right::OpenTab => "open_tab",
        }
    }
}

/// The events a plugin gets only when its manifest lists them and the user
/// ticked them.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Subscription {
    Start,
    PaneOpened,
    PaneClosed,
}

impl Subscription {
    /// Its name in a manifest and in the config.
    pub fn name(self) -> &'static str {
        match self {
            Subscription::Start => "start",
            Subscription::PaneOpened => "pane_opened",
            Subscription::PaneClosed => "pane_closed",
        }
    }
}

/// A usable manifest: checked, its texts cleaned by the title rules.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// The command line on this OS, as written: what the user confirms.
    pub run: Vec<String>,
    pub rights: Vec<Right>,
    pub events: Vec<Subscription>,
    #[allow(dead_code, reason = "listed in the palette (T-068)")]
    pub commands: Vec<Named>,
    pub panels: Vec<Named>,
    pub settings: Vec<Field>,
}

/// A command or a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub id: String,
    pub title: String,
    /// A panel's icon, by the name of one of Tern's own.
    pub icon: Option<String>,
}

/// One of the plugin's own settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Text,
    Bool,
    Number,
    Choice(Vec<String>),
}

impl Manifest {
    /// The program `run` names and its arguments. A first entry starting
    /// with `./` is inside `folder`; a bare name is looked up on `PATH`,
    /// never in the working directory (TM-plug-15).
    pub fn command(&self, folder: &Path) -> (PathBuf, &[String]) {
        let (first, args) = self.run.split_first().unwrap_or((&EMPTY, &[]));
        let program = match in_folder(first) {
            Some(inside) => folder.join(inside),
            None => PathBuf::from(first),
        };
        (program, args)
    }
}

static EMPTY: String = String::new();

/// The part of `program` after `./` (`.\` on Windows too).
fn in_folder(program: &str) -> Option<&str> {
    program
        .strip_prefix("./")
        .or_else(|| program.strip_prefix(".\\").filter(|_| cfg!(windows)))
}

/// A plugin folder and what its manifest is good for.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The folder's name.
    pub id: String,
    pub folder: PathBuf,
    /// The manifest, or why the plugin is unusable.
    pub manifest: Result<Manifest, String>,
}

/// Every folder in `root`, by name (flow 1). Reads files: call it off the
/// UI thread.
pub fn discover(root: &Path) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<Found> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            let folder = entry.path();
            let manifest = read(&folder).and_then(|text| parse(&id, &text));
            Some(Found {
                id,
                folder,
                manifest,
            })
        })
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    reject_shared_names(&mut found);
    found
}

fn read(folder: &Path) -> Result<String, String> {
    let file = std::fs::File::open(folder.join("plugin.yaml"))
        .map_err(|error| format!("plugin.yaml can't be read: {error}"))?;
    let mut text = String::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("plugin.yaml can't be read: {error}"))?;
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("plugin.yaml is larger than 64 KiB".into());
    }
    Ok(text)
}

/// A name another plugin has, ignoring case, or Tern's own, makes every
/// plugin with it unusable: a request must not read as coming from
/// something the user already trusts (TM-plug-01).
fn reject_shared_names(found: &mut [Found]) {
    let names: Vec<Option<String>> = found
        .iter()
        .map(|plugin| plugin.manifest.as_ref().ok().map(|m| m.name.to_lowercase()))
        .collect();
    for (ix, plugin) in found.iter_mut().enumerate() {
        let Some(name) = &names[ix] else {
            continue;
        };
        let twin = names
            .iter()
            .enumerate()
            .find(|(other, twin)| *other != ix && twin.as_ref() == Some(name));
        if name == "tern" {
            plugin.manifest = Err("its name is Tern's own".into());
        } else if twin.is_some() {
            plugin.manifest = Err("another plugin has the same name".into());
        }
    }
}

/// Only the version, read first: a manifest of another `api` may have any
/// shape.
#[derive(Deserialize)]
struct Versioned {
    api: u32,
}

#[derive(Deserialize)]
struct Raw {
    id: String,
    name: Scalar,
    #[serde(default)]
    version: Option<Scalar>,
    run: Run,
    #[serde(default)]
    rights: Vec<Right>,
    #[serde(default)]
    events: Vec<Subscription>,
    #[serde(default)]
    commands: Vec<RawNamed>,
    #[serde(default)]
    panels: Vec<RawNamed>,
    #[serde(default)]
    settings: Vec<RawField>,
}

/// `run:` as one command line, or one per OS.
#[derive(Deserialize)]
#[serde(untagged)]
enum Run {
    Everywhere(Vec<String>),
    PerOs {
        linux: Option<Vec<String>>,
        windows: Option<Vec<String>>,
        macos: Option<Vec<String>>,
    },
}

/// Text that YAML may read as a number, like `version: 1.0`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Scalar {
    Text(String),
    Integer(i64),
    Float(f64),
}

impl Scalar {
    fn text(self) -> String {
        match self {
            Scalar::Text(text) => text,
            Scalar::Integer(number) => number.to_string(),
            Scalar::Float(number) => number.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct RawNamed {
    id: String,
    title: Scalar,
    #[serde(default)]
    icon: Option<String>,
}

#[derive(Deserialize)]
struct RawField {
    key: String,
    label: Scalar,
    #[serde(rename = "type")]
    kind: RawKind,
    #[serde(default)]
    choices: Vec<Scalar>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawKind {
    Text,
    Bool,
    Number,
    Choice,
}

/// The manifest in `text`, for the plugin in folder `folder_name`, or why
/// it is unusable.
pub fn parse(folder_name: &str, text: &str) -> Result<Manifest, String> {
    let invalid = |error: serde_saphyr::Error| {
        let said = error.to_string();
        let first = said.lines().next().unwrap_or("it isn't valid YAML");
        format!("plugin.yaml: {}", shown(first, 200))
    };
    let Versioned { api } = serde_saphyr::from_str(text).map_err(invalid)?;
    if api != API {
        return Err(format!(
            "it is written for plugin API {api}; this Tern speaks {API}"
        ));
    }
    let raw: Raw = serde_saphyr::from_str(text).map_err(invalid)?;
    if raw.id != folder_name {
        return Err("its `id` differs from its folder's name".into());
    }
    if !is_id(&raw.id) {
        return Err(format!("its `id` isn't {ID_RULE}"));
    }
    let name = shown(&raw.name.text(), MAX_NAME_CHARS);
    if name.is_empty() {
        return Err("its `name` is empty".into());
    }
    let run = match raw.run {
        Run::Everywhere(run) => Some(run),
        Run::PerOs {
            linux,
            windows,
            macos,
        } => {
            if cfg!(windows) {
                windows
            } else if cfg!(target_os = "macos") {
                macos
            } else {
                linux
            }
        }
    };
    let run = run
        .filter(|run| !run.is_empty())
        .ok_or("its `run` names no program for this system")?;
    check_program(&run[0])?;
    Ok(Manifest {
        name,
        version: shown(&raw.version.map(Scalar::text).unwrap_or_default(), 32),
        run,
        rights: raw.rights,
        events: raw.events,
        commands: named(raw.commands, "command")?,
        panels: named(raw.panels, "panel")?,
        settings: fields(raw.settings)?,
    })
}

const ID_RULE: &str = "1 to 64 of a-z, 0-9, `-` and `_`";

/// IDs end up in command IDs and config keys, so they are plain.
fn is_id(id: &str) -> bool {
    (1..=MAX_ID_CHARS).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Untrusted text as it's shown: the title rules, cut at `max` characters.
pub(super) fn shown(text: &str, max: usize) -> String {
    clean_title(text).chars().take(max).collect()
}

/// A program is a bare name, an absolute path, or `./` and a path that
/// stays inside the plugin's folder.
fn check_program(program: &str) -> Result<(), String> {
    let path = Path::new(in_folder(program).unwrap_or(program));
    let inside = path
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    let fits = match in_folder(program) {
        Some(_) => inside,
        None => path.is_absolute() || (inside && path.components().count() == 1),
    };
    if program.is_empty() || !fits {
        return Err(
            "its `run` must start with a program's name, an absolute path, \
             or `./` and a path inside the plugin's folder"
                .into(),
        );
    }
    Ok(())
}

fn named(raw: Vec<RawNamed>, what: &str) -> Result<Vec<Named>, String> {
    let mut all: Vec<Named> = Vec::new();
    for RawNamed { id, title, icon } in raw {
        if !is_id(&id) {
            return Err(format!("a {what}'s `id` isn't {ID_RULE}"));
        }
        if all.iter().any(|known| known.id == id) {
            return Err(format!("{what} `{id}` is there twice"));
        }
        let title = shown(&title.text(), MAX_NAME_CHARS);
        if title.is_empty() {
            return Err(format!("{what} `{id}` has no `title`"));
        }
        all.push(Named { id, title, icon });
    }
    Ok(all)
}

/// The most fields one `set_settings` may draw.
const MAX_DYNAMIC_FIELDS: usize = 64;

/// The fields of a `set_settings` action, checked by the manifest's rules
/// (spec 0016, flow 8); used as serde's `deserialize_with`.
pub fn dynamic_fields<'de, D>(deserializer: D) -> Result<Vec<Field>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let raw = Vec::<RawField>::deserialize(deserializer)?;
    if raw.len() > MAX_DYNAMIC_FIELDS {
        return Err(D::Error::custom(format!(
            "more than {MAX_DYNAMIC_FIELDS} settings"
        )));
    }
    fields(raw).map_err(D::Error::custom)
}

fn fields(raw: Vec<RawField>) -> Result<Vec<Field>, String> {
    let mut all: Vec<Field> = Vec::new();
    for field in raw {
        let key = field.key;
        if !is_id(&key) {
            return Err(format!("a setting's `key` isn't {ID_RULE}"));
        }
        if all.iter().any(|known| known.key == key) {
            return Err(format!("setting `{key}` is there twice"));
        }
        let choices: Vec<String> = field
            .choices
            .into_iter()
            .map(|choice| shown(&choice.text(), MAX_NAME_CHARS))
            .collect();
        let kind = match field.kind {
            RawKind::Text => FieldKind::Text,
            RawKind::Bool => FieldKind::Bool,
            RawKind::Number => FieldKind::Number,
            RawKind::Choice if choices.is_empty() => {
                return Err(format!("setting `{key}` has no `choices`"));
            }
            RawKind::Choice => FieldKind::Choice(choices),
        };
        all.push(Field {
            key,
            label: shown(&field.label.text(), MAX_NAME_CHARS),
            kind,
        });
    }
    Ok(all)
}

/// Spec 0016 §Model and flow 1, threat model 0005 (TM-plug-01, -13, -15):
/// what makes a manifest usable, and every reason that makes it unusable.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use std::fs;

    /// The manifest of spec 0016 §Model, as written there.
    const SKETCH: &str = r#"
api: 1
id: docker                      # equals the folder name
name: Docker
version: 0.1.0
run: [./tern-docker]            # argv; first entry relative to the folder or on PATH
rights: [pane_info, terminal_write]
events: [pane_opened, pane_closed]
commands: [{ id: containers, title: "Docker: show containers" }]
panels: [{ id: containers, title: Containers, icon: container }]
settings: [{ key: socket, label: Docker socket, type: text }]
"#;

    /// A minimal usable manifest for the plugin in folder `id`.
    fn minimal(id: &str, name: &str) -> String {
        format!("api: 1\nid: {id}\nname: {name}\nrun: [./program]\n")
    }

    /// `root/<id>/plugin.yaml` holding `text`.
    fn folder(root: &TempDir, id: &str, text: &str) {
        let folder = root.path().join(id);
        fs::create_dir_all(&folder).expect("create the plugin's folder");
        fs::write(folder.join("plugin.yaml"), text).expect("write plugin.yaml");
    }

    /// Whether plugin `id` of `found` is usable.
    fn usable(found: &[Found], id: &str) -> bool {
        let plugin = found.iter().find(|plugin| plugin.id == id);
        plugin.expect("the folder is listed").manifest.is_ok()
    }

    #[test]
    fn the_specs_manifest_is_usable_and_declares_what_it_says() {
        let manifest = parse("docker", SKETCH).expect("the spec's sketch is usable");
        assert_eq!(manifest.name, "Docker");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.run, ["./tern-docker"]);
        assert_eq!(manifest.rights, [Right::PaneInfo, Right::TerminalWrite]);
        assert_eq!(
            manifest.events,
            [Subscription::PaneOpened, Subscription::PaneClosed]
        );
        let ids = |named: &[Named]| -> Vec<String> {
            named.iter().map(|named| named.id.clone()).collect()
        };
        assert_eq!(ids(&manifest.commands), ["containers"]);
        assert_eq!(manifest.commands[0].title, "Docker: show containers");
        assert_eq!(ids(&manifest.panels), ["containers"]);
        assert_eq!(manifest.panels[0].title, "Containers");
        assert_eq!(manifest.settings.len(), 1);
        assert_eq!(manifest.settings[0].key, "socket");
        assert_eq!(manifest.settings[0].kind, FieldKind::Text);
    }

    /// Only `api`, `id`, `name` and `run` say what a plugin is; one that
    /// declares nothing else asks for nothing.
    #[test]
    fn a_manifest_that_declares_no_rights_or_events_has_none() {
        let manifest = parse("plain", &minimal("plain", "Plain")).expect("usable");
        assert!(manifest.rights.is_empty());
        assert!(manifest.events.is_empty());
        assert!(manifest.commands.is_empty());
        assert!(manifest.panels.is_empty());
    }

    /// Flow 1: "an `id` that differs from its folder" is unusable.
    #[test]
    fn an_id_that_differs_from_the_folder_is_unusable() {
        assert!(parse("docker", SKETCH).is_ok());
        assert!(parse("docker2", SKETCH).is_err());
        assert!(parse("Docker", SKETCH).is_err());
    }

    /// AC1, flow 1: an unknown `api` is unusable, whatever else the
    /// manifest holds: a later version may have any shape.
    #[test]
    fn an_unknown_api_is_unusable() {
        for api in ["2", "0", "99"] {
            let same_shape = SKETCH.replace("api: 1", &format!("api: {api}"));
            assert!(parse("docker", &same_shape).is_err(), "api {api}");
        }
        let other_shape = "api: 2\nplugin: { id: docker }\nentry: main.wasm\n";
        assert!(parse("docker", other_shape).is_err());
        let no_api = SKETCH.replace("api: 1\n", "");
        assert!(parse("docker", &no_api).is_err());
    }

    /// Flow 1, TM-plug-13: a broken manifest is unusable with a reason,
    /// never a panic.
    #[test]
    fn a_broken_manifest_is_unusable_with_a_reason() {
        let broken = [
            "",
            "api: 1\nid: docker\nname: {oops\n",
            "- api: 1\n- id: docker\n",
            "just text",
            // No `run`: nothing to start.
            "api: 1\nid: docker\nname: Docker\n",
            "api: 1\nid: docker\nname: Docker\nrun: []\n",
            // No `name`: nothing to show.
            "api: 1\nid: docker\nrun: [./tern-docker]\n",
            "api: 1\nid: docker\nname: Docker\nrun: [./x]\nrights: [root]\n",
            "api: 1\nid: docker\nname: Docker\nrun: [./x]\nevents: [keystroke]\n",
        ];
        for text in broken {
            let reason = parse("docker", text).expect_err(text);
            assert!(!reason.is_empty(), "no reason for {text:?}");
        }
    }

    /// TM-plug-13: a YAML alias bomb ends in a reason, not in a stall.
    #[test]
    fn an_alias_bomb_is_unusable() {
        let mut text = String::from("api: 1\nid: bomb\nname: Bomb\nrun: [./x]\na0: &a0 [x, x]\n");
        for level in 1..40 {
            let below = level - 1;
            text.push_str(&format!(
                "a{level}: &a{level} [*a{below}, *a{below}, *a{below}, *a{below}]\n"
            ));
        }
        text.push_str("commands: *a39\n");
        assert!(parse("bomb", &text).is_err());
    }

    /// §Model, TM-plug-01: strings render by the title rules, capped at 64
    /// characters.
    #[test]
    fn names_and_titles_are_plain_text_of_at_most_64_characters() {
        let long = "x".repeat(100);
        let text = format!(
            "api: 1\nid: docker\nname: \"Do\\e[31mck\\ber\\u202E {long}\"\nrun: [./x]\n\
             commands: [{{ id: c, title: \"{long}\\a\" }}]\n\
             panels: [{{ id: p, title: \"\\e]0;spoof\\a{long}\" }}]\n"
        );
        let manifest = parse("docker", &text).expect("usable");
        let shown = [
            &manifest.name,
            &manifest.commands[0].title,
            &manifest.panels[0].title,
        ];
        for text in shown {
            assert!(text.chars().count() <= 64, "{text:?} is longer than 64");
            assert!(!text.is_empty());
            assert!(
                !text.chars().any(|c| c.is_control() || c == '\u{202E}'),
                "{text:?} isn't plain"
            );
        }
        assert_eq!(manifest.commands[0].title, "x".repeat(64));
    }

    /// Platform notes: "a manifest may give `run` per OS".
    #[test]
    fn run_per_os_is_the_command_line_of_this_os() {
        let text = concat!(
            "api: 1\nid: docker\nname: Docker\n",
            "run:\n",
            "  linux: [./tern-docker]\n",
            "  windows: [tern-docker.exe, --stdio]\n",
            "  macos: [./tern-docker-mac]\n",
        );
        let manifest = parse("docker", text).expect("usable");
        let expected: &[&str] = if cfg!(windows) {
            &["tern-docker.exe", "--stdio"]
        } else if cfg!(target_os = "macos") {
            &["./tern-docker-mac"]
        } else {
            &["./tern-docker"]
        };
        assert_eq!(manifest.run, expected);
    }

    /// A plugin with no command line for this OS has nothing to start.
    #[test]
    fn run_for_other_systems_only_is_unusable() {
        let other = if cfg!(windows) { "linux" } else { "windows" };
        let text = format!("api: 1\nid: docker\nname: Docker\nrun:\n  {other}: [./tern-docker]\n");
        assert!(parse("docker", &text).is_err());
    }

    /// §Model: the first entry of `run` is "relative to the folder or on
    /// PATH" (TM-plug-15): `./` and a path can't leave the folder.
    #[test]
    fn a_program_path_that_leaves_the_folder_is_unusable() {
        let escaping = ["./../evil", "./bin/../../evil", "../evil", "./..", "..", ""];
        for program in escaping {
            let text = format!("api: 1\nid: docker\nname: Docker\nrun: [\"{program}\", arg]\n");
            assert!(parse("docker", &text).is_err(), "{program:?} is usable");
        }
        for program in ["./tern-docker", "./bin/tern-docker", "python"] {
            let text = format!("api: 1\nid: docker\nname: Docker\nrun: [\"{program}\", arg]\n");
            assert!(parse("docker", &text).is_ok(), "{program:?} is unusable");
        }
    }

    /// TM-plug-15: `./program` is the file in the plugin's folder; a bare
    /// name is left for `PATH`, never joined to the folder; the arguments
    /// are passed as written.
    #[test]
    fn a_program_resolves_inside_the_folder_or_stays_a_bare_name() {
        let folder = Path::new("plugins").join("docker");
        let run = |program: &str| {
            let text = format!(
                "api: 1\nid: docker\nname: Docker\nrun: [\"{program}\", main.py, \"a b\"]\n"
            );
            parse("docker", &text).expect("usable")
        };
        let inside = run("./tern-docker");
        let (program, args) = inside.command(&folder);
        assert_eq!(program, folder.join("tern-docker"));
        assert_eq!(args, ["main.py", "a b"]);

        let on_path = run("python");
        let (program, args) = on_path.command(&folder);
        assert_eq!(program, Path::new("python"));
        assert_eq!(args, ["main.py", "a b"]);
    }

    /// Flow 1: every folder is listed, usable or not.
    #[test]
    fn discovery_lists_every_folder_with_its_manifest_or_the_reason() {
        let root = TempDir::new("plugins-discover-all");
        folder(&root, "docker", SKETCH);
        folder(
            &root,
            "future",
            "api: 2\nid: future\nname: Future\nrun: [./x]\n",
        );
        folder(&root, "moved", &minimal("original", "Moved"));
        folder(&root, "broken", "api: 1\nname: {oops\n");
        fs::write(root.path().join("a-file"), "not a plugin").expect("write a file");

        let found = discover(root.path());
        let ids: Vec<&str> = found.iter().map(|plugin| plugin.id.as_str()).collect();
        assert_eq!(ids, ["broken", "docker", "future", "moved"]);
        assert!(usable(&found, "docker"));
        for id in ["broken", "future", "moved"] {
            assert!(!usable(&found, id), "{id} is usable");
        }
        let docker = found.iter().find(|plugin| plugin.id == "docker");
        assert_eq!(
            docker.expect("listed").folder,
            root.path().join("docker"),
            "the folder a plugin runs in"
        );
    }

    /// No plugins folder yet means no plugins, not an error.
    #[test]
    fn a_missing_plugins_folder_holds_no_plugins() {
        let root = TempDir::new("plugins-discover-missing");
        assert_eq!(discover(&root.path().join("plugins")), []);
    }

    /// §Model, TM-plug-13: "A manifest over 64 KiB is unusable without
    /// being parsed"; one of exactly 64 KiB isn't over.
    #[test]
    fn a_manifest_over_64_kib_is_unusable() {
        let padded = |bytes: usize| {
            let mut text = minimal("big", "Big");
            text.push('#');
            while text.len() < bytes - 1 {
                text.push('x');
            }
            text.push('\n');
            assert_eq!(text.len(), bytes);
            text
        };
        let root = TempDir::new("plugins-discover-size");
        folder(&root, "big", &padded(64 * 1024));
        assert!(usable(&discover(root.path()), "big"), "64 KiB is not over");
        folder(&root, "big", &padded(64 * 1024 + 1));
        assert!(!usable(&discover(root.path()), "big"));
        // Far over, and no YAML at all: still only a reason.
        folder(&root, "big", &"{".repeat(4 * 1024 * 1024));
        assert!(!usable(&discover(root.path()), "big"));
    }

    /// §Model, TM-plug-01: a `name` equal, ignoring case, to another
    /// plugin's makes "both plugins" unusable; the others stay usable.
    #[test]
    fn plugins_sharing_a_name_ignoring_case_are_both_unusable() {
        let root = TempDir::new("plugins-discover-names");
        folder(&root, "docker", &minimal("docker", "Docker"));
        folder(&root, "imposter", &minimal("imposter", "dOCKER"));
        folder(&root, "other", &minimal("other", "Other"));
        let found = discover(root.path());
        assert!(!usable(&found, "docker"));
        assert!(!usable(&found, "imposter"));
        assert!(usable(&found, "other"));
    }

    /// §Model, TM-plug-01: no plugin is named `Tern`, in any case.
    #[test]
    fn a_plugin_named_tern_is_unusable() {
        let root = TempDir::new("plugins-discover-tern");
        for (id, name) in [("a", "Tern"), ("b", "tern"), ("c", "TERN")] {
            folder(&root, id, &minimal(id, name));
        }
        folder(&root, "d", &minimal("d", "Ternary"));
        let found = discover(root.path());
        for id in ["a", "b", "c"] {
            assert!(!usable(&found, id), "{id} is usable");
        }
        assert!(usable(&found, "d"));
    }

    /// An unusable plugin's name isn't taken: it can't make a usable one
    /// unusable.
    #[test]
    fn an_unusable_plugin_does_not_take_its_name_from_a_usable_one() {
        let root = TempDir::new("plugins-discover-unusable-name");
        folder(&root, "docker", &minimal("docker", "Docker"));
        folder(
            &root,
            "future",
            "api: 2\nid: future\nname: Docker\nrun: [./x]\n",
        );
        let found = discover(root.path());
        assert!(usable(&found, "docker"));
        assert!(!usable(&found, "future"));
    }
}
