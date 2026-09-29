use super::*;
use crate::services::{Services, fallback_settings};

fn data() -> Data {
    let (tx, _rx) = std::sync::mpsc::channel();
    let svc = Services::offline(fallback_settings(), tx);
    let st = svc.state();
    Data {
        profiles: st.profiles.ready().cloned().unwrap_or_default(),
        providers: st.providers.ready().cloned().unwrap_or_default(),
        usage: BTreeMap::new(),
        installed: vec![],
        dirs: vec![PathBuf::from("/code/bro"), PathBuf::from("/code/justgains"), PathBuf::from("/code/terminal")],
        recents: vec![],
        models: st.models.clone(),
        keyed: vec!["openrouter".into(), "openai".into()],
    }
}

fn key(l: &mut Launcher, code: KeyCode) -> Outcome {
    l.key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ctrl(l: &mut Launcher, c: char) -> Outcome {
    l.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}
fn typ(l: &mut Launcher, s: &str) {
    for c in s.chars() {
        key(l, KeyCode::Char(c));
    }
}
fn launched(o: Outcome) -> LaunchSpec {
    match o {
        Outcome::Launch(s, _) => s,
        _ => panic!("expected a launch"),
    }
}

#[test]
fn own_login_launches_without_a_model() {
    let mut l = Launcher::new(data(), Some(PathBuf::from("/code/bro")), Place::Tab);
    assert_eq!(l.harness, Harness::Claude);
    // first row is the first claude login: no model list at all
    assert!(matches!(l.current(), Some(Item::Account(Account { kind: AccountKind::Profile(ref p), .. })) if p.starts_with("claude:")));
    assert!(!l.needs_model() && l.models().is_empty());
    let s = launched(key(&mut l, KeyCode::Enter));
    assert!(s.profile_id.unwrap().starts_with("claude:"));
    assert!(s.model.is_none(), "the CLI picks its own default");
    assert_eq!(s.cwd, PathBuf::from("/code/bro"));
}

#[test]
fn openrouter_in_both_harnesses_needs_a_model() {
    for h in [Harness::Claude, Harness::Codex] {
        let mut l = Launcher::new(data(), None, Place::Tab);
        while l.harness != h {
            key(&mut l, KeyCode::Right);
        }
        typ(&mut l, "openrouter");
        assert!(l.needs_model(), "{h:?}");
        assert!(l.models().len() >= 5, "live catalogue rows");
        // Enter moves to the model list instead of launching
        assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::None));
        assert_eq!(l.focus, Focus::Models);
        typ(&mut l, "qwen");
        let s = launched(key(&mut l, KeyCode::Enter));
        assert_eq!(s.harness, h);
        assert_eq!(s.provider_id.as_deref(), Some("openrouter"));
        assert_eq!(s.model.as_deref(), Some("qwen/qwen3-coder"));
    }
}

#[test]
fn cross_logins_need_a_model_from_the_other_family() {
    let mut l = Launcher::new(data(), None, Place::Tab);
    key(&mut l, KeyCode::Right); // codex
    assert_eq!(l.harness, Harness::Codex);
    assert!(!l.needs_model(), "codex on its own login");
    typ(&mut l, "work"); // claude:work, via proxy
    assert!(l.needs_model());
    assert!(l.models().iter().any(|m| m.id.starts_with("claude-")), "claude models for a claude login");
    key(&mut l, KeyCode::Tab);
    let s = launched(key(&mut l, KeyCode::Enter));
    assert_eq!(s.profile_id.as_deref(), Some("claude:work"));
    assert!(s.model.unwrap().starts_with("claude-"));
}

#[test]
fn headers_group_the_list_and_are_skipped() {
    let l = Launcher::new(data(), None, Place::Tab);
    let items = l.items();
    let headers: Vec<&str> = items.iter().filter_map(|i| if let Item::Header(h) = i { Some(*h) } else { None }).collect();
    assert_eq!(headers, ["your logins", "other logins · via proxy", "providers"], "no recents yet");
    assert!(!matches!(items[l.cursor_row(&items).unwrap()], Item::Header(_)));
}

#[test]
fn recents_relaunch_and_remember_the_model() {
    let mut d = data();
    d.recents = vec![Recent { harness: Harness::Codex, profile_id: None, provider_id: Some("openrouter".into()), model: Some("z-ai/glm-5.3".into()), cwd: PathBuf::from("/code/justgains"), permission: Permission::Auto, browser: BrowserMode::Off, at: 1 }];
    let mut l = Launcher::new(d, None, Place::Tab);
    assert_eq!(l.harness, Harness::Codex, "opens on the last harness");
    assert_eq!(l.dir, PathBuf::from("/code/justgains"));
    assert_eq!(l.permission, Permission::Auto);
    assert!(matches!(l.current(), Some(Item::Recent(0))));
    let s = l.spec().unwrap();
    assert_eq!((s.provider_id.as_deref(), s.model.as_deref()), (Some("openrouter"), Some("z-ai/glm-5.3")));
    // picking openrouter by hand starts on the model used last time
    typ(&mut l, "openrouter");
    while !matches!(l.current(), Some(Item::Account(_))) {
        key(&mut l, KeyCode::Down);
    }
    assert_eq!(l.model().unwrap().id, "z-ai/glm-5.3");
}

#[test]
fn project_picker_toggles_and_esc_steps_back() {
    let mut l = Launcher::new(data(), Some(PathBuf::from("/code/bro")), Place::Tab);
    ctrl(&mut l, 'd');
    assert_eq!(l.focus, Focus::Dirs);
    typ(&mut l, "just");
    key(&mut l, KeyCode::Enter);
    assert_eq!(l.dir, PathBuf::from("/code/justgains"));
    assert_eq!(l.focus, Focus::List);
    ctrl(&mut l, 'd');
    typ(&mut l, "~/new-thing");
    assert!(l.dirs_view()[0].1, "typed path first");
    key(&mut l, KeyCode::Enter);
    assert!(l.dir.ends_with("new-thing"));
    ctrl(&mut l, 'e');
    ctrl(&mut l, 'e');
    assert_eq!(l.permission, Permission::Skip);
    ctrl(&mut l, 's');
    assert_eq!(l.place, Place::Split);
    typ(&mut l, "zz");
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::None), "esc clears the filter first");
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::Close));
}

#[test]
fn paths_and_dedup() {
    assert!(looks_like_path("~/x") && looks_like_path("C:\\x") && looks_like_path("/x") && !looks_like_path("bro"));
    let d = dedup_dirs(vec![vec![PathBuf::from("/a"), PathBuf::from("/b")], vec![PathBuf::from("/a"), PathBuf::from("/c")]]);
    assert_eq!(d, vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/c")]);
}
