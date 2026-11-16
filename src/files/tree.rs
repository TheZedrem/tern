//! The browser's tree of folders (spec 0013, Model): rooted at the folder
//! the path field shows, with folders expanded in place. Each folder is
//! listed once, when it's first expanded, and kept while it's shown; the
//! rows the list draws are rebuilt only when something changed, never per
//! frame, and the view lays out only the visible ones. A filter keeps the
//! rows whose names match and the folders above them, best first in each
//! folder (spec 0013, Filter).

use super::{Entry, Listing};
use crate::commands::fuzzy;
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
    /// Entry `ix` of the folder at path `folder`. Under a filter, `hits`
    /// are the characters of its name that matched, if it did.
    Entry {
        folder: Rc<str>,
        ix: usize,
        depth: usize,
        hits: Option<Rc<[usize]>>,
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
    /// The root, then the folders whose entries would be among the rows
    /// without a filter.
    shown: HashSet<String>,
    rows: Vec<Row>,
    /// What the filter field holds; `None` shows everything.
    filter: Option<String>,
    /// Under a filter, the row of the best match.
    best: Option<usize>,
}

/// A row and what's below it, as a filter ranks it in its folder.
struct Ranked {
    score: i32,
    rows: Vec<Row>,
    /// The best own match among `rows`: its score and row.
    best: Option<(i32, usize)>,
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
            filter: None,
            best: None,
        }
    }

    /// Filters the rows by `query`; an empty one shows everything.
    pub fn set_filter(&mut self, query: &str) {
        let filter = (!query.trim().is_empty()).then(|| query.to_owned());
        if filter != self.filter {
            self.filter = filter;
            self.rebuild();
        }
    }

    pub fn is_filtered(&self) -> bool {
        self.filter.is_some()
    }

    /// Under a filter, the row whose name matched best.
    pub fn best_match(&self) -> Option<usize> {
        self.best
    }

    /// The characters of row `ix`'s name the filter matched.
    pub fn hits(&self, ix: usize) -> Option<&[usize]> {
        match self.rows.get(ix)? {
            Row::Entry { hits, .. } => hits.as_deref(),
            _ => None,
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

    /// Shows `listing` as the root. A new root collapses everything and
    /// clears the filter; the same root again (a refresh) keeps both.
    pub fn set_root(&mut self, listing: Listing) {
        if self.root.as_deref() != Some(listing.path.as_str()) {
            self.folders.clear();
            self.expanded.clear();
            self.filter = None;
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

    /// The number a loaded listing gives the user (or with `group`, the
    /// group) named `name`: SFTP sets owners by number only (spec 0013,
    /// flow 10).
    pub fn find_id(&self, name: &str, group: bool) -> Option<u32> {
        self.folders
            .values()
            .filter_map(|folder| match folder {
                Folder::Listed(listing) => Some(&listing.entries),
                _ => None,
            })
            .flatten()
            .filter_map(|entry| {
                if group {
                    entry.perms.group.as_ref()
                } else {
                    entry.perms.owner.as_ref()
                }
            })
            .find(|id| id.name.as_deref() == Some(name))
            .map(|id| id.id)
    }

    /// The row showing the entry at `path`.
    pub fn row_of(&self, path: &str) -> Option<usize> {
        (0..self.rows.len()).find(|&ix| self.entry(ix).is_some_and(|(_, at)| at == path))
    }

    /// Lays the shown folders out as rows, depth first.
    fn rebuild(&mut self) {
        self.rows.clear();
        self.shown.clear();
        self.best = None;
        let Some(root) = self.root.clone() else {
            return;
        };
        match self.filter.clone() {
            None => self.push_folder(root, 0),
            Some(query) => {
                let mut matcher = fuzzy::Matcher::new(&query);
                if let Some(ranked) = self.rank_folder(&mut matcher, root, 0) {
                    self.rows = ranked.rows;
                    self.best = ranked.best.map(|(_, row)| row);
                }
            }
        }
    }

    /// Folder `path`'s rows under `matcher`'s query: the entries that match
    /// or hold a match below, each with its own, best first. `None` when
    /// nothing in it matches. Runs on every keystroke over up to 50,000
    /// names a folder, so each name is matched once and never copied.
    fn rank_folder(
        &mut self,
        matcher: &mut fuzzy::Matcher,
        path: Rc<str>,
        depth: usize,
    ) -> Option<Ranked> {
        let Some(Folder::Listed(listing)) = self.folders.get(&*path) else {
            return None;
        };
        // Entry, its own match, and its expanded folder's path.
        let matched: Vec<(usize, Option<fuzzy::Match>, Option<String>)> = listing
            .entries
            .iter()
            .enumerate()
            .filter_map(|(ix, entry)| {
                let own = matcher.matches(&entry.name);
                let child = entry
                    .is_folder()
                    .then(|| (self.join)(&path, &entry.name))
                    .filter(|child| self.expanded.contains(child));
                (own.is_some() || child.is_some()).then_some((ix, own, child))
            })
            .collect();
        self.shown.insert(path.to_string());
        let mut ranked: Vec<Ranked> = Vec::new();
        for (ix, own, child) in matched {
            let below = match &child {
                Some(child) => match self.folders.get(child.as_str()) {
                    Some(Folder::Loading) if own.is_some() => Some(Ranked {
                        score: i32::MIN,
                        rows: vec![Row::Loading { depth: depth + 1 }],
                        best: None,
                    }),
                    Some(Folder::Listed(_)) => {
                        self.rank_folder(matcher, child.as_str().into(), depth + 1)
                    }
                    _ => None,
                },
                None => None,
            };
            if own.is_none() && below.is_none() {
                continue;
            }
            let own_score = own.as_ref().map(|(score, _)| *score);
            let score = own_score
                .max(below.as_ref().map(|below| below.score))
                .unwrap_or(i32::MIN);
            // The best match here: this entry's own, else one below it,
            // one row further down; a tie goes to the row above.
            let below_best = below
                .as_ref()
                .and_then(|below| below.best)
                .map(|(score, row)| (score, row + 1));
            let best = match (own_score, below_best) {
                (Some(own), Some((under, row))) if under > own => Some((under, row)),
                (Some(own), _) => Some((own, 0)),
                (None, under) => under,
            };
            let mut rows = vec![Row::Entry {
                folder: Rc::clone(&path),
                ix,
                depth,
                hits: own.map(|(_, hits)| hits.into()),
            }];
            rows.extend(below.into_iter().flat_map(|below| below.rows));
            ranked.push(Ranked { score, rows, best });
        }
        if ranked.is_empty() {
            return None;
        }
        // Stable: equal scores keep the listing's order.
        ranked.sort_by_key(|ranked| std::cmp::Reverse(ranked.score));
        let score = ranked[0].score;
        let mut rows = Vec::new();
        let mut best: Option<(i32, usize)> = None;
        for item in ranked {
            if let Some((score, row)) = item.best
                && best.is_none_or(|(top, _)| score > top)
            {
                best = Some((score, rows.len() + row));
            }
            rows.extend(item.rows);
        }
        Some(Ranked { score, rows, best })
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
                hits: None,
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
            perms: Default::default(),
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

    /// The tree spec 0013 AC11 describes: `README.md` at the root,
    /// `docs/readme.txt` in an expanded `docs`, and a `src` that was
    /// listed once (it holds a `readme` too) and then collapsed again.
    fn readme_tree() -> Tree {
        let mut tree = Tree::new(join);
        tree.set_root(listing("/r", &["docs", "src"], &["notes.txt", "README.md"]));
        tree.expand("/r/docs");
        tree.listed(listing("/r/docs", &[], &["other.txt", "readme.txt"]));
        tree.expand("/r/src");
        tree.listed(listing("/r/src", &[], &["readme"]));
        tree.collapse("/r/src");
        tree
    }

    /// The name of row `ix` at each index its hits name, lowercased.
    fn hit_text(tree: &Tree, ix: usize) -> Option<String> {
        let (entry, _) = tree.entry(ix)?;
        let chars: Vec<char> = entry.name.chars().collect();
        Some(
            tree.hits(ix)?
                .iter()
                .map(|&at| chars[at].to_ascii_lowercase())
                .collect(),
        )
    }

    /// Spec 0013 AC11 and §Filter: `rdme` keeps only the matches and the
    /// folder above one, ties in listing order; the matched letters are
    /// the ones highlighted, a folder kept for its match below has none;
    /// a collapsed folder's `readme` stays hidden although it was listed.
    #[test]
    fn a_filter_keeps_matches_and_the_folders_above_them() {
        let mut tree = readme_tree();
        tree.set_filter("rdme");
        assert!(tree.is_filtered());
        assert_eq!(
            shown(&tree),
            ["/r/docs", "  /r/docs/readme.txt", "/r/README.md"]
        );
        assert_eq!(tree.hits(0), None, "docs itself doesn't match");
        assert_eq!(hit_text(&tree, 1).as_deref(), Some("rdme"));
        assert_eq!(hit_text(&tree, 2).as_deref(), Some("rdme"));
        let best = tree.best_match().expect("a match is selected");
        assert!(tree.hits(best).is_some(), "the best match is a match");
    }

    /// Spec 0013 AC11 and flow 8: clearing the filter (Esc) brings back
    /// every row as it was, with nothing highlighted.
    #[test]
    fn clearing_the_filter_brings_back_every_row() {
        let mut tree = readme_tree();
        let before = shown(&tree);
        tree.set_filter("rdme");
        tree.set_filter("");
        assert!(!tree.is_filtered());
        assert_eq!(shown(&tree), before);
        assert_eq!(
            before,
            [
                "/r/docs",
                "  /r/docs/other.txt",
                "  /r/docs/readme.txt",
                "/r/src",
                "/r/notes.txt",
                "/r/README.md"
            ]
        );
        assert!((0..tree.rows().len()).all(|ix| tree.hits(ix).is_none()));
        assert_eq!(tree.best_match(), None);
    }

    /// Spec 0013 §Filter: in each folder the rows rank best first, and the
    /// best match overall is the one selected (flow 8).
    #[test]
    fn filtered_rows_rank_best_first_in_their_folder() {
        let mut tree = Tree::new(join);
        tree.set_root(listing("/r", &[], &["catalog.txt", "log.txt"]));
        tree.set_filter("log");
        assert_eq!(shown(&tree), ["/r/log.txt", "/r/catalog.txt"]);
        assert_eq!(tree.best_match(), Some(0));
    }

    /// Spec 0013 flow 8: a filter that matches nothing leaves no rows and
    /// nothing to select ("No matches").
    #[test]
    fn a_filter_that_matches_nothing_shows_no_rows() {
        let mut tree = readme_tree();
        tree.set_filter("zzz");
        assert!(tree.is_filtered());
        assert!(tree.rows().is_empty());
        assert_eq!(tree.best_match(), None);
    }

    /// Spec 0013 §Filter: a new root clears the filter; it isn't carried
    /// to another folder.
    #[test]
    fn a_new_root_clears_the_filter() {
        let mut tree = readme_tree();
        tree.set_filter("rdme");
        tree.set_root(listing("/r/docs", &[], &["other.txt", "readme.txt"]));
        assert!(!tree.is_filtered());
        assert_eq!(shown(&tree), ["/r/docs/other.txt", "/r/docs/readme.txt"]);
    }

    /// Spec 0013 §Filter and flow 8: a folder expanded while filtering
    /// shows its matches once its listing arrives; the filter is applied
    /// to the listing, not dropped by it.
    #[test]
    fn a_listing_that_arrives_is_filtered_too() {
        let mut tree = Tree::new(join);
        tree.set_root(listing("/r", &["docs"], &["README.md"]));
        tree.set_filter("rdme");
        assert_eq!(shown(&tree), ["/r/README.md"]);
        assert!(tree.expand("/r/docs"));
        assert!(tree.listed(listing("/r/docs", &[], &["other.txt", "readme.txt"])));
        assert!(tree.is_filtered());
        assert_eq!(
            shown(&tree),
            ["/r/docs", "  /r/docs/readme.txt", "/r/README.md"]
        );
        // A refresh of the root, the same listing again, keeps it too.
        tree.set_root(listing("/r", &["docs"], &["README.md", "zzz.txt"]));
        assert!(tree.is_filtered());
        assert_eq!(shown(&tree).len(), 3);
    }

    /// Spec 0013 AC12: filtering an expanded folder of 50,000 entries
    /// selects the best match after each key. The bound on the time is
    /// deliberately loose (a debug build on a busy CI machine is far from
    /// a frame budget, so a frame-sized bound would be flaky); it catches
    /// work that grows faster than the folder, not a slow constant.
    #[test]
    fn filtering_fifty_thousand_entries_selects_a_match_after_each_key() {
        let names: Vec<String> = (0..crate::files::MAX_ENTRIES)
            .map(|n| format!("file{n:05}.txt"))
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut tree = Tree::new(join);
        tree.set_root(listing("/r", &["big"], &[]));
        tree.expand("/r/big");
        tree.listed(listing("/r/big", &[], &names));
        let query = "file49999";
        for end in 1..=query.len() {
            let started = std::time::Instant::now();
            tree.set_filter(&query[..end]);
            let took = started.elapsed();
            assert!(
                took < std::time::Duration::from_secs(2),
                "typing {:?} took {took:?}",
                &query[..end]
            );
            let best = tree.best_match().expect("a match is selected");
            assert!(tree.hits(best).is_some());
        }
        let best = tree.best_match().expect("a match is selected");
        assert_eq!(tree.entry(best).expect("entry").1, "/r/big/file49999.txt");
    }

    /// Spec 0013 flow 10: SFTP sets owners by number, so a typed name maps
    /// to the number a loaded listing gives it, users and groups apart; a
    /// name no listing has maps to nothing.
    #[test]
    fn a_typed_owner_name_maps_to_the_number_a_listing_gives_it() {
        use crate::files::permissions::{Id, Perms};
        let mut root = listing("/srv", &[], &["run.sh"]);
        root.entries[0].perms = Perms {
            mode: Some(0o644),
            owner: Some(Id {
                id: 1000,
                name: Some("alice".into()),
            }),
            group: Some(Id {
                id: 50,
                name: Some("staff".into()),
            }),
            readonly: None,
        };
        let mut tree = Tree::new(join);
        tree.set_root(root);
        assert_eq!(tree.find_id("alice", false), Some(1000));
        assert_eq!(tree.find_id("staff", true), Some(50));
        assert_eq!(tree.find_id("alice", true), None);
        assert_eq!(tree.find_id("bob", false), None);
    }

    /// A listing the tree no longer wants changes nothing.
    #[test]
    fn a_listing_for_a_folder_not_in_the_tree_is_ignored() {
        let mut tree = rooted();
        assert!(!tree.listed(listing("/elsewhere", &[], &["x"])));
        assert_eq!(shown(&tree).len(), 3);
    }
}
