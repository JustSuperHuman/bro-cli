//! Process-wide cache of each login's latest meter reading, with the throttle and
//! backoff that keep bro from polling the usage endpoints too often.
//!
//! [`super::fetch`] is the only thing that touches the network; it consults and
//! feeds this cache. Everything else (the TUI, the proxy's Claude pool, the
//! `/usage` routes) reads [`snapshot`] / [`for_config_dir`] / [`report`] and
//! never triggers a request.
use super::{AppSummary, LoginUsage, Usage, Window};
use crate::profiles::Profile;
use crate::util::now_secs;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// Background refresh: a login is re-read at most this often.
pub const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);
/// An explicit refresh (user action) still waits this long between reads of one login.
pub const MANUAL_MIN_AGE: Duration = Duration::from_secs(30);
/// First backoff after a failure; doubles per consecutive failure up to [`BACKOFF_MAX`].
const BACKOFF_START: Duration = Duration::from_secs(2 * 60);
const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);
/// A reading older than this is reported as stale.
const STALE_AFTER: i64 = 15 * 60;

/// The endpoint said 429: `retry_after` seconds, when it told us.
#[derive(Debug, Clone, Copy)]
pub struct RateLimited {
    pub retry_after: Option<u64>,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.retry_after {
            Some(s) => write!(f, "usage endpoint rate limited (retry after {s}s)"),
            None => write!(f, "usage endpoint rate limited"),
        }
    }
}

impl std::error::Error for RateLimited {}

/// One login's cached state.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub profile: Profile,
    /// Last good reading (kept across later failures)
    pub usage: Option<Usage>,
    /// Error of the most recent attempt, if it failed
    pub error: Option<String>,
    /// unix seconds of the most recent attempt
    pub attempted_at: i64,
    /// unix seconds before which no request is made (backoff after a failure)
    pub retry_at: Option<i64>,
    #[serde(skip)]
    failures: u32,
}

fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Seconds until `profile` may be fetched again because of a backoff, if it is in one.
pub fn backing_off(profile: &Profile) -> Option<i64> {
    let now = now_secs();
    cache().lock().get(&profile.id).and_then(|e| e.retry_at).filter(|t| *t > now).map(|t| t - now)
}

/// Whether `profile` should be fetched now: never read, or last attempt older than
/// `min_age`, and not backing off.
pub fn due(profile: &Profile, min_age: Duration) -> bool {
    let now = now_secs();
    match cache().lock().get(&profile.id) {
        None => true,
        Some(e) => e.retry_at.is_none_or(|t| t <= now) && now - e.attempted_at >= min_age.as_secs() as i64,
    }
}

/// Fold the outcome of a network read into the cache.
pub(crate) fn record(profile: &Profile, result: &anyhow::Result<Usage>) {
    let now = now_secs();
    let mut map = cache().lock();
    let entry = map.entry(profile.id.clone()).or_insert_with(|| Entry {
        profile: profile.clone(),
        usage: None,
        error: None,
        attempted_at: now,
        retry_at: None,
        failures: 0,
    });
    entry.profile = profile.clone();
    entry.attempted_at = now;
    match result {
        Ok(u) => {
            entry.usage = Some(u.clone());
            entry.error = None;
            entry.retry_at = None;
            entry.failures = 0;
        }
        Err(e) => {
            entry.failures = entry.failures.saturating_add(1);
            let doubled = BACKOFF_START.as_secs().saturating_mul(1 << (entry.failures - 1).min(8));
            let backoff = match e.downcast_ref::<RateLimited>().and_then(|r| r.retry_after) {
                Some(s) => s.max(BACKOFF_START.as_secs()),
                None => doubled,
            }
            .min(BACKOFF_MAX.as_secs());
            entry.retry_at = Some(now + backoff as i64);
            entry.error = Some(format!("{e:#}"));
        }
    }
}

/// Drop logins that no longer exist.
pub fn retain(ids: &[&str]) {
    cache().lock().retain(|id, _| ids.contains(&id.as_str()));
}

/// Every cached login, sorted by profile id.
pub fn snapshot() -> Vec<Entry> {
    let mut all: Vec<Entry> = cache().lock().values().cloned().collect();
    all.sort_by(|a, b| a.profile.id.cmp(&b.profile.id));
    all
}

/// The last reading of the Claude login in `config_dir` (the proxy's pool keys accounts by dir).
pub fn for_config_dir(config_dir: &Path) -> Option<Usage> {
    let want = crate::util::normalize_path(config_dir);
    cache()
        .lock()
        .values()
        .find(|e| e.profile.is_claude() && crate::util::normalize_path(&e.profile.dir) == want)
        .and_then(|e| e.usage.clone())
}

/// A reading shows the login out of quota right now: a window at 100% that has not reset yet.
pub fn exhausted(usage: &Usage) -> bool {
    let now = now_secs();
    [usage.five_hour.as_ref(), usage.weekly.as_ref()]
        .into_iter()
        .flatten()
        .any(|w| w.used_pct >= 100.0 && w.resets_at.is_none_or(|t| t > now))
}

#[cfg(test)]
pub(crate) fn clear() {
    cache().lock().clear();
}

fn window_json(w: &Window) -> Value {
    json!({ "usedPct": w.used_pct, "resetsAt": w.resets_at, "windowMins": w.window_mins })
}

/// Everything bro knows about subscription usage, as served by the `/usage` routes.
///
/// Blocking (headroom reads the usage history); call off async workers. Makes no
/// network requests.
pub fn report() -> Value {
    let entries = snapshot();
    let now = now_secs();
    let logins: Vec<Value> = entries
        .iter()
        .map(|e| {
            let p = &e.profile;
            let u = e.usage.as_ref();
            json!({
                "id": p.id,
                "app": if p.is_claude() { "claude" } else { "codex" },
                "name": p.name,
                "email": p.email,
                "plan": u.and_then(|u| u.plan.clone()).or_else(|| p.plan.clone()),
                "fiveHour": u.and_then(|u| u.five_hour.as_ref()).map(window_json),
                "weekly": u.and_then(|u| u.weekly.as_ref()).map(window_json),
                "scoped": u.map(|u| u.scoped.iter().map(|(n, w)| {
                    let mut v = window_json(w);
                    v["name"] = json!(n);
                    v
                }).collect::<Vec<_>>()).unwrap_or_default(),
                "credits": u.and_then(|u| u.credits),
                "headroom": u.map(|u| super::headroom_detail(p, u)),
                "exhausted": u.is_some_and(exhausted),
                "fetchedAt": u.map(|u| u.fetched_at),
                "stale": u.is_none_or(|u| now - u.fetched_at > STALE_AFTER),
                "error": e.error,
                "retryAt": e.retry_at.filter(|t| *t > now),
            })
        })
        .collect();
    let summary = |claude: bool| -> AppSummary {
        super::app_summary(entries.iter().filter(|e| e.profile.is_claude() == claude).map(|e| {
            let state = match (&e.usage, &e.error) {
                (Some(u), _) => LoginUsage::Ready(u),
                (None, Some(_)) => LoginUsage::Unavailable,
                (None, None) => LoginUsage::Loading,
            };
            (&e.profile, state)
        }))
    };
    let claude_cands: Vec<(Profile, Usage)> = entries
        .iter()
        .filter(|e| e.profile.is_claude())
        .filter_map(|e| Some((e.profile.clone(), e.usage.clone()?)))
        .collect();
    json!({
        "updatedAt": entries.iter().filter_map(|e| e.usage.as_ref().map(|u| u.fetched_at)).max(),
        "refreshEverySecs": REFRESH_EVERY.as_secs(),
        "claude": summary(true),
        "codex": summary(false),
        "picks": {
            "largeTask": super::pick_by_size(&claude_cands, true),
            "smallTask": super::pick_by_size(&claude_cands, false),
        },
        "logins": logins,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{ProfileKind, describe};
    use crate::util::test_env::sandbox;

    fn reading(h5: f32, resets_at: Option<i64>) -> Usage {
        Usage { five_hour: Some(Window { used_pct: h5, resets_at, window_mins: Some(300) }), fetched_at: now_secs(), ..Default::default() }
    }

    #[test]
    fn throttles_backs_off_and_reports() {
        let _sb = sandbox();
        clear();
        let p = describe(ProfileKind::CodexProfile, "cache-a", std::env::temp_dir().join("bro-cache-a"));
        assert!(due(&p, REFRESH_EVERY));
        record(&p, &Ok(reading(40.0, None)));
        assert!(!due(&p, REFRESH_EVERY), "fresh reading is not refetched");
        assert!(due(&p, Duration::ZERO));
        assert!(backing_off(&p).is_none());

        // 429 with retry-after: back off at least that long, keep the old reading.
        record(&p, &Err(anyhow::Error::new(RateLimited { retry_after: Some(900) })));
        let wait = backing_off(&p).expect("backing off");
        assert!((899..=900).contains(&wait), "{wait}");
        assert!(!due(&p, Duration::ZERO));
        let e = snapshot().into_iter().find(|e| e.profile.id == p.id).unwrap();
        assert_eq!(e.usage.unwrap().five_hour.unwrap().used_pct, 40.0);
        assert!(e.error.unwrap().contains("rate limited"));

        // Plain failures double from the start value.
        record(&p, &Err(anyhow::anyhow!("boom")));
        let wait = backing_off(&p).unwrap();
        assert!(wait > BACKOFF_START.as_secs() as i64, "second failure doubles: {wait}");

        let r = report();
        assert_eq!(r["logins"][0]["id"], p.id);
        assert_eq!(r["logins"][0]["fiveHour"]["usedPct"], 40.0);
        assert_eq!(r["codex"]["logins"], 1);

        retain(&[]);
        assert!(snapshot().is_empty());
    }

    #[test]
    fn exhausted_until_reset() {
        let now = now_secs();
        assert!(exhausted(&reading(100.0, Some(now + 60))));
        assert!(exhausted(&reading(100.0, None)));
        assert!(!exhausted(&reading(100.0, Some(now - 60))), "window already reset");
        assert!(!exhausted(&reading(99.0, None)));
    }
}
