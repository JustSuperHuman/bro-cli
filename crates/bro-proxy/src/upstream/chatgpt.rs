//! ChatGPT Codex backend (`chatgpt.com/backend-api/codex`), presenting as the Codex
//! CLI (ported from bro v1 codex-bridge.js). API-key Codex logins go to the OpenAI
//! Responses API instead.

use super::{
    Endpoint, UpstreamCall, UpstreamResponse, execute, finish, method_for, openai_url, set,
};
use crate::errors::ProxyError;
use crate::state::AppState;
use bro_core::creds::CodexAuth;
use http::HeaderMap;
use serde_json::Value;
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) const CLIENT_VERSION: &str = "0.144.1";

#[derive(Debug, Clone, PartialEq)]
pub struct CodexModel {
    pub id: String,
    pub name: String,
    pub default_effort: String,
    pub efforts: Vec<String>,
}

/// Used only when the live fetch and every cache are unavailable (v1 list).
fn fallback_models() -> Vec<CodexModel> {
    let m = |id: &str, name: &str, efforts: &[&str]| CodexModel {
        id: id.into(),
        name: name.into(),
        default_effort: "medium".into(),
        efforts: efforts.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        m(
            "gpt-5.6-sol",
            "GPT-5.6-Sol",
            &["low", "medium", "high", "xhigh"],
        ),
        m("gpt-5.6-terra", "GPT-5.6-Terra", &["low", "medium", "high"]),
        m("gpt-5.5", "GPT-5.5", &["low", "medium", "high"]),
        m("gpt-5.4-mini", "GPT-5.4-Mini", &["low", "medium", "high"]),
    ]
}

/// True when the login should use the ChatGPT backend (not an API-key login).
pub(crate) fn is_chatgpt_login(auth: &CodexAuth) -> bool {
    !(auth.access_token.is_empty() && auth.api_key.is_some())
}

pub(crate) fn backend_headers(auth: &CodexAuth, session_id: &str, stream: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    set(
        &mut h,
        "authorization",
        &format!("Bearer {}", auth.access_token),
    );
    if let Some(acct) = auth.account_id.as_deref().filter(|a| !a.is_empty()) {
        set(&mut h, "chatgpt-account-id", acct);
    }
    set(&mut h, "openai-beta", "responses=experimental");
    set(&mut h, "originator", "codex_cli_rs");
    set(
        &mut h,
        "user-agent",
        &format!("codex_cli_rs/{CLIENT_VERSION}"),
    );
    set(&mut h, "session_id", session_id);
    set(
        &mut h,
        "accept",
        if stream {
            "text/event-stream"
        } else {
            "application/json"
        },
    );
    set(&mut h, "content-type", "application/json");
    h
}

fn target(
    state: &AppState,
    auth: &CodexAuth,
    endpoint: Endpoint,
    stream: bool,
) -> (String, HeaderMap) {
    if is_chatgpt_login(auth) {
        let base = state.opts.chatgpt_base_url.trim_end_matches('/');
        let url = match endpoint {
            Endpoint::Models => format!("{base}/models?client_version={CLIENT_VERSION}"),
            _ => format!("{base}/responses"),
        };
        (url, backend_headers(auth, &state.session_id, stream))
    } else {
        let mut h = HeaderMap::new();
        set(
            &mut h,
            "authorization",
            &format!("Bearer {}", auth.api_key.as_deref().unwrap_or_default()),
        );
        set(&mut h, "content-type", "application/json");
        (openai_url(&state.opts.openai_base_url, endpoint), h)
    }
}

/// Send with the prefetched auth; retry once after a forced refresh on 401.
pub(crate) async fn send(
    state: &AppState,
    codex_home: &Path,
    call: &UpstreamCall,
) -> Result<UpstreamResponse, ProxyError> {
    let mut auth = match &call.codex_auth {
        Some(a) => a.clone(),
        None => state.codex_auth(codex_home, false).await?,
    };
    let mut forced = false;
    loop {
        let (url, headers) = target(state, &auth, call.endpoint, call.stream);
        let resp = execute(
            state,
            method_for(call.endpoint),
            &url,
            headers,
            call.body.clone(),
        )
        .await?;
        if resp.status() == 401 && !forced {
            tracing::info!(home = %codex_home.display(), "ChatGPT 401; refreshing token and retrying");
            forced = true;
            auth = state.codex_auth(codex_home, true).await?;
            continue;
        }
        return finish(resp, None).await;
    }
}

fn map_model_list(v: &Value) -> Vec<CodexModel> {
    v.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("visibility").and_then(Value::as_str) != Some("hide"))
        .filter_map(|m| {
            let id = m.get("slug").and_then(Value::as_str)?.to_string();
            Some(CodexModel {
                name: m
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string(),
                default_effort: m
                    .get("default_reasoning_level")
                    .and_then(Value::as_str)
                    .unwrap_or("medium")
                    .to_string(),
                efforts: m
                    .get("supported_reasoning_levels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|l| l.get("effort").and_then(Value::as_str).map(str::to_string))
                    .collect(),
                id,
            })
        })
        .collect()
}

/// The subscription's live model list (cached 10 min), else Codex's own
/// `models_cache.json`, else a static list.
pub(crate) async fn models(state: &AppState, codex_home: &Path) -> Vec<CodexModel> {
    if let Some((at, v)) = state.chatgpt_models.lock().get(codex_home)
        && at.elapsed() < Duration::from_secs(600)
    {
        return v.clone();
    }
    let live = tokio::time::timeout(Duration::from_secs(10), async {
        let resp = send(state, codex_home, &UpstreamCall::get_models())
            .await
            .ok()?;
        let body = super::read_all(resp.body).await.ok()?;
        let v: Value = serde_json::from_slice(&body).ok()?;
        let list = map_model_list(&v);
        (!list.is_empty()).then_some(list)
    })
    .await
    .ok()
    .flatten();
    let list = live
        .or_else(|| {
            let raw = std::fs::read(codex_home.join("models_cache.json")).ok()?;
            let list = map_model_list(&serde_json::from_slice(&raw).ok()?);
            (!list.is_empty()).then_some(list)
        })
        .unwrap_or_else(fallback_models);
    state
        .chatgpt_models
        .lock()
        .insert(codex_home.to_path_buf(), (Instant::now(), list.clone()));
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_model_list() {
        let v = json!({"models": [
            {"slug": "gpt-5.5", "display_name": "GPT-5.5", "default_reasoning_level": "high",
             "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}]},
            {"slug": "hidden", "visibility": "hide"}
        ]});
        let l = map_model_list(&v);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].efforts, vec!["low", "high"]);
        assert_eq!(l[0].default_effort, "high");
    }

    #[test]
    fn headers_present_as_codex() {
        let auth = CodexAuth {
            access_token: "at".into(),
            account_id: Some("acct".into()),
            api_key: None,
        };
        let h = backend_headers(&auth, "sess", true);
        assert_eq!(h["originator"], "codex_cli_rs");
        assert_eq!(h["chatgpt-account-id"], "acct");
        assert_eq!(h["openai-beta"], "responses=experimental");
        assert_eq!(h["session_id"], "sess");
        assert!(is_chatgpt_login(&auth));
        let key = CodexAuth {
            access_token: String::new(),
            account_id: None,
            api_key: Some("sk".into()),
        };
        assert!(!is_chatgpt_login(&key));
    }
}
