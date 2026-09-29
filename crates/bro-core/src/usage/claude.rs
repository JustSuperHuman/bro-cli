//! Claude subscription meters: `GET https://api.anthropic.com/api/oauth/usage`
//! (v1 `claude-usage.js`).
use super::{Usage, Window, history};
use crate::creds;
use crate::util::{now_secs, num_of, parse_iso_secs, str_of};
use anyhow::bail;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Claude's OAuth usage endpoint.
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TIMEOUT: Duration = Duration::from_secs(8);

fn window(v: Option<&Value>, mins: u32) -> Option<Window> {
    let v = v?;
    let used = num_of(v.get("utilization"))?;
    Some(Window {
        used_pct: used as f32,
        resets_at: str_of(v, "resets_at").and_then(parse_iso_secs),
        window_mins: Some(mins),
    })
}

/// Parse the usage payload: `five_hour`, `seven_day` (`utilization`, `resets_at`) and
/// `limits[]` entries of kind `weekly_scoped` (model display name → `percent`).
pub fn parse(payload: &Value) -> Usage {
    let mut usage = Usage {
        five_hour: window(payload.get("five_hour"), 300),
        weekly: window(payload.get("seven_day"), 7 * 24 * 60),
        fetched_at: now_secs(),
        ..Default::default()
    };
    for limit in payload.get("limits").and_then(Value::as_array).into_iter().flatten() {
        if str_of(limit, "kind") != Some("weekly_scoped") {
            continue;
        }
        let Some(name) = limit.pointer("/scope/model/display_name").and_then(Value::as_str) else { continue };
        let Some(pct) = num_of(limit.get("percent")).or_else(|| num_of(limit.get("utilization"))) else { continue };
        usage.scoped.push((
            name.to_string(),
            Window {
                used_pct: pct as f32,
                resets_at: str_of(limit, "resets_at").and_then(parse_iso_secs),
                window_mins: Some(7 * 24 * 60),
            },
        ));
    }
    usage
}

/// Where a config dir's readings are kept in usage-history (v1 `claudeHistoryKey`).
pub fn history_key(config_dir: &Path, identity: Option<&str>) -> String {
    match identity {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => format!("claude:{}", crate::util::normalize_path(config_dir).display()),
    }
}

/// Fetch a Claude login's meters (refreshing the token when expired, and once more
/// after a 401), and record the reading.
pub fn fetch(config_dir: &Path) -> anyhow::Result<Usage> {
    let request = |token: &str| {
        let bearer = format!("Bearer {token}");
        crate::http::get_json(
            USAGE_URL,
            &[("authorization", &bearer), ("anthropic-version", "2023-06-01"), ("anthropic-beta", "oauth-2025-04-20")],
            TIMEOUT,
        )
    };
    let mut resp = request(&creds::claude_access_token(config_dir, false)?)?;
    if resp.status == 401 {
        resp = request(&creds::claude_access_token(config_dir, true)?)?;
    }
    if !resp.ok() {
        bail!("usage request failed ({})", resp.status);
    }
    let login = creds::claude_login(config_dir);
    let mut usage = parse(&resp.body);
    usage.plan = login.plan.clone();
    if let (Some(s), Some(w)) = (&usage.five_hour, &usage.weekly) {
        let fable = usage.scoped.iter().find(|(n, _)| n == "Fable").map(|(_, w)| w.used_pct as f64);
        let reading = history::Reading {
            at: 0,
            session: s.used_pct as f64,
            weekly: w.used_pct as f64,
            fable,
            session_resets_at: resp.body.pointer("/five_hour/resets_at").and_then(Value::as_str).map(str::to_string),
            weekly_resets_at: resp.body.pointer("/seven_day/resets_at").and_then(Value::as_str).map(str::to_string),
        };
        history::record_reading(&history_key(config_dir, login.identity.as_deref()), &reading, login.tier.as_deref());
    }
    Ok(usage)
}
