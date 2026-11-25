use super::*;

const MIN: u64 = 60_000;

fn p(pid: u32, ppid: u32, started: u64, exe: &str) -> Proc {
    Proc { pid, ppid, started, cpu_ms: 0, mem: 100 << 20, exe: exe.into(), path: format!("C:/x/{exe}"), cmd: String::new(), held: false }
}

/// A session: claude (pid 10) with whatever else runs in it.
fn session(extra: Vec<Proc>) -> Vec<Proc> {
    let mut all = vec![p(10, 1, 1, "claude.exe")];
    all.extend(extra);
    all
}

/// A Remotion render's browser whose node (pid 30) is gone, with two renderers under it.
fn remotion_leftover() -> Vec<Proc> {
    let path = r"F:\work\film\node_modules\.remotion\chrome-headless-shell\win64\chrome-headless-shell.exe";
    let mut main = p(40, 30, 50, "chrome-headless-shell.exe");
    main.path = path.into();
    main.cmd = format!(r"{path} about:blank --headless --remote-debugging-port=0 --user-data-dir=C:\Temp\puppeteer_dev_chrome_profile-AbC123");
    let mut kids = vec![];
    for (pid, started) in [(41, 51), (42, 52)] {
        let mut k = p(pid, 40, started, "chrome-headless-shell.exe");
        k.path = path.into();
        k.cmd = format!("{path} --type=renderer");
        kids.push(k);
    }
    let mut all = vec![main];
    all.extend(kids);
    all
}

/// A hung script (its shell's parent gone) still holding the browser it drives.
fn hung_test_run() -> Vec<Proc> {
    let mut chrome = p(40, 30, 50, "chrome.exe");
    chrome.cmd = "chrome.exe --headless=new --remote-debugging-port=0".into();
    let mut node = p(30, 20, 30, "node.exe");
    node.cmd = r#""C:\Program Files\nodejs\node.exe" F:\app\node_modules\playwright\cli.js test"#.into();
    session(vec![p(20, 77, 20, "bash.exe"), node, chrome])
}

struct Run {
    c: Collector,
    ended: Vec<u32>,
}

impl Run {
    fn new() -> Run {
        Run { c: Collector::new(None), ended: vec![] }
    }

    fn look(&mut self, procs: &[Proc], around: &Around, s: &SlopSettings, now: u64, by_hand: bool) -> Report {
        let who = [Who { session: "s1", label: "Night Before", root: Some(10) }];
        let ended = &mut self.ended;
        self.c.decide(&who, &[procs.to_vec()], around, s, now, by_hand, &mut |_, group| {
            ended.extend(group.iter().map(|p| p.pid));
            group.len() as u32
        })
    }
}

fn on() -> SlopSettings {
    SlopSettings { on: true, ..SlopSettings::default() }
}

#[test]
fn a_browser_whose_script_is_gone_is_closed_once_the_wait_runs_out() {
    let procs = session(remotion_leftover());
    let mut r = Run::new();
    let first = r.look(&procs, &Around::default(), &on(), 0, false);
    assert_eq!(first.leftovers.len(), 1, "nothing can drive it: a leftover from the first look");
    let l = &first.leftovers[0];
    assert_eq!((l.title.as_str(), l.what.as_str()), ("Chrome from Remotion", "Remotion's headless Chrome"));
    assert_eq!(l.project.as_deref(), Some("film"));
    assert_eq!((l.processes, l.reason, l.due_secs), (3, Reason::Unattended, Some(600)));
    assert_eq!(l.session_label, "Night Before");

    // CPU doesn't hold back a browser nothing can drive
    let mut busy = procs.clone();
    busy[1].cpu_ms = 90_000;
    let later = r.look(&busy, &Around::default(), &on(), 5 * MIN, false);
    assert_eq!(later.leftovers[0].due_secs, Some(300));
    assert!(r.ended.is_empty());

    let done = r.look(&busy, &Around::default(), &on(), 10 * MIN, false);
    assert!(done.leftovers.is_empty());
    assert_eq!(r.ended, [40, 41, 42], "the browser first, then what ran under it");
    assert_eq!((done.ledger.cleanups, done.ledger.processes, done.ledger.memory), (1, 3, 300 << 20));
    let c = &done.ledger.recent[0];
    assert_eq!((c.name(), c.waited_secs, c.by_hand), ("Chrome from Remotion", 600, false));
    assert_eq!(r.c.released, ["puppeteer_dev_chrome_profile-abc123".to_string()].into(), "its profile is free to clear now");
}

#[test]
fn the_sessions_own_tree_is_never_slop() {
    // claude → bash → node → chrome: all still there
    let mut chrome = p(40, 30, 50, "chrome.exe");
    chrome.cmd = "chrome.exe --headless --remote-debugging-pipe".into();
    let procs = session(vec![p(20, 10, 20, "bash.exe"), p(30, 20, 30, "node.exe"), chrome]);
    let mut r = Run::new();
    let rep = r.look(&procs, &Around::default(), &on(), 0, true);
    assert!(rep.leftovers.is_empty() && rep.others.groups == 0);
    assert!(r.ended.is_empty(), "not even by hand");
    assert_eq!((rep.sessions, rep.processes), (1, 4));
}

#[test]
fn a_process_whose_parent_lives_elsewhere_is_still_held() {
    let mut procs = session(remotion_leftover());
    procs[1].held = true;
    let mut r = Run::new();
    let rep = r.look(&procs, &Around::default(), &on(), 0, true);
    assert!(rep.leftovers.is_empty() && r.ended.is_empty());
}

#[test]
fn a_parent_pid_held_by_a_newer_process_counts_as_gone() {
    // pid 30 is alive, but it started after the browser did: it isn't the browser's parent
    let mut procs = session(remotion_leftover());
    procs.push(p(30, 10, 99, "node.exe"));
    let rep = Run::new().look(&procs, &Around::default(), &on(), 0, false);
    assert_eq!(rep.leftovers.len(), 1);
    assert_eq!(rep.leftovers[0].processes, 3);
}

#[test]
fn windows_and_connections_from_outside_keep_a_browser_and_start_its_clock_over() {
    let procs = session(remotion_leftover());
    let s = on();
    let mut window = Around::default();
    window.windows.insert(41);
    let mut connected = Around::default();
    connected.listening.insert(40, vec![9222]);
    connected.clients.insert(40, vec![555]);
    let mut remote = Around::default();
    remote.listening.insert(40, vec![9222]);
    remote.clients.insert(40, vec![0]);
    for around in [&window, &connected, &remote] {
        let mut r = Run::new();
        let rep = r.look(&procs, around, &s, 0, true);
        assert!(rep.leftovers.is_empty(), "{around:?}");
        assert!(r.ended.is_empty());
    }
    // a browser listening with nobody connected is still a leftover
    let mut idle = Around::default();
    idle.listening.insert(40, vec![9222]);
    assert_eq!(Run::new().look(&procs, &idle, &s, 0, false).leftovers.len(), 1);

    // used at 8 minutes: unused counts from then, not from when it was first seen
    let mut r = Run::new();
    r.look(&procs, &idle, &s, 0, false);
    r.look(&procs, &connected, &s, 8 * MIN, false);
    let rep = r.look(&procs, &idle, &s, 12 * MIN, false);
    assert_eq!((rep.leftovers[0].waited_secs, rep.leftovers[0].due_secs), (240, Some(360)));
    assert!(r.ended.is_empty());
}

#[test]
fn a_render_server_feeding_its_own_browser_keeps_nothing_but_a_studio_or_dev_server_does() {
    let s = on();
    let mut procs = hung_test_run();
    // node serves the page its own Chrome loads
    let mut own = Around::default();
    own.listening.insert(30, vec![3000]);
    own.clients.insert(30, vec![40, 40]);
    let mut r = Run::new();
    r.look(&procs, &own, &s, 0, false);
    let rep = r.look(&procs, &own, &s, MIN, false);
    assert_eq!(rep.leftovers.len(), 1, "a render's server isn't one you use");

    // you open it in your browser: in use
    let mut yours = Around::default();
    yours.listening.insert(30, vec![3000]);
    yours.clients.insert(30, vec![40, 9001]);
    assert!(r.look(&procs, &yours, &s, 2 * MIN, false).leftovers.is_empty());

    // Remotion Studio is a server, whoever's connected
    procs[2].cmd = r#"node F:\film\node_modules\@remotion\cli\remotion-cli.js studio"#.into();
    let mut r = Run::new();
    r.look(&procs, &own, &s, 0, false);
    assert!(r.look(&procs, &own, &s, MIN, true).leftovers.is_empty());
    assert!(r.ended.is_empty());

    // and so is anything listening in a group with no browser in it
    let mut vite = p(30, 20, 30, "node.exe");
    vite.cmd = r#"node F:\site\node_modules\vite\bin\vite.js"#.into();
    let procs = session(vec![p(20, 77, 20, "bash.exe"), vite]);
    let mut serving = Around::default();
    serving.listening.insert(30, vec![5173]);
    let s = SlopSettings { anything_idle: true, ..on() };
    let mut r = Run::new();
    r.look(&procs, &serving, &s, 0, false);
    assert!(r.look(&procs, &serving, &s, 60 * MIN, true).leftovers.is_empty());
    assert!(r.ended.is_empty());
}

#[test]
fn a_hung_script_and_its_browser_are_closed_once_they_go_quiet() {
    let mut procs = hung_test_run();
    let s = on();
    let mut r = Run::new();
    let rep = r.look(&procs, &Around::default(), &s, 0, false);
    assert!(rep.leftovers.is_empty(), "first sight: it can't tell working from stuck yet");
    assert_eq!(rep.others.groups, 1);

    let rep = r.look(&procs, &Around::default(), &s, MIN, false);
    let l = &rep.leftovers[0];
    assert_eq!((l.reason, l.title.as_str(), l.what.as_str(), l.project.as_deref()), (Reason::Quiet, "Chrome from Playwright", "Playwright's headless Chrome", Some("app")));
    assert_eq!(l.waited_secs, 60);

    // it does some work at 4 minutes: working, so not a leftover, and the wait starts over
    procs[2].cpu_ms = 20_000;
    let rep = r.look(&procs, &Around::default(), &s, 4 * MIN, false);
    assert!(rep.leftovers.is_empty());
    assert_eq!(rep.others.names, ["Chrome from Playwright"]);
    // a little CPU (2% of a core or less) still counts as quiet
    procs[2].cpu_ms += 1_000;
    let rep = r.look(&procs, &Around::default(), &s, 9 * MIN, false);
    assert_eq!(rep.leftovers[0].due_secs, Some(300));
    let rep = r.look(&procs, &Around::default(), &s, 13 * MIN, false);
    assert_eq!(rep.leftovers.len(), 1, "quiet for 9 minutes: not yet");
    assert!(r.ended.is_empty());
    let rep = r.look(&procs, &Around::default(), &s, 14 * MIN, false);
    assert!(rep.leftovers.is_empty());
    assert_eq!(r.ended, [20, 30, 40]);
    assert_eq!(rep.ledger.recent[0].reason, Reason::Quiet);
}

#[test]
fn a_new_process_in_a_quiet_group_means_it_is_working() {
    let mut chrome = p(40, 30, 50, "chrome.exe");
    chrome.cmd = "chrome.exe --headless".into();
    let mut procs = session(vec![p(30, 77, 30, "node.exe"), chrome]);
    let mut r = Run::new();
    r.look(&procs, &Around::default(), &on(), 0, false);
    procs.push(p(43, 40, 60, "chrome.exe"));
    let rep = r.look(&procs, &Around::default(), &on(), 9 * MIN, false);
    assert!(rep.leftovers.is_empty());
    let rep = r.look(&procs, &Around::default(), &on(), 10 * MIN, false);
    assert_eq!(rep.leftovers[0].due_secs, Some(540));
}

#[test]
fn other_detached_processes_are_yours_unless_anything_idle_is_on() {
    let mut vite = p(30, 20, 30, "node.exe");
    vite.cmd = r#""C:\Program Files\nodejs\node.exe" F:\site\node_modules\vite\bin\vite.js"#.into();
    vite.mem = 300 << 20;
    let procs = session(vec![p(20, 66, 20, "bash.exe"), vite]);
    let mut r = Run::new();
    let rep = r.look(&procs, &Around::default(), &on(), 0, true);
    assert!(rep.leftovers.is_empty() && r.ended.is_empty());
    assert_eq!((rep.others.groups, rep.others.processes, rep.others.memory), (1, 2, 400 << 20));
    assert_eq!(rep.others.names, ["vite"]);

    let s = SlopSettings { anything_idle: true, ..on() };
    let mut r = Run::new();
    r.look(&procs, &Around::default(), &s, 0, false);
    let rep = r.look(&procs, &Around::default(), &s, MIN, false);
    let l = &rep.leftovers[0];
    assert_eq!((l.title.as_str(), l.what.as_str(), l.project.as_deref()), ("vite", "node · vite", Some("site")));
    r.look(&procs, &Around::default(), &s, 11 * MIN, false);
    assert_eq!(r.ended, [20, 30]);
}

#[test]
fn listing_only_until_you_say_and_clean_up_now_closes_what_it_lists() {
    let procs = session(remotion_leftover());
    let s = SlopSettings { collect: false, ..on() };
    let mut r = Run::new();
    let rep = r.look(&procs, &Around::default(), &s, 0, false);
    assert_eq!(rep.leftovers[0].due_secs, None);
    let rep = r.look(&procs, &Around::default(), &s, 600 * MIN, false);
    assert_eq!(rep.leftovers.len(), 1);
    assert!(r.ended.is_empty());
    let rep = r.look(&procs, &Around::default(), &s, 601 * MIN, true);
    assert!(rep.leftovers.is_empty());
    assert_eq!(r.ended.len(), 3);
    assert!(rep.ledger.recent[0].by_hand);
}

#[test]
fn clean_up_now_leaves_what_is_working() {
    let mut chrome = p(40, 30, 50, "chrome.exe");
    chrome.cmd = "chrome.exe --headless".into();
    let mut procs = session(vec![p(30, 77, 30, "node.exe"), chrome]);
    let mut r = Run::new();
    // first sight, and then working: never listed, never closed
    assert!(r.look(&procs, &Around::default(), &on(), 0, true).leftovers.is_empty());
    procs[1].cpu_ms = 10_000;
    let rep = r.look(&procs, &Around::default(), &on(), MIN, true);
    assert!(rep.leftovers.is_empty());
    assert!(r.ended.is_empty());
}

#[test]
fn remotion_helpers_and_browser_drivers_without_their_script_are_slop() {
    let mut comp = p(50, 30, 60, "remotion.exe");
    comp.path = r"F:\work\film\node_modules\@remotion\compositor-win32-x64-msvc\remotion.exe".into();
    let rep = Run::new().look(&session(vec![comp]), &Around::default(), &on(), 0, false);
    let l = &rep.leftovers[0];
    assert_eq!((l.title.as_str(), l.what.as_str(), l.project.as_deref(), l.reason), ("Video helper from Remotion", "Remotion's compositor", Some("film"), Reason::Unattended));

    let mut ffmpeg = p(50, 30, 60, "ffmpeg.exe");
    ffmpeg.path = r"F:\work\film\node_modules\@remotion\compositor-win32-x64-msvc\ffmpeg.exe".into();
    assert_eq!(Run::new().look(&session(vec![ffmpeg]), &Around::default(), &on(), 0, false).leftovers[0].title, "Video encoder from Remotion");

    // Selenium's chromedriver, its script gone, with its Chrome: listening on its port isn't serving you
    let mut driver = p(50, 30, 60, "chromedriver.exe");
    driver.path = r"C:\Users\j\.cache\selenium\chromedriver\win64\131\chromedriver.exe".into();
    let mut chrome = p(51, 50, 61, "chrome.exe");
    chrome.cmd = "chrome.exe --enable-automation --remote-debugging-port=0".into();
    let mut around = Around::default();
    around.listening.insert(50, vec![9515]);
    let rep = Run::new().look(&session(vec![driver, chrome]), &around, &on(), 0, false);
    let l = &rep.leftovers[0];
    assert_eq!((l.title.as_str(), l.reason, l.processes), ("Chrome from Selenium", Reason::Unattended, 2));
}

#[test]
fn a_browser_you_opened_is_not_automated() {
    // an agent ran `start chrome` while Chrome was closed: it lives in the session, with a window
    let mut chrome = p(40, 30, 50, "chrome.exe");
    chrome.cmd = r#""C:\Program Files\Google\Chrome\Application\chrome.exe" https://example.com"#.into();
    let procs = session(vec![chrome]);
    let rep = Run::new().look(&procs, &Around::default(), &on(), 0, false);
    assert!(rep.leftovers.is_empty());
    assert_eq!(rep.others.groups, 1, "listed for you, never closed without anything_idle");
}

#[test]
fn names() {
    assert_eq!(script_name(r"C:\a\node_modules\@remotion\cli\remotion-cli.js"), "@remotion/cli");
    assert_eq!(script_name("F:/site/node_modules/.bin/../vite/bin/vite.js"), "vite");
    assert_eq!(script_name("scripts/verify-homies-editor.mjs"), "verify-homies-editor");
    assert_eq!(script_name("F:/api/src/index.ts"), "src");
    let mut npx = p(30, 77, 30, "node.exe");
    npx.cmd = r#""C:\Program Files\nodejs\node.exe" "C:\Program Files\nodejs/node_modules/npm/bin/npx-cli.js" remotion render"#.into();
    assert_eq!(project_of(&npx), None, "npm's own folder isn't a project");
    let mut cached = p(30, 77, 30, "node.exe");
    cached.cmd = r"node C:\Users\j\AppData\Local\npm-cache\_npx\0123456789abcdef\node_modules\x\cli.js".into();
    assert_eq!(project_of(&cached), None);
    assert_eq!(bytes(7_194_000_000), "6.7 GB");
    assert_eq!(bytes(312 << 20), "312 MB");
    assert_eq!(bytes(0), "0 MB");
    assert_eq!(and_list(&["a".into(), "b".into(), "c".into()]), "a, b and c");
}

#[test]
fn the_ledger_outlives_the_collector() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2-slop.json");
    let procs = session(remotion_leftover());
    let mut c = Collector::new(Some(path.clone()));
    let who = [Who { session: "s1", label: "x", root: Some(10) }];
    c.decide(&who, &[procs], &Around::default(), &on(), 1_000, true, &mut |_, g| g.len() as u32);
    let again = Collector::new(Some(path)).idle_report(&on());
    assert_eq!((again.ledger.cleanups, again.ledger.seq, again.ledger.recent.len()), (1, 1, 1));
}

/// Temp folders: cleared on their own while cleaning up is on, listed until you say while it isn't; the profile of a
/// browser it just closed joins that browser's cleanup.
#[test]
fn temp_folders_are_cleared_and_counted() {
    let temp = tempfile::tempdir().unwrap();
    let make = |name: &str, n: usize| {
        let d = temp.path().join(name);
        std::fs::create_dir_all(d.join("Default")).unwrap();
        std::fs::write(d.join("Default/Cookies"), vec![1u8; n]).unwrap();
        d
    };
    let profile = make("puppeteer_dev_chrome_profile-AbC123", 5_000);
    let other = make("puppeteer_dev_chrome_profile-Young1", 7_000);
    let none = || temp::InUse::default();
    let mut c = Collector::new(None).with_temp(temp.path().to_path_buf());

    // the browser goes, and its profile with it, though it's minutes old
    let who = [Who { session: "s1", label: "Night Before", root: Some(10) }];
    let mut rep = c.decide(&who, &[session(remotion_leftover())], &Around::default(), &on(), 1_000, true, &mut |_, g| g.len() as u32);
    c.sweep(&mut rep, &on(), 1_000, false, &none);
    assert!(!profile.exists() && other.exists(), "only the closed browser's profile skips the wait");
    assert_eq!(rep.temp.folders, 0);
    let last = &rep.ledger.recent[0];
    assert_eq!((rep.ledger.cleanups, last.processes, last.folders, last.disk), (1, 3, 1, 5_000), "one cleanup: the browser and its profile");
    assert_eq!(rep.ledger.disk, 5_000);

    // listing only: the rest waits for you, and the next look doesn't list again before it's due
    let listing = SlopSettings { collect: false, ..on() };
    let mut later = Collector::new(None).with_temp(temp.path().to_path_buf());
    std::fs::rename(&other, temp.path().join("puppeteer_dev_chrome_profile-Old222")).unwrap();
    let mut rep = later.idle_report(&listing);
    later.temp.as_mut().unwrap().list(std::time::SystemTime::now() + temp::PROFILE_AGE * 2, 0, &none(), &HashSet::new());
    later.sweep(&mut rep, &listing, 1, false, &none);
    assert_eq!((rep.temp.folders, rep.temp.bytes, rep.temp.clearing), (1, 7_000, false));
    assert_eq!(rep.temp.tools, ["Puppeteer"]);
    // with cleaning up on, what's listed goes
    later.temp.as_mut().unwrap().list(std::time::SystemTime::now() + temp::PROFILE_AGE * 2, 2, &none(), &HashSet::new());
    let go = SlopSettings { collect: true, ..on() };
    later.sweep(&mut rep, &go, 3, false, &none);
    assert_eq!(rep.temp.folders, 0);
    let c = &rep.ledger.recent[0];
    assert!(c.temp_only());
    assert_eq!((c.name(), c.what.as_str(), c.folders, c.disk), ("Temp files", "Puppeteer", 1, 7_000));
    assert!(!later.clearing());
}

/// A real session tree: a launcher starts a "headless browser" and exits, and the collector closes the browser it
/// left (and only that).
#[cfg(windows)]
#[test]
fn ends_a_real_leftover_in_a_session_job() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("leaf");
    let browser = dir.path().join("chrome-headless-shell.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &browser).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "proc_tree::tests::process_tree_worker", "--ignored", "--nocapture"])
        .env("BRO_PROCESS_TEST_MODE", "intermediate-root")
        .env("BRO_PROCESS_TEST_MARKER", &marker)
        .env("BRO_PROCESS_TEST_GUI", &browser)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut root, tree) = crate::proc_tree::ProcessTree::spawn(&mut cmd).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let leaf: u32 = loop {
        if let Some(pid) = std::fs::read_to_string(&marker).ok().and_then(|s| s.trim().parse().ok()) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the leaf never started");
        std::thread::sleep(Duration::from_millis(20));
    };
    // the launcher between them exits once the leaf is up: wait until the leaf is detached
    let target = Target { session: "s1".into(), label: "test".into(), scope: tree.scope() };
    let mut c = Collector::new(None);
    let mut report = Report::default();
    while Instant::now() < deadline {
        report = c.look(std::slice::from_ref(&target), &on(), crate::util::now_ms() as u64, false);
        if !report.leftovers.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(report.leftovers.len(), 1, "{report:?}");
    assert_eq!((report.leftovers[0].title.as_str(), report.leftovers[0].what.as_str()), ("Chrome from a script", "headless Chrome"));
    assert!(report.processes >= 2, "the root and the leaf");
    let report = c.look(std::slice::from_ref(&target), &on(), crate::util::now_ms() as u64, true);
    assert_eq!(report.ledger.cleanups, 1);
    let gone = Instant::now() + Duration::from_secs(5);
    while tree.scope().pids().contains(&leaf) && Instant::now() < gone {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!tree.scope().pids().contains(&leaf), "the leftover survived");
    assert!(root.try_wait().unwrap().is_none(), "the session's own program keeps running");
    drop(target);
    drop(tree);
    let _ = root.wait();
}

/// What the sweeper would clear in this machine's real temp folder, read-only: `cargo test -p bro-core
/// what_the_real_temp_folder_holds -- --ignored --nocapture`.
#[cfg(windows)]
#[test]
#[ignore = "reads the real temp folder (never clears it)"]
fn what_the_real_temp_folder_holds() {
    let in_use = temp_in_use();
    println!("in use: {} profiles, oldest Remotion process running since: {:?}", in_use.profiles.len(), in_use.remotion.map(|t| chrono::DateTime::<chrono::Local>::from(t).to_string()));
    let mut s = temp::Sweeper::new(std::env::temp_dir());
    let started = std::time::Instant::now();
    s.list(std::time::SystemTime::now(), 0, &in_use, &HashSet::new());
    let sum = s.summary();
    println!("would clear {} folders, {} ({:?}), listed in {:?}", sum.folders, bytes(sum.bytes), sum.tools, started.elapsed());
}
