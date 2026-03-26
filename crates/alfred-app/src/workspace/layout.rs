use super::pane::PaneId;

/// Physical-pixel rectangle — all coordinates in physical (not logical) pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhysRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Direction of a pane split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitDir {
    /// Left | Right — a vertical divider.
    Vertical,
    /// Top / Bottom — a horizontal divider.
    Horizontal,
}

/// Binary split tree. Each leaf holds a pane; each interior node splits a rect.
pub enum PaneTree {
    Leaf(PaneId),
    Split {
        dir: SplitDir,
        /// Fraction [0,1] where the divider sits.
        ratio: f32,
        left: Box<PaneTree>,
        right: Box<PaneTree>,
    },
}

impl PaneTree {
    pub fn new_leaf(id: PaneId) -> Self {
        PaneTree::Leaf(id)
    }

    // ── Layout ─────────────────────────────────────────────────────────────

    /// Walk the tree and return `(pane_id, rect)` for every leaf, in
    /// left-to-right / top-to-bottom order.
    pub fn layout(&self, rect: PhysRect) -> Vec<(PaneId, PhysRect)> {
        let mut out = Vec::new();
        self.layout_inner(rect, &mut out);
        out
    }

    fn layout_inner(&self, rect: PhysRect, out: &mut Vec<(PaneId, PhysRect)>) {
        match self {
            PaneTree::Leaf(id) => out.push((*id, rect)),
            PaneTree::Split { dir, ratio, left, right } => {
                const DIVIDER: u32 = 2;
                let (l, r) = split_rect(rect, *dir, *ratio, DIVIDER);
                left.layout_inner(l, out);
                right.layout_inner(r, out);
            }
        }
    }

    // ── Mutation ────────────────────────────────────────────────────────────

    /// Insert `new_id` as the right/bottom sibling of the leaf `target_id`.
    /// Returns `true` if the leaf was found.
    pub fn split(&mut self, target_id: PaneId, dir: SplitDir, new_id: PaneId) -> bool {
        match self {
            PaneTree::Leaf(id) if *id == target_id => {
                // Replace this leaf with a Split node.
                let old = std::mem::replace(self, PaneTree::Leaf(0));
                *self = PaneTree::Split {
                    dir,
                    ratio: 0.5,
                    left: Box::new(old),
                    right: Box::new(PaneTree::Leaf(new_id)),
                };
                true
            }
            PaneTree::Leaf(_) => false,
            PaneTree::Split { left, right, .. } => {
                left.split(target_id, dir, new_id)
                    || right.split(target_id, dir, new_id)
            }
        }
    }

    /// Remove the leaf `target_id` from the tree. When a Split has one child
    /// removed, the Split node is replaced by the surviving child.
    ///
    /// **Precondition:** the tree has more than one leaf. Removing the last
    /// leaf is a no-op (returns immediately).
    pub fn remove_leaf(&mut self, target_id: PaneId) {
        // Take ownership of self by replacing with a throwaway placeholder.
        let placeholder = PaneTree::Leaf(PaneId::MAX);
        let old = std::mem::replace(self, placeholder);

        *self = match old {
            // A bare Leaf reached during recursion is always a non-target
            // (the parent's left_is_target/right_is_target handles the target
            // case and replaces the whole Split). Restore it unchanged.
            PaneTree::Leaf(id) => PaneTree::Leaf(id),
            PaneTree::Split { dir, ratio, left, right } => {
                let left_is_target = matches!(*left, PaneTree::Leaf(id) if id == target_id);
                let right_is_target = matches!(*right, PaneTree::Leaf(id) if id == target_id);

                if left_is_target {
                    *right // replace the Split with the surviving sibling
                } else if right_is_target {
                    *left
                } else {
                    // Target is deeper — recurse.
                    let mut left = left;
                    let mut right = right;
                    left.remove_leaf(target_id);
                    right.remove_leaf(target_id);
                    PaneTree::Split { dir, ratio, left, right }
                }
            }
        };
    }

    // ── Queries ─────────────────────────────────────────────────────────────

    /// All leaf pane IDs in left-to-right / top-to-bottom order.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.leaves_inner(&mut out);
        out
    }

    fn leaves_inner(&self, out: &mut Vec<PaneId>) {
        match self {
            PaneTree::Leaf(id) => out.push(*id),
            PaneTree::Split { left, right, .. } => {
                left.leaves_inner(out);
                right.leaves_inner(out);
            }
        }
    }
}

// ── Geometry helpers ────────────────────────────────────────────────────────

fn split_rect(rect: PhysRect, dir: SplitDir, ratio: f32, divider: u32) -> (PhysRect, PhysRect) {
    match dir {
        SplitDir::Vertical => {
            // Left | Right
            let left_w = ((rect.w as f32 * ratio) as u32).saturating_sub(divider / 2).max(1);
            let right_x = rect.x + left_w + divider;
            let right_w = rect.w.saturating_sub(left_w + divider).max(1);
            (
                PhysRect { x: rect.x,  y: rect.y, w: left_w,  h: rect.h },
                PhysRect { x: right_x, y: rect.y, w: right_w, h: rect.h },
            )
        }
        SplitDir::Horizontal => {
            // Top / Bottom
            let top_h = ((rect.h as f32 * ratio) as u32).saturating_sub(divider / 2).max(1);
            let bottom_y = rect.y + top_h + divider;
            let bottom_h = rect.h.saturating_sub(top_h + divider).max(1);
            (
                PhysRect { x: rect.x, y: rect.y,    w: rect.w, h: top_h    },
                PhysRect { x: rect.x, y: bottom_y,  w: rect.w, h: bottom_h },
            )
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u32, y: u32, w: u32, h: u32) -> PhysRect {
        PhysRect { x, y, w, h }
    }

    // ── layout ───────────────────────────────────────────────────────────────

    #[test]
    fn single_leaf_fills_rect() {
        let tree = PaneTree::new_leaf(0);
        let r = rect(0, 0, 1200, 800);
        let layout = tree.layout(r);
        assert_eq!(layout.len(), 1);
        let (id, got) = layout[0];
        assert_eq!(id, 0);
        assert_eq!(got.x, 0);
        assert_eq!(got.y, 0);
        assert_eq!(got.w, 1200);
        assert_eq!(got.h, 800);
    }

    #[test]
    fn vertical_split_sums_to_full_width() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        let r = rect(0, 0, 1000, 800);
        let layout = tree.layout(r);
        assert_eq!(layout.len(), 2);
        let (_, l) = layout[0];
        let (_, r2) = layout[1];
        // Both halves must be inside the original rect.
        assert_eq!(l.x, 0);
        assert_eq!(l.y, 0);
        assert_eq!(l.h, 800);
        assert_eq!(r2.y, 0);
        assert_eq!(r2.h, 800);
        // Right pane starts after left pane + 2px divider.
        assert_eq!(r2.x, l.x + l.w + 2);
        // Together they consume all pixels (minus divider).
        assert_eq!(l.w + 2 + r2.w, 1000);
    }

    #[test]
    fn horizontal_split_sums_to_full_height() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Horizontal, 1);
        let r = rect(0, 0, 800, 600);
        let layout = tree.layout(r);
        assert_eq!(layout.len(), 2);
        let (_, top) = layout[0];
        let (_, bot) = layout[1];
        assert_eq!(top.h + 2 + bot.h, 600);
        assert_eq!(bot.y, top.y + top.h + 2);
    }

    #[test]
    fn vertical_split_default_ratio_is_even() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        let r = rect(0, 0, 1000, 800);
        let layout = tree.layout(r);
        let (_, l) = layout[0];
        let (_, r2) = layout[1];
        // At ratio=0.5 and 1000px: left gets 499px, right gets 499px.
        // (1000 * 0.5 = 500 → 500 - divider/2 = 499, right = 1000 - 499 - 2 = 499)
        assert!((l.w as i32 - r2.w as i32).abs() <= 1, "halves should be nearly equal");
    }

    #[test]
    fn non_zero_origin_preserved() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        let r = rect(100, 50, 800, 600);
        let layout = tree.layout(r);
        let (_, l) = layout[0];
        let (_, r2) = layout[1];
        // Left pane starts at the rect origin.
        assert_eq!(l.x, 100);
        assert_eq!(l.y, 50);
        // Right pane y unchanged.
        assert_eq!(r2.y, 50);
    }

    // ── leaves ───────────────────────────────────────────────────────────────

    #[test]
    fn leaves_single() {
        let tree = PaneTree::new_leaf(7);
        assert_eq!(tree.leaves(), vec![7]);
    }

    #[test]
    fn leaves_after_two_splits() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        tree.split(1, SplitDir::Horizontal, 2);
        // Left-to-right, top-to-bottom: [0, 1, 2]
        let leaves = tree.leaves();
        assert_eq!(leaves.len(), 3);
        assert!(leaves.contains(&0));
        assert!(leaves.contains(&1));
        assert!(leaves.contains(&2));
        // 0 is the first (leftmost/topmost).
        assert_eq!(leaves[0], 0);
    }

    // ── split ────────────────────────────────────────────────────────────────

    #[test]
    fn split_returns_false_for_missing_id() {
        let mut tree = PaneTree::new_leaf(0);
        assert!(!tree.split(99, SplitDir::Vertical, 1));
    }

    #[test]
    fn split_returns_true_for_existing_id() {
        let mut tree = PaneTree::new_leaf(0);
        assert!(tree.split(0, SplitDir::Vertical, 1));
    }

    #[test]
    fn split_twice_yields_three_leaves() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        tree.split(0, SplitDir::Horizontal, 2);
        assert_eq!(tree.leaves().len(), 3);
    }

    // ── remove_leaf ──────────────────────────────────────────────────────────

    #[test]
    fn remove_left_child_leaves_right() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        tree.remove_leaf(0);
        assert_eq!(tree.leaves(), vec![1]);
    }

    #[test]
    fn remove_right_child_leaves_left() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        tree.remove_leaf(1);
        assert_eq!(tree.leaves(), vec![0]);
    }

    #[test]
    fn remove_middle_child_in_three_pane_tree() {
        // Tree: Split(0, Split(1, 2))
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        tree.split(1, SplitDir::Vertical, 2);
        // Remove 1 → remaining leaves should be [0, 2]
        tree.remove_leaf(1);
        let leaves = tree.leaves();
        assert_eq!(leaves.len(), 2);
        assert!(leaves.contains(&0));
        assert!(leaves.contains(&2));
    }

    // ── layout order ────────────────────────────────────────────────────────

    #[test]
    fn layout_order_left_before_right() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Vertical, 1);
        let layout = tree.layout(rect(0, 0, 800, 600));
        assert_eq!(layout[0].0, 0);
        assert_eq!(layout[1].0, 1);
        // Left rect x < right rect x.
        assert!(layout[0].1.x < layout[1].1.x);
    }

    #[test]
    fn layout_order_top_before_bottom() {
        let mut tree = PaneTree::new_leaf(0);
        tree.split(0, SplitDir::Horizontal, 1);
        let layout = tree.layout(rect(0, 0, 800, 600));
        assert_eq!(layout[0].0, 0);
        assert_eq!(layout[1].0, 1);
        // Top rect y < bottom rect y.
        assert!(layout[0].1.y < layout[1].1.y);
    }
}
