//! Proxy mode: every signed-in subscription login of a family serves one route.
//!
//! - Membership is live ([`AppState::logins`], re-read every few seconds): a login signed in or out
//!   while sessions run joins or leaves its pool.
//! - Each conversation stays on the login that first served it (its prompt cache lives there) until
//!   that login hits a limit. New conversations go to the login with the most usage left, read from
//!   the rate-limit headers of its last response, else from bro's usage meters.
//! - A limit (a 429, or one reported in the stream's first event) cools the login down until its
//!   reset, and the request moves to the next login before any byte reaches the client. When every
//!   login is out, the client gets the response of the one that resets first, as the upstream sent
//!   it, so the harness shows its own "limit reached · resets at" message.
//! - Requests keep the client's own headers and body. Only the credentials change (and the account
//!   Claude Code names in `metadata.user_id`), so Claude Code and Codex behave as if signed in
//!   directly. Clients that aren't them (Pi, omp, translated requests) are presented as them.
//! - Everything else a harness asks its backend (model lists, token counts) is relayed with the
//!   harness's own credentials ([`relay`]).

use super::{ByteStream, Endpoint, UpstreamCall, UpstreamResponse, anthropic_url, chatgpt, execute, method_for, oauth, set};
use crate::errors::{ErrorKind, ProxyError, relay_headers};
use crate::events::Counter;
use crate::sse::SseParser;
use crate::state::AppState;
use crate::util::{now_ms, now_secs};
use crate::{Dialect, LoginFamily, LoginStatus, PoolLogin};
use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use futures_util::StreamExt;
use http::{HeaderMap, Method};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Bytes of a stream held back while waiting for its first real event.
const PEEK_LIMIT: usize = 64 * 1024;
/// How long a stream is held back at most for that.
const PEEK_TIME: Duration = Duration::from_secs(20);
/// A plain rate limit (requests per minute, not the subscription's usage limit) with no reset given.
const SHORT_COOLDOWN_MS: i64 = 60_000;
/// A usage limit with no reset given.
const LONG_COOLDOWN_MS: i64 = 60 * 60 * 1000;
/// A login that can't sign in (signed out, refresh refused, no access) sits out this long before it's
/// asked again, unless it is the only one left.
const SIGN_IN_REST_MS: i64 = 5 * 60 * 1000;
/// An unread meter counts as half full: a login known to be roomy beats it, a nearly spent one doesn't.
const UNKNOWN_LEFT: f64 = 50.0;

/// A login's latest rate-limit reading, from the headers of its last response.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Meter {
    /// % used of the 5-hour and weekly windows
    pub h5: Option<f32>,
    pub wk: Option<f32>,
    /// unix seconds the windows reset
    pub h5_reset: Option<i64>,
    pub wk_reset: Option<i64>,
    /// The upstream refused this login until then (unix ms)
    pub rejected_until: Option<i64>,
    /// unix ms of the reading
    pub at_ms: i64,
}

fn header<'a>(h: &'a HeaderMap, k: &str) -> Option<&'a str> {
    h.get(k).and_then(|v| v.to_str().ok()).map(str::trim)
}

fn num(h: &HeaderMap, k: &str) -> Option<f64> {
    header(h, k)?.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Claude's unified rate-limit headers (utilization comes as a fraction).
pub(crate) fn claude_meter(h: &HeaderMap) -> Option<Meter> {
    let pct = |k: &str| num(h, k).map(|v| if v <= 1.0 { v * 100.0 } else { v } as f32);
    let rejected = header(h, "anthropic-ratelimit-unified-status") == Some("rejected");
    let m = Meter {
        h5: pct("anthropic-ratelimit-unified-5h-utilization"),
        wk: pct("anthropic-ratelimit-unified-7d-utilization"),
        h5_reset: num(h, "anthropic-ratelimit-unified-5h-reset").map(|v| v as i64),
        wk_reset: num(h, "anthropic-ratelimit-unified-7d-reset").map(|v| v as i64),
        rejected_until: rejected.then(|| num(h, "anthropic-ratelimit-unified-reset").map(|s| s as i64 * 1000)).flatten(),
        at_ms: now_ms(),
    };
    (m.h5.is_some() || m.wk.is_some() || rejected).then_some(m)
}

/// The ChatGPT Codex backend's `x-codex-{primary,secondary}-*` headers (percent used; the window
/// length says which is the 5-hour one).
pub(crate) fn codex_meter(h: &HeaderMap) -> Option<Meter> {
    let mut m = Meter { at_ms: now_ms(), ..Default::default() };
    let mut any = false;
    for slot in ["primary", "secondary"] {
        let Some(used) = num(h, &format!("x-codex-{slot}-used-percent")) else { continue };
        any = true;
        let reset = num(h, &format!("x-codex-{slot}-reset-at"))
            .map(|v| v as i64)
            .or_else(|| num(h, &format!("x-codex-{slot}-reset-after-seconds")).map(|s| now_secs() + s as i64));
        let weekly = num(h, &format!("x-codex-{slot}-window-minutes")).map_or(slot == "secondary", |mins| mins > 24.0 * 60.0);
        if weekly {
            (m.wk, m.wk_reset) = (Some(used as f32), reset);
        } else {
            (m.h5, m.h5_reset) = (Some(used as f32), reset);
        }
    }
    any.then_some(m)
}

/// (5h %, weekly %, 5h reset, weekly reset) from the newer of a login's response meter and bro's
/// usage reading. A window past its reset reads as unused.
fn windows(meter: Option<&Meter>, reading: Option<&bro_core::usage::Usage>) -> (Option<f32>, Option<f32>, Option<i64>, Option<i64>) {
    let now = now_secs();
    let fresh = |used: Option<f32>, reset: Option<i64>| used.map(|u| if reset.is_some_and(|r| r <= now) { 0.0 } else { u });
    let meter_newer = match (meter, reading) {
        (Some(m), Some(r)) => m.at_ms / 1000 >= r.fetched_at,
        (Some(_), None) => true,
        _ => false,
    };
    if meter_newer && let Some(m) = meter {
        return (fresh(m.h5, m.h5_reset), fresh(m.wk, m.wk_reset), m.h5_reset, m.wk_reset);
    }
    match reading {
        Some(r) => {
            let w = |w: Option<&bro_core::usage::Window>| (w.map(|w| w.used_pct), w.and_then(|w| w.resets_at));
            let ((h5, h5r), (wk, wkr)) = (w(r.five_hour.as_ref()), w(r.weekly.as_ref()));
            (fresh(h5, h5r), fresh(wk, wkr), h5r, wkr)
        }
        None => (None, None, None, None),
    }
}

/// How a login stands for a new conversation.
struct Standing {
    /// % left of its tighter window (None: nothing known)
    left: Option<f64>,
    /// Out of usage right now (a window at 100%, or refused until a reset still ahead)
    spent: bool,
}

fn standing(meter: Option<&Meter>, reading: Option<&bro_core::usage::Usage>) -> Standing {
    let (h5, wk, _, _) = windows(meter, reading);
    let used = [h5, wk].into_iter().flatten().fold(None::<f32>, |a, u| Some(a.map_or(u, |a| a.max(u))));
    let refused = meter.and_then(|m| m.rejected_until).is_some_and(|t| t > now_ms());
    Standing { left: used.map(|u| (100.0 - u as f64).clamp(0.0, 100.0)), spent: refused || used.is_some_and(|u| u >= 100.0) }
}

/// Where a login counts in its family's counters: v1's account name for Claude (v1 shares the file),
/// the profile id for Codex.
fn counter_key(family: LoginFamily, login: &PoolLogin) -> String {
    match family {
        LoginFamily::Claude => super::pool::pool_account_name(&login.dir),
        LoginFamily::Codex => login.id.clone(),
    }
}

/// The login a request goes to: the conversation's own unless it is cooling, spent or `tried`; else
/// the one with the most left (fewest requests in the window breaking ties, then taking turns).
fn pick(state: &AppState, family: LoginFamily, logins: &[PoolLogin], session: Option<&str>, tried: &HashSet<String>) -> Option<PoolLogin> {
    let counters = state.counters(family);
    let keys: Vec<String> = logins.iter().map(|l| counter_key(family, l)).collect();
    let usages = counters.usages(&keys.iter().map(String::as_str).collect::<Vec<_>>());
    let meters = state.meters.lock().clone();
    struct Cand {
        i: usize,
        standing: Standing,
        requests: u64,
    }
    let cands: Vec<Cand> = logins
        .iter()
        .enumerate()
        .filter(|(i, l)| !tried.contains(&l.id) && !usages[*i].is_cooling())
        .map(|(i, l)| Cand { i, standing: standing(meters.get(&l.id), bro_core::usage::cache::for_profile(&l.id).as_ref()), requests: usages[i].window_requests })
        .collect();
    if let Some(prior) = session.and_then(|s| counters.affinity(s))
        && let Some(c) = cands.iter().find(|c| keys[c.i] == prior && !c.standing.spent)
    {
        return Some(logins[c.i].clone());
    }
    let rank = |c: &Cand| (c.standing.spent, -(c.standing.left.unwrap_or(UNKNOWN_LEFT).round() as i64), c.requests);
    let best = cands.iter().map(rank).min()?;
    let tied: Vec<&Cand> = cands.iter().filter(|c| rank(c) == best).collect();
    let chosen = tied[counters.take_turn(tied.len())];
    if let Some(s) = session {
        counters.set_affinity(s, &keys[chosen.i]);
    }
    Some(logins[chosen.i].clone())
}

/// When every login is cooling down: the one whose cooldown ends first (it may be back already, and
/// if not, its answer is the one the client should see).
fn soonest(state: &AppState, family: LoginFamily, logins: &[PoolLogin]) -> Option<PoolLogin> {
    let counters = state.counters(family);
    let keys: Vec<String> = logins.iter().map(|l| counter_key(family, l)).collect();
    let usages = counters.usages(&keys.iter().map(String::as_str).collect::<Vec<_>>());
    logins.iter().zip(usages).min_by_key(|(_, u)| u.rate_limited_until.unwrap_or(0)).map(|(l, _)| l.clone())
}

enum Attempt {
    Done(UpstreamResponse),
    /// Try the next login; `until` = when this one is back (a limit), unix ms
    Next(ProxyError, Option<i64>),
}

/// Send `call` as the family's logins, picking and failing over (see the module docs).
pub(crate) async fn send(state: &AppState, family: LoginFamily, call: &UpstreamCall) -> Result<UpstreamResponse, ProxyError> {
    let logins = state.logins(family).await;
    if logins.is_empty() {
        let who = match family {
            LoginFamily::Claude => "Claude",
            LoginFamily::Codex => "Codex (ChatGPT)",
        };
        return Err(ProxyError::new(ErrorKind::Overloaded, format!("proxy mode has no signed-in {who} logins: sign one in from bro's profiles")).with_status(503));
    }
    let counters = state.counters(family);
    let session = call.session_key.as_deref();
    let mut tried: HashSet<String> = HashSet::new();
    let mut failures: Vec<(ProxyError, Option<i64>)> = vec![];
    loop {
        let next = pick(state, family, &logins, session, &tried).or_else(|| if tried.is_empty() { soonest(state, family, &logins) } else { None });
        let Some(login) = next else { break };
        tried.insert(login.id.clone());
        let key = counter_key(family, &login);
        let outcome = match family {
            LoginFamily::Claude => attempt_claude(state, &login, &key, call).await?,
            LoginFamily::Codex => attempt_codex(state, &login, &key, call).await?,
        };
        match outcome {
            Attempt::Done(mut resp) => {
                if let Some(s) = session {
                    counters.set_affinity(s, &key);
                }
                resp.account = Some(login.id.clone());
                resp.counter = Some(Counter { pool: counters.clone(), key });
                return Ok(resp);
            }
            Attempt::Next(e, until) => {
                tracing::warn!(login = %login.id, reason = %e.message, "proxy mode: moving to the next login");
                failures.push((e, until));
            }
        }
    }
    // Every login refused: a limit with the earliest reset if there is one, else the last failure.
    let limited = failures.iter().enumerate().filter_map(|(i, (_, u))| u.map(|u| (u, i))).min().map(|(_, i)| i);
    let i = limited.unwrap_or(failures.len().saturating_sub(1));
    Err(failures.into_iter().nth(i).map(|(e, _)| e).unwrap_or_else(|| ProxyError::new(ErrorKind::Overloaded, "proxy mode: no login could take the request").with_status(503)))
}

/// The request came from Claude Code itself (it carries its own identity and system prompt).
fn from_claude_code(h: &HeaderMap) -> bool {
    header(h, "user-agent").is_some_and(|ua| ua.starts_with("claude-cli/")) || header(h, "x-app") == Some("cli")
}

/// The request came from Codex itself (it names its originator).
fn from_codex(h: &HeaderMap) -> bool {
    h.contains_key("originator")
}

/// `metadata.user_id` naming `account` instead of the client's own: Claude Code's JSON form
/// (`{"device_id":…,"account_uuid":…,"session_id":…}`) or the older `user_…_account_…_session_…`.
pub(crate) fn rename_account(user_id: &str, account: &str) -> Option<String> {
    if let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(user_id) {
        let current = o.get("account_uuid").and_then(Value::as_str)?;
        if current == account {
            return None;
        }
        o.insert("account_uuid".into(), json!(account));
        return serde_json::to_string(&o).ok();
    }
    let (head, rest) = user_id.split_once("_account_")?;
    let (current, tail) = rest.split_once("_session_")?;
    (current != account).then(|| format!("{head}_account_{account}_session_{tail}"))
}

/// `body` with the first `from` replaced by `to`.
fn replace_once(body: &Bytes, from: &[u8], to: &[u8]) -> Bytes {
    match memchr::memmem::find(body, from) {
        Some(at) => {
            let mut out = Vec::with_capacity(body.len() + to.len());
            out.extend_from_slice(&body[..at]);
            out.extend_from_slice(to);
            out.extend_from_slice(&body[at + from.len()..]);
            Bytes::from(out)
        }
        None => body.clone(),
    }
}

/// What goes to Anthropic as `login`: the client's body, with Claude Code's identity first in the
/// system prompt when the client isn't Claude Code (subscription tokens require it), and the
/// login's own account in `metadata.user_id`.
fn claude_body(call: &UpstreamCall, login: &PoolLogin, cloak: bool) -> Option<Bytes> {
    let mut body = call.body.clone()?;
    if cloak && let Ok(mut v) = serde_json::from_slice::<Value>(&body) {
        crate::translate::ensure_claude_code_system_json(&mut v);
        body = Bytes::from(serde_json::to_vec(&v).unwrap_or_default());
    }
    if let (Some(uid), Some(account)) = (call.user_id.as_deref(), login.account.as_deref())
        && let Some(renamed) = rename_account(uid, account)
        && let (Ok(from), Ok(to)) = (serde_json::to_string(uid), serde_json::to_string(&renamed))
    {
        body = replace_once(&body, from.as_bytes(), to.as_bytes());
    }
    Some(body)
}

/// `login` couldn't sign in: it sits out a while and the request goes to the next login.
fn sign_in_failed(state: &AppState, family: LoginFamily, key: &str, e: ProxyError) -> Attempt {
    state.counters(family).rest(key, now_ms() + SIGN_IN_REST_MS, &e.message);
    Attempt::Next(e, None)
}

fn note_meter(state: &AppState, login: &PoolLogin, meter: Option<Meter>) {
    if let Some(m) = meter {
        state.meters.lock().insert(login.id.clone(), m);
    }
}

/// A usage limit (as opposed to a plain per-minute rate limit) from its message or body.
fn usage_limit(e: &ProxyError) -> bool {
    let body = e.raw.as_ref().map(|r| String::from_utf8_lossy(&r.body).to_lowercase()).unwrap_or_default();
    let m = e.message.to_lowercase();
    ["usage_limit_reached", "usage limit", "limit reached", "quota"].iter().any(|k| m.contains(k) || body.contains(k))
}

/// When a limited login is back (unix ms): what the upstream said, else a minute for a plain rate
/// limit and an hour for a usage limit.
fn back_at(family: LoginFamily, e: &ProxyError, h: &HeaderMap) -> i64 {
    let now = now_ms();
    let told = match family {
        LoginFamily::Claude => super::pool::reset_at(h),
        LoginFamily::Codex => {
            let body: Value = e.raw.as_ref().and_then(|r| serde_json::from_slice(&r.body).ok()).unwrap_or(Value::Null);
            let err = body.get("error").unwrap_or(&Value::Null);
            err.get("resets_at")
                .and_then(Value::as_i64)
                .map(|s| s * 1000)
                .or_else(|| err.get("resets_in_seconds").and_then(Value::as_i64).map(|s| now + s * 1000))
                .or_else(|| super::pool::reset_at(h))
                .or_else(|| {
                    // the spent window's reset
                    let m = codex_meter(h)?;
                    [(m.h5, m.h5_reset), (m.wk, m.wk_reset)].into_iter().filter(|(u, _)| u.is_some_and(|u| u >= 100.0)).filter_map(|(_, r)| r).max().map(|s| s * 1000)
                })
        }
    };
    told.filter(|t| *t > now).unwrap_or_else(|| now + if usage_limit(e) { LONG_COOLDOWN_MS } else { SHORT_COOLDOWN_MS })
}

/// A failed response from `login`: cool it down after a limit, and say whether another login should
/// get the request.
fn failed(state: &AppState, family: LoginFamily, key: &str, e: ProxyError, h: &HeaderMap) -> Result<Attempt, ProxyError> {
    let counters = state.counters(family);
    if e.status == 429 || e.is_rate_limit() {
        let until = back_at(family, &e, h);
        counters.rest(key, until, &e.message);
        return Ok(Attempt::Next(e, Some(until)));
    }
    if matches!(e.status, 401 | 403) {
        // this login's own problem (signed out, no access to the model): another may serve it
        return Ok(sign_in_failed(state, family, key, e));
    }
    // the request's own problem, or the service's: the other logins would say the same
    Err(e)
}

async fn attempt_claude(state: &AppState, login: &PoolLogin, key: &str, call: &UpstreamCall) -> Result<Attempt, ProxyError> {
    let family = LoginFamily::Claude;
    let counters = state.counters(family);
    let url = anthropic_url(&state.opts.anthropic_base_url, call.endpoint);
    let cloak = !call.translated && !from_claude_code(&call.client_headers);
    let body = claude_body(call, login, cloak);
    let mut force = false;
    loop {
        let token = match state.claude_token(&login.dir, force).await {
            Ok(t) => t,
            Err(e) => return Ok(sign_in_failed(state, family, key, e)),
        };
        let headers = oauth::oauth_headers_as(call, &token, call.translated || cloak);
        let resp = match execute(state, method_for(call.endpoint), &url, headers, body.clone()).await {
            Ok(r) => r,
            Err(e) => {
                counters.record_error(key, &e.message);
                return Ok(Attempt::Next(e, None));
            }
        };
        let status = resp.status();
        let h = resp.headers().clone();
        note_meter(state, login, claude_meter(&h));
        if status == 401 && !force {
            tracing::info!(login = %login.id, "proxy mode: Claude 401; refreshing the token and retrying");
            force = true;
            continue;
        }
        if !status.is_success() {
            let bytes = resp.bytes().await.unwrap_or_default();
            let e = ProxyError::from_upstream(status.as_u16(), &h, &bytes).with_raw(Dialect::Anthropic, status.as_u16(), &h, bytes);
            return failed(state, family, key, e, &h);
        }
        return held(state, family, key, call, resp).await;
    }
}

/// The Codex backend's URL for an endpoint.
fn codex_url(state: &AppState, endpoint: Endpoint) -> String {
    let base = state.opts.chatgpt_base_url.trim_end_matches('/');
    match endpoint {
        Endpoint::Models => format!("{base}/models?client_version={}", chatgpt::CLIENT_VERSION),
        _ => format!("{base}/responses"),
    }
}

/// Codex's own headers, signed as `auth` instead.
fn codex_relay_headers(client: &HeaderMap, auth: &bro_core::creds::CodexAuth, stream: bool) -> HeaderMap {
    let mut h = client.clone();
    h.remove("chatgpt-account-id");
    set(&mut h, "authorization", &format!("Bearer {}", auth.access_token));
    if let Some(acct) = auth.account_id.as_deref().filter(|a| !a.is_empty()) {
        set(&mut h, "chatgpt-account-id", acct);
    }
    set(&mut h, "content-type", "application/json");
    set(&mut h, "accept", if stream { "text/event-stream" } else { "application/json" });
    h
}

async fn attempt_codex(state: &AppState, login: &PoolLogin, key: &str, call: &UpstreamCall) -> Result<Attempt, ProxyError> {
    let family = LoginFamily::Codex;
    let counters = state.counters(family);
    let url = codex_url(state, call.endpoint);
    let native = !call.translated && from_codex(&call.client_headers);
    let mut force = false;
    loop {
        let auth = match state.codex_auth(&login.dir, force).await {
            Ok(a) if chatgpt::is_chatgpt_login(&a) => a,
            Ok(_) => return Ok(sign_in_failed(state, family, key, ProxyError::new(ErrorKind::Authentication, format!("{} is an API-key login, not a ChatGPT subscription", login.id)))),
            Err(e) => return Ok(sign_in_failed(state, family, key, e)),
        };
        let headers = if native {
            codex_relay_headers(&call.client_headers, &auth, call.stream)
        } else {
            chatgpt::backend_headers(&auth, call.session_key.as_deref().unwrap_or(&state.session_id), call.stream)
        };
        let resp = match execute(state, method_for(call.endpoint), &url, headers, call.body.clone()).await {
            Ok(r) => r,
            Err(e) => {
                counters.record_error(key, &e.message);
                return Ok(Attempt::Next(e, None));
            }
        };
        let status = resp.status();
        let h = resp.headers().clone();
        note_meter(state, login, codex_meter(&h));
        if status == 401 && !force {
            tracing::info!(login = %login.id, "proxy mode: ChatGPT 401; refreshing the token and retrying");
            force = true;
            continue;
        }
        if !status.is_success() {
            let bytes = resp.bytes().await.unwrap_or_default();
            let e = ProxyError::from_upstream(status.as_u16(), &h, &bytes).with_raw(Dialect::Responses, status.as_u16(), &h, bytes);
            return failed(state, family, key, e, &h);
        }
        return held(state, family, key, call, resp).await;
    }
}

/// What a stream's event says before anything was committed to the client.
enum Early {
    /// a keepalive or the stream opening: keep waiting
    Wait,
    /// the login is at its limit
    Limited(ProxyError),
    /// real content (or an error that isn't a limit): hand the stream over
    Commit,
}

fn early(family: LoginFamily, event: Option<&str>, data: &str) -> Early {
    let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
    let kind = v.get("type").and_then(Value::as_str).or(event).unwrap_or("");
    let err = match (family, kind) {
        (LoginFamily::Claude, "ping") | (LoginFamily::Codex, "response.created" | "response.in_progress" | "keepalive") => return Early::Wait,
        (LoginFamily::Claude, "error") => v.get("error").cloned().unwrap_or(Value::Null),
        (LoginFamily::Codex, "response.failed") => v.pointer("/response/error").cloned().unwrap_or(Value::Null),
        (LoginFamily::Codex, "error") => v.get("error").cloned().unwrap_or_else(|| v.clone()),
        _ => return Early::Commit,
    };
    let e = ProxyError::from_stream_error(&err);
    if e.is_rate_limit() { Early::Limited(e.with_status(429)) } else { Early::Commit }
}

/// A 2xx from `login`. A stream is held until its first real event, so a limit reported there still
/// moves the request to the next login.
async fn held(state: &AppState, family: LoginFamily, key: &str, call: &UpstreamCall, resp: reqwest::Response) -> Result<Attempt, ProxyError> {
    let status = resp.status();
    let headers = resp.headers().clone();
    let is_sse = headers.get(http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("event-stream"));
    let mut body: ByteStream = Box::pin(futures_util::TryStreamExt::map_err(resp.bytes_stream(), std::io::Error::other));
    let done = |body: ByteStream| Attempt::Done(UpstreamResponse { status, headers: headers.clone(), body, account: None, counter: None });
    if !(call.stream || is_sse) {
        return Ok(done(body));
    }
    let mut prefix: Vec<Bytes> = vec![];
    let mut size = 0;
    let mut parser = SseParser::new();
    let until = Instant::now() + PEEK_TIME;
    'peek: while size < PEEK_LIMIT {
        let Ok(next) = tokio::time::timeout(until.saturating_duration_since(Instant::now()), body.next()).await else { break };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| ProxyError::api(format!("upstream stream: {e}")).with_status(502))?;
        size += chunk.len();
        for ev in parser.push(&chunk) {
            match early(family, ev.event.as_deref(), &ev.data) {
                Early::Wait => {}
                Early::Commit => {
                    prefix.push(chunk);
                    break 'peek;
                }
                Early::Limited(e) => {
                    let until = now_ms() + if usage_limit(&e) { LONG_COOLDOWN_MS } else { SHORT_COOLDOWN_MS };
                    state.counters(family).rest(key, until, &e.message);
                    return Ok(Attempt::Next(e, Some(until)));
                }
            }
        }
        prefix.push(chunk);
    }
    let prefix = futures_util::stream::iter(prefix.into_iter().map(Ok));
    Ok(done(Box::pin(prefix.chain(body))))
}

// ------------------------------------------------------------------ relay

/// Request headers a relay doesn't pass on (the proxy's own key, and what the HTTP client sets).
const RELAY_DROP: &[&str] = &["host", "content-length", "connection", "keep-alive", "transfer-encoding", "te", "trailer", "upgrade", "proxy-authorization", "accept-encoding"];

/// Anything else a harness asks its own backend under a proxy-mode route (model lists, token
/// counts, files…), relayed with the harness's own credentials as if it were signed in directly.
/// `path` is what follows `/r/{route}` (with its query).
pub(crate) async fn relay(state: &AppState, family: LoginFamily, method: Method, path: &str, headers: &HeaderMap, body: Bytes) -> Result<Response, ProxyError> {
    let url = match family {
        LoginFamily::Claude => format!("{}{path}", state.opts.anthropic_base_url.trim_end_matches('/')),
        LoginFamily::Codex => {
            let rest = path.strip_prefix("/backend-api/codex").filter(|r| r.is_empty() || r.starts_with('/') || r.starts_with('?'));
            let rest = rest.ok_or_else(|| ProxyError::new(ErrorKind::NotFound, format!("no Codex backend endpoint for {path}")))?;
            format!("{}{rest}", state.opts.chatgpt_base_url.trim_end_matches('/'))
        }
    };
    let mut h = HeaderMap::new();
    for (k, v) in headers {
        if !RELAY_DROP.contains(&k.as_str()) {
            h.append(k.clone(), v.clone());
        }
    }
    let body = (!body.is_empty() || method != Method::GET).then_some(body);
    let resp = execute(state, method, &url, h, body).await?;
    let status = resp.status();
    let out_headers = relay_headers(resp.headers());
    let stream = futures_util::TryStreamExt::map_err(resp.bytes_stream(), std::io::Error::other);
    let mut out = Response::new(Body::from_stream(stream));
    *out.status_mut() = status;
    *out.headers_mut() = out_headers;
    Ok(out)
}

// ------------------------------------------------------------------ status

/// Proxy mode's logins of `family` as the proxy sees them (blocking).
pub(crate) fn status(state: &AppState, family: LoginFamily) -> Vec<LoginStatus> {
    let logins = state.logins_blocking(family);
    let counters = state.counters(family);
    let keys: Vec<String> = logins.iter().map(|l| counter_key(family, l)).collect();
    let usages = counters.usages(&keys.iter().map(String::as_str).collect::<Vec<_>>());
    let meters = state.meters.lock().clone();
    let now = now_ms();
    logins
        .into_iter()
        .zip(usages)
        .zip(keys)
        .map(|((l, usage), key)| {
            let reading = bro_core::usage::cache::for_profile(&l.id);
            let (h5, wk, h5r, wkr) = windows(meters.get(&l.id), reading.as_ref());
            LoginStatus {
                available: !usage.is_cooling(),
                cooling_until_ms: usage.rate_limited_until.filter(|t| *t > now),
                five_hour_pct: h5,
                weekly_pct: wk,
                five_hour_resets_at: h5r,
                weekly_resets_at: wkr,
                conversations: counters.conversations(&key),
                usage,
                id: l.id,
                dir: l.dir,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn meters_from_headers() {
        let m = claude_meter(&h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.1"),
            ("anthropic-ratelimit-unified-5h-reset", "4102444800"),
        ]))
        .unwrap();
        assert_eq!((m.h5, m.wk, m.h5_reset, m.rejected_until), (Some(42.0), Some(10.0), Some(4_102_444_800), None));
        let m = claude_meter(&h(&[("anthropic-ratelimit-unified-status", "rejected"), ("anthropic-ratelimit-unified-reset", "4102444800")])).unwrap();
        assert_eq!(m.rejected_until, Some(4_102_444_800_000));
        assert!(claude_meter(&HeaderMap::new()).is_none());

        let m = codex_meter(&h(&[
            ("x-codex-primary-used-percent", "12.5"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-at", "4102444800"),
            ("x-codex-secondary-used-percent", "80"),
            ("x-codex-secondary-window-minutes", "10080"),
        ]))
        .unwrap();
        assert_eq!((m.h5, m.wk, m.h5_reset), (Some(12.5), Some(80.0), Some(4_102_444_800)));
        // a weekly-only plan reports its week as the primary window
        let m = codex_meter(&h(&[("x-codex-primary-used-percent", "30"), ("x-codex-primary-window-minutes", "10080")])).unwrap();
        assert_eq!((m.h5, m.wk), (None, Some(30.0)));
    }

    #[test]
    fn standing_reads_the_newer_source_and_resets() {
        let past = now_secs() - 10;
        let m = Meter { h5: Some(100.0), h5_reset: Some(past), wk: Some(60.0), at_ms: now_ms(), ..Default::default() };
        let s = standing(Some(&m), None);
        assert_eq!((s.left, s.spent), (Some(40.0), false), "a 5h window past its reset counts as unused");
        let spent = Meter { h5: Some(100.0), h5_reset: Some(now_secs() + 600), at_ms: now_ms(), ..Default::default() };
        assert!(standing(Some(&spent), None).spent);
        let refused = Meter { rejected_until: Some(now_ms() + 60_000), at_ms: now_ms(), ..Default::default() };
        assert!(standing(Some(&refused), None).spent);
        // bro's meter reading wins when it is newer than the last response
        let old = Meter { h5: Some(90.0), at_ms: 1_000, ..Default::default() };
        let reading = bro_core::usage::Usage { five_hour: Some(bro_core::usage::Window { used_pct: 20.0, resets_at: None, window_mins: Some(300) }), fetched_at: now_secs(), ..Default::default() };
        assert_eq!(standing(Some(&old), Some(&reading)).left, Some(80.0));
        assert_eq!(standing(None, None).left, None);
    }

    #[test]
    fn user_id_names_the_serving_account() {
        let json_id = r#"{"device_id":"d1","account_uuid":"home","session_id":"s1"}"#;
        assert_eq!(rename_account(json_id, "other").unwrap(), r#"{"device_id":"d1","account_uuid":"other","session_id":"s1"}"#);
        assert_eq!(rename_account(json_id, "home"), None, "already its own");
        assert_eq!(rename_account("user_abc_account_home_session_s1", "other").unwrap(), "user_abc_account_other_session_s1");
        assert_eq!(rename_account("plain", "other"), None);
        let body = Bytes::from(serde_json::to_vec(&json!({"metadata": {"user_id": json_id}, "messages": []})).unwrap());
        let from = serde_json::to_string(json_id).unwrap();
        let to = serde_json::to_string(&rename_account(json_id, "other").unwrap()).unwrap();
        let out: Value = serde_json::from_slice(&replace_once(&body, from.as_bytes(), to.as_bytes())).unwrap();
        assert_eq!(out["metadata"]["user_id"], r#"{"device_id":"d1","account_uuid":"other","session_id":"s1"}"#);
    }

    #[test]
    fn early_events() {
        assert!(matches!(early(LoginFamily::Claude, Some("ping"), r#"{"type":"ping"}"#), Early::Wait));
        assert!(matches!(early(LoginFamily::Claude, Some("message_start"), r#"{"type":"message_start"}"#), Early::Commit));
        assert!(matches!(early(LoginFamily::Claude, Some("error"), r#"{"type":"error","error":{"type":"rate_limit_error","message":"x"}}"#), Early::Limited(_)));
        assert!(matches!(early(LoginFamily::Claude, Some("error"), r#"{"type":"error","error":{"type":"overloaded_error","message":"x"}}"#), Early::Commit));
        assert!(matches!(early(LoginFamily::Codex, Some("response.created"), r#"{"type":"response.created"}"#), Early::Wait));
        let failed = r#"{"type":"response.failed","response":{"error":{"code":"usage_limit_reached","message":"You've hit your usage limit"}}}"#;
        assert!(matches!(early(LoginFamily::Codex, None, failed), Early::Limited(_)));
        assert!(matches!(early(LoginFamily::Codex, None, r#"{"type":"response.output_text.delta"}"#), Early::Commit));
    }
}
