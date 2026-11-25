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
    for harness in Harness::ALL.into_iter().filter(|h| *h != Harness::Antigravity) {
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
    for harness in Harness::ALL.into_iter().filter(|h| *h != Harness::Antigravity) {
        let spec = l.spec().unwrap();
        assert_eq!(spec.harness, harness);
        assert_eq!(spec.profile_id.as_deref(), Some("gemini-sub:local"));
        assert_eq!(spec.model.as_deref(), Some("gemini-4-argon-high"));
        key(&mut l, KeyCode::Right);
    }
}

#[test]
fn antigravity_launcher_uses_its_native_account() {
    let l = Launcher::new(data(), None, Place::Tab).with_harness(Harness::Antigravity);
    assert!(matches!(&l.items()[..], [Item::Account(Account { kind: AccountKind::Native, .. })]));
    let spec = l.spec().unwrap();
    assert_eq!(spec.harness, Harness::Antigravity);
    assert_eq!(spec.profile_id, None);
    assert_eq!(spec.provider_id, None);
    assert_eq!(spec.model, None);
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
        orchestrator_running: BTreeMap::new(),
        shared: false,
        jsh: None,
        studio: Default::default(),
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

/// The list as its kinds: `main` rows, then what follows the "connected" heading, then the fold and what's under it.
fn shape(l: &Launcher) -> (Vec<AccountKind>, Vec<AccountKind>, Option<(usize, bool)>, Vec<AccountKind>) {
    let (mut main, mut connected, mut fold, mut under) = (vec![], vec![], None, vec![]);
    let mut part = 0;
    for i in l.items() {
        match i {
            Item::Header("connected") => part = 1,
            Item::Header(h) => panic!("unexpected heading {h}"),
            Item::Others { count, open } => {
                fold = Some((count, open));
                part = 2;
            }
            Item::Account(a) => [&mut main, &mut connected, &mut under][part].push(a.kind),
        }
    }
    (main, connected, fold, under)
}

fn provider(id: &str) -> AccountKind {
    AccountKind::Provider(id.into())
}

#[test]
fn the_list_is_the_main_providers_the_connected_ones_and_a_fold() {
    let mut l = Launcher::new(data(), None, Place::Tab);
    let (main, connected, fold, under) = shape(&l);
    // your logins, the other family's, Gemini: no heading over them, the page titles the list
    assert!(matches!(&main[0], AccountKind::Profile(p) if p.starts_with("claude:")), "{main:?}");
    assert!(main.iter().any(|k| matches!(k, AccountKind::Profile(p) if p.starts_with("codex:"))));
    assert!(main.iter().all(|k| !matches!(k, AccountKind::Provider(_))), "no API provider among them: {main:?}");
    // a key makes a provider connected; the rest wait under the fold, closed
    assert_eq!(connected.len(), 2, "{connected:?}");
    assert!(connected.contains(&provider("openrouter")) && connected.contains(&provider("openai")));
    let (count, open) = fold.expect("some providers have no key");
    assert!(count > 0 && !open && under.is_empty());
    let items = l.items();
    assert!(!matches!(items[l.cursor_row(&items).unwrap()], Item::Header(_)));

    // Enter on the fold opens it, and again closes it; the cursor stays on it
    while !matches!(l.current(), Some(Item::Others { .. })) {
        key(&mut l, KeyCode::Down);
    }
    assert!(l.spec().is_none() && !l.needs_model());
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::None));
    let (_, _, fold, under) = shape(&l);
    assert_eq!((fold, under.len()), (Some((count, true)), count));
    assert!(under.contains(&provider("deepseek")), "{under:?}");
    assert!(matches!(l.current(), Some(Item::Others { open: true, .. })));
    // a provider under it says why it can't run yet
    key(&mut l, KeyCode::Down);
    let Some(Item::Account(a)) = l.current() else { panic!("a provider") };
    assert_eq!((a.ready, a.blocked), (false, Some("no key")));
    key(&mut l, KeyCode::Up);
    key(&mut l, KeyCode::Char(' '));
    assert_eq!(shape(&l).2, Some((count, false)), "space folds it too");
    assert!(l.project_picker.is_none(), "the space went to the fold, not the project picker");

    // a click on it opens it at once, wherever the cursor was
    l.list_sel = 0;
    let row = Launcher::selectable(&l.items()).len() - 1;
    assert!(l.pick_fold(row) && l.others_open && l.list_sel == row);
    assert!(!l.pick_fold(0), "a login is picked the usual way");

    // a filter looks under the fold; going straight to a provider there opens it
    let mut l = Launcher::new(data(), None, Place::Tab);
    narrow(&mut l, "deepseek");
    assert!(matches!(l.current(), Some(Item::Account(a)) if a.kind == provider("deepseek")));
    let l = Launcher::new(data(), None, Place::Tab).with_provider("deepseek").unwrap();
    assert!(l.others_open && matches!(l.current(), Some(Item::Account(a)) if a.kind == provider("deepseek")));
}

#[test]
fn providers_read_by_name() {
    // the catalogue's "Z.ai (GLM)" is Z.ai's row, with what the brackets say beside it
    let mut d = data();
    d.providers.iter_mut().find(|p| p.id == "zai").unwrap().name = "Z.ai (GLM)".into();
    let l = Launcher::new(d, None, Place::Tab).with_provider("zai").unwrap();
    let Some(Item::Account(a)) = l.current() else { panic!() };
    assert_eq!(a.label, "Z.ai");
    assert!(a.detail.starts_with("GLM · "), "{}", a.detail);
    let l = Launcher::new(data(), None, Place::Tab).with_provider("openrouter").unwrap();
    let Some(Item::Account(a)) = l.current() else { panic!() };
    assert_eq!(a.label, "OpenRouter");
    assert!(a.detail.ends_with(" models") && a.blocked.is_none(), "{}", a.detail);
}

#[test]
fn the_just_superhuman_account_comes_straight_after_your_logins() {
    use bro_core::jsh::{Credits, PROVIDER_ID, provider_for};
    let mut d = data();
    let account = bro_core::jsh::Account {
        organization: bro_core::jsh::Organization { id: "demo-org".into(), name: "Rivera Studio".into(), slug: "rivera-studio".into() },
        credits: Credits { available: 2_250, monthly_allowance: 3_000, used: Some(750), ..Default::default() },
        claude: bro_core::jsh::ClaudeInfo { available: true, models: vec![bro_core::jsh::ClaudeModel { id: "claude-opus-5-5".into(), name: Some("Opus 5.5".into()) }], ..Default::default() },
        ..Default::default()
    };
    d.providers.push(provider_for(&account, bro_core::jsh::PRODUCTION_ORIGIN));
    d.keyed.push(PROVIDER_ID.into());
    d.models.insert(PROVIDER_ID.into(), vec![bro_core::catalogue::ModelRow { id: "claude-opus-5-5".into(), name: "Opus 5.5".into(), context: None, pricing: None, reasoning: false, created: None }]);
    d.jsh = Some(account.clone());
    let l = Launcher::new(d.clone(), None, Place::Tab);
    let items = l.items();
    let (main, connected, _, _) = shape(&l);
    // after the Claude logins and the pool, ahead of the ChatGPT logins through the proxy
    let at = main.iter().position(|k| *k == provider(PROVIDER_ID)).expect("a main provider");
    assert!(main[..at].iter().all(|k| *k == AccountKind::Pool || matches!(k, AccountKind::Profile(p) if p.starts_with("claude:"))), "{main:?}");
    assert!(matches!(&main[at + 1], AccountKind::Profile(p) if p.starts_with("codex:")), "{main:?}");
    assert!(!connected.contains(&provider(PROVIDER_ID)), "one row for the account, not one more among the connected");
    let rows: Vec<&Account> = items.iter().filter_map(|i| match i { Item::Account(a) if a.kind == AccountKind::Provider(PROVIDER_ID.into()) => Some(a), _ => None }).collect();
    let [row] = rows[..] else { panic!("one row for the account: {rows:?}") };
    assert_eq!((row.label.as_str(), row.detail.as_str(), row.ready, row.left), ("Just Superhuman", "Rivera Studio · 2,250 credits", true, Some(75.0)));
    assert!(matches!(l.current(), Some(Item::Account(a)) if a.kind != row.kind), "your own login stays the default");

    // typing the service's name finds it
    let mut found = Launcher::new(d.clone(), None, Place::Tab);
    narrow(&mut found, "superhuman");
    assert!(matches!(found.current(), Some(Item::Account(a)) if a.kind == row.kind));

    // straight to it, as the usage view's card and the sidebar's line go
    let on = Launcher::new(d.clone(), None, Place::Tab).with_provider(PROVIDER_ID).unwrap();
    let spec = on.spec().unwrap();
    assert_eq!((spec.provider_id.as_deref(), spec.model.as_deref()), (Some(PROVIDER_ID), Some("claude-opus-5-5")));
    // Claude can't run there: the row says why
    d.jsh.as_mut().unwrap().claude = bro_core::jsh::ClaudeInfo { available: false, reason: "AI is paused for Rivera Studio.".into(), ..Default::default() };
    let l = Launcher::new(d, None, Place::Tab);
    let detail = l.items().into_iter().find_map(|i| match i { Item::Account(a) if a.kind == AccountKind::Provider(PROVIDER_ID.into()) => Some(a.detail), _ => None }).unwrap();
    assert_eq!(detail, "AI is paused for Rivera Studio.");
}

#[test]
fn recents_relaunch_and_remember_the_model() {
    let mut d = data();
    d.recents = vec![Recent { harness: Harness::Codex, profile_id: None, provider_id: Some("openrouter".into()), model: Some("z-ai/glm-5.3".into()), cwd: PathBuf::from("/code/justgains"), permission: Permission::Auto, browser: BrowserMode::Off, at: 1, orchestrator: false }];
    let mut l = Launcher::new(d, None, Place::Tab);
    assert_eq!(l.harness, Harness::Codex, "opens on the last harness");
    assert_eq!(l.dir, PathBuf::from("/code/justgains"));
    assert_eq!(l.permission, Permission::Skip, "always starts on skip");
    // the cursor starts on what it ran last, the model picked: one click runs it again
    assert!(matches!(l.current(), Some(Item::Account(a)) if a.kind == provider("openrouter")));
    let s = l.spec().unwrap();
    assert_eq!((s.provider_id.as_deref(), s.model.as_deref()), (Some("openrouter"), Some("z-ai/glm-5.3")));
    let quick = l.quick_spec().expect("the same combo needs no decision");
    assert_eq!((quick.provider_id.as_deref(), quick.model.as_deref()), (Some("openrouter"), Some("z-ai/glm-5.3")));
    // another model is a decision again
    l.model_sel += 1;
    assert!(l.quick_spec().is_none());
    // moving away and back starts on the model used last time
    key(&mut l, KeyCode::Up);
    assert!(l.quick_spec().is_none_or(|s| s.provider_id.is_none()), "another row is not that combo");
    key(&mut l, KeyCode::Down);
    assert_eq!(l.model().unwrap().id, "z-ai/glm-5.3");
    // each harness starts on its own last run: Claude has none, so its first login
    key(&mut l, KeyCode::Left);
    assert_eq!(l.harness, Harness::Claude);
    assert!(matches!(l.current(), Some(Item::Account(Account { kind: AccountKind::Profile(ref p), .. })) if p.starts_with("claude:")));
    key(&mut l, KeyCode::Right);
    assert!(matches!(l.current(), Some(Item::Account(a)) if a.kind == provider("openrouter")));
}

#[test]
fn toggles_and_esc() {
    let mut l = Launcher::new(data(), Some(PathBuf::from("/code/bro")), Place::Tab);
    assert_eq!(l.dir, PathBuf::from("/code/bro"), "starts in the current project");
    assert_eq!(l.permission, Permission::Skip, "skips permissions by default");
    ctrl(&mut l, 'e');
    assert_eq!(l.permission, Permission::Default);
    ctrl(&mut l, 's');
    ctrl(&mut l, 'b');
    assert_eq!(l.place, Place::Tab, "placement comes from the entry point, not a hidden shortcut");
    assert_eq!(l.browser, BrowserMode::Off);
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
    assert!(!l.orchestrator && l.harness == Harness::Antigravity);
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
    assert_eq!(l.harness, Harness::Antigravity, "wraps");
    key(&mut l, KeyCode::Right);
    assert!(!l.orchestrator && l.harness == Harness::Claude);
    key(&mut l, KeyCode::Left);
    assert!(l.orchestrator);
    key(&mut l, KeyCode::Left);
    assert!(!l.orchestrator && l.harness == Harness::Antigravity);

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
    assert_eq!(selected_profile(&l), Some(busy.clone()));
    // …and is passed over once it's nearly out
    d.usage.insert(busy.clone(), 95.0);
    let l = Launcher::new(d.clone(), None, Place::Tab).with_orchestrator();
    assert_eq!(selected_profile(&l), Some(roomy.clone()));

    // orchestrator combos stay on its tab: a plain session doesn't start on one…
    d.recents = vec![recent(&roomy)];
    let l = Launcher::new(d.clone(), None, Place::Tab).with_harness(Harness::Claude);
    assert_eq!(selected_profile(&l), Some(busy.clone()), "the first login, not the orchestrator's last");
    // …and plain ones stay off it: the roomiest login, not the one a session ran on last
    d.usage.insert(busy.clone(), 50.0);
    d.usage.insert(roomy.clone(), 20.0);
    let mut plain = recent(&busy);
    plain.orchestrator = false;
    d.recents = vec![plain];
    let l = Launcher::new(d.clone(), None, Place::Tab).with_orchestrator();
    assert_eq!(selected_profile(&l), Some(roomy));
    let l = Launcher::new(d, None, Place::Tab).with_harness(Harness::Claude);
    assert_eq!(selected_profile(&l), Some(busy), "a plain session starts where it last ran");
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
    assert_eq!((a.label.as_str(), a.ready, a.blocked), ("Gemini", false, Some("sign in with Google")));
    assert!(matches!(shape(&Launcher::new(l.data.clone(), None, Place::Tab)).0.last(), Some(k) if *k == provider("gemini")), "a main provider, not one under the fold");
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::SignIn));

    let mut d = data();
    d.profiles.retain(|p| !p.is_gemini());
    d.keyed.push("gemini".into());
    let mut l = Launcher::new(d, None, Place::Tab);
    narrow(&mut l, "google");
    let Some(Item::Account(a)) = l.current() else { panic!("gemini row") };
    assert!(a.ready && a.blocked.is_none() && a.detail.contains("Google AI subscription"), "{}", a.detail);
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
    // each family is one row, Claude's own first
    assert_eq!(shape(&l).0[..1], [AccountKind::Shared(true)]);
    assert!(shape(&l).0.contains(&AccountKind::Shared(false)));
    // combos naming one login (or the pool) have no row to start on: the newest that has one
    assert!(matches!(l.current(), Some(Item::Account(a)) if a.kind == provider("openrouter")));
    assert_eq!(l.quick_spec().and_then(|s| s.model).as_deref(), Some("qwen/qwen3-coder"));

    select(&mut l, AccountKind::Shared(true));
    let Some(Item::Account(own)) = l.current() else { panic!() };
    assert_eq!(own.label, "Claude");
    assert!(own.detail.ends_with(" logins · proxy mode") && !own.detail.contains("work"), "how many, not who: {}", own.detail);
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

    let Some(Item::Account(other)) = l.current() else { panic!() };
    assert_eq!(other.label, "ChatGPT");

    // a recent of the shared logins is their row
    assert_eq!(l.recent_kind(&recent(Harness::Claude, None, None, None)), AccountKind::Shared(true));
    assert_eq!(l.recent_kind(&recent(Harness::Claude, None, Some(bro_core::launch::LOGINS_CODEX), Some("gpt-5.5"))), AccountKind::Shared(false));
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
    assert_eq!(l.recent_kind(&recent(Harness::Claude, None, None, None)), AccountKind::Native);
}

// ------------------------------------------------------------------ Create Slop

/// The launcher with the demo's roster and toolkit, as the services publish them.
fn slop_data() -> Data {
    let (tx, _rx) = std::sync::mpsc::channel();
    let svc = Services::offline(fallback_settings(), tx);
    svc.refresh_studio(false);
    let studio = svc.state().studio.clone();
    Data { studio, ..data() }
}

fn slop() -> Launcher {
    let mut l = Launcher::new(slop_data(), Some(PathBuf::from("/films/commercial")), Place::Tab).with_studio();
    // a grid four cards wide, as the window would draw it
    l.studio.cols = 4;
    l
}

fn alt(l: &mut Launcher, c: char) -> Outcome {
    l.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
}

fn cast_names(l: &Launcher) -> Vec<String> {
    l.studio.picked.iter().map(|c| c.name.clone()).collect()
}

#[test]
fn create_slop_opens_from_the_keyboard_and_asks_for_its_roster_once() {
    let mut l = Launcher::new(data(), None, Place::Tab);
    assert_eq!(l.studio.ask, None, "nothing is fetched until the tab opens");
    alt(&mut l, 's');
    assert_eq!((l.tab, l.studio.ask.take()), (SessionTab::Slop, Some(false)));
    alt(&mut l, 'n');
    assert_eq!(l.tab, SessionTab::New);
    alt(&mut l, 's');
    assert_eq!(l.studio.ask, None);
    // ctrl+r fetches it again
    ctrl(&mut l, 'r');
    assert_eq!(l.studio.ask.take(), Some(true));
    // the orchestrator is its own kind of session: opening the tab from there leaves it
    let mut l = Launcher::new(data(), None, Place::Tab).with_orchestrator();
    alt(&mut l, 's');
    assert!(!l.orchestrator && l.spec().is_some_and(|s| !s.orchestrator && s.studio.is_some()));
}

#[test]
fn create_slop_casts_with_space_and_finds_by_typing() {
    let mut l = slop();
    assert_eq!(l.studio_current().unwrap().name, "Claudia", "Claudia leads the roster");
    key(&mut l, KeyCode::Char(' '));
    key(&mut l, KeyCode::Right);
    key(&mut l, KeyCode::Down);
    assert_eq!(l.studio.sel, 5, "one along, one row down");
    key(&mut l, KeyCode::Char(' '));
    assert_eq!(cast_names(&l), ["Claudia", "Astra"]);
    assert_eq!(l.studio_cast_names().as_deref(), Some("Claudia and Astra"));
    // typing looks for a character; the cursor goes to the best match
    typ(&mut l, "becky");
    assert_eq!((l.studio.filter.as_str(), l.studio.sel), ("becky", 0));
    assert_eq!(l.studio_current().unwrap().name, "Becky");
    key(&mut l, KeyCode::Char(' '));
    assert_eq!(l.studio_cast_names().as_deref(), Some("Claudia, Astra and Becky"));
    let more: Vec<_> = l.data.studio.cast.iter().filter(|c| c.name == "Rook" || c.name == "Gustav").cloned().collect();
    l.studio.picked.extend(more);
    assert_eq!(l.studio_cast_names().as_deref(), Some("Claudia, Astra and 3 more"));
    l.studio.picked.truncate(3);
    // a name is found as it is spelled: at its start first, then inside it, then by who made it or a tag
    let found = |l: &mut Launcher, q: &str| {
        l.studio.filter = q.into();
        l.studio_cast().iter().map(|&i| l.data.studio.cast[i].name.clone()).collect::<Vec<_>>()
    };
    assert_eq!(found(&mut l, "ro"), ["Rook"], "not every name with an r and an o in it");
    assert_eq!(found(&mut l, "bone"), ["Chinese Bone"]);
    assert_eq!(found(&mut l, "Chinese Bone"), ["Chinese Bone"], "spaces don't matter");
    assert_eq!(found(&mut l, "marty"), ["Becky", "Hildegarde", "Deep Singer"]);
    assert_eq!(found(&mut l, "popsinger"), ["Claudia"], "a tag");
    assert_eq!(found(&mut l, "hl"), ["Hollis", "Hildegarde", "Hal Denning"], "nothing spelled that way: the letters in order, best first");
    assert!(found(&mut l, "zzz").is_empty());
    l.studio.filter = "becky".into();
    // space on someone cast takes them out; esc clears the search before it closes
    key(&mut l, KeyCode::Char(' '));
    assert_eq!(cast_names(&l), ["Claudia", "Astra"]);
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::None));
    assert!(l.studio.filter.is_empty());
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::Close));
    // the keys stay in range at the edges
    let mut l = slop();
    key(&mut l, KeyCode::Left);
    key(&mut l, KeyCode::Up);
    assert_eq!(l.studio.sel, 0);
    key(&mut l, KeyCode::End);
    let last = l.studio_cast().len() - 1;
    assert_eq!(l.studio.sel, last);
    key(&mut l, KeyCode::Right);
    key(&mut l, KeyCode::Down);
    assert_eq!(l.studio.sel, last);
    // five across leaves a short last row: a row down from above its gap lands on its last card
    l.studio.cols = 5;
    l.studio.sel = 8;
    key(&mut l, KeyCode::Down);
    assert_eq!(l.studio.sel, last);
    key(&mut l, KeyCode::Up);
    assert_eq!(l.studio.sel, last - 5);
    // pasted text looks for a character too
    l.paste("  astra\n");
    assert_eq!(l.studio_current().unwrap().name, "Astra");
}

#[test]
fn create_slop_tools_are_on_until_switched_off() {
    use bro_core::studio::{ELEVENLABS, FFMPEG, HIGGSFIELD, JUSTIMAGINE, REMOTION, SLOPCANNON};
    let mut l = slop();
    assert_eq!(l.studio_kit().tools, [JUSTIMAGINE, ELEVENLABS, HIGGSFIELD, SLOPCANNON, FFMPEG, REMOTION]);
    // tab takes the keyboard to the toolkit; space switches the tool under the cursor
    key(&mut l, KeyCode::Tab);
    assert_eq!(l.studio.focus, studio::StudioFocus::Tools);
    key(&mut l, KeyCode::Down);
    key(&mut l, KeyCode::Down);
    key(&mut l, KeyCode::Char(' '));
    assert!(!l.studio_tool_on(HIGGSFIELD));
    assert_eq!(l.studio_kit().tools, [JUSTIMAGINE, ELEVENLABS, SLOPCANNON, FFMPEG, REMOTION]);
    key(&mut l, KeyCode::Char(' '));
    assert!(l.studio_tool_on(HIGGSFIELD));
    for _ in 0..20 {
        key(&mut l, KeyCode::Down);
    }
    assert_eq!(l.studio.tool_sel, bro_core::studio::TOOLS.len() - 1);
    // esc comes back to the cast before it closes
    assert!(matches!(key(&mut l, KeyCode::Esc), Outcome::None));
    assert_eq!(l.studio.focus, studio::StudioFocus::Cast);
    // typing in the toolkit looks for a character
    key(&mut l, KeyCode::Tab);
    typ(&mut l, "rook");
    assert_eq!((l.studio.focus, l.studio_current().unwrap().name.as_str()), (studio::StudioFocus::Cast, "Rook"));

    // a tool the machine doesn't have stays off, whatever is pressed
    let mut d = slop_data();
    d.studio.tools = bro_core::studio::statuses(&bro_core::studio::Facts::default());
    let mut l = Launcher::new(d, None, Place::Tab).with_studio();
    assert_eq!(l.studio_kit().tools, [HIGGSFIELD, SLOPCANNON]);
    l.studio_toggle_tool(FFMPEG);
    assert!(!l.studio_tool_on(FFMPEG));
}

#[test]
fn create_slop_launches_with_its_kit_on_what_the_new_tab_selected() {
    let mut l = slop();
    key(&mut l, KeyCode::Char(' '));
    let s = launched(key(&mut l, KeyCode::Enter));
    assert_eq!((s.harness, s.cwd.clone(), s.orchestrator), (Harness::Claude, PathBuf::from("/films/commercial"), false));
    let kit = s.studio.expect("the launch carries the kit");
    assert_eq!(kit.cast.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Claudia"]);
    assert_eq!(kit.tools.len(), bro_core::studio::TOOLS.len());
    // the same launcher on the New tab launches a plain session
    alt(&mut l, 'n');
    assert!(l.spec().unwrap().studio.is_none());

    // shift+←/→ changes the agent, without the orchestrator in the round
    let mut l = slop();
    for h in Harness::ALL.into_iter().cycle().skip(1).take(Harness::ALL.len()) {
        l.key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!((l.harness, l.orchestrator, l.tab), (h, false, SessionTab::Slop));
    }
    l.key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(l.harness, Harness::Antigravity);
    // ctrl+e steps the permissions here too
    let before = l.permission;
    ctrl(&mut l, 'e');
    assert_ne!(l.permission, before);

    // nothing to run on yet (a provider wants a model picked): enter goes to where that's chosen
    let mut l = slop();
    alt(&mut l, 'n');
    narrow(&mut l, "openrouter");
    assert!(l.needs_model());
    l.model_sel = usize::MAX;
    alt(&mut l, 's');
    assert!(l.spec().is_none());
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::None));
    assert_eq!((l.tab, l.focus), (SessionTab::New, Focus::Models));

    // an agent that isn't installed is installed first
    let mut d = slop_data();
    d.installed = vec![(Harness::Claude, false)];
    let mut l = Launcher::new(d, None, Place::Tab).with_studio();
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::Install(Harness::Claude)));

    // before the machine has been looked at there is no toolkit to launch with
    let mut l = Launcher::new(data(), None, Place::Tab).with_studio();
    assert!(!l.studio_ready());
    assert!(matches!(key(&mut l, KeyCode::Enter), Outcome::None));
    assert_eq!(l.tab, SessionTab::Slop);
}

#[test]
fn create_slop_starts_from_the_cast_the_project_last_cooked_with() {
    use bro_core::studio::{Character, Kit};
    let film = PathBuf::from("/films/commercial");
    let other = PathBuf::from("/films/other");
    let mut d = slop_data();
    let becky = d.studio.cast.iter().find(|c| c.name == "Becky").cloned().unwrap();
    d.studio.remembered = vec![(film.clone(), Kit { cast: vec![Character::claudia(), becky], tools: vec![] })];
    let mut l = Launcher::new(d.clone(), Some(film.clone()), Place::Tab).with_studio();
    assert_eq!(cast_names(&l), ["Claudia", "Becky"]);
    // another project has no cast yet; coming back brings this one's
    l.select_project(other.clone());
    assert!(l.studio.picked.is_empty());
    l.select_project(film.clone());
    assert_eq!(cast_names(&l), ["Claudia", "Becky"]);
    // once you change the cast it is yours: a refresh or another project leaves it alone
    key(&mut l, KeyCode::Char(' '));
    assert_eq!(cast_names(&l), ["Becky"]);
    l.set_data(d.clone());
    l.select_project(other);
    assert_eq!(cast_names(&l), ["Becky"]);

    // the roster arriving after the tab opened still brings the project's cast
    let mut l = Launcher::new(data(), Some(film), Place::Tab).with_studio();
    assert!(l.studio.picked.is_empty());
    l.set_data(d.clone());
    assert_eq!(cast_names(&l), ["Claudia", "Becky"]);
    // a character published again since shows as its newest version
    let mut newer = d.clone();
    newer.studio.cast.iter_mut().find(|c| c.name == "Becky").unwrap().version = 7;
    l.set_data(newer);
    assert_eq!(l.studio.picked[1].version, 7);
    // on the New tab nothing is cast behind your back
    let l = Launcher::new(d, Some(PathBuf::from("/films/commercial")), Place::Tab);
    assert!(l.studio.picked.is_empty());
}

#[test]
fn create_slop_draws_in_the_terminal_and_takes_clicks() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut l = slop();
    let theme = crate::theme::get("graphite");
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    term.draw(|f| view::draw(f, f.area(), &mut l, &theme, 0.0)).unwrap();
    let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    for want in ["Create Slop", "runs on", "Claudia", "Chinese Bone", "toolkit", "JustImagine", "running on port 8793", "Start cooking", "characters · character.slop.cc"] {
        assert!(text.contains(want), "the tab shows {want:?}");
    }
    assert_eq!(l.studio.cols, 1, "a list here: ↑/↓ move a row");
    // a click on a character casts it, one on a tool switches it, the button starts
    let at = |l: &Launcher, hit: Hit| l.hits.iter().find(|(_, h)| *h == hit).map(|(r, _)| ratatui::layout::Position::new(r.x + 1, r.y)).unwrap();
    let p = at(&l, Hit::Cast(1));
    l.click(p, KeyModifiers::NONE);
    assert_eq!(cast_names(&l), ["Chinese Bone"]);
    let p = at(&l, Hit::Tool(4));
    l.click(p, KeyModifiers::NONE);
    assert!(!l.studio_tool_on(bro_core::studio::FFMPEG));
    let p = at(&l, Hit::StudioRefresh);
    l.click(p, KeyModifiers::NONE);
    assert_eq!(l.studio.ask.take(), Some(true));
    let p = at(&l, Hit::Harness(Harness::Codex));
    l.click(p, KeyModifiers::NONE);
    assert_eq!((l.harness, l.tab), (Harness::Codex, SessionTab::Slop));
    let p = at(&l, Hit::Launch);
    let s = launched(l.click(p, KeyModifiers::NONE));
    assert_eq!(s.harness, Harness::Codex);
    assert_eq!(s.studio.unwrap().cast.len(), 1);
    // narrow terminals keep the cast and say which tools are on
    let mut term = Terminal::new(TestBackend::new(70, 24)).unwrap();
    term.draw(|f| view::draw(f, f.area(), &mut l, &theme, 0.0)).unwrap();
    let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(text.contains("Chinese Bone") && text.contains("tools") && text.contains("JustImagine"), "{text}");
}
