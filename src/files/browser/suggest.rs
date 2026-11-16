//! Suggestions in the path field (spec 0013, Suggestions and flow 9): the
//! folders of the typed path's folder whose names match the rest, best
//! first. That folder is listed once on the target, apart from the tree;
//! Up and Down pick, Tab completes, Enter opens, Esc closes.

use super::{Browser, Connection};
use crate::commands::fuzzy;
use crate::files::Listing;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, Window, anchored,
    deferred, div, px,
};
use gpui_component::input::{Escape, IndentInline, MoveDown, MoveUp};
use gpui_component::{ActiveTheme, Icon, IconName, Sizable};

/// The most folders the dropdown lists.
const MAX_SUGGESTIONS: usize = 10;

/// What the path field suggests.
#[derive(Default)]
pub(super) struct Suggest {
    /// The folder last listed for suggestions, as an absolute path.
    asked: Option<String>,
    /// Its listing, once it arrived.
    listing: Option<Listing>,
    /// The typed text up to and with its last separator.
    typed_folder: String,
    /// The typed text after it.
    query: String,
    /// The matching folders' names, best first.
    shown: Vec<String>,
    highlight: Option<usize>,
    /// The user typed since the field was last opened or closed.
    open: bool,
}

/// `text` split after its last separator: the folder part, as typed, and
/// the rest. A Windows disk takes `/` and `\`.
fn split(text: &str, backslash: bool) -> (&str, &str) {
    let at = if backslash {
        text.rfind(['/', '\\'])
    } else {
        text.rfind('/')
    };
    match at {
        Some(at) => text.split_at(at + 1),
        None => ("", text),
    }
}

/// The names of `listing`'s folders that match `query`, best first, at most
/// [`MAX_SUGGESTIONS`].
fn suggestions(listing: &Listing, query: &str) -> Vec<String> {
    let folders: Vec<&str> = listing
        .entries
        .iter()
        .filter(|entry| entry.is_folder())
        .map(|entry| entry.name.as_str())
        .collect();
    fuzzy::rank(query, folders.iter().copied())
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|ix| folders[ix].to_owned())
        .collect()
}

impl Browser {
    /// The separator a completion ends with on this target.
    fn separator(&self) -> char {
        if self.is_local() {
            std::path::MAIN_SEPARATOR
        } else {
            '/'
        }
    }

    /// The user typed in the path field: lists the typed folder if it's a
    /// new one, and matches the rest against what's there.
    pub(super) fn path_typed(&mut self, cx: &mut Context<Self>) {
        let Connection::Connected { home } = &self.connection else {
            return;
        };
        let text = self.path.read(cx).value().to_string();
        let (folder, query) = split(&text, self.is_local() && cfg!(windows));
        let resolved = self.resolve(home, folder);
        self.suggest.typed_folder = folder.to_owned();
        self.suggest.query = query.to_owned();
        self.suggest.open = true;
        if self.suggest.asked.as_deref() != Some(resolved.as_str()) {
            self.suggest.asked = Some(resolved.clone());
            self.suggest.listing = None;
            if let Some(link) = &self.link {
                link.list(resolved);
            }
        }
        self.match_suggestions();
        cx.notify();
    }

    /// A listing arrived: the suggestions take it if it's their folder's.
    pub(super) fn suggest_listed(&mut self, listing: &Listing) {
        if self.suggest.asked.as_deref() == Some(listing.path.as_str()) {
            self.suggest.listing = Some(listing.clone());
            self.match_suggestions();
        }
    }

    /// Listing `path` failed: no suggestions from it.
    pub(super) fn suggest_failed(&mut self, path: &str) {
        if self.suggest.asked.as_deref() == Some(path) {
            self.suggest.listing = None;
            self.suggest.shown.clear();
        }
    }

    fn match_suggestions(&mut self) {
        let suggest = &mut self.suggest;
        suggest.shown = suggest
            .listing
            .as_ref()
            .map(|listing| suggestions(listing, &suggest.query))
            .unwrap_or_default();
        suggest.highlight = None;
    }

    /// Closes the dropdown until the user types again.
    pub(super) fn close_suggestions(&mut self, cx: &mut Context<Self>) {
        self.suggest.open = false;
        self.suggest.highlight = None;
        cx.notify();
    }

    fn suggesting(&self) -> bool {
        self.suggest.open && !self.suggest.shown.is_empty()
    }

    /// The path of suggestion `ix`, as the field would show it typed.
    fn suggestion_text(&self, ix: usize) -> Option<String> {
        let name = self.suggest.shown.get(ix)?;
        Some(format!("{}{name}", self.suggest.typed_folder))
    }

    /// Enter in the path field: the highlighted folder, else what's typed.
    pub(super) fn path_entered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = match self.suggest.highlight.filter(|_| self.suggesting()) {
            Some(ix) => self.suggestion_text(ix),
            None => None,
        }
        .unwrap_or_else(|| self.path.read(cx).value().to_string());
        self.close_suggestions(cx);
        self.go(&typed, window, cx);
    }

    /// Tab: the field takes the highlighted folder, or the best, and a
    /// separator, and the next suggestions are that folder's.
    fn complete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.suggest.highlight.unwrap_or(0);
        let Some(text) = self.suggestion_text(ix) else {
            return;
        };
        let text = format!("{text}{}", self.separator());
        self.path
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.path_typed(cx);
    }

    fn step_suggestion(&mut self, step: isize, cx: &mut Context<Self>) {
        let last = self.suggest.shown.len().saturating_sub(1);
        self.suggest.highlight = Some(match self.suggest.highlight {
            Some(ix) => ix.saturating_add_signed(step).min(last),
            None if step > 0 => 0,
            None => last,
        });
        cx.notify();
    }

    /// Up, Down, Tab and Esc in the path field are the dropdown's while it
    /// shows; otherwise the field's own.
    pub(super) fn suggestion_keys<E: InteractiveElement>(
        &self,
        field: E,
        cx: &mut Context<Self>,
    ) -> E {
        field
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.suggesting() {
                    cx.stop_propagation();
                    this.step_suggestion(-1, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.suggesting() {
                    cx.stop_propagation();
                    this.step_suggestion(1, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.suggesting() {
                    cx.stop_propagation();
                    this.complete(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.suggesting() {
                    cx.stop_propagation();
                    this.close_suggestions(cx);
                }
            }))
    }

    /// The dropdown under the path field, while it has something to show.
    pub(super) fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.suggesting() {
            return None;
        }
        let theme = cx.theme();
        let (border, background, muted, active) = (
            theme.border,
            theme.popover,
            theme.muted_foreground,
            theme.list_active,
        );
        let rows = self.suggest.shown.iter().enumerate().map(|(ix, name)| {
            let hits = fuzzy::matches(&self.suggest.query, name).map(|(_, hits)| hits);
            div()
                .id(("files-suggestion", ix))
                .h(px(22.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(6.))
                .when(self.suggest.highlight == Some(ix), |row| row.bg(active))
                .child(Icon::new(IconName::Folder).xsmall().text_color(muted))
                .child(div().min_w_0().truncate().child(super::render::name_text(
                    name,
                    hits.as_deref(),
                    theme.primary,
                )))
                // On mouse down, before the field loses the keyboard.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.suggest.highlight = Some(ix);
                        this.path_entered(window, cx);
                    }),
                )
        });
        Some(
            deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(
                    div()
                        .mt(px(24.))
                        .min_w(px(200.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(border)
                        .bg(background)
                        .text_xs()
                        .children(rows),
                ),
            )
            .with_priority(1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::{Entry, Kind};

    /// The typed text splits after its last separator; a Windows disk
    /// takes both.
    #[test]
    fn the_typed_path_splits_into_its_folder_and_the_rest() {
        assert_eq!(split("~/do", false), ("~/", "do"));
        assert_eq!(split("/srv/app/", false), ("/srv/app/", ""));
        assert_eq!(split("logs", false), ("", "logs"));
        assert_eq!(split(r"C:\Users\a", false), ("", r"C:\Users\a"));
        assert_eq!(split(r"C:\Users\a", true), (r"C:\Users\", "a"));
        assert_eq!(split("C:/Users/a", true), ("C:/Users/", "a"));
    }

    /// Only folders are suggested, best first, at most ten.
    #[test]
    fn suggestions_are_matching_folders_best_first() {
        let entry = |name: &str, kind| Entry {
            name: name.into(),
            kind,
            link: false,
            size: None,
            modified: None,
            perms: Default::default(),
        };
        let mut entries: Vec<Entry> = ["Desktop", "Documents", "Downloads", "dotfiles"]
            .into_iter()
            .map(|name| entry(name, Kind::Folder))
            .chain([entry("doc.txt", Kind::File)])
            .chain((0..20).map(|n| entry(&format!("d{n}"), Kind::Folder)))
            .collect();
        crate::files::sort(&mut entries);
        let listing = Listing {
            path: "/home/a".into(),
            entries,
            truncated: false,
        };
        let shown = suggestions(&listing, "do");
        // Equal scores keep the listing's order.
        assert_eq!(shown[..3], ["Documents", "dotfiles", "Downloads"]);
        assert!(!shown.contains(&"doc.txt".to_owned()));
        assert_eq!(suggestions(&listing, "").len(), MAX_SUGGESTIONS);
    }

    /// A folder entry of a listing, a link to one if `link`.
    fn folder(name: &str, link: bool) -> Entry {
        Entry {
            name: name.into(),
            kind: Kind::Folder,
            link,
            size: None,
            modified: None,
            perms: Default::default(),
        }
    }

    /// Spec 0013 AC13 and §Suggestions: `~/do` in a home holding
    /// `Documents` and `Downloads` suggests those two; a link to a folder
    /// counts as a folder, a file never shows.
    #[test]
    fn the_home_folders_matching_the_typed_rest_are_suggested() {
        let (typed_folder, query) = split("~/do", false);
        assert_eq!((typed_folder, query), ("~/", "do"));
        let mut entries = vec![
            folder("Desktop", false),
            folder("Documents", false),
            folder("Downloads", false),
            folder("Music", false),
            Entry {
                kind: Kind::File,
                ..folder("dotnet-install.sh", false)
            },
        ];
        crate::files::sort(&mut entries);
        let listing = Listing {
            path: "/home/a".into(),
            entries,
            truncated: false,
        };
        // The matcher is fuzzy, so `Desktop` (d…o) matches too, below them.
        let shown = suggestions(&listing, query);
        assert_eq!(shown[..2], ["Documents", "Downloads"]);
        assert!(!shown.contains(&"Music".to_owned()));
        assert!(!shown.contains(&"dotnet-install.sh".to_owned()));

        let linked = Listing {
            path: "/home/a".into(),
            entries: vec![folder("data", true), folder("music", false)],
            truncated: false,
        };
        assert_eq!(suggestions(&linked, "da"), ["data"]);
        assert!(suggestions(&linked, "xyz").is_empty());
    }

    /// Spec 0013 §Suggestions: a better match ranks above an earlier one
    /// in the listing.
    #[test]
    fn a_better_match_is_suggested_first() {
        let listing = Listing {
            path: "/srv".into(),
            entries: vec![folder("backlog", false), folder("logs", false)],
            truncated: false,
        };
        assert_eq!(suggestions(&listing, "log"), ["logs", "backlog"]);
    }

    /// Spec 0013 flow 9: on Windows, Local splits on whichever separator
    /// came last, mixed or not; elsewhere a `\` is part of a name.
    #[test]
    fn a_windows_path_splits_on_its_last_separator_of_either_kind() {
        assert_eq!(split(r"C:\Users/a\do", true), (r"C:\Users/a\", "do"));
        assert_eq!(split(r"C:/Users\a/do", true), (r"C:/Users\a/", "do"));
        assert_eq!(split(r"D:\", true), (r"D:\", ""));
        assert_eq!(split(r"/srv/a\b", false), ("/srv/", r"a\b"));
    }
}
