//! Command history (spec 0021): the commands local shells report through
//! the shell-integration snippets, one list per shell kind, newest first.
//! [`Recorder`] is what the app holds; its file work runs on the history
//! thread, which starts after the first frame and never before it.

pub mod file;
pub mod filter;

use crate::config::CommandHistory;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// The most distinct commands kept per shell kind.
pub const CAP: usize = 5_000;

/// The shells the snippets report from. Commands of one don't make sense
/// in another, so each has its own history.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Bash,
    Zsh,
    Pwsh,
}

impl ShellKind {
    const ALL: [ShellKind; 3] = [ShellKind::Bash, ShellKind::Zsh, ShellKind::Pwsh];

    /// The kind a snippet names in its report.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            "pwsh" => Some(Self::Pwsh),
            _ => None,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// A command a shell ran, as its snippet reported it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub shell: ShellKind,
    pub command: String,
}

/// Every shell kind's distinct commands, newest first, at most [`CAP`]
/// each.
#[derive(Default, Debug)]
pub struct History {
    lists: [VecDeque<String>; 3],
}

impl History {
    /// Puts `command` at the front of `shell`'s list, moving it there if
    /// it was already in it. Never-stored commands are dropped; returns
    /// whether it was kept.
    pub fn record(&mut self, shell: ShellKind, command: &str) -> bool {
        if !filter::storable(command) {
            return false;
        }
        let list = &mut self.lists[shell.index()];
        list.retain(|kept| kept != command);
        list.push_front(command.to_owned());
        list.truncate(CAP);
        true
    }

    /// `shell`'s commands, newest first.
    #[cfg(test)]
    pub fn commands(&self, shell: ShellKind) -> impl Iterator<Item = &str> {
        self.lists[shell.index()].iter().map(String::as_str)
    }

    /// The rest of `shell`'s newest command that starts with `typed` and
    /// goes on after it (spec 0021, flow 3): exact and case-sensitive.
    pub fn suggest(&self, shell: ShellKind, typed: &str) -> Option<&str> {
        self.lists[shell.index()]
            .iter()
            .find_map(|command| command.strip_prefix(typed).filter(|rest| !rest.is_empty()))
    }

    /// How many commands are kept, over every shell kind.
    pub fn len(&self) -> usize {
        self.lists.iter().map(VecDeque::len).sum()
    }

    pub fn clear(&mut self) {
        self.lists = Default::default();
    }

    /// Adds `older`'s commands behind this one's: what this run recorded
    /// before the file was loaded stays newest.
    fn append_older(&mut self, older: History) {
        for (list, older) in self.lists.iter_mut().zip(older.lists) {
            let mut seen: HashSet<String> = list.iter().cloned().collect();
            for command in older {
                if list.len() >= CAP {
                    break;
                }
                if seen.insert(command.clone()) {
                    list.push_back(command);
                }
            }
        }
    }

    /// Every entry, oldest first: the order the file keeps.
    fn entries(&self) -> Vec<Entry> {
        ShellKind::ALL
            .iter()
            .flat_map(|&shell| {
                self.lists[shell.index()]
                    .iter()
                    .rev()
                    .map(move |command| Entry {
                        shell,
                        command: command.clone(),
                    })
            })
            .collect()
    }
}

/// What the history thread does with the file.
enum Op {
    /// Read the file and add it behind what's in memory.
    Load,
    Append(Entry),
    /// Replace the file with what's in memory.
    Rewrite,
    Delete,
}

/// The app's command history: the lists in memory, the setting, and the
/// thread that keeps the file.
pub struct Recorder {
    history: Arc<Mutex<History>>,
    mode: CommandHistory,
    ops: Sender<Op>,
}

impl Recorder {
    /// Starts the history thread on `path` and loads the file if `mode`
    /// keeps one, or deletes it if not (spec 0021, flows 2 and 6). Only
    /// after the first frame. The receiver gets a message for each file
    /// operation that failed.
    pub fn start(mode: CommandHistory, path: Option<PathBuf>) -> (Self, UnboundedReceiver<String>) {
        let history = Arc::new(Mutex::new(History::default()));
        let (ops, receive) = channel();
        let (errors, failed) = unbounded();
        let shared = Arc::clone(&history);
        // A thread that can't start leaves history in memory only; the
        // failed send below is how the first write reports it.
        let _ = std::thread::Builder::new()
            .name("history".into())
            .spawn(move || run(path, shared, receive, errors));
        let recorder = Self { history, mode, ops };
        recorder.send(match mode {
            CommandHistory::Save => Op::Load,
            CommandHistory::Memory | CommandHistory::Off => Op::Delete,
        });
        (recorder, failed)
    }

    /// Records what a local pane's shell ran (spec 0021, flow 1).
    pub fn record(&self, entry: Entry) {
        if self.mode == CommandHistory::Off {
            return;
        }
        let kept = self
            .history
            .lock()
            .is_ok_and(|mut history| history.record(entry.shell, &entry.command));
        if kept && self.mode == CommandHistory::Save {
            self.send(Op::Append(entry));
        }
    }

    /// What to suggest after `typed` at a prompt of `shell`.
    pub fn suggest(&self, shell: ShellKind, typed: &str) -> Option<String> {
        let history = self.history.lock().ok()?;
        history.suggest(shell, typed).map(str::to_owned)
    }

    /// Applies a changed `command_history`: `memory` and `off` delete the
    /// file, `off` forgets this run's commands too, and `save` writes
    /// what's kept so far.
    pub fn set_mode(&mut self, mode: CommandHistory) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        if mode == CommandHistory::Off
            && let Ok(mut history) = self.history.lock()
        {
            history.clear();
        }
        self.send(match mode {
            CommandHistory::Save => Op::Rewrite,
            CommandHistory::Memory | CommandHistory::Off => Op::Delete,
        });
    }

    /// Forgets every command and deletes the file (spec 0021, flow 6).
    pub fn clear(&self) {
        if let Ok(mut history) = self.history.lock() {
            history.clear();
        }
        self.send(Op::Delete);
    }

    fn send(&self, op: Op) {
        // The thread only ends with the app, or never started; either way
        // the commands stay in memory.
        let _ = self.ops.send(op);
    }
}

/// The history thread: does each file operation in turn until the app
/// drops its [`Recorder`].
fn run(
    path: Option<PathBuf>,
    history: Arc<Mutex<History>>,
    ops: Receiver<Op>,
    errors: UnboundedSender<String>,
) {
    let Some(path) = path else {
        let _ = errors.unbounded_send("this system has no data directory".into());
        return;
    };
    for op in ops {
        let result = match op {
            Op::Load => load(&path, &history),
            Op::Append(entry) => file::append(&path, &entry),
            Op::Rewrite => {
                let entries = history.lock().map(|h| h.entries()).unwrap_or_default();
                file::rewrite(&path, &entries)
            }
            Op::Delete => file::delete(&path),
        };
        if let Err(error) = result {
            let _ = errors.unbounded_send(error);
        }
    }
}

/// Reads the file behind what's in memory. A file with more than twice
/// the lines it keeps, or with entries today's rules wouldn't store, is
/// rewritten.
fn load(path: &std::path::Path, history: &Mutex<History>) -> Result<(), String> {
    let read = file::read(path)?;
    let mut older = History::default();
    let mut dropped = false;
    for entry in &read.entries {
        dropped |= !older.record(entry.shell, &entry.command);
    }
    let lines = read.lines;
    let kept = older.len();
    let entries = {
        let mut history = history.lock().map_err(|_| "history is unavailable")?;
        history.append_older(older);
        history.entries()
    };
    if dropped || lines > 2 * kept {
        file::rewrite(path, &entries)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TempDir;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn entry(shell: ShellKind, command: &str) -> Entry {
        Entry {
            shell,
            command: command.into(),
        }
    }

    fn commands(history: &History, shell: ShellKind) -> Vec<String> {
        history.commands(shell).map(str::to_owned).collect()
    }

    /// `entries` as the file holds them, oldest first, one JSON line each.
    fn lines_of(entries: &[Entry]) -> String {
        entries
            .iter()
            .map(|entry| serde_json::to_string(entry).expect("serialize") + "\n")
            .collect()
    }

    fn write_file(path: &Path, text: &str) {
        std::fs::write(path, text).expect("write the history file");
    }

    /// Waits up to 10 s for the history thread to make `done` true.
    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "not within 10 s: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn file_entries(path: &Path) -> Vec<Entry> {
        file::read(path).expect("read the file").entries
    }

    fn in_memory(recorder: &Recorder, shell: ShellKind) -> Vec<String> {
        commands(&recorder.history.lock().expect("lock"), shell)
    }

    fn kept(recorder: &Recorder) -> usize {
        recorder.history.lock().expect("lock").len()
    }

    /// Spec 0021, Model: running a command again moves it to the front,
    /// and the list stays distinct.
    #[test]
    fn running_a_command_again_moves_it_to_the_front() {
        let mut history = History::default();
        for command in ["git status", "cargo build", "git status"] {
            assert!(history.record(ShellKind::Bash, command));
        }
        assert_eq!(
            commands(&history, ShellKind::Bash),
            ["git status", "cargo build"]
        );
        assert_eq!(history.len(), 2);
    }

    /// Acceptance criterion 7: a command run in one shell kind is never in
    /// another's list.
    #[test]
    fn each_shell_kind_has_its_own_list() {
        let mut history = History::default();
        history.record(ShellKind::Pwsh, "Get-ChildItem");
        history.record(ShellKind::Bash, "ls -la");
        history.record(ShellKind::Zsh, "ls -la");
        assert_eq!(commands(&history, ShellKind::Pwsh), ["Get-ChildItem"]);
        assert_eq!(commands(&history, ShellKind::Bash), ["ls -la"]);
        assert_eq!(commands(&history, ShellKind::Zsh), ["ls -la"]);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn the_shell_kind_is_the_name_the_snippet_reports() {
        assert_eq!(ShellKind::parse("bash"), Some(ShellKind::Bash));
        assert_eq!(ShellKind::parse("zsh"), Some(ShellKind::Zsh));
        assert_eq!(ShellKind::parse("pwsh"), Some(ShellKind::Pwsh));
        for other in ["fish", "Bash", "cmd", ""] {
            assert_eq!(ShellKind::parse(other), None, "{other:?}");
        }
    }

    /// Spec 0021, never stored: such a command enters no list.
    #[test]
    fn a_never_stored_command_is_not_recorded() {
        let mut history = History::default();
        assert!(!history.record(ShellKind::Bash, " echo hi"));
        assert!(!history.record(ShellKind::Bash, "mysql --password=hunter2"));
        assert_eq!(history.len(), 0);
    }

    /// Spec 0021, Model: at most 5 000 per shell kind; the oldest go.
    #[test]
    fn each_list_is_capped_and_drops_the_oldest() {
        let mut history = History::default();
        for n in 0..=CAP {
            history.record(ShellKind::Bash, &format!("echo {n}"));
        }
        history.record(ShellKind::Zsh, "echo zsh");
        let bash = commands(&history, ShellKind::Bash);
        assert_eq!(bash.len(), CAP);
        assert_eq!(bash[0], format!("echo {CAP}"));
        assert_eq!(bash[CAP - 1], "echo 1");
        assert_eq!(commands(&history, ShellKind::Zsh), ["echo zsh"]);
    }

    /// Spec 0021, flow 2: what this run recorded before the file loaded
    /// stays newest, and a command in both isn't kept twice.
    #[test]
    fn older_commands_go_behind_this_runs() {
        let mut history = History::default();
        history.record(ShellKind::Bash, "git push");
        history.record(ShellKind::Bash, "git status");
        let mut older = History::default();
        for command in ["ls", "git status", "make"] {
            older.record(ShellKind::Bash, command);
        }
        older.record(ShellKind::Pwsh, "Get-Date");
        history.append_older(older);
        assert_eq!(
            commands(&history, ShellKind::Bash),
            ["git status", "git push", "make", "ls"]
        );
        assert_eq!(commands(&history, ShellKind::Pwsh), ["Get-Date"]);
    }

    #[test]
    fn older_commands_stop_at_the_cap() {
        let mut history = History::default();
        history.record(ShellKind::Bash, "new");
        let mut older = History::default();
        for n in 0..CAP {
            older.record(ShellKind::Bash, &format!("echo {n}"));
        }
        history.append_older(older);
        let bash = commands(&history, ShellKind::Bash);
        assert_eq!(bash.len(), CAP);
        assert_eq!(bash[0], "new");
        assert_eq!(bash[1], format!("echo {}", CAP - 1));
    }

    #[test]
    fn clear_empties_every_list() {
        let mut history = History::default();
        history.record(ShellKind::Bash, "ls");
        history.record(ShellKind::Pwsh, "dir");
        history.clear();
        assert_eq!(history.len(), 0);
    }

    /// The file keeps entries oldest first, so reading it back in order
    /// gives the same lists.
    #[test]
    fn entries_read_back_in_order_give_the_same_lists() {
        let mut history = History::default();
        for command in ["a", "b", "c"] {
            history.record(ShellKind::Bash, command);
        }
        history.record(ShellKind::Zsh, "z");
        let mut again = History::default();
        for entry in history.entries() {
            again.record(entry.shell, &entry.command);
        }
        assert_eq!(commands(&again, ShellKind::Bash), ["c", "b", "a"]);
        assert_eq!(commands(&again, ShellKind::Zsh), ["z"]);
    }

    /// Acceptance criterion 9: a 20 000-line file with a corrupt line
    /// loads without it, keeps the newest 5 000, and is compacted.
    #[test]
    fn loading_a_long_file_with_a_corrupt_line_keeps_the_newest_and_compacts_it() {
        let dir = TempDir::new("history-load-long");
        let path = dir.path().join("history.jsonl");
        let entries: Vec<Entry> = (0..20_000)
            .map(|n| entry(ShellKind::Bash, &format!("echo {n}")))
            .collect();
        let text = lines_of(&entries[..10_000]) + "{not json\n" + &lines_of(&entries[10_000..]);
        write_file(&path, &text);

        let history = Mutex::new(History::default());
        load(&path, &history).expect("load");
        let bash = commands(&history.lock().expect("lock"), ShellKind::Bash);
        assert_eq!(bash.len(), CAP);
        assert_eq!(bash[0], "echo 19999");
        assert_eq!(bash[CAP - 1], format!("echo {}", 20_000 - CAP));
        let read = file::read(&path).expect("read back");
        assert_eq!(read.lines, CAP, "not compacted");
        assert_eq!(
            read.entries.last(),
            Some(&entry(ShellKind::Bash, "echo 19999"))
        );
    }

    /// Spec 0021, flow 2: a file with many repeats of what it keeps is
    /// rewritten with each command once.
    #[test]
    fn loading_a_file_with_more_than_twice_the_lines_it_keeps_rewrites_it() {
        let dir = TempDir::new("history-load-repeats");
        let path = dir.path().join("history.jsonl");
        write_file(
            &path,
            &lines_of(&vec![entry(ShellKind::Bash, "git status"); 5]),
        );
        let history = Mutex::new(History::default());
        load(&path, &history).expect("load");
        assert_eq!(file_entries(&path), [entry(ShellKind::Bash, "git status")]);
    }

    /// Spec 0021, never stored: an entry today's rules drop (written by an
    /// older build, or by hand) is neither loaded nor kept in the file.
    #[test]
    fn loading_drops_and_rewrites_away_entries_the_rules_reject() {
        let dir = TempDir::new("history-load-rejected");
        let path = dir.path().join("history.jsonl");
        write_file(
            &path,
            &lines_of(&[
                entry(ShellKind::Bash, "ls"),
                entry(ShellKind::Bash, "mysql --password=hunter2"),
                entry(ShellKind::Bash, "make"),
            ]),
        );
        let history = Mutex::new(History::default());
        load(&path, &history).expect("load");
        assert_eq!(
            commands(&history.lock().expect("lock"), ShellKind::Bash),
            ["make", "ls"]
        );
        assert_eq!(
            file_entries(&path),
            [entry(ShellKind::Bash, "ls"), entry(ShellKind::Bash, "make")]
        );
    }

    /// Spec 0021, flow 2: lines that can't be read are skipped and the
    /// rest loads.
    #[test]
    fn loading_skips_lines_it_cannot_read() {
        let dir = TempDir::new("history-load-corrupt");
        let path = dir.path().join("history.jsonl");
        let text = lines_of(&[
            entry(ShellKind::Pwsh, "Get-Date"),
            entry(ShellKind::Zsh, "ls"),
        ]) + "{\"shell\":\"fish\",\"command\":\"ls\"}\ngarbage\n";
        write_file(&path, &text);
        let history = Mutex::new(History::default());
        load(&path, &history).expect("load");
        let history = history.lock().expect("lock");
        assert_eq!(commands(&history, ShellKind::Pwsh), ["Get-Date"]);
        assert_eq!(commands(&history, ShellKind::Zsh), ["ls"]);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn loading_a_missing_file_is_an_empty_history() {
        let dir = TempDir::new("history-load-missing");
        let history = Mutex::new(History::default());
        load(&dir.path().join("history.jsonl"), &history).expect("load");
        assert_eq!(history.lock().expect("lock").len(), 0);
    }

    /// Spec 0021, flow 1: with `save`, a recorded command is appended to
    /// the file (its folder created), and a never-stored one isn't.
    #[test]
    fn save_appends_each_recorded_command_to_the_file() {
        let dir = TempDir::new("history-save");
        let path = dir.path().join("tern/history.jsonl");
        let (recorder, _errors) = Recorder::start(CommandHistory::Save, Some(path.clone()));
        recorder.record(entry(ShellKind::Bash, "mysql --password=hunter2"));
        recorder.record(entry(ShellKind::Bash, "git status"));
        recorder.record(entry(ShellKind::Pwsh, "Get-Date"));
        wait_until("both commands in the file", || {
            path.exists() && file_entries(&path).len() == 2
        });
        assert_eq!(
            file_entries(&path),
            [
                entry(ShellKind::Bash, "git status"),
                entry(ShellKind::Pwsh, "Get-Date")
            ]
        );
        assert_eq!(in_memory(&recorder, ShellKind::Bash), ["git status"]);
    }

    /// Acceptance criterion 5: with `save`, the previous run's commands
    /// come back after a restart, behind this run's.
    #[test]
    fn save_loads_the_previous_runs_commands_behind_this_runs() {
        let dir = TempDir::new("history-restart");
        let path = dir.path().join("history.jsonl");
        {
            let (recorder, _errors) = Recorder::start(CommandHistory::Save, Some(path.clone()));
            recorder.record(entry(ShellKind::Bash, "git status"));
            recorder.record(entry(ShellKind::Bash, "make"));
            wait_until("the first run's commands in the file", || {
                file_entries(&path).len() == 2
            });
        }
        let (recorder, _errors) = Recorder::start(CommandHistory::Save, Some(path.clone()));
        recorder.record(entry(ShellKind::Bash, "cargo test"));
        wait_until("the file loaded", || {
            in_memory(&recorder, ShellKind::Bash).len() == 3
        });
        assert_eq!(
            in_memory(&recorder, ShellKind::Bash),
            ["cargo test", "make", "git status"]
        );
    }

    /// Acceptance criterion 5 and flow 6: with `memory`, a file from an
    /// earlier run is deleted, and commands are kept only in memory.
    #[test]
    fn memory_deletes_the_file_at_start_and_keeps_commands_only_in_memory() {
        let dir = TempDir::new("history-memory");
        let path = dir.path().join("history.jsonl");
        write_file(&path, &lines_of(&[entry(ShellKind::Bash, "old")]));
        let (recorder, _errors) = Recorder::start(CommandHistory::Memory, Some(path.clone()));
        recorder.record(entry(ShellKind::Bash, "git status"));
        wait_until("the file deleted", || !path.exists());
        assert_eq!(in_memory(&recorder, ShellKind::Bash), ["git status"]);
    }

    /// Acceptance criterion 5 and flow 6: with `off`, a file from an
    /// earlier run is deleted and nothing is recorded.
    #[test]
    fn off_deletes_the_file_at_start_and_records_nothing() {
        let dir = TempDir::new("history-off");
        let path = dir.path().join("history.jsonl");
        write_file(&path, &lines_of(&[entry(ShellKind::Bash, "old")]));
        let (recorder, _errors) = Recorder::start(CommandHistory::Off, Some(path.clone()));
        recorder.record(entry(ShellKind::Bash, "git status"));
        assert_eq!(kept(&recorder), 0);
        wait_until("the file deleted", || !path.exists());
    }

    /// Flow 6: switching to `memory` deletes the file but keeps this run's
    /// commands; back to `save` writes them out again; `off` deletes the
    /// file and forgets them.
    #[test]
    fn changing_the_mode_deletes_or_writes_the_file() {
        let dir = TempDir::new("history-set-mode");
        let path = dir.path().join("history.jsonl");
        let (mut recorder, _errors) = Recorder::start(CommandHistory::Save, Some(path.clone()));
        recorder.record(entry(ShellKind::Bash, "git status"));
        wait_until("the command in the file", || file_entries(&path).len() == 1);

        recorder.set_mode(CommandHistory::Memory);
        wait_until("memory deleted the file", || !path.exists());
        assert_eq!(in_memory(&recorder, ShellKind::Bash), ["git status"]);
        recorder.record(entry(ShellKind::Bash, "make"));

        recorder.set_mode(CommandHistory::Save);
        wait_until("save wrote the file", || file_entries(&path).len() == 2);
        assert_eq!(
            file_entries(&path),
            [
                entry(ShellKind::Bash, "git status"),
                entry(ShellKind::Bash, "make")
            ]
        );

        recorder.set_mode(CommandHistory::Off);
        assert_eq!(kept(&recorder), 0);
        wait_until("off deleted the file", || !path.exists());
        recorder.record(entry(ShellKind::Bash, "ls"));
        assert_eq!(kept(&recorder), 0);
    }

    /// Acceptance criterion 6: clearing forgets every shell kind's
    /// commands and deletes the file; recording goes on after it.
    #[test]
    fn clear_forgets_every_command_and_deletes_the_file() {
        let dir = TempDir::new("history-clear");
        let path = dir.path().join("history.jsonl");
        let (recorder, _errors) = Recorder::start(CommandHistory::Save, Some(path.clone()));
        recorder.record(entry(ShellKind::Bash, "git status"));
        recorder.record(entry(ShellKind::Pwsh, "Get-Date"));
        wait_until("both commands in the file", || {
            file_entries(&path).len() == 2
        });
        recorder.clear();
        assert_eq!(kept(&recorder), 0);
        wait_until("the file deleted", || !path.exists());

        recorder.record(entry(ShellKind::Zsh, "ls"));
        wait_until("the next command in a new file", || {
            file_entries(&path) == [entry(ShellKind::Zsh, "ls")]
        });
    }

    /// Flow 1: when the file can't be kept, the app hears about it, and
    /// recording goes on in memory.
    #[test]
    fn without_a_data_directory_a_failure_is_reported_and_memory_still_records() {
        let (recorder, mut errors) = Recorder::start(CommandHistory::Save, None);
        wait_until("a failure reported", || errors.try_recv().is_ok());
        recorder.record(entry(ShellKind::Bash, "git status"));
        assert_eq!(in_memory(&recorder, ShellKind::Bash), ["git status"]);
    }

    /// A bash history holding `commands`, run oldest first.
    fn bash_history(commands: &[&str]) -> History {
        let mut history = History::default();
        for command in commands {
            assert!(history.record(ShellKind::Bash, command), "{command}");
        }
        history
    }

    /// Acceptance criterion 1: after `git status`, typing `git s`
    /// suggests the rest, `tatus`, not the whole command.
    #[test]
    fn the_suggestion_is_the_rest_of_the_command_after_the_typed_text() {
        let history = bash_history(&["git status"]);
        assert_eq!(history.suggest(ShellKind::Bash, "git s"), Some("tatus"));
        assert_eq!(history.suggest(ShellKind::Bash, "g"), Some("it status"));
        assert_eq!(history.suggest(ShellKind::Bash, "cargo"), None);
    }

    /// Spec 0021, flow 3: "the newest entry … that starts with it". A
    /// command run again is the newest again (Model).
    #[test]
    fn the_newest_command_that_starts_with_the_typed_text_is_suggested() {
        let mut history = bash_history(&["git status", "git stash", "cargo test"]);
        assert_eq!(history.suggest(ShellKind::Bash, "git s"), Some("tash"));
        // Only the older one goes on from here.
        assert_eq!(history.suggest(ShellKind::Bash, "git stat"), Some("us"));
        history.record(ShellKind::Bash, "git status");
        assert_eq!(history.suggest(ShellKind::Bash, "git s"), Some("tatus"));
    }

    /// Spec 0021, flow 3: the match is "exact, case-sensitive" and only
    /// at the command's start.
    #[test]
    fn the_match_is_an_exact_case_sensitive_prefix() {
        let history = bash_history(&["Git Status", "ls  -la"]);
        assert_eq!(history.suggest(ShellKind::Bash, "Git S"), Some("tatus"));
        for typed in ["git s", "GIT S", "it S", "Status", " Git", "Git  S", "ls -"] {
            assert_eq!(history.suggest(ShellKind::Bash, typed), None, "{typed:?}");
        }
        assert_eq!(history.suggest(ShellKind::Bash, "ls  -"), Some("la"));
    }

    /// Spec 0021, flow 3 ("the rest of the newest entry"): a command
    /// that's typed out in full has no rest, so an older, longer one is
    /// suggested instead, and nothing if there's none.
    #[test]
    fn a_command_typed_out_in_full_gives_way_to_an_older_longer_one() {
        let history = bash_history(&["git status --short", "git status"]);
        assert_eq!(
            history.suggest(ShellKind::Bash, "git status"),
            Some(" --short")
        );
        assert_eq!(history.suggest(ShellKind::Bash, "git status --short"), None);
    }

    /// Acceptance criterion 7: "A command run in pwsh isn't suggested in
    /// a bash pane, and the reverse."
    #[test]
    fn a_command_is_suggested_only_in_its_own_shell_kind() {
        let mut history = History::default();
        history.record(ShellKind::Pwsh, "Get-ChildItem -Force");
        history.record(ShellKind::Bash, "getent hosts");
        history.record(ShellKind::Zsh, "gem list");
        assert_eq!(
            history.suggest(ShellKind::Pwsh, "Get-"),
            Some("ChildItem -Force")
        );
        assert_eq!(history.suggest(ShellKind::Bash, "Get-"), None);
        assert_eq!(history.suggest(ShellKind::Zsh, "Get-"), None);
        assert_eq!(history.suggest(ShellKind::Bash, "ge"), Some("tent hosts"));
        assert_eq!(history.suggest(ShellKind::Zsh, "ge"), Some("m list"));
        assert_eq!(history.suggest(ShellKind::Pwsh, "ge"), None);
    }

    /// Acceptance criterion 4: a never-stored command is never suggested.
    #[test]
    fn a_never_stored_command_is_not_suggested() {
        let mut history = History::default();
        history.record(ShellKind::Bash, "mysql --password=hunter2");
        history.record(ShellKind::Bash, " echo hi");
        assert_eq!(history.suggest(ShellKind::Bash, "mysql"), None);
        assert_eq!(history.suggest(ShellKind::Bash, " echo"), None);
    }

    /// Acceptance criterion 6: after clearing, nothing is suggested in
    /// any shell kind.
    #[test]
    fn nothing_is_suggested_after_clear() {
        let mut history = History::default();
        history.record(ShellKind::Bash, "git status");
        history.record(ShellKind::Pwsh, "git status");
        history.clear();
        for shell in ShellKind::ALL {
            assert_eq!(history.suggest(shell, "git"), None, "{shell:?}");
        }
    }

    /// Acceptance criteria 1 and 6, through the app's recorder: what a
    /// pane recorded is suggested in every pane of that shell kind right
    /// away, and **Clear command history** removes it.
    #[test]
    fn the_recorder_suggests_what_it_recorded_until_it_is_cleared() {
        let dir = TempDir::new("history-suggest");
        let path = dir.path().join("history.jsonl");
        let (recorder, _errors) = Recorder::start(CommandHistory::Memory, Some(path));
        recorder.record(entry(ShellKind::Bash, "git status"));
        assert_eq!(
            recorder.suggest(ShellKind::Bash, "git s").as_deref(),
            Some("tatus")
        );
        assert_eq!(recorder.suggest(ShellKind::Pwsh, "git s"), None);
        recorder.clear();
        assert_eq!(recorder.suggest(ShellKind::Bash, "git s"), None);
    }

    /// Acceptance criterion 5: a command from the previous run is
    /// suggested with `command_history: save`, once the file is loaded.
    #[test]
    fn a_command_of_the_previous_run_is_suggested_with_save() {
        let dir = TempDir::new("history-suggest-save");
        let path = dir.path().join("history.jsonl");
        write_file(
            &path,
            &lines_of(&[entry(ShellKind::Zsh, "kubectl get pods")]),
        );
        let (recorder, _errors) = Recorder::start(CommandHistory::Save, Some(path));
        wait_until("the file loaded", || kept(&recorder) == 1);
        assert_eq!(
            recorder.suggest(ShellKind::Zsh, "kube").as_deref(),
            Some("ctl get pods")
        );
    }

    /// Acceptance criterion 5: "With `off`, nothing is suggested or
    /// recorded", neither this run's commands nor an earlier run's.
    #[test]
    fn nothing_is_suggested_with_off() {
        let dir = TempDir::new("history-suggest-off");
        let path = dir.path().join("history.jsonl");
        write_file(&path, &lines_of(&[entry(ShellKind::Bash, "git stash")]));
        let (mut recorder, _errors) = Recorder::start(CommandHistory::Off, Some(path.clone()));
        recorder.record(entry(ShellKind::Bash, "git status"));
        wait_until("the file deleted", || !path.exists());
        assert_eq!(recorder.suggest(ShellKind::Bash, "git s"), None);

        // Turned off later, what this run recorded goes too.
        recorder.set_mode(CommandHistory::Memory);
        recorder.record(entry(ShellKind::Bash, "git status"));
        assert!(recorder.suggest(ShellKind::Bash, "git s").is_some());
        recorder.set_mode(CommandHistory::Off);
        assert_eq!(recorder.suggest(ShellKind::Bash, "git s"), None);
    }
}
