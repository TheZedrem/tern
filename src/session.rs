//! The session (spec 0003): the window's size and its tabs' layout, with
//! each pane's profile, directory and title (a files pane's target and
//! folder, spec 0013), kept in `session.json` so a relaunch comes back
//! the way the app was left. Never scrollback, commands or the
//! environment [load-bearing].

use crate::layout::tree::{Axis, LeafId, Node};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The format this build reads and writes. A file of another version is set
/// aside, not guessed at (spec 0003, flow 7).
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    /// The window's size in logical pixels.
    pub window: Option<WindowSize>,
    pub tabs: Vec<SavedTab>,
    /// The tab shown, an index into `tabs`.
    pub active: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
}

impl WindowSize {
    /// The smallest window, in logical pixels. Anything smaller that the
    /// platform reports is bogus, like the 120 × 0 a minimized window once
    /// left on Windows, and is neither saved nor restored.
    pub const MIN: Self = Self {
        width: 400.0,
        height: 240.0,
    };

    /// This size, if a window can have it.
    pub fn usable(self) -> Option<Self> {
        (self.width >= Self::MIN.width && self.height >= Self::MIN.height).then_some(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    /// The user's name for the tab.
    pub name: Option<String>,
    pub root: SavedNode,
    pub focused: LeafId,
    pub zoomed: Option<LeafId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedNode {
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
    Leaf(SavedLeaf),
}

/// A pane as saved: what it ran and where, never what it showed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedLeaf {
    pub id: LeafId,
    /// The ID of its profile.
    pub profile: String,
    pub cwd: Option<PathBuf>,
    /// The title its program set last.
    pub title: Option<String>,
    /// A files pane instead of a terminal (spec 0013, Saved); `profile`,
    /// `cwd` and `title` then go unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<SavedFiles>,
}

/// A files pane as saved: the target it showed and its folder, nothing
/// else (spec 0013, Saved).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedFiles {
    /// The SSH profile it showed; `None` for Local.
    pub target: Option<String>,
    pub folder: Option<String>,
}

impl Session {
    pub fn new(window: Option<WindowSize>, tabs: Vec<SavedTab>, active: usize) -> Self {
        Self {
            version: VERSION,
            window,
            tabs,
            active,
        }
    }

    /// Repairs what a restore can live with: a focused, zoomed or shown
    /// tab that isn't there, a divider off the edge. Refuses what it can't:
    /// a pane saved twice.
    fn check(&mut self) -> Result<(), String> {
        let mut seen = HashSet::new();
        for tab in &mut self.tabs {
            let ids: Vec<LeafId> = tab.root.leaves().iter().map(|leaf| leaf.id).collect();
            if let Some(twice) = ids.iter().find(|&&id| !seen.insert(id)) {
                return Err(format!("pane {twice} is saved twice"));
            }
            if !ids.contains(&tab.focused) {
                tab.focused = ids[0];
            }
            if tab.zoomed.is_some_and(|zoomed| !ids.contains(&zoomed)) {
                tab.zoomed = None;
            }
            tab.root.clamp_ratios();
        }
        if self.active >= self.tabs.len() {
            self.active = 0;
        }
        Ok(())
    }
}

impl SavedNode {
    /// Saves `node`, asking `leaf` for each pane's details.
    pub fn of(node: &Node, leaf: &mut impl FnMut(LeafId) -> SavedLeaf) -> Self {
        match node {
            Node::Leaf(id) => SavedNode::Leaf(leaf(*id)),
            Node::Split {
                axis,
                ratio,
                first,
                second,
            } => SavedNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(Self::of(first, leaf)),
                second: Box::new(Self::of(second, leaf)),
            },
        }
    }

    /// The split tree this saved.
    pub fn tree(&self) -> Node {
        match self {
            SavedNode::Leaf(leaf) => Node::Leaf(leaf.id),
            SavedNode::Split {
                axis,
                ratio,
                first,
                second,
            } => Node::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(first.tree()),
                second: Box::new(second.tree()),
            },
        }
    }

    /// Its panes, in reading order.
    pub fn leaves(&self) -> Vec<&SavedLeaf> {
        match self {
            SavedNode::Leaf(leaf) => vec![leaf],
            SavedNode::Split { first, second, .. } => {
                let mut leaves = first.leaves();
                leaves.extend(second.leaves());
                leaves
            }
        }
    }

    fn clamp_ratios(&mut self) {
        if let SavedNode::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            *ratio = if ratio.is_finite() {
                ratio.clamp(0.05, 0.95)
            } else {
                0.5
            };
            first.clamp_ratios();
            second.clamp_ratios();
        }
    }
}

/// `<data_local_dir>/tern/session.json`.
pub fn path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("tern").join("session.json"))
}

/// Reads the session at `path`: `Ok(None)` if there is none yet, `Err`
/// saying why for a file that can't be restored.
pub fn load(path: &Path) -> Result<Option<Session>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// A session from its JSON, checked (spec 0003, flow 7: a corrupt file or
/// one of another version can't be restored).
pub fn parse(text: &str) -> Result<Session, String> {
    #[derive(Deserialize)]
    struct Versioned {
        version: u32,
    }
    let Versioned { version } = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if version != VERSION {
        return Err(format!(
            "it's version {version}, and this build reads version {VERSION}"
        ));
    }
    let mut session: Session = serde_json::from_str(text).map_err(|error| error.to_string())?;
    session.check()?;
    Ok(session)
}

/// Sets a session that can't be restored aside as `session.json.bad`,
/// replacing an older one, so the next save doesn't silently discard it
/// (spec 0003, flow 7).
pub fn keep_bad(path: &Path) -> io::Result<()> {
    std::fs::rename(path, path.with_extension("json.bad"))
}

/// Writes `session` to `path` atomically, through a temp file beside it,
/// readable by the user only: titles and directories can be private.
pub fn save(path: &Path, session: &Session) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(session)?;
    let temp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temp)?;
    file.write_all(&json)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)
}

/// Deletes the saved session, as `restore_session: false` asks (spec 0003).
pub fn delete(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use std::collections::BTreeSet;
    use std::fs;

    #[test]
    fn a_window_smaller_than_the_minimum_is_not_usable() {
        let size = |width, height| WindowSize { width, height };
        assert_eq!(size(120.0, 0.0).usable(), None);
        assert_eq!(size(1000.0, 100.0).usable(), None);
        assert_eq!(WindowSize::MIN.usable(), Some(WindowSize::MIN));
        assert_eq!(size(1000.0, 640.0).usable(), Some(size(1000.0, 640.0)));
    }

    /// A saved pane. Shared with the tests of the modules that restore one.
    pub(crate) fn leaf(
        id: LeafId,
        profile: &str,
        cwd: Option<&str>,
        title: Option<&str>,
    ) -> SavedNode {
        SavedNode::Leaf(SavedLeaf {
            id,
            profile: profile.into(),
            cwd: cwd.map(PathBuf::from),
            title: title.map(str::to_owned),
            files: None,
        })
    }

    /// A saved files pane on SSH profile `target` (`None`: Local) at
    /// `folder`. Shared with the tests of the modules that restore one.
    pub(crate) fn files_leaf(id: LeafId, target: Option<&str>, folder: Option<&str>) -> SavedNode {
        SavedNode::Leaf(SavedLeaf {
            id,
            profile: "shell".into(),
            cwd: None,
            title: None,
            files: Some(SavedFiles {
                target: target.map(str::to_owned),
                folder: folder.map(str::to_owned),
            }),
        })
    }

    /// A saved split.
    pub(crate) fn split(axis: Axis, ratio: f32, first: SavedNode, second: SavedNode) -> SavedNode {
        SavedNode::Split {
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// A saved tab of `root`, focused on `focused`, unnamed and not zoomed.
    pub(crate) fn tab(root: SavedNode, focused: LeafId) -> SavedTab {
        SavedTab {
            name: None,
            root,
            focused,
            zoomed: None,
        }
    }

    /// Two tabs, the second one shown: a named tab of three panes, zoomed on
    /// its focused one, and an SSH host's.
    fn two_tabs() -> Session {
        let panes = split(
            Axis::Row,
            0.3,
            leaf(1, "shell", Some("/home/me/notes"), Some("vim todo.md")),
            split(
                Axis::Column,
                0.6,
                leaf(2, "htop", None, Some("htop")),
                leaf(3, "shell", Some("/tmp"), None),
            ),
        );
        let work = SavedTab {
            name: Some("work".into()),
            zoomed: Some(2),
            ..tab(panes, 2)
        };
        let prod = tab(leaf(4, "ssh:prod", None, Some("me@prod: ~")), 4);
        let window = WindowSize {
            width: 1200.0,
            height: 800.0,
        };
        Session::new(Some(window), vec![work, prod], 1)
    }

    /// `session` as JSON.
    fn json(session: &Session) -> String {
        serde_json::to_string(session).expect("serialize the session")
    }

    /// The first half of a session, as a crash or a full disk leaves it.
    fn truncated() -> String {
        let text = json(&two_tabs());
        text[..text.len() / 2].to_owned()
    }

    /// Every key of every object in `value`.
    fn keys(value: &serde_json::Value) -> BTreeSet<String> {
        match value {
            serde_json::Value::Object(map) => map
                .iter()
                .flat_map(|(key, value)| keys(value).into_iter().chain([key.clone()]))
                .collect(),
            serde_json::Value::Array(items) => items.iter().flat_map(keys).collect(),
            _ => BTreeSet::new(),
        }
    }

    /// Spec 0003 §Persistence and AC3: a save goes through a temp file and a
    /// rename, so it replaces the last session whole and leaves nothing
    /// beside it, and it loads back with its window size, layout and titles.
    #[test]
    fn a_save_replaces_the_last_session_whole_and_loads_back_as_saved() {
        let root = TempDir::new("session-save");
        let dir = root.path().join("tern");
        let path = dir.join("session.json");
        // A longer session first: anything left of it would show.
        let mut longer = two_tabs();
        longer.tabs[0].name = Some("x".repeat(10_000));
        save(&path, &longer).expect("save the first session");
        save(&path, &two_tabs()).expect("save over it");
        assert_eq!(load(&path), Ok(Some(two_tabs())));
        let files: Vec<String> = fs::read_dir(&dir)
            .expect("list the data dir")
            .map(|entry| entry.expect("a dir entry").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, ["session.json"]);
    }

    /// AC8 (spec 0003), §Security & privacy: the file holds the window size,
    /// the layout, and each pane's profile ID, cwd and title. Nothing else,
    /// so never scrollback, commands or the environment.
    #[test]
    fn the_file_holds_only_the_window_size_layout_and_each_panes_profile_cwd_and_title() {
        let root = TempDir::new("session-contents");
        let path = root.path().join("session.json");
        save(&path, &two_tabs()).expect("save");
        let text = fs::read_to_string(&path).expect("read session.json");
        let file = serde_json::from_str(&text).expect("session.json is JSON");
        // The format, the window and the tabs' layout.
        let layout = [
            "version", "window", "width", "height", "tabs", "active", "name", "root", "focused",
            "zoomed", "split", "axis", "ratio", "first", "second",
        ];
        let pane = ["leaf", "id", "profile", "cwd", "title"];
        let other: Vec<String> = keys(&file)
            .into_iter()
            .filter(|key| !layout.contains(&key.as_str()) && !pane.contains(&key.as_str()))
            .collect();
        assert!(other.is_empty(), "session.json also holds {other:?}");
    }

    /// Spec 0013, Saved: a files pane, on Local or on a host, saves and
    /// loads back as it was, beside terminal panes.
    #[test]
    fn a_files_pane_saves_and_loads_back_with_its_target_and_folder() {
        let root = TempDir::new("session-files");
        let path = root.path().join("session.json");
        let mut session = two_tabs();
        session.tabs[1] = tab(
            split(
                Axis::Row,
                0.5,
                leaf(4, "ssh:prod", None, None),
                files_leaf(5, Some("ssh:prod"), Some("/var/log")),
            ),
            5,
        );
        session
            .tabs
            .push(tab(files_leaf(6, None, Some("/home/me/notes")), 6));
        save(&path, &session).expect("save");
        assert_eq!(load(&path), Ok(Some(session)));
    }

    /// Spec 0013, Saved and §Security & privacy: a files pane adds only its
    /// target and folder to the file: no expanded folders, transfers,
    /// edits or other targets.
    #[test]
    fn a_files_pane_saves_only_its_target_and_folder() {
        let session = Session::new(
            None,
            vec![tab(files_leaf(1, Some("prod"), Some("/srv")), 1)],
            0,
        );
        let file: serde_json::Value =
            serde_json::from_str(&json(&session)).expect("session.json is JSON");
        let saved = &file["tabs"][0]["root"]["leaf"]["files"];
        assert_eq!(
            keys(saved),
            BTreeSet::from(["target".into(), "folder".into()])
        );
        assert_eq!(saved["target"], "prod");
        assert_eq!(saved["folder"], "/srv");
    }

    /// Spec 0013, Saved: a session saved before files panes existed, whose
    /// panes have no `files` key, still loads, all of them terminals.
    #[test]
    fn a_session_saved_before_files_panes_loads_with_terminal_panes() {
        let text = r#"{"version":1,"window":null,"tabs":[{"name":null,"root":{"leaf":{"id":1,"profile":"shell","cwd":"/tmp","title":"vim"}},"focused":1,"zoomed":null}],"active":0}"#;
        let session = parse(text).expect("an older session loads");
        let leaves = session.tabs[0].root.leaves();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].profile, "shell");
        assert_eq!(leaves[0].files, None);
    }

    /// Spec 0003, flow 7, and spec 0001, flow 3: before the first save there
    /// is no session, which is no error (boot opens one fresh tab), and
    /// looking for it writes nothing.
    #[test]
    fn no_session_file_is_no_session_and_looking_for_it_creates_nothing() {
        let root = TempDir::new("session-missing");
        let dir = root.path().join("tern");
        assert_eq!(load(&dir.join("session.json")), Ok(None));
        assert!(!dir.exists(), "{} was created", dir.display());
    }

    /// AC6 (spec 0003), flow 7: a truncated session, one of another version
    /// and one with a pane in two places can't be restored. Leaf IDs are
    /// stable and unique (§Model), so a doubled pane makes a session corrupt.
    #[test]
    fn a_truncated_newer_or_doubled_pane_session_cant_be_restored() {
        let mut newer = two_tabs();
        newer.version += 1;
        let mut in_two_tabs = two_tabs();
        in_two_tabs.tabs[1] = tab(leaf(1, "shell", None, None), 1);
        let mut twice_in_a_tab = two_tabs();
        let twice = split(
            Axis::Row,
            0.5,
            leaf(4, "shell", None, None),
            leaf(4, "shell", None, None),
        );
        twice_in_a_tab.tabs[1] = tab(twice, 4);
        let cases = [
            ("truncated", truncated()),
            ("newer", json(&newer)),
            ("pane in two tabs", json(&in_two_tabs)),
            ("pane twice in a tab", json(&twice_in_a_tab)),
        ];
        for (case, text) in cases {
            assert!(parse(&text).is_err(), "the {case} session was restored");
        }
    }

    /// AC6 (spec 0003), flow 7: a session that can't be restored is kept as
    /// `session.json.bad`, replacing an older one, so the next save doesn't
    /// discard it, and the next boot finds no session.
    #[test]
    fn a_session_that_cant_be_restored_is_kept_as_session_json_bad() {
        let root = TempDir::new("session-bad");
        let path = root.path().join("session.json");
        let bad = root.path().join("session.json.bad");
        fs::write(&bad, "an older bad session").expect("write an older .bad");
        fs::write(&path, truncated()).expect("write a truncated session");
        assert!(load(&path).is_err(), "the truncated session was restored");
        keep_bad(&path).expect("keep it as .bad");
        let kept = fs::read_to_string(&bad).expect("read the .bad");
        assert_eq!(kept, truncated());
        assert_eq!(load(&path), Ok(None));
    }

    /// Spec 0003, flow 7: what a restore can live with is repaired, not
    /// refused. A focused or zoomed pane that isn't in its tab and a shown
    /// tab past the end come back as ones that are there, and a divider at
    /// or past an edge moves inside it, leaving both of its panes room
    /// (§Model).
    #[test]
    fn dangling_references_and_off_edge_dividers_are_repaired_not_refused() {
        for ratio in [-0.2, 0.0, 1.0, 1.7] {
            let mut session = two_tabs();
            session.active = 2;
            let work = &mut session.tabs[0];
            (work.focused, work.zoomed) = (99, Some(98));
            if let SavedNode::Split { ratio: saved, .. } = &mut work.root {
                *saved = ratio;
            }
            let repaired = parse(&json(&session)).expect("repaired, not refused");
            assert!(repaired.active < repaired.tabs.len());
            let work = &repaired.tabs[0];
            let panes: Vec<LeafId> = work.root.leaves().iter().map(|leaf| leaf.id).collect();
            assert_eq!(panes, [1, 2, 3]);
            assert!(panes.contains(&work.focused), "focused: {}", work.focused);
            assert!(
                work.zoomed.is_none_or(|zoomed| panes.contains(&zoomed)),
                "zoomed: {:?}",
                work.zoomed
            );
            let SavedNode::Split { ratio: moved, .. } = &work.root else {
                panic!("the tab lost its split");
            };
            assert!(
                0.0 < *moved && *moved < 1.0,
                "a divider at {ratio} came back at {moved}"
            );
        }
    }

    /// Spec 0003 §Security & privacy: `restore_session: false` deletes the
    /// saved session on quit, and there being none, as on every quit after
    /// the first, is fine.
    #[test]
    fn deleting_the_session_removes_it_and_is_fine_without_one() {
        let root = TempDir::new("session-delete");
        let path = root.path().join("session.json");
        save(&path, &two_tabs()).expect("save");
        delete(&path).expect("delete the session");
        assert!(!path.exists(), "the session is still there");
        delete(&path).expect("delete no session");
    }

    /// Spec 0003 §Security & privacy: titles and directories can be private,
    /// so the saved session is readable by its owner only, even where it
    /// replaces a file others could read.
    #[cfg(unix)]
    #[test]
    fn the_saved_session_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = TempDir::new("session-mode");
        let path = root.path().join("session.json");
        let mode = |path: &Path| {
            let metadata = fs::metadata(path).expect("stat session.json");
            metadata.permissions().mode() & 0o777
        };
        save(&path, &two_tabs()).expect("save a first session");
        assert_eq!(mode(&path), 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod 644");
        save(&path, &two_tabs()).expect("save over it");
        assert_eq!(mode(&path), 0o600);
    }
}
