//! The permissions panel (spec 0013, Permissions and flow 10): Ctrl+P or
//! the context menu opens it over the tree, naming the entry and the
//! target. SFTP and a Unix disk show the mode, owner and group; a Windows
//! disk its read-only flag and Explorer's Properties. Enter applies what
//! changed, Esc cancels.

use super::{Browser, BrowserEvent, Link, PERMS_CONTEXT};
use crate::files::permissions::{Change, Perms, Who, mode_text, parse_mode};
use crate::terminal::clean_title;
use gpui::{
    AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Window, actions, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Escape, Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Sizable};

actions!(files, [ApplyPermissions, CancelPermissions, ToggleReadOnly]);

/// The panel's own keys, for the Windows form, which has no text field.
pub(super) fn bindings() -> [gpui::KeyBinding; 3] {
    [
        gpui::KeyBinding::new("enter", ApplyPermissions, Some(PERMS_CONTEXT)),
        gpui::KeyBinding::new("escape", CancelPermissions, Some(PERMS_CONTEXT)),
        gpui::KeyBinding::new("space", ToggleReadOnly, Some(PERMS_CONTEXT)),
    ]
}

/// The open panel: the entry, what its listing said, and what's typed.
pub(super) struct PermsPanel {
    path: String,
    /// The entry's name, by the title rules.
    name: SharedString,
    current: Perms,
    form: Form,
    focus: FocusHandle,
    _events: Vec<Subscription>,
}

enum Form {
    /// SFTP or a Unix disk.
    Unix {
        mode: Entity<InputState>,
        owner: Entity<InputState>,
        group: Entity<InputState>,
    },
    /// A Windows disk: the read-only flag as ticked.
    Windows { readonly: bool },
}

impl Browser {
    /// Ctrl+P: the selected entry's permissions, where its target has
    /// them; a link has none of its own.
    pub(super) fn open_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((entry, path)) = self.selected.and_then(|ix| self.entry(ix)) else {
            return;
        };
        if entry.link {
            cx.emit(BrowserEvent::Notice(
                "A link has no permissions of its own".into(),
            ));
            return;
        }
        let (current, name) = (entry.perms.clone(), clean_title(&entry.name));
        let focus = cx.focus_handle();
        let mut events = Vec::new();
        let form = if let Some(readonly) = current.readonly {
            window.focus(&focus, cx);
            Form::Windows { readonly }
        } else if current.mode.is_some() {
            let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| InputState::new(window, cx).default_value(value))
            };
            let label = |id: &Option<crate::files::permissions::Id>| {
                id.as_ref().map(|id| id.label()).unwrap_or_default()
            };
            let mode = field(
                format!("{:03o}", current.mode.unwrap_or_default()),
                window,
                cx,
            );
            let owner = field(label(&current.owner), window, cx);
            let group = field(label(&current.group), window, cx);
            for input in [&mode, &owner, &group] {
                events.push(
                    cx.subscribe_in(input, window, |this, _, event, window, cx| {
                        match event {
                            InputEvent::PressEnter { .. } => this.apply_permissions(window, cx),
                            // The mode's `rwx` form follows what's typed.
                            InputEvent::Change => cx.notify(),
                            _ => {}
                        }
                    }),
                );
            }
            mode.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            Form::Unix { mode, owner, group }
        } else {
            let notice = format!("{} has no permissions to show on {}", name, self.target);
            cx.emit(BrowserEvent::Notice(notice));
            return;
        };
        self.perms = Some(PermsPanel {
            path,
            name: name.into(),
            current,
            form,
            focus,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.perms.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Enter: sends what changed, if anything did, and closes the panel;
    /// typing that can't be applied says why and keeps it open.
    fn apply_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.perms else {
            return;
        };
        let applied = match &panel.form {
            Form::Windows { readonly } => {
                let readonly = (Some(*readonly) != panel.current.readonly).then_some(*readonly);
                self.send_local_change(
                    panel.path.clone(),
                    Change {
                        readonly,
                        ..Change::default()
                    },
                );
                Ok(())
            }
            Form::Unix { mode, owner, group } => {
                let typed = |input: &Entity<InputState>| input.read(cx).value().to_string();
                self.apply_unix(typed(mode), typed(owner), typed(group))
            }
        };
        match applied {
            Ok(()) => self.close_permissions(window, cx),
            Err(why) => cx.emit(BrowserEvent::Notice(why)),
        }
    }

    /// The Unix form's change: the mode in octal, the owner and group as
    /// names or numbers; what reads as it did stays.
    fn apply_unix(&self, mode: String, owner: String, group: String) -> Result<(), String> {
        let Some(panel) = self.perms.as_ref() else {
            return Ok(());
        };
        let current = &panel.current;
        let mode = parse_mode(&mode).ok_or("The mode is 3 or 4 octal digits, like 755")?;
        let mode = (Some(mode) != current.mode).then_some(mode);
        let changed = |typed: String, now: &Option<crate::files::permissions::Id>| {
            let unchanged = now.as_ref().is_some_and(|id| id.label() == typed.trim());
            if unchanged { None } else { Who::parse(&typed) }
        };
        let owner = changed(owner, &current.owner);
        let group = changed(group, &current.group);
        match &self.link {
            Some(Link::Local(_)) => {
                self.send_local_change(
                    panel.path.clone(),
                    Change {
                        mode,
                        owner,
                        group,
                        readonly: None,
                    },
                );
                Ok(())
            }
            Some(Link::Sftp(client)) => {
                // SFTP v3 sets owner and group as a pair, by number; a name
                // is one a loaded listing put a number to.
                let number = |who: Option<Who>,
                              now: &Option<crate::files::permissions::Id>,
                              group: bool| {
                    match who {
                        None => now
                            .as_ref()
                            .map(|id| id.id)
                            .ok_or_else(|| format!("{} doesn't say who owns it", self.target)),
                        Some(Who::Number(id)) => Ok(id),
                        Some(Who::Name(name)) => self.tree.find_id(&name, group).ok_or_else(|| {
                            let what = if group { "group" } else { "user" };
                            format!(
                                "No listing names the {what} {name} on {}: type its number",
                                self.target
                            )
                        }),
                    }
                };
                let ids = if owner.is_some() || group.is_some() {
                    Some((
                        number(owner, &current.owner, false)?,
                        number(group, &current.group, true)?,
                    ))
                } else {
                    None
                };
                if mode.is_some() || ids.is_some() {
                    client.set_perms(panel.path.clone(), mode, ids);
                }
                Ok(())
            }
            None => Err(format!("{} isn't connected", self.target)),
        }
    }

    fn send_local_change(&self, path: String, change: Change) {
        if change != Change::default()
            && let Some(Link::Local(worker)) = &self.link
        {
            worker.set_perms(path, change);
        }
    }

    /// Windows: Explorer's properties dialog for the entry, where its
    /// ACLs and owner live; the panel closes.
    #[cfg(windows)]
    fn show_properties(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(panel), Some(Link::Local(worker))) = (&self.perms, &self.link) {
            worker.properties(panel.path.clone());
        }
        self.close_permissions(window, cx);
    }

    fn panel_focused(&self, window: &Window) -> bool {
        self.perms
            .as_ref()
            .is_some_and(|panel| panel.focus.is_focused(window))
    }

    /// Ticks or clears the Windows form's read-only flag; `false` when
    /// the panel has none.
    fn toggle_readonly(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(PermsPanel {
            form: Form::Windows { readonly },
            ..
        }) = &mut self.perms
        else {
            return false;
        };
        *readonly = !*readonly;
        cx.notify();
        true
    }

    pub(super) fn render_permissions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let panel = self.perms.as_ref()?;
        let theme = cx.theme();
        let (border, background, muted) = (theme.border, theme.popover, theme.muted_foreground);
        let title = format!("Permissions of {} on {}", panel.name, self.target);
        let row = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(48.)).text_color(muted).child(label))
        };
        let form = match &panel.form {
            Form::Unix { mode, owner, group } => {
                let typed = mode.read(cx).value();
                let shown = parse_mode(&typed)
                    .map(mode_text)
                    .unwrap_or_else(|| "—".into());
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        row("Mode")
                            .child(div().w(px(64.)).child(Input::new(mode).xsmall()))
                            .child(div().font_family("monospace").child(shown)),
                    )
                    .child(row("Owner").child(div().flex_1().child(Input::new(owner).xsmall())))
                    .child(row("Group").child(div().flex_1().child(Input::new(group).xsmall())))
                    .into_any_element()
            }
            Form::Windows { readonly } => {
                let windows_row = div().flex().items_center().gap(px(8.)).child(
                    Checkbox::new("files-perms-readonly")
                        .label("Read-only (Space)")
                        .checked(*readonly)
                        .on_click(cx.listener(|this, _: &bool, _, cx| {
                            this.toggle_readonly(cx);
                        })),
                );
                #[cfg(windows)]
                let windows_row = windows_row.child(
                    Button::new("files-perms-properties")
                        .label("Properties…")
                        .xsmall()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_properties(window, cx)),
                        ),
                );
                windows_row.into_any_element()
            }
        };
        Some(
            div()
                .key_context(PERMS_CONTEXT)
                .track_focus(&panel.focus)
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    cx.stop_propagation();
                    this.close_permissions(window, cx);
                }))
                // Enter and Space are the panel's only while it has the
                // keyboard itself: on a button they press it, and in the
                // Unix form's fields Space is typed text.
                .on_action(cx.listener(|this, _: &ApplyPermissions, window, cx| {
                    if this.panel_focused(window) {
                        this.apply_permissions(window, cx);
                    } else {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &CancelPermissions, window, cx| {
                    this.close_permissions(window, cx);
                }))
                .on_action(cx.listener(|this, _: &ToggleReadOnly, window, cx| {
                    if !this.panel_focused(window) || !this.toggle_readonly(cx) {
                        cx.propagate();
                    }
                }))
                .m(px(6.))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(px(4.))
                .border_1()
                .border_color(border)
                .bg(background)
                .text_xs()
                .child(div().whitespace_normal().child(title))
                .child(form)
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            Button::new("files-perms-apply")
                                .label("Apply (Enter)")
                                .primary()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_permissions(window, cx)
                                })),
                        )
                        .child(
                            Button::new("files-perms-cancel")
                                .label("Cancel")
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_permissions(window, cx)
                                })),
                        ),
                ),
        )
    }
}
