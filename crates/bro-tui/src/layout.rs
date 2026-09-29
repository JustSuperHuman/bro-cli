//! The split tree for one tab: every node is either a pane or a split of two children, like tmux.
//! (Ported from z4-oriel.)

use ratatui::layout::Rect;

/// Identifies a pane for the whole run of the app.
pub type PaneId = u64;

/// Split direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    /// children side by side
    Right,
    /// children stacked
    Down,
}

/// A node of the split tree.
#[derive(Debug, Clone)]
pub enum Node {
    Leaf(PaneId),
    Split { dir: Dir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
    /// Every pane in the tree, in reading order.
    pub fn leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    /// Convenience: the leaves as a new Vec.
    pub fn leaf_ids(&self) -> Vec<PaneId> {
        let mut v = vec![];
        self.leaves(&mut v);
        v
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(x) => *x == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Replace leaf `target` with a split of (target, new).
    pub fn split(&mut self, target: PaneId, new: PaneId, dir: Dir) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split { dir, ratio: 0.5, a: Box::new(Node::Leaf(target)), b: Box::new(Node::Leaf(new)) };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, new, dir) || b.split(target, new, dir),
        }
    }

    /// Remove leaf `target`; its sibling takes the parent's place. False if target is the root leaf.
    pub fn remove(&mut self, target: PaneId) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                if matches!(**a, Node::Leaf(id) if id == target) {
                    *self = (**b).clone();
                    return true;
                }
                if matches!(**b, Node::Leaf(id) if id == target) {
                    *self = (**a).clone();
                    return true;
                }
                a.remove(target) || b.remove(target)
            }
        }
    }

    /// Pane rectangles. Splits leave no gap: each pane draws its own rounded frame.
    pub fn rects(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, area)),
            Node::Split { dir, ratio, a, b } => {
                let (ra, rb) = split_rect(area, *dir, *ratio);
                a.rects(ra, out);
                b.rects(rb, out);
            }
        }
    }

    /// Split borders, for mouse dragging: (rect of the whole split, dir, path-to-node).
    pub fn borders(&self, area: Rect, path: &mut Vec<bool>, out: &mut Vec<(Rect, Dir, Vec<bool>)>) {
        if let Node::Split { dir, ratio, a, b } = self {
            out.push((area, *dir, path.clone()));
            let (ra, rb) = split_rect(area, *dir, *ratio);
            path.push(false);
            a.borders(ra, path, out);
            path.pop();
            path.push(true);
            b.borders(rb, path, out);
            path.pop();
        }
    }

    /// The node at a path of (false = a, true = b) steps.
    pub fn node_at(&mut self, path: &[bool]) -> Option<&mut Node> {
        if path.is_empty() {
            return Some(self);
        }
        match self {
            Node::Leaf(_) => None,
            Node::Split { a, b, .. } => if path[0] { b } else { a }.node_at(&path[1..]),
        }
    }

    /// The ratio of the split at `path`, if it is one.
    pub fn ratio_at(&self, path: &[bool]) -> Option<f32> {
        match (self, path.first()) {
            (Node::Split { ratio, .. }, None) => Some(*ratio),
            (Node::Split { a, b, .. }, Some(&step)) => if step { b } else { a }.ratio_at(&path[1..]),
            _ => None,
        }
    }

    /// Grow/shrink the nearest split of `dir` that contains `id`. `delta` > 0 moves the divider right/down.
    pub fn resize(&mut self, id: PaneId, dir: Dir, delta: f32) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { dir: d, ratio, a, b } => {
                let in_a = a.contains(id);
                if !in_a && !b.contains(id) {
                    return false;
                }
                // deepest matching split wins
                let child = if in_a { a } else { b };
                if child.resize(id, dir, delta) {
                    return true;
                }
                if *d == dir {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    return true;
                }
                false
            }
        }
    }
}

/// Split `area` in two along `dir` at `ratio` (each side gets at least one cell when possible).
pub fn split_rect(area: Rect, dir: Dir, ratio: f32) -> (Rect, Rect) {
    match dir {
        Dir::Right => {
            let w = ((area.width as f32) * ratio).round() as u16;
            let w = w.clamp(1.min(area.width), area.width.saturating_sub(1));
            (Rect { width: w, ..area }, Rect { x: area.x + w, width: area.width - w, ..area })
        }
        Dir::Down => {
            let h = ((area.height as f32) * ratio).round() as u16;
            let h = h.clamp(1.min(area.height), area.height.saturating_sub(1));
            (Rect { height: h, ..area }, Rect { y: area.y + h, height: area.height - h, ..area })
        }
    }
}

/// The pane whose rect is nearest in a direction from `from` (for alt+arrow focus).
pub fn neighbor(rects: &[(PaneId, Rect)], from: PaneId, dx: i32, dy: i32) -> Option<PaneId> {
    let (_, r) = rects.iter().find(|(id, _)| *id == from)?;
    let (rx, ry, rw, rh) = (r.x as i32, r.y as i32, r.width as i32, r.height as i32);
    let (cx, cy) = (rx + rw / 2, ry + rh / 2);
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter(|(_, o)| {
            let (ox, oy, ow, oh) = (o.x as i32, o.y as i32, o.width as i32, o.height as i32);
            let overlap_y = oy < ry + rh && oy + oh > ry;
            let overlap_x = ox < rx + rw && ox + ow > rx;
            match (dx, dy) {
                (1, _) => ox >= rx + rw - 1 && overlap_y,
                (-1, _) => ox + ow <= rx + 1 && overlap_y,
                (_, 1) => oy >= ry + rh - 1 && overlap_x,
                _ => oy + oh <= ry + 1 && overlap_x,
            }
        })
        .min_by_key(|(_, o)| {
            let (ox, oy) = (o.x as i32 + o.width as i32 / 2, o.y as i32 + o.height as i32 / 2);
            (ox - cx).abs() + (oy - cy).abs()
        })
        .map(|(id, _)| *id)
}

/// Lay `ids` out as a stack that uses the space well: 2 side by side (or over/under when the area is tall),
/// 3 as one big + two stacked, 4+ as a grid shaped to the area (terminal cells are ~2.2x taller than wide).
pub fn stack_rects(area: Rect, ids: &[PaneId]) -> Vec<(PaneId, Rect)> {
    let n = ids.len();
    if n == 0 {
        return vec![];
    }
    let wide = area.width as f32 >= area.height as f32 * 2.2;
    if n == 1 {
        return vec![(ids[0], area)];
    }
    if n == 2 {
        let (a, b) = split_rect(area, if wide { Dir::Right } else { Dir::Down }, 0.5);
        return vec![(ids[0], a), (ids[1], b)];
    }
    if n == 3 {
        let (big, rest) = split_rect(area, if wide { Dir::Right } else { Dir::Down }, 0.5);
        let (b, c) = split_rect(rest, if wide { Dir::Down } else { Dir::Right }, 0.5);
        return vec![(ids[0], big), (ids[1], b), (ids[2], c)];
    }
    // grid: columns from the area's shape, the last row's panes widen to fill it
    let aspect = area.width as f32 / (area.height as f32 * 2.2).max(1.0);
    let cols = ((n as f32 * aspect).sqrt().round() as usize).clamp(1, n);
    let rows = n.div_ceil(cols);
    let mut out = vec![];
    for r in 0..rows {
        let y0 = area.y + (area.height as usize * r / rows) as u16;
        let y1 = area.y + (area.height as usize * (r + 1) / rows) as u16;
        let in_row = if r + 1 == rows { n - cols * (rows - 1) } else { cols };
        for c in 0..in_row {
            let x0 = area.x + (area.width as usize * c / in_row) as u16;
            let x1 = area.x + (area.width as usize * (c + 1) / in_row) as u16;
            out.push((ids[r * cols + c], Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::new(0, 0, 100, 40)
    }

    #[test]
    fn split_and_remove() {
        let mut n = Node::Leaf(1);
        assert!(n.split(1, 2, Dir::Right));
        assert!(n.split(2, 3, Dir::Down));
        assert_eq!(n.leaf_ids(), vec![1, 2, 3]);
        assert!(!n.split(9, 4, Dir::Down), "unknown target");
        assert!(n.remove(2));
        assert_eq!(n.leaf_ids(), vec![1, 3]);
        assert!(n.remove(1));
        assert_eq!(n.leaf_ids(), vec![3]);
        assert!(!n.remove(3), "the root leaf can't be removed");
    }

    #[test]
    fn rects_tile_the_area() {
        let mut n = Node::Leaf(1);
        n.split(1, 2, Dir::Right);
        n.split(2, 3, Dir::Down);
        let mut out = vec![];
        n.rects(area(), &mut out);
        let total: u32 = out.iter().map(|(_, r)| r.width as u32 * r.height as u32).sum();
        assert_eq!(total, 100 * 40);
        assert_eq!(out[0].1, Rect::new(0, 0, 50, 40));
        assert_eq!(out[1].1, Rect::new(50, 0, 50, 20));
        assert_eq!(out[2].1, Rect::new(50, 20, 50, 20));
    }

    #[test]
    fn neighbors() {
        // 1 | 2
        //   | 3
        let mut n = Node::Leaf(1);
        n.split(1, 2, Dir::Right);
        n.split(2, 3, Dir::Down);
        let mut r = vec![];
        n.rects(area(), &mut r);
        assert_eq!(neighbor(&r, 1, 1, 0), Some(2), "right of 1 is the nearer top pane");
        assert_eq!(neighbor(&r, 3, -1, 0), Some(1));
        assert_eq!(neighbor(&r, 2, 0, 1), Some(3));
        assert_eq!(neighbor(&r, 3, 0, -1), Some(2));
        assert_eq!(neighbor(&r, 1, -1, 0), None);
        assert_eq!(neighbor(&r, 2, 1, 0), None);
    }

    #[test]
    fn resize_clamps_and_targets_the_right_split() {
        let mut n = Node::Leaf(1);
        n.split(1, 2, Dir::Right);
        n.split(2, 3, Dir::Down);
        assert!(n.resize(3, Dir::Right, 0.2));
        assert_eq!(n.ratio_at(&[]), Some(0.7));
        assert!(n.resize(3, Dir::Down, 0.1));
        assert_eq!(n.ratio_at(&[true]), Some(0.6));
        for _ in 0..20 {
            n.resize(1, Dir::Right, 0.1);
        }
        assert_eq!(n.ratio_at(&[]), Some(0.9));
        let mut out = vec![];
        n.borders(area(), &mut vec![], &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn stacks_fill_the_area_without_overlap() {
        let wide = Rect { x: 0, y: 0, width: 200, height: 50 };
        let tall = Rect { x: 0, y: 0, width: 80, height: 60 };
        let two = stack_rects(wide, &[1, 2]);
        assert!(two[0].1.y == two[1].1.y && two[0].1.x < two[1].1.x, "wide: side by side");
        let two = stack_rects(tall, &[1, 2]);
        assert!(two[0].1.x == two[1].1.x && two[0].1.y < two[1].1.y, "tall: over/under");
        let three = stack_rects(wide, &[1, 2, 3]);
        assert_eq!(three[0].1.height, 50, "one big pane");
        for n in 1..=9 {
            let ids: Vec<PaneId> = (0..n).collect();
            let rects = stack_rects(wide, &ids);
            assert_eq!(rects.len(), n as usize);
            let area: u32 = rects.iter().map(|(_, r)| r.width as u32 * r.height as u32).sum();
            assert_eq!(area, 200 * 50, "n={n} covers the area exactly");
            for (i, (_, a)) in rects.iter().enumerate() {
                for (_, b) in &rects[i + 1..] {
                    assert!(a.intersection(*b).is_empty(), "n={n} overlap");
                }
            }
        }
    }
}
