use super::*;
use crate::services::{Services, fallback_settings};

#[test]
fn gemini_profile_stays_selected_when_switching_harnesses() {
    let mut d = data();
    let mut second = d.models["gemini:work"][0].clone();
    second.id = "gemini-picked-model".into();
    d.models.get_mut("gemini:work").unwrap().push(second);
    let mut l = Launcher::new(d, Some(PathBuf::from("/code/bro")), Place::Tab)
        .with_profile("gemini:work").unwrap();
    l.model_sel = 1;
    for harness in Harness::ALL {
        assert_eq!(l.harness, harness);
        assert!(l.needs_model());
        let spec = l.spec().unwrap();
        assert_eq!(spec.profile_id.as_deref(), Some("gemini:work"));
        assert!(spec.provider_id.is_none());
        assert_eq!(spec.model.as_deref(), Some("gemini-picked-model"));
        key(&mut l, KeyCode::Right);
    }
}

#[test]
fn gemini_subscription_stays_selected_when_switching_harnesses() {
    let mut l = Launcher::new(data(), None, Place::Tab).with_profile("gemini-sub:local").unwrap();
    for harness in Harness::ALL {
        let spec = l.spec().unwrap();
        assert_eq!(spec.harness, harness);
        assert_eq!(spec.profile_id.as_deref(), Some("gemini-sub:local"));
        assert_eq!(spec.model.as_deref(), Some("gemini-4-argon-high"));
        key(&mut l, KeyCode::Right);
    }
}

fn data() -> Data {
    let (tx, _rx) = std::sync::mpsc::channel();
    let svc = Services::offline(fallback_settings(), tx);
    let st = svc.state();
    Data {
        profiles: st.profiles.ready().cloned().unwrap_or_default(),
        providers: st.providers.ready().cloned().unwrap_or_default(),
        usage: BTreeMap::new(),
        usage_status: BTreeMap::new(),
        installed: vec![],
        recents: vec![],
        models: st.models.clone(),
        keyed: vec!["openrouter".into(), "openai".into()],
        orchestrator_running: None,
        shared: false,
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
/// Narrow the run-on list (typing there goes to the project picker instead).
fn narrow(l: &mut Launcher, s: &str) {
    l.list_filter = s.into();
    l.filter_changed();
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
fn refreshed_data_preserves_account_and_model_when_rows_shift() {
    let mut l = Launcher::new(data(), None, Place::Tab).with_harness(Harness::Codex).with_profile("claude:work").unwrap();
    let old = l.current().unwrap();
    assert!(l.needs_model());
    l.focus = Focus::Models;
    let model = l.model().unwrap().id;
    let mut fresh = l.data.clone();
    // A newly discovered login before the selected row must not change who will launch.
    let mut added = fresh.profiles.iter().find(|p| p.is_claude()).unwrap().clone();
    added.id = "claude:new".into();
    added.name = "new".into();
    fresh.profiles.insert(0, added);
    fresh.models.get_mut("claude").unwrap().reverse();
    fresh.usage.insert("claude:work".into(), 83.0);
    l.set_data(fresh);
    let Item::Account(old) = old else { panic!() };
    let Some(Item::Account(new)) = l.current() else { panic!() };
    assert_eq!(new.kind, old.kind);
    assert_eq!(new.left, Some(17.0));
    assert_eq!(l.model().unwrap().id, model);
    assert_eq!(l.focus, Focus::Models);
}

#[test]
fn exact_profile_selection_does_not_fall_back_to_another_account() {
    let l = Launcher::new(data(), None, Place::Tab).with_harness(Harness::Codex).with_profile("codex:team").unwrap();
    assert_eq!(l.quick_spec().unwrap().profile_id.as_deref(), Some("codex:team"));
    assert!(Launcher::new(data(), None, Place::Tab).with_profile("claude:removed").is_none());
    let mut d = data();
    d.profiles.iter_mut().find(|p| p.id == "claude:work").unwrap().authenticated = false;
    let l = Launcher::new(d, None, Place::Tab).with_harness(Harness::Claude).with_profile("claude:work").unwrap();
    assert!(l.quick_spec().is_none(), "a logged-out account must not silently launch a different login");
    assert_eq!(l.spec().unwrap().profile_id.as_deref(), Some("claude:work"));
}

#[test]
fn openrouter_in_both_harnesses_needs_a_model() {
    for h in [Harness::Claude, Harness::Codex] {
        let mut l = Launcher::new(data(), None, Place::Tab);
        while l.harness != h {
            key(&mut l, KeyCode::Right);
        }
        narrow(&mut l, "openrouter");
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
    narrow(&mut l, "work"); // claude:work, via proxy
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
    assert_eq!(headers, ["your logins", "other logins · via proxy", "Gemini profiles", "providers"], "no recents yet");
    assert!(!matches!(items[l.cursor_row(&items).unwrap()], Item::Header(_)));
}

#[test]
fn recents_relaunch_and_remember_the_model() {
    let mut d = data();
    d.recents = vec![Recent { harness: Harness::Codex, profile_id: None, provider_id: Some("openrouter".into()), model: Some("z-ai/glm-5.3".into()), cwd: PathBuf::from("/code/justgains"), permission: Permission::Auto, browser: BrowserMode::Off, at: 1, orchestrator: false }];
    let mut l = Launcher::new(d, None, Place::Tab);
    assert_eq!(l.harness, Harness::Codex, "opens on the last harness");
    assert_eq!(l.dir, PathBuf::from("/code/justgains"));
    assert_eq!(l.permission, Permission::Skip, "always starts on skip");
    assert!(matches!(l.current(), Some(Item::Recent(0))));
    let s = l.spec().unwrap();
    assert_eq!((s.provider_id.as_deref(), s.model.as_deref()), (Some("openrouter"), Some("z-ai/glm-5.3")));
    // picking openrouter by hand starts on the model used last time
    narrow(&mut l, "openrouter");
    while !matches!(l.current(), Some(Item::Account(_))) {
        key(&mut l, KeyCode::Down);
    }
    assert_eq!(l.model().unwrap().id, "z-ai/glm-5.3");
}

#[test]
fn toggles_and_esc() {
    let mut l = Launcher::new(data(), Some(PathBuf::from("/code/bro")), Place::Tab);
    assert_eq!(l.dir, PathBuf::from("/code/bro"), "starts in the current project");
    assert_eq!(l.permission, Permission::Skip, "skips permissions by default");
    ctrl(&mut l, 'e');
    assert_eq!(l.permission, Permission::Default);
    ctrl(&mut l, 's');
    assert_eq!(l.place, Place::Split);
    narrow(&mut l, "zz");
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::None), "esc clears the filter first");
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::Close));
}

#[test]
fn typing_on_the_list_picks_the_project_and_esc_comes_back() {
    let mut l = Launcher::new(data(), Some(PathBuf::from("/code/bro")), Place::Tab);
    let account = format!("{:?}", l.current());
    typ(&mut l, "zz");
    assert_eq!(l.project_picker.as_ref().unwrap().filter, "zz", "the keys went to the project picker");
    assert!(l.list_filter.is_empty(), "the logins aren't filtered");
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::None), "esc clears the project filter");
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::None), "then closes the picker");
    assert!(l.project_picker.is_none());
    assert_eq!(format!("{:?}", l.current()), account, "back on the same login");
    l.paste("zz");
    assert_eq!(l.project_picker.as_ref().unwrap().filter, "zz", "a paste is a project too");
}

#[test]
fn changing_project_keeps_the_session_choices_and_uses_folder_completion() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let nested = root.path().join("second project");
    for dir in [&first, &second, &nested] {
        std::fs::create_dir(dir).unwrap();
    }
    let mut l = Launcher::new(data(), Some(first.clone()), Place::Tab).with_projects(vec![first.clone(), second.clone()]).with_harness(Harness::Codex);
    narrow(&mut l, "team");
    let account = l.current();
    l.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
    let picker = l.project_picker.as_ref().unwrap();
    assert_eq!(picker.view()[0].0, first);
    assert_eq!(picker.view()[1].0, second);
    key(&mut l, KeyCode::Down);
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::Project(ref dir) if *dir == second));
    assert_eq!(l.dir, second);
    assert!(l.project_picker.is_none());
    assert_eq!(l.harness, Harness::Codex);
    assert_eq!(l.list_filter, "team");
    assert_eq!(format!("{:?}", l.current()), format!("{account:?}"));
    assert_eq!(l.spec().unwrap().cwd, second);

    l.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
    l.paste(&format!("{}{}second p", root.path().display(), std::path::MAIN_SEPARATOR));
    assert!(l.project_picker.as_ref().unwrap().view().iter().any(|(path, kind)| *path == nested && *kind == folder::Kind::Completion));
    key(&mut l, KeyCode::Tab);
    assert!(l.project_picker.as_ref().unwrap().filter.ends_with(&format!("second project{}", std::path::MAIN_SEPARATOR)));
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::Project(ref dir) if *dir == nested));
    assert_eq!(l.spec().unwrap().cwd, nested);
}

#[test]
fn enter_on_a_missing_agent_installs_it() {
    let mut d = data();
    d.installed = vec![(Harness::Claude, true), (Harness::Codex, false)];
    let mut l = Launcher::new(d, None, Place::Tab);
    assert!(!matches!(key(&mut l, KeyCode::Enter), Outcome::Install(_)), "claude is installed");
    while l.harness != Harness::Codex {
        key(&mut l, KeyCode::Right);
    }
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::Install(Harness::Codex)));
}

fn shift(l: &mut Launcher, code: KeyCode) -> Outcome {
    l.key(KeyEvent::new(code, KeyModifiers::SHIFT))
}

fn selected_profile(l: &Launcher) -> Option<String> {
    l.spec().and_then(|s| s.profile_id.or(s.provider_id))
}

#[test]
fn orchestrator_is_a_tab_after_the_agents() {
    let mut l = Launcher::new(data(), Some(PathBuf::from("/code/bro")), Place::Split).with_harness(Harness::Omp);
    key(&mut l, KeyCode::Right);
    assert!(l.orchestrator);
    assert_eq!(l.harness, Harness::Claude, "Claude Code first: its tools are native there");
    let s = l.spec().unwrap();
    assert!(s.orchestrator && s.harness == Harness::Claude);
    // shift+←/→ picks the harness it runs on; plain ←/→ leaves the tab
    shift(&mut l, KeyCode::Right);
    assert!(l.orchestrator && l.harness == Harness::Codex);
    assert!(l.spec().unwrap().profile_id.unwrap().starts_with("codex:"), "lands on a codex login");
    shift(&mut l, KeyCode::Left);
    shift(&mut l, KeyCode::Left);
    assert_eq!(l.harness, Harness::Omp, "wraps");
    key(&mut l, KeyCode::Right);
    assert!(!l.orchestrator && l.harness == Harness::Claude);
    key(&mut l, KeyCode::Left);
    assert!(l.orchestrator);
    key(&mut l, KeyCode::Left);
    assert!(!l.orchestrator && l.harness == Harness::Omp);

    // it always opens as its own session, whatever the placement switch says
    let mut l = l.with_orchestrator();
    ctrl(&mut l, 's');
    assert_eq!(l.place, Place::Split, "^s does nothing on the orchestrator tab");
    match key(&mut l, KeyCode::Enter) {
        Outcome::Launch(s, place) => assert!(s.orchestrator && place == Place::Tab),
        _ => panic!("expected a launch"),
    }
}

#[test]
fn orchestrator_starts_on_the_roomiest_login_unless_your_last_one_has_room() {
    let mut d = data();
    let claude: Vec<String> = d.profiles.iter().filter(|p| p.is_claude() && p.authenticated).map(|p| p.id.clone()).collect();
    assert!(claude.len() >= 2, "{claude:?}");
    let (busy, roomy) = (claude[0].clone(), claude[1].clone());
    d.usage.insert(busy.clone(), 95.0);
    d.usage.insert(roomy.clone(), 20.0);
    let l = Launcher::new(d.clone(), None, Place::Tab).with_orchestrator();
    assert_eq!(selected_profile(&l), Some(roomy.clone()), "most left wins");

    // your last orchestrator combo comes first while its login has room…
    let recent = |profile: &str| crate::recents::Recent { harness: Harness::Claude, profile_id: Some(profile.into()), provider_id: None, model: None, cwd: PathBuf::from("/x"), permission: Permission::Skip, browser: BrowserMode::Off, at: 1, orchestrator: true };
    d.usage.insert(busy.clone(), 50.0);
    d.recents = vec![recent(&busy)];
    let l = Launcher::new(d.clone(), None, Place::Tab).with_orchestrator();
    assert!(matches!(l.current(), Some(Item::Recent(_))));
    assert_eq!(selected_profile(&l), Some(busy.clone()));
    // …and is passed over once it's nearly out
    d.usage.insert(busy.clone(), 95.0);
    let l = Launcher::new(d.clone(), None, Place::Tab).with_orchestrator();
    assert_eq!(selected_profile(&l), Some(roomy));

    // orchestrator combos stay on its tab; plain ones stay off it
    let l = Launcher::new(d.clone(), None, Place::Tab).with_harness(Harness::Claude);
    assert!(!l.items().iter().any(|i| matches!(i, Item::Recent(_))));
    let mut plain = recent(&busy);
    plain.orchestrator = false;
    d.recents = vec![plain];
    let l = Launcher::new(d, None, Place::Tab).with_orchestrator();
    assert!(!l.items().iter().any(|i| matches!(i, Item::Recent(_))));
}

#[test]
fn orchestrator_harness_follows_your_last_one() {
    let mut d = data();
    let codex = d.profiles.iter().find(|p| p.is_codex()).unwrap().id.clone();
    d.recents = vec![crate::recents::Recent { harness: Harness::Codex, profile_id: Some(codex), provider_id: None, model: None, cwd: PathBuf::from("/x"), permission: Permission::Skip, browser: BrowserMode::Off, at: 1, orchestrator: true }];
    let l = Launcher::new(d.clone(), None, Place::Tab).with_orchestrator();
    assert_eq!(l.harness, Harness::Codex);
    d.installed = vec![(Harness::Codex, false), (Harness::Claude, false)];
    let l = Launcher::new(d, None, Place::Tab).with_orchestrator();
    assert_eq!(l.harness, Harness::Pi, "an installed harness when the usual ones aren't");
}

#[test]
fn gemini_signs_in_before_it_launches() {
    let mut l = Launcher::new({ let mut d = data(); d.profiles.retain(|p| !p.is_gemini()); d }, None, Place::Tab);
    narrow(&mut l, "google");
    let Some(Item::Account(a)) = l.current() else { panic!("gemini row") };
    assert!(!a.ready && a.detail.contains("signs in with Google"), "{}", a.detail);
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::SignIn));

    let mut d = data();
    d.profiles.retain(|p| !p.is_gemini());
    d.keyed.push("gemini".into());
    let mut l = Launcher::new(d, None, Place::Tab);
    narrow(&mut l, "google");
    let Some(Item::Account(a)) = l.current() else { panic!("gemini row") };
    assert!(a.ready && a.detail.contains("google ai subscription"));
    key(&mut l, KeyCode::Enter);
    let s = launched(key(&mut l, KeyCode::Enter));
    assert_eq!((s.provider_id.as_deref(), s.model.as_deref()), (Some("gemini"), Some("gemini-4-argon")));
}

// ------------------------------------------------------------------ proxy mode

fn recent(harness: Harness, profile: Option<&str>, provider: Option<&str>, model: Option<&str>) -> Recent {
    Recent { harness, profile_id: profile.map(Into::into), provider_id: provider.map(Into::into), model: model.map(Into::into), cwd: PathBuf::from("/code/bro"), permission: Permission::Skip, browser: BrowserMode::Off, at: 1, orchestrator: false }
}

fn accounts(l: &Launcher) -> Vec<Account> {
    l.items().into_iter().filter_map(|i| match i { Item::Account(a) => Some(a), _ => None }).collect()
}

fn select(l: &mut Launcher, kind: AccountKind) {
    let items = l.items();
    l.list_sel = Launcher::selectable(&items).iter().position(|&i| matches!(&items[i], Item::Account(a) if a.kind == kind)).expect("row");
    l.reset_models();
}

#[test]
fn proxy_mode_offers_each_family_once_instead_of_its_logins() {
    let mut d = data();
    d.shared = true;
    d.usage.insert("claude:local".into(), 20.0);
    d.usage.insert("claude:work".into(), 60.0);
    d.recents = vec![recent(Harness::Claude, Some("claude:work"), None, None), recent(Harness::Claude, None, Some("pool"), None), recent(Harness::Claude, None, Some("openrouter"), Some("qwen/qwen3-coder"))];
    let mut l = Launcher::new(d, Some(PathBuf::from("/code/bro")), Place::Tab);
    let rows = accounts(&l);
    assert!(rows.iter().all(|a| !matches!(&a.kind, AccountKind::Profile(p) if p.starts_with("claude:") || p.starts_with("codex:")) && a.kind != AccountKind::Pool), "no login to pick: {rows:?}");
    assert!(l.items().iter().any(|i| matches!(i, Item::Header("proxy mode"))));
    assert_eq!(l.recents_for(Harness::Claude), vec![2], "combos naming one login (or the pool) aren't offered");

    select(&mut l, AccountKind::Shared(true));
    let Some(Item::Account(own)) = l.current() else { panic!() };
    assert_eq!(own.label, "all your logins");
    assert!(own.detail.starts_with("local, personal, work"), "{}", own.detail);
    assert_eq!(own.left, Some(60.0), "the average of what the read logins have left");
    assert!(!l.needs_model(), "/model picks inside, as ever");
    let s = l.spec().unwrap();
    assert_eq!((s.profile_id, s.provider_id, s.model), (None, None, None), "the launch picks the login it runs as");

    // the other family through the proxy takes a model from its own list
    select(&mut l, AccountKind::Shared(false));
    assert!(l.needs_model());
    assert_eq!(l.models().first().map(|m| m.id.clone()), l.data.models.get("codex").and_then(|m| m.first()).map(|m| m.id.clone()));
    let s = l.spec().unwrap();
    assert_eq!(s.provider_id.as_deref(), Some(bro_core::launch::LOGINS_CODEX));
    assert!(s.model.is_some());

    // a recent of the shared logins reads as them
    let (kind, who) = l.recent_row(&recent(Harness::Claude, None, None, None));
    assert_eq!((kind, who.as_str()), (AccountKind::Shared(true), "all your logins"));
    let (kind, who) = l.recent_row(&recent(Harness::Claude, None, Some(bro_core::launch::LOGINS_CODEX), Some("gpt-5.5")));
    assert_eq!((kind, who.as_str()), (AccountKind::Shared(false), "all your chatgpt logins"));
}

#[test]
fn proxy_mode_codex_starts_on_its_logins_and_pi_gets_both_families() {
    let mut d = data();
    d.shared = true;
    let codex = Launcher::new(d.clone(), None, Place::Tab).with_harness(Harness::Codex);
    let s = codex.quick_spec().expect("one click: no login, no model");
    assert_eq!((s.harness, s.profile_id, s.provider_id), (Harness::Codex, None, None));
    let pi = Launcher::new(d, None, Place::Tab).with_harness(Harness::Pi);
    let kinds: Vec<AccountKind> = accounts(&pi).into_iter().map(|a| a.kind).filter(|k| !matches!(k, AccountKind::Provider(_) | AccountKind::Profile(_))).collect();
    assert_eq!(kinds, [AccountKind::Native, AccountKind::Shared(true), AccountKind::Shared(false)]);
}

#[test]
fn proxy_mode_off_keeps_every_login() {
    let l = Launcher::new(data(), None, Place::Tab);
    assert!(accounts(&l).iter().all(|a| !matches!(a.kind, AccountKind::Shared(_))));
    assert_eq!(l.recent_row(&recent(Harness::Claude, None, None, None)).0, AccountKind::Native);
}
