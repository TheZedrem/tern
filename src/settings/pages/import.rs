//! The Import tab: a card per import source (spec 0014).

use crate::import::{self, Outcome, Source};
use crate::layout::show_notice;
use crate::settings::{Page, Row, SettingsEvent, SettingsView};
use gpui::{
    AppContext, Context, IntoElement, ParentElement, PathPromptOptions, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Disableable, Sizable};
use std::path::PathBuf;

/// An import source's card on the Import tab (spec 0014, Model).
pub(in crate::settings) struct ImportCard {
    source: Source,
    /// The file it reads: the chosen one, else the default place. `None`
    /// where the app doesn't run and nothing was chosen.
    file: Option<PathBuf>,
    state: CardState,
    /// What the last import from this card did.
    imported: Option<Outcome>,
}

pub(in crate::settings) enum CardState {
    Looking,
    /// No file to read.
    Missing,
    /// What importing the file would do.
    Ready(Outcome),
    Importing,
    Failed(String),
}

impl SettingsView {
    /// Looks at every import source's file afresh (spec 0014, flow 1):
    /// the default place, or the file chosen on its card.
    pub(in crate::settings) fn look_for_imports(&mut self, cx: &mut Context<Self>) {
        if self.imports.is_empty() {
            self.imports = Source::ALL
                .into_iter()
                .map(|source| ImportCard {
                    source,
                    file: source.default_path(),
                    state: CardState::Looking,
                    imported: None,
                })
                .collect();
        }
        for source in Source::ALL {
            self.import_from(source, None, false, cx);
        }
    }

    /// Plans the import of `source`'s card or, with `write`, runs it, from
    /// `file` if given, on the background executor; the card shows the
    /// outcome. An import plans again after writing, so the card shows
    /// what's left (spec 0014, flow 4).
    fn import_from(
        &mut self,
        source: Source,
        file: Option<PathBuf>,
        write: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.imports.iter_mut().find(|card| card.source == source) else {
            return;
        };
        if let Some(file) = file {
            card.file = Some(file);
            card.imported = None;
        }
        let Some(file) = card.file.clone() else {
            card.state = CardState::Missing;
            cx.notify();
            return;
        };
        card.state = if write {
            CardState::Importing
        } else {
            CardState::Looking
        };
        cx.notify();
        let work = cx.background_spawn(async move {
            if !file.exists() {
                return (None, None);
            }
            let imported = write.then(|| import::outcome(&file, false));
            let planned = match &imported {
                Some(Err(error)) => Err(error.clone()),
                _ => import::outcome(&file, true),
            };
            (imported, Some(planned))
        });
        cx.spawn(async move |this, cx| {
            let (imported, planned) = work.await;
            let _ = this.update(cx, |this, cx| {
                if imported.as_ref().is_some_and(Result::is_ok) {
                    cx.emit(SettingsEvent::Written);
                }
                let Some(card) = this.imports.iter_mut().find(|card| card.source == source) else {
                    return;
                };
                if let Some(Ok(outcome)) = imported {
                    card.imported = Some(outcome);
                }
                card.state = match planned {
                    None => CardState::Missing,
                    Some(Ok(outcome)) => CardState::Ready(outcome),
                    Some(Err(error)) => CardState::Failed(error),
                };
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
                let _ = this.update(cx, |this, cx| {
                    this.import_from(source, Some(path), false, cx)
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
                let file = card
                    .file
                    .as_ref()
                    .filter(|_| !matches!(card.state, CardState::Missing))
                    .map(|file| file.display().to_string());
                let control = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .py(px(4.))
                    .children(lines.into_iter().map(|line| div().text_sm().child(line)))
                    .children(
                        file.map(|file| div().text_xs().text_color(muted).truncate().child(file)),
                    )
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.import_from(source, None, true, cx);
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
/// has anything to do (spec 0014, flows 2 to 6).
fn card_lines(card: &ImportCard) -> (Vec<String>, bool) {
    let title = card.source.title();
    let mut lines = Vec::new();
    if let Some(done) = &card.imported {
        lines.push(format!("Imported {}.", hosts(done.new)));
    }
    let mut can_import = false;
    match &card.state {
        CardState::Looking => lines.push("Looking…".into()),
        CardState::Importing => lines.push("Importing…".into()),
        CardState::Missing => lines.push(match &card.file {
            Some(file) => format!("Not found at {}", file.display()),
            None => format!("{title} runs on Windows: choose an exported .mxtsessions file."),
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
            if !also.is_empty() {
                lines.push(also.join("; "));
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
            state,
            imported: None,
        }
    }

    fn ready(format: &'static str, new: usize, groups: usize) -> CardState {
        CardState::Ready(Outcome {
            format,
            new,
            groups,
            duplicates: 1,
            skipped: 2,
        })
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
            format: "Tabby",
            new: 4,
            groups: 1,
            duplicates: 0,
            skipped: 0,
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

    /// Spec 0014 AC5, flow 2: no file at the default place names the
    /// place; MobaXterm where it doesn't run asks for an export instead,
    /// and neither can import.
    #[test]
    fn a_missing_file_names_where_it_was_looked_for() {
        let path = PathBuf::from("somewhere").join("tabby").join("config.yaml");
        let mut missing = card(Source::Tabby, None, CardState::Missing);
        missing.file = Some(path.clone());
        let (lines, can_import) = card_lines(&missing);
        assert!(!can_import);
        assert_eq!(lines, [format!("Not found at {}", path.display())]);

        let (lines, can_import) = card_lines(&card(Source::MobaXterm, None, CardState::Missing));
        assert!(!can_import);
        let text = lines.join("\n");
        assert!(text.contains(".mxtsessions"), "{text}");
        assert!(text.contains("Windows"), "{text}");
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
}
