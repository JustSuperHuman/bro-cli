//! Gemini on a Google AI subscription: Google sign-in with the Antigravity CLI's public OAuth
//! client (PKCE + a loopback callback), token refresh, and the Cloud Code Assist project
//! requests bill to. bro-proxy does the talking; [`auth`] hands it a token and the project.
//!
//! Stored in `~/.bro/gemini/oauth.json` (bro's own file; v1 has nothing like it).
//!
//! Google's terms only allow this sign-in in Google's own apps; `bro gemini login` says so
//! before it opens the browser.
use crate::creds::{dir_lock, restrict_permissions};
use crate::http::{JsonResponse, get_json, post_form, post_json_with};
use crate::util::{env_str, now_ms, read_json, write_json_pretty};
use anyhow::{Context, anyhow, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The Google OAuth client bro signs in with: Antigravity's installed-app client. bro doesn't ship it; set
/// `BRO_GEMINI_CLIENT_ID` and `BRO_GEMINI_CLIENT_SECRET` (in your environment, or an env file with `bro --env`).
pub fn client() -> anyhow::Result<(String, String)> {
    let get = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    match (get("BRO_GEMINI_CLIENT_ID"), get("BRO_GEMINI_CLIENT_SECRET")) {
        (Some(id), Some(secret)) => Ok((id, secret)),
        _ => bail!("set BRO_GEMINI_CLIENT_ID and BRO_GEMINI_CLIENT_SECRET (Antigravity's installed-app Google client) to sign in with Google; bro doesn't ship them"),
    }
}
pub const SCOPES: [&str; 5] = [
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json";
/// Cloud Code Assist hosts, tried in order (the CLI talks to `daily-` first).
pub const CODE_ASSIST_BASES: [&str; 2] = ["https://daily-cloudcode-pa.googleapis.com", "https://cloudcode-pa.googleapis.com"];
/// Antigravity CLI version bro presents as. Argon is only offered to CLI 1.2.8 and later.
pub const CLI_VERSION: &str = "1.4.0";
/// Where Google sends the browser back to (any free port when this one is taken).
const CALLBACK_PORT: u16 = 51121;
const CALLBACK_PATH: &str = "/oauth-callback";
/// How long the browser gets to finish the sign-in.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const EXPIRY_MARGIN_MS: i64 = 60_000;

/// Shown wherever a launch needs the sign-in.
pub const SIGN_IN_HINT: &str =
    "gemini isn't signed in: run `bro gemini login` (or pick gemini in the launcher and press enter) to sign in with your Google account";

/// `~/.bro/gemini/oauth.json`.
pub fn creds_path() -> PathBuf {
    crate::paths::bro_dir().join("gemini").join("oauth.json")
}

/// Code Assist hosts to try (`BRO_GEMINI_BASE_URL` replaces them).
pub fn base_urls() -> Vec<String> {
    match env_str("BRO_GEMINI_BASE_URL") {
        Some(u) => vec![u.trim_end_matches('/').to_string()],
        None => CODE_ASSIST_BASES.iter().map(|s| s.to_string()).collect(),
    }
}

/// The Antigravity CLI's User-Agent (`BRO_GEMINI_USER_AGENT` overrides). Code Assist only
/// serves Argon to the CLI.
pub fn user_agent() -> String {
    if let Some(ua) = env_str("BRO_GEMINI_USER_AGENT") {
        return ua;
    }
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("antigravity/cli/{CLI_VERSION} (aidev_client; os_type={os}; arch={arch}; cl=982146307; auth_method=consumer)")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    /// ms since the epoch
    #[serde(default)]
    expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    /// Cloud Code Assist project the requests bill to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier_name: Option<String>,
}

fn load(path: &Path) -> Option<Stored> {
    serde_json::from_value(read_json(path)?).ok()
}

fn save(path: &Path, s: &Stored) -> anyhow::Result<()> {
    write_json_pretty(path, s).with_context(|| format!("writing {}", path.display()))?;
    restrict_permissions(path);
    Ok(())
}

/// Who is signed in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Account {
    pub email: Option<String>,
    pub project: Option<String>,
    /// Code Assist tier id (`standard-tier`, `free-tier`…)
    pub tier: Option<String>,
    /// Its display name ("Google AI Ultra"…), when Google sent one
    pub tier_name: Option<String>,
}

impl From<&Stored> for Account {
    fn from(s: &Stored) -> Self {
        Account { email: s.email.clone(), project: s.project.clone(), tier: s.tier.clone(), tier_name: s.tier_name.clone() }
    }
}

/// What bro-proxy needs per request.
#[derive(Debug, Clone, PartialEq)]
pub struct GeminiAuth {
    pub access_token: String,
    pub project: String,
}

/// A sign-in that can still refresh. Reads one small file.
pub fn signed_in(path: &Path) -> bool {
    load(path).is_some_and(|s| !s.refresh_token.is_empty())
}

pub fn account(path: &Path) -> Option<Account> {
    load(path).filter(|s| !s.refresh_token.is_empty()).map(|s| Account::from(&s))
}

/// Forget the sign-in. Ok(false) when there was none.
pub fn logout(path: &Path) -> anyhow::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// A valid access token and the Code Assist project, refreshing when expired or `force`
/// (after a 401) and finding the project when it isn't known yet. Blocking.
pub fn auth(path: &Path, force: bool) -> anyhow::Result<GeminiAuth> {
    let lock = dir_lock(path);
    let _guard = lock.lock();
    let mut s = load(path).filter(|s| !s.refresh_token.is_empty()).ok_or_else(|| anyhow!("{SIGN_IN_HINT}"))?;
    let mut dirty = false;
    if force || s.access_token.is_empty() || s.expires_at <= now_ms() + EXPIRY_MARGIN_MS {
        refresh(&mut s)?;
        dirty = true;
    }
    if s.project.as_deref().is_none_or(str::is_empty) {
        let found = discover(&s.access_token)?;
        s.project = Some(found.project);
        s.tier = found.tier.or(s.tier);
        s.tier_name = found.tier_name.or(s.tier_name);
        dirty = true;
    }
    if dirty {
        save(path, &s)?;
    }
    Ok(GeminiAuth { access_token: s.access_token.clone(), project: s.project.clone().unwrap_or_default() })
}

fn refresh(s: &mut Stored) -> anyhow::Result<()> {
    let (id, secret) = client()?;
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", s.refresh_token.as_str()),
        ("client_id", id.as_str()),
        ("client_secret", secret.as_str()),
    ];
    let resp = post_form(TOKEN_URL, &form, CALL_TIMEOUT)?;
    if !resp.ok() {
        let why = resp.body.get("error").and_then(Value::as_str).unwrap_or("");
        if why == "invalid_grant" {
            bail!("your Google sign-in expired or was revoked: run `bro gemini login` again");
        }
        bail!("Google token refresh failed (HTTP {}{})", resp.status, if why.is_empty() { String::new() } else { format!(", {why}") });
    }
    apply_tokens(s, &resp.body)
}

/// Merge a token endpoint response (refresh or code exchange) into the stored sign-in.
fn apply_tokens(s: &mut Stored, body: &Value) -> anyhow::Result<()> {
    let access = body.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| anyhow!("Google returned no access token"))?;
    s.access_token = access.to_string();
    // Google keeps the refresh token unless it sends a new one.
    if let Some(r) = body.get("refresh_token").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        s.refresh_token = r.to_string();
    }
    let secs = body.get("expires_in").and_then(Value::as_i64).unwrap_or(3600);
    s.expires_at = now_ms() + secs * 1000;
    Ok(())
}

// ---------------------------------------------------------------- Code Assist

fn ca_headers(token: &str) -> [(String, String); 2] {
    [("user-agent".into(), user_agent()), ("authorization".into(), format!("Bearer {token}"))]
}

/// POST `{base}/v1internal:{method}` on the first host that answers (a 2xx, or a 4xx that
/// isn't 404: those are about the request, not the host).
fn ca_post(token: &str, method: &str, body: &Value) -> anyhow::Result<JsonResponse> {
    let headers = ca_headers(token);
    let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut last: Option<anyhow::Result<JsonResponse>> = None;
    for base in base_urls() {
        let url = format!("{base}/v1internal:{method}");
        match post_json_with(&url, &headers, body, CALL_TIMEOUT) {
            Ok(r) if r.ok() || ((400..500).contains(&r.status) && r.status != 404) => return Ok(r),
            other => last = Some(other),
        }
    }
    last.unwrap_or_else(|| Err(anyhow!("no Code Assist host configured")))
}

fn google_error(r: &JsonResponse) -> String {
    r.body.pointer("/error/message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("HTTP {}", r.status))
}

struct Discovered {
    project: String,
    tier: Option<String>,
    tier_name: Option<String>,
}

/// `cloudaicompanionProject` is a string or `{id}`.
fn project_of(v: Option<&Value>) -> Option<String> {
    let v = v?;
    v.as_str().or_else(|| v.get("id").and_then(Value::as_str)).filter(|s| !s.is_empty()).map(str::to_string)
}

/// A tier is a string or `{id, name}`.
fn tier_of(v: Option<&Value>) -> (Option<String>, Option<String>) {
    match v {
        Some(Value::String(s)) if !s.is_empty() => (Some(s.clone()), None),
        Some(v @ Value::Object(_)) => (
            v.get("id").and_then(Value::as_str).map(str::to_string),
            v.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
        ),
        _ => (None, None),
    }
}

/// The account's Code Assist project (`loadCodeAssist`), onboarding it first when it has none.
fn discover(token: &str) -> anyhow::Result<Discovered> {
    let meta = json!({ "metadata": { "ideType": "ANTIGRAVITY" } });
    let r = ca_post(token, "loadCodeAssist", &meta)?;
    if !r.ok() {
        bail!("Google Code Assist refused this account: {}", google_error(&r));
    }
    let (mut tier, mut tier_name) = tier_of(r.body.get("paidTier"));
    if tier.is_none() {
        (tier, tier_name) = tier_of(r.body.get("currentTier"));
    }
    if let Some(project) = project_of(r.body.get("cloudaicompanionProject")) {
        return Ok(Discovered { project, tier, tier_name });
    }
    let onboard_tier = r
        .body
        .get("allowedTiers")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|t| t.get("isDefault").and_then(Value::as_bool) == Some(true)))
        .and_then(|t| t.get("id").and_then(Value::as_str))
        .unwrap_or("free-tier")
        .to_string();
    let body = json!({ "tierId": onboard_tier, "metadata": { "ideType": "ANTIGRAVITY" } });
    for attempt in 0..10 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(5));
        }
        let r = ca_post(token, "onboardUser", &body)?;
        if !r.ok() {
            bail!("Google couldn't set up Code Assist for this account: {}", google_error(&r));
        }
        if let Some(project) = project_of(r.body.pointer("/response/cloudaicompanionProject")) {
            return Ok(Discovered { project, tier: tier.or(Some(onboard_tier)), tier_name });
        }
    }
    let why = r
        .body
        .get("ineligibleTiers")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find_map(|t| t.get("reasonMessage").and_then(Value::as_str)))
        .map(|m| format!(": {m}"))
        .unwrap_or_default();
    bail!("Google didn't give this account a Code Assist project{why}")
}

/// Quota left on one model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelQuota {
    /// Wire id (`gemini-4-argon-medium`)
    pub id: String,
    pub name: Option<String>,
    /// 0.0..=1.0
    pub remaining: Option<f64>,
    /// RFC 3339
    pub reset: Option<String>,
}

/// Models the subscription offers in Antigravity's picker, with quota left. Blocking.
pub fn quotas(path: &Path) -> anyhow::Result<Vec<ModelQuota>> {
    let a = auth(path, false)?;
    let r = ca_post(&a.access_token, "fetchAvailableModels", &json!({ "project": a.project }))?;
    if !r.ok() {
        bail!("{}", google_error(&r));
    }
    Ok(parse_quotas(&r.body))
}

fn parse_quotas(body: &Value) -> Vec<ModelQuota> {
    let Some(models) = body.get("models").and_then(Value::as_object) else { return vec![] };
    // `models` also lists retired entries; the picker groups are what's on offer.
    let picked: Vec<&str> = body
        .get("agentModelSorts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|s| s.get("groups").and_then(Value::as_array).into_iter().flatten())
        .flat_map(|g| g.get("modelIds").and_then(Value::as_array).into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let mut out: Vec<ModelQuota> = models
        .iter()
        .filter(|(id, _)| picked.is_empty() || picked.contains(&id.as_str()))
        .map(|(id, m)| ModelQuota {
            id: id.clone(),
            name: m.get("displayName").and_then(Value::as_str).map(str::to_string),
            remaining: m.pointer("/quotaInfo/remainingFraction").and_then(Value::as_f64),
            reset: m.pointer("/quotaInfo/resetTime").and_then(Value::as_str).map(str::to_string),
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

// ---------------------------------------------------------------- sign-in

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// (verifier, S256 challenge)
fn pkce() -> (String, String) {
    let verifier = b64url(&rand::random::<[u8; 32]>());
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn auth_url(client_id: &str, redirect: &str, challenge: &str, state: &str) -> String {
    let scope = SCOPES.join(" ");
    url::Url::parse_with_params(
        AUTH_URL,
        [
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", redirect),
            ("scope", scope.as_str()),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map(String::from)
    .unwrap_or_default()
}

/// Sign in with Google in the browser and save the result to `path`. `open` shows the URL
/// (returns false when no browser opened; the URL is still valid to paste); `say` reports
/// progress. Blocks until the browser comes back, up to 5 minutes.
pub fn login(path: &Path, open: &dyn Fn(&str) -> bool, say: &dyn Fn(&str)) -> anyhow::Result<Account> {
    let (id, secret) = client()?;
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .context("starting the sign-in callback listener")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let (verifier, challenge) = pkce();
    let state = b64url(&rand::random::<[u8; 16]>());
    open(&auth_url(&id, &redirect, &challenge, &state));
    say("waiting for you to finish signing in in the browser…");
    let code = wait_for_code(&listener, &state)?;

    say("signed in; fetching tokens…");
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("client_id", id.as_str()),
        ("client_secret", secret.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    let resp = post_form(TOKEN_URL, &form, CALL_TIMEOUT)?;
    if !resp.ok() {
        let why = resp.body.get("error_description").or_else(|| resp.body.get("error")).and_then(Value::as_str).unwrap_or("");
        bail!("Google wouldn't exchange the sign-in code (HTTP {}) {why}", resp.status);
    }
    let mut s = Stored::default();
    apply_tokens(&mut s, &resp.body)?;
    if s.refresh_token.is_empty() {
        bail!("Google returned no refresh token; sign in again and allow offline access");
    }
    let bearer = format!("Bearer {}", s.access_token);
    s.email = get_json(USERINFO_URL, &[("authorization", bearer.as_str())], CALL_TIMEOUT)
        .ok()
        .and_then(|r| r.body.get("email").and_then(Value::as_str).map(str::to_string));

    say("finding your Code Assist project…");
    let found = discover(&s.access_token)?;
    s.project = Some(found.project);
    s.tier = found.tier;
    s.tier_name = found.tier_name;
    save(path, &s)?;
    Ok(Account::from(&s))
}

/// Serve the loopback callback until Google redirects back with a code (or an error).
fn wait_for_code(listener: &TcpListener, state: &str) -> anyhow::Result<String> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    while Instant::now() < deadline {
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e).context("waiting for the browser"),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Some(target) = read_request_target(&mut stream) else { continue };
        let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else { continue };
        if url.path() != CALLBACK_PATH {
            respond(&mut stream, "404 Not Found", "Not found", "This isn't the page you're looking for.");
            continue;
        }
        let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
        if let Some(err) = q("error") {
            respond(&mut stream, "200 OK", "Sign-in cancelled", "Google didn't sign you in. You can close this tab and run bro gemini login again.");
            bail!("Google sign-in failed: {err}");
        }
        if q("state").as_deref() != Some(state) {
            respond(&mut stream, "400 Bad Request", "Sign-in didn't match", "This reply belongs to a different sign-in. Close this tab and start again from bro.");
            continue;
        }
        let Some(code) = q("code").filter(|c| !c.is_empty()) else {
            respond(&mut stream, "400 Bad Request", "No sign-in code", "Google came back without a code. Close this tab and start again from bro.");
            continue;
        };
        respond(&mut stream, "200 OK", "You're signed in", "bro has your Google sign-in. Close this tab and head back to the terminal.");
        return Ok(code);
    }
    bail!("the sign-in timed out after 5 minutes; run `bro gemini login` to try again")
}

/// `GET /path?query HTTP/1.1` → `/path?query`.
fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut parts = text.lines().next()?.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_string))?
}

fn respond(stream: &mut TcpStream, status: &str, title: &str, body: &str) {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title} · bro</title>\
         <style>body{{font-family:Inter,Poppins,system-ui,sans-serif;background:#0f1115;color:#e8e8ea;display:flex;\
         align-items:center;justify-content:center;height:100vh;margin:0}}main{{background:#181b22;padding:40px 48px;\
         border-radius:16px;box-shadow:0 12px 40px rgba(0,0,0,.4);max-width:420px}}h1{{font-size:22px;margin:0 0 12px}}\
         p{{color:#a9adb8;line-height:1.5;margin:0}}</style></head><body><main><h1>{title}</h1><p>{body}</p></main></body></html>"
    );
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let (v, c) = pkce();
        assert_eq!(v.len(), 43);
        assert_eq!(c, b64url(&Sha256::digest(v.as_bytes())));
    }

    #[test]
    fn auth_url_has_pkce_and_offline() {
        let u = url::Url::parse(&auth_url("test-client.apps.googleusercontent.com", "http://127.0.0.1:51121/oauth-callback", "CH", "ST")).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "test-client.apps.googleusercontent.com");
        assert_eq!(q["code_challenge"], "CH");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["access_type"], "offline");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:51121/oauth-callback");
        assert!(q["scope"].contains("cloud-platform"));
    }

    #[test]
    fn stored_round_trip_and_logout() {
        let sb = sandbox();
        let path = sb.bro().join("gemini").join("oauth.json");
        assert!(!signed_in(&path));
        let mut s = Stored { refresh_token: "r".into(), email: Some("a@b.c".into()), ..Default::default() };
        apply_tokens(&mut s, &json!({ "access_token": "at", "expires_in": 3599 })).unwrap();
        assert_eq!(s.refresh_token, "r");
        assert!(s.expires_at > now_ms());
        save(&path, &s).unwrap();
        assert!(signed_in(&path));
        assert_eq!(account(&path).unwrap().email.as_deref(), Some("a@b.c"));
        assert!(logout(&path).unwrap());
        assert!(!logout(&path).unwrap());
    }

    #[test]
    fn project_and_tier_shapes() {
        assert_eq!(project_of(Some(&json!("p-1"))).as_deref(), Some("p-1"));
        assert_eq!(project_of(Some(&json!({ "id": "p-2" }))).as_deref(), Some("p-2"));
        assert_eq!(project_of(Some(&json!(""))), None);
        assert_eq!(tier_of(Some(&json!({ "id": "standard-tier", "name": "Google AI Ultra" }))), (Some("standard-tier".into()), Some("Google AI Ultra".into())));
    }

    #[test]
    fn quotas_follow_the_picker() {
        let body = json!({
            "models": {
                "gemini-4-argon-high": { "displayName": "Gemini 4 Argon (High)", "quotaInfo": { "remainingFraction": 0.75, "resetTime": "2026-10-24T00:00:00Z" } },
                "gemini-2.5-pro": { "displayName": "retired" }
            },
            "agentModelSorts": [ { "groups": [ { "modelIds": ["gemini-4-argon-high"] } ] } ]
        });
        let q = parse_quotas(&body);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].id, "gemini-4-argon-high");
        assert_eq!(q[0].remaining, Some(0.75));
    }

    #[test]
    fn user_agent_is_the_cli() {
        unsafe { std::env::remove_var("BRO_GEMINI_USER_AGENT") };
        assert!(user_agent().starts_with("antigravity/cli/"));
    }
}
