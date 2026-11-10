//! The Import tab: a card per import source (spec 0014), with the users
//! asked for PuTTY's and WinSCP's hosts that saved none (spec 0015).

use crate::import::{self, Host, Outcome, Place, Source, Users};
use crate::layout::show_notice;
use crate::settings::{Page, Row, SettingsEvent, SettingsView};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, PathPromptOptions, SharedString,
    Styled, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable, Sizable};
use std::path::PathBuf;

/// Hosts past this many take the "for all" user and get no input of their
/// own, so a registry of thousands keeps the tab responsive.
const USER_ROWS: usize = 50;

/// What a host's user input shows while "for all" is blank.
const NO_USER: &str = "no user";

/// An import source's card on the Import tab (spec 0014, Model).
pub(in crate::settings) struct ImportCard {
    source: Source,
    /// The file chosen on the card; `None` reads where the app keeps its
    /// sessions ([`Source::find`]).
    file: Option<PathBuf>,
    /// What was read last, shown under the card's lines.
    place: Option<Place>,
    state: CardState,
    /// What the last import from this card did.
    imported: Option<Outcome>,
    /// The users entered for its hosts without one, while it has some.
    users: Option<UserInputs>,
}

pub(in crate::settings) enum CardState {
    Looking,
    /// Nothing to read.
    Missing,
    /// What importing the file would do.
    Ready(Outcome),
    Importing,
    Failed(String),
}

/// A card's user inputs (spec 0015, flow 5): one for all, one per host
/// (up to [`USER_ROWS`]) whose placeholder shows the "for all" entry.
struct UserInputs {
    all: Entity<InputState>,
    /// Per host: its [`Host::site`], its label and its input.
    each: Vec<(String, String, Entity<InputState>)>,
    /// How many hosts asking for a user have no row.
    more: usize,
    _all_changed: Subscription,
}

impl UserInputs {
    fn new(asking: &[Host], window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let all = cx.new(|cx| InputState::new(window, cx).placeholder("User for all"));
        let each: Vec<_> = asking
            .iter()
            .take(USER_ROWS)
            .map(|host| {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(NO_USER));
                (
                    host.site(),
                    format!("{} ({})", host.site(), host.host),
                    input,
                )
            })
            .collect();
        let rows: Vec<_> = each.iter().map(|(_, _, input)| input.clone()).collect();
        let all_changed = cx.subscribe_in(
            &all,
            window,
            move |_, all, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    let text = all.read(cx).value();
                    let shown: SharedString = if text.trim().is_empty() {
                        NO_USER.into()
                    } else {
                        text
                    };
                    for row in &rows {
                        row.update(cx, |row, cx| row.set_placeholder(shown.clone(), window, cx));
                    }
                }
            },
        );
        Self {
            all,
            each,
            more: asking.len().saturating_sub(USER_ROWS),
            _all_changed: all_changed,
        }
    }

    /// Whether these inputs are for exactly the hosts in `asking`.
    fn asks_for(&self, asking: &[Host]) -> bool {
        self.each.len() + self.more == asking.len()
            && self
                .each
                .iter()
                .zip(asking)
                .all(|((site, _, _), host)| *site == host.site())
    }

    /// What's entered, for the import.
    fn users(&self, cx: &gpui::App) -> Users {
        Users {
            all: self.all.read(cx).value().to_string(),
            each: self
                .each
                .iter()
                .map(|(site, _, input)| (site.clone(), input.read(cx).value().to_string()))
                .collect(),
        }
    }
}

impl SettingsView {
    /// Looks at every import source afresh (spec 0014, flow 1): where the
    /// app keeps its sessions, or the file chosen on its card.
    pub(in crate::settings) fn look_for_imports(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.imports.is_empty() {
            self.imports = Source::ALL
                .into_iter()
                .map(|source| ImportCard {
                    source,
                    file: None,
                    place: None,
                    state: CardState::Looking,
                    imported: None,
                    users: None,
                })
                .collect();
        }
        for source in Source::ALL {
            self.import_from(source, None, false, window, cx);
        }
    }

    /// Plans the import of `source`'s card or, with `write`, runs it, from
    /// `file` if given, on the background executor [load-bearing: the
    /// registry and files are never read on the UI thread]; the card shows
    /// the outcome. An import plans again after writing, so the card shows
    /// what's left (spec 0014, flow 4).
    fn import_from(
        &mut self,
        source: Source,
        file: Option<PathBuf>,
        write: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.imports.iter_mut().find(|card| card.source == source) else {
            return;
        };
        if let Some(file) = file {
            card.file = Some(file);
            card.imported = None;
            card.users = None;
        }
        let chosen = card.file.clone();
        let users = card
            .users
            .as_ref()
            .map(|inputs| inputs.users(cx))
            .unwrap_or_default();
        card.state = if write {
            CardState::Importing
        } else {
            CardState::Looking
        };
        cx.notify();
        let work = cx.background_spawn(async move {
            let place = match chosen {
                Some(file) => file.exists().then_some(Place::File(file)),
                None => source.find(),
            };
            let Some(place) = place else {
                return (None, None, None);
            };
            let imported = write.then(|| import::outcome(&place, &users, false));
            let planned = match &imported {
                Some(Err(error)) => Err(error.clone()),
                _ => import::outcome(&place, &users, true),
            };
            (Some(place), imported, Some(planned))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (place, imported, planned) = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if imported.as_ref().is_some_and(Result::is_ok) {
                    cx.emit(SettingsEvent::Written);
                }
                let Some(card) = this.imports.iter_mut().find(|card| card.source == source) else {
                    return;
                };
                card.place = place;
                if let Some(Ok(outcome)) = imported {
                    card.imported = Some(outcome);
                }
                card.state = match planned {
                    None => CardState::Missing,
                    Some(Ok(outcome)) => CardState::Ready(outcome),
                    Some(Err(error)) => CardState::Failed(error),
                };
                // Entries stay while the card asks for the same hosts.
                match &card.state {
                    CardState::Ready(outcome) if outcome.asking.is_empty() => card.users = None,
                    CardState::Ready(outcome)
                        if !card
                            .users
                            .as_ref()
                            .is_some_and(|users| users.asks_for(&outcome.asking)) =>
                    {
                        card.users = Some(UserInputs::new(&outcome.asking, window, cx));
                    }
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks the OS's file picker for a file to import on `source`'s card
    /// (spec 0014, flow 5); cancelling changes nothing.
    fn choose_import(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    let notice = format!("The file picker didn't open: {error:#}");
                    let _ = cx.update(|window, cx| show_notice(notice, window, cx));
                    None
                }
            };
            if let Some(path) = path {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.import_from(source, Some(path), false, window, cx)
                });
            }
        })
        .detach();
    }

    /// The Import tab: a row per source, with what its file holds and the
    /// buttons to import it (spec 0014).
    pub(in crate::settings) fn import_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        let muted = cx.theme().muted_foreground;
        self.imports
            .iter()
            .map(|card| {
                let source = card.source;
                let title = source.title();
                let (lines, can_import) = card_lines(card);
                let place = card
                    .place
                    .as_ref()
                    .filter(|_| !matches!(card.state, CardState::Missing))
                    .map(ToString::to_string);
                let users = card
                    .users
                    .as_ref()
                    .filter(|_| matches!(card.state, CardState::Ready(_)))
                    .map(|users| {
                        let row = |label: SharedString, input: &Entity<InputState>| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().w(px(220.)).text_xs().truncate().child(label))
                                .child(div().w(px(160.)).child(Input::new(input).small()))
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(row("User for all".into(), &users.all))
                            .children(
                                users
                                    .each
                                    .iter()
                                    .map(|(_, label, input)| row(label.clone().into(), input)),
                            )
                            .children((users.more > 0).then(|| {
                                div().text_xs().text_color(muted).child(format!(
                                    "{} more take the user for all.",
                                    hosts(users.more)
                                ))
                            }))
                    });
                let control = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .py(px(4.))
                    .children(lines.into_iter().map(|line| div().text_sm().child(line)))
                    .children(
                        place
                            .map(|place| div().text_xs().text_color(muted).truncate().child(place)),
                    )
                    .children(users)
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                Button::new(("import-run", source as usize))
                                    .label("Import")
                                    .small()
                                    .primary()
                                    .disabled(!can_import)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.import_from(source, None, true, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("import-choose", source as usize))
                                    .label("Choose file…")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose_import(source, window, cx);
                                    })),
                            ),
                    );
                Row {
                    page: Page::Import,
                    group: "Import SSH hosts",
                    label: title.into(),
                    keywords: format!("import migrate hosts sessions ssh {}", title.to_lowercase()),
                    control: control.into_any_element(),
                }
            })
            .collect()
    }
}

/// What an import card says, line by line, and whether its Import button
/// has anything to do (spec 0014, flows 2 to 6; spec 0015, flow 4).
fn card_lines(card: &ImportCard) -> (Vec<String>, bool) {
    let title = card.source.title();
    let mut lines = Vec::new();
    if let Some(done) = &card.imported {
        let mut line = format!("Imported {}", hosts(done.new));
        if done.without_key > 0 {
            line.push_str(&format!(
                ", {} without key (PuTTY format)",
                done.without_key
            ));
        }
        lines.push(line + ".");
    }
    let mut can_import = false;
    match &card.state {
        CardState::Looking => lines.push("Looking…".into()),
        CardState::Importing => lines.push("Importing…".into()),
        CardState::Missing => lines.push(match &card.file {
            Some(file) => format!("Not found at {}", file.display()),
            None => card.source.missing(),
        }),
        CardState::Failed(error) => lines.push(error.clone()),
        CardState::Ready(outcome) => {
            if outcome.format != title {
                lines.push(format!("This is a {} file.", outcome.format));
            }
            if outcome.new == 0 {
                lines.push("Everything is already in Tern.".into());
            } else {
                can_import = true;
                let groups = match outcome.groups {
                    0 => String::new(),
                    1 => " in 1 group".into(),
                    n => format!(" in {n} groups"),
                };
                let new = match outcome.new {
                    1 => "1 new host".to_owned(),
                    n => format!("{n} new hosts"),
                };
                lines.push(format!("{new}{groups}."));
            }
            let mut also = Vec::new();
            if outcome.duplicates > 0 {
                also.push(format!("{} already there", outcome.duplicates));
            }
            if outcome.skipped > 0 {
                also.push(format!(
                    "{} skipped (not SSH, or behind a jump host or proxy)",
                    outcome.skipped
                ));
            }
            if outcome.without_key > 0 {
                also.push(format!(
                    "{} without key (PuTTY format)",
                    outcome.without_key
                ));
            }
            if !also.is_empty() {
                lines.push(also.join("; "));
            }
            if !outcome.asking.is_empty() {
                lines.push(format!(
                    "{} saved no user: enter one below, or leave it blank.",
                    hosts(outcome.asking.len())
                ));
            }
        }
    }
    (lines, can_import)
}

/// `n hosts`, or `1 host`.
fn hosts(n: usize) -> String {
    if n == 1 {
        "1 host".to_owned()
    } else {
        format!("{n} hosts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An import card of `source` reading `file`, in `state`.
    fn card(source: Source, file: Option<&str>, state: CardState) -> ImportCard {
        ImportCard {
            source,
            file: file.map(PathBuf::from),
            place: None,
            state,
            imported: None,
            users: None,
        }
    }

    fn outcome(format: &'static str, new: usize, groups: usize) -> Outcome {
        Outcome {
            format,
            new,
            groups,
            duplicates: 1,
            skipped: 2,
            without_key: 0,
            asking: Vec::new(),
        }
    }

    fn ready(format: &'static str, new: usize, groups: usize) -> CardState {
        CardState::Ready(outcome(format, new, groups))
    }

    /// Spec 0014 flow 3: a found file says how many new hosts in how many
    /// groups, with the duplicates and skips, and can be imported.
    #[test]
    fn a_card_with_new_hosts_counts_them_and_offers_import() {
        let (lines, can_import) = card_lines(&card(
            Source::MobaXterm,
            Some("MobaXterm.ini"),
            ready("MobaXterm", 3, 2),
        ));
        assert!(can_import);
        let text = lines.join("\n");
        assert_eq!(lines[0], "3 new hosts in 2 groups.", "{text}");
        assert!(text.contains("1 already there"), "{text}");
        assert!(text.contains("2 skipped"), "{text}");
        assert!(!text.contains("This is a"), "{text}");
    }

    /// Spec 0014 AC3, flow 3: with nothing new the button is disabled and
    /// the card says everything is already in Tern.
    #[test]
    fn a_card_with_nothing_new_disables_import() {
        for source in Source::ALL {
            let (lines, can_import) =
                card_lines(&card(source, Some("f"), ready(source.title(), 0, 0)));
            assert!(!can_import);
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains("Everything is already in Tern")),
                "{lines:?}"
            );
        }
    }

    /// Spec 0014 AC3, flow 4: after an import the card says what it did,
    /// next to the up-to-date state.
    #[test]
    fn after_an_import_the_card_says_how_many_were_imported() {
        let mut done = card(Source::Tabby, Some("config.yaml"), ready("Tabby", 0, 0));
        done.imported = Some(Outcome {
            duplicates: 0,
            skipped: 0,
            ..outcome("Tabby", 4, 1)
        });
        let (lines, can_import) = card_lines(&done);
        assert!(!can_import);
        assert!(
            lines.iter().any(|l| l.contains("Imported 4 hosts")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Everything is already in Tern")),
            "{lines:?}"
        );
    }

    /// Spec 0014 AC6: a Tabby file on the MobaXterm card says it's Tabby.
    #[test]
    fn a_card_says_when_its_file_is_the_other_format() {
        let (lines, can_import) = card_lines(&card(
            Source::MobaXterm,
            Some("config.yaml"),
            ready("Tabby", 2, 1),
        ));
        assert!(can_import);
        assert!(lines.iter().any(|l| l.contains("Tabby file")), "{lines:?}");
    }

    /// Spec 0014 AC5, flow 2: a chosen file that's gone names its path;
    /// with nothing chosen and nothing found, the card says what the
    /// source says, and neither can import.
    #[test]
    fn a_missing_file_names_where_it_was_looked_for() {
        let path = PathBuf::from("somewhere").join("tabby").join("config.yaml");
        let mut missing = card(Source::Tabby, None, CardState::Missing);
        missing.file = Some(path.clone());
        let (lines, can_import) = card_lines(&missing);
        assert!(!can_import);
        assert_eq!(lines, [format!("Not found at {}", path.display())]);

        for source in Source::ALL {
            let (lines, can_import) = card_lines(&card(source, None, CardState::Missing));
            assert!(!can_import);
            assert_eq!(lines, [source.missing()]);
        }
    }

    /// Spec 0014 AC7, flow 6: a failure shows its error on one line and
    /// the card can't import; neither can a card still looking.
    #[test]
    fn a_failed_card_shows_its_error_and_cannot_import() {
        let error = "x.ini is over 10 MB";
        let (lines, can_import) = card_lines(&card(
            Source::MobaXterm,
            Some("x.ini"),
            CardState::Failed(error.into()),
        ));
        assert!(!can_import);
        assert_eq!(lines, [error]);
        for state in [CardState::Looking, CardState::Importing] {
            assert!(!card_lines(&card(Source::Tabby, Some("f"), state)).1);
        }
    }

    /// A user-less PuTTY host named `name`.
    fn userless(name: &str) -> Host {
        Host {
            name: name.into(),
            host: format!("{name}.example.com"),
            port: None,
            user: None,
            identity_file: None,
            group: None,
            ppk: None,
        }
    }

    /// Spec 0015 flows 4–5: a PuTTY card counts the hosts left without a
    /// `.ppk` key and says how many saved no user.
    #[test]
    fn a_putty_card_counts_ppk_keys_and_hosts_without_a_user() {
        let state = CardState::Ready(Outcome {
            without_key: 2,
            asking: vec![userless("a"), userless("b"), userless("c")],
            ..outcome("PuTTY", 3, 0)
        });
        let (lines, can_import) = card_lines(&card(Source::PuTTY, None, state));
        assert!(can_import);
        let text = lines.join("\n");
        assert!(text.contains("2 without key (PuTTY format)"), "{text}");
        assert!(text.contains("3 hosts saved no user"), "{text}");
    }

    /// Spec 0015 flows 4–5: without dropped keys or user-less hosts the
    /// card mentions neither.
    #[test]
    fn a_card_without_ppk_keys_or_missing_users_mentions_neither() {
        let (lines, _) = card_lines(&card(Source::WinSCP, None, ready("WinSCP", 2, 1)));
        let text = lines.join("\n");
        assert!(!text.contains("without key"), "{text}");
        assert!(!text.contains("saved no user"), "{text}");
    }

    /// Spec 0015 flow 4: the result of an import adds the hosts imported
    /// without their `.ppk` key.
    #[test]
    fn after_an_import_the_card_names_hosts_without_key() {
        let mut done = card(Source::PuTTY, None, ready("PuTTY", 0, 0));
        done.imported = Some(Outcome {
            without_key: 1,
            ..outcome("PuTTY", 3, 0)
        });
        let (lines, _) = card_lines(&done);
        assert_eq!(lines[0], "Imported 3 hosts, 1 without key (PuTTY format).");
    }

    /// Spec 0015 flows 2–3: a PuTTY or WinSCP card with nothing found and
    /// no file chosen says what its source says, and can't import.
    #[test]
    fn a_missing_putty_or_winscp_card_says_what_its_source_says() {
        for source in [Source::PuTTY, Source::WinSCP] {
            let (lines, can_import) = card_lines(&card(source, None, CardState::Missing));
            assert!(!can_import);
            assert_eq!(lines, [source.missing()]);
        }
    }
}
