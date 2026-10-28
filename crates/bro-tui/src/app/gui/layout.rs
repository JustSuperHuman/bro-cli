//! Where everything goes this frame: the sidebar, the project tabs, the media panel and the session cards.
//!
//! Sessions are laid out exactly as in the terminal UI — tabs and splits, the all-sessions grid with its
//! project groups and the orchestrator's column, held tile edges, the priority grow — by running the same
//! algorithms on a virtual grid of terminal-sized cells, then scaling to pixels. The workspace's own geometry
//! (`outer`, `inner`, `body`…) is kept in those virtual cells, so focus moves, split directions, tile-edge drags
//! and every other rule that reads it behave as they do in the terminal.

use super::super::App;
use crate::layout::PaneId;
use bro_gfx::Rect;
use ratatui::layout::Rect as Cells;

/// Gaps and margins (logical px).
pub const MARGIN: f32 = 8.0;
pub const GAP: f32 = 8.0;
pub const HEADER: f32 = 34.0;
pub const STRIP: f32 = 40.0;
/// The row the window's buttons sit in when there are no project tabs to share it.
pub const CAPTION_ROW: f32 = 32.0;
/// Below this window width the sidebar hides (the terminal UI's 72 columns).
pub const SIDEBAR_MIN_WINDOW: f32 = 640.0;

/// A session (or view) card.
#[derive(Clone, Debug)]
pub struct PaneBox {
    pub id: PaneId,
    /// the whole card
    pub card: Rect,
    pub header: Rect,
    /// the terminal or view inside
    pub content: Rect,
    pub cells: Cells,
    pub focused: bool,
    /// folded out of the way of a grown session: header only
    pub folded: bool,
    /// inside a group frame (the frame names the project)
    pub grouped: bool,
    /// its sidebar number inside a group (1–9)
    pub number: Option<usize>,
    /// the sides it shares one line with the tile beside it (the shared-borders setting): no gutter there, and
    /// square corners
    pub joined: Joined,
}

/// Which sides of a tile meet another tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Joined {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Joined {
    pub fn any(self) -> bool {
        self.left || self.top || self.right || self.bottom
    }
    /// The card's corner radii: square where it meets a neighbour, `r` elsewhere.
    pub fn corners(self, r: f32) -> bro_gfx::Corners {
        let c = |a: bool, b: bool| if a || b { 0.0 } else { r };
        bro_gfx::Corners { tl: c(self.left, self.top), tr: c(self.right, self.top), br: c(self.right, self.bottom), bl: c(self.left, self.bottom) }
    }
}

impl PaneBox {
    /// Where a tile edge it shares with the tile to its right or below is grabbed (the line between them, no
    /// gutter to grab instead): (strip, vertical).
    pub fn edge_strips(&self) -> Vec<(Rect, bool)> {
        let c = self.card;
        let mut v = vec![];
        if self.joined.right {
            v.push((Rect::new(c.right() - 4.0, c.y + 8.0, 6.0, (c.h - 16.0).max(0.0)), true));
        }
        if self.joined.bottom {
            v.push((Rect::new(c.x + 8.0, c.bottom() - 4.0, (c.w - 16.0).max(0.0), 6.0), false));
        }
        v
    }
}

/// Which sides of `r` another of `all` touches (as laid out, in cells).
pub fn joined(r: Cells, all: &[Cells]) -> Joined {
    let across_x = |q: &&Cells| q.y < r.bottom() && r.y < q.bottom();
    let across_y = |q: &&Cells| q.x < r.right() && r.x < q.right();
    Joined {
        left: all.iter().filter(across_x).any(|q| q.right() == r.x),
        top: all.iter().filter(across_y).any(|q| q.bottom() == r.y),
        right: all.iter().filter(across_x).any(|q| q.x == r.right()),
        bottom: all.iter().filter(across_y).any(|q| q.y == r.bottom()),
    }
}

/// A frame around tiles that share a project, agent, login, model and folder.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct GroupBox {
    pub rect: Rect,
    pub key: String,
    pub label: String,
    pub count: usize,
    pub focused: bool,
    /// one of its sessions (they share project and agent): what the label names
    pub pane: Option<PaneId>,
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct Layout {
    pub window: Rect,
    pub sidebar: Rect,
    pub strip: Rect,
    pub media: Rect,
    /// where the cards go (inside the margins)
    pub body: Rect,
    /// one virtual cell, logical px
    pub cell: (f32, f32),
    pub panes: Vec<PaneBox>,
    pub groups: Vec<GroupBox>,
    pub zoomed: bool,
    pub grown: bool,
    /// all sessions are minimized: say so where the grid would be
    pub all_minimized: bool,
}

impl Layout {
    pub fn pane(&self, id: PaneId) -> Option<&PaneBox> {
        self.panes.iter().find(|p| p.id == id)
    }
    /// Pixels → the virtual cell grid the workspace's geometry is kept in.
    pub fn to_cells(&self, x: f32, y: f32) -> ratatui::layout::Position {
        let cx = ((x - self.body.x) / self.cell.0).floor().max(0.0) as u16;
        let cy = ((y - self.body.y) / self.cell.1).floor().max(0.0) as u16;
        ratatui::layout::Position { x: cx, y: cy }
    }
    /// Virtual cells → pixels.
    pub fn to_px(&self, r: Cells) -> Rect {
        let x0 = self.body.x + r.x as f32 * self.cell.0;
        let y0 = self.body.y + r.y as f32 * self.cell.1;
        let x1 = self.body.x + r.right() as f32 * self.cell.0;
        let y1 = self.body.y + r.bottom() as f32 * self.cell.1;
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// The areas around the body: (sidebar, strip, media, body, caption). `sidebar_w` and `media_w` are 0 when hidden.
/// `buttons` > 0 is a window with no title bar: a row across the top right of the sidebar ends in that much room
/// for the window's buttons (`caption`, over the media panel too), holding the project tabs when they're on or
/// standing empty, [`CAPTION_ROW`] tall, when they're not.
pub fn frame(window: Rect, sidebar_w: f32, media_w: f32, strip: bool, buttons: f32) -> (Rect, Rect, Rect, Rect, Rect) {
    let side_w = if window.w >= SIDEBAR_MIN_WINDOW { sidebar_w.min(window.w * 0.5) } else { 0.0 };
    let (sidebar, rest) = window.split_left(side_w);
    let (row, rest) = if buttons > 0.0 { rest.split_top(if strip { STRIP } else { CAPTION_ROW }) } else { (Rect { h: 0.0, ..rest }, rest) };
    let (row, caption) = row.split_right(buttons.min(row.w));
    // the sessions keep at least 360 px; a panel squeezed under 240 px hides instead
    let media_w = media_w.min((rest.w - 360.0).max(0.0));
    let media_w = if media_w < 240.0 { 0.0 } else { media_w };
    let (main, media) = rest.split_right(media_w);
    let (strip, body) = match (strip, buttons > 0.0) {
        (true, true) => (row, main),
        (true, false) => main.split_top(STRIP),
        (false, _) => (Rect { h: 0.0, ..main }, main),
    };
    let body = Rect::new(body.x + MARGIN, body.y + if strip.h > 0.0 { 0.0 } else { MARGIN }, (body.w - 2.0 * MARGIN).max(0.0), (body.h - if strip.h > 0.0 { MARGIN } else { 2.0 * MARGIN }).max(0.0));
    (sidebar, strip, media, body, caption)
}

/// Lay the sessions out in `body` (logical px) with virtual cells of `cell` px, writing the workspace's
/// geometry back into `app` as the terminal UI's draw pass does.
pub fn panes(app: &mut App, body: Rect, cell: (f32, f32), overlay_open: bool) -> (Vec<PaneBox>, Vec<GroupBox>, bool, bool, bool) {
    let vcols = (body.w / cell.0).floor().max(1.0) as u16;
    let vrows = (body.h / cell.1).floor().max(1.0) as u16;
    // spread the virtual grid over the whole body
    let cell = (body.w / vcols as f32, body.h / vrows as f32);
    let vbody = Cells { x: 0, y: 0, width: vcols, height: vrows };
    app.body = vbody;
    // project tabs that aren't on screen can't hold the keys (as the terminal UI's draw pass has it)
    if app.strip.height == 0 {
        app.strip_sel = None;
    }
    let geo = Layout { body, cell, ..Default::default() };
    if app.tabs.is_empty() {
        app.outer.clear();
        app.inner.clear();
        app.tile_regions.clear();
        app.grown = false;
        return (vec![], vec![], false, false, false);
    }

    // ---- the terminal UI's layout pass (app/draw.rs `draw_panes`), on virtual cells
    let mut rects: Vec<(PaneId, Cells)> = vec![];
    let mut groups: Vec<(Cells, String, String, usize, Option<PaneId>)> = vec![];
    let mut regions: Vec<(Cells, bool)> = vec![];
    let mut group_tags: Vec<usize> = vec![];
    let mut column = app.orchestrator_column(vbody);
    let mut tile_body = match column {
        Some((_, c)) => Cells { width: vbody.width - c.width, ..vbody },
        None => vbody,
    };
    let tiled: Vec<PaneId> = app.stack.iter().copied().filter(|id| column.is_none_or(|(c, _)| c != *id)).collect();
    let (focus, zoomed) = if app.stacked() {
        let focus = app.focused().unwrap_or(0);
        let by_project = if app.stack_zoom { vec![] } else { app.tile_groups() };
        rects = if app.stack_zoom {
            if focus == 0 { vec![] } else { vec![(focus, tile_body)] }
        } else if by_project.len() >= 2 || by_project.first().is_some_and(|(_, _, ids)| ids.len() >= 2) {
            let floor = tiled.len().div_ceil(4);
            let weights: Vec<usize> = by_project.iter().map(|(_, _, ids)| ids.len().max(floor)).collect();
            let mut out = vec![];
            for ((key, name, ids), region) in by_project.into_iter().zip(crate::layout::group_regions(tile_body, &weights)) {
                let framed = ids.len() >= 2 && region.width >= 14 && region.height >= 6;
                let inside = if framed { Cells { x: region.x + 1, y: region.y + 1, width: region.width - 2, height: region.height - 2 } } else { region };
                out.extend(crate::layout::stack_rects(inside, &ids));
                if framed {
                    group_tags.push(regions.len());
                    groups.push((region, key, name, ids.len(), ids.first().copied()));
                }
                regions.push((region, framed));
            }
            out
        } else {
            crate::layout::stack_rects(tile_body, &tiled)
        };
        (focus, app.stack_zoom)
    } else {
        let tab = &app.tabs[app.cur.min(app.tabs.len() - 1)];
        if tab.zoom {
            rects.push((tab.focus, vbody));
        } else {
            tab.root.rects(vbody, &mut rects);
        }
        (tab.focus, tab.zoom)
    };
    if app.stacked() && !zoomed && !app.locks.is_empty() {
        let all: Vec<_> = rects.iter().copied().chain(column).collect();
        if let Some(held) = crate::priority::lock(vbody, &all, &regions, &app.locks) {
            let at = |id| held.panes.iter().find(|(p, _)| *p == id).map(|x| x.1).unwrap_or_default();
            for (i, (region, framed)) in regions.iter_mut().enumerate() {
                *region = if *framed {
                    held.frames.iter().find(|(tag, _)| *tag == i).map_or(*region, |x| x.1)
                } else {
                    rects.iter().filter(|(_, r)| region.contains(r.as_position())).fold(Cells::default(), |u, (id, _)| if u.is_empty() { at(*id) } else { u.union(at(*id)) })
                };
            }
            for (i, &tag) in group_tags.iter().enumerate() {
                groups[i].0 = regions[tag].0;
            }
            column = column.map(|(id, _)| (id, at(id)));
            if let Some((_, c)) = column {
                tile_body.width = c.x - tile_body.x;
            }
            rects = rects.into_iter().map(|(id, _)| (id, at(id))).collect();
        }
    }
    app.tile_regions = regions.clone();
    let mut folded = vec![];
    let grown = app.priority.filter(|_| !zoomed).and_then(|id| crate::priority::prioritize(tile_body, &rects, &regions, id, crate::priority::want(tile_body)));
    app.grown = grown.is_some();
    if let Some(g) = grown {
        for (tag, r) in g.frames {
            if let Some(i) = group_tags.iter().position(|t| *t == tag) {
                groups[i].0 = r;
            }
        }
        rects = g.panes;
        folded = g.folded;
    }
    rects.extend(column);
    app.outer = rects.clone();
    let all_minimized = app.tile_all && rects.is_empty() && !app.minimized.is_empty();

    // ---- pixels
    let side_focus = app.side_focus;
    let numbers: Vec<PaneId> = if groups.is_empty() { vec![] } else { crate::sidebar::live_order(&app.live_infos(), &app.past_infos(), &app.open_infos()) };
    let mut boxes = vec![];
    let mut inner_cells = vec![];
    let group_boxes: Vec<GroupBox> = groups
        .iter()
        .map(|(region, key, name, count, pane)| {
            let holds_focus = rects.iter().any(|(id, r)| *id == focus && region.contains(r.as_position())) && !side_focus && !overlay_open;
            GroupBox { rect: inset(geo.to_px(*region), GAP / 2.0), key: key.clone(), label: name.clone(), count: *count, focused: holds_focus, pane: *pane }
        })
        .collect();
    // Shared borders (a setting): tiles that sit together meet on one line, no gutter between them (the terminal
    // UI draws each tile one cell onto its neighbours); the layout in cells stays as it is.
    let shared = app.shared_borders() && !zoomed;
    let laid: Vec<Cells> = rects.iter().map(|(_, r)| *r).collect();
    for (id, r) in &rects {
        let grouped = groups.iter().any(|(region, ..)| region.contains(r.as_position()));
        let joined = if shared { joined(*r, &laid) } else { Joined::default() };
        // a gutter on each free side (more inside a group frame, for its label); a shared side reaches 1px onto the
        // neighbour so the two edges are one line
        let free = GAP / 2.0 + if grouped { 2.0 } else { 0.0 };
        let px = geo.to_px(*r);
        let x0 = px.x + if joined.left { 0.0 } else { free };
        let y0 = px.y + if joined.top { 0.0 } else { free };
        let x1 = px.right() + if joined.right { 1.0 } else { -free };
        let y1 = px.bottom() + if joined.bottom { 1.0 } else { -free };
        let card = Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
        let is_folded = folded.contains(id);
        let header = Rect { h: HEADER.min(card.h), ..card };
        let content = Rect::new(card.x + 1.0, header.bottom(), (card.w - 2.0).max(0.0), (card.h - HEADER - 1.0).max(0.0));
        let focused = *id == focus && !side_focus && !overlay_open;
        let number = numbers.iter().position(|n| n == id).map(|i| i + 1).filter(|n| *n <= 9);
        boxes.push(PaneBox { id: *id, card, header, content, cells: *r, focused, folded: is_folded, grouped, number, joined });
        if !is_folded {
            // the content area in virtual cells (links, prompt rows and selection use pixels; this keeps the
            // workspace's own bookkeeping consistent)
            let inner = Cells { x: r.x, y: r.y.saturating_add(1).min(r.bottom().saturating_sub(1)), width: r.width, height: r.height.saturating_sub(1).max(1) };
            inner_cells.push((*id, inner));
        }
    }
    app.inner = inner_cells;
    (boxes, group_boxes, zoomed, app.grown, all_minimized)
}

/// Shrink by `v` on every side (never below zero).
pub fn inset(r: Rect, v: f32) -> Rect {
    Rect::new(r.x + v, r.y + v, (r.w - 2.0 * v).max(0.0), (r.h - 2.0 * v).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_that_touch_share_a_side() {
        let a = Cells { x: 0, y: 0, width: 10, height: 10 };
        let b = Cells { x: 10, y: 0, width: 10, height: 5 };
        let c = Cells { x: 10, y: 5, width: 10, height: 5 };
        let all = [a, b, c];
        assert_eq!(joined(a, &all), Joined { right: true, ..Default::default() });
        assert_eq!(joined(b, &all), Joined { left: true, bottom: true, ..Default::default() });
        assert_eq!(joined(c, &all), Joined { left: true, top: true, ..Default::default() });
        // across a gutter (a group frame's ring) nothing is shared
        let d = Cells { x: 21, y: 0, width: 5, height: 10 };
        assert!(!joined(d, &[a, b, c, d]).any());
        // square where it meets a neighbour, rounded elsewhere
        let k = joined(b, &all).corners(8.0);
        assert_eq!((k.tl, k.tr, k.br, k.bl), (0.0, 8.0, 0.0, 0.0));
    }
}
