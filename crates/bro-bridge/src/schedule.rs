//! The orchestrator's scheduler. The orchestrator adds and removes triggers with its tools; bro fires them
//! itself: a timer on the bridge runtime hands the prompt to bro, which types it into the orchestrator session.
//! The agent never polls or keeps time.
//!
//! Session triggers live in memory and last as long as this bro's orchestrator. Persistent ones live in
//! `<data_root>/schedule.json`, shared by every bro on the machine. An occurrence fires within
//! [`WINDOW_SECS`] of when it was due, by whichever bro claims it first (an atomic marker file per occurrence);
//! the other bros, and any bro that comes by later than that, let it pass.

use crate::state::AppState;
use chrono::{Datelike, Local, NaiveTime, TimeZone};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An occurrence fires within this many seconds of its time, or not at all.
pub const WINDOW_SECS: i64 = 60;
/// How often the timer looks.
const TICK: Duration = Duration::from_secs(5);
const FILE: &str = "schedule.json";
const CLAIMS: &str = "schedule-claims";
/// Claim markers older than this are swept.
const CLAIM_KEEP_SECS: u64 = 2 * 86_400;

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// When a trigger fires.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum When {
    /// Once, at this unix time.
    Once { at: i64 },
    /// Every `secs`, counted from `anchor`.
    Every { secs: u64, anchor: i64 },
    /// Every day at local `time` ("HH:MM"), only on `days` (0 = Monday) when any are listed.
    Daily {
        time: String,
        #[serde(default)]
        days: Vec<u8>,
    },
}

impl When {
    /// The first occurrence strictly after `after` (None: a one-off that has passed).
    pub fn next_after(&self, after: i64) -> Option<i64> {
        match self {
            When::Once { at } => (*at > after).then_some(*at),
            When::Every { secs, anchor } => {
                let secs = (*secs).max(1) as i64;
                if after < *anchor {
                    return Some(*anchor);
                }
                Some(anchor + ((after - anchor) / secs + 1) * secs)
            }
            When::Daily { time, days } => {
                let t = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
                let start = Local.timestamp_opt(after, 0).single()?.date_naive();
                (0..9).find_map(|d| {
                    let day = start + chrono::Days::new(d);
                    if !days.is_empty() && !days.contains(&(day.weekday().num_days_from_monday() as u8)) {
                        return None;
                    }
                    let at = Local.from_local_datetime(&day.and_time(t)).earliest()?.timestamp();
                    (at > after).then_some(at)
                })
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            When::Once { at } => format!("once at {}", local(*at)),
            When::Every { secs, .. } => format!("every {}", duration_text(*secs)),
            When::Daily { time, days } if days.is_empty() => format!("daily at {time}"),
            When::Daily { time, days } => {
                let names: Vec<&str> = days.iter().filter_map(|d| WEEKDAYS.get(*d as usize).copied()).collect();
                format!("{} at {time}", names.join(", "))
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub id: String,
    pub name: String,
    /// What bro types into the orchestrator when it fires.
    pub prompt: String,
    pub when: When,
    /// Kept in `schedule.json` and run by whichever bro is up; else only for this bro's orchestrator.
    pub persist: bool,
    pub created_at: i64,
    /// The occurrence it waits for (None: done).
    pub next_run: Option<i64>,
    #[serde(default)]
    pub last_run: Option<i64>,
    #[serde(default)]
    pub runs: u64,
}

impl Trigger {
    pub fn public(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "prompt": self.prompt,
            "when": self.when.describe(),
            "persist": self.persist,
            "nextRun": self.next_run.map(local),
            "lastRun": self.last_run.map(local),
            "runs": self.runs,
        })
    }
}

/// Session triggers in memory, persistent ones on disk.
pub struct Schedule {
    session: Mutex<Vec<Trigger>>,
    path: PathBuf,
    claims: PathBuf,
}

impl Schedule {
    pub fn new(data_root: &Path) -> Self {
        Self { session: Mutex::new(vec![]), path: data_root.join(FILE), claims: data_root.join(CLAIMS) }
    }

    fn load(&self) -> Vec<Trigger> {
        std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, list: &[Trigger]) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(list).unwrap_or_default())
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| format!("couldn't save {}: {e}", self.path.display()))
    }

    /// Add a trigger; it waits for its first occurrence after `now`.
    pub fn add(&self, mut t: Trigger, now: i64) -> Result<Trigger, String> {
        t.next_run = t.when.next_after(now);
        if t.next_run.is_none() {
            return Err("That time has already passed.".into());
        }
        if t.persist {
            let mut list = self.load();
            list.push(t.clone());
            self.save(&list)?;
        } else {
            self.session.lock().push(t.clone());
        }
        Ok(t)
    }

    /// Remove by id (or unique id prefix, or exact name).
    pub fn remove(&self, wanted: &str) -> Result<Trigger, String> {
        let pick = |list: &[Trigger]| -> Option<usize> {
            let w = wanted.trim().to_lowercase();
            list.iter().position(|t| t.id == wanted || t.name.to_lowercase() == w).or_else(|| {
                let hits: Vec<usize> = list.iter().enumerate().filter(|(_, t)| t.id.starts_with(&w)).map(|(i, _)| i).collect();
                (hits.len() == 1).then(|| hits[0])
            })
        };
        {
            let mut session = self.session.lock();
            if let Some(i) = pick(&session) {
                return Ok(session.remove(i));
            }
        }
        let mut list = self.load();
        let i = pick(&list).ok_or_else(|| format!("No scheduled trigger matches \"{wanted}\". schedule_list shows them."))?;
        let t = list.remove(i);
        self.save(&list)?;
        Ok(t)
    }

    /// Every trigger: this bro's session ones, then the persistent ones.
    pub fn list(&self) -> Vec<Trigger> {
        let mut all = self.session.lock().clone();
        all.extend(self.load());
        all
    }

    /// The orchestrator closed: its session triggers go with it.
    pub fn clear_session(&self) {
        self.session.lock().clear();
    }

    /// Claim one occurrence for this bro: true for exactly one caller across every bro on the machine.
    fn claim(&self, id: &str, at: i64) -> bool {
        let _ = std::fs::create_dir_all(&self.claims);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.claims.join(format!("{id}-{at}.claim")))
            .is_ok()
    }

    fn sweep_claims(&self) {
        let Ok(dir) = std::fs::read_dir(&self.claims) else { return };
        for e in dir.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age.as_secs() > CLAIM_KEEP_SECS);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// The occurrences this bro fires at `now`. Every due trigger moves on to its next occurrence; one more than
    /// [`WINDOW_SECS`] late, or claimed (or run, per `lastRun`) by another bro, is let pass.
    pub fn due(&self, now: i64) -> Vec<Trigger> {
        let mut fire = vec![];
        let step = |t: &mut Trigger, fire: &mut Vec<Trigger>, claim: &dyn Fn(&Trigger, i64) -> bool| -> bool {
            let Some(at) = t.next_run.filter(|at| *at <= now) else { return false };
            let ran_already = t.last_run.is_some_and(|last| (last - at).abs() <= WINDOW_SECS);
            if now - at <= WINDOW_SECS && !ran_already && claim(t, at) {
                t.last_run = Some(now);
                t.runs += 1;
                fire.push(t.clone());
            }
            t.next_run = t.when.next_after(now.max(at));
            true
        };
        {
            let mut session = self.session.lock();
            for t in session.iter_mut() {
                step(t, &mut fire, &|_, _| true);
            }
            session.retain(|t| t.next_run.is_some());
        }
        let mut list = self.load();
        let mut changed = false;
        for t in list.iter_mut() {
            changed |= step(t, &mut fire, &|t, at| self.claim(&t.id, at));
        }
        if changed {
            list.retain(|t| t.next_run.is_some());
            let _ = self.save(&list);
            self.sweep_claims();
        }
        fire
    }
}

/// The timer: while bro listens for orchestrator commands, hand it what's due.
pub(crate) async fn run(app: AppState) {
    let mut stop = app.shutdown.subscribe();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = stop.changed() => return,
        }
        if !app.orchestrator.routed() {
            continue;
        }
        let schedule = app.schedule.clone();
        let now = chrono::Utc::now().timestamp();
        let due = tokio::task::spawn_blocking(move || schedule.due(now)).await.unwrap_or_default();
        for t in due {
            app.send_command(crate::BridgeCommand::Orchestrator(crate::OrchestratorCommand::Scheduled {
                id: t.id.clone(),
                name: t.name.clone(),
                prompt: t.prompt.clone(),
                persist: t.persist,
            }));
        }
    }
}

// ---------------------------------------------------------------------------- parsing (the tools' arguments)

/// "90s", "10m", "2h", "1d", "1h30m"; a bare number is minutes.
pub fn parse_duration(text: &str) -> Option<u64> {
    let s = text.trim().to_lowercase().replace(' ', "");
    if let Ok(n) = s.parse::<u64>() {
        return Some(n * 60);
    }
    let mut total = 0u64;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        total += n * match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return None,
        };
    }
    (num.is_empty() && total > 0).then_some(total)
}

fn duration_text(secs: u64) -> String {
    match secs {
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// "17:30" (the next one), "2026-10-20 09:00" (local), or RFC 3339.
pub fn parse_at(text: &str, now: i64) -> Option<i64> {
    let s = text.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp());
    }
    for f in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Local.from_local_datetime(&t).earliest().map(|t| t.timestamp());
        }
    }
    let time = parse_clock(s)?;
    When::Daily { time, days: vec![] }.next_after(now)
}

/// "9:05", "09:05", "9am", "5:30pm" → "HH:MM".
pub fn parse_clock(text: &str) -> Option<String> {
    let s = text.trim().to_lowercase().replace(' ', "");
    let (s, pm) = match (s.strip_suffix("pm"), s.strip_suffix("am")) {
        (Some(x), _) => (x.to_string(), Some(true)),
        (_, Some(x)) => (x.to_string(), Some(false)),
        _ => (s, None),
    };
    let (h, m) = match s.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (s.parse::<u32>().ok()?, 0),
    };
    let h = match pm {
        Some(true) if h < 12 => h + 12,
        Some(false) if h == 12 => 0,
        _ => h,
    };
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

fn parse_days(v: &Value) -> Result<Vec<u8>, String> {
    let items: Vec<String> = match v {
        Value::Null => return Ok(vec![]),
        Value::String(s) => s.split([',', ' ']).filter(|x| !x.is_empty()).map(str::to_string).collect(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => return Err("days: a list like [\"mon\", \"fri\"]".into()),
    };
    let mut out = vec![];
    for d in items {
        let d = d.trim().to_lowercase();
        match d.as_str() {
            "weekdays" => out.extend(0..5),
            "weekends" => out.extend(5..7),
            _ => out.push(WEEKDAYS.iter().position(|w| d.starts_with(w)).ok_or_else(|| format!("unknown day \"{d}\""))? as u8),
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// A trigger from `schedule_add`'s arguments: `prompt`, `name`, `persist`, and one of `in`, `at`, `every`, `daily`
/// (with `days`).
pub fn trigger_from_args(args: &Value, now: i64) -> Result<Trigger, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    let prompt = s("prompt").ok_or("prompt is required: what to do when it fires.")?.to_string();
    let when = if let Some(v) = s("in") {
        When::Once { at: now + parse_duration(v).ok_or_else(|| format!("in: \"{v}\" isn't a duration like 10m or 2h"))? as i64 }
    } else if let Some(v) = s("every") {
        let secs = parse_duration(v).ok_or_else(|| format!("every: \"{v}\" isn't a duration like 30m or 1d"))?;
        if secs < 60 {
            return Err("every: at least a minute apart.".into());
        }
        let anchor = match s("at") {
            Some(at) => parse_at(at, now).ok_or_else(|| format!("at: \"{at}\" isn't a time"))?,
            None => now + secs as i64,
        };
        When::Every { secs, anchor }
    } else if let Some(v) = s("daily") {
        When::Daily { time: parse_clock(v).ok_or_else(|| format!("daily: \"{v}\" isn't a time like 09:00"))?, days: parse_days(args.get("days").unwrap_or(&Value::Null))? }
    } else if let Some(v) = s("at") {
        When::Once { at: parse_at(v, now).ok_or_else(|| format!("at: \"{v}\" isn't a time like 17:30 or 2026-10-20 09:00"))? }
    } else {
        return Err("Say when: in (\"10m\"), at (\"17:30\"), every (\"30m\") or daily (\"09:00\").".into());
    };
    let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let name = s("name").map(str::to_string).unwrap_or_else(|| prompt.chars().take(40).collect());
    Ok(Trigger {
        id,
        name,
        prompt,
        when,
        persist: args.get("persist").and_then(Value::as_bool).unwrap_or(false),
        created_at: now,
        next_run: None,
        last_run: None,
        runs: 0,
    })
}

fn local(at: i64) -> String {
    Local.timestamp_opt(at, 0).single().map(|t| t.format("%a %Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every(secs: u64, anchor: i64, persist: bool, id: &str) -> Trigger {
        Trigger { id: id.into(), name: id.into(), prompt: "check".into(), when: When::Every { secs, anchor }, persist, created_at: 0, next_run: None, last_run: None, runs: 0 }
    }

    #[test]
    fn occurrences() {
        assert_eq!(When::Once { at: 100 }.next_after(50), Some(100));
        assert_eq!(When::Once { at: 100 }.next_after(100), None);
        let e = When::Every { secs: 60, anchor: 1000 };
        assert_eq!(e.next_after(10), Some(1000));
        assert_eq!(e.next_after(1000), Some(1060));
        assert_eq!(e.next_after(1059), Some(1060));
        let d = When::Daily { time: "09:30".into(), days: vec![] };
        let next = d.next_after(chrono::Utc::now().timestamp()).unwrap();
        let t = Local.timestamp_opt(next, 0).unwrap();
        assert_eq!(t.format("%H:%M").to_string(), "09:30");
        let weekend = When::Daily { time: "08:00".into(), days: vec![5, 6] };
        let n = Local.timestamp_opt(weekend.next_after(chrono::Utc::now().timestamp()).unwrap(), 0).unwrap();
        assert!(n.weekday().num_days_from_monday() >= 5);
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("15"), Some(900));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_clock("5:30pm").as_deref(), Some("17:30"));
        assert_eq!(parse_clock("12am").as_deref(), Some("00:00"));
        assert_eq!(parse_clock("9").as_deref(), Some("09:00"));
        assert_eq!(parse_clock("25:00"), None);
        let now = chrono::Utc::now().timestamp();
        let t = trigger_from_args(&json!({ "prompt": "status", "every": "30m" }), now).unwrap();
        assert_eq!(t.when, When::Every { secs: 1800, anchor: now + 1800 });
        assert!(!t.persist);
        let t = trigger_from_args(&json!({ "prompt": "standup", "daily": "9am", "days": "weekdays", "persist": true }), now).unwrap();
        assert_eq!(t.when, When::Daily { time: "09:00".into(), days: vec![0, 1, 2, 3, 4] });
        assert!(t.persist && t.when.describe() == "mon, tue, wed, thu, fri at 09:00");
        assert!(trigger_from_args(&json!({ "prompt": "x" }), now).is_err());
        assert!(trigger_from_args(&json!({ "prompt": "x", "every": "10s" }), now).is_err());
        assert_eq!(trigger_from_args(&json!({ "prompt": "x", "in": "10m" }), now).unwrap().when, When::Once { at: now + 600 });
    }

    #[test]
    fn session_triggers_fire_in_their_window_then_move_on() {
        let dir = tempfile::tempdir().unwrap();
        let s = Schedule::new(dir.path());
        s.add(every(600, 1000, false, "a"), 900).unwrap();
        assert!(s.due(999).is_empty());
        assert_eq!(s.due(1030).len(), 1, "30 s late: inside the window");
        assert!(s.due(1040).is_empty(), "fired once");
        assert_eq!(s.list()[0].next_run, Some(1600));
        // bro was away: an occurrence more than 60 s late passes
        assert!(s.due(1700).is_empty());
        assert_eq!(s.list()[0].next_run, Some(2200));
        // a one-off goes once it ran
        let once = Trigger { when: When::Once { at: 2000 }, ..every(1, 0, false, "b") };
        s.add(once, 1900).unwrap();
        assert_eq!(s.due(2001).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(s.list().len(), 1);
        s.clear_session();
        assert!(s.list().is_empty());
    }

    #[test]
    fn persistent_triggers_fire_on_exactly_one_bro() {
        let dir = tempfile::tempdir().unwrap();
        // two bros sharing one data root
        let (a, b) = (Schedule::new(dir.path()), Schedule::new(dir.path()));
        a.add(every(3600, 10_000, true, "p"), 9_000).unwrap();
        assert_eq!(b.list().len(), 1, "both see it");
        let fired_a = a.due(10_020);
        let fired_b = b.due(10_030);
        assert_eq!(fired_a.len() + fired_b.len(), 1, "one bro runs it, the other lets it pass");
        assert_eq!(b.list()[0].next_run, Some(13_600));
        assert_eq!(b.list()[0].runs, 1);
        // even with the claim marker gone, a run inside the window counts as done
        std::fs::remove_dir_all(dir.path().join(CLAIMS)).unwrap();
        let mut list = a.load();
        list[0].next_run = Some(10_000);
        a.save(&list).unwrap();
        assert!(b.due(10_040).is_empty());
        // removal by id prefix or name
        assert_eq!(a.remove("p").unwrap().id, "p");
        assert!(b.list().is_empty());
        assert!(a.remove("nope").is_err());
    }
}
