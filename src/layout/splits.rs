//! Splitting panes and dragging them (spec 0007): the room check, drops
//! onto a pane's side, divider drags, and drawing the split tree.

use super::chrome::{DraggedTab, TabGhost};
use super::tabs::Dragged;
use super::tree::{Axis, Direction, LeafId, Node};
use super::{Workspace, tree};
use crate::config::Profile;
use crate::terminal::pty::{self};
use crate::theme::{self, Theme};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AppContext, Bounds, Context, Div, DragMoveEvent, ElementId, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, Render, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::ActiveTheme;

/// Thickness of a split divider, in logical pixels.
pub(super) const DIVIDER: f32 = 4.0;

/// The smallest pane a divider may leave (spec 0003).
const MIN_PANE_CELLS: (f32, f32) = (10.0, 3.0);

/// Dragging a pane of a split tab by its grab handle (spec 0007).
#[derive(Clone)]
pub(super) struct DraggedPane {
    pub(super) leaf: LeafId,
    pub(super) title: SharedString,
}

/// Dragging the divider of the split at this path.
#[derive(Clone)]
struct DraggedDivider(Vec<bool>);

/// A divider drag shows nothing but the moving divider itself.
pub(super) struct DividerGhost;

impl Render for DividerGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The 6-dot handle a pane of a split tab is dragged by, in the pane's top
/// right corner, shown while the pointer is over the pane's `group` (spec
/// 0007, flow 1). It keeps the mouse from the terminal below, which ends
/// the pane's hover too, so it also shows while hovered itself. It's
/// faded out rather than hidden: a hidden element can't be hovered.
fn grab_handle(leaf: LeafId, group: &SharedString, theme: Theme) -> Stateful<Div> {
    let dot = || {
        div()
            .size(px(3.))
            .rounded_full()
            .bg(theme::hsla(theme.foreground).opacity(0.7))
    };
    let row = || div().flex().gap(px(3.)).child(dot()).child(dot());
    div()
        .id(("pane-handle", leaf))
        .absolute()
        .top(px(4.))
        .right(px(4.))
        .p(px(4.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .rounded(px(4.))
        .bg(theme::hsla(theme.background).opacity(0.85))
        .cursor_grab()
        .occlude()
        .opacity(0.)
        .hover(|style| style.opacity(1.))
        .group_hover(group.clone(), |style| style.opacity(1.))
        .child(row())
        .child(row())
        .child(row())
}

impl Workspace {
    /// Splits the focused pane, unless that would leave either half below
    /// the minimum pane size (spec 0003); then the shortcut does nothing.
    /// The new pane opens `picked` (spec 0007, flow 6), else the same
    /// profile as configured now, in the same directory, unless `picked`
    /// sets one of its own.
    pub(super) fn split(
        &mut self,
        axis: Axis,
        picked: Option<Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_room(axis, window) {
            return;
        }
        let source = self.tabs.active().focused;
        let (source_profile, source_cwd) = match self.panes.get(&source) {
            Some(pane) => {
                let pane = pane.read(cx);
                (Some(pane.profile().id.clone()), pane.cwd())
            }
            // Beside a files pane, a shell starts in the local folder shown.
            None => (
                None,
                self.files
                    .get(&source)
                    .and_then(|files| files.pane.read(cx).local_folder(cx)),
            ),
        };
        let (profile, cwd) = match picked {
            Some(profile) => {
                let cwd = source_cwd.filter(|_| profile.cwd().is_none());
                (profile, cwd)
            }
            None => {
                let profile = match source_profile {
                    Some(id) => self.config.profile_or_default(&id),
                    None => self.config.default_profile(),
                };
                (profile, source_cwd)
            }
        };
        let leaf = self.tabs.split(axis);
        self.spawn_pane(leaf, profile, cwd, window, cx);
        self.show_active(window, cx);
    }

    /// Whether the focused pane has room for a split along `axis` (spec
    /// 0003). Before the first layout there is nothing to measure yet.
    pub(super) fn has_room(&self, axis: Axis, window: &Window) -> bool {
        self.content_size.get().is_none_or(|size| {
            let area = (f32::from(size.width), f32::from(size.height));
            self.tabs
                .can_split(axis, area, self.pane_min(window), DIVIDER)
        })
    }

    /// Marks the side of pane `leaf` that `dragged` would land on, with the
    /// pointer at `position`: the nearest one, if it has room (spec 0007).
    /// Every pane hears every move of a drag, so a pane only clears the mark
    /// when it's its own.
    fn drag_over_pane(
        &mut self,
        leaf: LeafId,
        dragged: Dragged,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let side = bounds.contains(&position).then(|| {
            let x = (position.x - bounds.left()) / bounds.size.width;
            let y = (position.y - bounds.top()) / bounds.size.height;
            tree::nearest_side(x, y)
        });
        let target = side
            .filter(|&side| {
                self.content_size.get().is_some_and(|size| {
                    let area = (f32::from(size.width), f32::from(size.height));
                    let pane_min = self.pane_min(window);
                    self.tabs
                        .can_drop(dragged, leaf, side, area, pane_min, DIVIDER)
                })
            })
            .map(|side| (leaf, side));
        let ours = self.drop_target.is_some_and(|(marked, _)| marked == leaf);
        if (target.is_some() || ours) && self.drop_target != target {
            self.drop_target = target;
            cx.notify();
        }
    }

    /// Drops `dragged` on the side of pane `leaf` its drag marked (spec
    /// 0007, flows 3 and 4). Without a mark, nothing moves.
    fn drop_on_pane(
        &mut self,
        leaf: LeafId,
        dragged: Dragged,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target, side)) = self
            .drop_target
            .take()
            .filter(|(marked, _)| *marked == leaf)
        else {
            return;
        };
        if self.tabs.drop_in_split(dragged, target, side) {
            self.show_active(window, cx);
        }
        cx.notify();
    }

    /// Makes a pane dropped on the tab bar a tab of its own at `at` (spec
    /// 0007, flow 5).
    pub(super) fn pane_to_tab(
        &mut self,
        leaf: LeafId,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if self.tabs.pane_to_tab(leaf, at) {
            self.show_active(window, cx);
        }
    }

    /// Sets a divider from the mouse, keeping both sides at least
    /// `MIN_PANE_CELLS` (spec 0003).
    fn drag_divider(
        &mut self,
        path: &[bool],
        axis: Axis,
        event: &DragMoveEvent<DraggedDivider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.drag(cx).0 != path {
            return;
        }
        // A drag that ended outside the window can leave its drop mark.
        self.drop_target = None;
        let bounds = event.bounds;
        let position = event.event.position;
        let (offset, length) = match axis {
            Axis::Row => (position.x - bounds.left(), bounds.size.width),
            Axis::Column => (position.y - bounds.top(), bounds.size.height),
        };
        let (offset, length) = (f32::from(offset), f32::from(length));
        let pane_min = self.pane_min(window);
        let Some(Node::Split {
            ratio,
            first,
            second,
            ..
        }) = self.tabs.active_mut().root.split_at_mut(path)
        else {
            return;
        };
        let (first_min, second_min) = {
            let (a, b) = (
                first.min_size(pane_min, DIVIDER),
                second.min_size(pane_min, DIVIDER),
            );
            match axis {
                Axis::Row => (a.0, b.0),
                Axis::Column => (a.1, b.1),
            }
        };
        let wanted = (offset - DIVIDER / 2.0) / (length - DIVIDER);
        *ratio = tree::clamp_ratio(wanted, length, first_min, second_min, DIVIDER);
        self.save_soon(window, cx);
        cx.notify();
    }

    /// The smallest pane in pixels: `MIN_PANE_CELLS` of the current font,
    /// plus the grid's padding.
    pub(super) fn pane_min(&self, window: &Window) -> (f32, f32) {
        let cell = self.font.cell_size(window);
        (
            MIN_PANE_CELLS.0 * f32::from(cell.width) + 2.0 * pty::PADDING,
            MIN_PANE_CELLS.1 * f32::from(cell.height) + 2.0 * pty::PADDING,
        )
    }

    /// A pane, which tabs and panes are dropped on (spec 0007). In a split
    /// tab that isn't zoomed, it has the grab handle it's dragged by.
    pub(super) fn render_leaf(&self, leaf: LeafId, cx: &mut Context<Self>) -> AnyElement {
        let pane = match (self.panes.get(&leaf), self.files.get(&leaf)) {
            (Some(pane), _) => pane.clone().into_any_element(),
            (None, Some(files)) => files.pane.clone().into_any_element(),
            (None, None) => return div().size_full().into_any_element(),
        };
        let tab = self.tabs.active();
        let group = SharedString::from(format!("pane-{leaf}"));
        let handle = (tab.zoomed.is_none() && matches!(tab.root, Node::Split { .. })).then(|| {
            let title = self.pane_title(leaf, cx);
            grab_handle(leaf, &group, self.theme)
                .on_drag(DraggedPane { leaf, title }, |pane, _, _, cx| {
                    cx.new(|_| TabGhost(pane.title.clone()))
                })
        });
        let mark = self
            .drop_target
            .filter(|(marked, _)| *marked == leaf && cx.has_active_drag())
            .map(|(_, side)| {
                let theme = cx.theme();
                div()
                    .absolute()
                    .bg(theme.drop_target)
                    .border_2()
                    .border_color(theme.drag_border)
                    .map(|half| match side {
                        Direction::Left => half.top_0().left_0().h_full().w_1_2(),
                        Direction::Right => half.top_0().right_0().h_full().w_1_2(),
                        Direction::Up => half.top_0().left_0().w_full().h_1_2(),
                        Direction::Down => half.bottom_0().left_0().w_full().h_1_2(),
                    })
            });
        div()
            .group(group)
            .relative()
            .size_full()
            .child(pane)
            .children(handle)
            .children(mark)
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<DraggedTab>, window, cx| {
                    let dragged = Dragged::Tab(event.drag(cx).id);
                    this.drag_over_pane(
                        leaf,
                        dragged,
                        event.bounds,
                        event.event.position,
                        window,
                        cx,
                    );
                },
            ))
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<DraggedPane>, window, cx| {
                    let dragged = Dragged::Pane(event.drag(cx).leaf);
                    this.drag_over_pane(
                        leaf,
                        dragged,
                        event.bounds,
                        event.event.position,
                        window,
                        cx,
                    );
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.drop_on_pane(leaf, Dragged::Tab(dragged.id), window, cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, cx| {
                this.drop_on_pane(leaf, Dragged::Pane(dragged.leaf), window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn render_node(
        &self,
        node: &Node,
        path: Vec<bool>,
        pane_min: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (axis, ratio, first, second) = match node {
            Node::Leaf(leaf) => return self.render_leaf(*leaf, cx),
            Node::Split {
                axis,
                ratio,
                first,
                second,
            } => (*axis, ratio, first, second),
        };
        let key: String = path.iter().map(|&b| if b { '1' } else { '0' }).collect();
        // Both halves share the space left after the divider by weight, so
        // the first gets exactly `ratio` of it. Their minimum sizes keep a
        // shrinking window from squeezing a pane below the minimum.
        let half = |share: f32, node: &Node, child: AnyElement| {
            let (min_width, min_height) = node.min_size(pane_min, DIVIDER);
            let mut half = div()
                .flex_basis(px(0.))
                .overflow_hidden()
                .map(|half| match axis {
                    Axis::Row => half.min_w(px(min_width)).min_h_0(),
                    Axis::Column => half.min_h(px(min_height)).min_w_0(),
                })
                .child(child);
            half.style().flex_grow = Some(share);
            half.style().flex_shrink = Some(1.);
            half
        };
        let child_path = |second_child: bool| {
            let mut child = path.clone();
            child.push(second_child);
            child
        };
        let first = half(
            *ratio,
            first,
            self.render_node(first, child_path(false), pane_min, cx),
        );
        let second = half(
            1.0 - *ratio,
            second,
            self.render_node(second, child_path(true), pane_min, cx),
        );
        let divider = div()
            .id(ElementId::Name(format!("divider-{key}").into()))
            .flex_shrink_0()
            .bg(theme::hsla(self.theme.foreground).opacity(0.12))
            .map(|divider| match axis {
                Axis::Row => divider.w(px(DIVIDER)).h_full().cursor_col_resize(),
                Axis::Column => divider.h(px(DIVIDER)).w_full().cursor_row_resize(),
            })
            .on_drag(DraggedDivider(path.clone()), |_, _, _, cx| {
                cx.new(|_| DividerGhost)
            });
        let container = div()
            .id(ElementId::Name(format!("split-{key}").into()))
            .size_full()
            .flex()
            .map(|container: gpui::Stateful<Div>| match axis {
                Axis::Row => container.flex_row(),
                Axis::Column => container.flex_col(),
            })
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<DraggedDivider>, window, cx| {
                    this.drag_divider(&path, axis, event, window, cx);
                },
            ));
        container
            .child(first)
            .child(divider)
            .child(second)
            .into_any_element()
    }
}
