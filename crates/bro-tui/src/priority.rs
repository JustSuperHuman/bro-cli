//! Priority grow: ctrl+click a small session and it grows to a comfortable working size inside the arrangement
//! it already has, then snaps back.
//!
//! The panes on screen (a tab's splits, the tiled grid, project groups in their frames) are read back as a
//! guillotine tree: straight cuts across the whole area, flattened so panes side by side share one level.
//! Growing a pane along an axis walks the cuts of that axis that hold it, innermost first, so its nearest
//! neighbours make room before distant ones. At each level every neighbour gives up the same share of its
//! spare room (its size above a useful minimum) divided by its distance from the pane: near and large
//! neighbours give most, far ones barely move. Only when the spare room is spent everywhere do neighbours
//! fold, nearest first, to a strip that shows just their title. Nothing is reordered, and the base layout is
//! never touched: the grow is applied when drawing, so dropping the priority restores the exact sizes.
//!
//! Held edges ([`lock`]) read the same tree: a tile edge you dragged moves its cut back to where you left it
//! whatever arrangement the tiles take, while every other cut keeps flowing.

use crate::layout::PaneId;
use ratatui::layout::Rect;

/// A comfortable size for an agent's TUI (outer cells): 100x30 inside the frame.
pub const COMFORT: [u16; 2] = [102, 32];
/// Below this a neighbour isn't worth keeping open: it folds instead.
pub const USEFUL: [u16; 2] = [32, 7];
/// A folded neighbour: a title bar (the frame's top and bottom edges) or a narrow strip.
pub const FOLDED: [u16; 2] = [10, 2];

/// How big the prioritized pane wants to be in `body`: comfortable, leaving at least a fifth for the others.
pub fn want(body: Rect) -> [u16; 2] {
    let fifth = |n: u16| (n as u32 * 4 / 5) as u16;
    [COMFORT[0].min(fifth(body.width)), COMFORT[1].min(fifth(body.height))]
}

/// The layout with one pane grown.
#[derive(Debug, Clone, PartialEq)]
pub struct Grown {
    /// Every pane, in the order given.
    pub panes: Vec<(PaneId, Rect)>,
    /// The new region of each framed group, by its index in `regions`.
    pub frames: Vec<(usize, Rect)>,
    /// Neighbours folded to a strip: draw their title only.
    pub folded: Vec<PaneId>,
}

/// `panes` as laid out in `area` (inside `regions` when grouped, each `(region, framed)` with a one-cell ring
/// when framed) with `id` grown toward `want`. None when nothing would change: the pane is already big
/// enough, isn't on screen, or the layout isn't made of straight cuts.
pub fn prioritize(area: Rect, panes: &[(PaneId, Rect)], regions: &[(Rect, bool)], id: PaneId, want: [u16; 2]) -> Option<Grown> {
    if panes.len() < 2 || !panes.iter().any(|(p, _)| *p == id) || panes.iter().any(|(_, r)| r.is_empty()) {
        return None;
    }
    let mut tree = tree(area, panes, regions)?;
    let before = tree.clone();
    tree.grow(id, want);
    if tree == before {
        return None;
    }
    let (mut placed, mut frames) = (vec![], vec![]);
    tree.place(area.x, area.y, &mut placed, &mut frames);
    let at = |p: PaneId| placed.iter().find(|(q, _)| *q == p).map(|x| x.1).unwrap_or_default();
    let folded = panes
        .iter()
        .filter(|(p, old)| {
            let new = at(*p);
            new.width < old.width.min(USEFUL[0]) || new.height < old.height.min(USEFUL[1])
        })
        .map(|(p, _)| *p)
        .collect();
    Some(Grown { panes: panes.iter().map(|(p, _)| (*p, at(*p))).collect(), frames, folded })
}

/// A tile edge you dragged and let go: `pane`'s right or bottom edge (`far`), else its left or top, along `ax`
/// (0 = across, 1 = down), held `at` cells from the start of the area (the edge's first cell past the pane when
/// `far`). Whatever arrangement the tiles take later, the cut that makes this edge moves back there; every
/// other cut keeps flowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lock {
    pub pane: PaneId,
    pub ax: usize,
    pub far: bool,
    pub at: u16,
}

/// The least a pane is squeezed to beside a held edge.
pub const LOCK_MIN: [u16; 2] = [16, 5];

/// `panes` as laid out in `area` (see [`prioritize`] for `regions`; panes outside every region, like the
/// orchestrator's column, are parts of their own) with every edge in `locks` held where it was left. Outer cuts
/// move first, so an edge inside a moved one still lands where it was left; of two locks on one cut, the later
/// wins. A cut never squeezes a pane below [`LOCK_MIN`]. None when nothing would change.
pub fn lock(area: Rect, panes: &[(PaneId, Rect)], regions: &[(Rect, bool)], locks: &[Lock]) -> Option<Grown> {
    if panes.len() < 2 || locks.is_empty() || panes.iter().any(|(_, r)| r.is_empty()) {
        return None;
    }
    let mut tree = tree(area, panes, regions)?;
    let before = tree.clone();
    let mut held: Vec<(usize, Vec<usize>, Lock)> = locks
        .iter()
        .filter_map(|l| {
            let path = tree.path(l.pane)?;
            Some((tree.cut_of(&path, l.ax, l.far)?, path, *l))
        })
        .collect();
    held.sort_by_key(|(k, ..)| *k);
    for (k, path, l) in held {
        tree.hold(&path, k, l);
    }
    if tree == before {
        return None;
    }
    let (mut placed, mut frames) = (vec![], vec![]);
    tree.place(area.x, area.y, &mut placed, &mut frames);
    let at = |p: PaneId| placed.iter().find(|(q, _)| *q == p).map(|x| x.1).unwrap_or_default();
    Some(Grown { panes: panes.iter().map(|(p, _)| (*p, at(*p))).collect(), frames, folded: vec![] })
}

/// Whether two tile edges are one cut in this layout (a pane's right edge and the left edge of the one across
/// it, or of any pane along the same divider).
pub fn same_cut(area: Rect, panes: &[(PaneId, Rect)], regions: &[(Rect, bool)], a: Lock, b: Lock) -> bool {
    let Some(tree) = tree(area, panes, regions) else { return false };
    let id = |l: Lock| {
        let path = tree.path(l.pane)?;
        let k = tree.cut_of(&path, l.ax, l.far)?;
        let i = if l.far { path[k] } else { path[k] - 1 };
        Some((l.ax, path[..k].to_vec(), i))
    };
    a.ax == b.ax && id(a).is_some_and(|x| id(b) == Some(x))
}

/// The guillotine tree of `panes` tiling `area`: each of `regions` one part (inside its ring when framed), and
/// each pane outside them another. None unless it holds every pane.
fn tree(area: Rect, panes: &[(PaneId, Rect)], regions: &[(Rect, bool)]) -> Option<Tile> {
    let leaf = |&(p, r): &(PaneId, Rect)| (r, Tile { size: [r.width, r.height], kind: Kind::Leaf(p) });
    let tree = if regions.is_empty() {
        cut(area, panes.iter().map(leaf).collect())?
    } else {
        let mut parts = vec![];
        for (tag, &(region, framed)) in regions.iter().enumerate() {
            let inside = if framed { Rect { x: region.x + 1, y: region.y + 1, width: region.width.checked_sub(2)?, height: region.height.checked_sub(2)? } } else { region };
            let sub = cut(inside, panes.iter().filter(|(_, r)| inside.contains(r.as_position())).map(leaf).collect())?;
            let tile = if framed { Tile { size: [region.width, region.height], kind: Kind::Frame(tag, Box::new(sub)) } } else { sub };
            parts.push((region, tile));
        }
        parts.extend(panes.iter().filter(|(_, r)| !regions.iter().any(|(g, _)| g.contains(r.as_position()))).map(leaf));
        cut(area, parts)?
    };
    (tree.count() == panes.len()).then_some(tree)
}

/// A node of the guillotine tree. `size` is (width, height); axis 0 is side by side, 1 is stacked.
#[derive(Debug, Clone, PartialEq)]
struct Tile {
    size: [u16; 2],
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Leaf(PaneId),
    /// A project group: its panes inside a one-cell ring.
    Frame(usize, Box<Tile>),
    /// Children laid side by side (axis 0) or stacked (axis 1), each spanning the whole other axis.
    Split(usize, Vec<Tile>),
}

fn start(r: Rect, ax: usize) -> u16 {
    if ax == 0 { r.x } else { r.y }
}

fn end(r: Rect, ax: usize) -> u16 {
    if ax == 0 { r.right() } else { r.bottom() }
}

/// The tree for `parts` that tile `area` exactly: every straight cut across it, then each strip in turn.
fn cut(area: Rect, parts: Vec<(Rect, Tile)>) -> Option<Tile> {
    if parts.len() == 1 {
        let (r, t) = parts.into_iter().next()?;
        return (r == area).then_some(t);
    }
    if parts.is_empty() {
        return None;
    }
    // rows first: a full grid reads as rows of panes, as the tiles are made
    for ax in [1, 0] {
        let (lo, hi) = (start(area, ax), end(area, ax));
        let mut cuts: Vec<u16> = parts
            .iter()
            .map(|(r, _)| end(*r, ax))
            .filter(|&p| p > lo && p < hi && parts.iter().all(|(r, _)| end(*r, ax) <= p || start(*r, ax) >= p))
            .collect();
        if cuts.is_empty() {
            continue;
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(hi);
        let (mut kids, mut from, mut rest) = (vec![], lo, parts);
        for c in cuts {
            let (mine, others): (Vec<_>, Vec<_>) = rest.into_iter().partition(|(r, _)| end(*r, ax) <= c);
            rest = others;
            let strip = if ax == 0 { Rect { x: from, width: c - from, ..area } } else { Rect { y: from, height: c - from, ..area } };
            kids.push(cut(strip, mine)?);
            from = c;
        }
        return Some(Tile { size: [area.width, area.height], kind: Kind::Split(ax, kids) });
    }
    None
}

impl Tile {
    fn count(&self) -> usize {
        match &self.kind {
            Kind::Leaf(_) => 1,
            Kind::Frame(_, t) => t.count(),
            Kind::Split(_, kids) => kids.iter().map(Tile::count).sum(),
        }
    }

    fn contains(&self, id: PaneId) -> bool {
        match &self.kind {
            Kind::Leaf(p) => *p == id,
            Kind::Frame(_, t) => t.contains(id),
            Kind::Split(_, kids) => kids.iter().any(|k| k.contains(id)),
        }
    }

    /// Child indices from here down to `id` (a frame counts as one step).
    fn path(&self, id: PaneId) -> Option<Vec<usize>> {
        match &self.kind {
            Kind::Leaf(p) => (*p == id).then(Vec::new),
            Kind::Frame(_, t) => t.path(id).map(|mut p| {
                p.insert(0, 0);
                p
            }),
            Kind::Split(_, kids) => kids.iter().enumerate().find_map(|(i, k)| {
                k.path(id).map(|mut p| {
                    p.insert(0, i);
                    p
                })
            }),
        }
    }

    fn node(&self, path: &[usize]) -> &Tile {
        match (path.first(), &self.kind) {
            (Some(_), Kind::Frame(_, t)) => t.node(&path[1..]),
            (Some(&i), Kind::Split(_, kids)) => kids[i].node(&path[1..]),
            _ => self,
        }
    }

    fn node_mut(&mut self, path: &[usize]) -> &mut Tile {
        if path.is_empty() || matches!(self.kind, Kind::Leaf(_)) {
            return self;
        }
        match &mut self.kind {
            Kind::Frame(_, t) => t.node_mut(&path[1..]),
            Kind::Split(_, kids) => kids[path[0]].node_mut(&path[1..]),
            Kind::Leaf(_) => unreachable!(),
        }
    }

    /// The least this can shrink to along `ax` with every pane at `floor` (or its size, if already smaller).
    fn floor(&self, ax: usize, floor: [u16; 2]) -> u16 {
        match &self.kind {
            Kind::Leaf(_) => self.size[ax].min(floor[ax]),
            Kind::Frame(_, t) => t.floor(ax, floor) + 2,
            Kind::Split(a, kids) if *a == ax => kids.iter().map(|k| k.floor(ax, floor)).sum(),
            Kind::Split(_, kids) => kids.iter().map(|k| k.floor(ax, floor)).max().unwrap_or(0),
        }
    }

    /// Grow `id` along each axis toward `want`.
    fn grow(&mut self, id: PaneId, want: [u16; 2]) {
        let Some(path) = self.path(id) else { return };
        for (ax, goal) in want.into_iter().enumerate() {
            let mut need = goal.saturating_sub(self.node(&path).size[ax]) as u32;
            // the cuts of this axis that hold the pane, innermost (nearest neighbours) first
            let levels: Vec<usize> = (0..path.len()).rev().filter(|&k| matches!(self.node(&path[..k]).kind, Kind::Split(a, _) if a == ax)).collect();
            // spare room everywhere first; folding only when that isn't enough
            for fold in [false, true] {
                for &k in &levels {
                    if need == 0 {
                        continue;
                    }
                    let t = path[k];
                    let Kind::Split(_, kids) = &mut self.node_mut(&path[..k]).kind else { continue };
                    let others: Vec<usize> = (0..kids.len()).filter(|&i| i != t).collect();
                    let sizes: Vec<u16> = others.iter().map(|&i| kids[i].size[ax]).collect();
                    let useful: Vec<u16> = others.iter().map(|&i| kids[i].floor(ax, USEFUL)).collect();
                    let folded: Vec<u16> = others.iter().map(|&i| kids[i].floor(ax, FOLDED)).collect();
                    let dist: Vec<u32> = others.iter().map(|&i| i.abs_diff(t) as u32).collect();
                    let new = reduce(&sizes, &useful, &folded, &dist, need, fold);
                    let taken = sizes.iter().map(|&s| s as u32).sum::<u32>() - new.iter().map(|&s| s as u32).sum::<u32>();
                    for (&i, n) in others.iter().zip(new) {
                        kids[i].fit(ax, n, i < t);
                    }
                    kids[t].grow_toward(ax, taken as u16, id);
                    need = need.saturating_sub(taken);
                }
            }
        }
    }

    /// Add `extra` along `ax`, all of it on the way to `id` (panes beside it in the same column/row widen too).
    fn grow_toward(&mut self, ax: usize, extra: u16, id: PaneId) {
        self.size[ax] += extra;
        match &mut self.kind {
            Kind::Leaf(_) => {}
            Kind::Frame(_, t) => t.grow_toward(ax, extra, id),
            Kind::Split(a, kids) if *a != ax => {
                for k in kids {
                    if k.contains(id) {
                        k.grow_toward(ax, extra, id);
                    } else {
                        let n = k.size[ax] + extra;
                        k.fit(ax, n, false);
                    }
                }
            }
            Kind::Split(_, kids) => {
                if let Some(k) = kids.iter_mut().find(|k| k.contains(id)) {
                    k.grow_toward(ax, extra, id);
                }
            }
        }
    }

    /// Resize to `len` along `ax`, spreading the change over the children: growth in proportion to their
    /// size, shrinking like a level of [`Tile::grow`] measured from the edge nearest the grown pane
    /// (`near_end`: that's the end).
    fn fit(&mut self, ax: usize, len: u16, near_end: bool) {
        let cur = self.size[ax];
        self.size[ax] = len;
        match &mut self.kind {
            Kind::Leaf(_) => {}
            Kind::Frame(_, t) => t.fit(ax, len.saturating_sub(2), near_end),
            Kind::Split(a, kids) if *a != ax => {
                for k in kids {
                    k.fit(ax, len, near_end);
                }
            }
            Kind::Split(_, kids) => {
                let sizes: Vec<u16> = kids.iter().map(|k| k.size[ax]).collect();
                let new = if len >= cur {
                    spread(&sizes, len - cur)
                } else {
                    let n = kids.len();
                    let dist: Vec<u32> = (0..n).map(|i| if near_end { (n - i) as u32 } else { i as u32 + 1 }).collect();
                    let useful: Vec<u16> = kids.iter().map(|k| k.floor(ax, USEFUL)).collect();
                    let folded: Vec<u16> = kids.iter().map(|k| k.floor(ax, FOLDED)).collect();
                    let mut new = reduce(&sizes, &useful, &folded, &dist, (cur - len) as u32, true);
                    // folding is all or nothing: what it freed beyond the need goes to the largest pane still open
                    let total: u32 = new.iter().map(|&s| s as u32).sum();
                    if let Some(surplus) = (len as u32).checked_sub(total).filter(|s| *s > 0) {
                        let open = (0..n).filter(|&i| new[i] > folded[i]).max_by_key(|&i| new[i]);
                        let far = (0..n).max_by_key(|&i| dist[i]).unwrap_or(0);
                        new[open.unwrap_or(far)] += surplus as u16;
                    }
                    new
                };
                for (k, n) in kids.iter_mut().zip(new) {
                    k.fit(ax, n, near_end);
                }
            }
        }
    }

    /// The depth of the cut that makes the edge of the pane at `path`: the innermost split along `ax` where the
    /// pane isn't the last child (`far`) or the first. None on the area's own edge.
    fn cut_of(&self, path: &[usize], ax: usize, far: bool) -> Option<usize> {
        (0..path.len()).rev().find(|&k| match &self.node(&path[..k]).kind {
            Kind::Split(a, kids) if *a == ax => if far { path[k] + 1 < kids.len() } else { path[k] > 0 },
            _ => false,
        })
    }

    /// Where the node at `path` starts along `ax`, from this one's start.
    fn offset(&self, path: &[usize], ax: usize) -> u16 {
        match (path.first(), &self.kind) {
            (Some(_), Kind::Frame(_, t)) => 1 + t.offset(&path[1..], ax),
            (Some(&i), Kind::Split(a, kids)) => {
                let before: u16 = if *a == ax { kids[..i].iter().map(|k| k.size[ax]).sum() } else { 0 };
                before + kids[i].offset(&path[1..], ax)
            }
            _ => 0,
        }
    }

    /// Move the cut at depth `k` on `path` (see [`Tile::cut_of`]) so the pane's edge lands on `l.at`. The two
    /// children beside the cut trade the room; once one is down to [`LOCK_MIN`] the next one along gives way
    /// too, so an edge can be dragged past its neighbour.
    fn hold(&mut self, path: &[usize], k: usize, l: Lock) {
        let ax = l.ax;
        let start = self.offset(&path[..k], ax);
        // project frames between the cut and the pane: a ring cell each
        let rings = (k + 1..path.len()).filter(|&j| matches!(self.node(&path[..j]).kind, Kind::Frame(..))).count() as u16;
        let Kind::Split(_, kids) = &mut self.node_mut(&path[..k]).kind else { return };
        let i = if l.far { path[k] } else { path[k] - 1 };
        let cut = if l.far { l.at.saturating_add(rings) } else { l.at.saturating_sub(rings) };
        let mut new: Vec<u16> = kids.iter().map(|c| c.size[ax]).collect();
        let floors: Vec<u16> = kids.iter().map(|c| c.floor(ax, LOCK_MIN)).collect();
        let sum = |s: &[u16]| s.iter().sum::<u16>();
        let (cur, total) = (sum(&new[..=i]), sum(&new));
        let (lo, hi) = (sum(&floors[..=i]), total.saturating_sub(sum(&floors[i + 1..])));
        if lo > hi {
            return;
        }
        let want = cut.saturating_sub(start).clamp(lo, hi);
        // the child beside the cut takes or gives it all; the other side gives or takes it nearest first
        let (grow, give): (usize, Vec<usize>) = if want >= cur { (i, (i + 1..new.len()).collect()) } else { (i + 1, (0..=i).rev().collect()) };
        let mut left = want.abs_diff(cur);
        new[grow] += left;
        for j in give {
            let d = left.min(new[j] - floors[j]);
            new[j] -= d;
            left -= d;
        }
        for (c, n) in kids.iter_mut().zip(new) {
            c.scale(ax, n);
        }
    }

    /// Resize to `len` along `ax`: growth spread in proportion to the children's sizes, shrinking taken from
    /// their room above [`LOCK_MIN`] in proportion to that room (from the end, past it, on a tiny screen).
    fn scale(&mut self, ax: usize, len: u16) {
        let cur = self.size[ax];
        self.size[ax] = len;
        match &mut self.kind {
            Kind::Leaf(_) => {}
            Kind::Frame(_, t) => t.scale(ax, len.saturating_sub(2)),
            Kind::Split(a, kids) if *a != ax => {
                for k in kids {
                    k.scale(ax, len);
                }
            }
            Kind::Split(_, kids) => {
                let sizes: Vec<u16> = kids.iter().map(|k| k.size[ax]).collect();
                let new = if len >= cur {
                    spread(&sizes, len - cur)
                } else {
                    let spare: Vec<u32> = kids.iter().map(|k| k.size[ax].saturating_sub(k.floor(ax, LOCK_MIN)) as u32).collect();
                    let (need, room) = ((cur - len) as u32, spare.iter().sum::<u32>());
                    let mut new = share(&sizes, &spare, &vec![1; kids.len()], need.min(room));
                    let mut left = need.saturating_sub(room) as u16;
                    for s in new.iter_mut().rev() {
                        let d = left.min(*s);
                        *s -= d;
                        left -= d;
                    }
                    new
                };
                for (k, n) in kids.iter_mut().zip(new) {
                    k.scale(ax, n);
                }
            }
        }
    }

    fn place(&self, x: u16, y: u16, panes: &mut Vec<(PaneId, Rect)>, frames: &mut Vec<(usize, Rect)>) {
        match &self.kind {
            Kind::Leaf(id) => panes.push((*id, Rect::new(x, y, self.size[0], self.size[1]))),
            Kind::Frame(tag, t) => {
                frames.push((*tag, Rect::new(x, y, self.size[0], self.size[1])));
                t.place(x + 1, y + 1, panes, frames);
            }
            Kind::Split(ax, kids) => {
                let (mut x, mut y) = (x, y);
                for k in kids {
                    k.place(x, y, panes, frames);
                    if *ax == 0 { x += k.size[0] } else { y += k.size[1] }
                }
            }
        }
    }
}

/// Siblings' new sizes after giving up `need` cells. Spare room (size above `useful`) goes first, shared by
/// [`share`]; if that is not enough and `fold` is set, whole siblings fold to `folded`, nearest first (the
/// larger of two at the same distance), until the rest can cover what is left. A fold can free more than
/// needed; less than `need` is taken only when everything is spent.
fn reduce(sizes: &[u16], useful: &[u16], folded: &[u16], dist: &[u32], need: u32, fold: bool) -> Vec<u16> {
    let n = sizes.len();
    let spare: Vec<u32> = (0..n).map(|i| sizes[i].saturating_sub(useful[i]) as u32).collect();
    let room: u32 = spare.iter().sum();
    if room >= need || !fold {
        return share(sizes, &spare, dist, need.min(room));
    }
    let gain = |i: usize| sizes[i].saturating_sub(folded[i]) as u32;
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| (dist[i], std::cmp::Reverse(sizes[i])));
    let mut is_folded = vec![false; n];
    for &i in &order {
        is_folded[i] = true;
        let avail: u32 = (0..n).map(|j| if is_folded[j] { gain(j) } else { spare[j] }).sum();
        if avail >= need {
            break;
        }
    }
    let mut out = sizes.to_vec();
    let mut freed = 0;
    for i in 0..n {
        if is_folded[i] {
            freed += gain(i);
            out[i] = folded[i].min(sizes[i]);
        }
    }
    if freed < need {
        let spare: Vec<u32> = (0..n).map(|i| if is_folded[i] { 0 } else { spare[i] }).collect();
        let left = need - freed;
        out = share(&out, &spare, dist, left.min(spare.iter().sum()));
    }
    out
}

/// `sizes` grown by `extra` in proportion to their size (whole cells by largest remainder).
fn spread(sizes: &[u16], extra: u16) -> Vec<u16> {
    let total: u32 = sizes.iter().map(|&s| s as u32).sum();
    let mut out = sizes.to_vec();
    if out.is_empty() || extra == 0 {
        return out;
    }
    if total == 0 {
        if let Some(last) = out.last_mut() {
            *last += extra;
        }
        return out;
    }
    let exact: Vec<u32> = sizes.iter().map(|&s| s as u32 * extra as u32).collect();
    let mut given = 0;
    for (o, e) in out.iter_mut().zip(&exact) {
        *o += (e / total) as u16;
        given += e / total;
    }
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(exact[i] % total));
    for &i in order.iter().take((extra as u32 - given) as usize) {
        out[i] += 1;
    }
    out
}

/// Take `amount` (at most the total `spare`) from `sizes`: each gives `min(spare, λ·spare/dist)` for the one λ
/// that adds up, so neighbours at the same distance give up the same fraction of their spare room and farther
/// ones less. Whole cells by largest remainder, nearer first on ties.
fn share(sizes: &[u16], spare: &[u32], dist: &[u32], amount: u32) -> Vec<u16> {
    let n = sizes.len();
    let mut out = sizes.to_vec();
    if amount == 0 || n == 0 {
        return out;
    }
    let take = |l: f64, i: usize| (spare[i] as f64).min(l * spare[i] as f64 / dist[i].max(1) as f64);
    let (mut lo, mut hi) = (0.0, dist.iter().copied().max().unwrap_or(1).max(1) as f64);
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        if (0..n).map(|i| take(mid, i)).sum::<f64>() < amount as f64 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let exact: Vec<f64> = (0..n).map(|i| take(hi, i)).collect();
    let mut got: Vec<u32> = (0..n).map(|i| (exact[i].floor() as u32).min(spare[i])).collect();
    while got.iter().sum::<u32>() > amount {
        let Some(i) = (0..n).filter(|&i| got[i] > 0).max_by_key(|&i| dist[i]) else { break };
        got[i] -= 1;
    }
    let mut left = amount.saturating_sub(got.iter().sum());
    while left > 0 {
        let frac = |i: usize| exact[i] - got[i] as f64;
        let Some(i) = (0..n)
            .filter(|&i| got[i] < spare[i])
            .max_by(|&a, &b| frac(a).partial_cmp(&frac(b)).unwrap_or(std::cmp::Ordering::Equal).then(dist[b].cmp(&dist[a])))
        else {
            break;
        };
        got[i] += 1;
        left -= 1;
    }
    for i in 0..n {
        out[i] -= got[i] as u16;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Dir, Node, group_regions, stack_rects};

    fn find(g: &Grown, id: PaneId) -> Rect {
        g.panes.iter().find(|(p, _)| *p == id).unwrap().1
    }

    /// The panes tile `area` exactly: inside it, no overlaps, nothing missing.
    fn tiles(area: Rect, rects: &[(PaneId, Rect)]) {
        let covered: u32 = rects.iter().map(|(_, r)| r.width as u32 * r.height as u32).sum();
        assert_eq!(covered, area.width as u32 * area.height as u32, "{rects:?}");
        for (i, (_, a)) in rects.iter().enumerate() {
            assert_eq!(area.union(*a), area, "{a:?} leaves {area:?}");
            for (_, b) in &rects[i + 1..] {
                assert!(a.intersection(*b).is_empty(), "{a:?} overlaps {b:?}");
            }
        }
    }

    /// Same arrangement: the new rects are the base layout's cuts, in the same order, at other sizes.
    fn same_order(base: &[(PaneId, Rect)], after: &[(PaneId, Rect)]) {
        let area = base.iter().fold(base[0].1, |a, (_, r)| a.union(*r));
        let leaf = |&(p, r): &(PaneId, Rect)| (r, Tile { size: [r.width, r.height], kind: Kind::Leaf(p) });
        let tree = cut(area, base.iter().map(leaf).collect()).unwrap();
        assert!(shaped(&tree, area, after), "{base:?}
{after:?}");
    }

    fn ids(t: &Tile, out: &mut Vec<PaneId>) {
        match &t.kind {
            Kind::Leaf(p) => out.push(*p),
            Kind::Frame(_, t) => ids(t, out),
            Kind::Split(_, kids) => kids.iter().for_each(|k| ids(k, out)),
        }
    }

    fn shaped(t: &Tile, area: Rect, after: &[(PaneId, Rect)]) -> bool {
        match &t.kind {
            Kind::Leaf(id) => after.iter().any(|(p, r)| p == id && *r == area),
            Kind::Frame(_, inner) => shaped(inner, Rect { x: area.x + 1, y: area.y + 1, width: area.width - 2, height: area.height - 2 }, after),
            Kind::Split(ax, kids) => {
                let mut from = start(area, *ax);
                for k in kids {
                    let mut mine = vec![];
                    ids(k, &mut mine);
                    let rects: Vec<Rect> = after.iter().filter(|(p, _)| mine.contains(p)).map(|x| x.1).collect();
                    let bb = rects.iter().fold(rects[0], |a, r| a.union(*r));
                    let to = end(bb, *ax);
                    if start(bb, *ax) != from || to <= from {
                        return false;
                    }
                    let strip = if *ax == 0 { Rect { x: from, width: to - from, ..area } } else { Rect { y: from, height: to - from, ..area } };
                    if !shaped(k, strip, after) {
                        return false;
                    }
                    from = to;
                }
                from == end(area, *ax)
            }
        }
    }

    #[test]
    fn grows_a_grid_pane_from_its_row_and_the_rows_above() {
        let area = Rect::new(0, 0, 200, 48);
        let ids: Vec<PaneId> = (1..=5).collect(); // 66 | 67 | 67 over 100 | 100
        let base = stack_rects(area, &ids);
        let g = prioritize(area, &base, &[], 4, want(area)).expect("grows");
        tiles(area, &g.panes);
        same_order(&base, &g.panes);
        assert_eq!((find(&g, 4).width, find(&g, 4).height), (102, 32));
        assert_eq!((find(&g, 5).width, find(&g, 5).height), (98, 32), "its row-mate gives the width");
        assert!(g.folded.is_empty(), "the spare room is enough: {g:?}");
        for ((_, old), (_, new)) in base.iter().zip(&g.panes).take(3) {
            assert_eq!(new.width, old.width, "the row above keeps its widths");
            assert_eq!(new.height, 48 - 32);
        }
    }

    #[test]
    fn nearest_neighbours_give_most() {
        let area = Rect::new(0, 0, 240, 40);
        let ids = [1, 2, 3, 4];
        let base: Vec<(PaneId, Rect)> = ids.iter().map(|&i| (i, Rect::new((i as u16 - 1) * 60, 0, 60, 40))).collect();
        let g = prioritize(area, &base, &[], 1, [130, 10]).unwrap();
        tiles(area, &g.panes);
        let w = |id| find(&g, id).width;
        assert_eq!(w(1), 130);
        assert!(w(2) < w(3) && w(3) < w(4) && w(4) < 60, "{} {} {}", w(2), w(3), w(4));
        assert!(g.folded.is_empty());
    }

    #[test]
    fn neighbours_fold_when_spare_room_runs_out() {
        let area = Rect::new(0, 0, 120, 40);
        let base = stack_rects(area, &[1, 2]); // 60 | 60
        let g = prioritize(area, &base, &[], 2, [102, 32]).unwrap();
        tiles(area, &g.panes);
        assert_eq!(find(&g, 1).width, FOLDED[0], "60 - 32 spare isn't 42: the neighbour folds");
        assert_eq!(find(&g, 2).width, 110, "a fold is whole; the pane keeps the surplus");
        assert_eq!(g.folded, vec![1]);
        // stacked: the other pane folds to a title bar
        let tall = Rect::new(0, 0, 100, 40);
        let base = vec![(1, Rect::new(0, 0, 100, 20)), (2, Rect::new(0, 20, 100, 20))];
        let g = prioritize(tall, &base, &[], 1, [80, 36]).unwrap();
        assert_eq!(find(&g, 2).height, FOLDED[1]);
        assert_eq!(find(&g, 1), Rect::new(0, 0, 100, 38));
    }

    #[test]
    fn folds_nearest_first_and_spares_the_rest() {
        let area = Rect::new(0, 0, 160, 30);
        let base: Vec<(PaneId, Rect)> = (0..4).map(|i| (i + 1, Rect::new(i as u16 * 40, 0, 40, 30))).collect();
        let g = prioritize(area, &base, &[], 1, [102, 30]).unwrap();
        tiles(area, &g.panes);
        assert_eq!(g.folded, vec![2, 3], "the two next to it fold");
        assert_eq!(find(&g, 4).width, USEFUL[0], "the farthest stays open");
        assert_eq!(find(&g, 1).width, 108);
    }

    #[test]
    fn already_big_enough_changes_nothing() {
        let area = Rect::new(0, 0, 240, 70);
        let base = stack_rects(area, &[1, 2, 3, 4]);
        assert_eq!(prioritize(area, &base, &[], 3, want(area)), None);
        assert_eq!(prioritize(area, &base, &[], 9, want(area)), None, "not on screen");
        assert_eq!(prioritize(area, &base[..1], &[], 1, want(area)), None, "alone");
    }

    #[test]
    fn split_tree_grows_with_its_column() {
        // 1 | 2
        //   | 3
        let mut n = Node::Leaf(1);
        n.split(1, 2, Dir::Right);
        n.split(2, 3, Dir::Down);
        let area = Rect::new(0, 0, 160, 40);
        let mut base = vec![];
        n.rects(area, &mut base);
        let g = prioritize(area, &base, &[], 2, want(area)).unwrap();
        tiles(area, &g.panes);
        same_order(&base, &g.panes);
        assert_eq!(find(&g, 2).width, 102);
        assert_eq!(find(&g, 3).width, 102, "the pane below shares the column");
        assert_eq!(find(&g, 1).width, 58);
        assert_eq!(find(&g, 2).height, 32);
        assert_eq!(find(&g, 3).height, 8);
    }

    #[test]
    fn grouped_projects_keep_their_frames() {
        let area = Rect::new(0, 0, 200, 44);
        let regions = group_regions(area, &[2, 1]);
        let mut panes = vec![];
        let mut framed = vec![];
        for (r, ids) in regions.iter().zip([vec![1, 2], vec![3]]) {
            let inside = Rect { x: r.x + 1, y: r.y + 1, width: r.width - 2, height: r.height - 2 };
            panes.extend(stack_rects(inside, &ids));
            framed.push((*r, true));
        }
        let g = prioritize(area, &panes, &framed, 3, want(area)).unwrap();
        assert_eq!(find(&g, 3).width, 102);
        assert_eq!(g.frames.len(), 2);
        let frame = |tag| g.frames.iter().find(|(t, _)| *t == tag).unwrap().1;
        assert_eq!(frame(1).width, 104, "the group frame grows around it");
        assert_eq!(frame(0).width + frame(1).width, 200);
        for (tag, ids) in [(0, vec![1, 2]), (1, vec![3])] {
            let f = frame(tag);
            let inside = Rect { x: f.x + 1, y: f.y + 1, width: f.width - 2, height: f.height - 2 };
            let mine: Vec<_> = g.panes.iter().filter(|(p, _)| ids.contains(p)).copied().collect();
            tiles(inside, &mine);
        }
    }

    #[test]
    fn grows_are_pure_so_restoring_is_exact() {
        let area = Rect::new(0, 0, 180, 50);
        let base = stack_rects(area, &(1..=7).collect::<Vec<_>>());
        let copy = base.clone();
        let a = prioritize(area, &base, &[], 2, want(area)).unwrap();
        assert_eq!(base, copy);
        assert_eq!(prioritize(area, &base, &[], 2, want(area)).unwrap(), a, "deterministic");
        // moving the priority grows the other pane from the same base
        let b = prioritize(area, &base, &[], 6, want(area)).unwrap();
        assert!(find(&b, 6).width > base[5].1.width || find(&b, 6).height > base[5].1.height);
    }

    #[test]
    fn tiny_screens_never_break_the_tiling() {
        for w in 0..=40 {
            for h in 0..=14 {
                let area = Rect::new(3, 1, w, h);
                for n in 1..=7 {
                    let ids: Vec<PaneId> = (1..=n).collect();
                    let base = stack_rects(area, &ids);
                    for &id in &ids {
                        for want in [want(area), COMFORT, [w, h], [u16::MAX, u16::MAX]] {
                            if let Some(g) = prioritize(area, &base, &[], id, want) {
                                tiles(area, &g.panes);
                                same_order(&base, &g.panes);
                                let (old, new) = (base.iter().find(|(p, _)| *p == id).unwrap().1, find(&g, id));
                                assert!(new.width >= old.width && new.height >= old.height, "{old:?} -> {new:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    fn held(pane: PaneId, ax: usize, far: bool, at: u16) -> Lock {
        Lock { pane, ax, far, at }
    }

    #[test]
    fn a_held_edge_stays_put_as_the_tiles_change() {
        let area = Rect::new(0, 0, 200, 48);
        let l = held(1, 0, true, 130);
        let g = lock(area, &stack_rects(area, &[1, 2]), &[], &[l]).unwrap();
        tiles(area, &g.panes);
        assert_eq!((find(&g, 1).right(), find(&g, 2).width), (130, 70));
        // a third session opens: 1 keeps its edge, the two beside it share the rest
        let base = stack_rects(area, &[1, 2, 3]);
        let g = lock(area, &base, &[], &[l]).unwrap();
        tiles(area, &g.panes);
        same_order(&base, &g.panes);
        assert_eq!(find(&g, 1).right(), 130);
        assert_eq!((find(&g, 2).x, find(&g, 3).x), (130, 130));
        // in a grid it's the cut in 1's own row
        let base = stack_rects(area, &(1..=5).collect::<Vec<_>>());
        let g = lock(area, &base, &[], &[l]).unwrap();
        tiles(area, &g.panes);
        assert_eq!(find(&g, 1).right(), 130);
        assert_eq!(find(&g, 4).width, base[3].1.width, "other rows keep flowing");
        assert_eq!(lock(area, &base, &[], &[held(9, 0, true, 50)]), None, "not on screen");
        assert_eq!(lock(area, &base, &[], &[held(3, 0, true, 120)]), None, "the area's own edge");
    }

    #[test]
    fn near_edges_bottoms_and_two_locks() {
        let area = Rect::new(0, 0, 200, 48);
        let base = stack_rects(area, &[1, 2, 3]); // 1 | 2 over 3
        let g = lock(area, &base, &[], &[held(2, 0, false, 80), held(2, 1, true, 30)]).unwrap();
        tiles(area, &g.panes);
        assert_eq!((find(&g, 2).x, find(&g, 2).bottom()), (80, 30));
        assert_eq!((find(&g, 1).width, find(&g, 3).height), (80, 18));
        // the bottom held from the pane below, the edge between them is the same cut
        let g = lock(area, &base, &[], &[held(2, 1, true, 30), held(3, 1, false, 20)]).unwrap();
        assert_eq!(find(&g, 3).y, 20, "the later of two locks on one cut wins");
        assert!(same_cut(area, &base, &[], held(2, 1, true, 0), held(3, 1, false, 0)));
        assert!(same_cut(area, &base, &[], held(1, 0, true, 0), held(3, 0, false, 0)), "3 is across the same divider");
        assert!(!same_cut(area, &base, &[], held(1, 0, true, 0), held(2, 1, true, 0)));
        // a 2x2 grid is two rows: the dividers in each row line up but are separate cuts
        let grid = stack_rects(Rect::new(0, 0, 120, 60), &[1, 2, 3, 4]);
        assert!(!same_cut(Rect::new(0, 0, 120, 60), &grid, &[], held(1, 0, true, 0), held(3, 0, true, 0)));
    }

    #[test]
    fn held_edges_stop_short_of_squeezing_a_pane() {
        let area = Rect::new(0, 0, 120, 40);
        let base = stack_rects(area, &[1, 2]);
        let g = lock(area, &base, &[], &[held(1, 0, true, 119)]).unwrap();
        assert_eq!(find(&g, 2).width, LOCK_MIN[0]);
        let g = lock(area, &base, &[], &[held(1, 0, true, 0)]).unwrap();
        assert_eq!(find(&g, 1).width, LOCK_MIN[0]);
    }

    #[test]
    fn held_edges_move_project_frames_and_the_column() {
        let area = Rect::new(0, 0, 200, 44);
        let tiles_area = Rect { width: 150, ..area };
        let regions = group_regions(tiles_area, &[2, 1]);
        let mut panes = vec![];
        let mut framed = vec![];
        for (r, ids) in regions.iter().zip([vec![1, 2], vec![3]]) {
            let inside = Rect { x: r.x + 1, y: r.y + 1, width: r.width - 2, height: r.height - 2 };
            panes.extend(stack_rects(inside, &ids));
            framed.push((*r, true));
        }
        panes.push((9, Rect::new(150, 0, 50, 44))); // the orchestrator's column
        // 2 is the last pane in the first frame: its right edge is the frame's
        let g = lock(area, &panes, &framed, &[held(2, 0, true, 80)]).unwrap();
        let outer: Vec<(PaneId, Rect)> = g.frames.iter().map(|&(t, r)| (100 + t as PaneId, r)).chain([(9, find(&g, 9))]).collect();
        tiles(area, &outer);
        for (tag, ids) in [(0, vec![1, 2]), (1, vec![3])] {
            let f = frame_of(&g, tag);
            let inside = Rect { x: f.x + 1, y: f.y + 1, width: f.width - 2, height: f.height - 2 };
            tiles(inside, &g.panes.iter().filter(|(p, _)| ids.contains(p)).copied().collect::<Vec<_>>());
        }
        assert_eq!(find(&g, 2).right(), 80);
        assert_eq!((frame_of(&g, 0).right(), frame_of(&g, 1).x, frame_of(&g, 1).right()), (81, 81, 150));
        assert_eq!(find(&g, 3).x, 82);
        assert_eq!(find(&g, 9).x, 150, "the column stays");
        // the column's own edge
        let g = lock(area, &panes, &framed, &[held(9, 0, false, 130)]).unwrap();
        assert_eq!((find(&g, 9).x, find(&g, 9).width), (130, 70));
        assert_eq!(frame_of(&g, 1).right(), 130);
    }

    fn frame_of(g: &Grown, tag: usize) -> Rect {
        g.frames.iter().find(|(t, _)| *t == tag).unwrap().1
    }

    #[test]
    fn held_edges_never_break_the_tiling() {
        for w in [0, 1, 5, 17, 33, 40, 90, 200] {
            for h in [0, 1, 3, 6, 11, 14, 50] {
                let area = Rect::new(3, 1, w, h);
                for n in 1..=7 {
                    let ids: Vec<PaneId> = (1..=n).collect();
                    let base = stack_rects(area, &ids);
                    for &id in &ids {
                        for ax in [0, 1] {
                            for far in [false, true] {
                                for at in [0, 1, w / 3, h / 2, w, u16::MAX] {
                                    let locks = [held(id, ax, far, at), held(1, 1 - ax, !far, at / 2)];
                                    if let Some(g) = lock(area, &base, &[], &locks) {
                                        tiles(area, &g.panes);
                                        same_order(&base, &g.panes);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn share_is_proportional_and_exact() {
        // same distance: the same fraction of spare room
        let out = share(&[50, 100], &[20, 70], &[1, 1], 45);
        assert_eq!(out, [40, 65]);
        assert_eq!(share(&[10, 10], &[3, 3], &[1, 2], 6), [7, 7], "everything when asked for everything");
        let out = reduce(&[40, 40], &[32, 32], &[10, 10], &[1, 2], 20, true);
        assert_eq!(out, [10, 40], "fold the nearest; it frees 30");
        let out = reduce(&[40, 40], &[32, 32], &[10, 10], &[1, 2], 20, false);
        assert_eq!(out, [32, 32], "without folding: only the spare room");
    }
}
