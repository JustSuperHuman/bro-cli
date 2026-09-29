//! OAuth credentials for Claude and Codex logins, with refresh + write-back.
//! Used by usage fetching AND by bro-proxy (upstream auth), so it must be
//! thread-safe: refreshes for the same dir are serialized by an internal lock.
//!
//! Storage is the harness's own (v1 `claude-oauth-bridge.js` / `codex-auth.js`):
//! Claude's `<dir>/.credentials.json` (`claudeAiOauth`) and Codex's `<home>/auth.json`.
//! Rotated tokens are written back atomically with every other key preserved, so the
//! CLI that owns the login keeps seeing one coherent credential.
use crate::paths;
use crate::util::{atomic_write, env_str, jwt_payload, now_ms, path_key, read_json, str_of};
use anyhow::{Context, anyhow, bail};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Claude's public OAuth client id (override `CLAUDE_OAUTH_CLIENT_ID`).
pub const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Claude token endpoint (override `CLAUDE_OAUTH_TOKEN_URL`).
pub const CLAUDE_OAUTH_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// Codex CLI's public OAuth client id.
pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Codex / ChatGPT token endpoint.
pub const CODEX_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

/// Refresh this long before the recorded expiry, so a token never dies mid-request.
const EXPIRY_MARGIN_MS: i64 = 60_000;
const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);

/// One lock per credential directory: concurrent callers (proxy requests, usage
/// fetches) wait for a single refresh instead of racing refresh-token rotation.
fn dir_lock(dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(Default::default).lock().entry(path_key(dir)).or_default().clone()
}

// ---------------------------------------------------------------- Claude

/// `<dir>/.credentials.json`.
pub fn claude_credentials_path(config_dir: &Path) -> PathBuf {
    config_dir.join(".credentials.json")
}

/// The parsed Claude credential document (`None` when absent/malformed). On macOS a
/// login without a file falls back to the Keychain item Claude Code uses.
pub fn read_claude_credentials(config_dir: &Path) -> Option<Value> {
    read_json(&claude_credentials_path(config_dir)).or_else(|| keychain::read(config_dir))
}

fn write_claude_credentials(config_dir: &Path, doc: &Value) -> anyhow::Result<()> {
    let path = claude_credentials_path(config_dir);
    if !path.exists() && keychain::write(config_dir, doc) {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(doc)?;
    atomic_write(&path, text.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
    restrict_permissions(&path);
    Ok(())
}

/// Returns a valid Claude OAuth access token for `config_dir`
/// (`<dir>/.credentials.json` → `claudeAiOauth`), refreshing via
/// `POST https://platform.claude.com/v1/oauth/token` when expired or `force_refresh`.
pub fn claude_access_token(config_dir: &Path, force_refresh: bool) -> anyhow::Result<String> {
    let lock = dir_lock(config_dir);
    let _guard = lock.lock();
    // Read inside the lock: a refresh that finished while we waited is picked up here.
    let mut doc = read_claude_credentials(config_dir)
        .ok_or_else(|| anyhow!("no Claude login in {} (run claude and /login)", config_dir.display()))?;
    let oauth = doc.get("claudeAiOauth").ok_or_else(|| anyhow!("no claudeAiOauth credentials in {}", config_dir.display()))?;
    let token = str_of(oauth, "accessToken").filter(|t| !t.is_empty()).ok_or_else(|| anyhow!("missing OAuth access token"))?;
    let expires_at = oauth.get("expiresAt").and_then(Value::as_f64).map(|v| v as i64);
    let fresh = expires_at.is_none_or(|exp| exp > now_ms() + EXPIRY_MARGIN_MS);
    if fresh && !force_refresh {
        return Ok(token.to_string());
    }
    refresh_claude(config_dir, &mut doc)
}

fn refresh_claude(config_dir: &Path, doc: &mut Value) -> anyhow::Result<String> {
    let refresh = doc
        .pointer("/claudeAiOauth/refreshToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow!("missing OAuth refresh token"))?
        .to_string();
    let url = env_str("CLAUDE_OAUTH_TOKEN_URL").unwrap_or_else(|| CLAUDE_OAUTH_TOKEN_URL.into());
    let client_id = env_str("CLAUDE_OAUTH_CLIENT_ID").unwrap_or_else(|| CLAUDE_OAUTH_CLIENT_ID.into());
    let body = json!({ "grant_type": "refresh_token", "refresh_token": refresh, "client_id": client_id });
    let resp = crate::http::post_json(&url, &body, REFRESH_TIMEOUT)?;
    if !resp.ok() {
        bail!("OAuth refresh failed ({})", resp.status);
    }
    let access = str_of(&resp.body, "access_token").ok_or_else(|| anyhow!("OAuth refresh returned no access token"))?.to_string();
    apply_claude_refresh(doc, &resp.body);
    write_claude_credentials(config_dir, doc)?;
    Ok(access)
}

/// Fold a token-endpoint response into a credential document (v1 field semantics).
pub(crate) fn apply_claude_refresh(doc: &mut Value, body: &Value) {
    if !doc.is_object() {
        *doc = json!({});
    }
    let Some(root) = doc.as_object_mut() else { return };
    let oauth = root.entry("claudeAiOauth").or_insert_with(|| json!({}));
    let Some(o) = oauth.as_object_mut() else { return };
    if let Some(a) = str_of(body, "access_token") {
        o.insert("accessToken".into(), json!(a));
    }
    if let Some(r) = str_of(body, "refresh_token").filter(|r| !r.is_empty()) {
        o.insert("refreshToken".into(), json!(r));
    }
    let expires_in = body.get("expires_in").and_then(Value::as_f64).unwrap_or(3600.0);
    o.insert("expiresAt".into(), json!(now_ms() + (expires_in * 1000.0) as i64));
    if let Some(scope) = str_of(body, "scope") {
        o.insert("scopes".into(), json!(scope.split_whitespace().collect::<Vec<_>>()));
    }
}

/// Summary of a Claude login, read from disk only (no network).
#[derive(Debug, Clone, Default)]
pub struct ClaudeLogin {
    pub authenticated: bool,
    /// `subscriptionType` ("max", "pro", "team"…)
    pub plan: Option<String>,
    /// `rateLimitTier`
    pub tier: Option<String>,
    /// unix ms
    pub expires_at: Option<i64>,
    /// `accountUuid:organizationUuid` from `.claude.json` (v1 `claudeIdentity`)
    pub identity: Option<String>,
    pub email: Option<String>,
}

/// Read a Claude config dir's login state (v1 `listAccounts` + `claudeIdentity`).
/// For the machine's own `~/.claude`, `.claude.json` is also looked for in `$HOME`,
/// where Claude Code keeps it when `CLAUDE_CONFIG_DIR` is unset.
pub fn claude_login(config_dir: &Path) -> ClaudeLogin {
    let mut out = ClaudeLogin::default();
    if let Some(oauth) = read_claude_credentials(config_dir).and_then(|d| d.get("claudeAiOauth").cloned()) {
        out.authenticated = str_of(&oauth, "accessToken").is_some_and(|t| !t.is_empty());
        out.plan = str_of(&oauth, "subscriptionType").map(str::to_string);
        out.tier = str_of(&oauth, "rateLimitTier").map(str::to_string);
        out.expires_at = oauth.get("expiresAt").and_then(Value::as_f64).map(|v| v as i64);
    }
    let mut candidates = vec![config_dir.join(".claude.json")];
    if crate::util::same_path(config_dir, &paths::claude_local_dir()) {
        candidates.push(crate::util::home_dir().join(".claude.json"));
    }
    for file in candidates {
        let Some(account) = read_json(&file).and_then(|d| d.get("oauthAccount").cloned()) else { continue };
        if let Some(uuid) = str_of(&account, "accountUuid").filter(|s| !s.is_empty()) {
            out.identity = Some(format!("{uuid}:{}", str_of(&account, "organizationUuid").unwrap_or("")));
        }
        out.email = str_of(&account, "emailAddress").map(str::to_string);
        break;
    }
    out
}

// ---------------------------------------------------------------- Codex

#[derive(Debug, Clone)]
pub struct CodexAuth {
    pub access_token: String,
    pub account_id: Option<String>,
    /// Present when the login is an API-key login rather than ChatGPT OAuth
    pub api_key: Option<String>,
}

/// bro v1's own fallback store for the machine's Codex login (`~/.bro/codex-auth.json`).
pub fn bro_codex_auth_path() -> PathBuf {
    paths::bro_dir().join("codex-auth.json")
}

/// Where credentials are looked for, in order (v1 `authPaths`): a profile keeps its
/// own `auth.json`; the machine's own login checks bro's file first, then the CLI's.
pub fn codex_auth_paths(codex_home: &Path) -> Vec<PathBuf> {
    if crate::util::same_path(codex_home, &paths::codex_local_dir()) {
        vec![bro_codex_auth_path(), codex_home.join("auth.json")]
    } else {
        vec![codex_home.join("auth.json")]
    }
}

struct StoredCodex {
    path: PathBuf,
    doc: Value,
}

fn load_codex(codex_home: &Path) -> Option<StoredCodex> {
    let mut api_key_login = None;
    for path in codex_auth_paths(codex_home) {
        let Some(doc) = read_json(&path) else { continue };
        let tokens = doc.get("tokens");
        let has = |k: &str| tokens.and_then(|t| str_of(t, k)).is_some_and(|s| !s.is_empty());
        if has("access_token") && has("refresh_token") {
            return Some(StoredCodex { path, doc });
        }
        if api_key_login.is_none() && str_of(&doc, "OPENAI_API_KEY").is_some_and(|k| !k.is_empty()) {
            api_key_login = Some(StoredCodex { path, doc });
        }
    }
    api_key_login
}

fn account_id_from_tokens(tokens: &Value) -> Option<String> {
    if let Some(id) = str_of(tokens, "account_id").filter(|s| !s.is_empty()) {
        return Some(id.to_string());
    }
    for key in ["id_token", "access_token"] {
        let claim = str_of(tokens, key).and_then(jwt_payload);
        if let Some(id) = claim.as_ref().and_then(|c| c.pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")).and_then(Value::as_str) {
            return Some(id.to_string());
        }
    }
    None
}

fn auth_from(doc: &Value) -> Option<CodexAuth> {
    if let Some(tokens) = doc.get("tokens")
        && let Some(access) = str_of(tokens, "access_token").filter(|s| !s.is_empty())
    {
        return Some(CodexAuth { access_token: access.into(), account_id: account_id_from_tokens(tokens), api_key: None });
    }
    let key = str_of(doc, "OPENAI_API_KEY").filter(|k| !k.is_empty())?;
    Some(CodexAuth { access_token: key.into(), account_id: None, api_key: Some(key.into()) })
}

/// Reads `<home>/auth.json` (ChatGPT tokens or OPENAI_API_KEY), refreshing tokens via
/// `https://auth.openai.com/oauth/token` when expired or `force_refresh`.
pub fn codex_auth(codex_home: &Path, force_refresh: bool) -> anyhow::Result<CodexAuth> {
    let lock = dir_lock(codex_home);
    let _guard = lock.lock();
    let mut stored = load_codex(codex_home)
        .ok_or_else(|| anyhow!("not logged in to Codex in {} (run codex login)", codex_home.display()))?;
    let auth = auth_from(&stored.doc).ok_or_else(|| anyhow!("unreadable Codex credentials in {}", stored.path.display()))?;
    if auth.api_key.is_some() {
        return Ok(auth);
    }
    let exp = jwt_payload(&auth.access_token).and_then(|c| c.get("exp").and_then(Value::as_i64));
    let expiring = exp.is_some_and(|e| e * 1000 < now_ms() + EXPIRY_MARGIN_MS);
    if !force_refresh && !expiring {
        return Ok(auth);
    }
    refresh_codex(&mut stored)?;
    auth_from(&stored.doc).ok_or_else(|| anyhow!("Codex token refresh produced no token"))
}

fn refresh_codex(stored: &mut StoredCodex) -> anyhow::Result<()> {
    let refresh = stored
        .doc
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing Codex refresh token"))?
        .to_string();
    let body = json!({
        "client_id": CODEX_OAUTH_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "scope": "openid profile email",
    });
    let resp = crate::http::post_json(CODEX_OAUTH_TOKEN_URL, &body, REFRESH_TIMEOUT)?;
    if !resp.ok() {
        bail!("Codex token refresh failed (HTTP {}). Run: codex login", resp.status);
    }
    if str_of(&resp.body, "access_token").is_none() {
        bail!("Codex token refresh returned no access token");
    }
    apply_codex_refresh(&mut stored.doc, &resp.body);
    let text = serde_json::to_string_pretty(&stored.doc)?;
    atomic_write(&stored.path, text.as_bytes()).with_context(|| format!("writing {}", stored.path.display()))?;
    restrict_permissions(&stored.path);
    Ok(())
}

/// Merge refreshed tokens into an auth.json document (v1 `saveTokens`).
pub(crate) fn apply_codex_refresh(doc: &mut Value, body: &Value) {
    if !doc.is_object() {
        *doc = json!({ "OPENAI_API_KEY": null });
    }
    let Some(root) = doc.as_object_mut() else { return };
    root.insert("auth_mode".into(), json!("chatgpt"));
    let tokens = root.entry("tokens").or_insert_with(|| json!({}));
    if !tokens.is_object() {
        *tokens = json!({});
    }
    if let Some(t) = tokens.as_object_mut() {
        for key in ["access_token", "refresh_token", "id_token"] {
            if let Some(v) = str_of(body, key).filter(|s| !s.is_empty()) {
                t.insert(key.into(), json!(v));
            }
        }
    }
    root.insert("last_refresh".into(), json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)));
}

/// Summary of a Codex login, read from disk only (v1 `codexAuthStatus`).
#[derive(Debug, Clone, Default)]
pub struct CodexLogin {
    pub authenticated: bool,
    /// Which file holds the credentials
    pub source: Option<PathBuf>,
    /// `chatgpt_plan_type` claim ("plus", "pro", "team"…), or "api" for an API-key login
    pub plan: Option<String>,
    pub account_id: Option<String>,
    /// One user within a workspace: `chatgpt_account_user_id`, else `user_id__account_id`
    pub identity: Option<String>,
    pub email: Option<String>,
}

/// Read a Codex home's login state without touching the network.
pub fn codex_login(codex_home: &Path) -> CodexLogin {
    let Some(stored) = load_codex(codex_home) else { return CodexLogin::default() };
    let Some(auth) = auth_from(&stored.doc) else { return CodexLogin::default() };
    let mut out = CodexLogin { authenticated: true, source: Some(stored.path.clone()), ..Default::default() };
    if auth.api_key.is_some() {
        out.plan = Some("api".into());
        return out;
    }
    out.account_id = auth.account_id.clone();
    let claim = jwt_payload(&auth.access_token)
        .and_then(|c| c.get("https://api.openai.com/auth").cloned())
        .unwrap_or(Value::Null);
    out.plan = str_of(&claim, "chatgpt_plan_type").map(str::to_string);
    out.identity = str_of(&claim, "chatgpt_account_user_id").map(str::to_string).or_else(|| {
        let user = str_of(&claim, "chatgpt_user_id")?;
        Some(format!("{user}__{}", auth.account_id.as_deref()?))
    });
    let id_claims = stored.doc.pointer("/tokens/id_token").and_then(Value::as_str).and_then(jwt_payload);
    out.email = id_claims
        .as_ref()
        .and_then(|c| str_of(c, "email"))
        .map(str::to_string)
        .or_else(|| {
            jwt_payload(&auth.access_token)?
                .pointer("/https:~1~1api.openai.com~1profile/email")?
                .as_str()
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty());
    out
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// macOS Keychain fallback for the machine's default Claude login (v1 behaviour).
/// A no-op everywhere else.
mod keychain {
    use serde_json::Value;
    use std::path::Path;

    #[cfg(target_os = "macos")]
    fn applies(config_dir: &Path) -> bool {
        crate::util::same_path(config_dir, &crate::paths::claude_local_dir())
    }

    #[cfg(target_os = "macos")]
    pub fn read(config_dir: &Path) -> Option<Value> {
        if !applies(config_dir) {
            return None;
        }
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        serde_json::from_slice(String::from_utf8_lossy(&out.stdout).trim().as_bytes()).ok()
    }

    #[cfg(target_os = "macos")]
    pub fn write(config_dir: &Path, doc: &Value) -> bool {
        if !applies(config_dir) {
            return false;
        }
        std::process::Command::new("security")
            .args(["add-generic-password", "-U", "-s", "Claude Code-credentials", "-w", &doc.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn read(_config_dir: &Path) -> Option<Value> {
        None
    }

    #[cfg(not(target_os = "macos"))]
    pub fn write(_config_dir: &Path, _doc: &Value) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;
    use base64::Engine;

    pub(crate) fn fake_jwt(claims: Value) -> String {
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", enc.encode(br#"{"alg":"none"}"#), enc.encode(claims.to_string()))
    }

    #[test]
    fn claude_token_fresh_is_returned_without_network() {
        let sb = sandbox();
        let dir = sb.home().join("acct");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = json!({ "claudeAiOauth": { "accessToken": "tok", "refreshToken": "r", "expiresAt": now_ms() + 3_600_000 }, "mcpOAuth": {"x": 1} });
        std::fs::write(dir.join(".credentials.json"), doc.to_string()).unwrap();
        assert_eq!(claude_access_token(&dir, false).unwrap(), "tok");
        assert!(claude_access_token(&sb.home().join("nope"), false).is_err());
    }

    #[test]
    fn claude_refresh_merge_preserves_other_keys() {
        let mut doc = json!({ "claudeAiOauth": { "accessToken": "old", "refreshToken": "r1", "subscriptionType": "max" }, "mcpOAuth": {"k": 1} });
        apply_claude_refresh(&mut doc, &json!({ "access_token": "new", "expires_in": 10, "scope": "a b" }));
        assert_eq!(doc["claudeAiOauth"]["accessToken"], "new");
        assert_eq!(doc["claudeAiOauth"]["refreshToken"], "r1");
        assert_eq!(doc["claudeAiOauth"]["subscriptionType"], "max");
        assert_eq!(doc["claudeAiOauth"]["scopes"], json!(["a", "b"]));
        assert_eq!(doc["mcpOAuth"]["k"], 1);
    }

    #[test]
    fn claude_login_reads_identity() {
        let sb = sandbox();
        let dir = sb.home().join("acct");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a","subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}"#).unwrap();
        std::fs::write(dir.join(".claude.json"), r#"{"oauthAccount":{"accountUuid":"U","organizationUuid":"O","emailAddress":"a@b.c"}}"#).unwrap();
        let login = claude_login(&dir);
        assert!(login.authenticated);
        assert_eq!(login.plan.as_deref(), Some("max"));
        assert_eq!(login.tier.as_deref(), Some("default_claude_max_20x"));
        assert_eq!(login.identity.as_deref(), Some("U:O"));
        assert_eq!(login.email.as_deref(), Some("a@b.c"));
    }

    #[test]
    fn codex_auth_and_login_from_jwt() {
        let sb = sandbox();
        let home = sb.home().join("cx");
        std::fs::create_dir_all(&home).unwrap();
        let access = fake_jwt(json!({
            "exp": now_ms() / 1000 + 3600,
            "https://api.openai.com/auth": { "chatgpt_plan_type": "pro", "chatgpt_account_id": "acc", "chatgpt_account_user_id": "user__acc" }
        }));
        let id_token = fake_jwt(json!({ "email": "me@x.y" }));
        let doc = json!({ "auth_mode": "chatgpt", "OPENAI_API_KEY": null, "tokens": { "access_token": access, "refresh_token": "r", "id_token": id_token } });
        std::fs::write(home.join("auth.json"), doc.to_string()).unwrap();
        let auth = codex_auth(&home, false).unwrap();
        assert_eq!(auth.account_id.as_deref(), Some("acc"));
        assert!(auth.api_key.is_none());
        let login = codex_login(&home);
        assert!(login.authenticated);
        assert_eq!(login.plan.as_deref(), Some("pro"));
        assert_eq!(login.identity.as_deref(), Some("user__acc"));
        assert_eq!(login.email.as_deref(), Some("me@x.y"));
    }

    #[test]
    fn codex_api_key_login() {
        let sb = sandbox();
        let home = sb.home().join("cx");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-1","tokens":null}"#).unwrap();
        let auth = codex_auth(&home, true).unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("sk-1"));
        assert_eq!(codex_login(&home).plan.as_deref(), Some("api"));
    }

    #[test]
    fn codex_refresh_merge() {
        let mut doc = json!({ "OPENAI_API_KEY": null, "tokens": { "access_token": "a", "refresh_token": "r", "account_id": "acc" } });
        apply_codex_refresh(&mut doc, &json!({ "access_token": "b", "id_token": "i" }));
        assert_eq!(doc["tokens"]["access_token"], "b");
        assert_eq!(doc["tokens"]["refresh_token"], "r");
        assert_eq!(doc["tokens"]["account_id"], "acc");
        assert_eq!(doc["auth_mode"], "chatgpt");
        assert!(doc["last_refresh"].is_string());
    }
}
