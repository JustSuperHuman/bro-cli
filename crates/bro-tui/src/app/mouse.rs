//! Mouse: sidebar rows and footer blocks, the tab bar, pane close buttons, divider dragging, text selection
//! (copied from the rendered buffer on release) and passthrough to programs that track the mouse.

use super::overlays::Overlay;
use super::{App, Sel, SideHit};
use crate::layout::{Dir, split_rect};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
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
                Overlay::Launcher(l) if left => {
                    match l.click(pos) {
                        crate::launcher::Outcome::None => {}
                        crate::launcher::Outcome::Close => self.overlay = Overlay::None,
                        crate::launcher::Outcome::Launch(spec, place) => {
                            self.overlay = Overlay::None;
                            self.launch_spec(spec, place);
                        }
                    }
                    return;
                }
                Overlay::Launcher(l) if wheel => {
                    l.scroll(pos, down);
                    return;
                }
                Overlay::Folder(p) if left => {
                    if let crate::folder::Outcome::Open(dir) = p.click(pos) {
                        self.overlay = Overlay::None;
                        self.add_project(dir);
                    }
                    return;
                }
                Overlay::Continue(p) if left => {
                    if let crate::continue_picker::Outcome::Resume(idx) = p.click(pos) {
                        self.overlay = Overlay::None;
                        self.open_resume(idx);
                    }
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
                        SideHit::Proxy => self.open_view("proxy"),
                        SideHit::Bridge => self.open_view("bridge"),
                        SideHit::Quick(h) => self.quick_launch(h),
                        SideHit::Launcher => self.open_launcher(None, crate::pane::Place::Tab),
                        SideHit::TileAll => self.run_act(crate::keymap::Act::TileAll),
                    }
                }
                MouseEventKind::Down(MouseButton::Middle) => {
                    if let SideHit::Row(i) = hit
                        && let Some(crate::sidebar::Row::Live { info, .. }) = self.rows().get(i) {
                            self.ask_close(info.pane);
                        }
                }
                MouseEventKind::ScrollDown => self.side_scroll += 2,
                MouseEventKind::ScrollUp => self.side_scroll = self.side_scroll.saturating_sub(2),
                _ => {}
            }
            return;
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            if let Some(&(_, id)) = self.pane_close.iter().find(|(r, _)| r.contains(pos)) {
                self.ask_close(id);
                return;
            }
            // a split divider: start dragging
            if let Some(t) = self.tabs.get(self.cur).filter(|t| !t.zoom && self.stack.len() < 2) {
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
                    let _ = self.tx.send(crate::pane::Event::OpenLink(uri));
                }
                return;
            }
        }
        // the pane under the cursor
        let Some(&(id, _)) = self.outer.iter().find(|(_, r)| r.contains(pos)) else { return };
        let inner = self.inner.iter().find(|(i, _)| *i == id).map(|x| x.1).unwrap_or_default();
        let wants = self.panes.get(&id).is_some_and(|p| p.wants_mouse());
        let link = if matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && inner.contains(pos) && !m.modifiers.contains(KeyModifiers::SHIFT)
            && (!wants || m.modifiers.contains(KeyModifiers::CONTROL)) {
                self.panes.get(&id).and_then(|p| p.link_at(pos.y - inner.y, pos.x - inner.x))
            } else { None };
        // A click on an option of the prompt an agent is waiting on answers it (the key, then Enter, as the
        // phone's prompt buttons do).
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && link.is_none()
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
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && link.is_none()
        {
            let ours = !inner.contains(pos) || !wants || m.modifiers.contains(KeyModifiers::SHIFT);
            let double = ours
                && self.last_pane_click.is_some_and(|(at, pane, first)| {
                    pane == id && at.elapsed() < Duration::from_millis(400) && first.x.abs_diff(pos.x) <= 1 && first.y.abs_diff(pos.y) <= 1
                });
            self.last_pane_click = (ours && !double).then(|| (Instant::now(), id, pos));
            if double {
                self.sel = None;
                self.zoom_pane(id);
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
        if inner.contains(pos) && !(m.modifiers.contains(KeyModifiers::SHIFT) && wants) {
            self.with_pane(id, |p, cx| p.mouse(m, inner, cx));
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

    pub(super) fn save_sidebar_width(&self) {
        let mut settings = self.svc.settings();
        if settings.sidebar_width != self.sidebar_width {
            settings.sidebar_width = self.sidebar_width;
            self.svc.save_settings(settings);
        }
    }
}
