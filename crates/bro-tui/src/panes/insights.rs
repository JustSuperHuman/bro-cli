//! What an agent pane's transcript says (`docs/agent-insights.md`): a worker per Claude / Codex pane binds the
//! pane to the transcript it is writing (`bro_core::sessions::live::Binder`), tails it once a second (only new
//! bytes) into `AgentFacts`, and publishes a snapshot the UI reads without touching the disk. It wakes the pane
//! when something changed. A generated `/recap` lands here too ([`Insights::set_generated`]), and a Claude
//! session's agent team ([`bro_core::agent_teams`]) is read alongside, once a second. The same lines build the
//! conversation the desktop app's focus mode shows ([`Conversation`]), each parsed once for both; a long
//! transcript's conversation is read from its start when bound ([`Conversation::history`]), not only its last lines.
//! The subagents the session starts are followed through their own transcripts on the same beat
//! ([`bro_core::sessions::subagents::Follower`]).

use crate::pane::Waker;
use bro_core::Harness;
use bro_core::agent_teams::{self, Team};
use bro_core::recap::CachedRecap;
use bro_core::sessions::conversation::Conversation;
use bro_core::sessions::live::{AgentFacts, BindSpec, Binder, Tail};
use bro_core::sessions::subagents::{Follower, Subagent};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often the worker looks for new transcript lines.
const EVERY: Duration = Duration::from_secs(1);

/// What the UI reads: cheap to clone (the facts are shared).
#[derive(Clone, Default)]
pub struct Snapshot {
    pub facts: Option<Arc<AgentFacts>>,
    /// What was said and done, for focus mode (its items are shared, so this is cheap to hand out)
    pub conversation: Option<Arc<Conversation>>,
    /// The transcript the pane is bound to
    pub transcript: Option<PathBuf>,
    /// The agent's own session id (Claude: from `sessions/<pid>.json`, else the transcript)
    pub session_id: Option<String>,
    /// A `/recap` bro generated that still matches the transcript
    pub generated: Option<CachedRecap>,
    /// The agent's own word on whether a turn is running: Claude's `sessions/<pid>.json` status, Codex's
    /// `task_started` / `task_complete`. None = unknown (fall back to the screen).
    pub busy: Option<bool>,
    /// The agent team this Claude session leads, as Claude Code last wrote it (None: agent teams off, or no
    /// session id yet). A team of the lead alone is still Some: see [`Team::is_empty`].
    pub team: Option<Arc<Team>>,
    /// The subagents the session started (nested ones too), the oldest first, each with its own conversation as
    /// far as it's read (None until the transcript is bound)
    pub subagents: Option<Arc<Vec<Arc<Subagent>>>>,
    /// Bumped on every change
    pub rev: u64,
}

struct Inner {
    snap: Mutex<Snapshot>,
    stop: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

/// A handle to one pane's transcript worker. Clones share it; [`Insights::stop`] ends the worker.
#[derive(Clone)]
pub struct Insights {
    inner: Arc<Inner>,
}

impl Insights {
    fn empty(waker: Option<Waker>) -> Insights {
        Insights { inner: Arc::new(Inner { snap: Mutex::new(Snapshot::default()), stop: AtomicBool::new(false), waker: Mutex::new(waker) }) }
    }

    /// Start following the transcript of the pane described by `spec` on a worker thread.
    pub fn start(spec: BindSpec, waker: Waker) -> Insights {
        let me = Insights::empty(Some(waker));
        let worker = me.clone();
        let _ = std::thread::Builder::new().name("bro-insights".into()).spawn(move || worker.run(spec));
        me
    }

    /// A worker-less handle that only carries `team` (`bro --demo`'s lead).
    pub fn fixed_team(team: Team) -> Insights {
        let me = Insights::empty(None);
        me.set_team(Some(team));
        me
    }

    /// Give a fixed pane a team (tests, the demo).
    pub fn set_team(&self, team: Option<Team>) {
        let mut s = self.inner.snap.lock();
        s.team = team.map(Arc::new);
        s.rev += 1;
    }

    /// Give a fixed pane its subagents (tests, the demo).
    pub fn set_subagents(&self, subagents: Vec<Subagent>) {
        let mut s = self.inner.snap.lock();
        s.subagents = Some(Arc::new(subagents.into_iter().map(Arc::new).collect()));
        s.rev += 1;
    }

    /// Tests: facts from a whole transcript, read now on this thread (no worker, no binding).
    #[cfg(test)]
    pub fn from_transcript(harness: Harness, path: &Path, session_id: Option<&str>) -> Insights {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let me = Insights::fixed_transcript(harness, &text);
        {
            let mut s = me.inner.snap.lock();
            if let Some(id) = session_id {
                s.session_id = Some(id.to_string());
            }
            s.transcript = Some(path.to_path_buf());
        }
        me
    }

    /// A worker-less handle with the facts and conversation of a whole transcript (`bro --demo`'s sessions).
    pub fn fixed_transcript(harness: Harness, jsonl: &str) -> Insights {
        let me = Insights::empty(None);
        let mut facts = AgentFacts::new(harness);
        let mut conversation = Conversation::new(harness);
        for line in jsonl.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                facts.ingest_value(&v);
                conversation.ingest(&v);
            }
        }
        {
            let mut s = me.inner.snap.lock();
            s.session_id = facts.session_id.clone();
            s.busy = facts.turn_open;
            s.facts = Some(Arc::new(facts));
            s.conversation = Some(Arc::new(conversation));
            s.rev = 1;
        }
        me
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.snap.lock().clone()
    }

    /// [`Snapshot::session_id`] without cloning the rest.
    pub fn session_id(&self) -> Option<String> {
        self.inner.snap.lock().session_id.clone()
    }

    /// [`Snapshot::team`] without cloning the rest.
    pub fn team(&self) -> Option<Arc<Team>> {
        self.inner.snap.lock().team.clone()
    }

    /// [`Snapshot::busy`] without cloning the rest.
    pub fn busy(&self) -> Option<bool> {
        self.inner.snap.lock().busy
    }

    /// [`Snapshot::conversation`] and [`Snapshot::subagents`] without cloning the rest (what's at work in the
    /// background, read every frame).
    pub fn work(&self) -> (Option<Arc<Conversation>>, Option<Arc<Vec<Arc<Subagent>>>>) {
        let s = self.inner.snap.lock();
        (s.conversation.clone(), s.subagents.clone())
    }

    #[cfg(test)]
    pub fn rev(&self) -> u64 {
        self.inner.snap.lock().rev
    }

    /// End the worker (the pane closed).
    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
    }

    /// A `/recap` finished for this pane's session: show it until the transcript changes.
    pub fn set_generated(&self, recap: CachedRecap) {
        {
            let mut s = self.inner.snap.lock();
            s.generated = Some(recap);
            s.rev += 1;
        }
        self.wake();
    }

    fn wake(&self) {
        if let Some(w) = self.inner.waker.lock().as_ref() {
            w.wake();
        }
    }

    fn run(&self, spec: BindSpec) {
        let harness = spec.harness;
        let mut binder = Binder::new(spec);
        let mut tail: Option<Tail> = None;
        let mut facts: Option<AgentFacts> = None;
        let mut conversation: Option<Conversation> = None;
        let mut busy = None;
        let mut team: Option<Arc<Team>> = None;
        let mut follower: Option<Follower> = None;
        while !self.inner.stop.load(Ordering::SeqCst) {
            let step = crate::services::guard("insights::step", || step(harness, &mut binder, &mut tail, &mut facts, &mut conversation));
            match step {
                Ok(Some(update)) => self.publish(update),
                Ok(None) => {}
                // a transcript that breaks the parser: start over on the next binding
                Err(_) => {
                    tail = None;
                    facts = None;
                    conversation = None;
                }
            }
            // the subagents of the transcript it's bound to (a new binding starts afresh)
            let bound = tail.as_ref().map(|t| t.path().to_path_buf());
            if follower.as_ref().map(|f| f.parent().to_path_buf()) != bound {
                let since = Some(binder.spec().started);
                follower = bound.map(|path| Follower::new(harness, path, since));
                self.set_followed(follower.as_ref().map(Follower::subagents));
            }
            if let Some(f) = follower.as_mut() {
                let spawned = conversation.as_ref().map(|c| c.subagents.clone()).unwrap_or_default();
                let polled = crate::services::guard("insights::subagents", || f.poll(&spawned, Instant::now()));
                match polled {
                    Ok(true) => self.set_followed(Some(f.subagents())),
                    Ok(false) => {}
                    Err(_) => follower = None,
                }
            }
            let now = match harness {
                Harness::Claude => binder.claude_busy(),
                Harness::Codex => facts.as_ref().and_then(|f| f.turn_open),
                Harness::Pi | Harness::Omp | Harness::Antigravity => None,
            };
            if now != busy {
                busy = now;
                {
                    let mut s = self.inner.snap.lock();
                    s.busy = now;
                    s.rev += 1;
                }
                self.wake();
            }
            if harness == Harness::Claude {
                let sid = binder.session_id().map(str::to_string).or_else(|| self.inner.snap.lock().session_id.clone());
                let now = crate::services::guard("insights::team", || sid.and_then(|sid| agent_teams::read(&binder.config_dir(), &sid))).ok().flatten();
                if now.as_ref() != team.as_deref() {
                    team = now.map(Arc::new);
                    {
                        let mut s = self.inner.snap.lock();
                        s.team = team.clone();
                        s.rev += 1;
                    }
                    self.wake();
                }
            }
            // sleep in short slices so a closed pane's worker ends promptly
            let until = Instant::now() + EVERY;
            while Instant::now() < until && !self.inner.stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// The subagents followed now (None: no transcript to follow them from).
    fn set_followed(&self, subagents: Option<Arc<Vec<Arc<Subagent>>>>) {
        {
            let mut s = self.inner.snap.lock();
            if s.subagents.is_none() && subagents.as_ref().is_none_or(|l| l.is_empty()) {
                s.subagents = subagents;
                return;
            }
            s.subagents = subagents;
            s.rev += 1;
        }
        self.wake();
    }

    fn publish(&self, u: Update) {
        {
            let mut s = self.inner.snap.lock();
            s.facts = u.facts.map(Arc::new);
            s.conversation = u.conversation.map(Arc::new);
            s.transcript = u.transcript;
            s.session_id = u.session_id;
            s.generated = u.generated;
            s.rev += 1;
        }
        self.wake();
    }
}

/// A new snapshot from one worker step (nothing read: the binding was dropped).
struct Update {
    facts: Option<AgentFacts>,
    conversation: Option<Conversation>,
    transcript: Option<PathBuf>,
    session_id: Option<String>,
    generated: Option<CachedRecap>,
}

/// One poll: re-bind when due, read new lines, and say what changed (None = nothing).
fn step(harness: Harness, binder: &mut Binder, tail: &mut Option<Tail>, facts: &mut Option<AgentFacts>, conversation: &mut Option<Conversation>) -> Option<Update> {
    step_at(harness, binder, tail, facts, conversation, Instant::now())
}

fn step_at(harness: Harness, binder: &mut Binder, tail: &mut Option<Tail>, facts: &mut Option<AgentFacts>, conversation: &mut Option<Conversation>, now: Instant) -> Option<Update> {
    let mut changed = false;
    if binder.poll(now) {
        let Some(path) = binder.path() else {
            // A reset or a corrected binding: show none until this session writes its own transcript.
            (*tail, *facts, *conversation) = (None, None, None);
            let session_id = binder.session_id().map(str::to_string);
            return Some(Update { facts: None, conversation: None, transcript: None, session_id, generated: None });
        };
        *tail = Some(Tail::new(path));
        *facts = Some(AgentFacts::new(harness));
        *conversation = Some(Conversation::new(harness));
        changed = true;
    }
    let (Some(t), Some(f)) = (tail.as_mut(), facts.as_mut()) else { return None };
    let c = conversation.get_or_insert_with(|| Conversation::new(harness));
    if let Ok(read) = t.read() {
        if read.reset {
            *f = AgentFacts::new(harness);
            *c = Conversation::new(harness);
            changed = true;
        }
        // a long transcript's first read holds only its latest lines: the conversation is read from its start
        let history = read.began.is_some_and(|b| b > 0);
        // each line parsed once, for the facts and the conversation both
        let before = f.revision;
        for line in &read.lines {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                f.ingest_value(&v);
                if !history {
                    changed |= c.ingest(&v);
                }
            }
        }
        if history {
            // and the pictures and videos from before those lines, for the media panel and the phone's gallery
            if let Some(began) = read.began {
                let _ = f.media_history(t.path(), began);
            }
            match Conversation::history(harness, t.path(), t.lines_end()) {
                Ok(whole) => *c = whole,
                Err(_) => {
                    c.ingest_lines(&read.lines);
                }
            }
            changed = true;
        }
        changed |= f.revision != before;
    }
    if !changed {
        return None;
    }
    let transcript = t.path().to_path_buf();
    let session_id = binder.session_id().map(str::to_string).or_else(|| f.session_id.clone());
    let generated = generated_for(harness, session_id.as_deref(), &transcript);
    Some(Update { facts: Some(f.clone()), conversation: Some(c.clone()), transcript: Some(transcript), session_id, generated })
}

/// The cached `/recap` while it still matches the transcript (Claude only).
fn generated_for(harness: Harness, session_id: Option<&str>, transcript: &Path) -> Option<CachedRecap> {
    (harness == Harness::Claude).then_some(())?;
    bro_core::recap::load_fresh(session_id?, transcript)
}

/// The ID the pane will use (if known) and the source ID it must avoid following when forking.
pub fn launch_session_ids(harness: Harness, args: &[String]) -> (Option<String>, Option<String>) {
    let args = &args[..args.iter().position(|a| a == "--").unwrap_or(args.len())];
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with('-')).cloned();
    match harness {
        Harness::Claude => {
            let resume = after("--resume").or_else(|| after("-r"));
            if args.iter().any(|a| a == "--fork-session") {
                (after("--session-id"), resume)
            } else {
                (resume.or_else(|| after("--session-id")), None)
            }
        }
        Harness::Codex if args.first().is_some_and(|a| a == "fork") => (None, after("fork")),
        Harness::Codex => (after("resume"), None),
        Harness::Pi | Harness::Omp | Harness::Antigravity => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_claude_publishes_an_empty_conversation_before_the_next_prompt() {
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("project");
        let projects = dir.path().join("projects").join(bro_core::sessions::live::claude_project_slug(&cwd));
        let sessions = dir.path().join("sessions");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        let old = "11111111-1111-4111-8111-111111111111";
        let fresh = "22222222-2222-4222-8222-222222222222";
        let status = sessions.join("4242.json");
        let write_status = |id| std::fs::write(&status, json!({"pid":4242,"sessionId":id,"cwd":cwd}).to_string()).unwrap();
        write_status(old);
        std::fs::write(projects.join(format!("{old}.jsonl")), format!("{}\n", json!({"type":"user","sessionId":old,"message":{"content":"Old conversation"}}))).unwrap();
        let mut binder = Binder::new(BindSpec {
            harness: Harness::Claude, profile_id: None, config_dir: Some(dir.path().into()), pid: Some(4242),
            cwd: cwd.clone(), started: std::time::SystemTime::now(), resume_id: None, fork_source: None,
        });
        let (mut tail, mut facts, mut conversation) = (None, None, None);
        let insights = Insights::empty(None);
        let now = Instant::now();
        insights.publish(step_at(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation, now).unwrap());
        assert_eq!(insights.snapshot().conversation.unwrap().items.len(), 1);
        write_status(fresh);
        insights.publish(step_at(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation, now + Duration::from_secs(6)).expect("a new session clears the snapshot without a transcript"));
        let cleared = insights.snapshot();
        assert_eq!(cleared.session_id.as_deref(), Some(fresh));
        assert!(cleared.conversation.is_none() && cleared.facts.is_none() && cleared.transcript.is_none());
        assert!(tail.is_none() && facts.is_none() && conversation.is_none());
        assert!(step_at(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation, now + Duration::from_secs(12)).is_none());
        std::fs::write(projects.join(format!("{fresh}.jsonl")), format!("{}\n", json!({"type":"user","sessionId":fresh,"message":{"content":"Fresh prompt"}}))).unwrap();
        let update = step_at(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation, now + Duration::from_secs(18)).unwrap();
        let items = update.conversation.unwrap().items;
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0].as_ref(), bro_core::sessions::conversation::Item::You {text, ..} if text == "Fresh prompt"));
    }

    #[test]
    fn fork_sources_are_distinct_from_the_new_sessions_ids() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(launch_session_ids(Harness::Claude, &s(&["--resume", "abc", "--fork-session"])), (None, Some("abc".into())));
        assert_eq!(launch_session_ids(Harness::Claude, &s(&["--resume", "abc", "--fork-session", "--session-id", "def"])), (Some("def".into()), Some("abc".into())));
        assert_eq!(launch_session_ids(Harness::Claude, &s(&["--resume", "abc", "--", "--fork-session"])), (Some("abc".into()), None));
        assert_eq!(launch_session_ids(Harness::Codex, &s(&["resume", "019a"])), (Some("019a".into()), None));
        assert_eq!(launch_session_ids(Harness::Codex, &s(&["fork", "019b"])), (None, Some("019b".into())));
        assert_eq!(launch_session_ids(Harness::Codex, &s(&["resume", "--last"])), (None, None));
    }

    #[test]
    fn a_transcript_becomes_a_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let line = serde_json::json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","sessionId":"0b6e3c1e-0000-4000-8000-000000000001","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm publish","description":"Publish the package"}}]}});
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let i = Insights::from_transcript(Harness::Claude, &path, None);
        let s = i.snapshot();
        assert_eq!(s.facts.as_ref().and_then(|f| f.pending.as_ref()).map(|p| p.reason.as_str()), Some("Bash: Publish the package"));
        let before = i.rev();
        i.set_generated(CachedRecap { text: "did things".into(), at: 1, size: 0, mtime: 0, cost_usd: None });
        assert!(i.rev() > before);
        assert_eq!(i.snapshot().generated.map(|g| g.text).as_deref(), Some("did things"));
    }

    #[test]
    fn the_worker_follows_a_growing_transcript_step_by_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "").unwrap();
        let mut tail = Some(Tail::new(&path));
        let mut facts = Some(AgentFacts::new(Harness::Claude));
        let mut conversation = None;
        let mut binder = Binder::new(BindSpec { harness: Harness::Pi, profile_id: None, config_dir: None, pid: None, cwd: dir.path().into(), started: std::time::SystemTime::now(), resume_id: None, fork_source: None });
        assert!(step(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation).is_none(), "nothing yet");
        let line = serde_json::json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","message":{"id":"m1","content":[{"type":"text","text":"All done. Want me to ship it?"}]}});
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let u = step(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation).expect("new lines change the facts");
        assert!(u.facts.unwrap().asked.is_some());
        assert_eq!(u.conversation.unwrap().items.len(), 1, "the same line is the conversation's");
        assert!(step(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation).is_none(), "no new bytes, no update");
    }

    #[test]
    fn a_long_transcript_shows_its_whole_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        // more tool output between the prompt and the answer than the first read takes
        let lines = [
            serde_json::json!({"type":"user","timestamp":"2026-10-02T10:00:00Z","message":{"role":"user","content":"Redo the hero video"}}),
            serde_json::json!({"type":"user","timestamp":"2026-10-02T10:00:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"x".repeat(bro_core::sessions::live::FIRST_READ_MAX as usize)}]}}),
            serde_json::json!({"type":"assistant","timestamp":"2026-10-02T10:00:02Z","message":{"id":"m1","content":[{"type":"text","text":"The hero plays the new cut."}]}}),
        ];
        std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        let mut tail = Some(Tail::new(&path));
        let mut facts = Some(AgentFacts::new(Harness::Claude));
        let mut conversation = None;
        let mut binder = Binder::new(BindSpec { harness: Harness::Pi, profile_id: None, config_dir: None, pid: None, cwd: dir.path().into(), started: std::time::SystemTime::now(), resume_id: None, fork_source: None });
        let u = step(Harness::Claude, &mut binder, &mut tail, &mut facts, &mut conversation).expect("the transcript was read");
        assert_eq!(u.conversation.unwrap().items.len(), 2, "what you said and what it said");
    }
}
