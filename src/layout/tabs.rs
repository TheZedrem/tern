//! A window's tabs and their split trees (spec 0003). Pure data: the
//! workspace owns the panes and asks this model what to show, focus,
//! start and stop.

use super::tree::{Axis, Direction, LeafId, Node, Rect};

pub type TabId = u64;

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: TabId,
    /// The user's name for the tab; wins over the shell's title.
    pub name: Option<String>,
    pub root: Node,
    pub focused: LeafId,
    pub zoomed: Option<LeafId>,
}

/// What closing panes did to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// The tab still has panes.
    Pane,
    /// The tab closed too.
    Tab,
    /// That was the last tab: the app quits.
    Last,
}

/// What a drag into a split carries (spec 0007).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dragged {
    /// A pane of the active tab, by its grab handle.
    Pane(LeafId),
    /// A whole tab, from the tab bar.
    Tab(TabId),
}

/// The tabs of one window. Never empty: closing the last tab reports
/// [`Closed::Last`] and leaves the model as it was.
#[derive(Debug)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
}

impl Tabs {
    /// A window with one tab of one pane, which is returned.
    pub fn new() -> (Self, LeafId) {
        let mut tabs = Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
        };
        let leaf = tabs.new_tab();
        (tabs, leaf)
    }

    /// Restored tabs, tab `active` shown (spec 0003, flow 6). Their IDs are
    /// this run's own, and new tabs and panes get IDs past the restored
    /// panes'. `None` without tabs, or with pane IDs too large to count on
    /// from.
    pub fn restore(tabs: Vec<Tab>, active: usize) -> Option<Self> {
        let last_leaf = tabs.iter().flat_map(|tab| tab.root.leaves()).max()?;
        let next_id = last_leaf.checked_add(1)?;
        // The restored tabs take the IDs after it.
        next_id.checked_add(tabs.len() as u64)?;
        let mut restored = Self {
            active: active.min(tabs.len() - 1),
            tabs,
            next_id,
        };
        for ix in 0..restored.tabs.len() {
            restored.tabs[ix].id = restored.next_id();
        }
        Some(restored)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Opens a tab at the end, activates it and returns its pane.
    pub fn new_tab(&mut self) -> LeafId {
        let leaf = self.next_id();
        let id = self.next_id();
        self.tabs.push(Tab {
            id,
            name: None,
            root: Node::Leaf(leaf),
            focused: leaf,
            zoomed: None,
        });
        self.active = self.tabs.len() - 1;
        leaf
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// Whether the focused pane has room for two panes of at least
    /// `pane_min` pixels along `axis`, when the tab fills `area` (spec 0003:
    /// no pane below the minimum).
    pub fn can_split(
        &self,
        axis: Axis,
        area: (f32, f32),
        pane_min: (f32, f32),
        divider: f32,
    ) -> bool {
        let tab = self.active();
        let Some((_, rect)) = tab
            .root
            .rects(Rect::UNIT)
            .into_iter()
            .find(|(id, _)| *id == tab.focused)
        else {
            return false;
        };
        match axis {
            Axis::Row => rect.width * area.0 >= 2.0 * pane_min.0 + divider,
            Axis::Column => rect.height * area.1 >= 2.0 * pane_min.1 + divider,
        }
    }

    /// Splits the active tab's focused pane; the new pane, returned, takes
    /// focus. Ends a zoom.
    pub fn split(&mut self, axis: Axis) -> LeafId {
        let leaf = self.next_id();
        let tab = &mut self.tabs[self.active];
        tab.root.split(tab.focused, leaf, axis);
        tab.focused = leaf;
        tab.zoomed = None;
        leaf
    }

    /// Closes `leaf` in whichever tab holds it. Focus moves to the pane
    /// that takes its place; the tab closes with its last pane.
    pub fn close_pane(&mut self, leaf: LeafId) -> Closed {
        let Some(ix) = self.tabs.iter().position(|tab| tab.root.contains(leaf)) else {
            return Closed::Pane;
        };
        if self.detach(ix, leaf) {
            Closed::Pane
        } else {
            self.remove_tab(ix)
        }
    }

    /// Takes `leaf` out of tab `ix` as closing it would, the pane that
    /// takes its place getting its focus, but leaves the pane running.
    /// False if it's the tab's only pane.
    fn detach(&mut self, ix: usize, leaf: LeafId) -> bool {
        let tab = &mut self.tabs[ix];
        let Some(successor) = tab.root.successor(leaf) else {
            return false;
        };
        let root = std::mem::replace(&mut tab.root, Node::Leaf(successor));
        tab.root = root.remove(leaf).unwrap_or(Node::Leaf(successor));
        if tab.focused == leaf {
            tab.focused = successor;
        }
        if tab.zoomed == Some(leaf) {
            tab.zoomed = None;
        }
        true
    }

    /// What dropping `dragged` on pane `target` of the active tab puts into
    /// a split, and the tree it goes into: the active tab's, without the
    /// pane if a pane of its own moves. `None` for a drop on the dragged
    /// pane or tab itself.
    fn drop_parts(&self, dragged: Dragged, target: LeafId) -> Option<(Node, Node)> {
        let tab = self.active();
        if !tab.root.contains(target) {
            return None;
        }
        match dragged {
            Dragged::Pane(leaf) if leaf != target && tab.root.contains(leaf) => {
                Some((Node::Leaf(leaf), tab.root.clone().remove(leaf)?))
            }
            Dragged::Pane(_) => None,
            Dragged::Tab(id) => {
                let ix = self.index_of(id).filter(|&ix| ix != self.active)?;
                Some((self.tabs[ix].root.clone(), tab.root.clone()))
            }
        }
    }

    /// Whether dropping `dragged` on `side` of pane `target` in the active
    /// tab would move anything: not onto itself, and with room for both
    /// halves when the tab fills `area` (spec 0007).
    pub fn can_drop(
        &self,
        dragged: Dragged,
        target: LeafId,
        side: Direction,
        area: (f32, f32),
        pane_min: (f32, f32),
        divider: f32,
    ) -> bool {
        self.drop_parts(dragged, target)
            .is_some_and(|(node, rest)| {
                rest.can_insert(target, &node, side, area, pane_min, divider)
            })
    }

    /// Drops `dragged` on `side` of pane `target` in the active tab: a pane
    /// leaves its old place, a tab brings its whole tree and closes. What
    /// moved has the focus, and the zoom ends (spec 0007, flows 3 and 4).
    /// False if nothing moved. Room is the caller's check
    /// ([`Self::can_drop`]).
    pub fn drop_in_split(&mut self, dragged: Dragged, target: LeafId, side: Direction) -> bool {
        let Some((node, mut root)) = self.drop_parts(dragged, target) else {
            return false;
        };
        let focused = match dragged {
            Dragged::Pane(leaf) => leaf,
            Dragged::Tab(id) => self
                .index_of(id)
                .map_or_else(|| node.first_leaf(), |ix| self.tabs[ix].focused),
        };
        root.insert(target, node, side);
        if let Dragged::Tab(id) = dragged
            && let Some(ix) = self.index_of(id)
        {
            let active_id = self.active().id;
            self.tabs.remove(ix);
            self.active = self.index_of(active_id).unwrap_or(0);
        }
        let tab = self.active_mut();
        tab.root = root;
        tab.focused = focused;
        tab.zoomed = None;
        true
    }

    /// Makes pane `leaf` of the active tab a tab of its own at position
    /// `at`, or at the end past it, and activates it (spec 0007, flow 5).
    /// False if it's not in the active tab or is its only pane.
    pub fn pane_to_tab(&mut self, leaf: LeafId, at: usize) -> bool {
        if !self.active().root.contains(leaf) || !self.detach(self.active, leaf) {
            return false;
        }
        let id = self.next_id();
        let at = at.min(self.tabs.len());
        self.tabs.insert(
            at,
            Tab {
                id,
                name: None,
                root: Node::Leaf(leaf),
                focused: leaf,
                zoomed: None,
            },
        );
        self.active = at;
        true
    }

    /// Closes tab `ix` with every pane in it. Returns those panes, so their
    /// programs can be stopped, and what happened.
    pub fn close_tab(&mut self, ix: usize) -> (Vec<LeafId>, Closed) {
        let Some(tab) = self.tabs.get(ix) else {
            return (Vec::new(), Closed::Pane);
        };
        let leaves = tab.root.leaves();
        (leaves, self.remove_tab(ix))
    }

    fn remove_tab(&mut self, ix: usize) -> Closed {
        if self.tabs.len() == 1 {
            return Closed::Last;
        }
        self.tabs.remove(ix);
        // The tab to the right takes the closed one's place, or the one to
        // its left if it was the last.
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        Closed::Tab
    }

    /// Activates tab `ix`; out of range does nothing.
    pub fn activate(&mut self, ix: usize) {
        if ix < self.tabs.len() {
            self.active = ix;
        }
    }

    pub fn next_tab(&mut self) {
        self.active = (self.active + 1) % self.tabs.len();
    }

    pub fn previous_tab(&mut self) {
        self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
    }

    /// Moves tab `from` to position `to`; the same tab stays active.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active_id = self.tabs[self.active].id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self.index_of(active_id).unwrap_or(0);
    }

    /// Names tab `ix`. A blank name goes back to the shell's title.
    pub fn rename(&mut self, ix: usize, name: Option<String>) {
        if let Some(tab) = self.tabs.get_mut(ix) {
            tab.name = name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty());
        }
    }

    /// Focuses `leaf` and activates its tab. False if no tab holds it.
    pub fn focus(&mut self, leaf: LeafId) -> bool {
        let Some(ix) = self.tabs.iter().position(|tab| tab.root.contains(leaf)) else {
            return false;
        };
        self.active = ix;
        self.tabs[ix].focused = leaf;
        true
    }

    /// Moves focus to the pane next to the focused one. Does nothing while
    /// zoomed or at the edge; returns the newly focused pane.
    pub fn focus_direction(&mut self, direction: Direction) -> Option<LeafId> {
        let tab = &mut self.tabs[self.active];
        if tab.zoomed.is_some() {
            return None;
        }
        let next = tab.root.neighbor(tab.focused, direction)?;
        tab.focused = next;
        Some(next)
    }

    /// Zooms the focused pane to fill its tab, or ends the zoom. A tab with
    /// one pane has nothing to zoom.
    pub fn toggle_zoom(&mut self) {
        let tab = &mut self.tabs[self.active];
        tab.zoomed = match tab.zoomed {
            Some(_) => None,
            None if matches!(tab.root, Node::Leaf(_)) => None,
            None => Some(tab.focused),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The tabs' IDs, left to right.
    fn order(tabs: &Tabs) -> Vec<TabId> {
        tabs.tabs().iter().map(|tab| tab.id).collect()
    }

    /// Four tabs with the third one active, and their IDs left to right.
    fn four_tabs() -> (Tabs, [TabId; 4]) {
        let (mut tabs, _) = Tabs::new();
        for _ in 0..3 {
            tabs.new_tab();
        }
        tabs.activate(2);
        let ids = order(&tabs).try_into().expect("four tabs");
        (tabs, ids)
    }

    /// A split, for writing out the layout a tab should have.
    fn split(axis: Axis, ratio: f32, first: Node, second: Node) -> Node {
        Node::Split {
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn a_new_window_has_one_tab_with_one_focused_pane() {
        let (tabs, leaf) = Tabs::new();
        assert_eq!(tabs.tabs().len(), 1);
        assert_eq!(tabs.active_index(), 0);
        let tab = tabs.active();
        assert_eq!(tab.root, Node::Leaf(leaf));
        assert_eq!(tab.focused, leaf);
        assert_eq!(tab.zoomed, None);
        assert_eq!(tab.name, None);
    }

    #[test]
    fn a_new_tab_opens_at_the_end_and_becomes_active() {
        let (mut tabs, first) = Tabs::new();
        tabs.new_tab();
        tabs.activate(0);
        let leaf = tabs.new_tab();
        assert_ne!(leaf, first);
        assert_eq!(tabs.tabs().len(), 3);
        assert_eq!(tabs.active_index(), 2);
        assert_eq!(tabs.active().root, Node::Leaf(leaf));
        assert_eq!(tabs.active().focused, leaf);
    }

    #[test]
    fn split_divides_the_focused_pane_and_focuses_the_new_one() {
        let (mut tabs, first) = Tabs::new();
        let right = tabs.split(Axis::Row);
        assert_eq!(tabs.active().focused, right);
        assert_eq!(
            tabs.active().root.neighbor(first, Direction::Right),
            Some(right)
        );
        let below = tabs.split(Axis::Column);
        assert_eq!(tabs.active().focused, below);
        assert_eq!(
            tabs.active().root.neighbor(below, Direction::Up),
            Some(right)
        );
        assert_eq!(tabs.active().root.leaves().len(), 3);
    }

    #[test]
    fn split_ends_a_zoom() {
        let (mut tabs, _) = Tabs::new();
        tabs.split(Axis::Row);
        tabs.toggle_zoom();
        tabs.split(Axis::Row);
        assert_eq!(tabs.active().zoomed, None);
    }

    #[test]
    fn closing_the_focused_pane_focuses_the_one_taking_its_place() {
        let (mut tabs, left) = Tabs::new();
        let top = tabs.split(Axis::Row);
        let bottom = tabs.split(Axis::Column);
        // left | top over bottom
        tabs.focus(left);
        assert_eq!(tabs.close_pane(left), Closed::Pane);
        assert_eq!(tabs.active().focused, top);
        assert_eq!(tabs.active().root.leaves(), [top, bottom]);
        assert_eq!(tabs.close_pane(top), Closed::Pane);
        assert_eq!(tabs.active().focused, bottom);
        assert_eq!(tabs.active().root, Node::Leaf(bottom));
    }

    #[test]
    fn closing_another_pane_keeps_the_focus() {
        let (mut tabs, left) = Tabs::new();
        let right = tabs.split(Axis::Row);
        assert_eq!(tabs.close_pane(left), Closed::Pane);
        assert_eq!(tabs.active().focused, right);
    }

    #[test]
    fn closing_the_zoomed_pane_ends_the_zoom() {
        let (mut tabs, _) = Tabs::new();
        tabs.split(Axis::Row);
        let zoomed = tabs.split(Axis::Row);
        tabs.toggle_zoom();
        tabs.close_pane(zoomed);
        assert_eq!(tabs.active().zoomed, None);
    }

    #[test]
    fn a_tab_closes_with_its_last_pane() {
        let (mut tabs, first) = Tabs::new();
        let second = tabs.new_tab();
        assert_eq!(tabs.close_pane(second), Closed::Tab);
        assert_eq!(tabs.tabs().len(), 1);
        assert_eq!(tabs.active().root, Node::Leaf(first));
    }

    #[test]
    fn closing_the_last_pane_of_the_last_tab_reports_last_and_changes_nothing() {
        let (mut tabs, leaf) = Tabs::new();
        let before = tabs.tabs().to_vec();
        assert_eq!(tabs.close_pane(leaf), Closed::Last);
        assert_eq!(tabs.tabs(), before);
        assert_eq!(tabs.close_tab(0), (vec![leaf], Closed::Last));
        assert_eq!(tabs.tabs(), before);
    }

    #[test]
    fn closing_a_tab_returns_all_of_its_panes() {
        let (mut tabs, first) = Tabs::new();
        let mut opened = vec![
            tabs.new_tab(),
            tabs.split(Axis::Row),
            tabs.split(Axis::Column),
        ];
        let (mut closed, what) = tabs.close_tab(1);
        closed.sort_unstable();
        opened.sort_unstable();
        assert_eq!((closed, what), (opened, Closed::Tab));
        assert_eq!(tabs.tabs().len(), 1);
        assert_eq!(tabs.active().root, Node::Leaf(first));
    }

    #[test]
    fn closing_a_pane_in_a_background_tab_keeps_the_active_tab() {
        let (mut tabs, first) = Tabs::new();
        let second = tabs.split(Axis::Row);
        tabs.new_tab();
        let active = tabs.active().id;
        assert_eq!(tabs.close_pane(first), Closed::Pane);
        assert_eq!(tabs.active().id, active);
        assert_eq!(tabs.tabs()[0].root, Node::Leaf(second));
        assert_eq!(tabs.close_pane(second), Closed::Tab);
        assert_eq!(order(&tabs), [active]);
    }

    #[test]
    fn closing_a_tab_before_or_after_the_active_one_keeps_it_active() {
        let (mut tabs, [_, _, c, _]) = four_tabs();
        tabs.close_tab(0);
        assert_eq!(tabs.active().id, c);
        tabs.close_tab(2);
        assert_eq!(tabs.active().id, c);
    }

    #[test]
    fn closing_the_active_tab_activates_the_one_to_its_right_or_else_left() {
        let (mut tabs, [_, b, _, d]) = four_tabs();
        tabs.close_tab(2);
        assert_eq!(tabs.active().id, d);
        // d is the last tab now.
        tabs.close_tab(2);
        assert_eq!(tabs.active().id, b);
    }

    #[test]
    fn next_and_previous_tab_wrap_around() {
        let (mut tabs, _) = Tabs::new();
        tabs.new_tab();
        tabs.new_tab();
        tabs.next_tab();
        assert_eq!(tabs.active_index(), 0);
        tabs.previous_tab();
        assert_eq!(tabs.active_index(), 2);
        tabs.previous_tab();
        assert_eq!(tabs.active_index(), 1);

        let (mut single, _) = Tabs::new();
        single.next_tab();
        single.previous_tab();
        assert_eq!(single.active_index(), 0);
    }

    #[test]
    fn going_to_a_tab_that_does_not_exist_does_nothing() {
        let (mut tabs, _) = Tabs::new();
        tabs.new_tab();
        tabs.activate(0);
        // Alt+9 with two tabs.
        tabs.activate(8);
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn moving_tabs_keeps_the_same_tab_active() {
        let (mut tabs, [a, b, c, d]) = four_tabs();
        tabs.move_tab(0, 3);
        assert_eq!(order(&tabs), [b, c, d, a]);
        assert_eq!(tabs.active().id, c);
        tabs.move_tab(3, 0);
        assert_eq!(order(&tabs), [a, b, c, d]);
        assert_eq!(tabs.active().id, c);
        tabs.move_tab(2, 0);
        assert_eq!(order(&tabs), [c, a, b, d]);
        assert_eq!(tabs.active().id, c);
    }

    #[test]
    fn rename_trims_and_a_blank_name_resets_to_none() {
        let (mut tabs, _) = Tabs::new();
        tabs.rename(0, Some("  logs \t".into()));
        assert_eq!(tabs.active().name.as_deref(), Some("logs"));
        tabs.rename(0, Some(" \t ".into()));
        assert_eq!(tabs.active().name, None);
    }

    #[test]
    fn focusing_a_pane_activates_its_tab() {
        let (mut tabs, first) = Tabs::new();
        tabs.split(Axis::Row);
        tabs.new_tab();
        assert!(tabs.focus(first));
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.active().focused, first);
        assert!(!tabs.focus(LeafId::MAX), "no tab holds that pane");
        assert_eq!(tabs.active_index(), 0);
    }

    #[test]
    fn directional_focus_moves_to_the_neighbor_but_not_while_zoomed() {
        let (mut tabs, left) = Tabs::new();
        let right = tabs.split(Axis::Row);
        assert_eq!(tabs.focus_direction(Direction::Left), Some(left));
        assert_eq!(tabs.active().focused, left);
        assert_eq!(tabs.focus_direction(Direction::Left), None, "at the edge");
        tabs.toggle_zoom();
        assert_eq!(tabs.focus_direction(Direction::Right), None, "zoomed");
        assert_eq!(tabs.active().focused, left);
        tabs.toggle_zoom();
        assert_eq!(tabs.focus_direction(Direction::Right), Some(right));
    }

    #[test]
    fn zoom_toggles_the_focused_pane_once_there_are_two() {
        let (mut tabs, _) = Tabs::new();
        tabs.toggle_zoom();
        assert_eq!(tabs.active().zoomed, None, "one pane has nothing to zoom");
        let right = tabs.split(Axis::Row);
        let layout = tabs.active().root.clone();
        tabs.toggle_zoom();
        assert_eq!(tabs.active().zoomed, Some(right));
        tabs.toggle_zoom();
        assert_eq!(tabs.active().zoomed, None);
        assert_eq!(tabs.active().root, layout, "the layout comes back");
    }

    /// For `can_split`: with 100×40 px panes and a 4 px divider, a split
    /// needs 204 px of width side by side or 84 px of height stacked.
    const PANE_MIN: (f32, f32) = (100.0, 40.0);
    const DIVIDER: f32 = 4.0;

    #[test]
    fn a_split_needs_room_for_two_minimum_panes_and_the_divider() {
        let (tabs, _) = Tabs::new();
        assert!(tabs.can_split(Axis::Row, (204.0, 1000.0), PANE_MIN, DIVIDER));
        assert!(!tabs.can_split(Axis::Row, (203.0, 1000.0), PANE_MIN, DIVIDER));
        assert!(tabs.can_split(Axis::Column, (1000.0, 84.0), PANE_MIN, DIVIDER));
        assert!(!tabs.can_split(Axis::Column, (1000.0, 83.0), PANE_MIN, DIVIDER));
    }

    #[test]
    fn a_wide_short_pane_can_split_right_but_not_down() {
        let (tabs, _) = Tabs::new();
        assert!(tabs.can_split(Axis::Row, (800.0, 60.0), PANE_MIN, DIVIDER));
        assert!(!tabs.can_split(Axis::Column, (800.0, 60.0), PANE_MIN, DIVIDER));
        // And a tall, narrow one the other way round.
        assert!(!tabs.can_split(Axis::Row, (150.0, 600.0), PANE_MIN, DIVIDER));
        assert!(tabs.can_split(Axis::Column, (150.0, 600.0), PANE_MIN, DIVIDER));
    }

    #[test]
    fn can_split_measures_the_focused_pane_not_the_whole_tab() {
        let (mut tabs, left) = Tabs::new();
        tabs.split(Axis::Row);
        // The focused right half: 204 px of a 408 px tab, 203 of 406.
        assert!(tabs.can_split(Axis::Row, (408.0, 600.0), PANE_MIN, DIVIDER));
        assert!(!tabs.can_split(Axis::Row, (406.0, 600.0), PANE_MIN, DIVIDER));
        tabs.split(Axis::Column);
        // The focused bottom-right quarter: 84 px of a 168 px tab, 83 of 166.
        assert!(tabs.can_split(Axis::Column, (408.0, 168.0), PANE_MIN, DIVIDER));
        assert!(!tabs.can_split(Axis::Column, (408.0, 166.0), PANE_MIN, DIVIDER));
        // In the same tab, the left pane spans the full height.
        tabs.focus(left);
        assert!(tabs.can_split(Axis::Column, (408.0, 166.0), PANE_MIN, DIVIDER));
    }

    /// AC1 (spec 0007): a tab dropped on the right side of the shown tab's
    /// pane joins it there, leaving one tab. Both panes keep their IDs, so
    /// neither program restarts or reconnects.
    #[test]
    fn a_tab_dropped_on_the_right_side_of_a_pane_joins_it_there() {
        // The model knows no profiles: the first tab stands for the local
        // shell, the second, shown, for the SSH session.
        let (mut tabs, local) = Tabs::new();
        let ssh = tabs.new_tab();
        let dragged = Dragged::Tab(tabs.tabs()[0].id);
        assert!(tabs.drop_in_split(dragged, ssh, Direction::Right));
        assert_eq!(tabs.tabs().len(), 1);
        let expected = split(Axis::Row, 0.5, Node::Leaf(ssh), Node::Leaf(local));
        assert_eq!(tabs.active().root, expected);
    }

    /// AC2 (spec 0007): a dropped tab of three panes brings them in its
    /// layout, ratios included, and is gone from the tab bar.
    #[test]
    fn a_dropped_tab_brings_its_whole_layout_and_leaves_the_tab_bar() {
        let (mut tabs, left) = Tabs::new();
        let right = tabs.split(Axis::Row);
        let target = tabs.active().id;
        let a = tabs.new_tab();
        let b = tabs.split(Axis::Row);
        let c = tabs.split(Axis::Column);
        // Its dividers moved off the middle.
        let layout = split(
            Axis::Row,
            0.3,
            Node::Leaf(a),
            split(Axis::Column, 0.7, Node::Leaf(b), Node::Leaf(c)),
        );
        tabs.active_mut().root = layout.clone();
        let dragged = Dragged::Tab(tabs.active().id);
        tabs.activate(0);
        assert!(tabs.drop_in_split(dragged, right, Direction::Down));
        assert_eq!(order(&tabs), [target]);
        let expected = split(
            Axis::Row,
            0.5,
            Node::Leaf(left),
            split(Axis::Column, 0.5, Node::Leaf(right), layout),
        );
        assert_eq!(tabs.active().root, expected);
    }

    /// Spec 0007, flow 3: the dropped tab's focused pane takes the focus,
    /// and the target tab's zoom ends.
    #[test]
    fn a_dropped_tabs_focused_pane_takes_the_focus_and_the_zoom_ends() {
        let (mut tabs, _) = Tabs::new();
        let zoomed = tabs.split(Axis::Row);
        tabs.toggle_zoom();
        // Focused on its first pane, not its newest.
        let first = tabs.new_tab();
        tabs.split(Axis::Row);
        tabs.focus(first);
        let dragged = Dragged::Tab(tabs.active().id);
        tabs.activate(0);
        assert!(tabs.drop_in_split(dragged, zoomed, Direction::Left));
        let tab = tabs.active();
        assert_eq!((tab.focused, tab.zoomed), (first, None));
    }

    /// AC4 (spec 0007), flow 4: a pane dropped on the top side of another
    /// pane of its tab leaves its place to its sibling, ends up above the
    /// other one, and keeps the focus.
    #[test]
    fn a_pane_dropped_on_the_top_side_of_another_ends_up_above_it() {
        let (mut tabs, left) = Tabs::new();
        let top = tabs.split(Axis::Row);
        let bottom = tabs.split(Axis::Column);
        // left | (top over bottom), with `bottom` focused.
        assert!(tabs.drop_in_split(Dragged::Pane(bottom), left, Direction::Up));
        let expected = split(
            Axis::Row,
            0.5,
            split(Axis::Column, 0.5, Node::Leaf(bottom), Node::Leaf(left)),
            Node::Leaf(top),
        );
        assert_eq!(tabs.active().root, expected);
        assert_eq!(tabs.active().focused, bottom);
    }

    /// AC4 (spec 0007): dropping a pane on itself, or a tab on one of its
    /// own panes, changes nothing, and no side of them is a target.
    #[test]
    fn dropping_a_pane_on_itself_or_a_tab_on_its_own_panes_changes_nothing() {
        let (mut tabs, left) = Tabs::new();
        let right = tabs.split(Axis::Row);
        let own = Dragged::Tab(tabs.active().id);
        let before = tabs.tabs().to_vec();
        let sides = [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ];
        for (dragged, target) in [(Dragged::Pane(left), left), (own, left), (own, right)] {
            for side in sides {
                let drop = format!("{dragged:?} on the {side:?} side of {target}");
                let area = (2000.0, 2000.0);
                assert!(
                    !tabs.can_drop(dragged, target, side, area, PANE_MIN, DIVIDER),
                    "{drop}"
                );
                assert!(!tabs.drop_in_split(dragged, target, side), "{drop}");
            }
        }
        assert_eq!(tabs.tabs(), before);
    }

    /// AC5 (spec 0007), flow 4: a pane moved within its tab has left its
    /// place when it's inserted, so the space it leaves counts as room.
    #[test]
    fn room_for_a_pane_moved_within_its_tab_counts_the_space_it_leaves() {
        let (mut tabs, left) = Tabs::new();
        let right = tabs.split(Axis::Row);
        tabs.new_tab();
        let other = Dragged::Tab(tabs.active().id);
        tabs.activate(0);
        // 150 px each of a 300 px tab: no room beside `left` for a pane
        // from elsewhere, but `right` leaving gives it the whole width.
        let area = (300.0, 600.0);
        let side = Direction::Left;
        assert!(!tabs.can_drop(other, left, side, area, PANE_MIN, DIVIDER));
        let moved = Dragged::Pane(right);
        assert!(tabs.can_drop(moved, left, side, area, PANE_MIN, DIVIDER));
    }

    /// AC3 (spec 0007), flow 5: a pane dragged to the tab bar becomes a tab
    /// where the marker shows, or at the end past the last tab, and that tab
    /// activates with the pane focused.
    #[test]
    fn a_pane_dragged_to_the_tab_bar_becomes_a_tab_at_the_marked_place() {
        let (mut tabs, first) = Tabs::new();
        let second = tabs.split(Axis::Row);
        tabs.split(Axis::Row);
        tabs.new_tab();
        let [a, b]: [TabId; 2] = order(&tabs).try_into().expect("two tabs");
        tabs.activate(0);
        assert!(tabs.pane_to_tab(second, 1));
        let moved = tabs.active();
        assert_eq!((&moved.root, moved.focused), (&Node::Leaf(second), second));
        assert_eq!(order(&tabs), [a, moved.id, b]);
        tabs.activate(0);
        assert!(tabs.pane_to_tab(first, usize::MAX));
        assert_eq!(tabs.active_index(), tabs.tabs().len() - 1);
        assert_eq!(tabs.active().root, Node::Leaf(first));
    }

    /// AC3 (spec 0007): the panes a pane dragged to the tab bar leaves
    /// behind fill their tab: its sibling takes its place and, as on close,
    /// its focus.
    #[test]
    fn a_pane_dragged_out_leaves_no_dead_space_in_its_old_tab() {
        let (mut tabs, left) = Tabs::new();
        let top = tabs.split(Axis::Row);
        let bottom = tabs.split(Axis::Column);
        // left | (top over bottom), `top` dragged out.
        tabs.focus(top);
        let old = tabs.active().id;
        assert!(tabs.pane_to_tab(top, usize::MAX));
        let old = &tabs.tabs()[tabs.index_of(old).expect("the old tab stays")];
        let expected = split(Axis::Row, 0.5, Node::Leaf(left), Node::Leaf(bottom));
        assert_eq!(old.root, expected);
        assert_eq!(old.focused, bottom);
    }

    /// Spec 0007, flow 1: a tab's only pane has no handle, the tab itself
    /// being dragged instead, so it doesn't become a tab of its own.
    #[test]
    fn a_tabs_only_pane_doesnt_become_a_tab_of_its_own() {
        let (mut tabs, only) = Tabs::new();
        tabs.new_tab();
        tabs.activate(0);
        let before = tabs.tabs().to_vec();
        assert!(!tabs.pane_to_tab(only, 2));
        assert_eq!(tabs.tabs(), before);
    }

    /// The tabs of a saved session as boot hands them over, every tab ID
    /// left at 0: pane 7, then panes 3 over 12, named, zoomed on 12.
    fn saved_tabs() -> Vec<Tab> {
        vec![
            Tab {
                id: 0,
                name: None,
                root: Node::Leaf(7),
                focused: 7,
                zoomed: None,
            },
            Tab {
                id: 0,
                name: Some("logs".into()),
                root: split(Axis::Column, 0.7, Node::Leaf(3), Node::Leaf(12)),
                focused: 12,
                zoomed: Some(12),
            },
        ]
    }

    /// AC3 (spec 0003), flow 6: restored tabs come back with their layout,
    /// names, focus and zoom, their panes keeping their IDs (§Model: leaf
    /// IDs are stable across restarts), and the saved tab is shown.
    #[test]
    fn restored_tabs_keep_their_layout_and_pane_ids_and_show_the_saved_tab() {
        let saved = saved_tabs();
        let tabs = Tabs::restore(saved.clone(), 1).expect("tabs to restore");
        assert_eq!(tabs.active_index(), 1);
        let kept = |tab: &Tab| (tab.name.clone(), tab.root.clone(), tab.focused, tab.zoomed);
        let restored: Vec<_> = tabs.tabs().iter().map(kept).collect();
        let expected: Vec<_> = saved.iter().map(kept).collect();
        assert_eq!(restored, expected);
    }

    /// Spec 0003 §Model: after a restore, every tab has an ID of its own, and
    /// new tabs and panes get IDs no restored one has, so no two are ever
    /// taken for each other.
    #[test]
    fn after_a_restore_new_tabs_and_panes_get_ids_nothing_restored_has() {
        let mut tabs = Tabs::restore(saved_tabs(), 0).expect("tabs to restore");
        let mut pane_ids = vec![7, 3, 12];
        pane_ids.push(tabs.new_tab());
        pane_ids.push(tabs.split(Axis::Row));
        let tab_ids = order(&tabs);
        let distinct = |ids: &[u64]| ids.iter().collect::<BTreeSet<_>>().len() == ids.len();
        assert!(distinct(&tab_ids), "tab IDs: {tab_ids:?}");
        assert!(distinct(&pane_ids), "pane IDs: {pane_ids:?}");
    }

    /// Spec 0003, flow 7: a window always has a tab, so there's nothing to
    /// restore without one (boot opens a fresh tab instead), and a shown tab
    /// past the end shows one that is there.
    #[test]
    fn a_restore_needs_a_tab_and_shows_one_that_is_there() {
        assert!(Tabs::restore(Vec::new(), 0).is_none());
        let tabs = Tabs::restore(saved_tabs(), 5).expect("tabs to restore");
        assert!(tabs.active_index() < tabs.tabs().len());
    }
}
