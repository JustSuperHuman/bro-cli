//! Auto-resume: an agent that stopped on an API error (connection lost mid-response, overloaded, 5xx,
//! timeouts, rate or usage limits) gets `continue` typed for it after a backoff, or once its limit resets.
//! Errors retrying can't fix (login, bad request, context too long) are surfaced instead.
//!
//! Where the error comes from: the transcript when bro reads one (Claude's synthetic `isApiErrorMessage`
//! entry, Codex's `error` event; exact, one id per occurrence), else the bottom of the screen. A banner on
//! screen has no id, so an occurrence there is told apart by where it sits: a fresh failure lands right
//! above the input box again, an old banner gets pushed up by the turn that followed it.
//!
//! Pure and clock-injected: `Term` feeds it what it sees each poll and carries out the steps.

use std::sync::LazyLock;
use std::time::Duration;

use bro_core::config::AutoResumeSettings;
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone};
use regex::Regex;

use crate::alerts::Kind;
use crate::pane::Activity;

/// Why the agent stopped, as far as retrying goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Cause {
    /// Connection lost, overloaded, 5xx, timeouts: retry soon.
    Transient,
    /// 429 / too many requests: retry, slower.
    RateLimited,
    /// Session / weekly / usage limit: wait for the reset (when the message says when).
    Limit(Option<DateTime<Local>>),
    /// Login, billing, bad request, context too long: retrying won't help.
    Fatal,
}

/// The error an idle agent stopped on.
#[derive(Clone, Debug, PartialEq)]
pub struct Stall {
    /// Identifies the occurrence: the transcript record, or the banner text on screen.
    pub key: String,
    pub text: String,
    pub cause: Cause,
    /// Screen only: non-empty rows below the banner.
    pub below: Option<usize>,
}

/// What `Term` saw this poll.
pub struct Seen {
    pub activity: Option<Activity>,
    /// Since the program last wrote anything.
    pub quiet: Duration,
    pub stall: Option<Stall>,
    /// Counts input from you (keys, paste, phone); a change means you've taken over.
    pub typed: u64,
}

/// What `Term` should do.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Type this and press enter.
    Send(String),
    Toast(Kind, String),
}

/// Idle this long with the error on screen before acting (Claude retries some errors itself first).
const SETTLE: Duration = Duration::from_secs(3);
/// A sent `continue` that hasn't got the agent working by now didn't take.
const NO_TAKE: i64 = 45;
/// Resume this long after a limit's stated reset.
const AFTER_RESET: i64 = 30;

#[derive(Debug)]
struct Occurrence {
    key: String,
    below: Option<usize>,
    /// The agent worked since this was seen.
    armed: bool,
}

#[derive(Debug)]
struct Episode {
    cause: Cause,
    text: String,
    /// Sends so far.
    attempts: u32,
    /// When to send next; None = not sending (needs you, gave up, or waiting for a sent one to take).
    due: Option<DateTime<Local>>,
    /// The last send, until the agent picks it up.
    sent_at: Option<DateTime<Local>>,
    typed: u64,
}

/// One pane's auto-resume state.
#[derive(Debug, Default)]
pub struct AutoResume {
    ep: Option<Episode>,
    seen: Option<Occurrence>,
    /// The agent has worked in this pane (a resumed session's old error on screen isn't ours to retry).
    ever_worked: bool,
}

impl AutoResume {
    pub fn step(&mut self, now: DateTime<Local>, seen: &Seen, cfg: &AutoResumeSettings) -> Vec<Step> {
        let mut out = vec![];
        if !cfg.enabled {
            self.ep = None;
            return out;
        }
        match seen.activity {
            Some(Activity::Working) => {
                self.ever_worked = true;
                if let Some(o) = &mut self.seen {
                    o.armed = true;
                }
                if let Some(ep) = &mut self.ep {
                    ep.sent_at = None;
                }
                return out;
            }
            Some(Activity::Idle) if seen.quiet >= SETTLE => {}
            _ => return out,
        }
        // you typed into it: it's yours again
        if self.ep.as_ref().is_some_and(|ep| ep.typed != seen.typed) {
            self.ep = None;
        }
        let fresh = self.observe(seen.stall.as_ref());
        match (fresh, seen.stall.as_ref()) {
            (true, Some(s)) if self.ever_worked => out.extend(self.stalled(now, s, seen.typed, cfg)),
            (true, _) => {}
            (false, _) if self.seen.is_none() || self.seen.as_ref().is_some_and(|o| o.armed) => {
                // no error any more, or only an old one: whatever we sent worked
                if let Some(ep) = self.ep.take()
                    && ep.attempts > 0
                {
                    let n = ep.attempts;
                    out.push(Step::Toast(Kind::Info, format!("back to work after {n} {}", if n == 1 { "retry" } else { "retries" })));
                }
            }
            (false, _) => {}
        }
        if let Some(ep) = &mut self.ep {
            if ep.sent_at.is_some_and(|t| (now - t).num_seconds() >= NO_TAKE) {
                ep.sent_at = None;
                out.push(Step::Toast(Kind::Error, "auto-resume: the agent didn't pick up `continue` — have a look".into()));
                self.ep = None;
            } else if ep.sent_at.is_none() && ep.due.is_some_and(|d| d <= now) {
                ep.due = None;
                ep.attempts += 1;
                ep.sent_at = Some(now);
                let limit = matches!(ep.cause, Cause::Limit(_));
                let text = if limit { "limit reset — resuming".to_string() } else { format!("↻ retrying ({}/{}) · {}", ep.attempts, cfg.max_attempts, short(&ep.text)) };
                out.push(Step::Send(cfg.prompt.clone()));
                out.push(Step::Toast(Kind::Info, text));
            }
        }
        out
    }

    /// Track the error on screen; true when it's a new occurrence.
    fn observe(&mut self, stall: Option<&Stall>) -> bool {
        let Some(s) = stall else {
            self.seen = None;
            return false;
        };
        let fresh = match &self.seen {
            None => true,
            Some(o) if o.key != s.key => true,
            // same banner text after the agent worked: a new failure sits as low as the first one did,
            // an old one has been pushed up by at least your prompt and a reply
            Some(o) if o.armed => match (o.below, s.below) {
                (Some(was), Some(now)) => now <= was + 1,
                _ => false,
            },
            Some(_) => false,
        };
        if fresh {
            self.seen = Some(Occurrence { key: s.key.clone(), below: s.below, armed: false });
        }
        fresh
    }

    /// A new failure: schedule the next try (or say why not).
    fn stalled(&mut self, now: DateTime<Local>, s: &Stall, typed: u64, cfg: &AutoResumeSettings) -> Vec<Step> {
        let attempts = self.ep.as_ref().map_or(0, |e| e.attempts);
        let mut ep = Episode { cause: s.cause.clone(), text: s.text.clone(), attempts, due: None, sent_at: None, typed };
        let toast = match &s.cause {
            Cause::Fatal => Some((Kind::NeedsYou, format!("stopped: {}", short(&s.text)))),
            _ if attempts >= cfg.max_attempts => Some((Kind::Error, format!("gave up after {attempts} retries: {}", short(&s.text)))),
            Cause::Limit(Some(at)) if cfg.after_limit_reset => {
                let at = (*at).max(now) + chrono::Duration::seconds(AFTER_RESET);
                ep.due = Some(at);
                Some((Kind::Usage, format!("hit a limit — resuming {}", when(at, now))))
            }
            Cause::Limit(Some(_)) => Some((Kind::NeedsYou, format!("stopped: {}", short(&s.text)))),
            cause => {
                ep.due = Some(now + chrono::Duration::seconds(backoff(cause, attempts)));
                matches!(cause, Cause::Limit(None)).then(|| (Kind::Usage, format!("hit a limit — retrying {}", when(ep.due.unwrap_or(now), now))))
            }
        };
        self.ep = Some(ep);
        toast.map(|(k, t)| Step::Toast(k, t)).into_iter().collect()
    }

    /// For the pane header: what auto-resume is waiting on.
    pub fn note(&self, now: DateTime<Local>) -> Option<String> {
        let ep = self.ep.as_ref()?;
        if ep.sent_at.is_some() {
            return Some("↻ resuming".into());
        }
        let due = ep.due?;
        Some(match ep.cause {
            Cause::Limit(_) => format!("↻ resumes {}", when(due, now)),
            _ => format!("↻ retry {} in {}s", ep.attempts + 1, (due - now).num_seconds().max(0)),
        })
    }
}

/// Seconds before try `attempts + 1`.
fn backoff(cause: &Cause, attempts: u32) -> i64 {
    let ladder: &[i64] = match cause {
        Cause::Transient => &[5, 15, 30, 60, 120, 300],
        Cause::RateLimited => &[30, 60, 120, 300, 600],
        _ => &[300, 600, 1200, 1800],
    };
    ladder[(attempts as usize).min(ladder.len() - 1)]
}

fn short(text: &str) -> String {
    let t = text.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
    let t = t.strip_prefix("API Error: ").unwrap_or(t);
    crate::ui::fit(&t.split_whitespace().collect::<Vec<_>>().join(" "), 60)
}

/// "2:40am", or "Oct 4 11:00am" when it isn't today.
fn when(at: DateTime<Local>, now: DateTime<Local>) -> String {
    if at.date_naive() == now.date_naive() { at.format("%-I:%M%P").to_string() } else { at.format("%b %-d %-I:%M%P").to_string() }
}

// ------------------------------------------------------------------ classifying

const FATAL: &[&str] = &[
    "/login", "login expired", "failed to authenticate", "authentication", "invalid api key", "invalid x-api-key", "unauthorized",
    "access grant", "permission denied", "credit balance", "billing", "context length", "context window", "prompt is too long",
    "too many tokens", "invalid_request", "invalid request", "api error: 400", "api error: 401", "api error: 403", "api error: 404",
    "api error: 413", "api error: 422", "status 400", "status 401", "status 403", "status 404", "model not found", "does not exist",
];
const LIMIT: &[&str] = &["hit your", "usage limit", "limit reached", "limit · resets", "limit resets", "out of extra usage", "try again at", "try again in"];
const RATE: &[&str] = &["429", "rate limit", "rate_limit", "too many requests", "exceeded retry limit"];
const TRANSIENT: &[&str] = &[
    "connection lost", "mid-response", "connection error", "connection reset", "connection refused", "connection closed", "overloaded",
    "529", "500", "502", "503", "504", "internal server error", "server error", "bad gateway", "service unavailable", "gateway timeout",
    "timed out", "timeout", "econnreset", "econnrefused", "etimedout", "epipe", "socket hang up", "fetch failed", "network error",
    "stream disconnected", "stream error", "stream closed", "at capacity", "temporarily unavailable", "request was aborted",
    "premature close", "upstream",
];

/// What an error message means for retrying. `code` is Claude's `error` field when it came from the transcript.
/// None: not an error we handle (an interrupt you made, say).
pub fn classify(text: &str, code: Option<&str>, now: DateTime<Local>) -> Option<Cause> {
    let t = text.to_lowercase();
    let has = |list: &[&str]| list.iter().any(|m| t.contains(m));
    if t.contains("interrupted") {
        return None;
    }
    if code == Some("authentication_failed") || has(FATAL) {
        return Some(Cause::Fatal);
    }
    if has(LIMIT) || (code == Some("rate_limit") && t.contains("reset")) {
        return Some(Cause::Limit(reset_time(&t, now)));
    }
    if code == Some("rate_limit") || has(RATE) {
        return Some(Cause::RateLimited);
    }
    if code == Some("server_error") || has(TRANSIENT) || t.starts_with("api error") {
        return Some(Cause::Transient);
    }
    None
}

static IN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:again|resets?|retry|available) in\s+((?:\d+\s*[a-z]+[\s,]*(?:and\s+)?)+)").unwrap());
static UNIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)\s*([a-z]+)").unwrap());
static AT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:resets|reset at|again at|available at|until)\s+(?:on\s+)?(?:(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+(\d{1,2})(?:st|nd|rd|th)?,?\s*(?:(\d{4}),?\s*)?(?:at\s+)?)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?")
        .unwrap()
});

/// When a limit message says it resets: "resets 2:40am (America/New_York)", "resets Oct 4, 11am",
/// "try again at 3:45 PM", "try again in 2 days 3 hours". Times are read as local (the zone these messages
/// name is yours).
pub fn reset_time(text: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let t = text.to_lowercase();
    if let Some(c) = IN.captures(&t) {
        let mut secs = 0i64;
        for u in UNIT.captures_iter(&c[1]) {
            let n: i64 = u[1].parse().ok()?;
            secs += n * match &u[2] {
                s if s.starts_with('d') => 86_400,
                s if s.starts_with('h') => 3_600,
                s if s.starts_with("mo") => return None,
                s if s.starts_with('m') => 60,
                s if s.starts_with('s') => 1,
                _ => 0,
            };
        }
        return (secs > 0).then(|| now + chrono::Duration::seconds(secs));
    }
    let c = AT.captures(&t)?;
    let ampm = c.get(6).map(|m| m.as_str());
    let min: u32 = c.get(5).map_or(Some(0), |m| m.as_str().parse().ok())?;
    // a bare number isn't a time
    if ampm.is_none() && c.get(5).is_none() {
        return None;
    }
    let mut hour: u32 = c[4].parse().ok()?;
    match ampm {
        Some("pm") if hour < 12 => hour += 12,
        Some("am") if hour == 12 => hour = 0,
        _ => {}
    }
    let at = |d: NaiveDate| Local.from_local_datetime(&d.and_hms_opt(hour, min, 0)?).earliest();
    match c.get(1) {
        Some(mon) => {
            let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| *m == mon.as_str())? as u32 + 1;
            let day: u32 = c[2].parse().ok()?;
            let year = c.get(3).and_then(|y| y.as_str().parse().ok());
            let t = at(NaiveDate::from_ymd_opt(year.unwrap_or(now.year()), month, day)?)?;
            if year.is_none() && t < now - chrono::Duration::days(1) {
                return at(NaiveDate::from_ymd_opt(now.year() + 1, month, day)?);
            }
            Some(t)
        }
        None => {
            let t = at(now.date_naive())?;
            if t > now { Some(t) } else { at(now.date_naive().succ_opt()?) }
        }
    }
}

/// Lines that start like an agent's error banner (after `⎿` / `■` / box drawing).
const BANNER: &[&str] = &[
    "api error", "stream disconnected", "selected model is at capacity", "exceeded retry limit", "unexpected status",
    "you've hit your", "you have hit your", "please run /login", "login expired", "failed to authenticate", "request timed out",
    "connection error", "overloaded",
];
/// How many non-empty rows from the bottom to look through (input box, status lines, the banner).
const BOTTOM: usize = 12;

/// The newest error banner near the bottom of the screen (rows top to bottom), as a [`Stall`].
pub fn screen_stall(rows: &[String], now: DateTime<Local>) -> Option<Stall> {
    let rows: Vec<&str> = rows.iter().map(|r| r.trim()).filter(|r| !r.is_empty()).collect();
    for (below, i) in (0..rows.len()).rev().take(BOTTOM).enumerate() {
        let raw = rows[i];
        let line = raw.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '/').trim();
        let lower = line.to_lowercase();
        if !(BANNER.iter().any(|b| lower.starts_with(b)) || raw.starts_with('■')) {
            continue;
        }
        // a long message wraps: read the next row too
        let text = match rows.get(i + 1) {
            Some(next) if below > 0 && !next.starts_with(['>', '›', '❯', '─', '╭', '│']) => format!("{line} {}", next.trim()),
            _ => line.to_string(),
        };
        let cause = classify(&text, None, now)?;
        return Some(Stall { key: format!("s:{line}"), text, cause, below: Some(below) });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Local> {
        Local.from_local_datetime(&chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()).unwrap()
    }

    #[test]
    fn classifies_the_errors_seen_in_the_wild() {
        let now = at("2026-10-07 23:00:00");
        let c = |t: &str, code: Option<&str>| classify(t, code, now);
        assert_eq!(c("API Error: Connection lost mid-response. The response above may be incomplete.", Some("server_error")), Some(Cause::Transient));
        assert_eq!(c("API Error: 529 {\"type\":\"overloaded_error\"}", None), Some(Cause::Transient));
        assert_eq!(c("stream disconnected before completion: Our servers are currently overloaded", None), Some(Cause::Transient));
        assert_eq!(c("Selected model is at capacity. Please try a different model.", None), Some(Cause::Transient));
        assert_eq!(c("API Error: Request timed out.", None), Some(Cause::Transient));
        assert_eq!(c("exceeded retry limit, last status: 429 Too Many Requests", None), Some(Cause::RateLimited));
        assert_eq!(c("You've hit your session limit · resets 2:40am (America/New_York)", Some("rate_limit")), Some(Cause::Limit(Some(at("2026-10-08 02:40:00")))));
        assert_eq!(c("You've hit your weekly limit · resets Oct 9, 11am (America/New_York)", Some("rate_limit")), Some(Cause::Limit(Some(at("2026-10-09 11:00:00")))));
        assert_eq!(c("Please run /login · API Error: 403 Access to this model requires an access grant", Some("authentication_failed")), Some(Cause::Fatal));
        assert_eq!(c("Login expired · Please run /login", None), Some(Cause::Fatal));
        assert_eq!(c("API Error: 400 This endpoint's maximum context length is 260000 tokens.", Some("unknown")), Some(Cause::Fatal));
        assert_eq!(c("Conversation interrupted - tell the model what to do differently", None), None);
        assert_eq!(c("all tests passed", None), None);
    }

    #[test]
    fn reads_reset_times() {
        let now = at("2026-10-07 23:00:00");
        assert_eq!(reset_time("you've hit your usage limit. upgrade to pro or try again at 3:45 PM.", now), Some(at("2026-10-08 15:45:00")));
        assert_eq!(reset_time("try again at 11:30pm", now), Some(at("2026-10-07 23:30:00")));
        assert_eq!(reset_time("try again in 2 days 3 hours 5 minutes", now), Some(now + chrono::Duration::seconds(2 * 86_400 + 3 * 3_600 + 300)));
        assert_eq!(reset_time("please try again in 20s", now), Some(now + chrono::Duration::seconds(20)));
        assert_eq!(reset_time("resets Jan 2, 9am", now), Some(at("2027-01-02 09:00:00")));
        assert_eq!(reset_time("resets oct 9th, 2026 3:45 pm", now), Some(at("2026-10-09 15:45:00")));
        assert_eq!(reset_time("resets soon", now), None);
        assert_eq!(reset_time("resets 5", now), None);
    }

    fn rows(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    const CLAUDE: &str = "⏺ Reading the config\n  ⎿  API Error: Connection lost mid-response. The response above may\n     be incomplete.\n\n╭──────────────────────────╮\n│ >                        │\n╰──────────────────────────╯\n  ⏵⏵ bypass permissions on";

    #[test]
    fn finds_the_banner_at_the_bottom_of_the_screen() {
        let now = at("2026-10-07 23:00:00");
        let s = screen_stall(&rows(CLAUDE), now).expect("claude banner");
        assert_eq!((s.cause.clone(), s.below), (Cause::Transient, Some(5)));
        assert!(s.text.ends_with("may be incomplete."), "{}", s.text);
        let codex = "• Edited src/main.rs\n■ stream disconnected before completion: Our servers are currently overloaded\n\n› Ask Codex to do anything\n  ? for shortcuts";
        assert_eq!(screen_stall(&rows(codex), now).map(|s| s.cause), Some(Cause::Transient));
        let interrupted = "■ Conversation interrupted - tell the model what to do differently\n\n› Ask Codex to do anything";
        assert_eq!(screen_stall(&rows(interrupted), now), None);
        // the agent's own prose mentioning a timeout isn't a banner
        assert_eq!(screen_stall(&rows("⏺ I raised the request timeout to 30s.\n> "), now), None);
        // too far up to be the last thing that happened
        let far = format!("API Error: overloaded\n{}", "line\n".repeat(20));
        assert_eq!(screen_stall(&rows(&far), now), None);
    }

    fn cfg() -> AutoResumeSettings {
        AutoResumeSettings { max_attempts: 3, ..Default::default() }
    }

    fn banner(below: usize) -> Option<Stall> {
        Some(Stall { key: "s:API Error: Connection lost".into(), text: "API Error: Connection lost".into(), cause: Cause::Transient, below: Some(below) })
    }

    fn idle(stall: Option<Stall>) -> Seen {
        Seen { activity: Some(Activity::Idle), quiet: Duration::from_secs(10), stall, typed: 0 }
    }

    fn working() -> Seen {
        Seen { activity: Some(Activity::Working), quiet: Duration::ZERO, stall: None, typed: 0 }
    }

    fn sends(steps: &[Step]) -> usize {
        steps.iter().filter(|s| matches!(s, Step::Send(_))).count()
    }

    #[test]
    fn retries_a_lost_connection_and_stands_down_once_it_works() {
        let (mut r, cfg, t0) = (AutoResume::default(), cfg(), at("2026-10-07 12:00:00"));
        r.step(t0, &working(), &cfg);
        assert_eq!(sends(&r.step(t0, &idle(banner(5)), &cfg)), 0);
        assert_eq!(r.note(t0).as_deref(), Some("↻ retry 1 in 5s"));
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(3), &idle(banner(5)), &cfg)), 0);
        let s = r.step(t0 + chrono::Duration::seconds(5), &idle(banner(5)), &cfg);
        assert_eq!(s[0], Step::Send("continue".into()));
        // nothing more while it's waiting for the send to take
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(20), &idle(banner(5)), &cfg)), 0);
        r.step(t0 + chrono::Duration::seconds(21), &working(), &cfg);
        // the turn finished; the old banner was pushed up by the prompt and reply
        let s = r.step(t0 + chrono::Duration::seconds(60), &idle(banner(9)), &cfg);
        assert_eq!(sends(&s), 0);
        assert!(matches!(&s[..], [Step::Toast(Kind::Info, t)] if t.contains("after 1 retry")), "{s:?}");
        assert!(r.ep.is_none());
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(600), &idle(banner(9)), &cfg)), 0);
    }

    #[test]
    fn a_retry_that_fails_again_backs_off_then_gives_up() {
        let (mut r, cfg, mut t) = (AutoResume::default(), cfg(), at("2026-10-07 12:00:00"));
        r.step(t, &working(), &cfg);
        let mut sent = 0;
        let mut gave_up = false;
        for _ in 0..40 {
            t += chrono::Duration::seconds(5);
            let s = r.step(t, &idle(banner(5)), &cfg);
            sent += sends(&s);
            gave_up |= s.iter().any(|x| matches!(x, Step::Toast(Kind::Error, m) if m.contains("gave up")));
            if sends(&s) > 0 {
                // it worked a moment, then failed the same way: a new banner just as low
                r.step(t, &working(), &cfg);
            }
        }
        assert_eq!(sent, 3);
        assert!(gave_up);
    }

    #[test]
    fn transcript_errors_are_told_apart_by_id() {
        let (mut r, cfg, t0) = (AutoResume::default(), cfg(), at("2026-10-07 12:00:00"));
        let err = |id: &str| Some(Stall { key: format!("t:{id}"), text: "API Error: Connection lost".into(), cause: Cause::Transient, below: None });
        r.step(t0, &working(), &cfg);
        r.step(t0, &idle(err("a")), &cfg);
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(5), &idle(err("a")), &cfg)), 1);
        r.step(t0 + chrono::Duration::seconds(6), &working(), &cfg);
        // failed again: a new record
        r.step(t0 + chrono::Duration::seconds(10), &idle(err("b")), &cfg);
        assert_eq!(r.note(t0 + chrono::Duration::seconds(10)).as_deref(), Some("↻ retry 2 in 15s"));
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(25), &idle(err("b")), &cfg)), 1);
        r.step(t0 + chrono::Duration::seconds(26), &working(), &cfg);
        assert!(!r.step(t0 + chrono::Duration::seconds(40), &idle(None), &cfg).is_empty());
        assert!(r.ep.is_none());
    }

    #[test]
    fn waits_for_a_limit_to_reset() {
        let (mut r, cfg, t0) = (AutoResume::default(), cfg(), at("2026-10-07 23:00:00"));
        let lim = Some(Stall { key: "t:x".into(), text: "You've hit your session limit · resets 2:40am".into(), cause: Cause::Limit(Some(at("2026-10-08 02:40:00"))), below: None });
        r.step(t0, &working(), &cfg);
        let s = r.step(t0, &idle(lim.clone()), &cfg);
        assert!(matches!(&s[..], [Step::Toast(Kind::Usage, m)] if m.contains("Oct 8 2:40am")), "{s:?}");
        assert_eq!(sends(&r.step(at("2026-10-08 02:40:10"), &idle(lim.clone()), &cfg)), 0);
        assert_eq!(sends(&r.step(at("2026-10-08 02:40:30"), &idle(lim), &cfg)), 1);
    }

    #[test]
    fn leaves_alone_what_it_cannot_fix_or_should_not_touch() {
        let (cfg, t0) = (cfg(), at("2026-10-07 12:00:00"));
        // a login error: tell you, never type
        let mut r = AutoResume::default();
        r.step(t0, &working(), &cfg);
        let fatal = Some(Stall { key: "t:f".into(), text: "Please run /login".into(), cause: Cause::Fatal, below: None });
        let s = r.step(t0, &idle(fatal.clone()), &cfg);
        assert!(matches!(&s[..], [Step::Toast(Kind::NeedsYou, _)]));
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(3600), &idle(fatal), &cfg)), 0);
        // an error already on screen when the pane opened (a resumed session) isn't ours
        let mut r = AutoResume::default();
        assert_eq!(sends(&r.step(t0, &idle(banner(5)), &cfg)), 0);
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(60), &idle(banner(5)), &cfg)), 0);
        // you started typing: hands off
        let mut r = AutoResume::default();
        r.step(t0, &working(), &cfg);
        r.step(t0, &idle(banner(5)), &cfg);
        let typed = Seen { typed: 1, ..idle(banner(5)) };
        assert_eq!(sends(&r.step(t0 + chrono::Duration::seconds(10), &typed, &cfg)), 0);
        assert!(r.ep.is_none());
        // still settling, or switched off
        let mut r = AutoResume::default();
        r.step(t0, &working(), &cfg);
        let settling = Seen { quiet: Duration::from_secs(1), ..idle(banner(5)) };
        assert!(r.step(t0, &settling, &cfg).is_empty() && r.ep.is_none());
        let off = AutoResumeSettings { enabled: false, ..cfg.clone() };
        r.step(t0, &idle(banner(5)), &off);
        assert!(r.ep.is_none());
    }
}
