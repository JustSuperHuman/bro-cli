//! How much of a week one 5-hour window is worth, measured (v1 `usage-history.js`).
//!
//! Every usage fetch leaves a reading in `~/.bro/usage-history.json` (override
//! `BRO_USAGE_HISTORY`). Within one 5-hour window of one week, the weekly meter climbs
//! a steady fraction of what the 5-hour meter climbs — weekly points per 5-hour point —
//! which turns "what's left this week" into "how much of this window the week can
//! still pay for". The file format is v1's; both versions append to it.
use crate::util::{atomic_write, env_path, now_ms, parse_iso_ms, read_json};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::OnceLock;

const KEEP_MS: i64 = 14 * 24 * 60 * 60 * 1000;
const KEEP_READINGS: usize = 400;

/// Below this much total 5-hour movement there is no estimate at all (v1).
pub const MIN_SESSION_POINTS: f64 = 20.0;

/// `~/.bro/usage-history.json`.
pub fn history_path() -> PathBuf {
    env_path("BRO_USAGE_HISTORY").unwrap_or_else(|| crate::paths::bro_dir().join("usage-history.json"))
}

/// One reading of a login's meters, as passed to [`record_reading`].
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// unix ms; 0 = now
    pub at: i64,
    /// 5-hour used %
    pub session: f64,
    /// weekly used %
    pub weekly: f64,
    /// Fable-scoped weekly used %, if the plan has one
    pub fable: Option<f64>,
    /// ISO timestamps as the API reports them
    pub session_resets_at: Option<String>,
    pub weekly_resets_at: Option<String>,
}

/// A window's reset time as a whole minute (v1 `windowId`).
pub fn window_id(iso: Option<&str>) -> Option<i64> {
    parse_iso_ms(iso?).map(|ms| (ms as f64 / 60000.0).round() as i64)
}

/// A stored reading: `{ at, s, w, f, sr, wr }`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub at: f64,
    pub s: f64,
    pub w: f64,
    pub sr: Option<f64>,
    pub wr: Option<f64>,
}

fn point(v: &Value) -> Option<Point> {
    Some(Point {
        at: v.get("at")?.as_f64()?,
        s: v.get("s")?.as_f64()?,
        w: v.get("w")?.as_f64()?,
        sr: v.get("sr").and_then(Value::as_f64),
        wr: v.get("wr").and_then(Value::as_f64),
    })
}

/// How far the 5-hour and weekly meters climbed together (v1 `meterMovement`):
/// only consecutive readings of the same week and the same 5-hour window count
/// (or a pre-window 0% reading followed by the window that opened).
pub fn meter_movement(readings: &[Point]) -> (f64, f64) {
    let mut sorted = readings.to_vec();
    sorted.sort_by(|a, b| a.at.total_cmp(&b.at));
    let (mut session, mut weekly) = (0.0, 0.0);
    for pair in sorted.windows(2) {
        let (before, after) = (pair[0], pair[1]);
        if before.wr.is_none() || before.wr != after.wr {
            continue;
        }
        let same_window = before.sr == after.sr || (before.sr.is_none() && before.s == 0.0);
        if !same_window || after.sr.is_none() {
            continue;
        }
        let (ds, dw) = (after.s - before.s, after.w - before.w);
        if ds < 0.0 || dw < 0.0 {
            continue;
        }
        session += ds;
        weekly += dw;
    }
    (session, weekly)
}

/// Where a measured ratio came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatioSource {
    /// This account's own readings
    Account,
    /// Pooled over accounts on the same plan tier
    Tier,
    /// Pooled over every account
    Plans,
}

/// Weekly points per 5-hour point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredRatio {
    pub ratio: f64,
    pub session_points: f64,
    pub source: RatioSource,
}

fn ratio_from((session, weekly): (f64, f64)) -> Option<(f64, f64)> {
    (session >= MIN_SESSION_POINTS).then(|| (weekly / session, session))
}

struct Loaded {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    data: Value,
}

fn loaded() -> &'static Mutex<Option<Loaded>> {
    static CACHE: OnceLock<Mutex<Option<Loaded>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Current history document (cached until the file's mtime changes).
fn with_data<R>(f: impl FnOnce(&mut Value, &PathBuf) -> R) -> R {
    let path = history_path();
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut guard = loaded().lock();
    let stale = guard.as_ref().is_none_or(|l| l.path != path || l.modified != modified);
    if stale {
        let data = read_json(&path)
            .filter(|d| d.get("logins").is_some_and(Value::is_object))
            .unwrap_or_else(|| json!({ "version": 1, "logins": {} }));
        *guard = Some(Loaded { path: path.clone(), modified, data });
    }
    let l = guard.as_mut().expect("just set");
    let out = f(&mut l.data, &path);
    l.modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    out
}

/// Remember one reading of a login's meters (v1 `recordReading`). `key` names the
/// account (identity where known); `tier` is its plan tier, for pooling. Unchanged
/// meters add nothing. Write failures are ignored — the history is a refinement.
pub fn record_reading(key: &str, reading: &Reading, tier: Option<&str>) {
    if key.is_empty() || !reading.session.is_finite() || !reading.weekly.is_finite() {
        return;
    }
    with_data(|data, path| {
        let at = if reading.at > 0 { reading.at } else { now_ms() };
        let Some(logins) = data.get_mut("logins").and_then(Value::as_object_mut) else { return };
        let login = logins.entry(key.to_string()).or_insert_with(|| json!({ "tier": tier, "readings": [] }));
        if !login.is_object() {
            *login = json!({ "tier": tier, "readings": [] });
        }
        let Some(obj) = login.as_object_mut() else { return };
        if let Some(t) = tier {
            obj.insert("tier".into(), json!(t));
        }
        let new = json!({
            "at": at,
            "s": reading.session,
            "w": reading.weekly,
            "f": reading.fable,
            "sr": window_id(reading.session_resets_at.as_deref()),
            "wr": window_id(reading.weekly_resets_at.as_deref()),
        });
        let mut readings: Vec<Value> = obj.get("readings").and_then(Value::as_array).cloned().unwrap_or_default();
        let same = |a: &Value, b: &Value| ["s", "w", "f", "sr", "wr"].iter().all(|k| num_eq(a.get(*k), b.get(*k)));
        if readings.last().is_some_and(|last| same(last, &new)) {
            return;
        }
        readings.push(new);
        readings.retain(|r| r.get("at").and_then(Value::as_f64).is_some_and(|t| (at as f64) - t <= KEEP_MS as f64));
        if readings.len() > KEEP_READINGS {
            readings.drain(..readings.len() - KEEP_READINGS);
        }
        obj.insert("readings".into(), Value::Array(readings));
        if let Ok(text) = serde_json::to_string(data) {
            let _ = atomic_write(path, text.as_bytes());
        }
    });
}

fn num_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let n = |v: Option<&Value>| v.and_then(Value::as_f64);
    match (n(a), n(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => true,
        _ => false,
    }
}

fn points(login: &Value) -> Vec<Point> {
    login.get("readings").and_then(Value::as_array).map(|a| a.iter().filter_map(point).collect()).unwrap_or_default()
}

fn pooled(logins: &Map<String, Value>, include: impl Fn(&Value) -> bool) -> (f64, f64) {
    logins.values().filter(|l| include(l)).map(|l| meter_movement(&points(l))).fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1))
}

/// The measured ratio for a login (v1 `measuredRatio`): its own when it has moved
/// enough; else every login on its plan tier; else every login.
pub fn measured_ratio(key: &str, tier: Option<&str>) -> Option<MeasuredRatio> {
    with_data(|data, _| {
        let logins = data.get("logins").and_then(Value::as_object)?;
        let make = |(ratio, session_points), source| MeasuredRatio { ratio, session_points, source };
        if let Some(own) = logins.get(key).and_then(|l| ratio_from(meter_movement(&points(l)))) {
            return Some(make(own, RatioSource::Account));
        }
        if let Some(t) = tier
            && let Some(r) = ratio_from(pooled(logins, |l| l.get("tier").and_then(Value::as_str) == Some(t)))
        {
            return Some(make(r, RatioSource::Tier));
        }
        ratio_from(pooled(logins, |_| true)).map(|r| make(r, RatioSource::Plans))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn p(at: f64, s: f64, w: f64, sr: Option<f64>, wr: Option<f64>) -> Point {
        Point { at, s, w, sr, wr }
    }

    #[test]
    fn movement_counts_only_same_windows() {
        let r = [
            p(1.0, 0.0, 10.0, None, Some(100.0)),
            p(2.0, 10.0, 12.0, Some(5.0), Some(100.0)), // window opened: counts
            p(3.0, 30.0, 16.0, Some(5.0), Some(100.0)), // same window
            p(4.0, 5.0, 17.0, Some(6.0), Some(100.0)),  // new window: skipped
            p(5.0, 9.0, 18.0, Some(6.0), Some(200.0)),  // new week: skipped
            p(6.0, 8.0, 18.0, Some(6.0), Some(200.0)),  // went down: skipped
        ];
        assert_eq!(meter_movement(&r), (30.0, 6.0));
        assert_eq!(ratio_from((30.0, 6.0)), Some((0.2, 30.0)));
        assert_eq!(ratio_from((19.0, 6.0)), None);
    }

    #[test]
    fn records_and_measures() {
        let _sb = sandbox();
        let wr = Some("2026-10-01T00:00:00Z".to_string());
        let sr = Some("2026-09-29T15:00:00Z".to_string());
        let reading = |at, s, w| Reading { at, session: s, weekly: w, fable: None, session_resets_at: sr.clone(), weekly_resets_at: wr.clone() };
        let now = now_ms();
        record_reading("acct", &reading(now - 3000, 10.0, 20.0), Some("max20"));
        record_reading("acct", &reading(now - 2000, 10.0, 20.0), Some("max20")); // unchanged: dropped
        record_reading("acct", &reading(now - 1000, 40.0, 26.0), Some("max20"));
        let doc = read_json(&history_path()).unwrap();
        assert_eq!(doc["logins"]["acct"]["readings"].as_array().unwrap().len(), 2);
        assert_eq!(doc["logins"]["acct"]["tier"], "max20");
        let m = measured_ratio("acct", Some("max20")).unwrap();
        assert!((m.ratio - 0.2).abs() < 1e-9);
        assert_eq!(m.source, RatioSource::Account);
        let other = measured_ratio("new-acct", Some("max20")).unwrap();
        assert_eq!(other.source, RatioSource::Tier);
        assert_eq!(measured_ratio("new-acct", Some("pro")).unwrap().source, RatioSource::Plans);
        assert_eq!(window_id(Some("1970-01-01T00:01:00Z")), Some(1));
    }
}
