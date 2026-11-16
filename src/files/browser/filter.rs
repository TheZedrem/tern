//! The filter field (spec 0013, Filter and flow 8): typing on the tree, or
//! Ctrl+F, puts the keyboard here; each keystroke filters the loaded tree
//! and selects the best match. Up and Down move the tree's selection,
//! Enter opens it, Esc clears the filter and goes back to the tree.

use super::Browser;
use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    ScrollStrategy, Styled, Subscription, Window, div, px,
};
use gpui_component::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_component::{Icon, IconName, Sizable};

/// The filter field and its events.
pub(super) struct Filter {
    pub(super) input: Entity<InputState>,
    _events: Subscription,
}

impl Filter {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Browser>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter (Ctrl+F)"));
        let events = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::Change => this.apply_filter(cx),
            InputEvent::PressEnter { .. } => {
                if let Some(ix) = this.selected {
                    this.open(ix, cx);
                }
                // A folder opened becomes the root, which clears the filter.
                if !this.tree.is_filtered() {
                    window.focus(&this.focus, cx);
                }
            }
            _ => {}
        });
        Self {
            input,
            _events: events,
        }
    }
}

impl Browser {
    /// Filters the tree by what the field holds and selects the best match.
    pub(super) fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.input.read(cx).value().to_string();
        let keep = self.selected_path();
        self.tree.set_filter(&query);
        match self.tree.best_match() {
            Some(best) => {
                self.selected = Some(best);
                self.scroll.scroll_to_item(best, ScrollStrategy::Nearest);
            }
            None => self.reselect(keep),
        }
        cx.notify();
    }

    /// Empties the field, which shows every row again.
    pub(super) fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.filter.input.read(cx).value().is_empty() {
            self.filter
                .input
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.apply_filter(cx);
    }

    /// Ctrl+F: the keyboard goes to the filter field.
    pub(super) fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
    }

    /// A printable key typed on the tree starts the filter with it (flow
    /// 8). Keys bound on the tree never get here: actions go first.
    pub(super) fn type_to_filter(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        let Some(typed) = keystroke
            .key_char
            .clone()
            .filter(|typed| !typed.trim().is_empty() && !typed.chars().any(char::is_control))
        else {
            return;
        };
        cx.stop_propagation();
        self.filter.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.insert(typed, window, cx);
        });
        self.apply_filter(cx);
    }

    /// The field, under the path field; Up, Down and Esc are the
    /// browser's while it has the keyboard.
    pub(super) fn render_filter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(6.))
            .pb(px(4.))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.select(-1, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.select(1, cx);
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.clear_filter(window, cx);
                window.focus(&this.focus, cx);
            }))
            .child(
                Input::new(&self.filter.input)
                    .xsmall()
                    .prefix(Icon::new(IconName::Search).xsmall()),
            )
    }
}
