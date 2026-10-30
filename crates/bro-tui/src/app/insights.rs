//! Agent insights in the app (`docs/agent-insights.md` §3): why a pane is waiting, its recap, the files it
//! touched or sent. Each Claude / Codex pane's transcript worker (`panes::insights`) supplies the facts; this
//! turns them into the sidebar's second line, the toasts, the bridge's `attention` / `recap` / `agentFiles`,
//! and schedules Claude's own `/recap` for a pane that has waited unseen for a while (one at a time).

use super::App;
use crate::layout::PaneId;
use crate::pane::Activity;
use crate::panes::insights::{Insights, Snapshot};
use crate::panes::term::Term;
use bro_bridge::{AgentFiles, Attention, AttentionState, RecapSource, SentFile, SessionInsights, TouchKind, TouchedFile};
use bro_core::recap;
use bro_core::sessions::live::{self, AgentFacts};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// What the app remembers per agent pane between event batches.
pub(crate) struct Track {
    notice: Option<bro_bridge::insights::AttentionNotice>,
    wait_key: Option<String>,
    state: AttentionState,
    /// When it entered `state`
    since: Instant,
    since_wall: SystemTime,
    /// Last time you were looking at it
    last_seen: Option<Instant>,
    /// Fingerprint of the insights last given to the bridge
    sent: Option<u64>,
    /// (facts revision, transcript) of the files last given to the bridge
    files_sent: Option<(u64, Option<PathBuf>)>,
    /// [`Services::bridge_announcements`](crate::services::Services::bridge_announcements) when they were: a
    /// session the bridge learned of since (it started after the pane) has neither, so both are given again
    announced: u64,
    /// The wait (its `since`) a recap was already tried for: never twice for the same wait
    recap_tried: Option<Instant>,
}

/// Everything a `/recap` run needs, captured on the UI thread.
#[derive(Clone, Debug)]
pub(crate) struct RecapJob {
    pub program: String,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub cwd: PathBuf,
    pub session_id: String,
    pub transcript: PathBuf,
    pub closed: Arc<AtomicBool>,
}

/// Runs a recap job: Ok((text, cost)) or why not. Swapped out in tests.
pub(crate) type RecapRunner = Arc<dyn Fn(&RecapJob) -> Result<(String, Option<f64>), String> + Send + Sync>;

/// The recap scheduler's shared bits: one run at a time across all panes.
pub(crate) struct Recaps {
    pub busy: Arc<AtomicBool>,
    pub runner: RecapRunner,
    /// Clock offset for tests (added to `Instant::now()`)
    pub skew: Duration,
}

impl Default for Recaps {
    fn default() -> Self {
        Recaps { busy: Arc::default(), runner: Arc::new(run_recap), skew: Duration::ZERO }
    }
}

/// The attention state the sidebar dot already shows, in the bridge's words.
pub(crate) fn attention_state(activity: Option<Activity>, done: bool) -> Option<AttentionState> {
    Some(match activity? {
        Activity::Blocked => AttentionState::Blocked,
        Activity::Working => AttentionState::Working,
        Activity::Idle if done => AttentionState::Done,
        Activity::Idle => AttentionState::Idle,
    })
}

/// Why it's waiting: the pending call / the prompt on screen / the question its turn ended on. Nothing while it
/// works.
pub(crate) fn reason_for(state: AttentionState, facts: Option<&AgentFacts>, prompt_title: Option<&str>) -> Option<String> {
    match state {
        AttentionState::Working => None,
        AttentionState::Blocked => live::waiting_reason(facts, prompt_title, true),
        AttentionState::Done | AttentionState::Idle => live::waiting_reason(facts, None, false),
    }
}

/// The sidebar's second line: `⏸ reason` while it waits on you, else `↳ recap` (nothing while it works).
pub(crate) fn note_line(state: AttentionState, reason: Option<&str>, recap: Option<&str>) -> Option<String> {
    let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    match (state, reason, recap) {
        (AttentionState::Working, _, _) => None,
        (_, Some(r), _) if !r.trim().is_empty() => Some(format!("⏸ {}", clean(r))),
        (_, _, Some(r)) if !r.trim().is_empty() => Some(format!("↳ {}", clean(r))),
        _ => None,
    }
}

/// Whether a pane's wait has gone unseen long enough for a `/recap` (and wasn't tried yet).
pub(crate) fn recap_due(now: Instant, since: Instant, last_seen: Option<Instant>, seen_now: bool, tried: Option<Instant>) -> bool {
    if seen_now || tried == Some(since) {
        return false;
    }
    let from = last_seen.map_or(since, |s| s.max(since));
    now.saturating_duration_since(from) >= recap::UNSEEN_AFTER
}

fn iso(ms: Option<i64>) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms?).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn iso_wall(t: SystemTime) -> Option<String> {
    let ms = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as i64;
    iso(Some(ms))
}

fn touch_kind(k: live::TouchKind) -> TouchKind {
    match k {
        live::TouchKind::Edit => TouchKind::Edit,
        live::TouchKind::Write => TouchKind::Write,
        live::TouchKind::Create => TouchKind::Create,
        live::TouchKind::Delete => TouchKind::Delete,
        live::TouchKind::Move => TouchKind::Move,
        live::TouchKind::Read => TouchKind::Read,
    }
}

/// The facts' file lists, as the bridge serves them.
pub(crate) fn agent_files(snap: &Snapshot) -> AgentFiles {
    let Some(f) = snap.facts.as_deref() else { return AgentFiles { transcript: snap.transcript.clone(), ..AgentFiles::default() } };
    AgentFiles {
        transcript: snap.transcript.clone(),
        sent: f.sent.iter().map(|s| SentFile { path: s.path.clone(), caption: s.caption.clone(), at: iso(s.at_ms), mime_type: s.mime_type.clone(), is_image: s.is_image }).collect(),
        touched: f.touched.iter().map(|t| TouchedFile { path: t.path.clone(), kind: touch_kind(t.kind), at: iso(t.at_ms), line: t.line }).collect(),
    }
}

/// What to show and send for one pane right now.
pub(crate) struct View {
    pub state: AttentionState,
    pub reason: Option<String>,
    pub recap: Option<recap::Recap>,
    pub limit: Option<bro_core::sessions::limits::UsageLimit>,
}

impl View {
    pub(crate) fn of(state: AttentionState, term: &Term, snap: Option<&Snapshot>) -> View {
        let facts = snap.and_then(|s| s.facts.as_deref());
        let limit = term.limit_warning();
        let state = if limit.is_some() { AttentionState::Blocked } else { state };
        let reason = limit.as_ref().map(|l| l.message.clone()).or_else(|| reason_for(state, facts, term.prompt_title()));
        let recap = facts.and_then(|f| recap::choose(f, snap.and_then(|s| s.generated.as_ref())));
        View { state, reason, recap, limit }
    }

    pub(crate) fn note(&self) -> Option<String> {
        note_line(self.state, self.reason.as_deref(), self.recap.as_ref().map(|r| r.text.as_str()))
    }

    fn fingerprint(&self, since: SystemTime) -> u64 {
        let mut h = DefaultHasher::new();
        format!("{:?}", self.state).hash(&mut h);
        self.reason.hash(&mut h);
        self.limit.hash(&mut h);
        since.hash(&mut h);
        if let Some(r) = &self.recap {
            r.text.hash(&mut h);
            r.at_ms.hash(&mut h);
            r.source.as_str().hash(&mut h);
        }
        h.finish()
    }

    fn insights(&self, since: SystemTime) -> SessionInsights {
        let source = |s: recap::Source| match s {
            recap::Source::Claude => RecapSource::Claude,
            recap::Source::Generated => RecapSource::Generated,
            recap::Source::LastMessage => RecapSource::LastMessage,
        };
        SessionInsights {
            attention: Some(Attention { state: self.state, reason: self.reason.clone(), since: iso_wall(since), limit: self.limit.clone(), notice: None }),
            recap: self.recap.as_ref().map(|r| bro_bridge::Recap { text: r.text.clone(), at: iso(r.at_ms), source: source(r.source) }),
        }
    }
}

/// The `/recap` for this pane's session, if it can have one (a Claude pane bound to a transcript).
fn recap_job(term: &Term, snap: &Snapshot) -> Option<RecapJob> {
    let spawn = term.launch_spawn()?;
    Some(RecapJob {
        program: spawn.program.clone(),
        env: spawn.env.clone(),
        env_remove: spawn.env_remove.clone(),
        cwd: term.meta.cwd.clone(),
        session_id: snap.session_id.clone()?,
        transcript: snap.transcript.clone()?,
        closed: term.closed_signal(),
    })
}

impl App {
    pub(crate) fn session_notice(&self, id: PaneId) -> Option<&bro_bridge::insights::AttentionNotice> {
        if !self.svc.settings().insights.attention_indicators { return None; }
        self.tracks.get(&id)?.notice.as_ref()
    }

    pub(crate) fn acknowledge_session(&mut self, id: PaneId, event: &str) {
        if let Some(track) = self.tracks.get_mut(&id)
            && let Some(notice) = track.notice.as_mut().filter(|n| n.event_id == event) {
            notice.unread = false;
            track.sent = None;
            self.done.remove(&id);
        }
    }

    /// The reason and recap text for pane `id` right now (toasts).
    pub(crate) fn pane_view(&self, id: PaneId) -> Option<View> {
        let p = self.panes.get(&id)?;
        let t = p.as_term_ref()?;
        let state = attention_state(p.activity(), self.done.contains(&id))?;
        let snap = t.insights();
        Some(View::of(state, t, snap.as_ref()))
    }

    /// After each batch of events: refresh the sidebar notes, tell the bridge what changed, and start a recap
    /// where one is due.
    pub(crate) fn update_insights(&mut self) {
        let now = Instant::now() + self.recaps.skew;
        let focused = self.focused().filter(|_| self.term_focused && !self.side_focus && !self.overlay.is_open());
        let generate = self.svc.settings().insights.generate_recaps;
        let announced = self.svc.bridge_announcements();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        let mut start: Option<(RecapJob, Insights)> = None;
        for id in ids {
            let Some(p) = self.panes.get(&id) else { continue };
            let Some(t) = p.as_term_ref() else { continue };
            let Some(state) = attention_state(p.activity(), self.done.contains(&id)).filter(|_| t.meta.harness.is_some()) else {
                self.notes.remove(&id);
                continue;
            };
            let snap = t.insights();
            let view = View::of(state, t, snap.as_ref());
            match view.note() {
                Some(n) => self.notes.insert(id, n),
                None => self.notes.remove(&id),
            };
            let seen = focused == Some(id);
            let track = self.tracks.entry(id).or_insert_with(|| Track { notice: None, wait_key: None, state, since: now, since_wall: SystemTime::now(), last_seen: None, sent: None, files_sent: None, announced, recap_tried: None });
            let wait_key = if state == AttentionState::Blocked { Some(view.limit.as_ref().map(|l| l.title.clone()).unwrap_or_else(|| {
                let at = snap.as_ref().and_then(|s| s.facts.as_deref()).and_then(|f| f.pending.as_ref().and_then(|p| p.at_ms).or_else(|| f.last_assistant.as_ref().and_then(|a| a.at_ms)));
                format!("{}:{at:?}", view.reason.as_deref().unwrap_or_default())
            })) } else { None };
            let new_kind = if wait_key.is_some() && (track.wait_key != wait_key || track.notice.is_none()) {
                Some(bro_bridge::insights::NoticeKind::Input)
            } else if (track.state == AttentionState::Working && matches!(state, AttentionState::Idle | AttentionState::Done))
                || (state == AttentionState::Done && track.notice.is_none()) {
                Some(bro_bridge::insights::NoticeKind::Finished)
            } else { None };
            track.wait_key = wait_key;
            if state == AttentionState::Working { track.notice = None; }
            if let Some(kind) = new_kind {
                track.notice = Some(bro_bridge::insights::AttentionNotice { event_id: uuid::Uuid::new_v4().to_string(), kind, unread: true, enabled: true });
            }
            if let Some(notice) = &mut track.notice {
                if seen { notice.unread = false; }
                notice.enabled = self.svc.settings().insights.attention_indicators;
            }
            if track.announced != announced {
                track.announced = announced;
                (track.sent, track.files_sent) = (None, None);
            }
            if track.state != state {
                track.state = state;
                track.since = now;
                track.since_wall = SystemTime::now();
            }
            if seen {
                track.last_seen = Some(now);
            }
            // the bridge: attention + recap, then the file lists when the facts moved
            let mut hash = DefaultHasher::new();
            view.fingerprint(track.since_wall).hash(&mut hash);
            track.notice.hash(&mut hash);
            let print = hash.finish();
            if track.sent != Some(print) {
                track.sent = Some(print);
                let mut insights = view.insights(track.since_wall);
                if let Some(attention) = &mut insights.attention { attention.notice = track.notice.clone(); }
                self.svc.bridge_set_insights(&t.meta.sid, insights);
            }
            if let Some(s) = &snap {
                let key = (s.facts.as_ref().map_or(0, |f| f.revision), s.transcript.clone());
                if track.files_sent.as_ref() != Some(&key) {
                    track.files_sent = Some(key);
                    self.svc.bridge_set_agent_files(&t.meta.sid, agent_files(s));
                }
            }
            // Claude's own /recap for a wait nobody has looked at
            if start.is_none()
                && generate
                && t.meta.harness == Some(bro_core::Harness::Claude)
                && matches!(state, AttentionState::Done | AttentionState::Blocked)
                && !self.recaps.busy.load(Ordering::SeqCst)
                && recap_due(now, track.since, track.last_seen, seen, track.recap_tried)
                && let Some(s) = &snap
                && let Some(f) = s.facts.as_deref()
                && recap::worth_generating(f, s.generated.as_ref())
                && let (Some(job), Some(handle)) = (recap_job(t, s), t.insights_handle())
            {
                track.recap_tried = Some(track.since);
                start = Some((job, handle.clone()));
            }
        }
        self.tracks.retain(|id, _| self.panes.contains_key(id));
        self.notes.retain(|id, _| self.panes.contains_key(id));
        if let Some((job, handle)) = start {
            self.start_recap(job, handle);
        }
    }

    /// Run one `/recap` off the UI thread (inline in tests); the result lands in the pane's insights.
    fn start_recap(&mut self, job: RecapJob, handle: Insights) {
        let busy = self.recaps.busy.clone();
        let runner = self.recaps.runner.clone();
        busy.store(true, Ordering::SeqCst);
        let work = move || {
            let out = crate::services::guard("recap::run", || runner(&job));
            if let Ok(Ok((text, cost))) = out
                && let Ok(c) = recap::store(&job.session_id, &text, cost, &job.transcript)
            {
                handle.set_generated(c);
            }
            // a failed recap isn't retried for the same wait (see Track::recap_tried)
            busy.store(false, Ordering::SeqCst);
        };
        if cfg!(test) {
            work();
        } else {
            let _ = std::thread::Builder::new().name("bro-recap".into()).spawn(work);
        }
    }
}

/// `claude -p --resume <sid> --fork-session --no-session-persistence --output-format json /recap` in the
/// session's folder with the pane's env (minus bro's own), no console window, killed after `recap::TIMEOUT`.
fn run_recap(job: &RecapJob) -> Result<(String, Option<f64>), String> {
    if job.closed.load(Ordering::SeqCst) {
        return Err("the session closed".into());
    }
    use std::io::Read;
    use std::process::{Command, Stdio};
    let (program, pre) = crate::panes::term::resolve(&job.program);
    let (_, args) = recap::command(&program, &job.session_id);
    let mut cmd = Command::new(&program);
    cmd.args(&pre).args(&args).current_dir(&job.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if recap::strip_env(&k) || crate::panes::term::CLAUDE_SESSION_ENV.contains(&k.as_str()) {
            cmd.env_remove(&k);
        }
    }
    for k in &job.env_remove {
        cmd.env_remove(k);
    }
    for (k, v) in job.env.iter().filter(|(k, _)| !recap::strip_env(k)) {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let (mut child, mut tree) = bro_core::proc_tree::ProcessTree::spawn(&mut cmd).map_err(|e| format!("couldn't start {program}: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + recap::TIMEOUT;
    loop {
        if job.closed.load(Ordering::SeqCst) {
            tree.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err("the session closed".into());
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            _ => {
                tree.terminate();
                let _ = child.kill();
                let _ = child.wait();
                return Err("recap timed out".into());
            }
        }
    }
    tree.terminate();
    let out = reader.join().unwrap_or_default();
    recap::parse_output(&out).map_err(|f| f.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_events_survive_tiling_clear_on_focus_and_rearm_for_new_work() {
        crate::testkit::isolate();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = crate::services::Services::offline(crate::services::fallback_settings(), tx.clone());
        let mut app = App::new(svc, tx, crate::app::Opts { demo: true, fixed_demo: true, load_recents: false });
        let id = app.focused().unwrap();
        app.term_focused = false;
        let set = |app: &mut App, state| app.panes.get_mut(&id).unwrap().as_term().unwrap().set_screen_status(Some(state));
        set(&mut app, Activity::Working);
        app.update_insights();
        assert!(app.session_notice(id).is_none());
        set(&mut app, Activity::Idle);
        app.update_insights();
        let first = app.session_notice(id).unwrap().clone();
        assert!(first.unread);
        assert_eq!(first.kind, bro_bridge::insights::NoticeKind::Finished);
        app.update_insights();
        assert_eq!(app.session_notice(id).unwrap().event_id, first.event_id);
        app.term_focused = true;
        app.side_focus = false;
        app.update_insights();
        assert!(!app.session_notice(id).unwrap().unread);
        app.term_focused = false;
        set(&mut app, Activity::Working);
        app.update_insights();
        set(&mut app, Activity::Blocked);
        app.update_insights();
        let question = app.session_notice(id).unwrap().clone();
        assert!(question.unread);
        assert_ne!(question.event_id, first.event_id);
        app.acknowledge_session(id, &first.event_id);
        assert!(app.session_notice(id).unwrap().unread, "a delayed receipt cannot check a new event");
        let sid = app.panes[&id].as_term_ref().unwrap().meta.sid.clone();
        let focused = app.focused();
        app.bridge_command(bro_bridge::BridgeCommand::Seen { id: sid, event_id: question.event_id.clone() });
        assert_eq!(app.focused(), focused, "checking on mobile must not move the desktop keyboard");
        app.update_insights();
        assert!(!app.session_notice(id).unwrap().unread);
        assert_eq!(app.panes[&id].activity(), Some(Activity::Blocked), "checking is not answering");
        let mut settings = app.svc.settings();
        settings.insights.attention_indicators = false;
        app.svc.set_settings_unsaved(settings.clone());
        assert!(app.session_notice(id).is_none());
        settings.insights.attention_indicators = true;
        app.svc.set_settings_unsaved(settings);
        assert!(!app.session_notice(id).unwrap().unread, "turning the visuals back on retains the receipt");
    }

    #[cfg(windows)]
    #[test]
    fn closing_session_cancels_active_recap_and_its_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("recap.pid");
        let script = dir.path().join("recap.cmd");
        std::fs::write(&script, format!("@\"{}\" --exact panes::term::process_tests::session_process_worker --ignored --nocapture\r\n", std::env::current_exe().unwrap().display())).unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        let job = RecapJob {
            program: script.to_string_lossy().into_owned(),
            env: vec![("SESSION_PROCESS_TEST_MODE".into(), "stay".into()),
                ("SESSION_PROCESS_TEST_MARKER".into(), marker.to_string_lossy().into_owned())],
            env_remove: vec![], cwd: dir.path().into(), session_id: "local-test".into(),
            transcript: dir.path().join("transcript"), closed: closed.clone(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || { tx.send(run_recap(&job)).ok(); });
        let until = Instant::now() + Duration::from_secs(15);
        let pid: u32 = loop {
            if let Some(pid) = std::fs::read_to_string(&marker).ok().and_then(|s| s.trim().parse().ok()) { break pid; }
            assert!(Instant::now() < until, "the local recap worker did not start: {:?}", rx.try_recv());
            std::thread::sleep(Duration::from_millis(20));
        };
        closed.store(true, Ordering::SeqCst);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err(), "the session closed");
        worker.join().unwrap();
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
        unsafe {
            let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if !process.is_null() {
                let result = WaitForSingleObject(process, 5000);
                CloseHandle(process);
                assert_eq!(result, WAIT_OBJECT_0, "the recap's descendant survived session close");
            }
        }
    }

    #[test]
    fn states_map_from_the_sidebar_dot() {
        assert_eq!(attention_state(Some(Activity::Blocked), false), Some(AttentionState::Blocked));
        assert_eq!(attention_state(Some(Activity::Working), true), Some(AttentionState::Working));
        assert_eq!(attention_state(Some(Activity::Idle), true), Some(AttentionState::Done));
        assert_eq!(attention_state(Some(Activity::Idle), false), Some(AttentionState::Idle));
        assert_eq!(attention_state(None, false), None);
    }

    #[test]
    fn the_second_line_says_why_it_waits_else_the_recap() {
        assert_eq!(note_line(AttentionState::Blocked, Some("Bash: npm publish"), Some("did x")).as_deref(), Some("⏸ Bash: npm publish"));
        assert_eq!(note_line(AttentionState::Blocked, None, Some("did x")).as_deref(), Some("↳ did x"));
        assert_eq!(note_line(AttentionState::Done, None, Some("Shipped  the\nfix")).as_deref(), Some("↳ Shipped the fix"));
        assert_eq!(note_line(AttentionState::Idle, Some("Which DB?"), None).as_deref(), Some("⏸ Which DB?"));
        assert_eq!(note_line(AttentionState::Working, Some("x"), Some("y")), None, "nothing while it works");
        assert_eq!(note_line(AttentionState::Done, None, Some("  ")), None);
    }

    #[test]
    fn recaps_wait_for_an_unseen_wait_and_only_once() {
        let t0 = Instant::now();
        let after = |s: u64| t0 + Duration::from_secs(s);
        assert!(!recap_due(after(179), t0, None, false, None), "not before 180 s");
        assert!(recap_due(after(180), t0, None, false, None));
        assert!(!recap_due(after(500), t0, None, true, None), "not while you look at it");
        assert!(!recap_due(after(300), t0, Some(after(200)), false, None), "the clock restarts when you looked");
        assert!(recap_due(after(380), t0, Some(after(200)), false, None));
        assert!(!recap_due(after(900), t0, None, false, Some(t0)), "once per wait");
        assert!(recap_due(after(900), after(1), None, false, Some(t0)), "a new wait may have one");
    }

    #[test]
    fn the_bridge_gets_iso_times_and_the_file_lists() {
        let mut f = AgentFacts::new(bro_core::Harness::Claude);
        f.ingest(&[
            serde_json::json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Write","input":{"file_path":"/p/new.rs","content":"x"}}]}}).to_string(),
            serde_json::json!({"type":"user","timestamp":"2026-10-02T10:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"toolUseResult":{"type":"create","filePath":"/p/new.rs"}}).to_string(),
        ]);
        let snap = Snapshot { facts: Some(Arc::new(f)), transcript: Some(PathBuf::from("/t.jsonl")), ..Snapshot::default() };
        let files = agent_files(&snap);
        assert_eq!(files.transcript.as_deref(), Some(std::path::Path::new("/t.jsonl")));
        assert_eq!(files.touched.len(), 1);
        assert_eq!(files.touched[0].kind, TouchKind::Create);
        assert!(files.touched[0].at.as_deref().is_some_and(|a| a.starts_with("2026-10-02T10:00") && a.ends_with('Z')));
        let view = View { state: AttentionState::Done, reason: Some("Ship it?".into()), recap: None, limit: None };
        let ins = view.insights(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let att = ins.attention.unwrap();
        assert_eq!((att.state, att.reason.as_deref(), att.since.as_deref()), (AttentionState::Done, Some("Ship it?"), Some("2023-11-14T22:13:20.000Z")));
        assert_ne!(view.fingerprint(SystemTime::UNIX_EPOCH), View { state: AttentionState::Done, reason: None, recap: None, limit: None }.fingerprint(SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn reasons_follow_the_state() {
        let mut f = AgentFacts::new(bro_core::Harness::Claude);
        f.ingest(&[serde_json::json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"AskUserQuestion","input":{"questions":[{"header":"Database","question":"Which database?"}]}}]}}).to_string()]);
        assert_eq!(reason_for(AttentionState::Blocked, Some(&f), Some("Do you want to proceed?")).as_deref(), Some("Question: Database"));
        assert_eq!(reason_for(AttentionState::Blocked, None, Some("Do you want to proceed?")).as_deref(), Some("Do you want to proceed?"));
        assert_eq!(reason_for(AttentionState::Working, Some(&f), None), None);
    }
}
