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
    key(&mut a, KeyCode::Char('j'), KeyModifiers::NONE); // past "+ new session" and "+ open project" onto the first project
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
fn archive_hides_earlier_sessions_and_undo_restores() {
    use crate::sidebar::Row;
    let mut a = app(true);
    let past_rows = |a: &App| a.rows().iter().filter(|r| matches!(r, Row::Past { .. })).count();
    let before = past_rows(&a);
    assert!(before > 0);
    a.run_act(Act::FocusSidebar);
    let i = a.rows().iter().position(|r| matches!(r, Row::Past { .. })).unwrap();
    a.side_sel = i;
    key(&mut a, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(past_rows(&a), before - 1, "archived one");
    // a whole project
    let p = a.rows().iter().position(|r| matches!(r, Row::Project { .. })).unwrap();
    a.side_sel = p;
    key(&mut a, KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(past_rows(&a) < before - 1);
    // show archived: they come back dimmed; u undoes the project batch
    key(&mut a, KeyCode::Char('A'), KeyModifiers::NONE);
    assert_eq!(past_rows(&a), before.min(past_rows(&a)).max(past_rows(&a)));
    key(&mut a, KeyCode::Char('A'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Char('u'), KeyModifiers::NONE);
    key(&mut a, KeyCode::Char('u'), KeyModifiers::NONE);
    assert_eq!(past_rows(&a), before, "undo restores everything");
    // running sessions aren't archivable
    let live = a.rows().iter().position(|r| matches!(r, Row::Live { .. })).unwrap();
    a.side_sel = live;
    key(&mut a, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(past_rows(&a), before);
}

#[test]
fn clicking_the_x_on_a_sidebar_row_asks_to_close_that_session() {
    let mut a = app(true);
    let _ = draw(&mut a);
    let (r, id) = a.side_hits.iter().find_map(|(r, h)| if let SideHit::Close(id) = h { Some((*r, *id)) } else { None }).expect("a close hit");
    assert!(draw(&mut a).contains('×'));
    a.mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x, row: r.y, modifiers: KeyModifiers::NONE });
    match &a.overlay {
        Overlay::Confirm(_) => {}
        _ => assert!(!a.panes.contains_key(&id), "closed straight away or asked first"),
    }
}

#[test]
fn resuming_offers_every_login_of_that_kind() {
    use crate::sidebar::Row;
    use super::overlays::{ResumeFrom, ResumePicker};
    let mut a = app(true);
    a.run_act(Act::FocusSidebar);
    // an earlier claude session: Enter asks which login, current one first
    let i = a.rows().iter().position(|r| matches!(r, Row::Past { info } if info.harness == bro_core::Harness::Claude)).unwrap();
    a.side_sel = i;
    key(&mut a, KeyCode::Enter, KeyModifiers::NONE);
    let Overlay::Resume(p) = &a.overlay else { panic!("resume picker") };
    let p: &ResumePicker = p;
    assert!(matches!(p.from, ResumeFrom::Past(_)));
    assert!(p.targets[0].current);
    assert!(p.targets.len() >= 3 && p.targets.iter().all(|t| t.profile_id.starts_with("claude:")));
    let s = shot(&mut a, "resume-picker");
    assert!(s.contains("resume in") && s.contains("current") && s.contains("% left"), "{s}");
    key(&mut a, KeyCode::Esc, KeyModifiers::NONE);
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
        let (r, _) = *a.side_hits.iter().find(|(_, h)| matches!(h, SideHit::Row(x) if *x == i)).unwrap();
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
