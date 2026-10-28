//! The mouse: what a press, drag, release or wheel does, by what's under the pointer — the same rules as the
//! terminal UI (sidebar rows and buttons, tab strip, pane headers and buttons, split dividers and tile edges,
//! links, prompt options, the grow and flip gestures, passthrough to programs that track the mouse), plus what
//! a window does better: text selection in the terminal's history, smooth wheel scrolling, dropped files.

use super::actions::{Action, PaneAct};
use super::kit::Cursor;
use super::{Drag, Gui};
use crate::app::{Overlay, SideHit};
use crate::app::project_tabs::{StripHit, StripTab};
use crate::layout::Dir;
use crate::pane::{Event, Pane, PaneId};
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::{Selection, SelectionType};
use bro_gfx::{Point, Rect};
use crossterm::event::{KeyModifiers, MouseButton as CButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};
use winit::event::MouseButton;

const DOUBLE: Duration = Duration::from_millis(400);

fn hit_at(g: &Gui, p: Point) -> Option<Action> {
    g.hits.iter().rev().find(|h| h.rect.contains(p)).map(|h| h.action.clone())
}

fn mods(g: &Gui) -> KeyModifiers {
    super::input::mods(g.mods)
}

fn cbutton(b: MouseButton) -> Option<CButton> {
    match b {
        MouseButton::Left => Some(CButton::Left),
        MouseButton::Right => Some(CButton::Right),
        MouseButton::Middle => Some(CButton::Middle),
        _ => None,
    }
}

/// The terminal UI's mouse event for a cell.
fn cell_event(kind: MouseEventKind, col: u16, row: u16, m: KeyModifiers) -> MouseEvent {
    MouseEvent { kind, column: col, row, modifiers: m }
}

pub fn pressed(g: &mut Gui, p: Point, button: MouseButton) {
    // back / forward buttons walk the history of what you looked at
    match button {
        MouseButton::Back | MouseButton::Forward if g.media.expanded => return,
        MouseButton::Back => return g.app_event(Event::Navigate(true)),
        MouseButton::Forward => return g.app_event(Event::Navigate(false)),
        _ => {}
    }
    let Some(cb) = cbutton(button) else { return };
    // another button while a tile edge is dragged lets it go where it is
    if matches!(g.drag, Some(Drag::TileEdge { .. })) {
        g.drag = None;
    }
    // a click while the summary screensaver shows only wakes it (nothing lands in a session you couldn't see)
    if g.app.touched() {
        return;
    }
    g.app.clipboard_request = None;
    g.link_press = None;
    let hit = hit_at(g, p);
    let m = mods(g);
    if let Some(menu) = g.clipboard_menu.take() {
        if cb == CButton::Left {
            if let Some(Action::Clipboard(copy)) = hit {
                focus(&mut g.app, menu.pane);
                if copy {
                    if let Some(text) = menu.selection {
                        super::window::set_clipboard(&text);
                    }
                } else {
                    g.key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('v'), KeyModifiers::CONTROL));
                }
            }
            return;
        }
    }
    if g.app.prefix_armed && cb == CButton::Right {
        g.app.prefix_armed = false;
        return;
    }
    if g.app.prefix_armed && cb != CButton::Right {
        if cb == CButton::Left {
            g.app.prefix_armed = false;
            if let Some(Action::Menu(index)) = hit {
                if let Some((_, act)) = g.app.keymap.prefix_actions().get(index) {
                    g.app.run_act(*act);
                }
            }
        }
        return;
    }
    if cb == CButton::Right && !g.app.overlay.is_open() && g.app.renaming.is_none()
        && let Some(Action::Pane(id, _)) = &hit
        && g.layout.panes.iter().any(|pane| pane.id == *id && pane.header.contains(p))
    {
        focus(&mut g.app, *id);
        g.drag = None;
        g.media.focus = false;
        clear_selections(g, None);
        g.app.open_menu(Some((p.x, p.y)));
        return;
    }
    // clicks (and double clicks) are counted per spot
    let count = match g.last_press {
        Some((at, q, n)) if button == MouseButton::Left && at.elapsed() < DOUBLE && (q.x - p.x).abs() <= 4.0 && (q.y - p.y).abs() <= 4.0 => n + 1,
        _ => 1,
    };
    if button == MouseButton::Left {
        g.last_press = Some((Instant::now(), p, count));
    }
    if g.media.expanded && !g.app.overlay.is_open() {
        if cb == CButton::Left && let Some(Action::Media(n)) = hit {
            let focus = g.focused_owner();
            let ask = g.media.click(n, p, count, focus.as_ref());
            g.media_ask(ask);
        }
        return;
    }
    // anywhere on the sidebar (its blank spots too) is the sidebar; the project tabs aren't
    let on_side = on_sidebar(g, p);
    let side_hit = matches!(hit, Some(Action::Side(_) | Action::SideFilter | Action::SideList | Action::SideResize | Action::SideScroll | Action::MediaToggle));
    let on_strip = matches!(hit, Some(Action::Side(SideHit::Strip(_))));
    let in_sidebar = (on_side || side_hit) && !on_strip;
    if !matches!(hit, Some(Action::Media(_) | Action::MediaResize)) {
        g.media.focus = false;
    }
    if !in_sidebar {
        g.app.leave_sidebar();
    }
    // a click anywhere else ends a selection
    if cb == CButton::Left {
        clear_selections(g, None);
    }
    // the launcher fills the body: the sidebar and the tabs beside it keep working (not while its project
    // picker is open)
    if g.app.overlay.is_open() && !(beside_launcher(g) && (on_side || side_hit)) {
        match hit {
            Some(Action::Overlay(n)) if g.overlay_native => super::overlays::click(g, n, p, cb, m, count),
            Some(Action::Overlay(_)) => {
                if let Some(grid) = g.overlay_grid
                    && let Some(cell) = grid.cell(p)
                {
                    g.app.mouse(cell_event(MouseEventKind::Down(cb), cell.x, cell.y, m));
                }
            }
            Some(Action::Toast(i)) => dismiss_toast(g, i),
            Some(Action::CloseOverlay) | None => {
                if matches!(g.app.overlay, Overlay::Palette(_) | Overlay::Help(_)) {
                    if let Overlay::Palette(pl) = &g.app.overlay {
                        let before = pl.theme_before.clone();
                        g.app.set_theme(&before, false);
                    }
                    g.app.overlay = Overlay::None;
                }
            }
            _ => {}
        }
        return;
    }
    match hit {
        Some(Action::Toast(i)) => dismiss_toast(g, i),
        Some(Action::MediaToggle) if cb == CButton::Left => {
            g.prefs.media_open = !g.prefs.media_open;
            g.prefs.save(g.app.persist);
        }
        Some(Action::MediaResize) if cb == CButton::Left => {
            g.drag = Some(Drag::Media { start_x: p.x, start_w: g.prefs.media_width });
        }
        Some(Action::Media(n)) if cb == CButton::Left => {
            let focus = g.focused_owner();
            let ask = g.media.click(n, p, count, focus.as_ref());
            g.media_ask(ask);
        }
        Some(Action::Side(h)) => side_click(g, h, cb, m),
        // beside the open launcher the sidebar's blank spots don't take the keyboard (it keeps it)
        Some(Action::SideFilter) if cb == CButton::Left && !g.app.overlay.is_open() => {
            g.app.side_focus = true;
            g.app.side_filtering = true;
        }
        Some(Action::SideList) if cb == CButton::Left && !g.app.overlay.is_open() => {
            g.app.side_focus = true;
            g.sidebar.manual = false;
        }
        // the scrollbar scrolls (a press on its track jumps there); the keyboard stays where it is
        Some(Action::SideScroll) if cb == CButton::Left => {
            g.sidebar.grab_bar(p.y);
            g.app.side_nav();
        }
        Some(Action::SideResize) if cb == CButton::Left => {
            if count >= 2 {
                // back to the automatic width, remembered at once
                g.prefs.sidebar_width = super::prefs::Prefs::default().sidebar_width;
                g.prefs.save(g.app.persist);
                g.drag = None;
                g.sidebar.resizing = false;
            } else {
                g.drag = Some(Drag::Sidebar { start_x: p.x, start_w: g.prefs.sidebar_width });
                g.sidebar.resizing = true;
            }
        }
        Some(Action::Welcome(n)) if cb == CButton::Left => super::welcome::click(&mut g.app, n),
        Some(Action::View(id, n)) if cb == CButton::Left => {
            focus(&mut g.app, id);
            super::views::click(&mut g.app, id, n, p, m, count);
        }
        // a right-click: the menu of what's under it, in the views that have one
        Some(Action::View(id, n)) if cb == CButton::Right => {
            focus(&mut g.app, id);
            super::views::context(&mut g.app, id, n, p);
        }
        Some(Action::Focus(id, n)) if cb == CButton::Left => super::focus::click(g, id, n, p, count),
        Some(Action::Focus(id, _)) => focus(&mut g.app, id),
        Some(Action::Pane(id, act)) => pane_press(g, id, act, p, cb, m, count),
        // a blank spot on the sidebar takes the keyboard there
        _ if on_side => {
            if cb == CButton::Left && !g.app.overlay.is_open() {
                g.app.side_focus = true;
                g.sidebar.manual = false;
            }
        }
        _ => {
            // a gap between cards: a split divider or a tile edge
            if cb == CButton::Left {
                edge_press(g, p, m, count);
            }
        }
    }
}

/// `p` is on the sidebar (as drawn last frame).
fn on_sidebar(g: &Gui, p: Point) -> bool {
    g.layout.sidebar.w > 0.0 && g.layout.sidebar.contains(p)
}

/// The launcher is open and the sidebar and tabs beside it take clicks (not while its project picker is up).
fn beside_launcher(g: &Gui) -> bool {
    matches!(&g.app.overlay, Overlay::Launcher(l) if l.project_picker.is_none())
}

fn dismiss_toast(g: &mut Gui, i: usize) {
    let n = g.app.toasts.items.len();
    if i < n {
        g.app.toasts.items.remove(n - 1 - i);
    }
}

/// A sidebar target: what the terminal UI's sidebar click does.
fn side_click(g: &mut Gui, hit: SideHit, cb: CButton, m: KeyModifiers) {
    let app = &mut g.app;
    if cb == CButton::Middle {
        match hit {
            SideHit::Strip(h) => app.strip_click(h, true),
            SideHit::Row(i) => {
                if let Some(crate::sidebar::Row::Live { info, .. }) = app.rows().get(i) {
                    app.ask_close(info.pane);
                }
            }
            _ => {}
        }
        return;
    }
    if cb != CButton::Left {
        return;
    }
    if launcher_side_click(app, &hit) {
        return;
    }
    if matches!(hit, SideHit::Row(_)) {
        app.side_focus = true;
        g.sidebar.manual = false;
    }
    match hit {
        SideHit::Row(i) if m.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) => {
            if let Some(crate::sidebar::Row::Live { info, .. }) = app.rows().get(i) {
                app.toggle_stack(info.pane);
            }
        }
        SideHit::Row(i) => app.activate_row(i),
        SideHit::Close(id) => app.ask_close(id),
        SideHit::Team(id) => app.show_team(Some(id)),
        SideHit::CloseProject(root, live) => app.close_project(root, live),
        SideHit::Fable => app.show_fable = !app.show_fable,
        SideHit::Usage => app.open_view("usage"),
        SideHit::LaunchUsage(h, profile) => app.launch_usage(h, profile),
        SideHit::UsageToggle => app.toggle_usage_details(),
        SideHit::Stats => app.open_view("stats"),
        SideHit::Sessions => app.show_every_session(),
        SideHit::Proxy => app.open_view("proxy"),
        SideHit::Bridge => app.open_view("bridge"),
        SideHit::Settings => app.open_view("settings"),
        SideHit::Quick(h) => app.quick_launch(h),
        SideHit::Launcher => app.open_launcher(None, crate::pane::Place::Tab),
        SideHit::Continue => app.open_continue(),
        SideHit::TileAll => app.run_act(crate::keymap::Act::TileAll),
        SideHit::Minimized => app.toggle_minimized_visibility(),
        SideHit::Strip(h) => app.strip_click(h, false),
    }
    if app.overlay.is_open() {
        app.leave_sidebar();
    }
}

/// A sidebar click while the launcher is open (the terminal UI's `launcher_side_click`): a project becomes the
/// one it starts in, + New / Continue switch its tab, toggles keep it open, anything else closes it first.
fn launcher_side_click(app: &mut crate::app::App, hit: &SideHit) -> bool {
    if !matches!(app.overlay, Overlay::Launcher(_)) {
        return false;
    }
    match hit {
        SideHit::Row(i) => {
            if let Some(crate::sidebar::Row::Project { root, .. }) = app.rows().get(*i) {
                let root = root.clone();
                app.select_launcher_project(root);
                return true;
            }
            app.overlay = Overlay::None;
            false
        }
        SideHit::Launcher | SideHit::Continue => {
            if let Overlay::Launcher(l) = &mut app.overlay {
                if matches!(hit, SideHit::Continue) {
                    l.show_continue();
                } else {
                    l.tab = crate::launcher::SessionTab::New;
                    l.hits.clear();
                }
            }
            true
        }
        SideHit::Strip(StripHit::Tab(StripTab::Project(key))) => {
            if let Some(root) = app.project_tabs().into_iter().find(|t| t.key == *key).map(|t| t.root) {
                app.select_launcher_project(root);
            }
            true
        }
        SideHit::Fable | SideHit::UsageToggle | SideHit::Minimized | SideHit::Strip(StripHit::Minimized) => false,
        _ => {
            app.overlay = Overlay::None;
            false
        }
    }
}

/// Focus `id` (inside a stack, just move the focus there).
fn focus(app: &mut crate::app::App, id: PaneId) {
    if app.stacked() {
        app.stack_focus = Some(id);
    } else if let Some(t) = app.tabs.get_mut(app.cur) {
        t.focus = id;
    }
    app.leave_sidebar();
}

fn zoom(app: &mut crate::app::App, id: PaneId) {
    app.leave_sidebar();
    if app.stacked() {
        app.stack_focus = Some(id);
        app.stack_zoom = !app.stack_zoom;
    } else if let Some(t) = app.tabs.get_mut(app.cur) {
        t.focus = id;
        t.zoom = !t.zoom;
    }
}

#[allow(clippy::too_many_arguments)]
fn pane_press(g: &mut Gui, id: PaneId, act: PaneAct, p: Point, cb: CButton, m: KeyModifiers, count: u32) {
    let alt = m.contains(KeyModifiers::ALT);
    match act {
        PaneAct::Close if cb == CButton::Left => g.app.ask_close(id),
        PaneAct::Minimize if cb == CButton::Left => g.app.minimize_pane(id),
        PaneAct::Header => {
            if cb == CButton::Middle {
                g.app.ask_close(id);
                return;
            }
            if cb != CButton::Left {
                return;
            }
            if m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::SHIFT) {
                g.app.toggle_priority(id);
                return;
            }
            if count == 2 {
                if alt { g.app.flip_pane(id) } else { zoom(&mut g.app, id) }
                return;
            }
            focus(&mut g.app, id);
        }
        PaneAct::Body => body_press(g, id, p, cb, m, count),
        _ => {}
    }
}

/// A press on a pane's content.
fn body_press(g: &mut Gui, id: PaneId, p: Point, cb: CButton, m: KeyModifiers, count: u32) {
    let Some(pane) = g.app.panes.get(&id) else { return };
    // a view: hand it the click in its own cells
    if pane.as_term_ref().is_none() {
        focus(&mut g.app, id);
        if let Some(grid) = g.painted.views.iter().find(|(v, _)| *v == id).map(|x| x.1)
            && let Some(cell) = grid.cell(p)
        {
            let area = ratatui::layout::Rect::new(0, 0, grid.cols, grid.rows);
            g.app.with_pane(id, |pane, cx| pane.mouse(cell_event(MouseEventKind::Down(cb), cell.x, cell.y, m), area, cx));
        }
        return;
    }
    let flipped = g.app.flips.contains_key(&id);
    let alt = m.contains(KeyModifiers::ALT);
    let shift = m.contains(KeyModifiers::SHIFT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let wants = !flipped && pane.wants_mouse();
    if cb == CButton::Right && !alt && (!wants || shift) {
        let selection = with_screen(g, id, |s| s.term.selection_to_string()).flatten().map(|text| tidy_copy(&text)).filter(|text| !text.is_empty());
        focus(&mut g.app, id);
        g.drag = None;
        g.clipboard_menu = Some(super::menu::ClipboardMenu { pane: id, anchor: p, selection });
        return;
    }
    // links on the back of a turned card open on press
    if flipped {
        if cb == CButton::Left && !alt && let Some((_, _, uri)) = g.painted.card_links.iter().find(|(r, ..)| r.contains(p)).cloned() {
            open_link(g, id, &uri);
            return;
        }
        if cb == CButton::Left && count == 2 && alt {
            g.app.flip_pane(id);
        }
        focus(&mut g.app, id);
        return;
    }
    let st = g.panes.get(&id);
    let drawn = st.map(|s| s.drawn).unwrap_or_default();
    let view_top = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.view_top() as usize).unwrap_or(0);
    let cell = drawn.cell_at(p);
    let link = if cb == CButton::Left && !shift && (!wants || ctrl) {
        cell.and_then(|(r, c)| st.and_then(|s| s.links.iter().find(|l| l.row == r + view_top && (l.start..l.end).contains(&c)).cloned()))
    } else {
        None
    };
    // ctrl+click (not on a link): grow it to a working size, again: back
    if cb == CButton::Left && link.is_none() && ctrl && !shift {
        g.app.toggle_priority(id);
        return;
    }
    // a click on an option of the prompt the agent waits on answers it
    if cb == CButton::Left && link.is_none() && !alt && (!wants || shift)
        && let Some((row, _)) = cell
        && let Some(key) = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.prompt_option_at(row as u16))
    {
        focus(&mut g.app, id);
        if let Some(t) = g.app.panes.get_mut(&id).and_then(|p| p.as_term()) {
            t.send(format!("{key}\r").as_bytes());
        }
        return;
    }
    // alt+double-click turns the card over
    if cb == CButton::Left && alt && count == 2 {
        g.app.flip_pane(id);
        return;
    }
    focus(&mut g.app, id);
    if let Some(l) = link {
        let (r, c) = cell.unwrap_or_default();
        g.link_press = Some((id, l.uri.clone(), (r, c)));
        return;
    }
    let ours = !wants || shift;
    if cb == CButton::Left && ours && !alt {
        // select: a click starts it, a double-click takes the word, a triple the line
        let ty = match count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let (point, right) = drawn.point_at(p);
        let side = if right { Side::Right } else { Side::Left };
        with_screen(g, id, |s| s.term.selection = Some(Selection::new(ty, point, side)));
        g.drag = Some(Drag::Select { pane: id });
        return;
    }
    // the program wants the mouse
    if wants && !alt && !(shift && wants)
        && let Some((r, c)) = cell
    {
        let area = ratatui::layout::Rect::new(0, 0, drawn.cols as u16, drawn.rows as u16);
        g.app.with_pane(id, |pane, cx| pane.mouse(cell_event(MouseEventKind::Down(cb), c as u16, r as u16, m), area, cx));
        g.drag = Some(Drag::Program { pane: id });
    }
}

fn mirror_of(g: &Gui, id: PaneId) -> Option<std::sync::Arc<dyn crate::panes::mirror::ScreenMirror>> {
    g.app.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.mirror().cloned())
}

/// Run `f` on pane `id`'s alacritty screen.
pub fn with_screen<R>(g: &Gui, id: PaneId, f: impl FnOnce(&mut super::term::mirror::Screen) -> R) -> Option<R> {
    let m = mirror_of(g, id)?;
    let mirror = super::term::mirror::Mirror::of(m.as_ref())?;
    let mut s = mirror.lock();
    Some(f(&mut s))
}

/// Clear every terminal's selection except `keep`'s.
pub fn clear_selections(g: &mut Gui, keep: Option<PaneId>) {
    let ids: Vec<PaneId> = g.app.panes.keys().copied().filter(|id| Some(*id) != keep).collect();
    for id in ids {
        with_screen(g, id, |s| s.term.selection = None);
    }
}

/// The tile edge in the gutter at `p` (the workspace keeps its geometry in cells). With shared borders the line
/// between two tiles belongs to the right or lower one, so the gutter's near half reaches across to it.
pub fn tile_edge_at(app: &crate::app::App, layout: &super::layout::Layout, p: Point) -> Option<(PaneId, usize, bool)> {
    let at = |p: Point| app.tile_edge(layout.to_cells(p.x, p.y));
    at(p).or_else(|| {
        if !app.shared_borders() {
            return None;
        }
        let d = super::layout::GAP / 2.0 + 0.5;
        at(Point::new(p.x + d, p.y)).or_else(|| at(Point::new(p.x, p.y + d)))
    })
}

/// A divider between splits, or a tile edge in the grid, under `p` (in the gaps between cards).
fn edge_press(g: &mut Gui, p: Point, m: KeyModifiers, count: u32) {
    // a split divider: start dragging
    if let Some((path, dir, area)) = divider_at(g, p) {
        g.drag = Some(Drag::Divider { path, dir, area });
        return;
    }
    // a tile edge: hold it where the drag leaves it; a double-click on a held edge lets it go. The press is a click
    // on the pane the edge belongs to as well: it takes the focus, and a double-click on an edge that isn't held
    // zooms it as usual.
    if m.is_empty()
        && let Some((id, ax, far)) = tile_edge_at(&g.app, &g.layout, p)
    {
        // the grab is measured from the pointer's own cell, as the drag that follows is
        let cells = g.layout.to_cells(p.x, p.y);
        let pressed = crate::priority::Lock { pane: id, ax, far, at: 0 };
        let held = g.app.held_cut(pressed);
        let same = |a: crate::priority::Lock, b: crate::priority::Lock| crate::priority::same_cut(g.app.body, &g.app.outer, &g.app.tile_regions, a, b);
        if count == 2 && held.is_some() {
            let keep: Vec<_> = g.app.locks.iter().copied().filter(|l| !same(*l, pressed)).collect();
            g.app.locks = keep;
            return;
        }
        if count == 2 {
            zoom(&mut g.app, id);
            return;
        }
        let anchor = held.unwrap_or(pressed);
        let keep: Vec<_> = g.app.locks.iter().copied().filter(|l| *l == anchor || !same(*l, pressed)).collect();
        g.app.locks = keep;
        if let Some(&(_, r)) = g.app.outer.iter().find(|(pid, _)| *pid == anchor.pane) {
            let edge = match (ax, anchor.far) {
                (0, true) => r.right(),
                (0, false) => r.x,
                (_, true) => r.bottom(),
                (_, false) => r.y,
            };
            let grab = if ax == 0 { cells.x } else { cells.y };
            g.drag = Some(Drag::TileEdge { pane: anchor.pane, ax, far: anchor.far, offset: edge as i32 - grab as i32 });
        }
        focus(&mut g.app, id);
    }
}

/// The split divider under `p`: (path, direction, the split's area in cells).
pub fn divider_at(g: &Gui, p: Point) -> Option<(Vec<bool>, Dir, ratatui::layout::Rect)> {
    divider_near(&g.app, &g.layout, p)
}

/// The split divider near `p` in the gap between two cards.
pub fn divider_near(app: &crate::app::App, layout: &super::layout::Layout, p: Point) -> Option<(Vec<bool>, Dir, ratatui::layout::Rect)> {
    let t = app.tabs.get(app.cur).filter(|t| !t.zoom && !app.stacked() && !app.grown)?;
    let mut out = vec![];
    t.root.borders(app.body, &mut vec![], &mut out);
    for (area, dir, path) in out.into_iter().rev() {
        let Some(ratio) = t.root.ratio_at(&path) else { continue };
        let (a, _) = crate::layout::split_rect(area, dir, ratio);
        let line = layout.to_px(a);
        let whole = layout.to_px(area);
        let near = match dir {
            Dir::Right => (p.x - line.right()).abs() <= super::layout::GAP && p.y >= whole.y && p.y < whole.bottom(),
            Dir::Down => (p.y - line.bottom()).abs() <= super::layout::GAP && p.x >= whole.x && p.x < whole.right(),
        };
        if near {
            return Some((path, dir, area));
        }
    }
    None
}

pub fn released(g: &mut Gui, p: Point, button: MouseButton) {
    let Some(cb) = cbutton(button) else { return };
    g.app.touched();
    g.sidebar.bar_drag = None;
    g.sidebar.resizing = false;
    if g.media.released() {
        return;
    }
    let m = mods(g);
    match g.drag.take() {
        Some(Drag::Sidebar { .. } | Drag::Media { .. }) => g.prefs.save(g.app.persist),
        Some(Drag::Select { pane }) => {
            // copied on release, like the terminal UI
            let text = with_screen(g, pane, |s| s.term.selection_to_string()).flatten().map(|t| tidy_copy(&t)).filter(|s| !s.trim().is_empty());
            if let Some(text) = text {
                super::window::set_clipboard(&text);
                let n = text.chars().count();
                g.app.toast(crate::alerts::Kind::Info, format!("copied {n} character{}", if n == 1 { "" } else { "s" }));
            } else {
                with_screen(g, pane, |s| s.term.selection = None);
            }
        }
        Some(Drag::Program { pane }) => {
            let drawn = g.panes.get(&pane).map(|s| s.drawn).unwrap_or_default();
            let (pt, _) = drawn.point_at(p);
            let row = (pt.line.0 + drawn.display_offset as i32).max(0) as u16;
            let area = ratatui::layout::Rect::new(0, 0, drawn.cols as u16, drawn.rows as u16);
            g.app.with_pane(pane, |pn, cx| pn.mouse(cell_event(MouseEventKind::Up(cb), pt.column.0 as u16, row, m), area, cx));
        }
        Some(_) => {}
        None => {}
    }
    // a link opens on release at the same cell
    if let Some((id, uri, (r, c))) = g.link_press.take()
        && button == MouseButton::Left
    {
        let drawn = g.panes.get(&id).map(|s| s.drawn).unwrap_or_default();
        if drawn.cell_at(p) == Some((r, c)) {
            open_link(g, id, &uri);
        }
    }
}

/// Open a link the user clicked: files in the preview, the rest with the system.
pub fn open_link(g: &mut Gui, id: PaneId, uri: &str) {
    if uri.starts_with("file:") {
        g.app.open_file_link(id, uri);
    } else {
        g.app_event(Event::OpenLink(uri.to_string()));
    }
}

pub fn moved(g: &mut Gui, p: Point) {
    let m = mods(g);
    // a move counts as you being here (the summary screensaver turns back), and is still handled
    g.app.touched();
    if g.clipboard_menu.is_some() {
        return;
    }
    if g.app.prefix_armed {
        if let Some(Action::Menu(index)) = hit_at(g, p) {
            g.app.menu.sel = index;
        }
        return;
    }
    if g.pressing && g.media.dragged(p) {
        return;
    }
    // the sidebar's scrollbar, dragged
    if g.sidebar.bar_drag.is_some() {
        if g.pressing {
            g.sidebar.drag_bar(p.y);
            g.app.side_nav();
            return;
        }
        g.sidebar.bar_drag = None;
    }
    match g.drag.clone() {
        Some(Drag::Sidebar { start_x, start_w }) => {
            let max = (g.layout.window.w * 0.5).max(220.0);
            g.prefs.sidebar_width = (start_w + p.x - start_x).clamp(220.0, max);
        }
        Some(Drag::Media { start_x, start_w }) => {
            g.prefs.media_width = (start_w - (p.x - start_x)).clamp(280.0, (g.layout.window.w * 0.6).max(300.0));
        }
        Some(Drag::Divider { path, dir, area }) => {
            let whole = g.layout.to_px(area);
            let r = match dir {
                Dir::Right => (p.x - whole.x) / whole.w.max(1.0),
                Dir::Down => (p.y - whole.y) / whole.h.max(1.0),
            };
            let cur = g.app.cur;
            if let Some(crate::layout::Node::Split { ratio, .. }) = g.app.tabs.get_mut(cur).and_then(|t| t.root.node_at(&path)) {
                *ratio = r.clamp(0.1, 0.9);
            }
        }
        Some(Drag::TileEdge { pane, ax, far, offset }) => {
            let cells = g.layout.to_cells(p.x, p.y);
            let body = g.app.body;
            let (pos, start) = if ax == 0 { (cells.x, body.x) } else { (cells.y, body.y) };
            let at = (pos as i32 + offset - start as i32).clamp(0, u16::MAX as i32) as u16;
            // an edge only holds once the drag has moved it a cell (the terminal reports drags per cell): a
            // click, or a double-click, on an edge holds nothing
            let held = g.app.locks.iter().any(|l| (l.pane, l.ax, l.far) == (pane, ax, far));
            let now = g.app.outer.iter().find(|(id, _)| *id == pane).map(|&(_, r)| {
                let edge = match (ax, far) {
                    (0, true) => r.right(),
                    (0, false) => r.x,
                    (_, true) => r.bottom(),
                    (_, false) => r.y,
                };
                edge.saturating_sub(start)
            });
            if held || now != Some(at) {
                g.app.locks.retain(|l| (l.pane, l.ax, l.far) != (pane, ax, far));
                g.app.locks.push(crate::priority::Lock { pane, ax, far, at });
                g.last_press = None;
            }
        }
        Some(Drag::Select { pane }) => {
            let drawn = g.panes.get(&pane).map(|s| s.drawn).unwrap_or_default();
            // past the top or bottom edge: scroll the history while selecting
            if p.y < drawn.origin.y && let Some(t) = g.app.panes.get_mut(&pane).and_then(|p| p.as_term()) {
                let n = t.scroll_offset() + 1;
                t.set_scroll_offset(n.min(drawn.history));
            } else if p.y > drawn.origin.y + drawn.rows as f32 * drawn.cell_h && let Some(t) = g.app.panes.get_mut(&pane).and_then(|p| p.as_term()) {
                let n = t.scroll_offset().saturating_sub(1);
                t.set_scroll_offset(n);
            }
            let (point, right) = drawn.point_at(p);
            with_screen(g, pane, |s| {
                if let Some(sel) = &mut s.term.selection {
                    sel.update(point, if right { Side::Right } else { Side::Left });
                }
            });
        }
        Some(Drag::Program { pane }) => {
            let drawn = g.panes.get(&pane).map(|s| s.drawn).unwrap_or_default();
            if let Some((r, c)) = drawn.cell_at(p) {
                let area = ratatui::layout::Rect::new(0, 0, drawn.cols as u16, drawn.rows as u16);
                g.app.with_pane(pane, |pn, cx| pn.mouse(cell_event(MouseEventKind::Drag(CButton::Left), c as u16, r as u16, m), area, cx));
            }
        }
        None => {
            // programs tracking every motion get it
            if let Some(Action::Pane(id, PaneAct::Body)) = hit_at(g, p)
                && !g.app.overlay.is_open()
                && let Some(t) = g.app.panes.get(&id).and_then(|p| p.as_term_ref())
                && t.program_modes().2 == vt100::MouseProtocolMode::AnyMotion
            {
                let drawn = g.panes.get(&id).map(|s| s.drawn).unwrap_or_default();
                if let Some((r, c)) = drawn.cell_at(p) {
                    let area = ratatui::layout::Rect::new(0, 0, drawn.cols as u16, drawn.rows as u16);
                    g.app.with_pane(id, |pn, cx| pn.mouse(cell_event(MouseEventKind::Moved, c as u16, r as u16, m), area, cx));
                }
            }
        }
    }
}

pub fn wheel(g: &mut Gui, p: Point, _dx: f32, dy: f32) {
    if dy == 0.0 {
        return;
    }
    // a turn of the wheel while the summary screensaver shows only wakes it
    if g.app.touched() {
        return;
    }
    let down = dy < 0.0;
    if g.app.prefix_armed {
        g.app.menu_move(if down { 3 } else { -3 });
        return;
    }
    let m = mods(g);
    let hit = hit_at(g, p);
    let overlay = g.app.overlay.is_open();
    if g.media.expanded && !overlay {
        if let Some(Action::Media(n)) = hit {
            g.media.wheel(n, p, dy);
        }
        return;
    }
    // nothing drawn over this spot but the sidebar and the tabs themselves (a dialog's scrim is)
    let bare = matches!(hit, None | Some(Action::None | Action::Side(_) | Action::SideList | Action::SideFilter | Action::SideScroll | Action::SideResize | Action::MediaToggle));
    // anywhere over the project tabs steps them, once per burst (not while something is open over them)
    if bare && g.layout.strip.h > 0.0 && g.layout.strip.contains(p) {
        if !overlay {
            g.app.strip_wheel(down);
        }
        return;
    }
    // anywhere over the sidebar (its gaps and footer too) scrolls the list; it holds until a key moves the selection
    if bare && on_sidebar(g, p) && (!overlay || beside_launcher(g)) {
        g.sidebar.scroll = (g.sidebar.scroll - dy * 44.0).max(0.0);
        g.sidebar.manual = true;
        g.app.side_nav();
        return;
    }
    if overlay {
        if g.overlay_native {
            let n = match hit {
                Some(Action::Overlay(n)) => Some(n),
                _ => None,
            };
            for _ in 0..(dy.abs().round().max(1.0) as usize).min(5) {
                super::overlays::wheel(g, n, down);
            }
            return;
        }
        if let Some(Action::Overlay(_)) = hit
            && let Some(grid) = g.overlay_grid
            && let Some(cell) = grid.cell(p)
        {
            let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
            for _ in 0..(dy.abs().round().max(1.0) as usize).min(5) {
                g.app.mouse(cell_event(kind, cell.x, cell.y, m));
            }
        }
        return;
    }
    match hit {
        Some(Action::Side(SideHit::Strip(_))) => g.app.strip_wheel(down),
        Some(Action::Focus(id, _)) => super::focus::wheel(g, id, dy),
        Some(Action::Pane(id, _)) => pane_wheel(g, id, p, dy, m),
        Some(Action::Media(n)) => g.media.wheel(n, p, dy),
        Some(Action::View(id, n)) => {
            for _ in 0..(dy.abs().round().max(1.0) as usize).min(5) {
                super::views::wheel(&mut g.app, id, Some(n), down);
            }
        }
        _ => {}
    }
}

fn pane_wheel(g: &mut Gui, id: PaneId, p: Point, dy: f32, m: KeyModifiers) {
    let Some(pane) = g.app.panes.get(&id) else { return };
    let down = dy < 0.0;
    let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
    if pane.as_term_ref().is_none() {
        if let Some(grid) = g.painted.views.iter().find(|(v, _)| *v == id).map(|x| x.1)
            && let Some(cell) = grid.cell(p)
        {
            let area = ratatui::layout::Rect::new(0, 0, grid.cols, grid.rows);
            g.app.with_pane(id, |pn, cx| pn.mouse(cell_event(kind, cell.x, cell.y, m), area, cx));
        }
        return;
    }
    // a card turned over shows bro's own text: the program (and its history) under it doesn't scroll
    if g.app.flips.contains_key(&id) {
        return;
    }
    let wants = pane.wants_mouse();
    let drawn = g.panes.get(&id).map(|s| s.drawn).unwrap_or_default();
    if wants && !m.contains(KeyModifiers::SHIFT) {
        if let Some((r, c)) = drawn.cell_at(p) {
            let area = ratatui::layout::Rect::new(0, 0, drawn.cols as u16, drawn.rows as u16);
            for _ in 0..(dy.abs().round().max(1.0) as usize).min(5) {
                g.app.with_pane(id, |pn, cx| pn.mouse(cell_event(kind, c as u16, r as u16, m), area, cx));
            }
        }
        return;
    }
    // the alternate screen (full-screen programs) has no history: the wheel moves the cursor instead
    let alt_screen = drawn.mode.is_some_and(|md| md.contains(alacritty_terminal::term::TermMode::ALT_SCREEN) && md.contains(alacritty_terminal::term::TermMode::ALTERNATE_SCROLL));
    let st = g.panes.entry(id).or_default();
    st.scroll_acc += dy * 3.0;
    let lines = st.scroll_acc.trunc() as i32;
    if lines == 0 {
        return;
    }
    st.scroll_acc -= lines as f32;
    if alt_screen {
        let key = if lines > 0 { "\x1b[A" } else { "\x1b[B" };
        if let Some(t) = g.app.panes.get_mut(&id).and_then(|p| p.as_term()) {
            let (_, app_cursor, ..) = t.program_modes();
            let key = if app_cursor { key.replace('[', "O") } else { key.to_string() };
            t.send(key.repeat(lines.unsigned_abs() as usize).as_bytes());
        }
        return;
    }
    if let Some(t) = g.app.panes.get_mut(&id).and_then(|p| p.as_term()) {
        let cur = t.scroll_offset() as i32;
        let next = (cur + lines).clamp(0, drawn.history as i32);
        t.set_scroll_offset(next as usize);
    }
}

/// Text typed with an input method: into the focused terminal as typed text, else where pastes go.
pub fn type_text(g: &mut Gui, text: &str) {
    // you're here (the summary screensaver turns back); what you composed still lands, as a paste does
    g.app.touched();
    let to_terminal = !g.app.overlay.is_open() && g.app.renaming.is_none() && !g.app.side_focus && !g.app.prefix_armed;
    // a session in focus mode: into its reply box
    if to_terminal
        && let Some(id) = g.app.focused()
        && g.app.focus_view(id)
    {
        g.app.focus_paste(id, text);
        return;
    }
    if to_terminal
        && let Some(id) = g.app.focused()
        && let Some(t) = g.app.panes.get_mut(&id).and_then(|p| p.as_term())
    {
        t.send(text.as_bytes());
        return;
    }
    g.app.paste(text);
}

/// A file dropped on the window: its path, pasted where the keyboard is (as terminals do).
pub fn dropped(g: &mut Gui, path: std::path::PathBuf) {
    g.app.touched();
    // a picture or a video dropped on the window shows beside the session too
    if super::media::is_media(&path) {
        let owner = g.focused_owner();
        g.media.add(path.clone(), super::media::Source::Dropped, owner, None);
        if g.media.items.iter().any(|it| it.path == path) {
            g.show_media(&path);
        }
    }
    // into a reply box in focus mode: a picture as a thumbnail, another file as its path
    if !g.app.overlay.is_open()
        && let Some(id) = g.app.focused()
        && g.app.focus_view(id)
    {
        g.app.focus_attach(id, vec![path]);
        return;
    }
    // a space after it, so a second file dropped next isn't glued on
    let text = crate::clip::paste_form(&[path.to_string_lossy().to_string()]);
    g.app.paste(&format!("{text} "));
}

/// The pointer shape where it is, when something other than the hit's own shape applies.
pub fn cursor_at(g: &Gui, p: Point) -> Option<Cursor> {
    match hit_at(g, p) {
        // a turned card: its links and files open on click; the program's own links under it don't show
        Some(Action::Pane(id, PaneAct::Body)) if g.app.flips.contains_key(&id) => {
            Some(if g.painted.card_links.iter().any(|(r, pane, _)| *pane == id && r.contains(p)) { Cursor::Pointer } else { Cursor::Default })
        }
        Some(Action::Pane(id, PaneAct::Body)) => {
            let st = g.panes.get(&id)?;
            let view_top = g.app.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.view_top() as usize).unwrap_or(0);
            let (r, c) = st.drawn.cell_at(p)?;
            if st.links.iter().any(|l| l.row == r + view_top && (l.start..l.end).contains(&c)) {
                return Some(Cursor::Pointer);
            }
            let term = g.app.panes.get(&id)?.as_term_ref()?;
            if term.prompt_option_at(r as u16).is_some() && !term.wants_mouse() {
                return Some(Cursor::Pointer);
            }
            if term.wants_mouse() { Some(Cursor::Default) } else { Some(Cursor::Text) }
        }
        None | Some(Action::None) => {
            if on_sidebar(g, p) {
                return None;
            }
            if let Some((_, dir, _)) = divider_at(g, p) {
                return Some(if dir == Dir::Right { Cursor::ResizeH } else { Cursor::ResizeV });
            }
            tile_edge_at(&g.app, &g.layout, p).map(|(_, ax, _)| if ax == 0 { Cursor::ResizeH } else { Cursor::ResizeV })
        }
        _ => None,
    }
}

pub fn has_selection(g: &Gui) -> bool {
    g.app.panes.keys().any(|id| with_screen(g, *id, |s| s.term.selection.as_ref().is_some_and(|sel| !sel.is_empty())).unwrap_or(false))
}

/// Copy the selection (ctrl+shift+c, or ctrl+c while something is selected). True when something was copied.
pub fn copy_selection(g: &mut Gui) -> bool {
    let ids: Vec<PaneId> = g.app.panes.keys().copied().collect();
    for id in ids {
        if let Some(text) = with_screen(g, id, |s| s.term.selection_to_string()).flatten().map(|t| tidy_copy(&t)).filter(|t| !t.is_empty()) {
            super::window::set_clipboard(&text);
            let n = text.chars().count();
            g.app.toast(crate::alerts::Kind::Info, format!("copied {n} character{}", if n == 1 { "" } else { "s" }));
            clear_selections(g, None);
            return true;
        }
    }
    false
}

/// What a terminal selection copies as, the terminal UI's way: each line without the blanks the screen pads it
/// with, and no trailing newline (a triple-clicked line copies as just its text).
fn tidy_copy(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches(['\r', ' '])).collect();
    lines.join("\n").trim_end_matches('\n').to_string()
}

/// For the frame: rects of the hits (debug) — unused in release builds.
#[allow(dead_code)]
pub fn hit_rects(g: &Gui) -> Vec<Rect> {
    g.hits.iter().map(|h| h.rect).collect()
}
