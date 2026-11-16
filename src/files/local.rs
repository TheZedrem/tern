//! The Local target (spec 0013): this machine's disk, browsed on a thread
//! of its own so nothing reads the disk on the UI thread. Its [`Worker`]
//! answers with the same [`Event`]s an SFTP connection does. Nothing
//! watches the disk: a folder changes on a refresh and on the browser's
//! own writes only. Also here: local paths as the path field takes them,
//! and what opening a local file does (§Security: a click never runs a
//! program).

use super::permissions::{Change, Perms};
use super::{Entry, Event, Kind, Listing, MAX_ENTRIES};
use crate::terminal::pty::Command;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::fs;
use std::io;
use std::path::{Component, MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};
use std::sync::mpsc;
use std::time::UNIX_EPOCH;

/// What the browser asks of the disk. Paths are absolute local paths.
enum Request {
    List(String),
    Rename {
        from: String,
        to: String,
        replace: bool,
    },
    /// Move a file, a link (never followed) or a folder with all it holds
    /// to the OS's trash.
    Trash(String),
    /// Remove a file, a link (never followed) or an empty folder for good.
    Delete(String),
    MakeFolder(String),
    /// Apply the permissions panel's change (spec 0013, flow 10).
    SetPerms {
        path: String,
        change: Change,
    },
    /// Open Explorer's properties dialog (Windows).
    #[cfg(windows)]
    Properties(String),
}

/// The browser's end of the disk's thread, which ends when this drops.
pub struct Worker {
    requests: mpsc::Sender<Request>,
}

impl Worker {
    /// Starts the thread. Its first event is [`Event::Connected`] with the
    /// home folder.
    pub fn start() -> (Self, UnboundedReceiver<Event>) {
        let (requests, requests_rx) = mpsc::channel();
        let (events, events_rx) = unbounded();
        let thread_events = events.clone();
        let started = std::thread::Builder::new()
            .name("files".into())
            .spawn(move || serve(&requests_rx, &thread_events));
        if let Err(error) = started {
            let reason = format!("Couldn't read the disk: {error}");
            let _ = events.unbounded_send(Event::Ended(reason));
        }
        (Self { requests }, events_rx)
    }

    pub fn list(&self, path: String) {
        let _ = self.requests.send(Request::List(path));
    }

    /// Renames `from` to `to`. Unless `replace`, an existing `to` is left
    /// alone and [`Event::RenameExists`] says so.
    pub fn rename(&self, from: String, to: String, replace: bool) {
        let _ = self.requests.send(Request::Rename { from, to, replace });
    }

    /// Moves `path` to the trash; when the OS can't, [`Event::TrashFailed`]
    /// says why and nothing changed (spec 0013, flow 5).
    pub fn trash(&self, path: String) {
        let _ = self.requests.send(Request::Trash(path));
    }

    pub fn delete(&self, path: String) {
        let _ = self.requests.send(Request::Delete(path));
    }

    pub fn make_folder(&self, path: String) {
        let _ = self.requests.send(Request::MakeFolder(path));
    }

    pub fn set_perms(&self, path: String, change: Change) {
        let _ = self.requests.send(Request::SetPerms { path, change });
    }

    /// Opens Explorer's properties dialog for `path`; only a failure
    /// reports.
    #[cfg(windows)]
    pub fn properties(&self, path: String) {
        let _ = self.requests.send(Request::Properties(path));
    }
}

/// Answers requests until the browser drops its [`Worker`].
fn serve(requests: &mpsc::Receiver<Request>, events: &UnboundedSender<Event>) {
    let _ = events.unbounded_send(Event::Connected { home: home() });
    while let Ok(request) = requests.recv() {
        let event = match request {
            Request::List(path) => match list(&path) {
                Ok(listing) => Event::Listed(listing),
                Err(error) => Event::ListFailed {
                    path,
                    error: error.to_string(),
                },
            },
            Request::Rename { from, to, replace } => {
                // The look and the rename are two calls; a name taken in
                // between is replaced or refused as the OS does (flow 5).
                if !replace && fs::symlink_metadata(&to).is_ok() {
                    Event::RenameExists { from, to }
                } else {
                    changed(fs::rename(&from, &to), &to)
                }
            }
            Request::Trash(path) => match trash::delete(&path) {
                Ok(()) => Event::Changed {
                    folder: parent(&path),
                },
                Err(error) => Event::TrashFailed {
                    path,
                    error: trash_error(error),
                },
            },
            Request::Delete(path) => changed(delete(Path::new(&path)), &path),
            Request::MakeFolder(path) => changed(fs::create_dir(&path), &path),
            Request::SetPerms { path, change } => changed(set_perms(&path, change), &path),
            #[cfg(windows)]
            Request::Properties(path) => match super::permissions::show_properties(&path) {
                Ok(()) => continue,
                Err(error) => Event::Failed(error),
            },
        };
        if events.unbounded_send(event).is_err() {
            return;
        }
    }
}

/// What a change to `path` reports: its folder changed, or why it failed.
fn changed(done: io::Result<()>, path: &str) -> Event {
    match done {
        Ok(()) => Event::Changed {
            folder: parent(path),
        },
        Err(error) => Event::Failed(error.to_string()),
    }
}

/// Why the trash refused, in words: the crate's own messages are its
/// debug output.
fn trash_error(error: trash::Error) -> String {
    match error {
        #[cfg(all(unix, not(target_os = "macos")))]
        trash::Error::FileSystem { source, .. } => source.to_string(),
        trash::Error::Os { description, .. } | trash::Error::Unknown { description } => description,
        trash::Error::TargetedRoot => "a root folder can't go to the trash".into(),
        trash::Error::CouldNotAccess { .. } | trash::Error::CanonicalizePath { .. } => {
            "it can't be reached".into()
        }
        other => format!("{other:?}"),
    }
}

/// The user's home folder, else the root.
fn home() -> String {
    dirs::home_dir()
        .and_then(|home| home.to_str().map(str::to_owned))
        .unwrap_or_else(|| normalize(Path::new(MAIN_SEPARATOR_STR)))
}

/// Lists the folder at `path`: at most [`MAX_ENTRIES`] entries, links
/// resolved to what they point to, sorted.
// TODO(OQ-028): names that aren't UTF-8 are left out, since a lossy name
// would act on the wrong file.
fn list(path: &str) -> io::Result<Listing> {
    let mut entries = Vec::new();
    let mut names = Names::default();
    let mut truncated = false;
    for dir_entry in fs::read_dir(path)? {
        let Ok(dir_entry) = dir_entry else {
            continue;
        };
        let Ok(name) = dir_entry.file_name().into_string() else {
            continue;
        };
        if super::hidden(&name) {
            continue;
        }
        if entries.len() == MAX_ENTRIES {
            truncated = true;
            break;
        }
        entries.push(entry(&dir_entry, name, &mut names));
    }
    super::sort(&mut entries);
    Ok(Listing {
        path: path.to_owned(),
        entries,
        truncated,
    })
}

/// A directory entry as the listing shows it; a link shows what it
/// points to, and a broken one is `Other`.
fn entry(dir_entry: &fs::DirEntry, name: String, names: &mut Names) -> Entry {
    let link = dir_entry.file_type().is_ok_and(|kind| kind.is_symlink());
    // Follows a link, so its target decides the kind.
    let metadata = fs::metadata(dir_entry.path()).ok();
    let kind = match &metadata {
        Some(metadata) if metadata.is_dir() => Kind::Folder,
        Some(metadata) if metadata.is_file() => Kind::File,
        _ => Kind::Other,
    };
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| u32::try_from(since.as_secs()).unwrap_or(u32::MAX));
    // A link has no permissions of its own to change (spec 0013), so its
    // target's are never shown as its.
    let perms = match &metadata {
        Some(metadata) if !link => names.perms(metadata),
        _ => Perms::default(),
    };
    Entry {
        name,
        kind,
        link,
        size: metadata.as_ref().map(fs::Metadata::len),
        modified,
        perms,
    }
}

/// Owner and group names, looked up once per listing: a folder's entries
/// rarely have more than a few owners between them.
#[derive(Default)]
struct Names {
    #[cfg(unix)]
    users: std::collections::HashMap<u32, Option<String>>,
    #[cfg(unix)]
    groups: std::collections::HashMap<u32, Option<String>>,
}

impl Names {
    #[cfg(unix)]
    fn perms(&mut self, metadata: &fs::Metadata) -> Perms {
        use super::permissions::{Id, users};
        use std::os::unix::fs::MetadataExt;
        let (uid, gid) = (metadata.uid(), metadata.gid());
        let owner = self
            .users
            .entry(uid)
            .or_insert_with(|| users::user_name(uid));
        let owner = Id {
            id: uid,
            name: owner.clone(),
        };
        let group = self
            .groups
            .entry(gid)
            .or_insert_with(|| users::group_name(gid));
        let group = Id {
            id: gid,
            name: group.clone(),
        };
        Perms {
            mode: Some(metadata.mode() & 0o7777),
            owner: Some(owner),
            group: Some(group),
            readonly: None,
        }
    }

    #[cfg(windows)]
    fn perms(&mut self, metadata: &fs::Metadata) -> Perms {
        Perms {
            readonly: Some(metadata.permissions().readonly()),
            ..Perms::default()
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn perms(&mut self, _: &fs::Metadata) -> Perms {
        Perms::default()
    }
}

/// Applies the permissions panel's `change` to `path` (spec 0013, flow
/// 10): the mode, then the owner and group, by name through the user
/// database or by number.
#[cfg(unix)]
fn set_perms(path: &str, change: Change) -> io::Result<()> {
    use super::permissions::{Who, users};
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = change.mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    let id = |who: Option<Who>, lookup: fn(&str) -> Option<u32>, what: &str| match who {
        None => Ok(None),
        Some(Who::Number(id)) => Ok(Some(id)),
        Some(Who::Name(name)) => lookup(&name)
            .map(Some)
            .ok_or_else(|| io::Error::other(format!("There's no {what} named {name}"))),
    };
    let owner = id(change.owner, users::user_id, "user")?;
    let group = id(change.group, users::group_id, "group")?;
    if owner.is_some() || group.is_some() {
        std::os::unix::fs::chown(path, owner, group)?;
    }
    Ok(())
}

/// Applies the read-only flag, the one permission a Windows disk shows
/// (spec 0013, Permissions).
#[cfg(windows)]
fn set_perms(path: &str, change: Change) -> io::Result<()> {
    if let Some(readonly) = change.readonly {
        let mut permissions = fs::metadata(path)?.permissions();
        // On Windows this clears the read-only attribute and nothing else;
        // the lint is about Unix, where it would make a file world-writable.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(readonly);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn set_perms(_: &str, _: Change) -> io::Result<()> {
    Err(io::Error::other("Permissions aren't supported here"))
}

/// Removes what's at `path` without following a link (flow 5): a link
/// to a folder on Windows is a directory reparse point, which only
/// `remove_dir` takes; elsewhere a link is a file. A folder must be empty.
fn delete(path: &Path) -> io::Result<()> {
    let kind = fs::symlink_metadata(path)?.file_type();
    #[cfg(windows)]
    let folder = {
        use std::os::windows::fs::FileTypeExt;
        kind.is_dir() || kind.is_symlink_dir()
    };
    #[cfg(not(windows))]
    let folder = kind.is_dir();
    if folder {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

/// `name` inside local folder `folder`.
pub fn join(folder: &str, name: &str) -> String {
    Path::new(folder).join(name).to_string_lossy().into_owned()
}

/// The folder above `path`; a root (`/`, or a drive's on Windows) stays.
pub fn parent(path: &str) -> String {
    Path::new(path).parent().map_or_else(
        || path.to_owned(),
        |parent| parent.to_string_lossy().into_owned(),
    )
}

/// Where the path field's `input` goes on this machine: `~` and paths
/// under it, and relative paths, resolve against `home`; an absolute path,
/// or on Windows any drive (`D:\x`, `D:`), is used as it is. The result is
/// normalized without asking the disk.
pub fn resolve(home: &str, input: &str) -> String {
    let input = input.trim();
    let path = match input.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', MAIN_SEPARATOR]) => {
            PathBuf::from(format!("{home}{rest}"))
        }
        _ => {
            let typed = Path::new(input);
            let drive = matches!(typed.components().next(), Some(Component::Prefix(_)));
            if typed.has_root() || drive {
                typed.to_path_buf()
            } else {
                Path::new(home).join(typed)
            }
        }
    };
    normalize(&path)
}

/// An absolute path with `.`, `..` and repeated separators taken out; `..`
/// at a root stays there, and a bare drive is its root.
fn normalize(path: &Path) -> String {
    let mut prefix = None;
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(drive) => prefix = Some(drive.as_os_str()),
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(part) => parts.push(part),
        }
    }
    let mut normal = PathBuf::new();
    if let Some(prefix) = prefix {
        normal.push(prefix);
    }
    normal.push(MAIN_SEPARATOR_STR);
    normal.extend(parts);
    normal.to_string_lossy().into_owned()
}

/// What Enter or a double click does to a local file (spec 0013, flow 4
/// and §Security).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    /// Its default app opens it.
    Open,
    /// It's a program: `files_executable` says whether it runs, opens as
    /// plain text, or the user is asked.
    Runnable,
    /// Its default app would run it, so it only ever opens as plain text.
    EditOnly,
}

/// How a local file named `name` opens. Runnable: on Windows an extension
/// in `pathext` (`PATHEXT`), elsewhere any `executable` bit. Edit-only:
/// the rest of the types the OS runs rather than opens (spec 0008
/// §Security).
pub fn opening(name: &str, executable: bool, windows: bool, pathext: Option<&str>) -> Opening {
    let runnable = if windows {
        crate::sftp::in_pathext(name, pathext)
    } else {
        executable
    };
    if runnable {
        Opening::Runnable
    } else if crate::sftp::runs_when_opened(name, windows, pathext) {
        Opening::EditOnly
    } else {
        Opening::Open
    }
}

/// How the local file at `path` opens, from its name and, off Windows,
/// its execute bits (a link's target's). Reads the disk: call it off the
/// UI thread.
pub fn opening_of(path: &Path) -> io::Result<Opening> {
    let metadata = fs::metadata(path)?;
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = {
        let _ = &metadata;
        false
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pathext = std::env::var("PATHEXT").ok();
    Ok(opening(
        &name,
        executable,
        cfg!(windows),
        pathext.as_deref(),
    ))
}

/// What Run starts in a new pane: the file itself, or on Windows
/// `cmd /c "<file>"`, so every `PATHEXT` type runs (spec 0013, Platform
/// notes).
pub fn run_command(file: &Path) -> Command {
    let file = file.to_string_lossy().into_owned();
    if cfg!(windows) {
        Command {
            program: "cmd".into(),
            args: vec!["/c".into(), file],
            env: Default::default(),
        }
    } else {
        Command {
            program: file,
            args: Vec::new(),
            env: Default::default(),
        }
    }
}

/// Opens `file` with the app the OS opens plain text with, never the app
/// for the file's own type (spec 0013, flow 4). Reads the disk and starts
/// a process: call it off the UI thread.
pub fn edit(file: &Path) -> Result<(), String> {
    let mut command = text_editor(file)?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("Couldn't start the text editor: {error}"))?;
    // Reaped on a thread of its own, so it leaves no zombie behind.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Windows: Notepad.
// TODO(OQ-027): the `.txt` association needs `AssocQueryStringW`, which
// takes a new crate (`windows-sys`); Notepad until that's decided.
#[cfg(windows)]
fn text_editor(file: &Path) -> Result<std::process::Command, String> {
    let mut command = std::process::Command::new("notepad.exe");
    command.arg(file);
    Ok(command)
}

/// Linux: the `text/plain` handler `xdg-mime` names, started as its
/// desktop entry's `Exec` line says.
#[cfg(not(windows))]
fn text_editor(file: &Path) -> Result<std::process::Command, String> {
    let output = std::process::Command::new("xdg-mime")
        .args(["query", "default", "text/plain"])
        .output()
        .map_err(|error| format!("Couldn't ask xdg-mime for the text editor: {error}"))?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if id.is_empty() {
        return Err("No app is set to open plain text".into());
    }
    let entry = desktop_entry(&id).ok_or_else(|| format!("The text editor {id} wasn't found"))?;
    let exec = exec_line(&entry).ok_or_else(|| format!("{id} has no Exec line"))?;
    let words = exec_words(&exec, file);
    let (program, args) = words
        .split_first()
        .ok_or_else(|| format!("{id} has an empty Exec line"))?;
    let mut command = std::process::Command::new(program);
    command.args(args);
    Ok(command)
}

/// The desktop entry `id` in the XDG data folders, the user's first.
#[cfg(not(windows))]
fn desktop_entry(id: &str) -> Option<String> {
    let user = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    user.into_iter()
        .chain(system.split(':').map(PathBuf::from))
        .find_map(|dir| fs::read_to_string(dir.join("applications").join(id)).ok())
}

/// The `Exec` value of a desktop entry's `[Desktop Entry]` group.
#[cfg(not(windows))]
fn exec_line(entry: &str) -> Option<String> {
    let mut main = false;
    for line in entry.lines().map(str::trim) {
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
        } else if main && let Some(exec) = line.strip_prefix("Exec=") {
            return Some(exec.to_owned());
        }
    }
    None
}

/// An `Exec` line as program and arguments, `file` in place of its file
/// or URL field code, or appended when it has none; other field codes
/// go. Quoting as the desktop entry spec has it, simplified: double
/// quotes group, a backslash escapes the next character.
#[cfg(not(windows))]
fn exec_words(exec: &str, file: &Path) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut chars = exec.chars();
    let mut started = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    let file = file.to_string_lossy().into_owned();
    let mut placed = false;
    let mut expanded = Vec::new();
    for word in words {
        match word.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                if !placed {
                    expanded.push(file.clone());
                    placed = true;
                }
            }
            "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            _ => expanded.push(word.replace("%%", "%")),
        }
    }
    if !placed {
        expanded.push(file);
    }
    expanded
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::config::tests::TempDir;

    /// Spec 0013, flow 3: on Linux the path field takes `~`, relative and
    /// absolute paths, normalized; `..` stops at the root.
    #[cfg(unix)]
    #[test]
    fn the_local_path_field_resolves_like_the_remote_one() {
        let home = "/home/alice";
        for (input, expected) in [
            ("", "/home/alice"),
            ("~", "/home/alice"),
            ("~/notes/", "/home/alice/notes"),
            ("notes/../src", "/home/alice/src"),
            ("/var//log/./x", "/var/log/x"),
            ("/../..", "/"),
            ("~bob", "/home/alice/~bob"),
        ] {
            assert_eq!(resolve(home, input), expected, "{input:?}");
        }
        assert_eq!(parent("/home/alice"), "/home");
        assert_eq!(parent("/"), "/");
        assert_eq!(join("/", "etc"), "/etc");
    }

    /// Spec 0013, flow 3 and AC10: on Windows any drive goes, a bare
    /// drive is its root, and going up stops at a drive's root.
    #[cfg(windows)]
    #[test]
    fn the_windows_path_field_takes_any_drive_and_stops_at_its_root() {
        let home = r"C:\Users\alice";
        assert_eq!(resolve(home, r"D:\x\..\y"), r"D:\y");
        assert_eq!(resolve(home, "D:"), r"D:\");
        assert_eq!(resolve(home, "~/notes"), r"C:\Users\alice\notes");
        assert_eq!(resolve(home, r"C:\.."), r"C:\");
        assert_eq!(parent(r"D:\"), r"D:\");
        assert_eq!(parent(r"D:\x"), r"D:\");
    }

    /// Spec 0013 §Security and AC6: on Linux a file with an execute bit
    /// is runnable, a launcher without one only opens as text, and
    /// anything else opens with its default app.
    #[test]
    fn on_linux_the_execute_bit_makes_a_file_runnable() {
        assert_eq!(opening("run.sh", true, false, None), Opening::Runnable);
        assert_eq!(opening("x.desktop", false, false, None), Opening::EditOnly);
        assert_eq!(
            opening("App.AppImage", false, false, None),
            Opening::EditOnly
        );
        assert_eq!(opening("notes.txt", false, false, None), Opening::Open);
        assert_eq!(opening("run.sh", false, false, None), Opening::Open);
    }

    /// Spec 0013 §Security: on Windows `PATHEXT` makes a file runnable,
    /// whatever its bits; the rest of the run-list only opens as text.
    #[test]
    fn on_windows_pathext_makes_a_file_runnable() {
        assert_eq!(opening("run.bat", false, true, None), Opening::Runnable);
        assert_eq!(opening("Setup.EXE", false, true, None), Opening::Runnable);
        assert_eq!(opening("a.lnk", false, true, None), Opening::EditOnly);
        assert_eq!(opening("x.hta", false, true, None), Opening::EditOnly);
        assert_eq!(opening("notes.txt", true, true, None), Opening::Open);
        assert_eq!(
            opening("tool.py", false, true, Some(".EXE;.PY")),
            Opening::Runnable
        );
    }

    /// The text editor's `Exec` line: the file goes where its field code
    /// is, other field codes go, quotes group.
    #[cfg(not(windows))]
    #[test]
    fn an_exec_line_takes_the_file_in_place_of_its_field_code() {
        let file = Path::new("/tmp/a b.sh");
        assert_eq!(
            exec_words("gnome-text-editor %U", file),
            ["gnome-text-editor", "/tmp/a b.sh"]
        );
        assert_eq!(
            exec_words(r#""/opt/My Editor/edit" --new %F %i"#, file),
            ["/opt/My Editor/edit", "--new", "/tmp/a b.sh"]
        );
        assert_eq!(exec_words("kate -b", file), ["kate", "-b", "/tmp/a b.sh"]);
        let entry = "[Desktop Entry]\nName=X\nExec=xed %U\n[Desktop Action new]\nExec=other\n";
        assert_eq!(exec_line(entry).as_deref(), Some("xed %U"));
    }

    /// Spec 0013, Model: a local listing is sorted like a remote one, a
    /// link to a folder counts as a folder, a broken link is `Other`.
    #[cfg(unix)]
    #[test]
    fn a_local_listing_resolves_links_and_sorts() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new("local-listing");
        fs::create_dir(dir.path().join("docs")).expect("create docs");
        fs::write(dir.path().join("a.txt"), b"hello").expect("write a.txt");
        symlink("docs", dir.path().join("link")).expect("link");
        symlink("missing", dir.path().join("broken")).expect("broken link");
        let path = dir.path().to_str().expect("utf-8 temp dir");
        let listing = list(path).expect("list");
        let shown: Vec<(&str, Kind, bool)> = listing
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.kind, entry.link))
            .collect();
        assert_eq!(
            shown,
            [
                ("docs", Kind::Folder, false),
                ("link", Kind::Folder, true),
                ("a.txt", Kind::File, false),
                ("broken", Kind::Other, true),
            ]
        );
        assert_eq!(listing.entries[2].size, Some(5));
    }

    /// Spec 0013, flow 5: deleting a link to a folder removes the link,
    /// never what it points to; a folder with anything in it stays.
    #[cfg(unix)]
    #[test]
    fn deleting_a_link_to_a_folder_removes_only_the_link() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new("local-delete");
        let target = dir.path().join("target");
        fs::create_dir(&target).expect("create");
        fs::write(target.join("kept.txt"), b"x").expect("write");
        symlink(&target, dir.path().join("link")).expect("link");
        delete(&dir.path().join("link")).expect("delete the link");
        assert!(target.join("kept.txt").exists());
        assert!(delete(&target).is_err(), "a full folder was deleted");
        assert!(target.join("kept.txt").exists());
    }

    /// Drives a [`Worker`] as the browser does, from outside its thread.
    mod worker {
        use super::super::*;
        use crate::config::tests::TempDir;
        use futures::StreamExt;
        use futures::executor::block_on;
        use futures::future::{Either, select};
        use std::time::Duration;

        /// The next event within 10 s; panics if none comes.
        fn next(events: &mut UnboundedReceiver<Event>) -> Event {
            let (timer, expired) = futures::channel::oneshot::channel::<()>();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(10));
                let _ = timer.send(());
            });
            match block_on(select(events.next(), expired)) {
                Either::Left((Some(event), _)) => event,
                Either::Left((None, _)) => panic!("the worker ended"),
                Either::Right(_) => panic!("no event within 10 s"),
            }
        }

        /// A started worker, its `Connected` event taken off.
        fn started() -> (Worker, UnboundedReceiver<Event>) {
            let (worker, mut events) = Worker::start();
            match next(&mut events) {
                Event::Connected { .. } => (worker, events),
                other => panic!("expected Connected first, got {other:?}"),
            }
        }

        /// `name` in `dir`, as the browser names a local path.
        fn at(dir: &TempDir, name: &str) -> String {
            join(dir.path().to_str().expect("utf-8 temp dir"), name)
        }

        /// `dir` as a local path string.
        fn folder(dir: &TempDir) -> String {
            dir.path().to_str().expect("utf-8 temp dir").to_owned()
        }

        /// Expects `Changed` for `expected`.
        fn assert_changed(event: Event, expected: &str) {
            match event {
                Event::Changed { folder } => assert_eq!(folder, expected),
                other => panic!("expected Changed {{ {expected} }}, got {other:?}"),
            }
        }

        /// Spec 0013, Model: the local session's first event is
        /// `Connected` with the home folder, where relative paths start.
        #[test]
        fn the_first_event_is_connected_with_the_home_folder() {
            let (_worker, mut events) = Worker::start();
            match next(&mut events) {
                Event::Connected { home: shown } => assert_eq!(shown, home()),
                other => panic!("expected Connected, got {other:?}"),
            }
        }

        /// Spec 0013, Model and AC3: a folder that can't be listed says
        /// why, and the next listing still works.
        #[test]
        fn a_missing_folder_fails_its_listing_and_the_worker_keeps_working() {
            let dir = TempDir::new("local-worker-list");
            std::fs::write(dir.path().join("kept"), b"").expect("write kept");
            let (worker, mut events) = started();
            let missing = at(&dir, "not-there");
            worker.list(missing.clone());
            match next(&mut events) {
                Event::ListFailed { path, error } => {
                    assert_eq!(path, missing);
                    assert!(!error.is_empty());
                }
                other => panic!("expected ListFailed, got {other:?}"),
            }
            worker.list(folder(&dir));
            match next(&mut events) {
                Event::Listed(listing) => {
                    assert_eq!(listing.path, folder(&dir));
                    let names: Vec<&str> =
                        listing.entries.iter().map(|e| e.name.as_str()).collect();
                    assert_eq!(names, ["kept"]);
                }
                other => panic!("expected Listed, got {other:?}"),
            }
        }

        /// Spec 0013, flow 5 and AC7: renaming onto an existing name is
        /// reported, not done, so the browser can ask; declining changes
        /// nothing.
        #[test]
        fn a_rename_onto_an_existing_name_reports_it_and_changes_nothing() {
            let dir = TempDir::new("local-worker-rename-exists");
            std::fs::write(dir.path().join("a.txt"), b"a").expect("write a");
            std::fs::write(dir.path().join("b.txt"), b"b").expect("write b");
            let (worker, mut events) = started();
            let (from, to) = (at(&dir, "a.txt"), at(&dir, "b.txt"));
            worker.rename(from.clone(), to.clone(), false);
            match next(&mut events) {
                Event::RenameExists {
                    from: reported_from,
                    to: reported_to,
                } => {
                    assert_eq!(reported_from, from);
                    assert_eq!(reported_to, to);
                }
                other => panic!("expected RenameExists, got {other:?}"),
            }
            assert_eq!(std::fs::read(dir.path().join("a.txt")).expect("a"), b"a");
            assert_eq!(std::fs::read(dir.path().join("b.txt")).expect("b"), b"b");
        }

        /// Spec 0013, flow 5: a rename onto an existing folder is reported
        /// too, on every platform, whatever the OS would do.
        #[test]
        fn a_rename_onto_an_existing_folder_is_reported_too() {
            let dir = TempDir::new("local-worker-rename-folder");
            std::fs::create_dir(dir.path().join("empty")).expect("create empty");
            std::fs::create_dir(dir.path().join("docs")).expect("create docs");
            let (worker, mut events) = started();
            worker.rename(at(&dir, "empty"), at(&dir, "docs"), false);
            assert!(
                matches!(next(&mut events), Event::RenameExists { .. }),
                "renamed over a folder"
            );
            assert!(dir.path().join("empty").is_dir());
        }

        /// Spec 0013, flow 5: once confirmed, the rename replaces the
        /// existing file and its folder changes.
        #[test]
        fn a_confirmed_rename_replaces_the_existing_file() {
            let dir = TempDir::new("local-worker-rename-replace");
            std::fs::write(dir.path().join("a.txt"), b"a").expect("write a");
            std::fs::write(dir.path().join("b.txt"), b"b").expect("write b");
            let (worker, mut events) = started();
            worker.rename(at(&dir, "a.txt"), at(&dir, "b.txt"), true);
            assert_changed(next(&mut events), &folder(&dir));
            assert!(!dir.path().join("a.txt").exists());
            assert_eq!(std::fs::read(dir.path().join("b.txt")).expect("b"), b"a");
        }

        /// Spec 0013, flow 5: a rename to a free name renames and reports
        /// its folder changed.
        #[test]
        fn a_rename_to_a_free_name_changes_its_folder() {
            let dir = TempDir::new("local-worker-rename");
            std::fs::write(dir.path().join("old.txt"), b"x").expect("write old");
            let (worker, mut events) = started();
            worker.rename(at(&dir, "old.txt"), at(&dir, "new.txt"), false);
            assert_changed(next(&mut events), &folder(&dir));
            assert!(!dir.path().join("old.txt").exists());
            assert_eq!(
                std::fs::read(dir.path().join("new.txt")).expect("new"),
                b"x"
            );
        }

        /// Spec 0013, flow 5: delete removes a file and an empty folder,
        /// each reporting its folder changed; a folder with anything in
        /// it fails saying why and keeps its contents.
        #[test]
        fn delete_removes_files_and_empty_folders_but_not_full_ones() {
            let dir = TempDir::new("local-worker-delete");
            std::fs::write(dir.path().join("gone.txt"), b"x").expect("write gone");
            std::fs::create_dir(dir.path().join("empty")).expect("create empty");
            std::fs::create_dir(dir.path().join("full")).expect("create full");
            std::fs::write(dir.path().join("full/kept.txt"), b"x").expect("write kept");
            let (worker, mut events) = started();
            worker.delete(at(&dir, "gone.txt"));
            assert_changed(next(&mut events), &folder(&dir));
            worker.delete(at(&dir, "empty"));
            assert_changed(next(&mut events), &folder(&dir));
            worker.delete(at(&dir, "full"));
            match next(&mut events) {
                Event::Failed(reason) => assert!(!reason.is_empty()),
                other => panic!("expected Failed, got {other:?}"),
            }
            assert!(!dir.path().join("gone.txt").exists());
            assert!(!dir.path().join("empty").exists());
            assert!(dir.path().join("full/kept.txt").exists());
        }

        /// Spec 0013, flow 5: a new folder reports its parent changed; one
        /// whose name is taken fails with the OS's reason.
        #[test]
        fn a_new_folder_changes_its_parent_and_a_taken_name_fails() {
            let dir = TempDir::new("local-worker-mkdir");
            let (worker, mut events) = started();
            worker.make_folder(at(&dir, "fresh"));
            assert_changed(next(&mut events), &folder(&dir));
            assert!(dir.path().join("fresh").is_dir());
            worker.make_folder(at(&dir, "fresh"));
            match next(&mut events) {
                Event::Failed(reason) => assert!(!reason.is_empty()),
                other => panic!("expected Failed, got {other:?}"),
            }
        }

        /// The entry named `name` in the next event, a listing of `dir`.
        fn listed_entry(events: &mut UnboundedReceiver<Event>, dir: &TempDir, name: &str) -> Entry {
            match next(events) {
                Event::Listed(listing) => {
                    assert_eq!(listing.path, folder(dir));
                    listing
                        .entries
                        .into_iter()
                        .find(|entry| entry.name == name)
                        .unwrap_or_else(|| panic!("{name} isn't listed"))
                }
                other => panic!("expected Listed, got {other:?}"),
            }
        }

        /// Spec 0013 flow 5: when the OS can't move something to the
        /// trash, the browser hears why and which path, so it can offer
        /// to delete for good. A path that isn't there can't go, and
        /// nothing reaches the real trash.
        #[test]
        fn what_cant_go_to_the_trash_reports_why_and_changes_nothing() {
            let dir = TempDir::new("local-worker-trash-missing");
            std::fs::write(dir.path().join("kept.txt"), b"x").expect("write kept");
            let (worker, mut events) = started();
            let missing = at(&dir, "not-there");
            worker.trash(missing.clone());
            match next(&mut events) {
                Event::TrashFailed { path, error } => {
                    assert_eq!(path, missing);
                    assert!(!error.is_empty());
                }
                other => panic!("expected TrashFailed, got {other:?}"),
            }
            assert!(dir.path().join("kept.txt").exists());
        }

        /// Spec 0013 AC14: a file, a folder with files in it and a link to
        /// a folder each go to the trash and report their folder changed;
        /// the link's target stays. Ignored by default: it puts real items
        /// into the user's trash (Recycle Bin), which a test must not do
        /// unasked. Run it with `cargo test -- --ignored` and empty the
        /// trash afterwards.
        #[test]
        #[ignore = "moves temp files into the user's real trash or Recycle Bin"]
        fn a_file_a_full_folder_and_a_link_go_to_the_trash() {
            let dir = TempDir::new("local-worker-trash");
            std::fs::write(dir.path().join("tern-trash-test.txt"), b"x").expect("write");
            std::fs::create_dir(dir.path().join("tern-trash-full")).expect("create full");
            std::fs::write(dir.path().join("tern-trash-full/inside.txt"), b"x").expect("write");
            let target = dir.path().join("target");
            std::fs::create_dir(&target).expect("create target");
            std::fs::write(target.join("kept.txt"), b"x").expect("write kept");
            #[cfg(unix)]
            let linked = std::os::unix::fs::symlink(&target, dir.path().join("tern-trash-link"));
            // Needs Developer Mode or elevation; without it the link part
            // is skipped.
            #[cfg(windows)]
            let linked =
                std::os::windows::fs::symlink_dir(&target, dir.path().join("tern-trash-link"));
            let (worker, mut events) = started();
            for name in ["tern-trash-test.txt", "tern-trash-full"] {
                worker.trash(at(&dir, name));
                assert_changed(next(&mut events), &folder(&dir));
                assert!(!dir.path().join(name).exists(), "{name} is still there");
            }
            if linked.is_ok() {
                worker.trash(at(&dir, "tern-trash-link"));
                assert_changed(next(&mut events), &folder(&dir));
                assert!(
                    std::fs::symlink_metadata(dir.path().join("tern-trash-link")).is_err(),
                    "the link is still there"
                );
                assert!(target.join("kept.txt").exists(), "the link's target went");
            }
        }

        /// Spec 0013 AC16: on Windows, ticking Read-only and applying it
        /// makes the file read-only, the folder re-lists, and the next
        /// listing shows it; unticking undoes it. Windows shows no mode,
        /// owner or group.
        #[cfg(windows)]
        #[test]
        fn read_only_goes_on_and_off_and_the_listing_shows_it() {
            use crate::files::permissions::Change;
            let dir = TempDir::new("local-worker-readonly");
            let file = dir.path().join("notes.txt");
            std::fs::write(&file, b"x").expect("write");
            let (worker, mut events) = started();
            worker.list(folder(&dir));
            let before = listed_entry(&mut events, &dir, "notes.txt");
            assert_eq!(before.perms.readonly, Some(false));
            assert_eq!(before.perms.mode, None);
            assert_eq!(before.perms.owner, None);

            for readonly in [true, false] {
                let change = Change {
                    readonly: Some(readonly),
                    ..Change::default()
                };
                worker.set_perms(at(&dir, "notes.txt"), change);
                assert_changed(next(&mut events), &folder(&dir));
                let on_disk = std::fs::metadata(&file).expect("metadata");
                assert_eq!(on_disk.permissions().readonly(), readonly);
                worker.list(folder(&dir));
                let entry = listed_entry(&mut events, &dir, "notes.txt");
                assert_eq!(entry.perms.readonly, Some(readonly));
            }
        }

        /// Spec 0013 flow 10: Enter applies what changed and nothing else;
        /// a change that names nothing leaves the file as it was.
        #[cfg(windows)]
        #[test]
        fn an_empty_change_leaves_the_file_as_it_was() {
            use crate::files::permissions::Change;
            let dir = TempDir::new("local-worker-perms-empty");
            let file = dir.path().join("notes.txt");
            std::fs::write(&file, b"x").expect("write");
            let (worker, mut events) = started();
            worker.set_perms(at(&dir, "notes.txt"), Change::default());
            assert_changed(next(&mut events), &folder(&dir));
            assert!(
                !std::fs::metadata(&file)
                    .expect("metadata")
                    .permissions()
                    .readonly()
            );
        }

        /// Spec 0013 AC15: on Linux, `755` applied to a `644` file makes it
        /// runnable, the folder re-lists, and the listing shows the new
        /// mode with the owner's and group's numbers. A link has no
        /// permissions of its own.
        #[cfg(unix)]
        #[test]
        fn a_mode_applies_and_the_listing_shows_it_but_a_link_has_none() {
            use crate::files::permissions::{Change, Perms};
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let dir = TempDir::new("local-worker-mode");
            let file = dir.path().join("run.sh");
            std::fs::write(&file, b"#!/bin/sh\n").expect("write");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644))
                .expect("chmod 644");
            std::os::unix::fs::symlink(&file, dir.path().join("link")).expect("link");
            let (worker, mut events) = started();
            worker.list(folder(&dir));
            assert_eq!(
                listed_entry(&mut events, &dir, "run.sh").perms.mode,
                Some(0o644)
            );

            let change = Change {
                mode: Some(0o755),
                ..Change::default()
            };
            worker.set_perms(at(&dir, "run.sh"), change);
            assert_changed(next(&mut events), &folder(&dir));
            let on_disk = std::fs::metadata(&file).expect("metadata");
            assert_eq!(on_disk.mode() & 0o7777, 0o755);

            worker.list(folder(&dir));
            let entry = listed_entry(&mut events, &dir, "run.sh");
            assert_eq!(entry.perms.mode, Some(0o755));
            assert_eq!(entry.perms.owner.map(|owner| owner.id), Some(on_disk.uid()));
            assert_eq!(entry.perms.group.map(|group| group.id), Some(on_disk.gid()));
            assert_eq!(entry.perms.readonly, None);

            worker.list(folder(&dir));
            let link = listed_entry(&mut events, &dir, "link");
            assert!(link.link);
            assert_eq!(link.perms, Perms::default());
        }

        /// Spec 0013 flow 5 and AC10: on Windows, deleting a link to a
        /// folder for good removes the link, never its target. Creating
        /// the link needs Developer Mode or elevation; without either the
        /// test has nothing to check and says so.
        #[cfg(windows)]
        #[test]
        fn deleting_a_link_to_a_folder_for_good_keeps_its_target_on_windows() {
            let dir = TempDir::new("local-worker-delete-link");
            let target = dir.path().join("target");
            std::fs::create_dir(&target).expect("create target");
            std::fs::write(target.join("kept.txt"), b"x").expect("write kept");
            let link = dir.path().join("link");
            if let Err(error) = std::os::windows::fs::symlink_dir(&target, &link) {
                eprintln!("skipped: can't create a symlink here ({error})");
                return;
            }
            let (worker, mut events) = started();
            worker.delete(at(&dir, "link"));
            assert_changed(next(&mut events), &folder(&dir));
            assert!(std::fs::symlink_metadata(&link).is_err(), "the link stays");
            assert!(target.join("kept.txt").exists(), "the target went");
        }
    }
}
