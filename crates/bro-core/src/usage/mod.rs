//! Live subscription meters (v1 claude-usage.js / codex-usage.js / usage.js /
//! usage-history.js / task-size.js).
pub mod claude;
pub mod codex;
pub mod history;

use crate::profiles::Profile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    /// 0..=100
    pub used_pct: f32,
    /// unix seconds
    pub resets_at: Option<i64>,
    pub window_mins: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub five_hour: Option<Window>,
    pub weekly: Option<Window>,
    /// Model-scoped weekly limits, e.g. ("Fable", window)
    pub scoped: Vec<(String, Window)>,
    pub plan: Option<String>,
    pub credits: Option<f64>,
    /// unix seconds when fetched
    pub fetched_at: i64,
}

impl Usage {
    /// The Fable-scoped weekly meter, on plans that have one.
    pub fn fable(&self) -> Option<&Window> {
        self.scoped.iter().find(|(n, _)| n == "Fable").map(|(_, w)| w)
    }
}

/// Blocking fetch (Claude OAuth usage endpoint / `codex app-server` rateLimits).
/// Records a reading into `~/.bro/usage-history.json`.
pub fn fetch(profile: &crate::profiles::Profile) -> anyhow::Result<Usage> {
    let mut usage = if profile.is_claude() { claude::fetch(&profile.dir)? } else { codex::fetch(&profile.dir)? };
    if usage.plan.is_none() {
        usage.plan = profile.plan.clone();
    }
    Ok(usage)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Headroom {
    /// % of the 5h window realistically usable now (capped by weekly/ratio)
    pub now: f32,
    pub week: f32,
    /// true when `now` was capped by the measured ratio (render with "≈")
    pub capped: bool,
}

/// Fable's allowance: "up to 50% of your weekly usage limits" (v1 `FABLE_SHARE`).
pub const FABLE_SHARE: f64 = 0.5;
/// Below this % left in either window a login can't be relied on (v1 `READY_FLOOR`).
pub const READY_FLOOR: f64 = 5.0;

/// Every window's headroom as % left of its own allowance (v1 `claudeHeadroom` /
/// `codexHeadroom`). `None` = the login doesn't report that window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HeadroomDetail {
    pub h5: Option<f64>,
    pub wk: Option<f64>,
    pub fable_5h: Option<f64>,
    pub fable_wk: Option<f64>,
    /// A weekly-points-per-5h-point ratio was available
    pub measured: bool,
    /// `h5` was cut down by the ratio
    pub h5_estimated: bool,
    pub fable_5h_estimated: bool,
}

fn left(used: Option<f64>) -> Option<f64> {
    used.filter(|u| u.is_finite()).map(|u| (100.0 - u).clamp(0.0, 100.0))
}

fn tighter(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => Some(a.min(b)),
    }
}

/// (value, estimated) — v1 `windowLeft`.
fn window_left(session: Option<f64>, week_left: Option<f64>, affordable: Option<f64>) -> (Option<f64>, bool) {
    let Some(session) = session else { return (None, false) };
    if week_left == Some(0.0) {
        return (Some(0.0), false);
    }
    match affordable {
        Some(a) if a < session => (Some(a), true),
        _ => (Some(session), false),
    }
}

/// v1 `claudeHeadroom`: with `ratio` (weekly points per 5h point), each 5h figure is
/// what the window has left *and the week can still pay for*.
pub fn claude_headroom(usage: &Usage, ratio: Option<f64>) -> HeadroomDetail {
    let ratio = ratio.filter(|r| *r > 0.0);
    let wk = left(usage.weekly.as_ref().map(|w| w.used_pct as f64));
    let session = left(usage.five_hour.as_ref().map(|w| w.used_pct as f64));
    let affordable = |weekly_points: Option<f64>| Some(weekly_points? / ratio?);
    let fable_own = left(usage.fable().map(|w| w.used_pct as f64));
    let fable_wk = fable_own.map(|f| tighter(Some(f), wk.map(|w| (w / FABLE_SHARE).min(100.0))).unwrap_or(f));
    let (h5, h5_estimated) = window_left(session, wk, affordable(wk));
    let (fable_5h, fable_5h_estimated) = match fable_wk {
        None => (None, false),
        Some(fw) => window_left(session, Some(fw), affordable(Some(fw * FABLE_SHARE))),
    };
    HeadroomDetail { h5, wk, fable_5h, fable_wk, measured: ratio.is_some(), h5_estimated, fable_5h_estimated }
}

/// v1 `codexHeadroom`: an exhausted week closes the 5h window too.
pub fn codex_headroom(usage: &Usage) -> HeadroomDetail {
    let wk = left(usage.weekly.as_ref().map(|w| w.used_pct as f64));
    let h5 = left(usage.five_hour.as_ref().map(|w| w.used_pct as f64)).map(|h| if wk == Some(0.0) { 0.0 } else { h });
    HeadroomDetail { h5, wk, ..Default::default() }
}

/// The measured 5h↔week ratio for a Claude profile (its own history, else its tier's,
/// else every plan's). Codex keeps no history → `None`.
pub fn ratio_for(profile: &Profile) -> Option<f64> {
    if !profile.is_claude() {
        return None;
    }
    let key = claude::history_key(&profile.dir, profile.identity.as_deref());
    history::measured_ratio(&key, profile.tier.as_deref()).map(|m| m.ratio)
}

/// Per-window headroom for any profile.
pub fn headroom_detail(profile: &Profile, usage: &Usage) -> HeadroomDetail {
    if profile.is_claude() { claude_headroom(usage, ratio_for(profile)) } else { codex_headroom(usage) }
}

/// v1 `claudeHeadroom` with the measured 5h↔week ratio from usage-history.
///
/// A missing window borrows the other one (Codex Pro reports only a weekly window);
/// with neither, both read 100 (unknown ≠ exhausted).
pub fn headroom(profile: &crate::profiles::Profile, usage: &Usage) -> Headroom {
    let d = headroom_detail(profile, usage);
    let now = d.h5.or(d.wk).unwrap_or(100.0);
    let week = d.wk.or(d.h5).unwrap_or(100.0);
    Headroom { now: now as f32, week: week as f32, capped: d.h5_estimated }
}

/// What a login can still deliver as (week, now) in weekly points (v1 `capacityOf`).
pub fn capacity_of(room: &HeadroomDetail, ratio: Option<f64>) -> Option<(f64, f64)> {
    match (room.wk, room.h5) {
        (None, None) => None,
        (None, Some(w)) => Some((w, w)),
        (Some(wk), None) => Some((wk, wk)),
        (Some(wk), Some(h5)) => {
            let now = match ratio.filter(|r| *r > 0.0) {
                Some(r) => h5 * r,
                None => h5,
            };
            Some((wk, now.min(wk)))
        }
    }
}

/// Every meter the login reports is above [`READY_FLOOR`] (v1 `isReady`).
pub fn is_ready(room: &HeadroomDetail) -> bool {
    [room.wk, room.h5].into_iter().flatten().all(|v| v >= READY_FLOOR)
}

/// v1 `--large-task` / `--small-task` pick among ready profiles of one family.
///
/// The week ranks, the 5h window breaks ties, the id settles it; only ready logins
/// are considered unless none is. Returns the chosen profile id, or `None` when no
/// candidate reported any window.
pub fn pick_by_size(candidates: &[(crate::profiles::Profile, Usage)], large: bool) -> Option<String> {
    struct Rated<'a> {
        id: &'a str,
        week: f64,
        now: f64,
        ready: bool,
    }
    let rated: Vec<Rated> = candidates
        .iter()
        .filter_map(|(p, u)| {
            let ratio = ratio_for(p);
            let room = if p.is_claude() { claude_headroom(u, ratio) } else { codex_headroom(u) };
            let (week, now) = capacity_of(&room, ratio)?;
            Some(Rated { id: &p.id, week, now, ready: is_ready(&room) })
        })
        .collect();
    let any_ready = rated.iter().any(|r| r.ready);
    let dir = if large { -1.0 } else { 1.0 };
    rated
        .into_iter()
        .filter(|r| !any_ready || r.ready)
        .min_by(|a, b| {
            (dir * (a.week - b.week))
                .partial_cmp(&0.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| (dir * (a.now - b.now)).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.id.cmp(b.id))
        })
        .map(|r| r.id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{ProfileKind, describe};
    use crate::util::test_env::sandbox;
    use serde_json::json;

    fn w(used: f32) -> Option<Window> {
        Some(Window { used_pct: used, resets_at: None, window_mins: None })
    }

    fn usage(h5: Option<f32>, wk: Option<f32>, fable: Option<f32>) -> Usage {
        let mut u = Usage { five_hour: h5.and_then(w), weekly: wk.and_then(w), ..Default::default() };
        if let Some(f) = fable {
            u.scoped.push(("Fable".into(), w(f).unwrap()));
        }
        u
    }

    #[test]
    fn claude_usage_payload_parses() {
        let payload = json!({
            "five_hour": { "utilization": 17.0, "resets_at": "2026-09-29T15:00:00.123456+00:00" },
            "seven_day": { "utilization": 28, "resets_at": "2026-10-03T00:00:00Z" },
            "limits": [
                { "kind": "weekly_scoped", "percent": 53, "scope": { "model": { "display_name": "Fable" } } },
                { "kind": "other", "percent": 1 }
            ]
        });
        let u = claude::parse(&payload);
        assert_eq!(u.five_hour.as_ref().unwrap().used_pct, 17.0);
        assert_eq!(u.five_hour.as_ref().unwrap().window_mins, Some(300));
        assert!(u.five_hour.as_ref().unwrap().resets_at.is_some());
        assert_eq!(u.weekly.as_ref().unwrap().used_pct, 28.0);
        assert_eq!(u.scoped.len(), 1);
        assert_eq!(u.fable().unwrap().used_pct, 53.0);
        let empty = claude::parse(&json!({}));
        assert!(empty.five_hour.is_none() && empty.weekly.is_none());
    }

    #[test]
    fn claude_headroom_math() {
        // No ratio: 5h is what the window has left.
        let d = claude_headroom(&usage(Some(5.0), Some(99.0), None), None);
        assert_eq!((d.h5, d.wk, d.h5_estimated), (Some(95.0), Some(1.0), false));
        // Ratio 0.2 weekly points per 5h point: 1 weekly point pays for 5 of the window.
        let d = claude_headroom(&usage(Some(5.0), Some(99.0), None), Some(0.2));
        assert_eq!(d.h5, Some(5.0));
        assert!(d.h5_estimated && d.measured);
        // Exhausted week closes the window.
        let d = claude_headroom(&usage(Some(0.0), Some(100.0), None), None);
        assert_eq!(d.h5, Some(0.0));
        // Fable: own 50% used → 50 left, capped by week 20 left / 0.5 = 40.
        let d = claude_headroom(&usage(Some(10.0), Some(80.0), Some(50.0)), None);
        assert_eq!(d.fable_wk, Some(40.0));
        assert_eq!(d.fable_5h, Some(90.0));
        // With a ratio, fable 5h is bounded by fable_wk * share / ratio = 40*0.5/0.5 = 40.
        let d = claude_headroom(&usage(Some(10.0), Some(80.0), Some(50.0)), Some(0.5));
        assert_eq!(d.fable_5h, Some(40.0));
        assert!(d.fable_5h_estimated);
        assert_eq!(claude_headroom(&usage(None, None, None), None), HeadroomDetail::default());
    }

    #[test]
    fn codex_headroom_and_capacity() {
        let d = codex_headroom(&usage(Some(10.0), Some(100.0), None));
        assert_eq!((d.h5, d.wk), (Some(0.0), Some(0.0)));
        let d = codex_headroom(&usage(None, Some(30.0), None));
        assert_eq!(capacity_of(&d, None), Some((70.0, 70.0)));
        let room = HeadroomDetail { h5: Some(50.0), wk: Some(20.0), ..Default::default() };
        assert_eq!(capacity_of(&room, Some(0.2)), Some((20.0, 10.0)));
        assert_eq!(capacity_of(&room, None), Some((20.0, 20.0)));
        assert!(is_ready(&room));
        assert!(!is_ready(&HeadroomDetail { h5: Some(4.0), wk: Some(90.0), ..Default::default() }));
        assert_eq!(capacity_of(&HeadroomDetail::default(), None), None);
    }

    #[test]
    fn headroom_fallbacks() {
        let _sb = sandbox();
        let p = describe(ProfileKind::CodexProfile, "x", std::env::temp_dir().join("bro-none"));
        let h = headroom(&p, &usage(None, Some(40.0), None));
        assert_eq!((h.now, h.week, h.capped), (60.0, 60.0, false));
        let h = headroom(&p, &Usage::default());
        assert_eq!((h.now, h.week), (100.0, 100.0));
    }

    #[test]
    fn picks_by_size() {
        let _sb = sandbox();
        let mk = |name: &str| describe(ProfileKind::CodexProfile, name, std::env::temp_dir().join(format!("bro-none-{name}")));
        let cands = vec![
            (mk("roomy"), usage(Some(0.0), Some(10.0), None)),     // wk 90
            (mk("nearly"), usage(Some(0.0), Some(80.0), None)),    // wk 20
            (mk("spent5h"), usage(Some(99.0), Some(0.0), None)),   // not ready
            (mk("unknown"), usage(None, None, None)),              // no capacity
        ];
        assert_eq!(pick_by_size(&cands, true).as_deref(), Some("codex:roomy"));
        assert_eq!(pick_by_size(&cands, false).as_deref(), Some("codex:nearly"));
        // Tie on week → 5h breaks it; then name.
        let tie = vec![(mk("b"), usage(Some(50.0), Some(50.0), None)), (mk("a"), usage(Some(10.0), Some(50.0), None))];
        assert_eq!(pick_by_size(&tie, true).as_deref(), Some("codex:a"));
        let same = vec![(mk("b"), usage(Some(10.0), Some(50.0), None)), (mk("a"), usage(Some(10.0), Some(50.0), None))];
        assert_eq!(pick_by_size(&same, false).as_deref(), Some("codex:a"));
        // Nobody ready → rank the whole field.
        let spent = vec![(mk("x"), usage(Some(99.0), Some(10.0), None)), (mk("y"), usage(Some(98.0), Some(20.0), None))];
        assert_eq!(pick_by_size(&spent, true).as_deref(), Some("codex:x"));
        assert_eq!(pick_by_size(&[], true), None);
    }

    #[test]
    #[ignore = "network + real home"]
    fn fetch_real_usage() {
        let now = crate::util::now_ms();
        for p in crate::profiles::list().into_iter().filter(|p| p.authenticated) {
            // Only logins whose token is still fresh, so this never rotates credentials.
            let fresh = !p.is_claude() || crate::creds::claude_login(&p.dir).expires_at.is_some_and(|e| e > now + 120_000);
            if fresh {
                let u = fetch(&p);
                let h = u.as_ref().ok().map(|u| headroom(&p, u));
                println!("{} {:?} {:?}", p.id, u.map(|u| (u.five_hour, u.weekly, u.scoped, u.plan)), h);
            }
        }
    }
}
