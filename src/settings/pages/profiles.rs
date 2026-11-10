//! The Profiles tab: the profile list and the form that adds or edits one.

use crate::config::{Profile, ProfileKind};
use crate::layout::show_notice;
use crate::settings::change::{self, Change, ProfileForm};
use crate::settings::{Page, Row, SettingsView};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, PathPromptOptions, Styled, Window,
    div, px,
};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};

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

    /// Asks the OS's folder picker for the profile form's directory and
    /// writes the pick into its field; cancelling leaves the field alone,
    /// and a picker that can't open says why.
    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
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
                    let notice = format!("The folder picker didn't open: {error}");
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
                        .find(|(label, _)| *label == "Directory")
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
                    let field = match *label {
                        "Directory" => field.child(
                            Button::new("profile-browse-cwd")
                                .label("Browse…")
                                .small()
                                .ghost()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                                ),
                        ),
                        _ => field,
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
                                .on_click(cx.listener(|this, _, _, cx| this.save_form(cx))),
                        )
                        .child(
                            Button::new("profile-cancel")
                                .label("Cancel")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.form = None;
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
