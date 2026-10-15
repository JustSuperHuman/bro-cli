//! Watchdogs on bro's side: when to wake one (its session stopped and sat idle, or has waited on a question
//! too long), which login reviews (the one with the most usage left, Claude first), the setup / status
//! overlay ([`view`]) and the prompt editor ([`prompt_editor`]). The bridge keeps each plan and runs the
//! reviewer (`bro_bridge::watchdog`).

pub mod prompt_editor;
pub mod view;

use crate::services::UsageEntry;
use bro_bridge::{ReviewerKind, ReviewerLaunch, WatchdogSnapshot};
use bro_core::config::WatchdogSettings;
use bro_core::profiles::{Profile, ProfileKind};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A question on screen waits at least this long before the watchdog may answer it.
const QUESTION_WAIT: Duration = Duration::from_secs(120);
/// After a failed review: try again after these pauses, then wait for the session to move.
const RETRY: [Duration; 3] = [Duration::from_secs(120), Duration::from_secs(300), Duration::from_secs(900)];

/// The choices the setup form and settings offer.
pub const IDLE_CHOICES: [u64; 6] = [20, 60, 120, 300, 600, 1800];
pub const BUDGET_CHOICES: [u32; 5] = [10, 25, 50, 100, 250];
pub const MODEL_CHOICES: [&str; 4] = ["opus", "fable", "sonnet", "haiku"];

/// "20s", "1 min", "30 min".
pub fn human(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{}m {}s", s / 60, s % 60),
    }
}

/// The next / previous of `choices` from `value` (values off the list snap to the nearest).
pub fn step_choice<T: Copy + PartialOrd>(choices: &[T], value: T, dir: isize) -> T {
    let i = choices.iter().position(|c| *c >= value).unwrap_or(choices.len() - 1) as isize;
    let exact = choices.get(i as usize).is_some_and(|c| *c == value);
    let next = if exact || dir > 0 { i + if exact { dir } else { 0 } } else { i - 1 };
    choices[next.clamp(0, choices.len() as isize - 1) as usize]
}

/// What the session is doing this poll.
pub struct Seen<'a> {
    /// the agent is mid-turn
    pub busy: bool,
    /// it waits on you: the question, when bro read one
    pub question: Option<&'a str>,
    /// a helper session the reviewer opened is mid-turn
    pub helpers_busy: bool,
}

/// One watched session's trigger.
#[derive(Debug, Default)]
pub struct Trigger {
    /// The session worked since the last wake-up (a reviewer that sent nothing doesn't wake again until
    /// something moves).
    armed: bool,
    quiet_since: Option<Instant>,
    blocked_since: Option<Instant>,
    /// the last finished wake-up seen, to notice failures
    last_run: Option<String>,
    retry_at: Option<Instant>,
    failures: usize,
}

impl Trigger {
    /// `armed`: wake once the session has been idle long enough even if it hasn't worked since.
    pub fn new(armed: bool) -> Trigger {
        Trigger { armed, ..Trigger::default() }
    }

    /// The reason to wake now, if it's time.
    pub fn step(&mut self, now: Instant, seen: &Seen, dog: &WatchdogSnapshot) -> Option<String> {
        if let Some(last) = dog.runs.last()
            && self.last_run.as_deref() != Some(last.finished_at.as_str())
        {
            self.last_run = Some(last.finished_at.clone());
            if last.ok {
                self.failures = 0;
                self.retry_at = None;
            } else {
                self.retry_at = RETRY.get(self.failures).map(|d| now + *d);
                self.failures += 1;
            }
        }
        if seen.busy || seen.helpers_busy {
            self.armed = true;
            self.quiet_since = None;
            self.blocked_since = None;
            return None;
        }
        if !dog.wakeable() {
            self.quiet_since = None;
            self.blocked_since = None;
            return None;
        }
        let idle = Duration::from_secs(dog.plan.idle_secs.max(10));
        let retry = self.retry_at.is_some_and(|at| now >= at);
        if let Some(question) = seen.question {
            self.quiet_since = None;
            let since = *self.blocked_since.get_or_insert(now);
            let wait = idle.max(QUESTION_WAIT);
            if (self.armed || retry) && now.duration_since(since) >= wait {
                self.fire();
                return Some(format!("it has waited {} on a question: {}", human(wait.as_secs()), crate::ui::fit(question, 120)));
            }
            return None;
        }
        self.blocked_since = None;
        let since = *self.quiet_since.get_or_insert(now);
        if retry {
            self.fire();
            return Some("trying again: the last review failed".into());
        }
        if self.armed && now.duration_since(since) >= idle {
            self.fire();
            return Some(format!("it finished a turn and sat idle {}", human(idle.as_secs())));
        }
        None
    }

    fn fire(&mut self) {
        self.armed = false;
        self.retry_at = None;
        self.quiet_since = None;
        self.blocked_since = None;
    }
}

/// % left of a login's tighter window, when bro has a reading.
pub fn left_of(p: &Profile, usage: &BTreeMap<String, UsageEntry>) -> Option<f64> {
    let u = usage.get(&p.id)?.usage.as_ref()?;
    let d = bro_core::usage::headroom_detail(p, u);
    [d.h5, d.wk].into_iter().flatten().reduce(f64::min)
}

/// The login that reviews: the one settings name, else the roomiest Claude login, else the roomiest Codex
/// one. Logins under bro's ready floor only when nothing else is signed in. With the % left it has.
pub fn pick_reviewer<'a>(profiles: &'a [Profile], usage: &BTreeMap<String, UsageEntry>, cfg: &WatchdogSettings) -> Option<(&'a Profile, Option<f64>)> {
    let signed_in = profiles.iter().filter(|p| p.authenticated && (p.is_claude() || p.is_codex()));
    if cfg.reviewer != "auto"
        && let Some(p) = profiles.iter().find(|p| p.id == cfg.reviewer && p.authenticated)
    {
        return Some((p, left_of(p, usage)));
    }
    let floor = bro_core::usage::READY_FLOOR;
    let mut ranked: Vec<(u8, f64, &Profile, Option<f64>)> = signed_in
        .map(|p| {
            let left = left_of(p, usage);
            let tier = match (p.is_claude(), left) {
                (true, Some(l)) if l >= floor => 0,
                (false, Some(l)) if l >= floor => 1,
                (true, None) => 2,
                (false, None) => 3,
                _ => 9,
            };
            (tier, left.unwrap_or(0.0), p, left)
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    ranked.into_iter().find(|r| r.0 < 9).map(|r| (r.2, r.3))
}

/// How the bridge starts a reviewer on `p`.
pub fn reviewer_launch(p: &Profile, cfg: &WatchdogSettings) -> ReviewerLaunch {
    if p.is_codex() {
        return ReviewerLaunch {
            kind: ReviewerKind::Codex,
            program: "codex".into(),
            env: vec![("CODEX_HOME".into(), p.dir.to_string_lossy().into_owned())],
            env_remove: vec![],
            model: None,
            label: format!("codex · {}", p.name),
        };
    }
    let mut env = vec![];
    let mut env_remove: Vec<String> = ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "ANTHROPIC_MODEL"].iter().map(|v| v.to_string()).collect();
    if p.kind == ProfileKind::ClaudeAccount {
        env.push(("CLAUDE_CONFIG_DIR".into(), p.dir.to_string_lossy().into_owned()));
    } else {
        env_remove.push("CLAUDE_CONFIG_DIR".into());
    }
    let model = Some(cfg.model.trim()).filter(|m| !m.is_empty()).map(str::to_string);
    let label = match &model {
        Some(m) => format!("claude · {} · {m}", p.name),
        None => format!("claude · {}", p.name),
    };
    ReviewerLaunch { kind: ReviewerKind::Claude, program: "claude".into(), env, env_remove, model, label }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_bridge::{RunRecord, WatchdogPlan};
    use bro_core::usage::{Usage, Window};

    fn dog() -> WatchdogSnapshot {
        WatchdogSnapshot { plan: WatchdogPlan { idle_secs: 60, max_wakes: 10, ..WatchdogPlan::default() }, ..WatchdogSnapshot::default() }
    }

    const IDLE: Seen = Seen { busy: false, question: None, helpers_busy: false };
    const BUSY: Seen = Seen { busy: true, question: None, helpers_busy: false };

    #[test]
    fn wakes_once_per_stop_after_the_idle_wait() {
        let (mut t, d, t0) = (Trigger::new(false), dog(), Instant::now());
        assert_eq!(t.step(t0, &IDLE, &d), None);
        assert_eq!(t.step(t0 + Duration::from_secs(600), &IDLE, &d), None, "never worked since: not armed");
        t.step(t0 + Duration::from_secs(601), &BUSY, &d);
        assert_eq!(t.step(t0 + Duration::from_secs(620), &IDLE, &d), None);
        assert_eq!(t.step(t0 + Duration::from_secs(679), &IDLE, &d), None);
        let why = t.step(t0 + Duration::from_secs(681), &IDLE, &d).expect("wakes");
        assert!(why.contains("idle 1 min"), "{why}");
        assert_eq!(t.step(t0 + Duration::from_secs(2000), &IDLE, &d), None, "one wake per stop");
        // helpers working count as work
        t.step(t0 + Duration::from_secs(2001), &Seen { helpers_busy: true, ..IDLE }, &d);
        assert_eq!(t.step(t0 + Duration::from_secs(2100), &IDLE, &d), None);
        assert!(t.step(t0 + Duration::from_secs(2161), &IDLE, &d).is_some());
    }

    #[test]
    fn questions_wait_longer_and_failures_retry() {
        let (mut t, mut d, t0) = (Trigger::new(true), dog(), Instant::now());
        let q = Seen { question: Some("Run the migration?"), ..IDLE };
        assert_eq!(t.step(t0, &q, &d), None);
        assert_eq!(t.step(t0 + Duration::from_secs(90), &q, &d), None);
        assert!(t.step(t0 + Duration::from_secs(121), &q, &d).unwrap().contains("Run the migration?"));
        // the review failed: retry after two minutes even though nothing moved
        d.runs.push(RunRecord { finished_at: "a".into(), ok: false, ..RunRecord::default() });
        let t1 = t0 + Duration::from_secs(200);
        assert_eq!(t.step(t1, &IDLE, &d), None);
        assert!(t.step(t1 + Duration::from_secs(121), &IDLE, &d).unwrap().contains("failed"));
        // paused / finished: nothing
        let mut t = Trigger::new(true);
        d.paused = true;
        assert_eq!(t.step(t0 + Duration::from_secs(9999), &IDLE, &d), None);
    }

    fn profile(id: &str, kind: ProfileKind) -> Profile {
        Profile { id: id.into(), kind, name: id.split(':').nth(1).unwrap_or(id).into(), dir: format!("/p/{id}").into(), authenticated: true, plan: None, tier: None, identity: None, email: None }
    }

    fn usage(used: f32) -> UsageEntry {
        UsageEntry { usage: Some(Usage { five_hour: Some(Window { used_pct: used, resets_at: None, window_mins: Some(300) }), ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn picks_the_roomiest_claude_then_codex() {
        let ps = vec![profile("claude:work", ProfileKind::ClaudeAccount), profile("claude:home", ProfileKind::ClaudeAccount), profile("codex:me", ProfileKind::CodexProfile)];
        let cfg = WatchdogSettings::default();
        let mut u = BTreeMap::new();
        u.insert("claude:work".to_string(), usage(80.0));
        u.insert("claude:home".to_string(), usage(30.0));
        u.insert("codex:me".to_string(), usage(0.0));
        assert_eq!(pick_reviewer(&ps, &u, &cfg).unwrap().0.id, "claude:home");
        u.insert("claude:work".to_string(), usage(100.0));
        u.insert("claude:home".to_string(), usage(99.0));
        assert_eq!(pick_reviewer(&ps, &u, &cfg).unwrap().0.id, "codex:me", "Claude is out: Codex reviews");
        let fixed = WatchdogSettings { reviewer: "claude:work".into(), ..cfg.clone() };
        assert_eq!(pick_reviewer(&ps, &u, &fixed).unwrap().0.id, "claude:work");
        let launch = reviewer_launch(&ps[0], &cfg);
        assert_eq!(launch.label, "claude · work · opus");
        assert!(launch.env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR"));
        assert_eq!(reviewer_launch(&ps[2], &cfg).kind, ReviewerKind::Codex);
    }

    #[test]
    fn choices_step_and_snap() {
        assert_eq!(step_choice(&IDLE_CHOICES, 60, 1), 120);
        assert_eq!(step_choice(&IDLE_CHOICES, 60, -1), 20);
        assert_eq!(step_choice(&IDLE_CHOICES, 1800, 1), 1800);
        assert_eq!(step_choice(&IDLE_CHOICES, 90, 1), 120, "off the list snaps up");
        assert_eq!(step_choice(&IDLE_CHOICES, 90, -1), 60);
        assert_eq!(human(20), "20s");
        assert_eq!(human(120), "2 min");
    }
}
