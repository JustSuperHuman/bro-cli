//! Live subscription meters (v1 claude-usage.js / codex-usage.js / usage.js /
//! usage-history.js).
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

/// Blocking fetch (Claude OAuth usage endpoint / `codex app-server` rateLimits).
/// Records a reading into `~/.bro/usage-history.json`.
pub fn fetch(profile: &crate::profiles::Profile) -> anyhow::Result<Usage> { todo!() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Headroom {
    /// % of the 5h window realistically usable now (capped by weekly/ratio)
    pub now: f32,
    pub week: f32,
    /// true when `now` was capped by the measured ratio (render with "≈")
    pub capped: bool,
}
/// v1 `claudeHeadroom` with the measured 5h↔week ratio from usage-history.
pub fn headroom(profile: &crate::profiles::Profile, usage: &Usage) -> Headroom { todo!() }

/// v1 `--large-task` / `--small-task` pick among ready profiles of one family.
pub fn pick_by_size(candidates: &[(crate::profiles::Profile, Usage)], large: bool) -> Option<String> { todo!() }
