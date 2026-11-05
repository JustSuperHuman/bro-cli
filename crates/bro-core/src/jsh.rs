//! Just Superhuman accounts: sign in from bro (RFC 8628 device flow against
//! `<origin>/api/connect/v1`), keep the revocable `jsh_user_` token and the account it belongs to, and
//! offer the account's Claude as the `justsuperhuman` provider. Claude Code launched on it talks to
//! Just Superhuman's Claude proxy with that token and spends the organization's credits.
//!
//! The contract is F:\JustSuperHuman\JustSuperServer\docs\CONNECT.md.
//!
//! Storage (`~/.bro/jsh/`, bro's own; v1 has nothing like it):
//! * `token.json`: the token and the origin it was issued by (0600 where the OS has modes). Never copied
//!   into `config.json`, `v2.toml` or the account cache.
//! * `account.json`: the last account the server sent, for showing credits and the provider's models
//!   without a request.
//!
//! One [`Login`] per process is shared by the desktop app, the terminal UI, the phone bridge and the CLI:
//! a sign-in started from the phone shows in the window and the other way round.
use crate::creds::restrict_permissions;
use crate::providers::{Model, Provider, ProviderMode};
use crate::util::{atomic_write, env_str, now_ms, read_json};
use anyhow::{Context, anyhow, bail};
use parking_lot::Mutex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The provider id Just Superhuman's Claude has in the launcher.
pub const PROVIDER_ID: &str = "justsuperhuman";
pub const PROVIDER_NAME: &str = "Just Superhuman";
/// The profile id the phone bridge advertises for Claude Code on this provider.
pub const BRIDGE_PROFILE_ID: &str = "jsh:claude";
/// Where the accounts live.
pub const PRODUCTION_ORIGIN: &str = "https://console.justsuperhuman.com";
/// Name bro gives the server (shown on the person's Account page).
const CLIENT: &str = "bro";
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Polls that may fail in a row (offline, a restart) before the sign-in gives up.
const MAX_POLL_FAILURES: u32 = 6;
/// How old the cached account may get before the background refresher asks again.
pub const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);

/// The accounts server: `BRO_JSH_ORIGIN` (`http://auth.localhost:8790` for local development) or production.
pub fn origin() -> String {
    env_str("BRO_JSH_ORIGIN").map(|o| o.trim().trim_end_matches('/').to_string()).filter(|o| !o.is_empty()).unwrap_or_else(|| PRODUCTION_ORIGIN.into())
}

fn dir() -> PathBuf {
    crate::paths::bro_dir().join("jsh")
}

/// `~/.bro/jsh/token.json`.
pub fn token_path() -> PathBuf {
    dir().join("token.json")
}

/// `~/.bro/jsh/account.json`.
pub fn account_path() -> PathBuf {
    dir().join("account.json")
}

// ---------------------------------------------------------------- the account

/// Whole numbers, whichever way the server writes them (`1840` or `1840.0`).
fn whole<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(|n| n.round() as i64).unwrap_or(0))
}

fn whole_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(|n| n.round() as i64))
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slug: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    /// What can be spent now: this month's allowance left plus one-time granted credits.
    #[serde(default, deserialize_with = "whole")]
    pub available: i64,
    /// One-time credits (invite codes); they never expire.
    #[serde(default, deserialize_with = "whole")]
    pub granted: i64,
    #[serde(default, deserialize_with = "whole")]
    pub monthly_allowance: i64,
    /// When the monthly allowance renews, ms since the epoch.
    #[serde(default, deserialize_with = "whole_opt")]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaudeModel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeInfo {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub available: bool,
    /// Plain English, when `available` is false (out of credits, AI paused, shared Claude down).
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub models: Vec<ClaudeModel>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Host {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub status: String,
}

/// `GET /api/connect/v1/account`, the parts bro uses.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub user: User,
    #[serde(default)]
    pub organization: Organization,
    #[serde(default)]
    pub credits: Credits,
    #[serde(default)]
    pub claude: ClaudeInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Host>,
}

impl Account {
    /// "Sam Rivera", or the email when the account has no name.
    pub fn who(&self) -> &str {
        if self.user.name.trim().is_empty() { &self.user.email } else { &self.user.name }
    }

    /// "1,840 credits".
    pub fn credits_text(&self) -> String {
        let n = self.credits.available;
        format!("{} credit{}", grouped(n), if n == 1 { "" } else { "s" })
    }

    /// One line for pickers: "1,840 credits · Rivera Studio".
    pub fn summary(&self) -> String {
        let mut parts = vec![self.credits_text()];
        if !self.organization.name.is_empty() {
            parts.push(self.organization.name.clone());
        }
        parts.join(" · ")
    }
}

/// 1840 -> "1,840".
pub fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

// ---------------------------------------------------------------- the server

/// `POST /device`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// One look at `POST /token`.
#[derive(Debug, Clone, PartialEq)]
pub enum Poll {
    Pending,
    /// Polled too fast: wait five seconds longer between polls from now on.
    SlowDown,
    Denied,
    Expired,
    Done { token: String, account: Box<Account> },
}

/// What `GET /account` said.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched {
    Account(Box<Account>),
    /// 401: the token was revoked.
    Revoked,
}

/// The computer's name, shown beside bro on the person's Account page.
pub fn device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME", "HOST"]
        .iter()
        .find_map(|v| env_str(v))
        .or_else(|| {
            let out = std::process::Command::new("hostname").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "this computer".into())
}

fn transport(origin: &str, e: anyhow::Error) -> anyhow::Error {
    anyhow!("Couldn't reach Just Superhuman at {origin}. Check your connection and try again. ({e:#})")
}

/// The server's own sentence for a failure, else a plain one.
fn server_message(body: &Value, status: u16) -> String {
    let say = |v: &Value| v.as_str().map(str::to_string).filter(|s| !s.is_empty());
    say(&body["message"]).or_else(|| say(&body["error_description"])).or_else(|| say(&body["error"])).unwrap_or_else(|| format!("Just Superhuman answered with HTTP {status}."))
}

/// `POST /api/connect/v1/device`.
pub fn start_device(origin: &str) -> anyhow::Result<DeviceStart> {
    let body = json!({ "client": CLIENT, "device": device_name(), "version": env!("CARGO_PKG_VERSION") });
    let url = format!("{origin}/api/connect/v1/device");
    let resp = crate::http::post_json(&url, &body, CALL_TIMEOUT).map_err(|e| transport(origin, e))?;
    if !resp.ok() {
        bail!("Just Superhuman wouldn't start a sign-in: {}", server_message(&resp.body, resp.status));
    }
    let start: DeviceStart = serde_json::from_value(resp.body).context("Just Superhuman's sign-in answer wasn't what bro expects. Update bro and try again")?;
    if start.device_code.is_empty() || start.user_code.is_empty() {
        bail!("Just Superhuman's sign-in answer had no code. Try again in a moment.");
    }
    Ok(start)
}

/// `POST /api/connect/v1/token`, once.
pub fn poll_token(origin: &str, device_code: &str) -> anyhow::Result<Poll> {
    let url = format!("{origin}/api/connect/v1/token");
    let resp = crate::http::post_json(&url, &json!({ "device_code": device_code }), CALL_TIMEOUT).map_err(|e| transport(origin, e))?;
    if resp.ok() {
        let token = resp.body.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| anyhow!("Just Superhuman approved the sign-in but sent no token. Try again."))?;
        let account: Account = serde_json::from_value(resp.body.get("account").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        return Ok(Poll::Done { token: token.to_string(), account: Box::new(account) });
    }
    match resp.body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => Ok(Poll::SlowDown),
        Some("access_denied") => Ok(Poll::Denied),
        Some("expired_token") => Ok(Poll::Expired),
        _ if resp.status == 429 => Ok(Poll::SlowDown),
        _ => bail!("Just Superhuman couldn't check the sign-in: {}", server_message(&resp.body, resp.status)),
    }
}

/// `GET /api/connect/v1/account`.
pub fn fetch_account(origin: &str, token: &str) -> anyhow::Result<Fetched> {
    let url = format!("{origin}/api/connect/v1/account");
    let resp = crate::http::get_json(&url, &[("authorization", &format!("Bearer {token}"))], CALL_TIMEOUT).map_err(|e| transport(origin, e))?;
    match resp.status {
        401 => Ok(Fetched::Revoked),
        _ if resp.ok() => Ok(Fetched::Account(Box::new(serde_json::from_value(resp.body).context("Just Superhuman's account answer wasn't what bro expects")?))),
        s => bail!("Just Superhuman couldn't load the account: {}", server_message(&resp.body, s)),
    }
}

/// `POST /api/connect/v1/revoke`. A 401 means it was revoked already, which is what was asked.
pub fn revoke_token(origin: &str, token: &str) -> anyhow::Result<()> {
    let url = format!("{origin}/api/connect/v1/revoke");
    let resp = crate::http::post_json_with(&url, &[("authorization", &format!("Bearer {token}"))], &json!({}), CALL_TIMEOUT).map_err(|e| transport(origin, e))?;
    if resp.ok() || resp.status == 401 {
        return Ok(());
    }
    bail!("Just Superhuman wouldn't revoke this sign-in: {}", server_message(&resp.body, resp.status))
}

// ---------------------------------------------------------------- storage

/// What `token.json` holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub token: String,
    /// The server that issued it; later requests go there even if `BRO_JSH_ORIGIN` changes.
    pub origin: String,
    #[serde(default, rename = "savedAt")]
    pub saved_at: i64,
}

/// The stored sign-in (`None` when signed out).
pub fn stored() -> Option<Stored> {
    let s: Stored = serde_json::from_value(read_json(&token_path())?).ok()?;
    (!s.token.trim().is_empty()).then_some(s)
}

/// The token the provider sends as its key.
pub fn token() -> Option<String> {
    stored().map(|s| s.token)
}

/// There is a sign-in on this computer (the server may have revoked it since).
pub fn signed_in() -> bool {
    stored().is_some()
}

/// The last account saved (credits as of `fetched_at`).
pub fn cached_account() -> Option<(Account, i64)> {
    let doc = read_json(&account_path())?;
    let account = serde_json::from_value(doc.get("account")?.clone()).ok()?;
    Some((account, doc.get("fetchedAt").and_then(Value::as_i64).unwrap_or(0)))
}

fn save_token(token: &str, origin: &str) -> anyhow::Result<()> {
    let doc = json!({ "token": token, "origin": origin, "savedAt": now_ms() });
    let path = token_path();
    atomic_write(&path, serde_json::to_string_pretty(&doc)?.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
    restrict_permissions(&path);
    Ok(())
}

fn save_account(account: &Account) -> anyhow::Result<()> {
    let path = account_path();
    let doc = json!({ "account": account, "fetchedAt": now_ms() });
    atomic_write(&path, serde_json::to_string_pretty(&doc)?.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

fn forget() {
    let _ = std::fs::remove_file(token_path());
    let _ = std::fs::remove_file(account_path());
}

// ---------------------------------------------------------------- the provider

/// The `justsuperhuman` provider while signed in: Anthropic Messages at the account's Claude base URL,
/// the account's models, the stored token as its key (see `Config::key_for`).
pub fn provider() -> Option<Provider> {
    let stored = stored()?;
    let account = cached_account().map(|(a, _)| a).unwrap_or_default();
    let base = Some(account.claude.base_url.trim().to_string()).filter(|b| !b.is_empty()).unwrap_or_else(|| format!("{}/api/claude", stored.origin.trim_end_matches('/')));
    let models = account
        .claude
        .models
        .iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| Model { id: m.id.clone(), name: m.name.clone(), context: None, extra: Default::default() })
        .collect();
    let mut extra = serde_json::Map::new();
    extra.insert("section".into(), json!("main"));
    Some(Provider {
        id: PROVIDER_ID.into(),
        name: PROVIDER_NAME.into(),
        mode: ProviderMode::Anthropic,
        base_url: Some(base),
        responses_base_url: None,
        key_env: None,
        key_url: None,
        no_key: false,
        disable_1m_context: false,
        env: Default::default(),
        models,
        extra,
    })
}

/// The model a launch without a pick uses: the account's first.
pub fn default_model() -> Option<String> {
    provider()?.models.into_iter().map(|m| m.id).find(|id| !id.is_empty())
}

// ---------------------------------------------------------------- the sign-in flow

/// How a sign-in ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done { token: String, account: Box<Account> },
    Denied,
    Expired,
    Cancelled,
    Failed(String),
}

/// Poll until the person decides, honouring `interval` and `slow_down`. Time is counted in the waits the
/// loop asked for, so a test with a fake `sleep` runs in no time.
pub fn drive(start: &DeviceStart, poll: &mut dyn FnMut() -> anyhow::Result<Poll>, sleep: &mut dyn FnMut(Duration), cancelled: &dyn Fn() -> bool) -> Outcome {
    let mut interval = start.interval.max(1);
    let mut waited = 0u64;
    let mut failures = 0u32;
    loop {
        if cancelled() {
            return Outcome::Cancelled;
        }
        sleep(Duration::from_secs(interval));
        waited += interval;
        if cancelled() {
            return Outcome::Cancelled;
        }
        match poll() {
            Ok(Poll::Pending) => failures = 0,
            Ok(Poll::SlowDown) => {
                failures = 0;
                interval += 5;
            }
            Ok(Poll::Denied) => return Outcome::Denied,
            Ok(Poll::Expired) => return Outcome::Expired,
            Ok(Poll::Done { token, account }) => return Outcome::Done { token, account },
            Err(e) => {
                failures += 1;
                if failures >= MAX_POLL_FAILURES {
                    return Outcome::Failed(format!("{e:#}"));
                }
            }
        }
        if waited >= start.expires_in {
            return Outcome::Expired;
        }
    }
}

/// A sign-in waiting for the person, as the windows and the phone show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    /// ms since the epoch
    pub expires_at: i64,
}

/// Everything a view needs, copied out of the lock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub signed_in: bool,
    pub account: Option<Account>,
    /// When `account` was fetched (ms), 0 when never.
    pub fetched_at: i64,
    pub pending: Option<Pending>,
    /// What went wrong last, in plain English. Cleared by the next success or sign-in attempt.
    pub error: Option<String>,
    pub refreshing: bool,
}

#[derive(Default)]
struct Inner {
    pending: Option<Pending>,
    cancel: Option<Arc<AtomicBool>>,
    error: Option<String>,
    refreshing: bool,
    /// bumped by every sign-in attempt, so a flow that was replaced can't write its result
    attempt: u64,
}

/// The process's one sign-in: flow state plus a notifier for whoever shows it.
pub struct Login {
    inner: Mutex<Inner>,
    notify: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// The process-wide [`Login`].
pub fn login() -> &'static Login {
    static LOGIN: OnceLock<Login> = OnceLock::new();
    LOGIN.get_or_init(|| Login { inner: Mutex::new(Inner::default()), notify: Mutex::new(None) })
}

/// What `sign_out` did.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedOut {
    /// The server confirmed the token is revoked. False when it couldn't be reached: the token is gone from
    /// this computer, but the person should revoke the device on their Account page.
    pub revoked: bool,
    pub was_signed_in: bool,
}

impl Login {
    /// Called after every change (signed in or out, a new account reading, a flow ending). Replaces the last one.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.notify.lock() = Some(Arc::new(f));
    }

    fn changed(&self) {
        let f = self.notify.lock().clone();
        if let Some(f) = f {
            f();
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let signed_in = signed_in();
        let (account, fetched_at) = if signed_in { cached_account().map(|(a, t)| (Some(a), t)).unwrap_or((None, 0)) } else { (None, 0) };
        let inner = self.inner.lock();
        Snapshot { signed_in, account, fetched_at, pending: inner.pending.clone(), error: inner.error.clone(), refreshing: inner.refreshing }
    }

    /// Start signing in, or return the sign-in already waiting (the second element is true for a new one).
    /// Blocks for the first request only; the polling runs on its own thread. Fails when the server can't be
    /// reached.
    pub fn begin(&'static self) -> anyhow::Result<(Pending, bool)> {
        self.begin_at(&origin())
    }

    fn begin_at(&'static self, origin: &str) -> anyhow::Result<(Pending, bool)> {
        {
            let inner = self.inner.lock();
            if let Some(p) = inner.pending.clone().filter(|p| p.expires_at > now_ms()) {
                return Ok((p, false));
            }
        }
        let start = match start_device(origin) {
            Ok(s) => s,
            Err(e) => {
                self.inner.lock().error = Some(format!("{e:#}"));
                self.changed();
                return Err(e);
            }
        };
        let pending = Pending {
            user_code: start.user_code.clone(),
            verification_uri: start.verification_uri.clone(),
            verification_uri_complete: start.verification_uri_complete.clone(),
            expires_at: now_ms() + start.expires_in as i64 * 1000,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let attempt = {
            let mut inner = self.inner.lock();
            if let Some(old) = inner.cancel.replace(cancel.clone()) {
                old.store(true, Ordering::Release);
            }
            inner.attempt += 1;
            inner.pending = Some(pending.clone());
            inner.error = None;
            inner.attempt
        };
        self.changed();
        let origin = origin.to_string();
        std::thread::spawn(move || {
            let me = login();
            let mut poll = || poll_token(&origin, &start.device_code);
            let flag = cancel.clone();
            let mut sleep = move |d: Duration| {
                let until = std::time::Instant::now() + d;
                while std::time::Instant::now() < until && !flag.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(200).min(until.saturating_duration_since(std::time::Instant::now())));
                }
            };
            let flag = cancel.clone();
            let outcome = drive(&start, &mut poll, &mut sleep, &move || flag.load(Ordering::Acquire));
            me.finish(attempt, &origin, outcome);
        });
        Ok((pending, true))
    }

    fn finish(&self, attempt: u64, origin: &str, outcome: Outcome) {
        {
            let mut inner = self.inner.lock();
            if inner.attempt != attempt {
                return;
            }
            inner.pending = None;
            inner.cancel = None;
            inner.error = match &outcome {
                Outcome::Done { .. } | Outcome::Cancelled => None,
                Outcome::Denied => Some("The sign-in was declined in the browser. Start again when you're ready.".into()),
                Outcome::Expired => Some("That code expired before it was approved. Start again for a fresh one.".into()),
                Outcome::Failed(e) => Some(e.clone()),
            };
        }
        if let Outcome::Done { token, account } = outcome {
            match save_token(&token, origin).and_then(|_| save_account(&account)) {
                Ok(()) => {}
                Err(e) => self.inner.lock().error = Some(format!("Signed in, but bro couldn't save the sign-in on this computer: {e:#}")),
            }
        }
        self.changed();
    }

    /// Stop waiting for an approval.
    pub fn cancel(&self) {
        {
            let mut inner = self.inner.lock();
            if let Some(c) = inner.cancel.take() {
                c.store(true, Ordering::Release);
            }
            inner.attempt += 1;
            inner.pending = None;
        }
        self.changed();
    }

    /// Revoke the token on the server and forget it here. Without a server answer it is forgotten here anyway.
    pub fn sign_out(&self) -> SignedOut {
        self.cancel();
        let Some(stored) = stored() else {
            return SignedOut { revoked: true, was_signed_in: false };
        };
        let result = revoke_token(&stored.origin, &stored.token);
        forget();
        self.inner.lock().error = result.as_ref().err().map(|_| {
            "Signed out here, but Just Superhuman couldn't be reached to revoke this device. Revoke it under Account > Devices on console.justsuperhuman.com.".to_string()
        });
        self.changed();
        SignedOut { revoked: result.is_ok(), was_signed_in: true }
    }

    /// Ask the server for the account now (blocking). A 401 signs out here and says so; a failed request keeps the
    /// last reading and notes why.
    pub fn refresh(&self) -> anyhow::Result<()> {
        let Some(stored) = stored() else { return Ok(()) };
        {
            let mut inner = self.inner.lock();
            if inner.refreshing {
                return Ok(());
            }
            inner.refreshing = true;
        }
        self.changed();
        let result = fetch_account(&stored.origin, &stored.token);
        let still_ours = self::stored().is_some_and(|s| s.token == stored.token);
        let out = match result {
            _ if !still_ours => Ok(()),
            Ok(Fetched::Account(account)) => {
                let saved = save_account(&account);
                self.inner.lock().error = saved.as_ref().err().map(|e| format!("{e:#}"));
                if saved.is_ok() { Ok(()) } else { saved }
            }
            Ok(Fetched::Revoked) => {
                forget();
                self.inner.lock().error = Some("Just Superhuman signed this computer out because the device was revoked. Sign in again to keep using it.".into());
                Ok(())
            }
            Err(e) => {
                self.inner.lock().error = Some(format!("{e:#}"));
                Err(e)
            }
        };
        self.inner.lock().refreshing = false;
        self.changed();
        out
    }

    /// [`Login::refresh`] when the last reading is older than `max_age`.
    pub fn refresh_if_stale(&self, max_age: Duration) {
        if !signed_in() {
            return;
        }
        let fetched = cached_account().map(|(_, t)| t).unwrap_or(0);
        if now_ms() - fetched >= max_age.as_millis() as i64 {
            let _ = self.refresh();
        }
    }
}

/// Keep the account fresh while bro runs: a reading shortly after start, then every few minutes. Once per process.
pub fn spawn_refresher() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(|| {
        loop {
            login().refresh_if_stale(REFRESH_EVERY);
            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

/// "in 12 days", "tomorrow", "today": when the allowance renews, for a card.
pub fn renews_text(resets_at_ms: i64) -> String {
    let secs = (resets_at_ms - now_ms()) / 1000;
    match secs {
        s if s <= 0 => "renews today".into(),
        s if s < 86_400 => "renews today".into(),
        s if s < 2 * 86_400 => "renews tomorrow".into(),
        s => format!("renews in {} days", s / 86_400),
    }
}

#[cfg(test)]
mod tests;
