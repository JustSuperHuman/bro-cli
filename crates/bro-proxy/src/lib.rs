//! bro-proxy — a local, route-based API translating proxy.
//!
//! Inbound dialect is chosen by path, upstream dialect by route:
//!
//! ```text
//! {base}/r/{route_id}/v1/messages               Anthropic Messages in
//! {base}/r/{route_id}/v1/messages/count_tokens
//! {base}/r/{route_id}/v1/chat/completions       OpenAI Chat Completions in
//! {base}/r/{route_id}/v1/responses              OpenAI Responses in
//! {base}/r/{route_id}/v1/models                 model list in the inbound dialect
//! {base}/health
//! ```
//!
//! Same-dialect requests are passed through (auth swapped, model mapped); cross-dialect
//! requests are translated in both directions including streaming SSE, tools,
//! tool results, images, thinking/reasoning, usage and stop reasons.
//!
//! Contract rule: public items here are what bro-tui builds against. Add freely;
//! do not rename or remove.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How to reach and authenticate with the real upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Upstream {
    /// OpenAI-compatible Chat Completions (`{base_url}/chat/completions`)
    OpenAiChat { base_url: String, api_key: Option<String> },
    /// OpenAI Responses API (`{base_url}/responses`)
    OpenAiResponses { base_url: String, api_key: Option<String> },
    /// ChatGPT Codex backend (`https://chatgpt.com/backend-api/codex/responses`) using a
    /// Codex login's OAuth tokens (bro_core::creds::codex_auth)
    ChatGptCodex { codex_home: PathBuf },
    /// Anthropic Messages with an API key / bearer token (Anthropic, OpenRouter, z.ai…)
    Anthropic { base_url: String, api_key: Option<String>, bearer: bool },
    /// api.anthropic.com with a Claude subscription login (bro_core::creds::claude_access_token)
    ClaudeOAuth { config_dir: PathBuf },
    /// Claude account pool: least-loaded account, sticky per conversation, fail over on
    /// rate limits before any bytes are sent (v1 pool semantics)
    ClaudePool { config_dirs: Vec<PathBuf> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub id: String,
    pub upstream: Upstream,
    /// Model sent upstream when the client's model is a foreign alias
    /// (e.g. Claude Code asks for "claude-sonnet-…" but upstream is gpt-5)
    pub default_model: Option<String>,
    /// Model for "small/fast" requests (haiku/mini aliases)
    pub small_model: Option<String>,
    /// Exact client-model -> upstream-model overrides
    #[serde(default)]
    pub model_map: Vec<(String, String)>,
    /// Human label for the Proxy view
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub bind: String,
    /// 0 = any free port; otherwise try `port..port+20`
    pub port: u16,
    /// Required from clients as `x-api-key` / `Authorization: Bearer`; None = loopback-only open
    pub token: Option<String>,
}

/// One completed (or failed) request, for the live Proxy view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyEvent {
    pub at_ms: i64,
    pub route_id: String,
    pub inbound: String,
    pub upstream: String,
    pub model: String,
    pub status: u16,
    pub stream: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub latency_ms: u64,
    pub error: Option<String>,
    /// Pool account used, if any
    pub account: Option<String>,
}

/// Cheap-to-clone handle to a running proxy (it owns its own tokio runtime thread).
#[derive(Clone)]
pub struct ProxyHandle {
    _private: (),
}

impl ProxyHandle {
    /// Start on a dedicated thread with its own multi-threaded tokio runtime.
    /// Blocks only until the listener is bound.
    pub fn start(cfg: ProxyConfig) -> anyhow::Result<ProxyHandle> { todo!() }
    pub fn port(&self) -> u16 { todo!() }
    /// "http://127.0.0.1:{port}"
    pub fn base_url(&self) -> String { todo!() }
    pub fn upsert_route(&self, route: Route) { todo!() }
    pub fn remove_route(&self, id: &str) { todo!() }
    pub fn routes(&self) -> Vec<Route> { todo!() }
    /// Subscribe to request events. The callback runs on a proxy thread; keep it cheap
    /// (e.g. push into a channel and wake the UI).
    pub fn on_event(&self, f: Box<dyn Fn(ProxyEvent) + Send + Sync>) { todo!() }
    /// Most recent events (ring buffer, newest last)
    pub fn recent(&self, n: usize) -> Vec<ProxyEvent> { todo!() }
    pub fn shutdown(&self) { todo!() }
}
