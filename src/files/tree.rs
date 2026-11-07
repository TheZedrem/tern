//! The browser's tree of folders (spec 0013, Model): rooted at the folder
//! the path field shows, with folders expanded in place. Each folder is
//! listed once, when it's first expanded, and kept while it's shown; the
//! rows the list draws are rebuilt only when something changed, never per
//! frame, and the view lays out only the visible ones.

use super::{Entry, Listing};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A folder of the tree, as far as its listing got.
enum Folder {
    Loading,
    Listed(Listing),
    /// Listing it failed, and why; shown on its row.
    Failed(String),
}

/// One line of the tree, `depth` levels below the root.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// Entry `ix` of the folder at path `folder`.
    Entry {
        folder: Rc<str>,
        ix: usize,
        depth: usize,
    },
    /// An expanded folder's entries are on their way.
    Loading { depth: usize },
    /// The folder above held more than [`super::MAX_ENTRIES`].
    Truncated { depth: usize },
}

impl Row {
    pub fn depth(&self) -> usize {
        match self {
            Row::Entry { depth, .. } | Row::Loading { depth } | Row::Truncated { depth } => *depth,
        }
    }
}

pub struct Tree {
    /// `name` inside a folder, as the target joins paths.
    join: fn(&str, &str) -> String,
    /// The root, once its listing arrived.
    root: Option<Rc<str>>,
    /// Every folder listed or being listed, the root included, by path.
    folders: HashMap<String, Folder>,
    /// The folders shown open, by path; the root always is.
    expanded: HashSet<String>,
    /// The root, then the folders whose entries are among the rows.
    shown: HashSet<String>,
    rows: Vec<Row>,
}

impl Tree {
    /// An empty tree over a target whose paths `join` builds.
    pub fn new(join: fn(&str, &str) -> String) -> Self {
        Self {
            join,
            root: None,
            folders: HashMap::new(),
            expanded: HashSet::new(),
            shown: HashSet::new(),
            rows: Vec::new(),
        }
    }

    pub fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    /// The root's listing, once it arrived.
    pub fn root_listing(&self) -> Option<&Listing> {
        match self.folders.get(self.root.as_deref()?)? {
            Folder::Listed(listing) => Some(listing),
            _ => None,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Shows `listing` as the root. A new root collapses everything; the
    /// same root again (a refresh) keeps what's expanded.
    pub fn set_root(&mut self, listing: Listing) {
        if self.root.as_deref() != Some(listing.path.as_str()) {
            self.folders.clear();
            self.expanded.clear();
            self.root = Some(listing.path.as_str().into());
        }
        self.folders
            .insert(listing.path.clone(), Folder::Listed(listing));
        self.rebuild();
    }

    /// A listing for a folder of the tree arrived. `false` if the tree
    /// doesn't want it (any more): nothing changed.
    pub fn listed(&mut self, listing: Listing) -> bool {
        match self.folders.get_mut(&listing.path) {
            Some(folder) => *folder = Folder::Listed(listing),
            None => return false,
        }
        self.rebuild();
        true
    }

    /// Listing folder `path` failed with `error`. `false` if the tree
    /// doesn't hold it.
    pub fn failed(&mut self, path: &str, error: String) -> bool {
        match self.folders.get_mut(path) {
            Some(folder) => *folder = Folder::Failed(error),
            None => return false,
        }
        self.rebuild();
        true
    }

    /// Opens folder `path`. `true` when it was never listed and must be
    /// listed now; it shows a loading row until then.
    pub fn expand(&mut self, path: &str) -> bool {
        if !self.expanded.insert(path.to_owned()) {
            return false;
        }
        let new = !self.folders.contains_key(path);
        if new {
            self.folders.insert(path.to_owned(), Folder::Loading);
        }
        self.rebuild();
        new
    }

    /// Closes folder `path`; `false` if it wasn't open.
    pub fn collapse(&mut self, path: &str) -> bool {
        if !self.expanded.remove(path) {
            return false;
        }
        self.rebuild();
        true
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    /// Why listing folder `path` failed, if it did.
    pub fn error(&self, path: &str) -> Option<&str> {
        match self.folders.get(path)? {
            Folder::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// Whether folder `path`'s entries are shown, so a change there
    /// re-lists it.
    pub fn shows(&self, path: &str) -> bool {
        self.shown.contains(path)
    }

    /// What a refresh re-lists: the root first, then every expanded folder
    /// that's shown. Folders out of sight are forgotten, to be listed
    /// afresh when next expanded.
    pub fn refresh(&mut self) -> Vec<String> {
        let shown = &self.shown;
        let expanded = &self.expanded;
        self.folders
            .retain(|path, _| shown.contains(path) || expanded.contains(path));
        let root = self.root.as_deref().map(str::to_owned);
        let mut folders: Vec<String> = root.iter().cloned().collect();
        folders.extend(
            self.shown
                .iter()
                .filter(|path| Some(path.as_str()) != root.as_deref())
                .cloned(),
        );
        folders
    }

    /// The entry on row `ix`, with its path.
    pub fn entry(&self, ix: usize) -> Option<(&Entry, String)> {
        let Row::Entry { folder, ix, .. } = self.rows.get(ix)? else {
            return None;
        };
        let Folder::Listed(listing) = self.folders.get(&**folder)? else {
            return None;
        };
        let entry = listing.entries.get(*ix)?;
        Some((entry, (self.join)(folder, &entry.name)))
    }

    /// The folder holding the entry on row `ix`.
    pub fn folder_of(&self, ix: usize) -> Option<&str> {
        match self.rows.get(ix)? {
            Row::Entry { folder, .. } => Some(folder),
            _ => None,
        }
    }

    /// The row of the folder that row `ix` is in, if it isn't the root.
    pub fn parent_row(&self, ix: usize) -> Option<usize> {
        let depth = self.rows.get(ix)?.depth();
        (0..ix).rev().find(|&row| self.rows[row].depth() < depth)
    }

    /// The row showing the entry at `path`.
    pub fn row_of(&self, path: &str) -> Option<usize> {
        (0..self.rows.len()).find(|&ix| self.entry(ix).is_some_and(|(_, at)| at == path))
    }

    /// Lays the shown folders out as rows, depth first.
    fn rebuild(&mut self) {
        self.rows.clear();
        self.shown.clear();
        if let Some(root) = self.root.clone() {
            self.push_folder(root, 0);
        }
    }

    fn push_folder(&mut self, path: Rc<str>, depth: usize) {
        let truncated = match self.folders.get(&*path) {
            Some(Folder::Listed(listing)) => listing.truncated,
            Some(Folder::Loading) => {
                self.rows.push(Row::Loading { depth });
                return;
            }
            Some(Folder::Failed(_)) | None => return,
        };
        self.shown.insert(path.to_string());
        let count = match self.folders.get(&*path) {
            Some(Folder::Listed(listing)) => listing.entries.len(),
            _ => 0,
        };
        for ix in 0..count {
            self.rows.push(Row::Entry {
                folder: Rc::clone(&path),
                ix,
                depth,
            });
            let child = match self.folders.get(&*path) {
                Some(Folder::Listed(listing)) => {
                    let entry = &listing.entries[ix];
                    entry
                        .is_folder()
                        .then(|| (self.join)(&path, &entry.name))
                        .filter(|child| self.expanded.contains(child))
                }
                _ => None,
            };
            if let Some(child) = child {
                self.push_folder(child.into(), depth + 1);
            }
        }
        if truncated {
            self.rows.push(Row::Truncated { depth });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Kind;

    fn join(folder: &str, name: &str) -> String {
        crate::sftp::join(folder, name)
    }

    /// A listing of `path` with folders `folders` and files `files`.
    fn listing(path: &str, folders: &[&str], files: &[&str]) -> Listing {
        let entry = |name: &&str, kind| Entry {
            name: (*name).into(),
            kind,
            link: false,
            size: None,
            modified: None,
        };
        let mut entries: Vec<Entry> = folders
            .iter()
            .map(|name| entry(name, Kind::Folder))
            .chain(files.iter().map(|name| entry(name, Kind::File)))
            .collect();
        crate::files::sort(&mut entries);
        Listing {
            path: path.into(),
            entries,
            truncated: false,
        }
    }

    /// Each row as its path, `…` for a loading row, indented two spaces
    /// per level.
    fn shown(tree: &Tree) -> Vec<String> {
        (0..tree.rows().len())
            .map(|ix| {
                let row = &tree.rows()[ix];
                let indent = "  ".repeat(row.depth());
                match row {
                    Row::Entry { .. } => format!("{indent}{}", tree.entry(ix).expect("entry").1),
                    Row::Loading { .. } => format!("{indent}…"),
                    Row::Truncated { .. } => format!("{indent}(more)"),
                }
            })
            .collect()
    }

    fn rooted() -> Tree {
        let mut tree = Tree::new(join);
        tree.set_root(listing("/srv", &["app", "logs"], &["README"]));
        tree
    }

    /// Spec 0013 AC3: expanding a folder lists it once, shows a loading row
    /// until then, and then its entries indented below it; the root stays.
    #[test]
    fn expanding_a_folder_shows_its_entries_indented_below_it() {
        let mut tree = rooted();
        assert!(tree.expand("/srv/app"), "a new folder must be listed");
        assert_eq!(
            shown(&tree),
            ["/srv/app", "  …", "/srv/logs", "/srv/README"]
        );
        assert!(tree.listed(listing("/srv/app", &["bin"], &["main.rs"])));
        assert_eq!(
            shown(&tree),
            [
                "/srv/app",
                "  /srv/app/bin",
                "  /srv/app/main.rs",
                "/srv/logs",
                "/srv/README"
            ]
        );
        assert_eq!(tree.root(), Some("/srv"));
    }

    /// Spec 0013, Model: collapsing hides a folder's entries; opening it
    /// again shows them without listing it again.
    #[test]
    fn collapsing_hides_and_reopening_needs_no_new_listing() {
        let mut tree = rooted();
        tree.expand("/srv/app");
        tree.listed(listing("/srv/app", &[], &["main.rs"]));
        assert!(tree.collapse("/srv/app"));
        assert_eq!(shown(&tree), ["/srv/app", "/srv/logs", "/srv/README"]);
        assert!(!tree.expand("/srv/app"), "listed again");
        assert_eq!(shown(&tree)[1], "  /srv/app/main.rs");
    }

    /// Spec 0013 AC3: a folder that can't be listed says why, and has no
    /// entries.
    #[test]
    fn a_folder_that_cant_be_listed_keeps_its_error() {
        let mut tree = rooted();
        tree.expand("/srv/logs");
        assert!(tree.failed("/srv/logs", "Permission denied".into()));
        assert_eq!(tree.error("/srv/logs"), Some("Permission denied"));
        assert_eq!(shown(&tree), ["/srv/app", "/srv/logs", "/srv/README"]);
    }

    /// Spec 0013, Model: changing the root collapses everything.
    #[test]
    fn a_new_root_collapses_everything() {
        let mut tree = rooted();
        tree.expand("/srv/app");
        tree.listed(listing("/srv/app", &["bin"], &[]));
        tree.set_root(listing("/srv/app", &["bin"], &[]));
        assert_eq!(shown(&tree), ["/srv/app/bin"]);
        assert!(!tree.is_expanded("/srv/app"));
        // Its old children are listed afresh when opened again.
        assert!(tree.expand("/srv/app/bin"));
    }

    /// Spec 0013, Model: a refresh of the same root keeps what's expanded,
    /// and re-lists the root and every expanded folder that's shown.
    #[test]
    fn a_refresh_keeps_the_tree_and_relists_what_it_shows() {
        let mut tree = rooted();
        tree.expand("/srv/app");
        tree.listed(listing("/srv/app", &["bin"], &[]));
        tree.expand("/srv/app/bin");
        tree.listed(listing("/srv/app/bin", &[], &["tool"]));
        let mut relisted = tree.refresh();
        assert_eq!(relisted.remove(0), "/srv");
        relisted.sort();
        assert_eq!(relisted, ["/srv/app", "/srv/app/bin"]);
        tree.set_root(listing("/srv", &["app", "logs"], &["README", "new"]));
        assert_eq!(shown(&tree)[2], "    /srv/app/bin/tool");
        assert!(tree.shows("/srv/app/bin"));
        assert!(!tree.shows("/srv/logs"));
    }

    /// Spec 0013, flow 3: Left from an entry goes to the folder it's in.
    #[test]
    fn the_parent_row_is_the_folder_above() {
        let mut tree = rooted();
        tree.expand("/srv/app");
        tree.listed(listing("/srv/app", &[], &["a", "b"]));
        assert_eq!(tree.parent_row(2), Some(0));
        assert_eq!(tree.parent_row(0), None);
        assert_eq!(tree.row_of("/srv/app/b"), Some(2));
        assert_eq!(tree.folder_of(2), Some("/srv/app"));
    }

    /// A listing the tree no longer wants changes nothing.
    #[test]
    fn a_listing_for_a_folder_not_in_the_tree_is_ignored() {
        let mut tree = rooted();
        assert!(!tree.listed(listing("/elsewhere", &[], &["x"])));
        assert_eq!(shown(&tree).len(), 3);
    }
}
