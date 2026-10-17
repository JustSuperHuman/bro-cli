//! Mouse: sidebar rows and footer blocks, the tab bar, pane close buttons, divider dragging, text selection
//! (copied from the rendered buffer on release) and passthrough to programs that track the mouse.

use super::overlays::Overlay;
use super::{App, Sel, SideHit};
use crate::layout::{Dir, split_rect};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

impl App {
    pub(crate) fn mouse(&mut self, m: MouseEvent) {
        if !matches!(m.kind, MouseEventKind::Moved) {
            self.clipboard_request = None;
        }
        let pos = Position { x: m.column, y: m.row };
        self.hover = pos;
        if matches!(m.kind, MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
            self.link_press = None;
        }
        if self.overlay.is_open() {
            let left = matches!(m.kind, MouseEventKind::Down(MouseButton::Left));
            if matches!(self.overlay, super::overlays::Overlay::Confirm(_)) {
                if left && let Some(&(_, yes)) = self.confirm_hits.iter().find(|(r, _)| r.contains(pos)) {
                    self.confirm_answer(yes);
                }
                return;
            }
            let wheel = matches!(m.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp);
            let down = matches!(m.kind, MouseEventKind::ScrollDown);
            match &mut self.overlay {
                Overlay::File(v) => {
                    let o = v.mouse(m);
                    self.file_view_outcome(o);
                    return;
                }
                Overlay::AgentFiles(v) => {
                    let o = v.mouse(m);
                    self.agent_files_outcome(o);
                    return;
                }
                Overlay::Launcher(l) if left => {
                    match l.click(pos, m.modifiers) {
                        crate::launcher::Outcome::None => self.remember_launcher_permission(),
                        crate::launcher::Outcome::Close => self.overlay = Overlay::None,
                        crate::launcher::Outcome::Profiles => {
                            self.overlay = Overlay::None;
                            self.open_view("profiles");
                        }
                        crate::launcher::Outcome::Project(dir) => self.select_launcher_project(dir),
                        crate::launcher::Outcome::CreateProject(dir, request) => self.create_project_folder(dir, request),
                        crate::launcher::Outcome::Launch(spec, place) => {
                            self.overlay = Overlay::None;
                            self.launch_spec(spec, place);
                        }
                        crate::launcher::Outcome::Resume(idx) => {
                            self.overlay = Overlay::None;
                            self.open_resume(idx);
                        }
                        crate::launcher::Outcome::ResumeMany(idx) => {
                            self.overlay = Overlay::None;
                            self.resume_many(&idx);
                        }
                        crate::launcher::Outcome::Archive(id, on) => self.continue_archive(id, on),
                        crate::launcher::Outcome::UndoArchive => self.undo_archive(),
                        crate::launcher::Outcome::Install(h) => {
                    self.overlay = Overlay::None;
                    self.install_agent(h);
                }
                    }
                    return;
                }
                Overlay::Launcher(l) if wheel => {
                    l.scroll(pos, down);
                    return;
                }
                Overlay::Watchdog(_) if left => {
                    self.watchdog_click(pos);
                    return;
                }
                Overlay::Resume(p) if left => {
                    if let Some(i) = p.hits.iter().position(|r| r.contains(pos)) {
                        if i == p.sel {
                            self.resume_chosen();
                        } else {
                            p.sel = i;
                        }
                    }
                    return;
                }
                _ => {}
            }
            // clicks outside a light overlay close it; the launcher and dialogs stay
            if matches!(m.kind, MouseEventKind::Down(_)) && matches!(self.overlay, Overlay::Palette(_) | Overlay::Help(_)) {
                if let Overlay::Palette(p) = &self.overlay {
                    let before = p.theme_before.clone();
                    self.set_theme(&before, false);
                }
                self.overlay = Overlay::None;
            }
            return;
        }
        // Capture the entire gesture before row hits and terminal mouse passthrough, including
        // release over a different pane. Save once on release, not on every movement.
        if self.side_drag {
            match m.kind {
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                    if matches!(m.kind, MouseEventKind::Drag(_)) {
                        self.side_edge_click = None;
                    }
                    let width = pos.x.saturating_sub(self.side_area.x).saturating_add(1);
                    self.sidebar_width = Some(Self::clamp_sidebar_width(width, self.side_area.width + self.body.width));
                    if matches!(m.kind, MouseEventKind::Up(_)) {
                        self.side_drag = false;
                        self.save_sidebar_width();
                    }
                }
                _ => {}
            }
            return;
        }
        // A tile edge being dragged holds the gesture the same way; it's held where the drag leaves it.
        if let Some((pane, ax, far, offset)) = self.tile_drag {
            match m.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    let body = self.body_panes();
                    let (p, start) = if ax == 0 { (pos.x, body.x) } else { (pos.y, body.y) };
                    let at = (p as i32 + offset - start as i32).clamp(0, u16::MAX as i32) as u16;
                    self.locks.retain(|l| (l.pane, l.ax, l.far) != (pane, ax, far));
                    self.locks.push(crate::priority::Lock { pane, ax, far, at });
                    self.last_pane_click = None;
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.tile_drag = None;
                    return;
                }
                MouseEventKind::Down(_) => self.tile_drag = None,
                _ => {}
            }
        }
        let on_edge = self.side_area.width > 0 && pos.x == self.side_area.right() - 1
            && pos.y >= self.side_area.y && pos.y < self.side_area.bottom();
        if on_edge && matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.sel = None;
            self.drag = None;
            let double = self.side_edge_click.take().is_some_and(|at| at.elapsed() < Duration::from_millis(400));
            if double {
                self.sidebar_width = None;
                self.save_sidebar_width();
            } else {
                self.side_edge_click = Some(Instant::now());
                self.side_drag = true;
            }
            return;
        }
        // sidebar
        if self.side_area.width > 0 && self.side_area.contains(pos)
            && matches!(m.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp)
        {
            self.side_manual_scroll = true;
            self.side_scroll = if matches!(m.kind, MouseEventKind::ScrollDown) {
                self.side_scroll.saturating_add(1)
            } else {
                self.side_scroll.saturating_sub(1)
            };
            self.side_nav();
            return;
        }
        if let Some((r, hit)) = self.side_hits.iter().find(|(r, _)| r.contains(pos)).cloned() {
            let _ = r;
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let double = self.last_click.is_some_and(|(t, x, y)| t.elapsed() < Duration::from_millis(400) && x == m.column && y == m.row);
                    self.last_click = Some((Instant::now(), m.column, m.row));
                    match hit {
                        SideHit::Row(i) if m.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) => {
                            // ctrl+click (terminals keep shift+click for their own selection) or shift+click: show it alongside
                            if let Some(crate::sidebar::Row::Live { info, .. }) = self.rows().get(i) {
                                self.toggle_stack(info.pane);
                            }
                        }
                        SideHit::Row(i) => {
                            let _ = double;
                            self.activate_row(i);
                        }
                        SideHit::Close(id) => self.ask_close(id),
                        SideHit::CloseProject(root, live) => self.close_project(root, live),
                        SideHit::Fable => self.show_fable = !self.show_fable,
                        SideHit::Usage => self.open_view("usage"),
                        SideHit::LaunchUsage(h, profile) => self.launch_usage(h, profile),
                        SideHit::UsageToggle => self.toggle_usage_details(),
                        SideHit::Stats => self.open_view("stats"),
                        SideHit::Proxy => self.open_view("proxy"),
                        SideHit::Bridge => self.open_view("bridge"),
                        SideHit::Settings => self.open_view("settings"),
                        SideHit::Quick(h) => self.quick_launch(h),
                        SideHit::Launcher => self.open_launcher(None, crate::pane::Place::Tab),
                        SideHit::Continue => self.open_continue(),
                        SideHit::TileAll => self.run_act(crate::keymap::Act::TileAll),
                        SideHit::Minimized => self.toggle_minimized_visibility(),
                    }
                }
                MouseEventKind::Down(MouseButton::Middle) => {
                    if let SideHit::Row(i) = hit
                        && let Some(crate::sidebar::Row::Live { info, .. }) = self.rows().get(i) {
                            self.ask_close(info.pane);
                        }
                }
                _ => {}
            }
            return;
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            if let Some(&(_, id)) = self.pane_minimize.iter().find(|(r, _)| r.contains(pos)) {
                self.minimize_pane(id);
                return;
            }
            if let Some(&(_, id)) = self.pane_close.iter().find(|(r, _)| r.contains(pos)) {
                self.ask_close(id);
                return;
            }
            // a split divider: start dragging
            if let Some(t) = self.tabs.get(self.cur).filter(|t| !t.zoom && !self.stacked() && !self.grown) {
                let mut out = vec![];
                t.root.borders(self.body_panes(), &mut vec![], &mut out);
                for (area, dir, path) in out.into_iter().rev() {
                    let Some(ratio) = t.root.ratio_at(&path) else { continue };
                    let (a, _) = split_rect(area, dir, ratio);
                    let hit = match dir {
                        Dir::Right => (m.column == a.right() || m.column + 1 == a.right()) && m.row >= area.y && m.row < area.bottom(),
                        Dir::Down => (m.row == a.bottom() || m.row + 1 == a.bottom()) && m.column >= area.x && m.column < area.right(),
                    };
                    if hit {
                        self.drag = Some((path, dir, area));
                        return;
                    }
                }
            }
            // A tile edge: dragging it holds it there. A plain click still focuses the pane and a double-click
            // still zooms it, except on a held edge, where a double-click lets the edge go.
            if m.modifiers.is_empty()
                && let Some((id, ax, far)) = self.tile_edge(pos)
            {
                let pressed = crate::priority::Lock { pane: id, ax, far, at: 0 };
                let held = self.held_cut(pressed);
                let double = self.last_pane_click.is_some_and(|(at, pane, first)| pane == id && first == pos && at.elapsed() < Duration::from_millis(400));
                if double && held.is_some() {
                    self.locks = self.locks.iter().copied().filter(|l| !self.same_cut(*l, pressed)).collect();
                    self.last_pane_click = None;
                    return;
                }
                // one lock per cut: keep the one already there, so the edge stays tied to its pane
                let anchor = held.unwrap_or(pressed);
                self.locks = self.locks.iter().copied().filter(|l| *l == anchor || !self.same_cut(*l, pressed)).collect();
                if let Some(&(_, r)) = self.outer.iter().find(|(p, _)| *p == anchor.pane) {
                    let edge = match (ax, anchor.far) {
                        (0, true) => r.right(),
                        (0, false) => r.x,
                        (_, true) => r.bottom(),
                        (_, false) => r.y,
                    };
                    let grab = if ax == 0 { pos.x } else { pos.y };
                    self.tile_drag = Some((anchor.pane, ax, anchor.far, edge as i32 - grab as i32));
                }
            }
        }
        if let (Some((path, dir, area)), MouseEventKind::Drag(MouseButton::Left)) = (&self.drag, m.kind) {
            let (path, dir, area) = (path.clone(), *dir, *area);
            let r = match dir {
                Dir::Right => (m.column.saturating_sub(area.x) as f32 + 0.5) / area.width.max(1) as f32,
                Dir::Down => (m.row.saturating_sub(area.y) as f32 + 0.5) / area.height.max(1) as f32,
            };
            if let Some(crate::layout::Node::Split { ratio, .. }) = self.tabs.get_mut(self.cur).and_then(|t| t.root.node_at(&path)) {
                *ratio = r.clamp(0.1, 0.9);
            }
            return;
        }
        if let MouseEventKind::Up(_) = m.kind
            && self.drag.take().is_some() {
                return;
            }
        // dragging out a text selection
        if let (Some(sel), MouseEventKind::Drag(MouseButton::Left)) = (&mut self.sel, m.kind) {
            let r = sel.area;
            let b = Position { x: pos.x.clamp(r.x, r.right().saturating_sub(1)), y: pos.y.clamp(r.y, r.bottom().saturating_sub(1)) };
            if b != sel.a || sel.active {
                sel.active = true;
                sel.b = b;
                return;
            }
        }
        if let MouseEventKind::Up(MouseButton::Left) = m.kind {
            if self.sel.is_some_and(|s| s.active) {
                self.copy_pending = true; // copied right after the next draw, from what's on screen
                return;
            }
            self.sel = None;
            if let Some((id, pressed, uri)) = self.link_press.take() {
                if pressed == pos && self.inner.iter().any(|(pane, area)| *pane == id && area.contains(pos)) {
                    if uri.starts_with("file:") {
                        self.open_file_link(id, &uri);
                    } else {
                        let _ = self.tx.send(crate::pane::Event::OpenLink(uri));
                    }
                }
                return;
            }
        }
        // a link or file on a turned-over card: open it (a dev server in the browser, a file in the preview)
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && !m.modifiers.contains(KeyModifiers::ALT)
            && let Some((_, id, uri)) = self.card_links.iter().find(|(r, ..)| r.contains(pos)).cloned()
        {
            self.sel = None;
            self.last_pane_click = None;
            if uri.starts_with("file:") {
                self.open_file_link(id, &uri);
            } else {
                let _ = self.tx.send(crate::pane::Event::OpenLink(uri));
            }
            return;
        }
        // the pane under the cursor
        let Some(&(id, _)) = self.outer.iter().find(|(_, r)| r.contains(pos)) else { return };
        let inner = self.inner.iter().find(|(i, _)| *i == id).map(|x| x.1).unwrap_or_default();
        // a card turned over shows bro's own text: the program under it gets no mouse
        let flipped = self.flips.contains_key(&id);
        let alt = m.modifiers.contains(KeyModifiers::ALT);
        let wants = !flipped && self.panes.get(&id).is_some_and(|p| p.wants_mouse());
        let link = if matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && !flipped
            && inner.contains(pos) && !m.modifiers.contains(KeyModifiers::SHIFT)
            && (!wants || m.modifiers.contains(KeyModifiers::CONTROL)) {
                self.panes.get(&id).and_then(|p| p.link_at(pos.y - inner.y, pos.x - inner.x))
            } else { None };
        // Ctrl+click a session (anywhere but on a link, which ctrl+click still opens): it grows to a working
        // size, and the same again snaps it back.
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && link.is_none()
            && m.modifiers.contains(KeyModifiers::CONTROL)
            && !m.modifiers.contains(KeyModifiers::SHIFT)
        {
            self.toggle_priority(id);
            return;
        }
        // A click on an option of the prompt an agent is waiting on answers it (the key, then Enter, as the
        // phone's prompt buttons do).
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && link.is_none()
            && !flipped
            && !alt
            && inner.contains(pos)
            && (!wants || m.modifiers.contains(KeyModifiers::SHIFT))
            && let Some(key) = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.prompt_option_at(pos.y - inner.y))
        {
            if self.stacked() {
                self.stack_focus = Some(id);
            } else if let Some(t) = self.tabs.get_mut(self.cur) {
                t.focus = id;
            }
            self.side_focus = false;
            self.sel = None;
            self.last_pane_click = None;
            if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                t.send(format!("{key}\r").as_bytes());
            }
            return;
        }
        // Double-click a pane: full screen, and again back to the tiles or splits. On the frame always; inside
        // the terminal unless the program there uses the mouse (shift takes it back, as for selecting).
        // Alt+double-click turns the card over instead (anywhere on it; alt clicks are always bro's).
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && link.is_none()
        {
            let ours = alt || !inner.contains(pos) || !wants || m.modifiers.contains(KeyModifiers::SHIFT);
            let double = ours
                && self.last_pane_click.is_some_and(|(at, pane, first)| {
                    pane == id && at.elapsed() < Duration::from_millis(400) && first.x.abs_diff(pos.x) <= 1 && first.y.abs_diff(pos.y) <= 1
                });
            self.last_pane_click = (ours && !double).then(|| (Instant::now(), id, pos));
            if double {
                self.sel = None;
                if alt {
                    self.flip_pane(id);
                } else {
                    self.zoom_pane(id);
                }
                return;
            }
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            // programs that use the mouse keep it unless shift is held
            self.sel = (inner.contains(pos) && (!wants || link.is_some() || m.modifiers.contains(KeyModifiers::SHIFT))).then_some(Sel { pane: id, area: inner, a: pos, b: pos, active: false });
        }
        if let MouseEventKind::Down(_) = m.kind {
            if self.stacked() {
                self.stack_focus = Some(id);
            } else if let Some(t) = self.tabs.get_mut(self.cur) {
                t.focus = id;
            }
            self.side_focus = false;
        }
        if let Some(uri) = link {
            self.link_press = Some((id, pos, uri));
            return;
        }
        if inner.contains(pos) && !flipped && !alt && !(m.modifiers.contains(KeyModifiers::SHIFT) && wants) {
            self.with_pane(id, |p, cx| p.mouse(m, inner, cx));
        }
    }

    /// Focus `id` and turn its card over: the back says what it's working on; again turns it face up.
    pub(crate) fn flip_pane(&mut self, id: crate::pane::PaneId) {
        if !self.panes.get(&id).is_some_and(|p| p.as_term_ref().is_some()) {
            return;
        }
        self.side_focus = false;
        if self.stacked() {
            self.stack_focus = Some(id);
        } else if let Some(t) = self.tabs.get_mut(self.cur) {
            t.focus = id;
        }
        self.turn_card(id, Duration::ZERO);
    }

    /// Turn every session on screen over; when they all already are, turn them all face up. They go one after
    /// another, left to right and top to bottom, a beat apart.
    pub(crate) fn flip_all(&mut self) {
        let back = self.cards_on_screen().iter().any(|id| !self.flips.get(id).is_some_and(|f| f.back));
        self.turn_all(back);
    }

    /// The sessions on screen, left to right and top to bottom.
    fn cards_on_screen(&self) -> Vec<crate::pane::PaneId> {
        let mut ids: Vec<(crate::pane::PaneId, ratatui::layout::Rect)> = self
            .outer
            .iter()
            .filter(|(id, _)| self.panes.get(id).is_some_and(|p| p.as_term_ref().is_some()))
            .copied()
            .collect();
        ids.sort_by_key(|(_, r)| (r.y, r.x));
        ids.into_iter().map(|(id, _)| id).collect()
    }

    /// Turn every session on screen to its back (`back`) or face up, one after another; returns the ones that
    /// turned.
    pub(super) fn turn_all(&mut self, back: bool) -> Vec<crate::pane::PaneId> {
        let mut delay = Duration::ZERO;
        let mut turned = vec![];
        for id in self.cards_on_screen() {
            if self.flips.get(&id).is_some_and(|f| f.back) != back {
                self.turn_card(id, delay);
                delay += crate::card::CASCADE;
                turned.push(id);
            }
        }
        turned
    }

    /// Start `id`'s card turning over, `delay` from now.
    pub(super) fn turn_card(&mut self, id: crate::pane::PaneId, delay: Duration) {
        let now = Instant::now();
        let mut flip = crate::card::Flip::toggled(self.flips.get(&id).copied(), now);
        flip.at += delay;
        self.flips.insert(id, flip);
    }

    /// Grow `id` to a working size (focusing it), or snap it back if it already is the one grown. Only one
    /// session holds the priority: another ctrl+click moves it.
    pub(crate) fn toggle_priority(&mut self, id: crate::pane::PaneId) {
        self.sel = None;
        self.last_pane_click = None;
        self.side_focus = false;
        if self.priority == Some(id) {
            self.priority = None;
            return;
        }
        self.priority = Some(id);
        if self.stacked() {
            self.stack_focus = Some(id);
        } else if let Some(t) = self.tabs.get_mut(self.cur) {
            t.focus = id;
        }
    }

    /// Focus `id` and toggle full screen for it (the tiled grid or the tab's splits come back unchanged).
    fn zoom_pane(&mut self, id: crate::pane::PaneId) {
        self.side_focus = false;
        if self.stacked() {
            self.stack_focus = Some(id);
            self.stack_zoom = !self.stack_zoom;
        } else if let Some(t) = self.tabs.get_mut(self.cur) {
            t.focus = id;
            t.zoom = !t.zoom;
        }
    }

    /// The tile edge at `pos` in the tiled view, when another tile is across it: (pane, axis, far side).
    pub(super) fn tile_edge(&self, pos: Position) -> Option<(crate::pane::PaneId, usize, bool)> {
        if !self.stacked() || self.stack_zoom || self.grown {
            return None;
        }
        let &(id, r) = self.outer.iter().find(|(_, r)| r.contains(pos))?;
        // with shared borders a pane is drawn onto the one touching it on the right / below: its own last column /
        // row is content, and the line between them is the neighbour's edge
        let shared = self.shared_borders();
        [(0, true, pos.x + 1 == r.right()), (0, false, pos.x == r.x), (1, true, pos.y + 1 == r.bottom()), (1, false, pos.y == r.y)]
            .into_iter()
            .find(|&(ax, far, on)| {
                let mut across = self.across(id, r, ax, far).filter(|(_, q)| if ax == 0 { q.y <= pos.y && pos.y < q.bottom() } else { q.x <= pos.x && pos.x < q.right() });
                on && across.next().is_some_and(|(_, q)| !(shared && far && if ax == 0 { q.x == r.right() } else { q.y == r.bottom() }))
            })
            .map(|(ax, far, _)| (id, ax, far))
    }

    /// The tiles across `r`'s edge (`ax`, `far` side): touching it, or up to two cells on past project frames.
    pub(super) fn across(&self, pane: crate::pane::PaneId, r: Rect, ax: usize, far: bool) -> impl Iterator<Item = (crate::pane::PaneId, Rect)> + '_ {
        self.outer.iter().copied().filter(move |&(id, q)| {
            let (gap, overlap) = if ax == 0 {
                (if far { q.x as i32 - r.right() as i32 } else { r.x as i32 - q.right() as i32 }, q.y < r.bottom() && r.y < q.bottom())
            } else {
                (if far { q.y as i32 - r.bottom() as i32 } else { r.y as i32 - q.bottom() as i32 }, q.x < r.right() && r.x < q.right())
            };
            id != pane && (0..=2).contains(&gap) && overlap
        })
    }

    /// Whether two tile edges are one divider in the tiled view as last drawn.
    fn same_cut(&self, a: crate::priority::Lock, b: crate::priority::Lock) -> bool {
        crate::priority::same_cut(self.body_panes(), &self.outer, &self.tile_regions, a, b)
    }

    /// The newest lock on the same divider as `edge`.
    pub(super) fn held_cut(&self, edge: crate::priority::Lock) -> Option<crate::priority::Lock> {
        self.locks.iter().rev().find(|l| self.same_cut(**l, edge)).copied()
    }

    pub(super) fn save_sidebar_width(&self) {
        let mut settings = self.svc.settings();
        if settings.sidebar_width != self.sidebar_width {
            settings.sidebar_width = self.sidebar_width;
            self.svc.save_settings(settings);
        }
    }
}
