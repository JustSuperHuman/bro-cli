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
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 5, row: r.y, modifiers: KeyModifiers::NONE });
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
fn tiled_minimize_button_and_sidebar_visibility_keep_only_the_latest_minimized() {
    let mut a = app(true);
    a.run_act(Act::TileAll);
    let before: Vec<_> = a.tabs.iter().map(|t| format!("{:?}", t.root)).collect();
    assert!(draw(&mut a).contains("minimized 0"));
    assert_eq!(a.pane_minimize.len(), 5);
    let first = a.pane_minimize[0];
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: first.0.x + 1, row: first.0.y, modifiers: KeyModifiers::NONE });
    assert_eq!(a.minimized, vec![first.1]);
    assert!(!a.stack.contains(&first.1));
    assert_eq!(a.stack.len(), 4);
    let screen = draw(&mut a);
    assert!(screen.contains("minimized 1"));
    assert!(a.outer.iter().all(|(id, _)| *id != first.1));

    let minimized = a.side_hits.iter().find_map(|(r, hit)| matches!(hit, SideHit::Minimized).then_some(*r)).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: minimized.x + 1, row: minimized.y, modifiers: KeyModifiers::NONE });
    assert!(a.show_minimized);
    assert_eq!(a.stack.len(), 5);
    draw(&mut a);
    assert!(a.outer.iter().any(|(id, _)| *id == first.1));

    let second = a.pane_minimize.iter().find(|(_, id)| *id != first.1).copied().unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: second.0.x + 1, row: second.0.y, modifiers: KeyModifiers::NONE });
    assert_eq!(a.minimized, vec![second.1], "visible mode releases earlier minimized sessions");
    assert!(!a.show_minimized, "a new minimization returns to hidden mode");
    assert_eq!(a.stack.len(), 4);
    assert!(a.stack.contains(&first.1));
    assert!(!a.stack.contains(&second.1));
    a.toggle_minimized_visibility();
    assert!(a.show_minimized);
    assert_eq!(a.stack.len(), 5);
    a.toggle_minimized_visibility();
    assert!(!a.show_minimized);
    assert_eq!(a.stack.len(), 4);
    assert!(a.stack.contains(&first.1));
    assert!(!a.stack.contains(&second.1));
    a.go_session(second.1);
    assert!(a.minimized.is_empty(), "selecting a hidden session restores it");
    assert_eq!(a.stack.len(), 5);
    a.run_act(Act::TileAll);
    assert!(!a.tile_all);
    assert_eq!(before, a.tabs.iter().map(|t| format!("{:?}", t.root)).collect::<Vec<_>>());
}

#[test]
fn all_tiled_sessions_can_be_minimized_and_shown_again() {
    let mut a = app(true);
    a.run_act(Act::TileAll);
    let ids = a.stack.clone();
    for id in &ids {
        a.minimize_pane(*id);
    }
    assert!(a.tile_all && a.stack.is_empty());
    assert_eq!(a.minimized.len(), ids.len());
    assert!(a.visible().is_empty());
    assert!(draw(&mut a).contains("All sessions minimized"));
    a.toggle_minimized_visibility();
    assert_eq!(a.visible().len(), ids.len());
    assert_eq!(a.minimized.len(), ids.len());
    a.toggle_minimized_visibility();
    a.go_session(ids[0]);
    assert_eq!(a.visible(), vec![ids[0]]);
    assert_eq!(a.minimized.len(), ids.len() - 1);
}

#[test]
fn a_new_session_joins_the_grid_when_the_others_are_minimized() {
    use crate::panes::term::Term;
    let mut a = app(true);
    a.run_act(Act::TileAll);
    let ids = a.stack.clone();
    let meta = a.panes[&ids[0]].as_term_ref().unwrap().meta.clone();
    for id in &ids {
        a.minimize_pane(*id);
    }
    let added = a.new_tab(Box::new(Term::fixed(meta, "new session", a.svc.clone())));
    assert!(a.tile_all);
    assert_eq!(a.visible(), vec![added]);
    assert_eq!(a.minimized.len(), ids.len());
    a.close(added);
    assert!(a.tile_all && a.visible().is_empty());
    a.toggle_minimized_visibility();
    assert_eq!(a.visible().len(), ids.len());
    a.close(ids[0]);
    assert!(!a.minimized.contains(&ids[0]));
    assert_eq!(a.visible().len(), ids.len() - 1);
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
            assert_eq!(buf[(area.x + 2, area.y)].symbol(), "−");
            let head = &buf[(area.x + 4, area.y)];
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
        assert_ne!(a.project_color("a"), Color::Reset);
        // the folder name alone decides: same name, same color, wherever it lives and whatever else is open
        assert_eq!(a.project_color("f:/work/justgains"), a.project_color("c:/users/james/JustGains"));
        assert_eq!(a.project_color("f:/work/justgains"), crate::theme::project_color("d:/elsewhere/JUSTGAINS", &crate::theme::get(name)));
        assert_ne!(a.project_color("f:/work/justgains"), a.project_color("f:/work/bro-cli-v2"));
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
fn ctrl_clicking_a_session_grows_it_and_snaps_back() {
    let mut a = app(true);
    a.run_act(Act::TileAll);
    draw(&mut a);
    let base = a.outer.clone();
    assert!(base.len() >= 3);
    let ids = |v: &[(PaneId, Rect)]| v.iter().map(|x| x.0).collect::<Vec<_>>();
    let at = |a: &App, id: PaneId| a.outer.iter().find(|(p, _)| *p == id).unwrap().1;
    let ctrl = |a: &mut App, r: Rect| a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 8, row: r.y, modifiers: KeyModifiers::CONTROL });
    let (small, r) = *base.iter().min_by_key(|(_, r)| r.width as u32 * r.height as u32).unwrap();
    ctrl(&mut a, r);
    assert_eq!((a.priority, a.focused()), (Some(small), Some(small)), "ctrl+click grows and focuses");
    let screen = draw(&mut a);
    let grown = at(&a, small);
    assert!(grown.width > r.width || grown.height > r.height, "{r:?} -> {grown:?}");
    assert_eq!(ids(&a.outer), ids(&base), "no session moves to another slot");
    assert!(screen.contains("grown ·") && screen.contains("restores"), "hint names the snap-back key");
    // again: back to exactly what it was
    ctrl(&mut a, grown);
    assert_eq!(a.priority, None);
    draw(&mut a);
    assert_eq!(a.outer, base);
    // ctrl+click another session: the priority moves there
    ctrl(&mut a, r);
    draw(&mut a);
    let (other, _) = *base.iter().find(|(p, _)| *p != small).unwrap();
    let r2 = at(&a, other);
    ctrl(&mut a, r2);
    a.after_events();
    assert_eq!(a.priority, Some(other));
    draw(&mut a);
    // a plain click elsewhere focuses that session and the grown one snaps back
    let (third, _) = *base.iter().find(|(p, _)| *p != small && *p != other).unwrap();
    let r3 = at(&a, third);
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r3.x + 8, row: r3.y, modifiers: KeyModifiers::NONE });
    a.after_events();
    assert_eq!((a.priority, a.focused()), (None, Some(third)));
    draw(&mut a);
    assert_eq!(a.outer, base);
}

#[test]
fn ctrl_up_grows_the_focused_session_and_moving_on_snaps_it_back() {
    let mut a = app(true);
    a.run_act(Act::TileAll);
    draw(&mut a);
    let base = a.outer.clone();
    let focus = a.focused().unwrap();
    key(&mut a, KeyCode::Up, KeyModifiers::CONTROL);
    a.after_events();
    assert_eq!(a.priority, Some(focus), "ctrl+up grows the focused session");
    draw(&mut a);
    key(&mut a, KeyCode::Up, KeyModifiers::CONTROL);
    a.after_events();
    assert_eq!(a.priority, None, "ctrl+up again snaps back");
    draw(&mut a);
    assert_eq!(a.outer, base);
    key(&mut a, KeyCode::Up, KeyModifiers::CONTROL);
    key(&mut a, KeyCode::Down, KeyModifiers::CONTROL);
    a.after_events();
    assert_eq!(a.priority, None, "ctrl+down snaps back");
    // Grown, then move to another session: the grow resets.
    key(&mut a, KeyCode::Up, KeyModifiers::CONTROL);
    a.after_events();
    assert_eq!(a.priority, Some(focus));
    a.run_act(Act::NextSession);
    a.after_events();
    assert_ne!(a.focused(), Some(focus));
    assert_eq!(a.priority, None, "moving to another session resets the grow");
    draw(&mut a);
    assert_eq!(a.outer, base);
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
    click(&mut a, r.x + 5, r.y);
    release(&mut a, r.x + 5, r.y);
    assert_eq!(a.focused(), Some(other));
    assert!(!a.stack_zoom);
    // A second click on the header: full screen.
    click(&mut a, r.x + 5, r.y);
    release(&mut a, r.x + 5, r.y);
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
    click(&mut a, r.x + 5, r.y);
    a.last_pane_click = a.last_pane_click.map(|(at, id, pos)| (at - std::time::Duration::from_millis(600), id, pos));
    click(&mut a, r.x + 5, r.y);
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
            args: vec!["-NoLogo".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), "Write-Output 'boom: no daemon'; Start-Sleep -Milliseconds 300; exit 7".into()],
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
        let toasts: Vec<String> = a.toasts.items.iter().map(|t| t.text.clone()).collect();
        a.shutdown();
        assert!(removed, "an exited {harness:?} process must not wait for ConPTY output EOF");
        assert!(toasts.iter().any(|t| t.contains("exited: boom: no daemon")), "a quick failure says why: {toasts:?}");
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
    assert!(s.contains("+ New  ↻ Continue"), "session actions
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
    assert!(s.contains("Profiles · Claude and Codex logins") && s.contains("claude:personal"), "{s}");
    assert!(s.contains("Remove claude:personal?") && s.contains("accounts"), "confirm names the dir\n{s}");
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

/// Draw at w×h and keep the buffer (to look at single cells).
fn draw_buf(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).expect("backend");
    term.draw(|f| app.draw(f)).expect("draw");
    term.backend().buffer().clone()
}

/// The sidebar's right border column, top to bottom.
fn side_edge(a: &App, buf: &ratatui::buffer::Buffer) -> String {
    let x = a.side_area.right() - 1;
    (a.side_area.y..a.side_area.bottom()).map(|y| buf[(x, y)].symbol().to_string()).collect()
}

#[test]
fn sidebar_scrollbar_shows_while_navigating_then_hides() {
    let mut a = app(true);
    a.run_act(Act::FocusSidebar);
    let (w, h) = (120, 22);
    let buf = draw_buf(&mut a, w, h);
    assert!(a.side_thumb.is_empty() && !side_edge(&a, &buf).contains('┃'), "hidden until something moves");
    // keys: down to the end scrolls the list; the thumb sits on the border at the bottom
    key(&mut a, KeyCode::End, KeyModifiers::NONE);
    let buf = draw_buf(&mut a, w, h);
    assert!(a.side_scroll > 0, "the demo list overflows {h} rows");
    let thumb = a.side_thumb;
    assert!(thumb.height >= 1 && thumb.x == a.side_area.right() - 1, "{thumb:?}");
    assert!(side_edge(&a, &buf).contains('┃'));
    // home: back at the top, the thumb moves up
    key(&mut a, KeyCode::Home, KeyModifiers::NONE);
    let _ = draw_buf(&mut a, w, h);
    assert!(a.side_thumb.y < thumb.y, "{:?} vs {thumb:?}", a.side_thumb);
    // a second after the last move it's gone, and the loop doesn't need to wake for it
    a.side_nav_at = Instant::now().checked_sub(Duration::from_secs(2));
    let buf = draw_buf(&mut a, w, h);
    assert!(a.side_thumb.is_empty() && !side_edge(&a, &buf).contains('┃'));
    assert_eq!(side_view::bar_wake(a.side_nav_at), None);
    // the wheel shows it too
    let r = a.side_area;
    a.mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: r.x + 4, row: r.y + 3, modifiers: KeyModifiers::NONE });
    let _ = draw_buf(&mut a, w, h);
    assert!(!a.side_thumb.is_empty());
    assert!(side_view::bar_wake(a.side_nav_at).is_some_and(|d| d <= Duration::from_secs(1)), "the loop wakes to hide it");
}

#[test]
fn sidebar_scrollbar_fades_then_hides() {
    use side_view::bar_alpha;
    assert_eq!(bar_alpha(Duration::ZERO), Some(1.0));
    assert!(bar_alpha(Duration::from_millis(900)).is_some_and(|a| a > 0.0 && a < 1.0));
    assert_eq!(bar_alpha(Duration::from_millis(1100)), None);
}

#[test]
fn sidebar_session_actions_stay_visible_and_clickable_after_scrolling() {
    let mut a = app(true);
    let positions = |a: &App| {
        a.side_hits.iter().filter_map(|(r, hit)| {
            match hit {
                SideHit::Launcher => Some((0, *r)),
                SideHit::Continue => Some((1, *r)),
                _ => None,
            }
        }).collect::<Vec<_>>()
    };
    draw_buf(&mut a, 140, 20);
    let initial = positions(&a);
    assert_eq!(initial.len(), 2);
    a.side_focus = true;
    key(&mut a, KeyCode::End, KeyModifiers::NONE);
    draw_buf(&mut a, 140, 20);
    assert!(a.side_scroll > 0);
    assert_eq!(positions(&a), initial);
    assert!(a.side_thumb.bottom() <= initial[0].1.y, "the scrollbar belongs to the project viewport");
    crate::testkit::snapshot("sidebar-session-actions", 140, 20, |f| a.draw(f));
    let open = initial[0].1;
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: open.x + 2, row: open.y, modifiers: KeyModifiers::NONE });
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.tab == crate::launcher::SessionTab::New));
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    let history = initial[1].1;
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: history.x + 2, row: history.y, modifiers: KeyModifiers::NONE });
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.tab == crate::launcher::SessionTab::Continue));
}

#[test]
fn sidebar_actions_remain_available_while_filtering_and_in_short_windows() {
    let mut a = app(true);
    a.side_focus = true;
    let initial = draw_buf(&mut a, 140, 24);
    let action = |a: &App| a.side_hits.iter().find_map(|(r, hit)| matches!(hit, SideHit::Continue).then_some(*r)).unwrap();
    let position = action(&a);
    key(&mut a, KeyCode::Char('/'), KeyModifiers::NONE);
    for c in "justgains".chars() {
        key(&mut a, KeyCode::Char(c), KeyModifiers::NONE);
    }
    let filtered = draw_buf(&mut a, 140, 24);
    assert_eq!(action(&a), position);
    assert!(matches!(a.rows().get(a.side_sel), Some(crate::sidebar::Row::Project { .. })), "filtering selects the first matching project");
    assert!(crate::testkit::dump(&initial).contains("Continue") && crate::testkit::dump(&filtered).contains("Continue"));
    a.side.filter = "no matching project".into();
    assert!(crate::testkit::dump(&draw_buf(&mut a, 140, 24)).contains("nothing matches"));
    a.side.filter.clear();
    a.side_filtering = false;
    for height in [2, 3, 4, 5, 8, 12, 22] {
        a.side_scroll = 50;
        a.side_manual_scroll = true;
        a.side_nav();
        draw_buf(&mut a, 140, height);
    }
}

#[test]
fn sidebar_wheel_keeps_its_viewport_when_keyboard_focus_stays_in_the_sidebar() {
    let mut a = app(true);
    a.side_focus = true;
    a.side_sel = 0;
    draw_buf(&mut a, 120, 22);
    let row = a.side_hits.iter().find_map(|(r, hit)| matches!(hit, SideHit::Row(0)).then_some(*r)).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: row.x + 3, row: row.y, modifiers: KeyModifiers::NONE });
    draw_buf(&mut a, 120, 22);
    assert!(a.side_scroll > 0, "wheel scrolling must not snap back to the selected row");
    assert!(a.side_focus, "scrolling must not send subsequent keys to an agent");
    let scrolled = a.side_scroll;
    draw_buf(&mut a, 120, 22);
    assert_eq!(a.side_scroll, scrolled, "the viewport stays where the wheel left it");
    key(&mut a, KeyCode::Home, KeyModifiers::NONE);
    draw_buf(&mut a, 120, 22);
    assert_eq!(a.side_scroll, 0, "keyboard navigation reveals its selection again");
}

#[test]
fn sidebar_wheel_works_over_the_gaps_between_projects() {
    let mut a = app(true);
    draw_buf(&mut a, 120, 22);
    let rows = a.rows();
    let gap = a.side_hits.iter().find_map(|(r, hit)| {
        let SideHit::Row(i) = hit else { return None };
        let pos = ratatui::layout::Position { x: r.x + 3, y: r.y.saturating_sub(1) };
        (matches!(rows.get(*i), Some(crate::sidebar::Row::Project { .. })) && !a.side_hits.iter().any(|(hit, _)| hit.contains(pos))).then_some(pos)
    }).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: gap.x, row: gap.y, modifiers: KeyModifiers::NONE });
    draw_buf(&mut a, 120, 22);
    assert!(a.side_scroll > 0, "blank space in the sidebar must receive wheel events");
}

#[test]
fn sidebar_scroll_and_terminal_paint_do_not_wait_for_the_output_parser() {
    let mut a = app(true);
    a.side_focus = true;
    a.side_sel = 0;
    let before = draw_buf(&mut a, 120, 22);
    let id = a.focused().unwrap();
    let area = a.inner.iter().find(|(pane, _)| *pane == id).unwrap().1;
    let symbols = |buf: &ratatui::buffer::Buffer| {
        (area.y..area.bottom()).flat_map(|y| (area.x..area.right()).map(move |x| buf[(x, y)].symbol().to_string())).collect::<Vec<_>>()
    };
    let parser = a.panes[&id].as_term_ref().unwrap().parser_for_test();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _guard = parser.lock();
        ready_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(2)).is_ok()
    });
    ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let row = a.side_hits.iter().find_map(|(r, hit)| matches!(hit, SideHit::Row(0)).then_some(*r)).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: row.x + 3, row: row.y, modifiers: KeyModifiers::NONE });
    a.after_events();
    let during = draw_buf(&mut a, 120, 22);
    let _ = release_tx.send(());
    assert!(worker.join().unwrap(), "the UI waited for the terminal parser worker to release its lock");
    assert!(a.side_scroll > 0);
    assert_eq!(symbols(&during), symbols(&before), "keep the last terminal frame while its parser is busy");
}

#[test]
fn sidebar_scrollbar_stays_hidden_when_everything_fits() {
    let mut a = app(true);
    a.run_act(Act::FocusSidebar);
    key(&mut a, KeyCode::End, KeyModifiers::NONE);
    let buf = draw_buf(&mut a, W, 80);
    assert!(a.side_nav_at.is_some() && a.side_scroll == 0);
    assert!(a.side_thumb.is_empty() && !side_edge(&a, &buf).contains('┃'));
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
fn earlier_sessions_live_in_the_continue_tab_not_the_sidebar() {
    let mut a = app(true);
    let s = draw(&mut a);
    assert!(s.contains("Continue") && !s.contains("port the oriel event loop"), "{s}");
    // alt+r opens the shared dialog on Continue, scoped to the current project
    key(&mut a, KeyCode::Char('r'), KeyModifiers::ALT);
    let count = |a: &App| match &a.overlay {
        Overlay::Launcher(l) => l.continue_picker.view().len(),
        _ => panic!("session dialog"),
    };
    let before = count(&a);
    assert!(before > 0);
    let s = shot(&mut a, "continue-picker");
    assert!(s.contains("Continue") && s.contains("All sessions"), "{s}");
    // del archives, ctrl+z brings it back
    key(&mut a, KeyCode::Delete, KeyModifiers::NONE);
    assert_eq!(count(&a), before - 1);
    key(&mut a, KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(count(&a), before);
    // Tab: every project
    key(&mut a, KeyCode::Tab, KeyModifiers::NONE);
    assert!(count(&a) >= before);
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.continue_picker.all));
    draw(&mut a);
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    let scoped = l.hits.iter().find_map(|(r, hit)| matches!(hit, crate::launcher::Hit::ContinueScope(false)).then_some(*r)).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: scoped.x + 1, row: scoped.y, modifiers: KeyModifiers::NONE });
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if !l.continue_picker.all && l.continue_picker.view().len() == before));
    // New Session remains reachable from the same window.
    draw(&mut a);
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    let new_tab = l.hits.iter().find_map(|(r, hit)| (*hit == crate::launcher::Hit::Tab(crate::launcher::SessionTab::New)).then_some(*r)).unwrap();
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: new_tab.x + 2, row: new_tab.y, modifiers: KeyModifiers::NONE });
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.tab == crate::launcher::SessionTab::New));
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::None));
}

#[test]
fn all_sessions_resumes_in_the_selected_sessions_project() {
    use super::overlays::ResumeFrom;
    let (mut a, rx) = usage_launch_app();
    a.open_continue();
    let Overlay::Launcher(l) = &mut a.overlay else { panic!("session dialog") };
    let selected_project = l.continue_picker.project.as_ref().unwrap().0.clone();
    assert!(!l.continue_picker.all);
    assert!(l.continue_picker.view().iter().all(|e| e.project_key == selected_project));
    l.continue_picker.set_all(true);
    let foreign = l.continue_picker.view().iter().position(|e| e.project_key != selected_project && e.harness == bro_core::Harness::Claude).expect("another project's Claude session");
    l.continue_picker.sel = foreign;
    let entry = l.continue_picker.view()[foreign].clone();
    let past = a.svc.state().past.ready().and_then(|sessions| sessions.get(entry.idx).cloned()).unwrap();
    let expected_dir = past.cwd.clone().or_else(|| past.project.as_ref().map(|p| p.root.clone())).unwrap();
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let Overlay::Resume(p) = &mut a.overlay else { panic!("login picker") };
    assert!(matches!(&p.from, ResumeFrom::Past(s) if s.id == entry.id));
    p.sel = p.targets.iter().position(|t| t.current).expect("original login");
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let launch = usage_launch_result(&rx);
    assert_eq!(launch.spec.resume.as_ref().unwrap().session_id, entry.id);
    assert_eq!(launch.spec.cwd, expected_dir);
    assert_eq!(a.svc.project_for(&launch.spec.cwd).key, entry.project_key);
}

#[test]
fn resuming_offers_every_login_of_that_kind() {
    use crate::sidebar::Row;
    use super::overlays::{ResumeFrom, ResumePicker};
    let mut a = app(true);
    // an earlier claude session from the continue picker: Enter asks which login, current one first
    key(&mut a, KeyCode::Char('r'), KeyModifiers::ALT);
    let Overlay::Launcher(l) = &mut a.overlay else { panic!("session dialog") };
    let c = &mut l.continue_picker;
    c.set_all(true);
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
    // The path bar opens the folder picker; a typed path opens that folder.
    a.run_act(Act::FocusSidebar);
    key(&mut a, KeyCode::Char('n'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Char('p'), KeyModifiers::ALT);
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.project_picker.is_some()));
    a.paste(&project.to_string_lossy());
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(a.open_projects.contains(&a.svc.project_for(&project).root));
    assert!(a.rows().iter().any(|r| matches!(r, Row::Project { name, .. } if name == "fresh")));
    // Opening the project immediately offers a session in its directory.
    let Overlay::Launcher(l) = &a.overlay else { panic!("launcher") };
    assert_eq!(l.dir, a.svc.project_for(&project).root);
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    // x on the project takes it off the list
    a.side_focus = true;
    a.side_sel = a.rows().iter().position(|r| matches!(r, Row::Project { name, .. } if name == "fresh")).unwrap();
    key(&mut a, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(!a.rows().iter().any(|r| matches!(r, Row::Project { name, .. } if name == "fresh")));
}

#[test]
fn clicking_a_project_in_the_path_bar_updates_the_session_dialog() {
    let mut a = app(true);
    let project = tempfile::tempdir().unwrap();
    let root = a.svc.project_for(project.path()).root;
    let panes = a.panes.len();
    open_project_picker(&mut a);
    a.paste(&project.path().to_string_lossy());
    draw(&mut a);
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    let p = l.project_picker.as_ref().expect("project picker");
    let r = p.hits.iter().find(|(_, i)| *i == p.sel).unwrap().0;
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE });
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    assert_eq!(l.dir, root);
    assert_eq!(l.place, Place::Tab);
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::None));
    assert!(a.open_projects.contains(&root));
    assert_eq!(a.panes.len(), panes, "cancel does not start a session");
    assert_eq!(a.preferred_dir(), Some(root));
}

#[test]
fn session_dialog_switches_to_an_open_project_without_resetting_the_agent() {
    let mut a = app(true);
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    a.open_projects.roots.clear();
    a.open_projects.add(first.clone());
    a.open_projects.add(second.clone());
    a.pick_project(first.clone());
    a.open_launcher(Some(first.clone()), Place::Tab);
    let rendered = draw(&mut a);
    assert!(rendered.contains("╭ PROJECT") && rendered.contains("Change project  Alt+P") && rendered.contains("first"), "the session dialog shows the selected project in a path box\n{rendered}");
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    let row = l.hits.iter().find_map(|(r, hit)| (*hit == crate::launcher::Hit::Project).then_some(*r)).unwrap();
    let tabs = l.hits.iter().find_map(|(r, hit)| matches!(hit, crate::launcher::Hit::Harness(_)).then_some(*r)).unwrap();
    assert!(row.bottom() < tabs.y, "the project path sits above the agent tabs");
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: row.x + 2, row: row.y, modifiers: KeyModifiers::NONE });
    let chooser = draw(&mut a);
    assert!(chooser.contains("choose project") && chooser.contains("current"));
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    let picker = l.project_picker.as_ref().unwrap();
    let second_row = picker.hits.iter().find_map(|(r, i)| (picker.view()[*i].0 == second).then_some(*r)).unwrap();
    let harness = l.harness;
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: second_row.x + 2, row: second_row.y, modifiers: KeyModifiers::NONE });
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog stays open") };
    assert_eq!(l.dir, second);
    assert_eq!(l.harness, harness);
    assert_eq!(l.spec().unwrap().cwd, second);
    let second_key = a.svc.project_for(&second).key;
    assert_eq!(l.continue_picker.project.as_ref().unwrap().0, second_key);
    assert!(!l.continue_picker.all && l.continue_picker.view().is_empty());
    assert_eq!(a.preferred_dir(), Some(second));
    key(&mut a, KeyCode::Char('r'), KeyModifiers::ALT);
    key(&mut a, KeyCode::Tab, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.continue_picker.all && !l.continue_picker.view().is_empty()));
    let compact = crate::testkit::dump(&draw_buf(&mut a, 66, 16));
    assert!(compact.contains("second") && compact.contains("Change"), "the project path stays visible in a short window\n{compact}");
    a.select_launcher_project(first);
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.continue_picker.all), "all sessions stays global when the path bar changes");
}

#[test]
fn session_dialog_path_opens_a_new_project_after_creation() {
    let (mut a, rx) = folder_creation_app();
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let new_project = root.path().join("new project");
    std::fs::create_dir(&first).unwrap();
    a.open_projects.add(first.clone());
    a.pick_project(first.clone());
    a.open_launcher(Some(first.clone()), Place::Tab);
    key(&mut a, KeyCode::Char('p'), KeyModifiers::ALT);
    a.paste(&new_project.to_string_lossy());
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.project_picker.as_ref().is_some_and(|p| p.pending.is_some())));
    assert!(!new_project.exists());
    finish_folder_creation(&mut a, &rx);
    assert!(new_project.is_dir());
    assert!(a.open_projects.contains(&new_project));
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog stays open") };
    assert!(l.project_picker.is_none());
    assert_eq!(l.spec().unwrap().cwd, new_project);
    assert_eq!(a.preferred_dir(), Some(new_project));
}

fn folder_creation_app() -> (App, std::sync::mpsc::Receiver<Event>) {
    testkit::isolate();
    let (tx, rx) = std::sync::mpsc::channel();
    let svc = Services::offline(fallback_settings(), tx.clone());
    (App::new(svc, tx, Opts { demo: false, fixed_demo: true, load_recents: false }), rx)
}

fn finish_folder_creation(a: &mut App, rx: &std::sync::mpsc::Receiver<Event>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("folder creation finished");
        let finished = matches!(event, Event::FolderCreated(_, _));
        a.handle(event);
        if finished { break; }
    }
}

fn open_project_picker(a: &mut App) {
    a.open_launcher(None, Place::Tab);
    key(a, KeyCode::Char('p'), KeyModifiers::ALT);
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.project_picker.is_some()));
}

fn choose_folder(a: &mut App, mouse: bool) {
    if mouse {
        draw(a);
        let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
        let p = l.project_picker.as_ref().expect("project picker");
        let r = p.hits.iter().find(|(_, i)| *i == p.sel).unwrap().0;
        a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE });
    } else {
        key(a, KeyCode::Enter, KeyModifiers::NONE);
    }
}

#[test]
fn creating_a_project_folder_uses_it_for_the_session_and_launch_command() {
    for mouse in [false, true] {
        for in_repo in [false, true] {
            let (mut a, rx) = folder_creation_app();
            let projects = a.open_projects.roots.clone();
            let dir = tempfile::tempdir().unwrap();
            if in_repo {
                std::fs::create_dir(dir.path().join(".git")).unwrap();
            }
            let project = dir.path().join("new parent").join("project with spaces 日本語");
            open_project_picker(&mut a);
            let suffix = if mouse { std::path::MAIN_SEPARATOR.to_string() } else { String::new() };
            a.paste(&format!("\"{}{suffix}\"", project.display()));
            assert!(draw(&mut a).contains("create and use"));
            assert!(!project.exists(), "typing a path does not create it");
            choose_folder(&mut a, mouse);
            assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.project_picker.as_ref().is_some_and(|p| p.pending.is_some())));
            assert_eq!(a.open_projects.roots, projects, "creation must finish before the project is added");
            finish_folder_creation(&mut a, &rx);
            assert!(project.is_dir(), "missing parents and the selected folder are created");
            let root = a.svc.project_for(&project).root;
            assert!(a.open_projects.contains(&root));
            if in_repo { assert_eq!(root, dir.path()); }
            let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
            assert_eq!(l.dir, project, "a git ancestor must not replace the selected working directory");
            assert_eq!(l.spec().unwrap().cwd, project);
            key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
            let launch = usage_launch_result(&rx);
            assert_eq!(launch.spec.cwd, project);
            assert_eq!(launch.result.unwrap().cwd, project, "the command sent to the PTY uses the created directory");
        }
    }
}

#[test]
fn folder_creation_errors_keep_the_modal_open_and_allow_a_corrected_path() {
    for mouse in [false, true] {
        for blocked_parent in [false, true] {
            let (mut a, rx) = folder_creation_app();
            let projects = a.open_projects.roots.clone();
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("existing file");
            std::fs::write(&file, "keep this content").unwrap();
            let blocked = if blocked_parent { file.join("child") } else { file.clone() };
            open_project_picker(&mut a);
            a.paste(&blocked.to_string_lossy());
            choose_folder(&mut a, mouse);
            if matches!(a.overlay, Overlay::Launcher(ref l) if l.project_picker.as_ref().is_some_and(|p| p.pending.is_some())) {
                finish_folder_creation(&mut a, &rx);
            }
            let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
            let p = l.project_picker.as_ref().expect("a failed folder stays in the picker");
            assert_eq!(p.filter, blocked.to_string_lossy());
            assert!(p.pending.is_none() && p.error.is_some());
            let error = p.error.as_ref().unwrap().clone();
            assert!(draw(&mut a).contains(&error.chars().take(35).collect::<String>()), "the error is visible in the modal");
            assert_eq!(a.open_projects.roots, projects);
            assert!(a.cur_project.is_none());
            assert!(a.panes.is_empty());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep this content");
            key(&mut a, KeyCode::Char('u'), KeyModifiers::CONTROL);
            let valid = dir.path().join("corrected project");
            a.paste(&valid.to_string_lossy());
            choose_folder(&mut a, mouse);
            finish_folder_creation(&mut a, &rx);
            assert!(valid.is_dir());
            assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.dir == valid));
        }
    }
}

#[test]
fn cancelling_folder_creation_ignores_its_completion_in_a_new_modal() {
    let (mut a, rx) = folder_creation_app();
    let projects = a.open_projects.roots.clone();
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("cancelled project");
    open_project_picker(&mut a);
    a.paste(&project.to_string_lossy());
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    let p = l.project_picker.as_ref().expect("creating folder");
    let request = p.pending.unwrap();
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    a.paste("ignored while creating");
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.project_picker.as_ref().is_some_and(|p| p.pending == Some(request) && p.filter == project.to_string_lossy())));
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    open_project_picker(&mut a);
    finish_folder_creation(&mut a, &rx);
    assert!(matches!(a.overlay, Overlay::Launcher(ref l) if l.project_picker.as_ref().is_some_and(|p| p.pending.is_none() && p.error.is_none() && p.filter.is_empty())));
    assert_eq!(a.open_projects.roots, projects, "a dismissed request cannot open a project later");
    assert!(rx.try_iter().all(|event| !matches!(event, Event::FolderCreated(_, _))), "repeated Enter did not start another worker");
}

#[cfg(windows)]
#[test]
#[ignore = "spawns a local PowerShell in ConPTY; no accounts or network"]
fn conpty_session_starts_in_the_folder_created_by_the_modal() {
    let (mut a, rx) = folder_creation_app();
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("native project 日本語");
    let proof = dir.path().join("native-cwd.txt");
    open_project_picker(&mut a);
    a.paste(&project.to_string_lossy());
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    finish_folder_creation(&mut a, &rx);
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let mut launch = usage_launch_result(&rx);
    let cmd = launch.result.as_mut().unwrap();
    assert_eq!(cmd.cwd, project);
    // Keep the modal's launch directory; use a local shell to report the child's actual cwd.
    cmd.program = "powershell.exe".into();
    cmd.args = vec!["-NoLogo".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(),
        format!("[IO.File]::WriteAllText('{}', (Get-Location).ProviderPath)", proof.to_string_lossy().replace('\'', "''"))];
    cmd.env.extend(["HOME", "USERPROFILE"].into_iter().map(|name| (name.into(), dir.path().to_string_lossy().into_owned())));
    a.handle(Event::Launched(launch));
    let deadline = Instant::now() + Duration::from_secs(15);
    let observed = loop {
        draw(&mut a);
        if let Ok(cwd) = std::fs::read_to_string(&proof) { break Some(cwd); }
        if Instant::now() >= deadline { break None; }
        if let Ok(event) = rx.recv_timeout(Duration::from_millis(30)) { a.handle(event); }
    };
    a.shutdown();
    assert_eq!(observed.as_deref(), Some(project.to_str().unwrap()), "the real child process must start in the created directory");
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
        key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
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
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
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
        open_project_picker(&mut a);
        a.paste("old search");
        if let Some((code, modifiers)) = shortcut {
            key(&mut a, code, modifiers);
            let request = a.clipboard_request.expect("shortcut starts a clipboard read");
            a.handle(Event::ClipboardText(request, Some(copied.clone())));
        } else {
            a.handle(Event::Input(crossterm::event::Event::Paste(copied.clone())));
        }
        key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
        assert!(a.open_projects.contains(&a.svc.project_for(&project).root));
        let Overlay::Launcher(l) = &a.overlay else { panic!("opening a pasted project shows the session picker") };
        assert_eq!(l.dir, a.svc.project_for(&project).root);
    }
}

#[test]
fn delayed_clipboard_reads_do_not_paste_into_a_new_popup() {
    let mut a = app(false);
    open_project_picker(&mut a);
    key(&mut a, KeyCode::Char('v'), KeyModifiers::CONTROL);
    let request = a.clipboard_request.unwrap();
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
    open_project_picker(&mut a);
    a.handle(Event::ClipboardText(request, Some("C:\\wrong-project".into())));
    let Overlay::Launcher(l) = &a.overlay else { panic!("session dialog") };
    let p = l.project_picker.as_ref().expect("project picker");
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
    let Overlay::Launcher(l) = &a.overlay else { panic!() };
    assert_eq!(l.permission, bro_core::launch::Permission::Auto, "the next launcher opens on the mode picked last");
    assert_eq!(a.svc.settings().permission, bro_core::launch::Permission::Auto);
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
    assert!(s.contains("+ New  ↻ Continue"), "{s}");
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
    assert!(draw(&mut a).contains("Terminal"));
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

#[test]
fn file_paths_link_once_confirmed_and_open_the_preview_and_agents_can_open_files() {
    use crate::file_view::{Body, Outcome as FileOutcome};
    use crate::panes::term::Term;
    let mut a = app(true);
    let (tx, rx) = std::sync::mpsc::channel();
    a.tx = tx;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/app.rs"), (1..=80).map(|n| format!("let line_{n} = {n};\n")).collect::<String>()).unwrap();
    let id = a.focused().unwrap();
    let mut meta = a.panes[&id].as_term_ref().unwrap().meta.clone();
    meta.cwd = dir.path().to_path_buf();
    meta.project = a.svc.project_for(dir.path());
    let sid = meta.sid.clone();
    // a URL, a real file with a line, a word that looks like a file but isn't one
    let output = "see https://example.com/x.rs and src/app.rs:42 or notes.txt\r\n";
    a.panes.insert(id, Box::new(Term::fixed(meta, output, a.svc.clone())));
    a.tabs[a.cur].zoom = true;
    a.side_focus = false;
    let _ = draw(&mut a);
    let inner = a.inner.iter().find(|(pane, _)| *pane == id).unwrap().1;
    let col = |s: &str| output.find(s).unwrap() as u16;
    assert!(a.panes[&id].link_at(0, col("src/app.rs")).is_none(), "not a link until it's known to be a file");
    a.panes[&id].as_term_ref().unwrap().resolve_file_links_now();
    let _ = draw(&mut a);
    let uri = a.panes[&id].link_at(0, col("src/app.rs") + 3).expect("confirmed files link");
    assert!(uri.starts_with("file:") && uri.ends_with("app.rs#L42"), "{uri}");
    assert!(a.panes[&id].link_at(0, col("notes.txt")).is_none(), "missing files never link");
    assert_eq!(a.panes[&id].link_at(0, col("example.com")).as_deref(), Some("https://example.com/x.rs"), "URLs win");

    // click: the preview opens at line 42 (loaded off the UI thread)
    let pos = Position { x: inner.x + col("src/app.rs") + 2, y: inner.y };
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        a.mouse(MouseEvent { kind, column: pos.x, row: pos.y, modifiers: KeyModifiers::NONE });
    }
    assert!(!rx.try_iter().any(|e| matches!(e, Event::OpenLink(_))), "file links preview in bro, not the OS");
    let pump = |a: &mut App| {
        let ev = rx.recv_timeout(Duration::from_secs(10)).expect("the preview loads");
        a.handle(ev);
    };
    pump(&mut a);
    let Overlay::File(v) = &a.overlay else { panic!("preview open") };
    assert!(matches!(v.body, Body::Text(_)) && v.line == Some(42));
    let screen = draw(&mut a);
    assert!(screen.contains("let line_42 = 42;") && screen.contains("src/app.rs:42"), "{screen}");
    // @ mentions it to the agent and closes
    key(&mut a, KeyCode::Char('@'), KeyModifiers::NONE);
    assert!(!a.overlay.is_open());
    let sent = String::from_utf8_lossy(&a.panes.get_mut(&id).unwrap().as_term().unwrap().sent).into_owned();
    assert_eq!(sent, "@src/app.rs ");

    // an agent's `bro open` in the session you're looking at: the preview opens
    a.bridge_command(bro_bridge::BridgeCommand::OpenFile { id: sid.clone(), path: dir.path().join("src/app.rs"), line: Some(7), column: None });
    assert!(matches!(&a.overlay, Overlay::File(v) if v.line == Some(7)));
    pump(&mut a);
    a.file_view_outcome(FileOutcome::Close);
    // …elsewhere: a toast, and prefix f shows it
    a.side_focus = true;
    a.bridge_command(bro_bridge::BridgeCommand::OpenFile { id: sid, path: dir.path().join("src/app.rs"), line: Some(9), column: None });
    assert!(!a.overlay.is_open());
    assert!(a.toasts.items.iter().any(|t| t.text.contains("opened src/app.rs:9")), "{:?}", a.toasts.items.iter().map(|t| &t.text).collect::<Vec<_>>());
    a.side_focus = false;
    a.run_act(Act::OpenedFile);
    assert!(matches!(&a.overlay, Overlay::File(v) if v.line == Some(9)));
}

#[test]
fn files_without_a_terminal_preview_open_in_the_system_viewer_without_enter() {
    use bro_bridge::file_preview::ResolveRoots;

    let mut a = app(true);
    let (tx, rx) = std::sync::mpsc::channel();
    a.tx = tx;
    let dir = tempfile::tempdir().unwrap();
    let roots = ResolveRoots { cwd: dir.path().to_path_buf(), project: None };

    for (name, bytes) in [("photo.png", &b"image"[..]), ("archive.bin", &b"\0binary"[..])] {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        a.open_file_view(None, roots.clone(), name.into(), None, None, None);
        let Event::FileView(loaded) = rx.recv_timeout(Duration::from_secs(10)).unwrap() else { panic!("file load") };
        a.handle(Event::FileView(loaded));
        assert!(!a.overlay.is_open(), "{name} should open without a second keypress");
        let Event::OpenLink(uri) = rx.recv_timeout(Duration::from_secs(10)).unwrap() else { panic!("system viewer request") };
        assert_eq!(uri, crate::panes::links::file_uri(&path, None, None).unwrap());
    }

    a.open_file_view(None, roots, "missing.pdf".into(), None, None, None);
    let Event::FileView(loaded) = rx.recv_timeout(Duration::from_secs(10)).unwrap() else { panic!("missing file result") };
    a.handle(Event::FileView(loaded));
    assert!(matches!(&a.overlay, Overlay::File(v) if matches!(v.body, crate::file_view::Body::Failed { path: None, .. })));
    assert!(rx.try_recv().is_err(), "a missing file must not open the system viewer");
}

/// A Claude pane over a fixture transcript (no process): `lines` as JSONL, fixed `activity`.
fn insight_pane(a: &mut App, dir: &std::path::Path, lines: &[serde_json::Value], activity: Activity) -> (PaneId, String) {
    use crate::panes::insights::Insights;
    use crate::panes::term::{Spawn, Term};
    let id = a.focused().unwrap();
    let mut meta = a.panes[&id].as_term_ref().unwrap().meta.clone();
    meta.harness = Some(bro_core::Harness::Claude);
    meta.cwd = dir.to_path_buf();
    meta.project = a.svc.project_for(dir);
    meta.name = Some("Pinky".into());
    meta.renamed = true;
    let sid = meta.sid.clone();
    let transcript = dir.join("session.jsonl");
    std::fs::write(&transcript, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    let mut t = Term::fixed(meta, "working on it\r\n", a.svc.clone());
    t.demo_activity = Some(activity);
    t.set_insights(Insights::from_transcript(bro_core::Harness::Claude, &transcript, None));
    t.set_launch(Spawn { program: "claude".into(), ..Spawn::default() });
    a.panes.insert(id, Box::new(t));
    (id, sid)
}

#[test]
fn a_pending_question_tool_means_needs_you_even_when_the_screen_reads_working() {
    use serde_json::json;
    let mut a = app(true);
    let dir = tempfile::tempdir().unwrap();
    let ask = json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[{"header":"Color","question":"Which color do you prefer?"}]}}]}});
    let (id, _) = insight_pane(&mut a, dir.path(), &[ask.clone()], Activity::Working);
    let term = a.panes.get_mut(&id).unwrap().as_term().unwrap();
    // Claude's question menu ends "esc to cancel", which the screen scan reads as working.
    term.set_screen_status(Some(Activity::Working));
    assert_eq!(a.panes[&id].activity(), Some(Activity::Blocked));
    a.after_events();
    assert_eq!(a.notes.get(&id).map(String::as_str), Some("⏸ Question: Color"));

    // Answered: the tool result arrives and the screen's own status is back in charge.
    let answered = json!({"type":"user","timestamp":"2026-10-02T10:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"red"}]}});
    let (id, _) = insight_pane(&mut a, dir.path(), &[ask, answered], Activity::Working);
    a.panes.get_mut(&id).unwrap().as_term().unwrap().set_screen_status(Some(Activity::Working));
    assert_eq!(a.panes[&id].activity(), Some(Activity::Working));
}

#[test]
fn agent_panes_say_why_they_wait_and_list_the_files_they_touched_or_sent() {
    use serde_json::json;
    let mut a = app(true);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/app.rs"), (1..=40).map(|n| format!("let line_{n} = {n};\n")).collect::<String>()).unwrap();
    let p = |s: &str| dir.path().join(s).to_string_lossy().into_owned();
    let sid = "0b6e3c1e-0000-4000-8000-0000000000aa";
    let lines = [
        json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","sessionId":sid,"message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":p("src/app.rs"),"old_string":"a","new_string":"b"}}]}}),
        json!({"type":"user","timestamp":"2026-10-02T10:00:01Z","sessionId":sid,"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"toolUseResult":{"filePath":p("src/app.rs"),"structuredPatch":[{"newStart":12}]}}),
        json!({"type":"assistant","timestamp":"2026-10-02T10:00:02Z","sessionId":sid,"message":{"id":"m2","content":[{"type":"tool_use","id":"t2","name":"SendUserFile","input":{"files":[p("src/app.rs")],"caption":"the change"}}]}}),
        json!({"type":"user","timestamp":"2026-10-02T10:00:03Z","sessionId":sid,"message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"1 file delivered"}]}}),
        json!({"type":"assistant","timestamp":"2026-10-02T10:00:04Z","sessionId":sid,"message":{"id":"m3","content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"npm publish","description":"Publish the package"}}]}}),
    ];
    let (id, _) = insight_pane(&mut a, dir.path(), &lines, Activity::Blocked);
    a.after_events();
    assert_eq!(a.notes.get(&id).map(String::as_str), Some("⏸ Bash: Publish the package"));
    let screen = draw(&mut a);
    assert!(screen.contains("⏸ Bash: Publish the package"), "the sidebar's second line says why it waits:\n{screen}");

    // prefix F: what it sent, then what it touched; enter previews at the line
    a.side_focus = false;
    a.run_act(Act::AgentFiles);
    let Overlay::AgentFiles(v) = &a.overlay else { panic!("agent files open") };
    let rows: Vec<(&str, &str, Option<u32>)> = v.items.iter().map(|i| (i.kind, i.shown.as_str(), i.line)).collect();
    assert_eq!(rows, vec![("sent", "src/app.rs", None), ("edit", "src/app.rs", Some(12))]);
    let screen = draw(&mut a);
    assert!(screen.contains("sent to you") && screen.contains("touched") && screen.contains("src/app.rs:12") && screen.contains("the change"), "{screen}");
    key(&mut a, KeyCode::Char('j'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&a.overlay, Overlay::File(v) if v.line == Some(12)), "the preview opens at the edit");

    // a shell has nothing to list
    a.overlay = Overlay::None;
    let shell = a.panes.keys().copied().find(|k| *k != id && a.panes[k].as_term_ref().is_some_and(|t| t.meta.harness.is_none()));
    if let Some(shell) = shell {
        a.focus_pane(shell);
        a.run_act(Act::AgentFiles);
        assert!(matches!(&a.overlay, Overlay::AgentFiles(v) if v.items.is_empty() && v.empty.contains("shell")));
    }
}

#[test]
fn a_recap_is_generated_once_for_a_wait_nobody_looked_at() {
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut a = app(true);
    let dir = tempfile::tempdir().unwrap();
    let sid = "0b6e3c1e-0000-4000-8000-0000000000bb";
    let lines = [json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","sessionId":sid,"message":{"id":"m1","content":[{"type":"text","text":"Refactored the parser. Tests pass."}]}})];
    let (id, _) = insight_pane(&mut a, dir.path(), &lines, Activity::Idle);
    a.done.insert(id);
    let runs = std::sync::Arc::new(AtomicUsize::new(0));
    let seen = runs.clone();
    a.recaps.runner = std::sync::Arc::new(move |job| {
        assert_eq!(job.session_id, "0b6e3c1e-0000-4000-8000-0000000000bb");
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(("You refactored the parser; next is the release.".into(), Some(0.02)))
    });
    // the last message is the recap until a generated one exists
    a.update_insights();
    assert_eq!(a.notes.get(&id).map(String::as_str), Some("↳ Refactored the parser."));
    // looking at it: never
    a.term_focused = true;
    a.recaps.skew = Duration::from_secs(200);
    a.update_insights();
    assert_eq!(runs.load(Ordering::SeqCst), 0, "not while you're looking at it");
    // unseen, but not for 180 s since you looked
    a.term_focused = false;
    a.recaps.skew = Duration::from_secs(300);
    a.update_insights();
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the clock restarts when you looked");
    a.recaps.skew = Duration::from_secs(400);
    a.update_insights();
    assert_eq!(runs.load(Ordering::SeqCst), 1, "one recap after 180 s unseen");
    a.update_insights(); // the next batch (the pane's wake) shows it
    assert_eq!(a.notes.get(&id).map(String::as_str), Some("↳ You refactored the parser; next is the release."));
    a.recaps.skew = Duration::from_secs(4000);
    a.update_insights();
    assert_eq!(runs.load(Ordering::SeqCst), 1, "once per wait");
    let view = a.pane_view(id).unwrap();
    assert_eq!(view.recap.map(|r| r.source), Some(bro_core::recap::Source::Generated));
}

/// A fixed pane sized by its tile like a live one, watched by `viewers` remote clients.
fn remote_pane(a: &mut App, transcript: &str, viewers: Option<usize>) -> PaneId {
    use crate::panes::term::Term;
    let mut meta = a.panes[&a.focused().unwrap()].as_term_ref().unwrap().meta.clone();
    meta.sid = format!("remote-{}", a.panes.len()); // its own bridge session
    let mut t = Term::fixed(meta, transcript, a.svc.clone());
    t.test_sized = true;
    t.test_viewers = viewers;
    a.new_tab(Box::new(t))
}

fn term_of(a: &mut App, id: PaneId) -> &mut crate::panes::term::Term {
    a.panes.get_mut(&id).unwrap().as_term().unwrap()
}

/// The text of tile row `row` of pane `id` as drawn.
fn tile_row(a: &mut App, id: PaneId, row: u16) -> String {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    term.draw(|f| a.draw(f)).unwrap();
    let inner = a.inner.iter().find(|(i, _)| *i == id).unwrap().1;
    let buf = term.backend().buffer();
    (inner.x..inner.right()).map(|x| buf[(x, inner.y + row)].symbol().to_string()).collect::<String>().trim_end().to_string()
}

#[test]
fn a_remotely_watched_session_gets_a_minimum_size_and_the_tile_shows_its_bottom_left() {
    let mut a = app(true);
    let id = remote_pane(&mut a, "", Some(1));
    draw(&mut a);
    let screen = draw(&mut a); // the header is laid out before the pane resizes in a frame
    let inner = a.inner.iter().find(|(i, _)| *i == id).unwrap().1;
    assert!(inner.height < 50 && inner.width > 80, "the test tile is short and wide: {inner:?}");
    assert_eq!(term_of(&mut a, id).size(), (50, inner.width), "rows grow to the minimum, the wide tile keeps its width");
    assert!(screen.contains(&format!("⇲ {}×50 remote", inner.width)), "the header says why:\n{screen}");

    // Fill the taller screen: the tile shows its last rows, the first ones are above it.
    let lines: String = (1..=120).map(|n| format!("line {n:03}\r\n")).collect();
    term_of(&mut a, id).feed(lines.as_bytes());
    term_of(&mut a, id).feed(b"see https://example.com/native here\r\n Do you want to proceed?\r\n \xe2\x9d\xaf 1. Yes\r\n   2. No\r\n");
    let hidden = 50 - inner.height; // the cursor sits on the last screen row
    let last = inner.height - 1;
    assert_eq!(tile_row(&mut a, id, last), "", "the cursor's (empty) row is the tile's last");
    assert_eq!(tile_row(&mut a, id, last - 1), "   2. No");
    assert_eq!(tile_row(&mut a, id, last - 4), "see https://example.com/native here");
    // Screen row `hidden` is the tile's top row; the rows above it are not drawn.
    let first_shown: u16 = 120 - (50 - 5 - hidden) + 1; // 5 rows of prompt + cursor below the numbered lines
    assert_eq!(tile_row(&mut a, id, 0), format!("line {first_shown:03}"));

    // Every tile-relative lookup is shifted by the hidden rows: links, prompt options, clicks.
    let col = |s: &str, needle: &str| s.find(needle).unwrap() as u16;
    let url_row = last - 4;
    assert_eq!(
        a.panes[&id].link_at(url_row, col("see https://example.com/native here", "example") ).as_deref(),
        Some("https://example.com/native")
    );
    term_of(&mut a, id).demo_activity = Some(Activity::Blocked);
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: inner.x + 6, row: inner.y + last - 1, modifiers: KeyModifiers::NONE });
    assert_eq!(std::mem::take(&mut term_of(&mut a, id).sent), b"2\r", "the option on the clicked tile row");
    term_of(&mut a, id).demo_activity = None;
    // A program that tracks the mouse gets screen rows, not tile rows.
    term_of(&mut a, id).feed(b"\x1b[?1000h\x1b[?1006h");
    draw(&mut a);
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: inner.x + 2, row: inner.y, modifiers: KeyModifiers::NONE });
    let sent = String::from_utf8(std::mem::take(&mut term_of(&mut a, id).sent)).unwrap();
    assert_eq!(sent, format!("\x1b[<0;3;{}M", hidden + 1), "tile row 0 is screen row {hidden}");
    term_of(&mut a, id).feed(b"\x1b[?1000l\x1b[?1006l");

    // Scrolling up first reveals the hidden screen rows, then the scrollback.
    let wheel = |a: &mut App| a.mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: inner.x + 2, row: inner.y + 2, modifiers: KeyModifiers::NONE });
    wheel(&mut a);
    assert_eq!(tile_row(&mut a, id, 0), format!("line {:03}", first_shown - 3));
    for _ in 0..((hidden as usize + 3) / 3 + 1) {
        wheel(&mut a);
    }
    let top = tile_row(&mut a, id, 0);
    let n: u16 = top.trim_start_matches("line ").parse().unwrap();
    assert!(n < first_shown - hidden, "past the screen's top, into the scrollback: {top}");
}

#[test]
fn the_remote_minimum_needs_a_viewer_outlives_it_briefly_and_can_be_turned_off() {
    let mut a = app(true);
    let id = remote_pane(&mut a, "", Some(0));
    draw(&mut a);
    let inner = a.inner.iter().find(|(i, _)| *i == id).unwrap().1;
    let tile = (inner.height, inner.width);
    assert_eq!(term_of(&mut a, id).size(), tile, "nobody watches: the tile's size");
    assert!(!draw(&mut a).contains("remote"));

    term_of(&mut a, id).test_viewers = Some(1);
    draw(&mut a);
    assert_eq!(term_of(&mut a, id).size(), (50, inner.width));
    term_of(&mut a, id).test_viewers = Some(0);
    draw(&mut a);
    assert_eq!(term_of(&mut a, id).size(), (50, inner.width), "held through the grace period");
    term_of(&mut a, id).test_skew = std::time::Duration::from_secs(16);
    draw(&mut a);
    assert_eq!(term_of(&mut a, id).size(), tile, "back to the tile after it");

    // 0 turns an axis off; the other still applies.
    a.svc.update(|st| {
        st.settings.bridge.remote_min_rows = 0;
        st.settings.bridge.remote_min_cols = inner.width + 20;
    });
    term_of(&mut a, id).test_viewers = Some(1);
    draw(&mut a);
    let screen = draw(&mut a);
    assert_eq!(term_of(&mut a, id).size(), (inner.height, inner.width + 20));
    assert!(screen.contains(&format!("⇲ {}×{} remote", inner.width + 20, inner.height)), "{screen}");
}

#[test]
fn hidden_panes_get_the_remote_minimum_without_being_drawn() {
    let mut a = app(true);
    let id = remote_pane(&mut a, "", Some(0));
    draw(&mut a);
    let inner = a.inner.iter().find(|(i, _)| *i == id).unwrap().1;
    // Put another tab in front: this pane is no longer drawn.
    let mut meta = a.panes[&id].as_term_ref().unwrap().meta.clone();
    meta.sid = "another-session".into();
    let other = a.new_tab(Box::new(crate::panes::term::Term::fixed(meta, "", a.svc.clone())));
    draw(&mut a);
    assert!(!a.visible().contains(&id) && a.visible().contains(&other));
    // A phone opens the hidden session: the tick that polls hidden panes grows it.
    term_of(&mut a, id).test_viewers = Some(1);
    a.handle(Event::Wake(id.into()));
    assert_eq!(term_of(&mut a, id).size(), (50, inner.width));
    // A remote resize request for a hidden pane is its new local size, still grown to the minimum.
    let sid = term_of(&mut a, id).meta.sid.clone();
    a.bridge_command(bro_bridge::BridgeCommand::Resize { id: sid, cols: 60, rows: 20 });
    assert_eq!(term_of(&mut a, id).size(), (50, 80));
    term_of(&mut a, id).test_viewers = Some(0);
    term_of(&mut a, id).test_skew = std::time::Duration::from_secs(16);
    a.handle(Event::Wake(id.into()));
    assert_eq!(term_of(&mut a, id).size(), (20, 60), "the phone left: what was asked for");
}

#[test]
fn usage_info_icon_opens_token_stats_in_two_tabs() {
    use bro_core::profiles::{Profile, ProfileKind};
    let (mut a, rx) = usage_launch_app();
    // logins in a tempdir: the demo's point at the real ~/.claude and ~/.codex
    let home = tempfile::tempdir().unwrap();
    let now = crate::util::now_secs();
    let iso = |s: i64| chrono::DateTime::from_timestamp(s, 0).unwrap().to_rfc3339();
    let claude = home.path().join("claude");
    std::fs::create_dir_all(claude.join("projects").join("p")).unwrap();
    std::fs::write(claude.join("stats-cache.json"), serde_json::json!({ "modelUsage": { "m": { "inputTokens": 9_000_000_000u64 } }, "lastComputedDate": "2099-01-01" }).to_string()).unwrap();
    std::fs::write(claude.join("projects").join("p").join("s.jsonl"), format!("{}\n", serde_json::json!({ "timestamp": iso(now - 60), "requestId": "r", "message": { "id": "m", "usage": { "input_tokens": 1_500_000, "output_tokens": 0 } } }))).unwrap();
    let codex = home.path().join("codex");
    std::fs::create_dir_all(codex.join("sessions")).unwrap();
    let u = serde_json::json!({ "input_tokens": 2_000_000, "cached_input_tokens": 0, "output_tokens": 500_000 });
    std::fs::write(codex.join("sessions").join("rollout.jsonl"), format!("{}\n", serde_json::json!({ "timestamp": iso(now - 60), "payload": { "type": "token_count", "info": { "total_token_usage": u, "last_token_usage": u } } }))).unwrap();
    let login = |id: &str, kind, dir: &std::path::Path| Profile { id: id.into(), kind, name: "local".into(), dir: dir.to_path_buf(), authenticated: true, plan: None, tier: None, identity: None, email: None };
    a.svc.update(|st| st.profiles = crate::services::Avail::Ready(vec![login("claude:local", ProfileKind::ClaudeLocal, &claude), login("codex:local", ProfileKind::CodexLocal, &codex)]));

    draw(&mut a);
    let info = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::Stats)).expect("ⓘ on usage left").0;
    let heading = a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::UsageToggle)).unwrap().0;
    assert!(heading.contains(ratatui::layout::Position { x: info.x, y: info.y }), "the ⓘ sits on the usage header");
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: info.x, row: info.y, modifiers: KeyModifiers::NONE });
    assert!(!a.usage_expanded, "the ⓘ doesn't toggle the usage block");
    let id = a.focused().unwrap();
    assert_eq!(a.panes[&id].view(), Some("stats"));

    draw(&mut a); // starts the scan
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("the scan finishes") {
            Event::Wake(w) if w.id == id => {
                w.acknowledge();
                break;
            }
            _ => {}
        }
    }
    let s = draw(&mut a);
    assert!(s.contains("lifetime") && s.contains("9.00B") && s.contains("2.50M"), "{s}");
    key(&mut a, KeyCode::Tab, KeyModifiers::NONE);
    let s = draw(&mut a);
    assert!(s.contains("24h") && s.contains("1.50M") && s.contains("4.00M"), "{s}");
}

#[test]
fn settings_view_switches_presets_and_overrides_switches() {
    let mut a = app(false);
    a.run_act(Act::Settings);
    let id = a.focused().expect("settings pane");
    assert_eq!(a.panes[&id].view(), Some("settings"));
    let screen = draw(&mut a);
    assert!(screen.contains("Auto-resume") && screen.contains("Standard") && screen.contains("Usage telemetry"), "{screen}");
    // bro's own options come first: auto-approve (off by default) and recaps toggle in place
    assert!(screen.contains("Auto-approve prompts"), "{screen}");
    let recaps = a.svc.settings().insights.generate_recaps;
    for _ in 0..4 {
        key(&mut a, KeyCode::Down, KeyModifiers::NONE);
    }
    assert!(!a.svc.settings().auto_approve);
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(a.svc.settings().auto_approve);
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // recaps
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(a.svc.settings().insights.generate_recaps, !recaps);
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // compression
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // caveman mode
    assert!(!draw(&mut a).contains("⛏ caveman"), "no badge while off");
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE); // off → caveman
    assert_eq!(a.svc.settings().caveman.active(), Some(bro_core::caveman::Level::Caveman));
    { let s = draw(&mut a); assert!(s.contains("⛏ caveman"), "the sidebar shows it: {s}"); }
    key(&mut a, KeyCode::Left, KeyModifiers::NONE); // back to off
    assert_eq!(a.svc.settings().caveman.active(), None);
    assert!(a.svc.settings().caveman.used);
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // subagents
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!a.svc.settings().caveman.subagents);
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // JustTools: the demo never installs or looks for it
    { let s = draw(&mut a); assert!(s.contains("integrations") && s.contains("not installed"), "{s}"); }
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE); // not installed: opens Quick Install with it ticked
    { let s = draw(&mut a); assert!(s.contains("QUICK INSTALL") && s.contains("[x] JustTools") && s.contains(".NET 11 SDK"), "{s}"); }
    key(&mut a, KeyCode::Char('i'), KeyModifiers::NONE); // the demo installs nothing
    a.run_act(Act::Settings); // back to settings, still on the JustTools row
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // Quick install
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // its skill
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(a.svc.settings().justtools.skill);
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!a.svc.settings().justtools.skill);
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // auto-update: on unless you turn it off
    assert!(a.svc.settings().justtools.auto_update);
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!a.svc.settings().justtools.auto_update);
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // Claude preset
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE); // Standard → Quiet
    assert_eq!(a.svc.settings().claude_env.preset, "quiet");
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // error reports: preset says off
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE); // → Claude default, an override
    assert_eq!(a.svc.settings().claude_env.summary(), "Quiet + 1 change");
    let screen = shot(&mut a, "settings");
    assert!(screen.contains("+ 1 change"), "{screen}");
    // the launch preview sits below the last switch
    key(&mut a, KeyCode::End, KeyModifiers::NONE);
    let screen = draw(&mut a);
    assert!(screen.contains("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY=1"), "{screen}");
    key(&mut a, KeyCode::Home, KeyModifiers::NONE);
    for _ in 0..13 {
        key(&mut a, KeyCode::Down, KeyModifiers::NONE); // back to error reports
    }
    key(&mut a, KeyCode::Char('r'), KeyModifiers::NONE);
    assert_eq!(a.svc.settings().claude_env.summary(), "Quiet");
    a.run_palette(crate::palette::Cmd::ClaudePreset("presenter".into()));
    assert_eq!(a.svc.settings().claude_env.preset, "presenter");
}

#[test]
fn settings_show_a_justtools_update_its_progress_and_how_it_ended() {
    use crate::services::justtools::{Job, JobKind};
    use bro_core::justtools::{Installed, Step};
    let mut a = app(false);
    a.run_act(Act::Settings);
    let _ = draw(&mut a);
    a.svc.update(|st| {
        st.justtools.installed = Some(Installed { bin: std::env::temp_dir(), version: Some("2.8.0".into()) });
        st.justtools.latest = Some("2.9.0".into());
    });
    let s = draw(&mut a);
    assert!(s.contains("2.8.0 → 2.9.0") && s.contains("enter updates"), "{s}");
    a.svc.update(|st| st.justtools.job = Some(Job { kind: JobKind::Update, step: Step::Download, started: Instant::now() }));
    let s = draw(&mut a);
    assert!(s.contains("updating") && s.contains("2 of 5 · downloading the release · 0s"), "{s}");
    a.svc.update(|st| {
        st.justtools.job = None;
        st.justtools.error = Some("the JustTools installer failed: Checksum verification failed".into());
    });
    let s = draw(&mut a);
    assert!(s.contains("✗ the JustTools installer failed") && s.contains("enter tries again"), "{s}");
    a.svc.update(|st| {
        st.justtools.error = None;
        st.justtools.installed.as_mut().unwrap().version = Some("2.9.0".into());
        st.justtools.done = Some("updated 2.8.0 → 2.9.0".into());
    });
    let s = draw(&mut a);
    assert!(s.contains("✓ updated 2.8.0 → 2.9.0") && s.contains("the newest release · enter checks again"), "{s}");
}

#[test]
fn caveman_from_palette_and_toggle() {
    use bro_core::caveman::Level;
    let mut a = app(false);
    a.run_palette(crate::palette::Cmd::Caveman(Some(Level::Ultracave)));
    assert_eq!(a.svc.settings().caveman.active(), Some(Level::Ultracave));
    a.run_act(Act::ToggleCaveman);
    assert_eq!(a.svc.settings().caveman.active(), None);
    a.run_act(Act::ToggleCaveman);
    assert_eq!(a.svc.settings().caveman.active(), Some(Level::Ultracave), "the toggle brings back the last level");
    a.open_palette("caveman");
    let screen = draw(&mut a);
    assert!(screen.contains("caveman mode ultracave · current"), "{screen}");
}

#[test]
fn sidebar_gear_opens_settings() {
    let mut a = app(false);
    draw(&mut a);
    let gear = a.side_hits.iter().find_map(|(r, hit)| matches!(hit, SideHit::Settings).then_some(*r)).expect("gear in the sidebar");
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: gear.x + 1, row: gear.y, modifiers: KeyModifiers::NONE });
    let id = a.focused().expect("settings pane");
    assert_eq!(a.panes[&id].view(), Some("settings"));
}

#[test]
fn painted_themes_recolour_the_whole_window() {
    let mut a = app(true);
    for name in ["tokyo-night", "catppuccin-mocha", "catppuccin-latte", "rose-pine", "rose-pine-dawn", "gruvbox", "nord", "kanagawa", "everforest", "nebula", "phosphor", "outrun", "blueprint", "cyber", "digital-rain", "vaporwave"] {
        a.set_theme(name, false);
        let s = shot(&mut a, &format!("theme-{name}"));
        assert!(s.contains("bro"), "{name}");
    }
    a.set_theme("tokyo-night", false);
    a.open_palette("theme ");
    let s = shot(&mut a, "theme-picker");
    assert!(s.contains("nebula") && s.contains("· light"), "{s}");
}

#[test]
fn rain_falls_everywhere_or_only_in_working_sessions() {
    use crate::skin::Backdrop;
    use bro_core::config::RainMode;
    let mut a = app(true);
    let set = |a: &mut App, m: RainMode| {
        let mut s = a.svc.settings();
        s.rain = m;
        a.svc.save_settings(s);
    };
    let area = ratatui::layout::Rect::new(0, 0, W, H);
    let _ = draw(&mut a);
    assert_eq!(a.backdrops(area), vec![(area, Backdrop::None, vec![])], "off: the theme decides");
    set(&mut a, RainMode::Everywhere);
    assert_eq!(a.backdrops(area), vec![(area, Backdrop::Rain, vec![])]);
    // working: rain everywhere but the sessions that are resting (the demo has one working, the others not)
    set(&mut a, RainMode::Working);
    let working = a.working_rects();
    assert!(!working.is_empty() && working.len() < a.inner.len(), "{} of {}", working.len(), a.inner.len());
    let bds = a.backdrops(area);
    let (r, b, skip) = &bds[0];
    assert_eq!((*r, *b), (area, Backdrop::Rain));
    assert_eq!(skip.len(), a.inner.len() - working.len(), "every resting session is kept clear");
    assert!(skip.iter().all(|s| working.iter().all(|w| !s.intersects(*w))), "no working session is kept clear");
    assert_eq!(bds.last(), Some(&(area, Backdrop::None, vec![])));
    let s = shot(&mut a, "rain-working");
    assert!(s.contains("wire the launcher"), "{s}");
    // the digital-rain theme's own rain follows the same rule
    a.set_theme("digital-rain", false);
    assert_eq!(a.backdrops(area).last(), Some(&(area, Backdrop::None, vec![])));
    set(&mut a, RainMode::Off);
    assert_eq!(a.backdrops(area), vec![(area, Backdrop::Rain, vec![])]);
}

#[test]
fn font_row_only_where_the_terminal_can_switch_and_steps_through_fonts() {
    let mut a = app(false);
    a.run_act(Act::Settings);
    assert!(!draw(&mut a).contains("terminal's own"), "no font choice outside the fork's window");
    crate::fonts::TEST_APP_WINDOW.with(|w| w.set(true));
    let screen = draw(&mut a);
    assert!(screen.contains("Font") && screen.contains("terminal's own"), "{screen}");
    key(&mut a, KeyCode::Down, KeyModifiers::NONE); // Font
    key(&mut a, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(a.svc.settings().font.as_deref(), Some("jetbrains-mono"));
    key(&mut a, KeyCode::Left, KeyModifiers::NONE);
    key(&mut a, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(a.svc.settings().font.as_deref(), Some("press-start-2p"), "wraps round to the wildest");
    let screen = draw(&mut a);
    assert!(screen.contains("8-bit arcade"), "{screen}");
}

#[test]
fn caveman_sessions_carry_a_header_badge() {
    // the demo's first Claude session was "launched" with caveman; the setting itself is off,
    // so the sidebar shows no badge and this one is the pane header's
    let mut a = app(true);
    assert!(a.svc.settings().caveman.active().is_none());
    let screen = draw(&mut a);
    assert!(screen.contains("⛏ caveman"), "{screen}");
}

#[test]
fn back_and_forward_step_through_viewed_sessions_keeping_full_screen_tiles_and_the_new_session_screen() {
    let mut a = app(true);
    let leaves = |root: &Node| {
        let mut out = vec![];
        root.leaves(&mut out);
        out
    };
    let ids: Vec<PaneId> = a.tabs.iter().flat_map(|t| leaves(&t.root)).collect();
    assert!(ids.len() >= 3);
    let nav = |a: &mut App, back: bool| {
        a.handle(Event::Navigate(back));
        a.after_events();
    };
    for &id in &ids[..3] {
        a.go_session(id);
        a.after_events();
    }
    nav(&mut a, true);
    assert_eq!(a.focused(), Some(ids[1]));
    nav(&mut a, true);
    assert_eq!(a.focused(), Some(ids[0]));
    nav(&mut a, false);
    assert_eq!(a.focused(), Some(ids[1]));

    // the new-session screen is a stop too; leaving it by starting something keeps it in history
    a.open_launcher(None, Place::Tab);
    a.after_events();
    a.overlay = Overlay::None;
    a.go_session(ids[2]);
    a.after_events();
    nav(&mut a, true);
    assert!(matches!(a.overlay, Overlay::Launcher(_)), "back to the new-session screen");
    nav(&mut a, true);
    assert!(!a.overlay.is_open());
    assert_eq!(a.focused(), Some(ids[1]));
    nav(&mut a, false);
    nav(&mut a, false);
    assert_eq!(a.focused(), Some(ids[2]));

    // a zoomed split stays zoomed on the session you step to
    if let Some(split) = a.tabs.iter().position(|t| !matches!(t.root, Node::Leaf(_))) {
        let (first, second) = (leaves(&a.tabs[split].root)[0], leaves(&a.tabs[split].root)[1]);
        a.go_session(first);
        a.after_events();
        a.go_session(second);
        a.tabs[a.cur].zoom = true;
        a.after_events();
        nav(&mut a, true);
        assert_eq!(a.focused(), Some(first));
        assert!(a.tabs[a.cur].zoom, "full screen stays full screen");
    }

    // full-screen tiles stay full screen; plain tiles stay tiled and step through tiled sessions only
    a.run_act(Act::TileAll);
    let tiled = a.stack.clone();
    a.go_session(tiled[0]);
    a.after_events();
    a.go_session(tiled[1]);
    a.after_events();
    a.stack_zoom = true;
    nav(&mut a, true);
    assert_eq!(a.focused(), Some(tiled[0]));
    assert!(a.stacked() && a.stack_zoom);
    a.stack_zoom = false;
    nav(&mut a, false);
    assert_eq!(a.focused(), Some(tiled[1]));
    assert!(a.stacked() && !a.stack_zoom);

    // a preview open on top closes first
    a.overlay = Overlay::Help(crate::help::Help::default());
    nav(&mut a, true);
    assert!(!a.overlay.is_open());
    assert_eq!(a.focused(), Some(tiled[1]));
}

#[test]
fn quick_install_ticks_the_missing_recommended_tools_and_toggles_them() {
    let mut a = app(true);
    a.run_act(Act::QuickInstall);
    let id = a.focused().expect("quick install pane");
    assert_eq!(a.panes[&id].view(), Some("quick-install"));
    let s = draw(&mut a);
    // the demo has nothing installed: the recommended tools start ticked, the rest don't
    assert!(s.contains("[x] JustTools") && s.contains("[x] Node.js LTS") && s.contains("[ ] .NET 9 SDK") && s.contains("[ ] Python 3"), "{s}");
    assert!(s.contains("Install 6 · JustTools, Git, Node.js LTS, Bun, Claude Code, Codex CLI"), "{s}");
    key(&mut a, KeyCode::Char(' '), KeyModifiers::NONE); // untick JustTools
    key(&mut a, KeyCode::Char('n'), KeyModifiers::NONE); // none
    assert!(draw(&mut a).contains("nothing ticked to install"));
    key(&mut a, KeyCode::Char('a'), KeyModifiers::NONE); // every missing tool
    let s = shot(&mut a, "quick_install");
    assert!(s.contains("[x] .NET 11 SDK") && s.contains("Install 16"), "{s}");
    key(&mut a, KeyCode::Char('i'), KeyModifiers::NONE); // the demo installs nothing
    assert_eq!(a.focused(), Some(id), "no install tab in the demo");
}
