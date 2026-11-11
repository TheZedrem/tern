//! The Profiles tab: the profile list and the form that adds or edits one.

use crate::config::{Profile, ProfileKind};
use crate::layout::show_notice;
use crate::settings::change::{self, Change, ProfileForm};
use crate::settings::{Page, Row, SettingsView};
use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, PathPromptOptions,
    SharedString, Styled, Window, div, px,
};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};

/// What a form field's "Browse…" button asks the OS's picker for.
#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Folder,
    File,
}

/// The profile being added or edited.
pub(in crate::settings) struct Form {
    /// `None` for a new profile, which gets its ID when it's saved.
    id: Option<String>,
    ssh: bool,
    env: Vec<(String, String)>,
    inputs: Vec<(&'static str, Entity<InputState>)>,
    problem: Option<&'static str>,
}

impl SettingsView {
    /// Opens the profile form, empty or filled from `profile`.
    fn open_form(
        &mut self,
        ssh: bool,
        profile: Option<&Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = dirs::home_dir();
        let form = profile.map_or_else(
            || ProfileForm {
                ssh,
                ..ProfileForm::default()
            },
            |profile| ProfileForm::of(profile, home.as_deref()),
        );
        let fields: &[(&'static str, &str)] = if form.ssh {
            &[
                ("Name", &form.name),
                ("Host", &form.host),
                ("User", &form.user),
                ("Port", &form.port),
                ("Key file", &form.identity_file),
                ("Group", &form.group),
                ("Theme", &form.theme),
            ]
        } else {
            &[
                ("Name", &form.name),
                ("Command", &form.command),
                ("Directory", &form.cwd),
                ("Theme", &form.theme),
            ]
        };
        let inputs = fields
            .iter()
            .map(|(label, value)| {
                let value = value.to_string();
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
                (*label, input)
            })
            .collect();
        self.form = Some(Form {
            id: profile.map(|profile| profile.id.clone()),
            ssh: form.ssh,
            env: form.env,
            inputs,
            problem: None,
        });
        cx.notify();
    }

    /// Asks the OS's picker for a path for the profile form's `field`, a
    /// folder or a file, and writes the pick into that field, `~` for home
    /// (spec 0009, flow 4); cancelling leaves the field alone, and a picker
    /// that can't open says why.
    fn pick_path(
        &mut self,
        field: &'static str,
        pick: Pick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: pick == Pick::File,
            directories: pick == Pick::Folder,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let failed = match picked.await {
                Ok(Ok(Some(paths))) => Ok(paths),
                Ok(Ok(None)) => return,
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err("it closed without an answer".to_owned()),
            };
            let paths = match failed {
                Ok(paths) => paths,
                Err(error) => {
                    let what = match pick {
                        Pick::Folder => "folder",
                        Pick::File => "file",
                    };
                    let notice = format!("The {what} picker didn't open: {error}");
                    let _ = cx.update(|window, cx| show_notice(notice, window, cx));
                    return;
                }
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let value = change::home_relative(&path, dirs::home_dir().as_deref());
            let _ = this.update_in(cx, |this, window, cx| {
                let input = this.form.as_ref().and_then(|form| {
                    form.inputs
                        .iter()
                        .find(|(label, _)| *label == field)
                        .map(|(_, input)| input.clone())
                });
                if let Some(input) = input {
                    input.update(cx, |input, cx| input.set_value(value, window, cx));
                }
            });
        })
        .detach();
    }

    /// Saves the form as a profile (spec 0009, flow 4), unless it isn't
    /// complete.
    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        let value = |label: &str| {
            form.inputs
                .iter()
                .find(|(found, _)| *found == label)
                .map(|(_, input)| input.read(cx).value().trim().to_owned())
                .unwrap_or_default()
        };
        let profile = ProfileForm {
            name: value("Name"),
            ssh: form.ssh,
            command: value("Command"),
            cwd: value("Directory"),
            host: value("Host"),
            user: value("User"),
            port: value("Port"),
            identity_file: value("Key file"),
            group: value("Group"),
            theme: value("Theme"),
            env: form.env.clone(),
        };
        if let Some(problem) = profile.problem() {
            form.problem = Some(problem);
            cx.notify();
            return;
        }
        let id = form.id.clone().unwrap_or_else(|| {
            let mut taken: Vec<String> =
                self.config.profiles.iter().map(|p| p.id.clone()).collect();
            taken.push("shell".into());
            let name = if profile.name.is_empty() {
                &profile.host
            } else {
                &profile.name
            };
            change::new_id(name, &taken)
        });
        self.form = None;
        self.set(
            Change::Profile {
                id,
                profile: Some(Box::new(profile)),
            },
            cx,
        );
        cx.notify();
    }

    fn delete_profile(&mut self, id: String, cx: &mut Context<Self>) {
        if self.deleting.as_ref() == Some(&id) {
            self.deleting = None;
            self.set(Change::Profile { id, profile: None }, cx);
        } else {
            self.deleting = Some(id);
        }
        cx.notify();
    }

    /// The profile form while one is added or edited; it takes the
    /// screen's body.
    pub(in crate::settings) fn render_form(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let form = self.form.as_ref()?;
        let title = match (&form.id, form.ssh) {
            (Some(id), _) => format!("Edit {id}"),
            (None, true) => "New SSH profile".to_owned(),
            (None, false) => "New local profile".to_owned(),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(Self::heading(title, cx))
                .children(form.inputs.iter().map(|(label, input)| {
                    let field = div()
                        .w(px(320.))
                        .flex()
                        .gap(px(6.))
                        .child(div().flex_1().child(Input::new(input).small()));
                    let name: &'static str = label;
                    let pick = match name {
                        "Directory" => Some(Pick::Folder),
                        "Key file" => Some(Pick::File),
                        _ => None,
                    };
                    let field = match pick {
                        Some(pick) => field.child(
                            Button::new(SharedString::from(format!("profile-browse-{label}")))
                                .label("Browse…")
                                .small()
                                .ghost()
                                // Where a test clicks; a no-op outside test builds.
                                .debug_selector(move || format!("profile-browse-{name}"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.pick_path(name, pick, window, cx);
                                })),
                        ),
                        None => field,
                    };
                    Self::row(*label, field)
                }))
                .children(form.problem.map(|problem| div().text_xs().child(problem)))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            Button::new("profile-save")
                                .label("Save")
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_form(cx);
                                    if this.form.is_none() {
                                        this.keep_keyboard(window, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("profile-cancel")
                                .label("Cancel")
                                .small()
                                .ghost()
                                // Where a test clicks; a no-op outside test builds.
                                .debug_selector(|| "profile-cancel".into())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.form = None;
                                    this.keep_keyboard(window, cx);
                                    cx.notify();
                                })),
                        ),
                ),
        )
    }

    pub(in crate::settings) fn profile_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        self.config
            .profiles
            .iter()
            .enumerate()
            .map(|(ix, profile)| {
                let what = match &profile.kind {
                    ProfileKind::Local(local) => local
                        .command
                        .as_ref()
                        .map_or("login shell".to_owned(), |command| command.join(" ")),
                    ProfileKind::Ssh(ssh) => format!("ssh {}", ssh.host),
                };
                let (edit, delete) = (profile.clone(), profile.id.clone());
                let asking = self.deleting.as_ref() == Some(&profile.id);
                let (group, keywords) = match &profile.kind {
                    ProfileKind::Local(_) => ("Local profiles", format!("{} {what}", profile.id)),
                    ProfileKind::Ssh(ssh) => (
                        "SSH profiles",
                        format!(
                            "{} {what} {} {} host server",
                            profile.id,
                            ssh.user.as_deref().unwrap_or_default(),
                            ssh.group.as_deref().unwrap_or_default()
                        ),
                    ),
                };
                let control = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(200.)).text_sm().truncate().child(what))
                    .child(
                        Button::new(("profile-edit", ix))
                            .label("Edit")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let ssh = matches!(edit.kind, ProfileKind::Ssh(_));
                                this.open_form(ssh, Some(&edit), window, cx);
                            })),
                    )
                    .child(
                        Button::new(("profile-delete", ix))
                            .label(if asking { "Delete it?" } else { "Delete" })
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_profile(delete.clone(), cx);
                            })),
                    );
                Row {
                    page: Page::Profiles,
                    group,
                    label: profile.display_name().to_owned().into(),
                    keywords,
                    control: control.into_any_element(),
                }
            })
            .collect()
    }

    /// Below the profile list: adding one.
    pub(in crate::settings) fn profile_buttons(cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap(px(6.))
            .pt(px(10.))
            .child(
                Button::new("profile-add-local")
                    .label("Add local profile")
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_form(false, None, window, cx);
                    })),
            )
            .child(
                Button::new("profile-add-ssh")
                    .label("Add SSH profile")
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_form(true, None, window, cx);
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use crate::settings::Page;
    use crate::settings::tests::{Screen, open_screen};
    use gpui::TestAppContext;

    /// Spec 0009, flow 4 and criterion 8: with a profile form field holding
    /// the keyboard, closing the form with its Cancel button leaves Esc
    /// working: it leaves the screen.
    #[gpui::test]
    fn esc_after_cancelling_the_profile_form_closes_the_screen(cx: &mut TestAppContext) {
        let mut screen = open_screen(cx);
        screen.click("settings-tab-Profiles");
        let view = screen.view.clone();
        screen.cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.page, Page::Profiles);
                view.open_form(false, None, window, cx);
            })
        });
        screen.cx.run_until_parked();
        let name = screen.cx.update(|_, cx| {
            let form = view.read(cx).form.as_ref().expect("the form is open");
            form.inputs[0].1.clone()
        });
        screen.focus(&name);
        assert!(screen.is_focused(&name));

        screen.click("profile-cancel");
        let open = screen.cx.update(|_, cx| view.read(cx).form.is_some());
        assert!(!open, "Cancel closed the form");
        screen.cx.simulate_keystrokes("escape");
        assert_eq!(screen.closed.get(), 1);
    }

    /// Opens an empty profile form, an SSH one if `ssh`, on the Profiles
    /// page and draws it.
    fn open_profile_form(screen: &mut Screen<'_>, ssh: bool) {
        screen.click("settings-tab-Profiles");
        let view = screen.view.clone();
        screen
            .cx
            .update(|window, cx| view.update(cx, |view, cx| view.open_form(ssh, None, window, cx)));
        screen.cx.run_until_parked();
        screen.cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// What the open form's field `label` holds.
    fn field(screen: &mut Screen<'_>, label: &str) -> String {
        let view = screen.view.clone();
        screen.cx.update(|_, cx| {
            let form = view.read(cx).form.as_ref().expect("the form is open");
            let (_, input) = form
                .inputs
                .iter()
                .find(|(name, _)| *name == label)
                .unwrap_or_else(|| panic!("the form has no {label} field"));
            input.read(cx).value().to_string()
        })
    }

    /// Spec 0009, flow 4: the SSH form has a "Browse…" button for its key
    /// file and none for a directory, and the local form the other way
    /// round.
    #[gpui::test]
    fn the_ssh_form_browses_for_a_key_file_and_the_local_form_for_a_directory(
        cx: &mut TestAppContext,
    ) {
        let mut screen = open_screen(cx);
        open_profile_form(&mut screen, true);
        assert!(screen.cx.debug_bounds("profile-browse-Key file").is_some());
        assert!(screen.cx.debug_bounds("profile-browse-Directory").is_none());

        screen.click("profile-cancel");
        open_profile_form(&mut screen, false);
        assert!(screen.cx.debug_bounds("profile-browse-Directory").is_some());
        assert!(screen.cx.debug_bounds("profile-browse-Key file").is_none());
    }

    /// Spec 0009, flow 4 and criterion 9: "Browse…" beside an SSH
    /// profile's key file opens the file picker for one file; picking
    /// `~/.ssh/id_ed25519` fills the field with exactly that, and
    /// cancelling the picker leaves the field unchanged.
    #[gpui::test]
    fn browsing_for_a_key_file_fills_in_the_pick_and_cancelling_keeps_the_field(
        cx: &mut TestAppContext,
    ) {
        let home = dirs::home_dir().expect("home dir");
        let mut screen = open_screen(cx);
        open_profile_form(&mut screen, true);

        screen.click("profile-browse-Key file");
        assert!(screen.cx.did_prompt_for_paths(), "no picker opened");
        screen.cx.simulate_path_prompt_response(|options| {
            assert!(options.files, "the picker doesn't offer files");
            assert!(!options.directories, "the picker offers folders");
            assert!(!options.multiple, "the picker allows several files");
            Some(vec![home.join(".ssh").join("id_ed25519")])
        });
        screen.cx.run_until_parked();
        assert_eq!(field(&mut screen, "Key file"), "~/.ssh/id_ed25519");

        screen.click("profile-browse-Key file");
        screen.cx.simulate_path_prompt_response(|_| None);
        screen.cx.run_until_parked();
        assert_eq!(field(&mut screen, "Key file"), "~/.ssh/id_ed25519");
    }

    /// Spec 0009, flow 4: "Browse…" beside a local profile's directory
    /// opens the folder picker, and picking the home folder fills in `~`.
    #[gpui::test]
    fn browsing_for_a_directory_picks_a_folder_and_writes_home_as_tilde(cx: &mut TestAppContext) {
        let home = dirs::home_dir().expect("home dir");
        let mut screen = open_screen(cx);
        open_profile_form(&mut screen, false);

        screen.click("profile-browse-Directory");
        screen.cx.simulate_path_prompt_response(|options| {
            assert!(options.directories, "the picker doesn't offer folders");
            assert!(!options.files, "the picker offers files");
            Some(vec![home.clone()])
        });
        screen.cx.run_until_parked();
        assert_eq!(field(&mut screen, "Directory"), "~");
    }
}
