//! One tab's split tree (spec 0003): a binary tree whose leaves are panes.
//! Pure data, so splitting, closing, focus moves and divider limits are
//! testable without a window.

/// A pane's place in a tree. Unique per app run; session restore (T-006)
/// persists it.
pub type LeafId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side, divided by a vertical line.
    Row,
    /// Stacked, divided by a horizontal line.
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(LeafId),
    Split {
        axis: Axis,
        /// The first child's share of the space, from 0 to 1.
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// An area in the unit square, 0 to 1 on both axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const UNIT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    fn right(&self) -> f32 {
        self.x + self.width
    }

    fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

impl Node {
    /// All panes, in reading order (first child before second).
    pub fn leaves(&self) -> Vec<LeafId> {
        let mut leaves = Vec::new();
        self.collect_leaves(&mut leaves);
        leaves
    }

    fn collect_leaves(&self, leaves: &mut Vec<LeafId>) {
        match self {
            Node::Leaf(id) => leaves.push(*id),
            Node::Split { first, second, .. } => {
                first.collect_leaves(leaves);
                second.collect_leaves(leaves);
            }
        }
    }

    pub fn contains(&self, leaf: LeafId) -> bool {
        match self {
            Node::Leaf(id) => *id == leaf,
            Node::Split { first, second, .. } => first.contains(leaf) || second.contains(leaf),
        }
    }

    pub fn first_leaf(&self) -> LeafId {
        match self {
            Node::Leaf(id) => *id,
            Node::Split { first, .. } => first.first_leaf(),
        }
    }

    /// Splits `target` along `axis`: `target` keeps the first half and
    /// `new` gets the second (right or below). False if `target` isn't here.
    pub fn split(&mut self, target: LeafId, new: LeafId, axis: Axis) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(Node::Leaf(target)),
                    second: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split(target, new, axis) || second.split(target, new, axis)
            }
        }
    }

    /// The pane that takes `leaf`'s place when it closes: the first pane of
    /// its sibling. `None` if `leaf` is the whole tree or isn't in it.
    pub fn successor(&self, leaf: LeafId) -> Option<LeafId> {
        let Node::Split { first, second, .. } = self else {
            return None;
        };
        match (first.as_ref(), second.as_ref()) {
            (Node::Leaf(id), sibling) | (sibling, Node::Leaf(id)) if *id == leaf => {
                Some(sibling.first_leaf())
            }
            _ => first.successor(leaf).or_else(|| second.successor(leaf)),
        }
    }

    /// Removes `leaf`; its sibling takes the parent split's place. `None`
    /// when the tree was only `leaf`.
    pub fn remove(self, leaf: LeafId) -> Option<Node> {
        match self {
            Node::Leaf(id) if id == leaf => None,
            Node::Leaf(id) => Some(Node::Leaf(id)),
            Node::Split {
                axis,
                ratio,
                first,
                second,
            } => match ((*first).remove(leaf), (*second).remove(leaf)) {
                (Some(first), Some(second)) => Some(Node::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// Each pane's area when the tree fills `area`.
    pub fn rects(&self, area: Rect) -> Vec<(LeafId, Rect)> {
        match self {
            Node::Leaf(id) => vec![(*id, area)],
            Node::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (a, b) = match axis {
                    Axis::Row => {
                        let width = area.width * ratio;
                        (
                            Rect { width, ..area },
                            Rect {
                                x: area.x + width,
                                width: area.width - width,
                                ..area
                            },
                        )
                    }
                    Axis::Column => {
                        let height = area.height * ratio;
                        (
                            Rect { height, ..area },
                            Rect {
                                y: area.y + height,
                                height: area.height - height,
                                ..area
                            },
                        )
                    }
                };
                let mut rects = first.rects(a);
                rects.extend(second.rects(b));
                rects
            }
        }
    }

    /// The pane next to `from` in `direction`: beyond `from`'s edge and
    /// overlapping it on the other axis. The smallest gap wins, then the
    /// largest overlap.
    pub fn neighbor(&self, from: LeafId, direction: Direction) -> Option<LeafId> {
        const EPSILON: f32 = 1e-4;
        let rects = self.rects(Rect::UNIT);
        let (_, origin) = *rects.iter().find(|(id, _)| *id == from)?;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
        rects
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|&(id, r)| {
                let (gap, shared) = match direction {
                    Direction::Left => (
                        origin.x - r.right(),
                        overlap(origin.y, origin.bottom(), r.y, r.bottom()),
                    ),
                    Direction::Right => (
                        r.x - origin.right(),
                        overlap(origin.y, origin.bottom(), r.y, r.bottom()),
                    ),
                    Direction::Up => (
                        origin.y - r.bottom(),
                        overlap(origin.x, origin.right(), r.x, r.right()),
                    ),
                    Direction::Down => (
                        r.y - origin.bottom(),
                        overlap(origin.x, origin.right(), r.x, r.right()),
                    ),
                };
                (gap >= -EPSILON && shared > EPSILON).then_some((id, gap, shared))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, ..)| id)
    }

    /// The split at `path` (`false`: first child, `true`: second), for
    /// moving its divider. `None` if the path doesn't lead to a split.
    pub fn split_at_mut(&mut self, path: &[bool]) -> Option<&mut Node> {
        let Some((&second_child, rest)) = path.split_first() else {
            return match self {
                Node::Split { .. } => Some(self),
                Node::Leaf(_) => None,
            };
        };
        match self {
            Node::Split { first, second, .. } => {
                let child = if second_child { second } else { first };
                child.split_at_mut(rest)
            }
            Node::Leaf(_) => None,
        }
    }

    /// The smallest (width, height) in pixels this subtree fits in, if every
    /// pane needs `pane` and every divider takes `divider`.
    pub fn min_size(&self, pane: (f32, f32), divider: f32) -> (f32, f32) {
        match self {
            Node::Leaf(_) => pane,
            Node::Split {
                axis,
                first,
                second,
                ..
            } => {
                let (a, b) = (
                    first.min_size(pane, divider),
                    second.min_size(pane, divider),
                );
                match axis {
                    Axis::Row => (a.0 + b.0 + divider, a.1.max(b.1)),
                    Axis::Column => (a.0.max(b.0), a.1 + b.1 + divider),
                }
            }
        }
    }
}

/// Clamps a divider so both sides keep their minimum size, when the split
/// spans `length` pixels including the `divider` itself. If the two
/// minimums don't fit, the space is shared in their proportion.
pub fn clamp_ratio(ratio: f32, length: f32, first_min: f32, second_min: f32, divider: f32) -> f32 {
    let usable = length - divider;
    if usable <= 0.0 || first_min + second_min >= usable {
        let total = first_min + second_min;
        return if total > 0.0 { first_min / total } else { 0.5 };
    }
    ratio.clamp(first_min / usable, 1.0 - second_min / usable)
}

#[cfg(test)]
mod tests {
    use super::Axis::{Column, Row};
    use super::Direction::{Down, Left, Right, Up};
    use super::Node::Leaf;
    use super::*;

    /// Slack for geometry computed from ratios.
    const EPSILON: f32 = 1e-5;

    /// A split, for building trees by hand.
    fn split(axis: Axis, ratio: f32, first: Node, second: Node) -> Node {
        Node::Split {
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// Where `leaf` is drawn when `root` fills the unit square.
    fn rect_of(root: &Node, leaf: LeafId) -> Rect {
        let rects = root.rects(Rect::UNIT);
        let found = rects.into_iter().find(|(id, _)| *id == leaf);
        found.expect("the pane is drawn").1
    }

    /// Fails unless `root`'s panes tile the unit square: every pane drawn
    /// once and inside it, no two overlapping, and nothing left uncovered.
    fn assert_tiles(root: &Node) {
        let rects = root.rects(Rect::UNIT);
        let mut drawn: Vec<_> = rects.iter().map(|(id, _)| *id).collect();
        let mut leaves = root.leaves();
        drawn.sort_unstable();
        leaves.sort_unstable();
        assert_eq!(drawn, leaves, "every pane is drawn once");
        for (id, r) in &rects {
            let inside = r.x >= 0.0
                && r.y >= 0.0
                && r.right() <= 1.0 + EPSILON
                && r.bottom() <= 1.0 + EPSILON;
            assert!(
                inside && r.width > 0.0 && r.height > 0.0,
                "pane {id}: {r:?}"
            );
        }
        for (ix, (a, ra)) in rects.iter().enumerate() {
            for (b, rb) in &rects[ix + 1..] {
                let width = ra.right().min(rb.right()) - ra.x.max(rb.x);
                let height = ra.bottom().min(rb.bottom()) - ra.y.max(rb.y);
                assert!(width <= EPSILON || height <= EPSILON, "{a} overlaps {b}");
            }
        }
        // Inside the square and disjoint, so they cover it iff the areas add up.
        let covered: f32 = rects.iter().map(|(_, r)| r.width * r.height).sum();
        assert!((covered - 1.0).abs() < EPSILON, "panes cover {covered}");
    }

    /// ```text
    /// 1 | 2
    /// --+--
    /// 3 | 4
    /// ```
    fn two_by_two() -> Node {
        split(
            Column,
            0.5,
            split(Row, 0.5, Leaf(1), Leaf(2)),
            split(Row, 0.5, Leaf(3), Leaf(4)),
        )
    }

    #[test]
    fn splitting_puts_the_new_pane_right_or_below_at_half_the_space() {
        let mut row = Leaf(1);
        assert!(row.split(1, 2, Row));
        assert_eq!(row, split(Row, 0.5, Leaf(1), Leaf(2)));
        assert_eq!(rect_of(&row, 2), rect(0.5, 0.0, 0.5, 1.0));

        let mut column = Leaf(1);
        assert!(column.split(1, 2, Column));
        assert_eq!(column, split(Column, 0.5, Leaf(1), Leaf(2)));
        assert_eq!(rect_of(&column, 2), rect(0.0, 0.5, 1.0, 0.5));
    }

    #[test]
    fn splitting_a_nested_pane_splits_only_that_pane() {
        let mut root = split(Row, 0.3, Leaf(1), Leaf(2));
        assert!(root.split(2, 3, Column));
        let expected = split(Row, 0.3, Leaf(1), split(Column, 0.5, Leaf(2), Leaf(3)));
        assert_eq!(root, expected);
        assert!(!root.split(9, 4, Row), "pane 9 isn't in the tree");
        assert_eq!(root, expected);
    }

    #[test]
    fn removing_a_pane_lets_its_sibling_take_the_parents_place() {
        let pair = split(Row, 0.5, Leaf(1), Leaf(2));
        assert_eq!(pair.clone().remove(1), Some(Leaf(2)));
        assert_eq!(pair.remove(2), Some(Leaf(1)));

        // 1 | (2 over (3 | 4)). A sibling subtree moves up whole, and the
        // splits above it keep their axis and ratio.
        let stack = split(Column, 0.6, Leaf(2), split(Row, 0.5, Leaf(3), Leaf(4)));
        let root = split(Row, 0.3, Leaf(1), stack.clone());
        assert_eq!(root.clone().remove(1), Some(stack));
        let without_2 = split(Row, 0.3, Leaf(1), split(Row, 0.5, Leaf(3), Leaf(4)));
        assert_eq!(root.clone().remove(2), Some(without_2));
        let without_4 = split(Row, 0.3, Leaf(1), split(Column, 0.6, Leaf(2), Leaf(3)));
        assert_eq!(root.remove(4), Some(without_4));
    }

    #[test]
    fn removing_the_only_pane_leaves_no_tree() {
        assert_eq!(Leaf(1).remove(1), None);
    }

    #[test]
    fn the_successor_is_the_first_pane_of_the_closed_panes_sibling() {
        // 1 | (2 over (3 | 4))
        let root = split(
            Row,
            0.5,
            Leaf(1),
            split(Column, 0.5, Leaf(2), split(Row, 0.5, Leaf(3), Leaf(4))),
        );
        assert_eq!(root.successor(1), Some(2));
        assert_eq!(root.successor(2), Some(3));
        assert_eq!(root.successor(3), Some(4));
        assert_eq!(root.successor(4), Some(3));
        // It names a pane that is still there once the closed one is gone.
        for leaf in root.leaves() {
            let successor = root.successor(leaf).expect("every pane has a sibling");
            let rest = root.clone().remove(leaf).expect("panes are left");
            assert!(successor != leaf && rest.contains(successor), "{leaf}");
        }
        assert_eq!(Leaf(1).successor(1), None, "the only pane");
        assert_eq!(root.successor(9), None, "not in the tree");
    }

    #[test]
    fn panes_tile_the_tab_whatever_the_splits_and_closes() {
        assert_tiles(&Leaf(1));
        // Uneven ratios, nested on both sides.
        let skewed = split(
            Row,
            0.3,
            split(Column, 0.8, Leaf(1), Leaf(2)),
            split(
                Column,
                0.35,
                Leaf(3),
                split(Row, 0.6, Leaf(4), split(Column, 0.1, Leaf(5), Leaf(6))),
            ),
        );
        assert_tiles(&skewed);
        for leaf in skewed.leaves() {
            assert_tiles(&skewed.clone().remove(leaf).expect("panes are left"));
        }
        // Grown one split at a time, splitting a different pane each time.
        let mut grown = Leaf(1);
        for (step, new) in (2..=12).enumerate() {
            let leaves = grown.leaves();
            let axis = if step % 2 == 0 { Row } else { Column };
            assert!(grown.split(leaves[step * 7 % leaves.len()], new, axis));
            assert_tiles(&grown);
        }
    }

    /// AC1: split right, split down, then close the middle pane: the
    /// remaining panes fill the tab with no dead space.
    #[test]
    fn closing_the_middle_pane_leaves_no_dead_space() {
        let mut root = Leaf(1);
        root.split(1, 2, Row);
        root.split(2, 3, Column);
        // 1 | 2 over 3
        let root = root.remove(2).expect("two panes are left");
        assert_tiles(&root);
        assert_eq!(rect_of(&root, 1), rect(0.0, 0.0, 0.5, 1.0));
        assert_eq!(rect_of(&root, 3), rect(0.5, 0.0, 0.5, 1.0));
    }

    #[test]
    fn the_neighbor_is_the_adjacent_pane_in_each_direction() {
        let grid = two_by_two();
        assert_eq!(grid.neighbor(1, Right), Some(2));
        assert_eq!(grid.neighbor(1, Down), Some(3));
        assert_eq!(grid.neighbor(4, Left), Some(3));
        assert_eq!(grid.neighbor(4, Up), Some(2));
        assert_eq!(grid.neighbor(2, Down), Some(4));
        assert_eq!(grid.neighbor(3, Up), Some(1));
    }

    #[test]
    fn there_is_no_neighbor_past_the_edge() {
        let grid = two_by_two();
        let edges = [
            (1, Left),
            (1, Up),
            (2, Right),
            (2, Up),
            (3, Left),
            (3, Down),
            (4, Right),
            (4, Down),
        ];
        for (leaf, direction) in edges {
            assert_eq!(grid.neighbor(leaf, direction), None, "{leaf} {direction:?}");
        }
        for direction in [Left, Right, Up, Down] {
            assert_eq!(Leaf(1).neighbor(1, direction), None);
        }
    }

    /// ```text
    /// +-------+---+---+
    /// |       | 2 |   |
    /// |       +---+   |
    /// |   1   |   | 4 |
    /// |       | 3 |   |
    /// +-------+---+---+
    /// ```
    ///
    /// 2 and 3 touch 1 and 4; 3 shares more of their edge. 4 lies to the
    /// right of 1 too and overlaps it the most, but further away.
    #[test]
    fn the_neighbor_is_the_closest_pane_then_the_most_overlapping() {
        let root = split(
            Row,
            0.5,
            Leaf(1),
            split(Row, 0.5, split(Column, 0.25, Leaf(2), Leaf(3)), Leaf(4)),
        );
        assert_eq!(root.neighbor(1, Right), Some(3));
        assert_eq!(root.neighbor(4, Left), Some(3));
        assert_eq!(root.neighbor(2, Left), Some(1));
        assert_eq!(root.neighbor(2, Right), Some(4));
    }

    #[test]
    fn split_at_mut_reaches_the_split_at_a_path() {
        // 1 | (2 over 3)
        let mut root = split(Row, 0.5, Leaf(1), split(Column, 0.5, Leaf(2), Leaf(3)));
        let Some(Node::Split { ratio, .. }) = root.split_at_mut(&[true]) else {
            panic!("no split at [second]");
        };
        *ratio = 0.25;
        let Some(Node::Split { ratio, .. }) = root.split_at_mut(&[]) else {
            panic!("no split at the root");
        };
        *ratio = 0.75;
        let moved = split(Row, 0.75, Leaf(1), split(Column, 0.25, Leaf(2), Leaf(3)));
        assert_eq!(root, moved);
    }

    #[test]
    fn split_at_mut_is_none_for_a_pane_or_a_path_past_one() {
        let mut root = split(Row, 0.5, Leaf(1), split(Column, 0.5, Leaf(2), Leaf(3)));
        let leaf_paths: [&[bool]; 4] = [&[false], &[true, false], &[true, true], &[false, true]];
        for path in leaf_paths {
            assert!(root.split_at_mut(path).is_none(), "{path:?}");
        }
        assert!(Leaf(1).split_at_mut(&[]).is_none());
    }

    #[test]
    fn min_size_adds_up_along_a_split_and_takes_the_largest_across_it() {
        let (pane, divider) = ((80.0, 50.0), 4.0);
        assert_eq!(Leaf(1).min_size(pane, divider), pane);
        // 1 | (2 over 3 over 4): two widths side by side, three heights
        // stacked, a divider between each.
        let wide = split(
            Row,
            0.5,
            Leaf(1),
            split(Column, 0.5, Leaf(2), split(Column, 0.5, Leaf(3), Leaf(4))),
        );
        assert_eq!(
            wide.min_size(pane, divider),
            (2.0 * 80.0 + 4.0, 3.0 * 50.0 + 2.0 * 4.0)
        );
        // The same tree with the axes swapped.
        let tall = split(
            Column,
            0.5,
            Leaf(1),
            split(Row, 0.5, Leaf(2), split(Row, 0.5, Leaf(3), Leaf(4))),
        );
        assert_eq!(
            tall.min_size(pane, divider),
            (3.0 * 80.0 + 2.0 * 4.0, 2.0 * 50.0 + 4.0)
        );
    }

    // In the clamp tests, 404 px with a 4 px divider leaves 400 px to share.

    #[test]
    fn clamp_ratio_passes_a_ratio_that_leaves_both_sides_their_minimum() {
        assert_eq!(clamp_ratio(0.5, 404.0, 100.0, 100.0, 4.0), 0.5);
        assert_eq!(clamp_ratio(0.25, 404.0, 100.0, 100.0, 4.0), 0.25);
        assert_eq!(clamp_ratio(0.75, 404.0, 100.0, 100.0, 4.0), 0.75);
    }

    #[test]
    fn clamp_ratio_keeps_both_sides_at_their_minimum() {
        // The first side keeps 100 of the 400 px, the second 50.
        assert_eq!(clamp_ratio(0.1, 404.0, 100.0, 50.0, 4.0), 0.25);
        assert_eq!(clamp_ratio(0.95, 404.0, 100.0, 50.0, 4.0), 0.875);
        assert_eq!(clamp_ratio(-1.0, 404.0, 100.0, 50.0, 4.0), 0.25);
        assert_eq!(clamp_ratio(2.0, 404.0, 100.0, 50.0, 4.0), 0.875);
    }

    #[test]
    fn clamp_ratio_shares_by_the_minimums_when_they_dont_fit() {
        // 100 px for minimums of 150 and 50: shared 3:1, whatever was asked.
        for wanted in [0.1, 0.5, 0.9] {
            assert_eq!(clamp_ratio(wanted, 104.0, 150.0, 50.0, 4.0), 0.75);
        }
        // Nothing left past the divider.
        assert_eq!(clamp_ratio(0.5, 3.0, 150.0, 50.0, 4.0), 0.75);
    }
}
