//! Watches the config directory for changes (spec 0005, flow 3).

use futures::channel::mpsc::UnboundedSender;
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
/// config would trigger a reload. Nor does the directory's own change of
/// contents or times: Windows reports one late for any file written in it,
/// and the files that matter are reported on their own.
fn affects_config(event: &notify::Event, dir: &Path, links: &[PathBuf]) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    let renamed_or_made = !matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_))
    );
    let themes = dir.join("themes");
    event.paths.iter().any(|path| {
        (path == dir && renamed_or_made)
            || path == &dir.join("config.yaml")
            || links.contains(path)
            || (path.parent() == Some(&themes) && path.extension().is_some_and(|ext| ext == "yaml"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;
    use crate::config::tests::TempDir;
    use crate::theme::Library;
    use futures::channel::mpsc::{UnboundedReceiver, unbounded};
    use std::fs;
    use std::time::{Duration, Instant};

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
        let dir = root.path().join("tern");
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
        let dir = root.path().join("tern");
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
        let dir = root.path().join("tern");
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
        let dir = root.path().join("tern");
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

    /// On a fresh system the config dir doesn't exist when Tern starts.
    #[test]
    fn a_config_dir_created_after_start_is_watched_once_armed() {
        let root = TempDir::new("watch-later");
        let dir = root.path().join("tern");
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
        let dir = root.path().join("tern");
        fs::create_dir(&dotfiles).expect("create the dotfiles dir");
        fs::create_dir(&dir).expect("create the config dir");
        fs::write(dotfiles.join("config.yaml"), "scrollback: 1\n").expect("write the config");
        std::os::unix::fs::symlink(dotfiles.join("config.yaml"), dir.join("config.yaml"))
            .expect("link config.yaml");
        let (_watcher, mut reports) = watch(&dir);

        fs::write(dir.join("config.yaml"), "scrollback: 2\n").expect("write config.yaml");
        assert!(reported(&mut reports, WITHIN), "writing through the link");
    }

    /// Linking the dotfiles in while Tern runs: the reload the new link
    /// causes arms the watcher, which then follows it.
    #[cfg(unix)]
    #[test]
    fn a_config_yaml_linked_after_start_is_followed_once_armed() {
        let root = TempDir::new("watch-linked-later");
        let dotfiles = root.path().join("dotfiles");
        let dir = root.path().join("tern");
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
        let themes = root.path().join("tern/themes");
        fs::create_dir(&dotfiles).expect("create the dotfiles dir");
        fs::create_dir_all(&themes).expect("create the themes dir");
        fs::write(dotfiles.join("midnight.yaml"), "background: \"#000001\"\n")
            .expect("write the theme");
        std::os::unix::fs::symlink(dotfiles.join("midnight.yaml"), themes.join("midnight.yaml"))
            .expect("link the theme");
        let (_watcher, mut reports) = watch(&root.path().join("tern"));

        fs::write(themes.join("midnight.yaml"), "background: \"#000002\"\n")
            .expect("write the theme");
        assert!(reported(&mut reports, WITHIN), "writing through the link");
    }
}
