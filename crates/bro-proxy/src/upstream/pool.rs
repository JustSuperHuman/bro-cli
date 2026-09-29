//! Claude account pool (port of bro v1 pool: accounts/manager.ts + upstream/anthropic.ts).
//!
//! - pick: sticky per conversation (`metadata.user_id`), else least-loaded (fewest
//!   requests in the rolling 5h window) with round-robin between ties;
//! - an account that rate-limits is cooled down until the upstream's reset time
//!   (default 1h) and the request fails over to the next account — but only if no
//!   bytes have been sent to the client yet (for streams we peek up to the first
//!   real SSE event);
//! - usage counters persist in v1's `usage.json` format (read-modify-write of our
//!   own entries only, atomic replace), so v1 and v2 can share the pool.

use super::oauth;
use super::{ByteStream, UpstreamCall, UpstreamResponse};
use crate::anthropic::Usage;
use crate::errors::{ErrorKind, ProxyError};
use crate::sse::SseParser;
use crate::state::AppState;
use crate::util::now_ms;
use bytes::Bytes;
use futures_util::StreamExt;
use http::HeaderMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const WINDOW_MS: i64 = 5 * 60 * 60 * 1000;
const COOLDOWN_MS: i64 = 60 * 60 * 1000;
const PEEK_LIMIT: usize = 64 * 1024;

/// v1 `AccountUsage` (camelCase on disk).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountUsage {
    pub window_start: i64,
    pub window_requests: u64,
    pub window_input_tokens: u64,
    pub window_output_tokens: u64,
    pub window_cost_usd: f64,
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub last_used_at: Option<i64>,
    pub last_error: Option<String>,
    pub rate_limited_until: Option<i64>,
}

impl AccountUsage {
    fn roll(&mut self, now: i64) {
        if self.window_start == 0 || now - self.window_start >= WINDOW_MS {
            self.window_start = now;
            self.window_requests = 0;
            self.window_input_tokens = 0;
            self.window_output_tokens = 0;
            self.window_cost_usd = 0.0;
        }
    }
    fn cooling(&self, now: i64) -> bool {
        self.rate_limited_until.is_some_and(|t| t > now)
    }
}

/// Account status for the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolAccountStatus {
    pub name: String,
    pub config_dir: PathBuf,
    pub available: bool,
    pub rate_limited_until_ms: Option<i64>,
    pub usage: AccountUsage,
}

/// v1 naming: the account's directory name; the default `~/.claude` login is
/// "claude-code-login".
pub fn pool_account_name(dir: &Path) -> String {
    match dir.file_name().and_then(|n| n.to_str()) {
        Some(".claude") | None => "claude-code-login".into(),
        Some(n) => n.to_string(),
    }
}

pub(crate) struct PoolManager {
    file: PathBuf,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    affinity: HashMap<String, String>,
    rr: usize,
}

impl PoolManager {
    pub fn new(file: PathBuf) -> Self {
        PoolManager {
            file,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn load(&self) -> Value {
        std::fs::read(&self.file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({"usage": {}}))
    }

    fn usage_of(doc: &Value, name: &str, now: i64) -> AccountUsage {
        let mut u: AccountUsage = doc
            .get("usage")
            .and_then(|u| u.get(name))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        u.roll(now);
        u
    }

    /// Read-modify-write one account's entry (other keys preserved), atomically.
    fn update(&self, name: &str, f: impl FnOnce(&mut AccountUsage)) {
        let _g = self.inner.lock(); // serialize writers within this process
        let now = now_ms();
        let mut doc = self.load();
        let mut u = Self::usage_of(&doc, name, now);
        f(&mut u);
        if !doc.get("usage").is_some_and(Value::is_object) {
            doc["usage"] = serde_json::json!({});
        }
        doc["usage"][name] = serde_json::to_value(&u).unwrap_or_default();
        if let Err(e) = atomic_write(
            &self.file,
            &serde_json::to_vec_pretty(&doc).unwrap_or_default(),
        ) {
            tracing::debug!("pool usage write failed (non-fatal): {e}");
        }
    }

    pub fn status(&self, dirs: &[PathBuf]) -> Vec<PoolAccountStatus> {
        let now = now_ms();
        let doc = self.load();
        dirs.iter()
            .map(|d| {
                let name = pool_account_name(d);
                let usage = Self::usage_of(&doc, &name, now);
                PoolAccountStatus {
                    available: !usage.cooling(now),
                    rate_limited_until_ms: usage.rate_limited_until.filter(|t| *t > now),
                    name,
                    config_dir: d.clone(),
                    usage,
                }
            })
            .collect()
    }

    /// v1 `pick`: affinity first, else least-loaded with round-robin ties.
    pub fn pick(
        &self,
        dirs: &[PathBuf],
        session: Option<&str>,
        exclude: &HashSet<String>,
    ) -> Option<(String, PathBuf)> {
        let now = now_ms();
        let doc = self.load();
        let accounts: Vec<(String, PathBuf, AccountUsage)> = dirs
            .iter()
            .map(|d| {
                let n = pool_account_name(d);
                let u = Self::usage_of(&doc, &n, now);
                (n, d.clone(), u)
            })
            .collect();
        let mut inner = self.inner.lock();
        if let Some(s) = session
            && let Some(prior) = inner.affinity.get(s).cloned()
        {
            if !exclude.contains(&prior)
                && let Some((n, d, u)) = accounts.iter().find(|(n, _, _)| *n == prior)
                && !u.cooling(now)
            {
                return Some((n.clone(), d.clone()));
            }
            inner.affinity.remove(s);
        }
        let available: Vec<&(String, PathBuf, AccountUsage)> = accounts
            .iter()
            .filter(|(n, _, u)| !u.cooling(now) && !exclude.contains(n))
            .collect();
        let min = available.iter().map(|(_, _, u)| u.window_requests).min()?;
        let tied: Vec<_> = available
            .iter()
            .filter(|(_, _, u)| u.window_requests == min)
            .collect();
        let chosen = if tied.len() > 1 {
            let c = tied[inner.rr % tied.len()];
            inner.rr = (inner.rr + 1) % tied.len();
            c
        } else {
            tied[0]
        };
        if let Some(s) = session {
            inner.affinity.insert(s.to_string(), chosen.0.clone());
        }
        Some((chosen.0.clone(), chosen.1.clone()))
    }

    pub fn set_affinity(&self, session: &str, account: &str) {
        self.inner
            .lock()
            .affinity
            .insert(session.to_string(), account.to_string());
    }

    pub fn record_success(&self, name: &str, usage: Option<&Usage>) {
        let (i, o) = usage
            .map(|u| (crate::translate::total_input(u), u.output_tokens))
            .unwrap_or((0, 0));
        self.update(name, |u| {
            u.window_requests += 1;
            u.window_input_tokens += i;
            u.window_output_tokens += o;
            u.total_requests += 1;
            u.total_input_tokens += i;
            u.total_output_tokens += o;
            u.last_used_at = Some(now_ms());
            u.last_error = None;
        });
    }

    pub fn record_error(&self, name: &str, message: &str) {
        let msg = crate::util::truncate(message, 500).to_string();
        self.update(name, |u| {
            u.last_error = Some(msg);
            u.last_used_at = Some(now_ms());
        });
    }

    pub fn mark_rate_limited(&self, name: &str, reset_at: Option<i64>) {
        let until = reset_at
            .filter(|t| *t > now_ms())
            .unwrap_or_else(|| now_ms() + COOLDOWN_MS);
        self.update(name, |u| {
            u.rate_limited_until = Some(until);
            u.last_error = Some("rate limited by Anthropic".into());
        });
        self.inner.lock().affinity.retain(|_, v| v != name);
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Rate-limit reset time from response headers (epoch ms).
pub(crate) fn reset_at(headers: &HeaderMap) -> Option<i64> {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::trim);
    if let Some(v) = get("anthropic-ratelimit-unified-reset")
        && let Ok(secs) = v.parse::<i64>()
    {
        return Some(secs * 1000);
    }
    if let Some(v) = get("retry-after") {
        if let Ok(secs) = v.parse::<i64>() {
            return Some(now_ms() + secs * 1000);
        }
        if let Ok(t) = chrono::DateTime::parse_from_rfc2822(v) {
            return Some(t.timestamp_millis());
        }
    }
    for k in [
        "anthropic-ratelimit-requests-reset",
        "anthropic-ratelimit-tokens-reset",
        "anthropic-ratelimit-input-tokens-reset",
        "anthropic-ratelimit-output-tokens-reset",
    ] {
        if let Some(t) = get(k).and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok()) {
            return Some(t.timestamp_millis());
        }
    }
    None
}

enum Attempt {
    Done(UpstreamResponse),
    Retry(ProxyError),
}

pub(crate) async fn send(
    state: &AppState,
    dirs: &[PathBuf],
    call: &UpstreamCall,
) -> Result<UpstreamResponse, ProxyError> {
    let pool = state.pool();
    let session = call.session_key.as_deref();
    let mut tried: HashSet<String> = HashSet::new();
    let no_account = || {
        let msg = if dirs.is_empty() {
            "No Claude accounts configured for this pool"
        } else {
            "All Claude pool accounts are currently unavailable (rate limited)"
        };
        ProxyError::new(ErrorKind::Overloaded, msg).with_status(503)
    };
    let mut next = pool.pick(dirs, session, &tried);
    let mut last: Option<ProxyError> = None;
    while let Some((name, dir)) = next {
        tried.insert(name.clone());
        match try_account(state, &pool, &name, &dir, call).await? {
            Attempt::Done(mut resp) => {
                if let Some(s) = session {
                    pool.set_affinity(s, &name);
                }
                resp.account = Some(name);
                return Ok(resp);
            }
            Attempt::Retry(e) => {
                next = pool.pick(dirs, session, &tried);
                if let Some((to, _)) = &next {
                    tracing::warn!(from = %name, to = %to, reason = %e.message, "pool failover");
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(no_account))
}

async fn try_account(
    state: &AppState,
    pool: &PoolManager,
    name: &str,
    dir: &Path,
    call: &UpstreamCall,
) -> Result<Attempt, ProxyError> {
    let resp = match oauth::attempt(state, dir, call).await {
        Ok(r) => r,
        Err(e) => {
            // Credential or network failure: try another account (v1 semantics).
            pool.record_error(name, &e.message);
            return Ok(Attempt::Retry(e));
        }
    };
    let status = resp.status();
    let headers = resp.headers().clone();
    if !status.is_success() {
        let body = resp.bytes().await.unwrap_or_default();
        let err = ProxyError::from_upstream(status.as_u16(), &headers, &body);
        if err.is_rate_limit() {
            pool.mark_rate_limited(name, reset_at(&headers));
            return Ok(Attempt::Retry(err));
        }
        pool.record_error(name, &err.message);
        return Err(err);
    }
    let is_sse = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.contains("event-stream"));
    let mut body: ByteStream = Box::pin(futures_util::TryStreamExt::map_err(
        resp.bytes_stream(),
        std::io::Error::other,
    ));
    if !(call.stream || is_sse) {
        return Ok(Attempt::Done(UpstreamResponse {
            status,
            headers,
            body,
            account: None,
        }));
    }

    // Peek until the first real event: a rate-limit error there can still fail over.
    let mut prefix: Vec<Bytes> = Vec::new();
    let mut size = 0;
    let mut parser = SseParser::new();
    let mut committed = false;
    while !committed && size < PEEK_LIMIT {
        let Some(chunk) = body.next().await else {
            break;
        };
        let chunk =
            chunk.map_err(|e| ProxyError::api(format!("upstream stream: {e}")).with_status(502))?;
        size += chunk.len();
        for ev in parser.push(&chunk) {
            let data: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
            let kind = data
                .get("type")
                .and_then(Value::as_str)
                .or(ev.event.as_deref())
                .unwrap_or("");
            match kind {
                "ping" => {}
                "error" => {
                    let err =
                        ProxyError::from_stream_error(data.get("error").unwrap_or(&Value::Null));
                    if err.is_rate_limit() {
                        pool.mark_rate_limited(name, None);
                        return Ok(Attempt::Retry(err.with_status(429)));
                    }
                    committed = true;
                }
                _ => committed = true,
            }
        }
        prefix.push(chunk);
    }
    let prefix_stream = futures_util::stream::iter(prefix.into_iter().map(Ok));
    let body: ByteStream = Box::pin(prefix_stream.chain(body));
    Ok(Attempt::Done(UpstreamResponse {
        status,
        headers,
        body,
        account: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/p/accounts/a"),
            PathBuf::from("/p/accounts/b"),
            PathBuf::from("/home/u/.claude"),
        ]
    }

    #[test]
    fn pick_round_robin_sticky_and_cooldown() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = PoolManager::new(tmp.path().join("usage.json"));
        let none = HashSet::new();
        let first = pool.pick(&dirs(), None, &none).unwrap().0;
        let second = pool.pick(&dirs(), None, &none).unwrap().0;
        assert_ne!(first, second, "round-robin among ties");
        // least-loaded wins
        pool.record_success("a", None);
        pool.record_success("claude-code-login", None);
        assert_eq!(pool.pick(&dirs(), None, &none).unwrap().0, "b");
        // stickiness
        let s = pool.pick(&dirs(), Some("sess"), &none).unwrap().0;
        pool.record_success(&s, None);
        pool.record_success(&s, None);
        assert_eq!(pool.pick(&dirs(), Some("sess"), &none).unwrap().0, s);
        // cooldown drops affinity and excludes the account
        pool.mark_rate_limited(&s, None);
        let after = pool.pick(&dirs(), Some("sess"), &none).unwrap().0;
        assert_ne!(after, s);
        let st = pool.status(&dirs());
        assert!(!st.iter().find(|a| a.name == s).unwrap().available);
        // exclusion
        let ex: HashSet<String> = ["a", "b", "claude-code-login"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(pool.pick(&dirs(), None, &ex).is_none());
    }

    #[test]
    fn usage_file_is_v1_compatible() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("usage.json");
        std::fs::write(&file, r#"{"usage":{"other":{"windowStart":1,"windowRequests":7,"totalRequests":9,"lastError":null,"rateLimitedUntil":null,"lastUsedAt":null,"windowInputTokens":0,"windowOutputTokens":0,"windowCostUsd":0,"totalInputTokens":0,"totalOutputTokens":0,"totalCostUsd":0}}}"#).unwrap();
        let pool = PoolManager::new(file.clone());
        let u = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: Some(100),
            ..Default::default()
        };
        pool.record_success("a", Some(&u));
        let doc: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            doc["usage"]["other"]["totalRequests"], 9,
            "other accounts preserved"
        );
        assert_eq!(doc["usage"]["a"]["windowRequests"], 1);
        assert_eq!(doc["usage"]["a"]["totalInputTokens"], 110);
        assert_eq!(doc["usage"]["a"]["windowOutputTokens"], 5);
        assert!(doc["usage"]["a"]["lastUsedAt"].is_number());
        assert_eq!(
            pool_account_name(Path::new("/x/.claude")),
            "claude-code-login"
        );
    }

    #[test]
    fn reset_headers() {
        let mut h = HeaderMap::new();
        h.insert(
            "anthropic-ratelimit-unified-reset",
            "1900000000".parse().unwrap(),
        );
        assert_eq!(reset_at(&h), Some(1_900_000_000_000));
        let mut h = HeaderMap::new();
        h.insert("retry-after", "30".parse().unwrap());
        assert!(reset_at(&h).unwrap() > now_ms());
        let mut h = HeaderMap::new();
        h.insert(
            "anthropic-ratelimit-tokens-reset",
            "2030-01-01T00:00:00Z".parse().unwrap(),
        );
        assert_eq!(reset_at(&h), Some(1_893_456_000_000));
    }
}
