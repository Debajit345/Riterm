//! Pane split tree and rectangle layout.

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn inset(&self, amount: f32) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            w: (self.w - amount * 2.0).max(1.0),
            h: (self.h - amount * 2.0).max(1.0),
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Orientation of a split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    /// Children are placed left and right.
    SideBySide,
    /// Children are placed top and bottom.
    Stacked,
}

/// A tab's pane tree.
#[derive(Clone, Debug)]
pub enum PaneNode {
    Leaf(u64),
    Split {
        dir: SplitDir,
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    pub fn leaf(id: u64) -> Self {
        Self::Leaf(id)
    }

    /// Lay out all leaves, returning pane rectangles and divider rectangles.
    pub fn layout(
        &self,
        rect: Rect,
        gap: f32,
        panes: &mut Vec<(u64, Rect)>,
        dividers: &mut Vec<Rect>,
    ) {
        match self {
            Self::Leaf(id) => panes.push((*id, rect)),
            Self::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let ratio = ratio.clamp(0.05, 0.95);
                match dir {
                    SplitDir::SideBySide => {
                        let usable = (rect.w - gap).max(2.0);
                        let first_w = usable * ratio;
                        let second_w = usable - first_w;
                        first.layout(
                            Rect::new(rect.x, rect.y, first_w, rect.h),
                            gap,
                            panes,
                            dividers,
                        );
                        second.layout(
                            Rect::new(rect.x + first_w + gap, rect.y, second_w, rect.h),
                            gap,
                            panes,
                            dividers,
                        );
                        dividers.push(Rect::new(rect.x + first_w, rect.y, gap, rect.h));
                    }
                    SplitDir::Stacked => {
                        let usable = (rect.h - gap).max(2.0);
                        let first_h = usable * ratio;
                        let second_h = usable - first_h;
                        first.layout(
                            Rect::new(rect.x, rect.y, rect.w, first_h),
                            gap,
                            panes,
                            dividers,
                        );
                        second.layout(
                            Rect::new(rect.x, rect.y + first_h + gap, rect.w, second_h),
                            gap,
                            panes,
                            dividers,
                        );
                        dividers.push(Rect::new(rect.x, rect.y + first_h, rect.w, gap));
                    }
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn leaves(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<u64>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    pub fn first_leaf(&self) -> u64 {
        match self {
            Self::Leaf(id) => *id,
            Self::Split { first, .. } => first.first_leaf(),
        }
    }

    /// Split `target` into two panes; the new pane is placed second.
    pub fn split_leaf(&mut self, target: u64, new_id: u64, dir: SplitDir) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                *self = Self::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(new_id)),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target, new_id, dir) || second.split_leaf(target, new_id, dir)
            }
        }
    }

    /// Remove a leaf, returning the pruned tree (or `None` if it was the last
    /// leaf). The caller replaces its tree with the result.
    pub fn remove_leaf(&self, target: u64) -> Option<Self> {
        match self {
            Self::Leaf(id) => {
                if *id == target {
                    None
                } else {
                    Some(self.clone())
                }
            }
            Self::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let first = first.remove_leaf(target);
                let second = second.remove_leaf(target);
                match (first, second) {
                    (None, None) => None,
                    (Some(node), None) | (None, Some(node)) => Some(node),
                    (Some(first), Some(second)) => Some(Self::Split {
                        dir: *dir,
                        ratio: *ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    }),
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn contains(&self, id: u64) -> bool {
        self.leaves().contains(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_remove() {
        let mut node = PaneNode::leaf(1);
        assert!(node.split_leaf(1, 2, SplitDir::SideBySide));
        assert!(node.split_leaf(2, 3, SplitDir::Stacked));
        assert_eq!(node.leaves(), vec![1, 2, 3]);

        let node = node.remove_leaf(2).unwrap();
        assert_eq!(node.leaves(), vec![1, 3]);
        assert_eq!(node.remove_leaf(1).unwrap().leaves(), vec![3]);
    }

    #[test]
    fn layout_fills_area() {
        let mut node = PaneNode::leaf(1);
        node.split_leaf(1, 2, SplitDir::SideBySide);
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        node.layout(
            Rect::new(0.0, 0.0, 100.0, 50.0),
            1.0,
            &mut panes,
            &mut dividers,
        );
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].1.w + dividers[0].w + panes[1].1.w, 100.0);
    }
}
