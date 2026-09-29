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
fn welcome_screen() {
    let mut a = app(false);
    let s = shot(&mut a, "welcome");
    assert!(s.contains("██████╗"), "logo\n{s}");
    assert!(s.contains("to launch your first agent"), "{s}");
    assert!(s.contains("alt+n"), "{s}");
    assert!(s.contains("+ new session"), "{s}");
    assert!(s.contains("bro-cli-v2") && s.contains("justgains"), "past sessions grouped by project even with nothing live
{s}");
    assert!(!s.contains("↺"), "no past-count glyphs
{s}");
}

#[test]
fn sidebar_with_three_projects() {
    let mut a = app(true);
    // open one project's past sessions, and focus the sidebar on it
    a.side.past_open.insert(a.rows().iter().find_map(|r| if let crate::sidebar::Row::Project { key, .. } = r { Some(key.clone()) } else { None }).unwrap());
    a.run_act(Act::FocusSidebar);
    let s = shot(&mut a, "sidebar-demo");
    for p in ["bro-cli-v2", "justgains", "terminal"] {
        assert!(s.contains(p), "project {p} missing\n{s}");
    }
    assert!(s.contains("+ new session"), "{s}");
    assert!(s.contains("usage left"), "{s}");
    assert!(s.contains("+ new") && s.contains("alt+n"), "tab bar new button
{s}");
    assert!(s.contains("proxy") && s.contains(":3458"), "{s}");
    assert!(s.contains("bridge") && s.contains(":10001"), "{s}");
    assert!(s.contains("claude · work · opus-5"), "pane title\n{s}");
    // the live sessions are numbered for alt+1..9
    let order = crate::sidebar::live_order(&a.live_infos(), &a.past_infos());
    assert_eq!(order.len(), 5);
}

#[test]
fn launcher_open() {
    let mut a = app(true);
    key(&mut a, KeyCode::Char('n'), KeyModifiers::ALT);
    assert!(matches!(a.overlay, Overlay::Launcher(_)));
    let s = shot(&mut a, "launcher");
    assert!(s.contains("HARNESS") && s.contains("ACCOUNT") && s.contains("MODEL") && s.contains("PROJECT"), "{s}");
    assert!(s.contains("pool") && s.contains("openrouter"), "{s}");
    // typing filters the focused column; tab moves on
    for c in "codex".chars() {
        key(&mut a, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(&mut a, KeyCode::Tab, KeyModifiers::NONE);
    let s = draw(&mut a);
    assert!(s.contains("team"), "{s}");
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
    assert_eq!(a.theme.name, "ultra", "restored on esc");
}

#[test]
fn keyboard_navigation() {
    let mut a = app(true);
    let _ = draw(&mut a);
    let order = crate::sidebar::live_order(&a.live_infos(), &a.past_infos());
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
    key(&mut a, KeyCode::Char('j'), KeyModifiers::NONE); // past "+ new session" onto the first project
    key(&mut a, KeyCode::Char('h'), KeyModifiers::NONE);
    assert_eq!(a.side.collapsed.len(), 1);
    key(&mut a, KeyCode::Char('l'), KeyModifiers::NONE);
    assert!(a.side.collapsed.is_empty());
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
    let (r, _) = *a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::Row(x) if *x == i)).unwrap();
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
    let order = crate::sidebar::live_order(&a.live_infos(), &a.past_infos());
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
    assert!(s.contains("codex · local · gpt-5.2"), "{s}");
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
