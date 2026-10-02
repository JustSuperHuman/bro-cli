//! Whole-app headless tests and HTML snapshots (saved to `crates/bro-tui/snapshots/`).

use super::*;
use crate::keymap::Act;
use crate::services::{Services, fallback_settings};
use crate::testkit;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

const W: u16 = 160;
const H: u16 = 46;

fn app(demo: bool) -> App {
    testkit::isolate();
    let (tx, rx) = std::sync::mpsc::channel();
    std::mem::forget(rx); // keep the channel open; tests don't pump it
    let svc = Services::offline(fallback_settings(), tx.clone());
    App::new(svc, tx, Opts { demo, fixed_demo: true, load_recents: false })
}

fn shot(app: &mut App, name: &str) -> String {
    testkit::snapshot(name, W, H, |f| app.draw(f))
}

/// Draw without saving a snapshot (intermediate states).
fn draw(app: &mut App) -> String {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).expect("backend");
    term.draw(|f| app.draw(f)).expect("draw");
    testkit::dump(term.backend().buffer())
}

fn key(app: &mut App, code: KeyCode, m: KeyModifiers) {
    app.key(KeyEvent::new(code, m));
}

#[test]
fn new_session_picker_requests_usage_on_every_launch_path() {
    let mut a = app(false);
    let (tx, rx) = std::sync::mpsc::channel();
    a.svc.set_usage_kick(tx);
    a.open_launcher(None, Place::Tab);
    assert!(rx.try_recv().is_ok());
    a.quick_launch(Some(bro_core::Harness::Codex));
    assert!(rx.try_recv().is_ok());
    a.launch_usage(bro_core::Harness::Codex, Some("codex:team".into()));
    assert!(rx.try_recv().is_ok());
    a.open_view("usage");
    assert!(rx.try_recv().is_ok());
    a.quick_launch(None);
    assert!(rx.try_recv().is_err(), "a plain shell has no account usage to refresh");
}

#[test]
fn new_session_picker_updates_usage_without_losing_choices() {
    let mut a = app(false);
    a.open_launcher(Some(std::env::temp_dir()), Place::SplitRight);
    if let Overlay::Launcher(l) = &mut a.overlay {
        l.harness = bro_core::Harness::Codex;
        l.list_filter = "team".into();
        l.browser = bro_core::browser::BrowserMode::Chrome;
    }
    let dir = match &a.overlay { Overlay::Launcher(l) => l.dir.clone(), _ => unreachable!() };
    a.svc.update(|st| {
        let e = st.usage.get_mut("codex:team").unwrap();
        e.fetching = true;
    });
    a.handle(Event::Services);
    assert!(draw(&mut a).contains("refreshing…"));
    a.svc.update(|st| {
        let e = st.usage.get_mut("codex:team").unwrap();
        e.usage.as_mut().unwrap().five_hour.as_mut().unwrap().used_pct = 28.0;
        e.fetching = false;
        e.error = None;
    });
    a.handle(Event::Services);
    let Overlay::Launcher(l) = &a.overlay else { panic!("launcher closed") };
    assert_eq!(l.spec().unwrap().profile_id.as_deref(), Some("codex:team"));
    assert_eq!(l.list_filter, "team");
    assert_eq!(l.dir, dir);
    assert_eq!(l.place, Place::SplitRight);
    assert_eq!(l.browser, bro_core::browser::BrowserMode::Chrome);
    assert!(draw(&mut a).contains("72% left"));
    a.svc.update(|st| st.usage.get_mut("codex:team").unwrap().error = Some("timeout".into()));
    a.handle(Event::Services);
    let screen = draw(&mut a);
    assert!(screen.contains("stale") && screen.contains("72% left"), "{screen}");
}

#[test]
fn new_session_picker_receives_accounts_loaded_after_it_opens() {
    let mut a = app(false);
    let profiles = a.svc.state().profiles.clone();
    a.svc.update(|st| st.profiles = crate::services::Avail::Loading);
    a.open_launcher(None, Place::Tab);
    a.svc.update(|st| st.profiles = profiles);
    a.handle(Event::Services);
    assert!(draw(&mut a).contains("personal"));
}

#[test]
fn new_session_picker_shows_weekly_only_codex_usage() {
    let mut a = app(false);
    a.svc.update(|st| {
        let e = st.usage.get_mut("codex:local").unwrap();
        let u = e.usage.as_mut().unwrap();
        u.five_hour = None;
        u.weekly.as_mut().unwrap().used_pct = 60.0;
    });
    a.open_launcher(None, Place::Tab);
    if let Overlay::Launcher(l) = &mut a.overlay {
        l.harness = bro_core::Harness::Codex;
        l.list_filter = "local".into();
    }
    let screen = draw(&mut a);
    assert!(screen.lines().any(|row| row.contains("local") && row.contains("weekly") && row.contains("40% left")), "{screen}");
}

#[test]
fn codex_alt_up_reaches_terminal_and_prefix_up_still_navigates() {
    use std::io::Write;
    struct Capture(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut a = app(true);
    draw(&mut a);
    let upper = a.focused().unwrap();
    let lower = a.open_shell(Some(std::env::temp_dir()), Place::SplitDown);
    // Draw the split so pane navigation knows where above is.
    draw(&mut a);
    let bytes = std::sync::Arc::new(parking_lot::Mutex::new(vec![]));
    let t = a.panes.get_mut(&lower).unwrap().as_term().unwrap();
    t.meta.harness = Some(bro_core::Harness::Codex);
    t.set_test_writer(Box::new(Capture(bytes.clone())));
    a.side_focus = false;
    key(&mut a, KeyCode::Up, KeyModifiers::ALT);
    assert_eq!(&*bytes.lock(), b"\x1b[1;3A");
    assert_eq!(a.focused(), Some(lower), "Codex's key must not move pane focus");
    key(&mut a, KeyCode::Char(' '), KeyModifiers::CONTROL);
    key(&mut a, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(a.focused(), Some(upper));
    assert_eq!(&*bytes.lock(), b"\x1b[1;3A", "prefix navigation must not reach Codex");
    a.focus_pane(lower);
    a.side_focus = true;
    key(&mut a, KeyCode::Up, KeyModifiers::ALT);
    assert_eq!(&*bytes.lock(), b"\x1b[1;3A", "sidebar focus must not type into Codex");
}

#[test]
fn sidebar_drag_saves_on_release_clamps_and_restores_after_window_resize() {
    let mut a = app(true);
    draw(&mut a);
    let mouse = |a: &mut App, kind, x, y| a.mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
    let edge = a.body.x - 1;
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), edge, 15);
    mouse(&mut a, MouseEventKind::Drag(MouseButton::Left), 57, 15);
    draw(&mut a);
    assert_eq!(a.body.x, 58);
    assert_eq!(a.svc.settings().sidebar_width, None, "don't write settings on every drag event");
    mouse(&mut a, MouseEventKind::Up(MouseButton::Left), 57, 15);
    assert_eq!(a.svc.settings().sidebar_width, Some(58));
    assert!(!a.side_drag && a.sel.is_none() && a.drag.is_none());
    let mut small = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, H)).unwrap();
    small.draw(|f| a.draw(f)).unwrap();
    assert_eq!(a.body.x, 45, "reserve at least half the window for sessions");
    draw(&mut a);
    assert_eq!(a.body.x, 58, "shrinking a window doesn't overwrite the preferred width");
    let (tx, _) = std::sync::mpsc::channel();
    let mut reopened = App::new(Services::offline(a.svc.settings(), tx.clone()), tx, Opts::default());
    draw(&mut reopened);
    assert_eq!(reopened.body.x, 58);
    a.side_edge_click = None;
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), 57, 15);
    mouse(&mut a, MouseEventKind::Drag(MouseButton::Left), 0, 15);
    draw(&mut a);
    assert_eq!(a.body.x, 26);
    mouse(&mut a, MouseEventKind::Drag(MouseButton::Left), 159, 15);
    draw(&mut a);
    assert_eq!(a.body.x, 80);
    mouse(&mut a, MouseEventKind::Up(MouseButton::Left), 159, 15);
    a.side_edge_click = None;
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), 79, 15);
    mouse(&mut a, MouseEventKind::Up(MouseButton::Left), 79, 15);
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), 79, 15);
    draw(&mut a);
    assert_eq!(a.body.x, 40, "double-click restores adaptive width");
    assert_eq!(a.svc.settings().sidebar_width, None);
    a.overlay = Overlay::Help(crate::help::Help::default());
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), 39, 15);
    assert!(!a.side_drag, "overlays own mouse input");
}

#[test]
fn tile_all_shortcut_preserves_splits_focus_and_zoom() {
    let mut a = app(true);
    let focus = a.focused().unwrap();
    let original: Vec<_> = a.tabs.iter().map(|t| format!("{:?}", t.root)).collect();
    // Hidden sidebar rows must not hide sessions from the all-sessions action.
    a.side.filter = "nothing matches".into();
    key(&mut a, KeyCode::Char('t'), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert!(a.tile_all);
    assert_eq!(a.focused(), Some(focus));
    assert_eq!(a.visible().len(), 5);
    a.side.filter.clear();
    shot(&mut a, "tiled-projects");
    assert_eq!(a.outer.len(), 5);
    let (other, r) = a.outer.iter().find(|(id, _)| !a.tabs[a.cur].root.contains(*id)).copied().unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 2, row: r.y, modifiers: KeyModifiers::NONE });
    assert_eq!(a.focused(), Some(other));
    key(&mut a, KeyCode::Char('z'), KeyModifiers::ALT);
    draw(&mut a);
    assert_eq!(a.outer.len(), 1);
    assert_eq!(a.visible(), vec![other]);
    key(&mut a, KeyCode::Char('z'), KeyModifiers::ALT);
    key(&mut a, KeyCode::Tab, KeyModifiers::CONTROL);
    assert!(a.tile_all && a.stacked(), "cycling sessions keeps the grid");
    a.go_session(focus);
    assert!(a.stacked(), "sidebar picks keep the grid too");
    key(&mut a, KeyCode::Char('T'), KeyModifiers::ALT);
    assert!(!a.stacked() && !a.tile_all);
    assert_eq!(a.focused(), Some(focus));
    assert_eq!(original, a.tabs.iter().map(|t| format!("{:?}", t.root)).collect::<Vec<_>>());
    key(&mut a, KeyCode::Char(' '), KeyModifiers::CONTROL);
    key(&mut a, KeyCode::Char('T'), KeyModifiers::SHIFT);
    assert!(a.tile_all, "prefix T is an alternative when a host captures Alt");
}

#[test]
fn tiled_sessions_follow_new_sessions_and_closes_without_losing_focus() {
    use crate::panes::term::Term;
    let mut a = app(true);
    let origin = a.focused().unwrap();
    let meta = a.panes[&origin].as_term_ref().unwrap().meta.clone();
    a.run_act(Act::TileAll);
    let added = a.new_tab(Box::new(Term::fixed(meta, "new session", a.svc.clone())));
    assert_eq!(a.stack.len(), 6);
    assert_eq!(a.focused(), Some(added));
    key(&mut a, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(a.stack.len(), 5);
    assert!(a.focused().is_some_and(|id| a.panes.contains_key(&id)));
    let survivor = *a.stack.last().unwrap();
    for id in a.stack.clone().into_iter().filter(|id| *id != survivor) {
        a.close(id);
    }
    assert!(!a.stacked());
    assert_eq!(a.focused(), Some(survivor));
    a.close(survivor);
    assert!(a.tabs.is_empty());
    a.run_act(Act::TileAll);
    assert!(!a.tile_all && a.stack.is_empty());
    draw(&mut a);
}

#[test]
fn project_row_tiles_only_its_sessions_then_all() {
    let mut a = app(true);
    let live = a.live_infos();
    let all = live.len();
    // a project with two or more sessions
    let key = live.iter().map(|l| l.project_key.clone()).find(|k| live.iter().filter(|l| l.project_key == *k).count() >= 2).expect("demo has a project with two sessions");
    let mine = live.iter().filter(|l| l.project_key == key).count();
    assert!(mine < all);
    let row = a.rows().iter().position(|r| matches!(r, crate::sidebar::Row::Project { key: k, .. } if *k == key)).unwrap();
    a.activate_row(row);
    assert_eq!(a.tile_project.as_deref(), Some(key.as_str()));
    assert_eq!(a.visible().len(), mine);
    assert!(a.visible().iter().all(|id| live.iter().any(|l| l.pane == *id && l.project_key == key)));
    let s = shot(&mut a, "tiled-one-project");
    assert!(s.contains("▦"), "{s}");
    a.activate_row(row);
    assert!(a.tile_all && a.tile_project.is_none());
    assert_eq!(a.visible().len(), all);
    // and from there the same project again narrows the grid
    a.activate_row(row);
    assert_eq!(a.visible().len(), mine);
    // the tile-all shortcut widens a project grid rather than leaving it
    a.run_act(Act::TileAll);
    assert!(a.tile_all && a.tile_project.is_none());
    assert_eq!(a.visible().len(), all);
}

#[test]
fn project_chrome_matches_sidebar_and_focus_is_distinct_in_dark_and_light_themes() {
    use ratatui::{backend::TestBackend, style::Color, Terminal};
    for name in ["graphite", "paper"] {
        let mut a = app(true);
        a.theme = crate::theme::get(name);
        a.run_act(Act::TileAll);
        let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let focus = a.focused().unwrap();
        for &(id, area) in &a.outer {
            let project = &a.panes[&id].as_term_ref().unwrap().meta.project;
            let color = a.project_color(&project.key);
            let buf = term.backend().buffer();
            let corner = &buf[(area.x, area.y)];
            let head = &buf[(area.x + 1, area.y)];
            if id == focus {
                assert_eq!(corner.symbol(), "┏");
                assert_eq!(corner.fg, color);
                assert_eq!(head.bg, color);
                assert_eq!(head.symbol(), "▶");
            } else {
                assert_eq!(corner.symbol(), "╭");
                assert_ne!(head.bg, color);
            }
            let row_idx = a.rows().iter().position(|r| matches!(r, crate::sidebar::Row::Live { info, .. } if info.pane == id)).unwrap();
            let row = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::Row(i) if *i == row_idx)).unwrap().0;
            assert_eq!(buf[(row.x, row.y)].fg, color);
            let inner = a.inner.iter().find(|(i, _)| *i == id).unwrap().1;
            assert_ne!(buf[(inner.x, inner.y)].bg, color, "project chrome doesn't recolor terminal output");
        }
        let unique: std::collections::HashSet<_> = a.project_colors.values().collect();
        assert_eq!(unique.len(), a.project_colors.len(), "small workspaces don't reuse project colors");
        assert_ne!(a.project_color("a"), Color::Reset);
        shot(&mut a, &format!("tiled-{name}"));
    }
}

#[test]
fn confirm_dialogs_answer_to_clicks_on_yes_and_no() {
    let mut a = app(true);
    let id = a.focused().unwrap();
    a.panes.get_mut(&id).unwrap().as_term().unwrap().demo_activity = Some(Activity::Working);
    let click = |a: &mut App, r: Rect| a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 2, row: r.y, modifiers: KeyModifiers::NONE });
    a.ask_close(id);
    assert!(matches!(a.overlay, super::overlays::Overlay::Confirm(_)), "a busy session asks first");
    draw(&mut a);
    let no = a.confirm_hits.iter().find(|(_, yes)| !yes).unwrap().0;
    click(&mut a, no);
    assert!(!a.overlay.is_open() && a.panes.contains_key(&id), "No keeps the session");
    a.ask_close(id);
    draw(&mut a);
    // A click elsewhere inside the dialog does nothing; the dialog stays.
    let yes = a.confirm_hits.iter().find(|(_, yes)| *yes).unwrap().0;
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: yes.x, row: yes.y - 2, modifiers: KeyModifiers::NONE });
    assert!(a.overlay.is_open());
    click(&mut a, yes);
    assert!(!a.overlay.is_open() && !a.panes.contains_key(&id), "Yes closes it");
}

#[test]
fn clicking_an_agents_prompt_option_answers_it() {
    use crate::panes::term::Term;
    let mut a = app(true);
    let meta = a.panes[&a.focused().unwrap()].as_term_ref().unwrap().meta.clone();
    let screen = "● Bash(rm -rf target)\r\n\r\n Do you want to proceed?\r\n ❯ 1. Yes\r\n   2. Yes, and don't ask again for rm commands\r\n   3. No, and tell Claude what to do differently (esc)\r\n";
    let id = a.new_tab(Box::new(Term::fixed(meta, screen, a.svc.clone())));
    draw(&mut a);
    let inner = a.inner.iter().find(|(i, _)| *i == id).unwrap().1;
    let row_of = |text: &str| screen.split("\r\n").position(|l| l.contains(text)).unwrap() as u16;
    let click = |a: &mut App, row: u16| a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: inner.x + 6, row: inner.y + row, modifiers: KeyModifiers::NONE });
    let sent = |a: &mut App| std::mem::take(&mut a.panes.get_mut(&id).unwrap().as_term().unwrap().sent);
    // Not waiting on anything: numbered lines are just text.
    click(&mut a, row_of("2. Yes"));
    assert!(sent(&mut a).is_empty());
    a.panes.get_mut(&id).unwrap().as_term().unwrap().demo_activity = Some(Activity::Blocked);
    click(&mut a, row_of("2. Yes"));
    assert_eq!(sent(&mut a), b"2\r", "the option's key, then enter");
    click(&mut a, row_of("3. No"));
    assert_eq!(sent(&mut a), b"3\r");
    click(&mut a, row_of("Do you want"));
    assert!(sent(&mut a).is_empty(), "the question itself isn't an option");
    // Hovering an option highlights its row.
    a.hover = ratatui::layout::Position { x: inner.x + 6, y: inner.y + row_of("1. Yes") };
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    term.draw(|f| a.draw(f)).unwrap();
    let buf = term.backend().buffer();
    let lit = buf[(inner.x + 1, inner.y + row_of("1. Yes"))].bg;
    let plain = buf[(inner.x + 1, inner.y + row_of("2. Yes"))].bg;
    assert_ne!(lit, plain, "the hovered option stands out");
}

#[test]
fn double_clicking_a_pane_goes_full_screen_and_back() {
    let mut a = app(true);
    a.run_act(Act::TileAll);
    draw(&mut a);
    assert_eq!(a.outer.len(), 5);
    let focus = a.focused().unwrap();
    let (other, r) = a.outer.iter().find(|(id, _)| *id != focus).copied().unwrap();
    let click = |a: &mut App, x, y| a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
    let release = |a: &mut App, x, y| a.mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
    // One click only focuses.
    click(&mut a, r.x + 3, r.y);
    release(&mut a, r.x + 3, r.y);
    assert_eq!(a.focused(), Some(other));
    assert!(!a.stack_zoom);
    // A second click on the header: full screen.
    click(&mut a, r.x + 3, r.y);
    release(&mut a, r.x + 3, r.y);
    assert!(a.stack_zoom, "double-click zooms");
    draw(&mut a);
    assert_eq!(a.outer.len(), 1);
    assert_eq!(a.visible(), vec![other]);
    // Double-click inside the full-screen pane's content: back to the tiles.
    let inner = a.inner.iter().find(|(id, _)| *id == other).unwrap().1;
    for _ in 0..2 {
        click(&mut a, inner.x + 4, inner.y + 2);
        release(&mut a, inner.x + 4, inner.y + 2);
    }
    assert!(!a.stack_zoom, "double-click again restores the grid");
    draw(&mut a);
    assert_eq!(a.outer.len(), 5);
    assert!(a.tile_all && a.stacked());
    // Two single clicks too far apart in time are not a double-click.
    let (_, r) = a.outer.iter().find(|(id, _)| *id == other).copied().unwrap();
    click(&mut a, r.x + 3, r.y);
    a.last_pane_click = a.last_pane_click.map(|(at, id, pos)| (at - std::time::Duration::from_millis(600), id, pos));
    click(&mut a, r.x + 3, r.y);
    assert!(!a.stack_zoom);
    // Split tabs zoom the same way.
    key(&mut a, KeyCode::Char('T'), KeyModifiers::ALT);
    assert!(!a.stacked());
    draw(&mut a);
    let (id, r) = a.outer[0];
    click(&mut a, r.x + 3, r.y);
    click(&mut a, r.x + 3, r.y);
    assert!(a.tabs[a.cur].zoom && a.focused() == Some(id));
    click(&mut a, r.x + 3, r.y);
    click(&mut a, r.x + 3, r.y);
    assert!(!a.tabs[a.cur].zoom);
}

#[test]
fn tiled_projects_each_get_one_colored_frame_around_their_sessions() {
    use ratatui::{backend::TestBackend, Terminal};
    let mut a = app(true);
    a.run_act(Act::TileAll);
    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    term.draw(|f| a.draw(f)).unwrap();
    let groups = a.tile_groups();
    assert!(groups.len() >= 2, "the demo workspace tiles more than one project");
    let buf = term.backend().buffer().clone();
    if std::env::var("BRO_PRINT_TILES").is_ok() {
        for y in 0..H {
            let row: String = (0..W).map(|x| buf[(x, y)].symbol().to_string()).collect();
            eprintln!("{row}");
        }
    }
    let rect_of = |id| a.outer.iter().find(|(i, _)| *i == id).unwrap().1;
    for (key, name, ids) in &groups {
        let color = a.project_color(key);
        // The frame is the bounding box of the group's panes grown by one cell.
        let x0 = ids.iter().map(|&id| rect_of(id).x).min().unwrap() - 1;
        let y0 = ids.iter().map(|&id| rect_of(id).y).min().unwrap() - 1;
        let x1 = ids.iter().map(|&id| rect_of(id).right()).max().unwrap();
        let y1 = ids.iter().map(|&id| rect_of(id).bottom()).max().unwrap();
        assert_eq!(buf[(x0, y0)].symbol(), "╭", "{name}: top-left corner");
        assert_eq!(buf[(x1, y1)].symbol(), "╯", "{name}: bottom-right corner");
        let title: String = (x0 + 2..x1).map(|x| buf[(x, y0)].symbol().to_string()).collect();
        assert!(title.contains(&format!("● {name}")), "{name}: title in {title:?}");
        let holds_focus = ids.contains(&a.focused().unwrap());
        if holds_focus {
            assert_eq!(buf[(x0, y0)].fg, color, "{name}: the active project's frame is in its color");
        } else {
            assert_ne!(buf[(x0, y0)].fg, color, "{name}: other frames are softened");
        }
        // No other project's session sits inside this frame.
        let frame = Rect { x: x0, y: y0, width: x1 - x0 + 1, height: y1 - y0 + 1 };
        for (id, r) in &a.outer {
            assert_eq!(frame.contains(r.as_position()), ids.contains(id), "{name}: only its own sessions inside");
        }
        // Session heads inside a frame don't repeat the project name.
        for &id in ids {
            let r = rect_of(id);
            let head: String = (r.x + 1..r.right() - 1).map(|x| buf[(x, r.y)].symbol().to_string()).collect();
            assert!(!head.contains(&format!("{name} ·")), "{name}: head repeats the project: {head:?}");
        }
    }
    shot(&mut a, "tiled-project-groups");
}

#[test]
fn ctrl_w_closes_individual_sessions_and_preserves_other_panes() {
    let mut a = app(true);
    let id = a.focused().unwrap();
    a.panes.get_mut(&id).unwrap().as_term().unwrap().demo_activity = Some(Activity::Idle);
    let before = a.panes.len();
    let tabs = a.tabs.len();
    let sibling = a.tabs[a.cur].root.leaf_ids().into_iter().find(|pane| *pane != id).unwrap();
    key(&mut a, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert!(!a.panes.contains_key(&id));
    assert!(a.panes.contains_key(&sibling));
    assert_eq!(a.panes.len(), before - 1);
    assert_eq!(a.tabs.len(), tabs, "the surviving split keeps its tab");
    assert_eq!(a.focused(), Some(sibling));
    a.panes.get_mut(&sibling).unwrap().as_term().unwrap().demo_activity = Some(Activity::Idle);
    key(&mut a, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(a.tabs.len(), tabs - 1, "closing the last pane removes its tab");
    assert!(!a.quit);
    while let Some(id) = a.focused() {
        a.panes.get_mut(&id).unwrap().as_term().unwrap().demo_activity = Some(Activity::Idle);
        key(&mut a, KeyCode::Char('w'), KeyModifiers::CONTROL);
    }
    assert!(a.tabs.is_empty() && a.panes.is_empty());
    assert!(!a.quit, "closing the last session leaves the workspace open");
}

#[test]
fn ctrl_w_uses_the_existing_busy_session_confirmation() {
    let mut a = app(true);
    let id = a.focused().unwrap();
    a.panes.get_mut(&id).unwrap().as_term().unwrap().demo_activity = Some(Activity::Working);
    key(&mut a, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert!(matches!(a.overlay, Overlay::Confirm(_)));
    assert!(a.panes.contains_key(&id));
    key(&mut a, KeyCode::Char('y'), KeyModifiers::NONE);
    assert!(!a.panes.contains_key(&id));
    assert!(!a.quit);
}

#[cfg(windows)]
#[test]
#[ignore = "spawns short-lived local processes in ConPTY; no accounts or network"]
fn conpty_process_exit_removes_agent_and_shell_tabs_without_output_eof() {
    use crate::panes::term::{Spawn, Term};
    for harness in [Some(bro_core::Harness::Claude), None] {
        let mut a = app(true);
        let dir = tempfile::tempdir().unwrap();
        let survivor = a.focused().unwrap();
        let mut meta = a.panes[&survivor].as_term_ref().unwrap().meta.clone();
        meta.sid = uuid::Uuid::new_v4().to_string();
        meta.cwd = dir.path().to_path_buf();
        meta.harness = harness;
        meta.route_id = None;
        meta.cleanup.clear();
        let spawn = Spawn {
            program: "powershell.exe".into(),
            args: vec!["-NoLogo".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), "exit 7".into()],
            env: ["HOME", "USERPROFILE", "BRO_DIR", "CLAUDE_POOL_DIR", "BRO_CODEX_PROFILES_DIR"].into_iter()
                .map(|name| (name.into(), dir.path().to_string_lossy().into_owned())).collect(),
            ..Spawn::default()
        };
        let before = a.tabs.len();
        let id = a.new_tab(Box::new(Term::new(meta, spawn, a.svc.clone())));
        a.start_now(id);
        // The exited process must be noticed even after switching to another tab.
        a.focus_pane(survivor);
        let deadline = Instant::now() + Duration::from_secs(4);
        while a.panes.contains_key(&id) && Instant::now() < deadline {
            a.handle(Event::Tick);
            a.after_events();
            std::thread::sleep(Duration::from_millis(25));
        }
        let removed = !a.panes.contains_key(&id);
        let tabs = a.tabs.len();
        let focus = a.focused();
        a.shutdown();
        assert!(removed, "an exited {harness:?} process must not wait for ConPTY output EOF");
        assert_eq!(tabs, before);
        assert_eq!(focus, Some(survivor));
    }
}

#[test]
fn welcome_screen() {
    let mut a = app(false);
    let s = shot(&mut a, "welcome");
    assert!(s.contains("██████╗"), "logo\n{s}");
    assert!(s.contains("to launch your first agent"), "{s}");
    assert!(s.contains("alt+n"), "{s}");
    assert!(!s.contains("+ new session"), "the strip above usage replaced the row
{s}");
    assert!(s.contains("bro-cli-v2") && s.contains("justgains"), "past sessions grouped by project even with nothing live
{s}");
    assert!(!s.contains("↺"), "no past-count glyphs
{s}");
}

#[test]
fn sidebar_with_three_projects() {
    let mut a = app(true);
    a.run_act(Act::FocusSidebar);
    let s = shot(&mut a, "sidebar-demo");
    for p in ["bro-cli-v2", "justgains", "terminal"] {
        assert!(s.contains(p), "project {p} missing\n{s}");
    }
    assert!(s.contains("  +  ") && s.contains("alt+n"), "new-session strip
{s}");
    assert!(s.contains("usage left"), "{s}");
    assert!(!s.contains("+ new  alt+n"), "no tab bar\n{s}");
    assert!(!s.contains("port 3458"), "proxy line hidden while usage is collapsed
{s}");
    a.usage_expanded = true;
    let s = draw(&mut a);
    assert!(s.contains("proxy") && s.contains("port 3458"), "{s}");
    assert!(s.contains("phone") && s.contains("port 10001") && s.contains("3 connected"), "{s}");
    a.usage_expanded = false;
    assert!(s.contains("work · opus-5") && !s.contains("claude · work"), "pane title without the agent word
{s}");
    // the live sessions are numbered for alt+1..9
    let order = crate::sidebar::live_order(&a.live_infos(), &a.past_infos(), &a.open_infos());
    assert_eq!(order.len(), 5);
}

#[test]
fn launcher_open() {
    let mut a = app(true);
    key(&mut a, KeyCode::Char('n'), KeyModifiers::ALT);
    assert!(matches!(a.overlay, Overlay::Launcher(_)));
    let s = shot(&mut a, "launcher");
    assert!(s.contains("your logins") && s.contains("providers") && s.contains("pool") && s.contains("openrouter"), "{s}");
    assert!(!s.contains("model…"), "no model list for your own login\n{s}");
    // → codex tab, then openrouter shows its model list beside the run-on list
    key(&mut a, KeyCode::Right, KeyModifiers::NONE);
    assert!(draw(&mut a).contains("team"));
    for c in "openrouter".chars() {
        key(&mut a, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let s = shot(&mut a, "launcher-openrouter");
    assert!(s.contains("moonshotai/kimi-k2.7-code") && s.contains("262k"), "{s}");
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::None));
}

#[test]
fn usage_view() {
    let mut a = app(true);
    key(&mut a, KeyCode::Char('u'), KeyModifiers::ALT);
    let s = shot(&mut a, "usage");
    assert!(s.contains("USAGE"), "{s}");
    assert!(s.contains("claude:work") && s.contains("58%") && s.contains("resets in"), "{s}");
    assert!(s.contains("≈"), "capped headroom\n{s}");
    assert!(s.contains("large task"), "{s}");
    // alt+u again goes back
    let before = a.cur;
    key(&mut a, KeyCode::Char('u'), KeyModifiers::ALT);
    assert_ne!(a.cur, before);
}

#[test]
fn proxy_view() {
    let mut a = app(true);
    key(&mut a, KeyCode::Char('y'), KeyModifiers::ALT);
    let s = shot(&mut a, "proxy");
    assert!(s.contains("PROXY") && s.contains(":3458"), "{s}");
    assert!(s.contains("r-7f3a") && s.contains("claude pool"), "{s}");
    assert!(s.contains("429") && s.contains("200"), "{s}");
    assert!(s.contains("claude pool") && s.contains("cooling"), "pool section
{s}");
}

#[test]
fn bridge_view() {
    let mut a = app(true);
    key(&mut a, KeyCode::Char('g'), KeyModifiers::ALT);
    let s = shot(&mut a, "bridge");
    assert!(s.contains("BRIDGE") && s.contains("3 clients"), "{s}");
    assert!(s.contains("••••••••e4b8"), "token masked\n{s}");
    assert!(!s.contains("demo-9f3a7c21e4b8"), "token hidden\n{s}");
    assert!(s.contains("scan to pair"), "{s}");
    key(&mut a, KeyCode::Char('r'), KeyModifiers::NONE);
    let s = draw(&mut a);
    assert!(s.contains("demo-9f3a7c21e4b8"), "{s}");
}

#[test]
fn profiles_view() {
    let mut a = app(true);
    key(&mut a, KeyCode::Char('o'), KeyModifiers::ALT);
    key(&mut a, KeyCode::Char('j'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Char('d'), KeyModifiers::NONE);
    let s = shot(&mut a, "profiles");
    assert!(s.contains("PROFILES") && s.contains("claude:personal"), "{s}");
    assert!(s.contains("remove claude:personal?") && s.contains("accounts"), "confirm names the dir\n{s}");
}

#[test]
fn help_overlay() {
    let mut a = app(true);
    key(&mut a, KeyCode::F(1), KeyModifiers::NONE);
    let s = shot(&mut a, "help");
    assert!(s.contains("SESSIONS") && s.contains("launch an agent session"), "{s}");
    assert!(s.contains("alt+n") && s.contains("ctrl+space"), "{s}");
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::None));
}

#[test]
fn palette_runs_actions_and_previews_themes() {
    let mut a = app(true);
    key(&mut a, KeyCode::Char('p'), KeyModifiers::ALT);
    for c in "theme ocean".chars() {
        key(&mut a, KeyCode::Char(c), KeyModifiers::NONE);
    }
    assert_eq!(a.theme.name, "ocean", "live preview");
    let _ = shot(&mut a, "palette");
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(a.theme.name, crate::theme::DEFAULT, "restored on esc");
}

#[test]
fn keyboard_navigation() {
    let mut a = app(true);
    let _ = draw(&mut a);
    let order = crate::sidebar::live_order(&a.live_infos(), &a.past_infos(), &a.open_infos());
    // alt+3 jumps to live session 3 (sidebar order)
    key(&mut a, KeyCode::Char('3'), KeyModifiers::ALT);
    assert_eq!(a.focused(), Some(order[2]));
    // alt+j / alt+k cycle
    key(&mut a, KeyCode::Char('j'), KeyModifiers::ALT);
    assert_eq!(a.focused(), Some(order[3]));
    key(&mut a, KeyCode::Char('k'), KeyModifiers::ALT);
    assert_eq!(a.focused(), Some(order[2]));
    // alt+a: the blocked session first
    key(&mut a, KeyCode::Char('a'), KeyModifiers::ALT);
    let blocked = a.focused().and_then(|id| a.panes.get(&id)).and_then(|p| p.activity());
    assert_eq!(blocked, Some(Activity::Blocked));
    // alt+J moves to the next project's first session
    let proj = |a: &App| a.focused().and_then(|id| a.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|t| t.meta.project.key.clone());
    let p0 = proj(&a);
    key(&mut a, KeyCode::Char('J'), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_ne!(proj(&a), p0);
    // sidebar: alt+b, move, collapse, filter
    key(&mut a, KeyCode::Char('b'), KeyModifiers::ALT);
    assert!(a.side_focus);
    key(&mut a, KeyCode::Char('g'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Char('j'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Char('j'), KeyModifiers::NONE); // past "+ open project", "continue" onto the first project
    let header = a.side_sel;
    key(&mut a, KeyCode::Char('l'), KeyModifiers::NONE);
    assert_eq!(a.side_sel, header + 1);
    key(&mut a, KeyCode::Char('h'), KeyModifiers::NONE);
    assert_eq!(a.side_sel, header);
    key(&mut a, KeyCode::Char('/'), KeyModifiers::NONE);
    for c in "just".chars() {
        key(&mut a, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let s = draw(&mut a);
    assert!(s.contains("justgains") && !s.contains("dotfiles"), "{s}");
    // rename the selected live session
    key(&mut a, KeyCode::Char('j'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Char('r'), KeyModifiers::NONE);
    assert!(a.renaming.is_some());
    for c in "timer fix".chars() {
        key(&mut a, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(a.live_infos().iter().any(|l| l.name.as_deref() == Some("timer fix")));
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!a.side_focus);
}

#[test]
fn prefix_table_and_splits() {
    let mut a = app(true);
    let _ = draw(&mut a);
    let n0 = a.tabs[a.cur].root.leaf_ids().len();
    key(&mut a, KeyCode::Char(' '), KeyModifiers::CONTROL);
    assert!(a.prefix_armed);
    let s = draw(&mut a);
    assert!(s.contains("ctrl+space"), "{s}");
    key(&mut a, KeyCode::Char('z'), KeyModifiers::NONE);
    assert!(a.tabs[a.cur].zoom);
    key(&mut a, KeyCode::Char('z'), KeyModifiers::ALT);
    assert!(!a.tabs[a.cur].zoom);
    // alt+left / right move between the split panes of the first tab
    let f0 = a.focused();
    key(&mut a, KeyCode::Right, KeyModifiers::ALT);
    assert_ne!(a.focused(), f0);
    key(&mut a, KeyCode::Left, KeyModifiers::ALT);
    assert_eq!(a.focused(), f0);
    assert_eq!(a.tabs[a.cur].root.leaf_ids().len(), n0);
}

#[test]
fn mouse_clicks_sidebar_rows_and_drags_dividers() {
    let mut a = app(true);
    let _ = draw(&mut a);
    // click the row of the blocked (justgains) session
    let rows = a.rows();
    let i = rows.iter().position(|r| matches!(r, crate::sidebar::Row::Live { info, .. } if info.activity == Some(Activity::Blocked))).unwrap();
    let (r, _) = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::Row(x) if *x == i)).cloned().unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 4, row: r.y, modifiers: KeyModifiers::NONE });
    assert_eq!(a.focused().and_then(|id| a.panes.get(&id)).and_then(|p| p.activity()), Some(Activity::Blocked));
    // back to the split tab; drag its divider
    a.cur = 0;
    let _ = draw(&mut a);
    let before = a.tabs[0].root.ratio_at(&[]).unwrap();
    let mid = a.outer[0].1.right();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: mid - 1, row: a.outer[0].1.y + 5, modifiers: KeyModifiers::NONE });
    a.mouse(MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), column: mid + 10, row: a.outer[0].1.y + 5, modifiers: KeyModifiers::NONE });
    a.mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: mid + 10, row: a.outer[0].1.y + 5, modifiers: KeyModifiers::NONE });
    assert!(a.tabs[0].root.ratio_at(&[]).unwrap() > before);
}

#[test]
fn selection_copies_from_the_rendered_buffer() {
    let mut a = app(true);
    let _ = draw(&mut a);
    let (_, inner) = a.inner[0];
    let ev = |kind, x, y| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
    a.mouse(ev(MouseEventKind::Down(MouseButton::Left), inner.x, inner.y + 1));
    a.mouse(ev(MouseEventKind::Drag(MouseButton::Left), inner.x + 10, inner.y + 1));
    a.mouse(ev(MouseEventKind::Up(MouseButton::Left), inner.x + 10, inner.y + 1));
    let _ = draw(&mut a);
    assert!(a.toasts.latest().is_some_and(|t| t.text.starts_with("copied")), "{:?}", a.toasts.latest());
}

#[test]
fn bridge_create_targets() {
    use super::sessions::create_target;
    use bro_core::Harness;
    assert_eq!(create_target(Some("shell")), None);
    assert_eq!(create_target(None), None);
    assert_eq!(create_target(Some("pi")), Some((Harness::Pi, None)));
    assert_eq!(create_target(Some("claude")), Some((Harness::Claude, None)));
    assert_eq!(create_target(Some("codex:team")), Some((Harness::Codex, Some("codex:team".into()))));
    assert_eq!(create_target(Some("claude:work")), Some((Harness::Claude, Some("claude:work".into()))));
    assert_eq!(create_target(Some("weird")), None);
}

#[test]
fn bridge_commands_drive_sessions() {
    let mut a = app(true);
    let _ = draw(&mut a);
    let sid = |a: &App, id: PaneId| a.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap();
    let order = crate::sidebar::live_order(&a.live_infos(), &a.past_infos(), &a.open_infos());
    let target = order[3];
    a.bridge_command(bro_bridge::BridgeCommand::Rename { id: sid(&a, target), title: "from phone".into() });
    assert!(a.live_infos().iter().any(|l| l.name.as_deref() == Some("from phone")));
    a.bridge_command(bro_bridge::BridgeCommand::Focus { id: sid(&a, target) });
    assert_eq!(a.focused(), Some(target));
    let n = a.panes.len();
    a.bridge_command(bro_bridge::BridgeCommand::Kill { id: sid(&a, target) });
    assert_eq!(a.panes.len(), n - 1);
}

#[test]
fn launched_sessions_open_panes_and_report_errors() {
    let mut a = app(false);
    let spec = bro_core::launch::LaunchSpec {
        harness: bro_core::Harness::Codex,
        profile_id: Some("codex:local".into()),
        provider_id: None,
        model: Some("gpt-5.2".into()),
        cwd: std::env::temp_dir(),
        resume: None,
        permission: bro_core::launch::Permission::Default,
        browser: bro_core::browser::BrowserMode::Off,
        extra_args: vec![],
    };
    let bad = crate::services::Launched { spec: spec.clone(), place: Place::Tab, reply: None, name: None, remember: false, result: Err("no key".into()), route_id: None, note: None };
    a.launched(bad);
    assert!(a.tabs.is_empty());
    assert!(a.toasts.latest().unwrap().text.contains("no key"));
    let cmd = bro_core::launch::CommandSpec { program: "definitely-not-a-program-xyz".into(), label: "codex · local · gpt-5.2".into(), cwd: std::env::temp_dir(), ..Default::default() };
    let ok = crate::services::Launched { spec, place: Place::Tab, reply: None, name: None, remember: true, result: Ok(cmd), route_id: None, note: None };
    a.launched(ok);
    assert_eq!(a.tabs.len(), 1);
    assert_eq!(a.recents.len(), 1);
    // spawning a missing program shows the error in the pane instead of crashing
    let s = draw(&mut a);
    assert!(s.contains("local · gpt-5.2"), "{s}");
    assert!(s.contains("couldn't start") || s.contains("definitely-not"), "{s}");
}

#[test]
fn usage_block_collapses_to_claude_and_codex_totals() {
    let mut a = app(true);
    assert!(!a.usage_expanded);
    let s = shot(&mut a, "usage-collapsed");
    let foot: Vec<&str> = s.lines().skip_while(|l| !l.contains("usage left")).take(4).collect();
    let foot = foot.join("\n");
    assert!(foot.contains("claude") && foot.contains("codex"), "totals\n{s}");
    assert!(!foot.contains("personal"), "no per-profile rows when collapsed\n{s}");
    a.usage_expanded = true; // (toggle_usage_details also saves; tests don't touch settings)
    let s = shot(&mut a, "usage-expanded");
    assert!(s.contains("personal") && s.contains("team"), "every profile\n{s}");
}

#[test]
fn earlier_sessions_live_in_the_continue_picker_not_the_sidebar() {
    use crate::continue_picker::ContinuePicker;
    use crate::sidebar::Row;
    let mut a = app(true);
    // the sidebar only has running sessions, plus one "continue" entry
    let rows = a.rows();
    assert!(rows.iter().any(|r| matches!(r, Row::Continue { .. })));
    let s = draw(&mut a);
    assert!(s.contains("continue session") && !s.contains("port the oriel event loop"), "{s}");
    // alt+r opens the picker for the current project
    key(&mut a, KeyCode::Char('r'), KeyModifiers::ALT);
    let count = |a: &App| match &a.overlay {
        Overlay::Continue(p) => {
            let p: &ContinuePicker = p;
            p.view().len()
        }
        _ => panic!("continue picker"),
    };
    let before = count(&a);
    assert!(before > 0);
    let s = shot(&mut a, "continue-picker");
    assert!(s.contains("continue a session"), "{s}");
    // del archives, ctrl+z brings it back
    key(&mut a, KeyCode::Delete, KeyModifiers::NONE);
    assert_eq!(count(&a), before - 1);
    key(&mut a, KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(count(&a), before);
    // tab: every project
    key(&mut a, KeyCode::Tab, KeyModifiers::NONE);
    assert!(count(&a) >= before);
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::None));
}

#[test]
fn resuming_offers_every_login_of_that_kind() {
    use crate::sidebar::Row;
    use super::overlays::{ResumeFrom, ResumePicker};
    let mut a = app(true);
    // an earlier claude session from the continue picker: Enter asks which login, current one first
    key(&mut a, KeyCode::Char('r'), KeyModifiers::ALT);
    let Overlay::Continue(c) = &mut a.overlay else { panic!("continue picker") };
    c.all = true;
    let pos = c.view().iter().position(|e| e.harness == bro_core::Harness::Claude && e.login.is_some()).unwrap();
    c.sel = pos;
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let Overlay::Resume(p) = &a.overlay else { panic!("resume picker") };
    let p: &ResumePicker = p;
    assert!(matches!(p.from, ResumeFrom::Past(_)));
    assert!(p.targets[0].current);
    assert!(p.targets.len() >= 3 && p.targets.iter().all(|t| t.profile_id.starts_with("claude:")));
    let s = shot(&mut a, "resume-picker");
    assert!(s.contains("resume in") && s.contains("current") && s.contains("% left"), "{s}");
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    a.run_act(Act::FocusSidebar);
    // a running claude session: f moves it to another login
    let live = a.rows().iter().position(|r| matches!(r, Row::Live { info, .. } if info.harness == Some(bro_core::Harness::Claude))).unwrap();
    a.side_sel = live;
    key(&mut a, KeyCode::Char('f'), KeyModifiers::NONE);
    let Overlay::Resume(p) = &a.overlay else { panic!("switch picker") };
    assert!(matches!(p.from, ResumeFrom::Live { .. }));
    assert!(!p.targets[p.sel].current, "starts on another login");
}

#[test]
fn nearly_empty_login_preselects_the_roomiest_other() {
    use super::overlays::ResumeTarget;
    let t = |id: &str, left: f64, current: bool| ResumeTarget { profile_id: id.into(), name: id.into(), detail: String::new(), left: Some(left), current };
    assert_eq!(App::resume_default(&[t("a", 50.0, true), t("b", 90.0, false)]), 0, "enough left: stay");
    assert_eq!(App::resume_default(&[t("a", 4.0, true), t("b", 90.0, false), t("c", 30.0, false)]), 1);
}

#[test]
fn projects_are_opened_explicitly_and_new_sessions_start_in_the_current_one() {
    use crate::sidebar::Row;
    let mut a = app(true);
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("fresh");
    std::fs::create_dir_all(&project).unwrap();
    // o opens the folder picker; a typed path opens that folder
    a.run_act(Act::FocusSidebar);
    key(&mut a, KeyCode::Char('o'), KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::Folder(_)));
    a.paste(&project.to_string_lossy());
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::None));
    assert!(a.open_projects.contains(&a.svc.project_for(&project).root));
    assert!(a.rows().iter().any(|r| matches!(r, Row::Project { name, .. } if name == "fresh")));
    // it's current: the launcher starts there
    key(&mut a, KeyCode::Char('n'), KeyModifiers::ALT);
    let Overlay::Launcher(l) = &a.overlay else { panic!("launcher") };
    assert_eq!(l.dir, a.svc.project_for(&project).root);
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    // x on the project takes it off the list
    a.side_focus = true;
    a.side_sel = a.rows().iter().position(|r| matches!(r, Row::Project { name, .. } if name == "fresh")).unwrap();
    key(&mut a, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(!a.rows().iter().any(|r| matches!(r, Row::Project { name, .. } if name == "fresh")));
}

fn usage_launch_app() -> (App, std::sync::mpsc::Receiver<Event>) {
    testkit::isolate();
    let (tx, rx) = std::sync::mpsc::channel();
    let svc = Services::offline(fallback_settings(), tx.clone());
    (App::new(svc, tx, Opts { demo: true, fixed_demo: true, load_recents: false }), rx)
}

fn usage_launch_result(rx: &std::sync::mpsc::Receiver<Event>) -> Box<crate::services::Launched> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("usage click launches a session") {
            Event::Launched(result) => return result,
            _ => {}
        }
    }
}

#[test]
fn usage_rows_launch_the_clicked_account_or_harness_in_the_current_project() {
    use bro_core::Harness;
    for (h, profile) in [(Harness::Claude, None), (Harness::Codex, None), (Harness::Claude, Some("claude:work")), (Harness::Codex, Some("codex:team"))] {
        let (mut a, rx) = usage_launch_app();
        let project = tempfile::tempdir().unwrap();
        a.add_project(project.path().to_path_buf());
        a.usage_expanded = profile.is_some();
        draw(&mut a);
        let r = a.side_hits.iter().find_map(|(r, hit)| match hit {
            SideHit::LaunchUsage(agent, id) if *agent == h && id.as_deref() == profile => Some(*r),
            _ => None,
        }).unwrap();
        a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 2, row: r.y, modifiers: KeyModifiers::NONE });
        let launch = usage_launch_result(&rx);
        assert_eq!(launch.spec.harness, h);
        assert_eq!(launch.spec.profile_id.as_deref(), profile.or(Some(if h == Harness::Claude { "claude:local" } else { "codex:local" })));
        assert_eq!(launch.spec.cwd, project.path());
        assert!(launch.spec.resume.is_none());
        assert!(launch.spec.provider_id.is_none());
        assert_eq!(launch.place, Place::Tab);
        assert!(matches!(a.overlay, Overlay::None));
        let count = a.panes.len();
        a.handle(Event::Launched(launch));
        assert_eq!(a.panes.len(), count + 1);
    }
}

#[test]
fn usage_launch_starts_in_the_focused_sessions_project() {
    use bro_core::Harness;
    let (mut a, rx) = usage_launch_app();
    let picked = tempfile::tempdir().unwrap();
    a.add_project(picked.path().to_path_buf());
    // a project picked in the sidebar holds while focus stays put
    a.follow_focus();
    assert_eq!(a.preferred_dir().as_deref(), Some(a.svc.project_for(picked.path()).root.as_path()));
    // moving focus onto a session makes its project the one new sessions start in
    let other = a.panes.iter()
        .filter_map(|(id, p)| p.as_term_ref().map(|t| (*id, t.meta.project.root.clone())))
        .find(|(id, _)| Some(*id) != a.focused())
        .expect("demo has a second session");
    a.focus_pane(other.0);
    a.side_focus = false;
    draw(&mut a);
    let r = a.side_hits.iter().find_map(|(r, hit)| matches!(hit, SideHit::LaunchUsage(Harness::Claude, None)).then_some(*r)).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 2, row: r.y, modifiers: KeyModifiers::NONE });
    assert_eq!(usage_launch_result(&rx).spec.cwd, other.1);
}

#[test]
fn usage_disclosure_stays_separate_from_launch_and_full_usage_cards_launch() {
    let (mut a, rx) = usage_launch_app();
    // Make the Fable allowance visible, then verify the arrow is its own hit target.
    a.svc.update(|st| {
        let u = st.usage.get_mut("claude:work").unwrap().usage.as_mut().unwrap();
        u.scoped.push(("Fable".into(), bro_core::usage::Window { used_pct: 20.0, resets_at: None, window_mins: None }));
    });
    draw(&mut a);
    let arrow = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::Fable)).unwrap().0;
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: arrow.x, row: arrow.y, modifiers: KeyModifiers::NONE });
    assert!(a.show_fable);
    assert!(!rx.try_iter().any(|e| matches!(e, Event::Launched(_))));
    let heading = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::UsageToggle)).unwrap().0;
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: heading.x, row: heading.y, modifiers: KeyModifiers::NONE });
    assert!(a.usage_expanded);
    let expected_dir = a.preferred_dir().unwrap();
    a.open_view("usage");
    draw(&mut a);
    let inner = a.inner.iter().find(|(id, _)| Some(*id) == a.focused()).unwrap().1;
    // Third account card: work, including its meter rows rather than only the title.
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: inner.x + 8, row: inner.y + 2 + 2 * 5 + 1, modifiers: KeyModifiers::NONE });
    let launch = usage_launch_result(&rx);
    assert_eq!(launch.spec.profile_id.as_deref(), Some("claude:work"));
    assert_eq!(launch.spec.cwd, expected_dir);
}

#[test]
fn terminal_links_open_on_click_but_dragging_and_shift_keep_selection() {
    use crate::panes::term::Term;
    for mouse_mode in [false, true] {
        let mut a = app(true);
        let (tx, rx) = std::sync::mpsc::channel();
        a.tx = tx;
        let id = a.focused().unwrap();
        let meta = a.panes[&id].as_term_ref().unwrap().meta.clone();
        let mode = if mouse_mode { "\x1b[?1000h\x1b[?1006h" } else { "" };
        let output = format!("{mode}\x1b]8;;https://example.com/docs?a=1&b=2\x1b\\Read docs\x1b]8;;\x1b\\\r\nhttps://example.org/plain");
        a.panes.insert(id, Box::new(Term::fixed(meta, &output, a.svc.clone())));
        a.tabs[a.cur].zoom = true;
        a.side_focus = false;
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let inner = a.inner.iter().find(|(pane, _)| *pane == id).unwrap().1;
        assert!(term.backend().buffer()[(inner.x, inner.y)].modifier.contains(ratatui::style::Modifier::UNDERLINED));
        assert!(term.backend().buffer()[(inner.x, inner.y + 1)].modifier.contains(ratatui::style::Modifier::UNDERLINED));
        let pos = Position { x: inner.x + 2, y: inner.y };
        let event = |kind, pos: Position, modifiers| MouseEvent { kind, column: pos.x, row: pos.y, modifiers };
        let opened = || rx.try_iter().filter_map(|ev| if let Event::OpenLink(uri) = ev { Some(uri) } else { None }).collect::<Vec<_>>();
        let modifier = if mouse_mode { KeyModifiers::CONTROL } else { KeyModifiers::NONE };

        if mouse_mode {
            a.mouse(event(MouseEventKind::Down(MouseButton::Left), pos, KeyModifiers::NONE));
            a.mouse(event(MouseEventKind::Up(MouseButton::Left), pos, KeyModifiers::NONE));
            assert!(opened().is_empty(), "unmodified clicks remain available to mouse-aware programs");
        }
        a.mouse(event(MouseEventKind::Down(MouseButton::Left), pos, modifier));
        assert!(opened().is_empty(), "a press alone must not open a link");
        a.mouse(event(MouseEventKind::Up(MouseButton::Left), pos, modifier));
        assert_eq!(opened(), ["https://example.com/docs?a=1&b=2"]);
        assert!(!a.copy_pending);

        let plain = Position { y: pos.y + 1, ..pos };
        a.mouse(event(MouseEventKind::Down(MouseButton::Left), plain, modifier));
        a.mouse(event(MouseEventKind::Up(MouseButton::Left), plain, modifier));
        assert_eq!(opened(), ["https://example.org/plain"]);

        a.mouse(event(MouseEventKind::Down(MouseButton::Left), pos, modifier));
        a.mouse(event(MouseEventKind::Drag(MouseButton::Left), Position { x: pos.x + 4, ..pos }, modifier));
        a.mouse(event(MouseEventKind::Up(MouseButton::Left), pos, modifier));
        assert!(opened().is_empty(), "a drag that returns to its starting cell is still a selection");
        assert!(a.copy_pending);
        let _ = draw(&mut a);

        a.mouse(event(MouseEventKind::Down(MouseButton::Left), pos, KeyModifiers::SHIFT));
        a.mouse(event(MouseEventKind::Up(MouseButton::Left), pos, KeyModifiers::SHIFT));
        assert!(opened().is_empty(), "shift reserves text selection");
        a.run_act(Act::Help);
        a.mouse(event(MouseEventKind::Down(MouseButton::Left), pos, modifier));
        a.mouse(event(MouseEventKind::Up(MouseButton::Left), pos, modifier));
        assert!(opened().is_empty(), "clicks on an overlay must not open links underneath");
    }
}

#[cfg(windows)]
#[test]
#[ignore = "spawns a local PowerShell in ConPTY; no accounts or network"]
fn conpty_output_keeps_named_hyperlinks_clickable() {
    use crate::panes::term::{Spawn, Term};
    let mut a = app(true);
    let (tx, rx) = std::sync::mpsc::channel();
    a.tx = tx;
    let dir = tempfile::tempdir().unwrap();
    let id = a.focused().unwrap();
    let mut meta = a.panes[&id].as_term_ref().unwrap().meta.clone();
    meta.cwd = dir.path().to_path_buf();
    meta.harness = None;
    meta.route_id = None;
    meta.cleanup.clear();
    let script = "[Console]::Write([char]27 + ']8;;https://example.com/native' + [char]7 + 'NATIVE-LINK' + [char]27 + ']8;;' + [char]7); Start-Sleep -Seconds 5";
    let spawn = Spawn {
        program: "powershell.exe".into(),
        args: vec!["-NoLogo".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script.into()],
        env: ["HOME", "USERPROFILE", "BRO_DIR", "CLAUDE_POOL_DIR", "BRO_CODEX_PROFILES_DIR"].into_iter()
            .map(|name| (name.into(), dir.path().to_string_lossy().into_owned())).collect(),
        ..Spawn::default()
    };
    a.panes.insert(id, Box::new(Term::new(meta, spawn, a.svc.clone())));
    a.tabs[a.cur].zoom = true;
    a.side_focus = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut hit = None;
    while Instant::now() < deadline {
        let _ = draw(&mut a);
        let inner = a.inner.iter().find(|(pane, _)| *pane == id).unwrap().1;
        hit = (0..inner.height).find_map(|row| (0..inner.width).find_map(|col| {
            (a.panes[&id].link_at(row, col).as_deref() == Some("https://example.com/native"))
                .then_some(Position { x: inner.x + col, y: inner.y + row })
        }));
        if hit.is_some() { break; }
        std::thread::sleep(Duration::from_millis(30));
    }
    let opened = hit.map(|pos| {
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
            a.mouse(MouseEvent { kind, column: pos.x, row: pos.y, modifiers: KeyModifiers::CONTROL });
        }
        rx.try_iter().any(|event| matches!(event, Event::OpenLink(ref uri) if uri == "https://example.com/native"))
    });
    a.shutdown();
    assert_eq!(opened, Some(true), "real ConPTY output must retain the hidden target through rendering and clicking");
}

#[test]
fn clipboard_shortcuts_and_terminal_paste_open_a_copied_project_path() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("copied project 日本語");
    std::fs::create_dir(&project).unwrap();
    let copied = format!("\"{}\"\r\n", project.display());
    let shortcuts = [
        None,
        Some((KeyCode::Char('v'), KeyModifiers::CONTROL)),
        Some((KeyCode::Char('V'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)),
        Some((KeyCode::Insert, KeyModifiers::SHIFT)),
    ];
    for shortcut in shortcuts {
        let mut a = app(false);
        a.open_folder();
        a.paste("old search");
        if let Some((code, modifiers)) = shortcut {
            key(&mut a, code, modifiers);
            let request = a.clipboard_request.expect("shortcut starts a clipboard read");
            a.handle(Event::ClipboardText(request, Some(copied.clone())));
        } else {
            a.handle(Event::Input(crossterm::event::Event::Paste(copied.clone())));
        }
        key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(a.overlay, Overlay::None));
        assert!(a.open_projects.contains(&a.svc.project_for(&project).root));
    }
}

#[test]
fn delayed_clipboard_reads_do_not_paste_into_a_new_popup() {
    let mut a = app(false);
    a.open_folder();
    key(&mut a, KeyCode::Char('v'), KeyModifiers::CONTROL);
    let request = a.clipboard_request.unwrap();
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    a.open_folder();
    a.handle(Event::ClipboardText(request, Some("C:\\wrong-project".into())));
    let Overlay::Folder(p) = &a.overlay else { panic!("folder picker") };
    assert!(p.filter.is_empty(), "a dismissed input must not receive a late paste");
}

#[test]
fn paste_stays_in_the_focused_editor_or_terminal() {
    use std::cell::RefCell;
    use std::rc::Rc;
    struct Receiver(Rc<RefCell<Vec<String>>>);
    impl Pane for Receiver {
        fn title(&self) -> String { "paste receiver".into() }
        fn render(&mut self, _: &mut ratatui::Frame, _: Rect, _: &mut Cx) {}
        fn key(&mut self, _: KeyEvent, _: &mut Cx) -> bool { true }
        fn paste(&mut self, text: &str, _: &mut Cx) { self.0.borrow_mut().push(text.into()); }
        fn is_terminal(&self) -> bool { true }
    }
    let mut a = app(false);
    let received = Rc::new(RefCell::new(vec![]));
    let id = a.new_tab(Box::new(Receiver(received.clone())));
    a.side_focus = false;
    key(&mut a, KeyCode::Char('v'), KeyModifiers::CONTROL);
    a.handle(Event::ClipboardText(a.clipboard_request.unwrap(), Some("hello\n世界".into())));
    assert_eq!(*received.borrow(), vec!["hello\n世界"]);
    received.borrow_mut().clear();

    a.run_act(Act::Help);
    a.paste("must not reach the terminal");
    a.overlay = Overlay::None;
    a.renaming = Some((id, "new ".into()));
    a.paste("name\r\n");
    assert_eq!(a.renaming.take().unwrap().1, "new name");
    a.side_focus = true;
    a.side_filtering = true;
    a.paste("project\n");
    assert_eq!(a.side.filter, "project");
    a.side_filtering = false;
    a.paste("sidebar has no active input");
    a.run_act(Act::Palette);
    a.paste("theme ocean");
    assert_eq!(a.theme.name, "ocean", "pasted palette search preserves preview");
    assert!(received.borrow().is_empty(), "UI paste must never leak through to a terminal");
}

#[test]
fn notifications_are_compact_and_top_right() {
    let mut a = app(false);
    a.toast(Kind::Info, "older notification");
    a.toast(Kind::Info, "newest notification");
    let s = shot(&mut a, "notifications-top-right");
    let rows: Vec<_> = s.lines().collect();
    assert!(rows[1].contains("newest notification"), "{s}");
    assert!(rows[2].contains("older notification"), "{s}");
    assert!(rows[1].find("newest notification").unwrap() > W as usize / 2);
    assert!(rows[3..].iter().all(|row| !row.contains("notification")));
    for (width, height) in [(8, 3), (20, 5), (60, 12), (100, 30)] {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        term.draw(|f| a.draw_toasts(f, f.area())).unwrap();
    }
}

#[test]
fn launcher_works_with_the_mouse() {
    use crate::launcher::{Focus, Hit, Item};
    let mut a = app(true);
    key(&mut a, KeyCode::Char('n'), KeyModifiers::ALT);
    let _ = draw(&mut a);
    let click = |a: &mut App, want: &dyn Fn(&Hit) -> bool| {
        let Overlay::Launcher(l) = &a.overlay else { panic!("launcher closed") };
        let (r, _) = *l.hits.iter().find(|(_, h)| want(h)).expect("hit");
        a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE });
        let _ = draw(a);
    };
    // the codex tab
    click(&mut a, &|h| *h == Hit::Harness(bro_core::Harness::Codex));
    let Overlay::Launcher(l) = &a.overlay else { panic!() };
    assert_eq!(l.harness, bro_core::Harness::Codex);
    // the openrouter row: first click selects, second opens the model list
    let row = {
        let items = l.items();
        let sel: Vec<&Item> = items.iter().filter(|i| !matches!(i, Item::Header(_))).collect();
        sel.iter().position(|i| matches!(i, Item::Account(acc) if acc.label == "openrouter")).unwrap()
    };
    click(&mut a, &|h| *h == Hit::Row(row));
    click(&mut a, &|h| *h == Hit::Row(row));
    let Overlay::Launcher(l) = &a.overlay else { panic!() };
    assert_eq!(l.focus, Focus::Models);
    // permission toggle by click
    click(&mut a, &|h| *h == Hit::Perm(bro_core::launch::Permission::Auto));
    let Overlay::Launcher(l) = &a.overlay else { panic!() };
    assert_eq!(l.permission, bro_core::launch::Permission::Auto);
    // a model: click selects, clicking it again launches
    click(&mut a, &|h| *h == Hit::Model(1));
    click(&mut a, &|h| *h == Hit::Model(1));
    assert!(matches!(a.overlay, Overlay::None), "launched");
    // clicking outside the launcher closes it
    key(&mut a, KeyCode::Char('n'), KeyModifiers::ALT);
    let _ = draw(&mut a);
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 0, modifiers: KeyModifiers::NONE });
    assert!(matches!(a.overlay, Overlay::None));
}

#[test]
fn themes_render_graphite_by_default_and_paper() {
    let mut a = app(true);
    assert_eq!(a.theme.name, "graphite");
    let _ = shot(&mut a, "theme-graphite");
    a.theme = crate::theme::get("paper");
    let s = shot(&mut a, "theme-paper");
    assert!(s.contains("bro-cli-v2"));
}

#[test]
fn shift_click_stacks_sessions_and_a_plain_click_unstacks() {
    use crate::sidebar::Row;
    let mut a = app(true);
    let _ = draw(&mut a);
    let live: Vec<usize> = a.rows().iter().enumerate().filter(|(_, r)| matches!(r, Row::Live { .. })).map(|(i, _)| i).collect();
    let click = |a: &mut App, i: usize, shift: bool| {
        let _ = draw(a);
        let (r, _) = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::Row(x) if *x == i)).cloned().unwrap();
        let modifiers = if shift { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 4, row: r.y, modifiers });
    };
    click(&mut a, live[0], false);
    click(&mut a, live[3], true);
    assert!(a.stacked(), "first session + the shift-clicked one");
    click(&mut a, live[4], true);
    assert_eq!(a.visible().len(), 3);
    let s = shot(&mut a, "stacked");
    assert_eq!(a.outer.len(), 3, "three panes drawn\n{s}");
    // shift-click one again: out of the stack
    click(&mut a, live[4], true);
    assert_eq!(a.stack.len(), 2);
    // a plain click shows just that session
    click(&mut a, live[1], false);
    assert!(!a.stacked());
    assert_eq!(a.visible().len(), 1);
}

#[test]
fn launcher_has_a_close_button() {
    use crate::launcher::Hit;
    let mut a = app(true);
    key(&mut a, KeyCode::Char('n'), KeyModifiers::ALT);
    let s = draw(&mut a);
    assert!(s.contains(" × "), "{s}");
    let Overlay::Launcher(l) = &a.overlay else { panic!() };
    let (r, _) = *l.hits.iter().find(|(_, h)| *h == Hit::Close).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE });
    assert!(matches!(a.overlay, Overlay::None));
}

#[test]
fn projects_close_from_their_x_with_a_confirm_when_sessions_run() {
    use crate::sidebar::Row;
    let mut a = app(true);
    let _ = draw(&mut a);
    // a project with running sessions asks first, then closes them with it
    let (r, root) = a.side_hits.iter().find_map(|(r, h)| if let SideHit::CloseProject(root, live) = h { (*live > 0).then(|| (*r, root.clone())) } else { None }).expect("project ×");
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x, row: r.y, modifiers: KeyModifiers::NONE });
    assert!(matches!(a.overlay, Overlay::Confirm(_)));
    key(&mut a, KeyCode::Char('y'), KeyModifiers::NONE);
    let key_ = a.svc.project_for(&root).key;
    assert!(!a.rows().iter().any(|r| matches!(r, Row::Project { key, .. } if *key == key_)), "project gone");
    assert!(!a.live_infos().iter().any(|l| l.project_key == key_), "its sessions closed");
}

#[test]
fn new_session_strip_above_usage() {
    let mut a = app(true);
    let s = shot(&mut a, "new-session-strip");
    assert!(s.contains("  +  ") && s.contains("alt+n"), "{s}");
    let hit = |a: &App, want: SideHit| a.side_hits.iter().find(|(_, h)| format!("{h:?}") == format!("{want:?}")).map(|x| x.0).expect("strip button");
    let click = |a: &mut App, r: ratatui::layout::Rect| a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE });
    // the strip sits right above the usage block
    let usage = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::UsageToggle)).map(|x| x.0);
    let term = hit(&a, SideHit::Quick(None));
    if let Some(u) = usage {
        assert_eq!(term.y + 2, u.y, "strip right above usage, a blank line between");
    }
    // hovering an icon names it where the key hint was
    a.mouse(MouseEvent { kind: MouseEventKind::Moved, column: term.x + 2, row: term.y, modifiers: KeyModifiers::NONE });
    assert!(draw(&mut a).contains("new terminal"));
    // terminal: a new shell tab
    let n = a.panes.len();
    click(&mut a, term);
    assert_eq!(a.panes.len(), n + 1);
    // "+": the launcher
    let _ = draw(&mut a);
    let plus = hit(&a, SideHit::Launcher);
    click(&mut a, plus);
    assert!(matches!(a.overlay, Overlay::Launcher(_)));
    a.overlay = Overlay::None;
    // codex: launches straight away, or the launcher opens on codex when there's still a choice to make
    let _ = draw(&mut a);
    let codex = hit(&a, SideHit::Quick(Some(bro_core::Harness::Codex)));
    click(&mut a, codex);
    if let Overlay::Launcher(l) = &a.overlay {
        assert_eq!(l.harness, bro_core::Harness::Codex);
    }
}
