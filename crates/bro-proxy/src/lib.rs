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

pub mod anthropic;
mod auth;
pub mod errors;
mod events;
mod flow;
mod models;
pub mod openai;
mod pipeline;
mod server;
pub mod sse;
mod state;
pub mod translate;
mod upstream;
pub(crate) mod util;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use upstream::pool::{PoolAccountStatus, pool_account_name};

/// API dialect of a request or upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    /// Anthropic Messages
    Anthropic,
    /// OpenAI Chat Completions
    Chat,
    /// OpenAI Responses
    Responses,
}

impl Dialect {
    pub fn as_str(self) -> &'static str {
        match self {
            Dialect::Anthropic => "anthropic",
            Dialect::Chat => "chat",
            Dialect::Responses => "responses",
        }
    }
}

impl Upstream {
    /// Dialect the upstream speaks.
    pub fn dialect(&self) -> Dialect {
        match self {
            Upstream::OpenAiChat { .. } => Dialect::Chat,
            Upstream::OpenAiResponses { .. } | Upstream::ChatGptCodex { .. } => Dialect::Responses,
            Upstream::Anthropic { .. }
            | Upstream::ClaudeOAuth { .. }
            | Upstream::ClaudePool { .. } => Dialect::Anthropic,
        }
    }

    /// Short kind name (`openai_chat`, `claude_pool`…) for logs and the Proxy view.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Upstream::OpenAiChat { .. } => "openai_chat",
            Upstream::OpenAiResponses { .. } => "openai_responses",
            Upstream::ChatGptCodex { .. } => "chatgpt_codex",
            Upstream::Anthropic { .. } => "anthropic",
            Upstream::ClaudeOAuth { .. } => "claude_oauth",
            Upstream::ClaudePool { .. } => "claude_pool",
        }
    }
}

/// Resolves a Claude OAuth access token for a config dir (`force_refresh` after a 401).
pub type ClaudeTokenFn = Arc<dyn Fn(&Path, bool) -> anyhow::Result<String> + Send + Sync>;
/// Resolves Codex/ChatGPT credentials for a `CODEX_HOME`.
pub type CodexAuthFn =
    Arc<dyn Fn(&Path, bool) -> anyhow::Result<bro_core::creds::CodexAuth> + Send + Sync>;

/// Advanced knobs (endpoints, credential sources, pool storage). `Default` is what
/// production uses; tests point these at mock servers and temp dirs.
#[derive(Clone)]
pub struct ProxyOptions {
    /// Base for ClaudeOAuth / ClaudePool upstreams
    pub anthropic_base_url: String,
    /// ChatGPT Codex backend base (`{base}/responses`, `{base}/models`)
    pub chatgpt_base_url: String,
    /// OpenAI API base used when a Codex login is an API-key login
    pub openai_base_url: String,
    /// v1-compatible pool usage file; default `$CLAUDE_POOL_DIR/usage.json`
    /// (else `~/.claude-max-pool/usage.json`)
    pub pool_usage_file: Option<PathBuf>,
    /// Default: `bro_core::creds::claude_access_token`
    pub claude_token: Option<ClaudeTokenFn>,
    /// Default: `bro_core::creds::codex_auth`
    pub codex_auth: Option<CodexAuthFn>,
    /// Idle interval after which streams get a keepalive (Anthropic `ping`)
    pub keepalive: Duration,
}

impl Default for ProxyOptions {
    fn default() -> Self {
        ProxyOptions {
            anthropic_base_url: "https://api.anthropic.com".into(),
            chatgpt_base_url: "https://chatgpt.com/backend-api/codex".into(),
            openai_base_url: "https://api.openai.com/v1".into(),
            pool_usage_file: None,
            claude_token: None,
            codex_auth: None,
            keepalive: Duration::from_secs(15),
        }
    }
}

impl std::fmt::Debug for ProxyOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyOptions")
            .field("anthropic_base_url", &self.anthropic_base_url)
            .field("chatgpt_base_url", &self.chatgpt_base_url)
            .field("openai_base_url", &self.openai_base_url)
            .field("pool_usage_file", &self.pool_usage_file)
            .field("keepalive", &self.keepalive)
            .finish_non_exhaustive()
    }
}

/// How to reach and authenticate with the real upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Upstream {
    /// OpenAI-compatible Chat Completions (`{base_url}/chat/completions`)
    OpenAiChat {
        base_url: String,
        api_key: Option<String>,
    },
    /// OpenAI Responses API (`{base_url}/responses`)
    OpenAiResponses {
        base_url: String,
        api_key: Option<String>,
    },
    /// ChatGPT Codex backend (`https://chatgpt.com/backend-api/codex/responses`) using a
    /// Codex login's OAuth tokens (bro_core::creds::codex_auth)
    ChatGptCodex { codex_home: PathBuf },
    /// Anthropic Messages with an API key / bearer token (Anthropic, OpenRouter, z.ai…)
    Anthropic {
        base_url: String,
        api_key: Option<String>,
        bearer: bool,
    },
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
/// The proxy stops when `shutdown` is called or the last handle is dropped.
#[derive(Clone)]
pub struct ProxyHandle {
    inner: Arc<server::Running>,
}

impl ProxyHandle {
    /// Start on a dedicated thread with its own multi-threaded tokio runtime.
    /// Blocks only until the listener is bound.
    pub fn start(cfg: ProxyConfig) -> anyhow::Result<ProxyHandle> {
        Self::start_with(cfg, ProxyOptions::default())
    }
    /// [`ProxyHandle::start`] with explicit [`ProxyOptions`].
    pub fn start_with(cfg: ProxyConfig, opts: ProxyOptions) -> anyhow::Result<ProxyHandle> {
        Ok(ProxyHandle {
            inner: Arc::new(server::Running::start(cfg, opts)?),
        })
    }
    pub fn port(&self) -> u16 {
        self.inner.port
    }
    /// "http://127.0.0.1:{port}"
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.inner.port)
    }
    /// Base URL clients should use for a route, e.g. `ANTHROPIC_BASE_URL`
    /// (`{base}/r/{id}`) — append `/v1` for OpenAI clients.
    pub fn route_url(&self, id: &str) -> String {
        format!("{}/r/{id}", self.base_url())
    }
    pub fn upsert_route(&self, route: Route) {
        self.inner.state.upsert_route(route)
    }
    pub fn remove_route(&self, id: &str) {
        self.inner.state.remove_route(id)
    }
    pub fn routes(&self) -> Vec<Route> {
        self.inner.state.routes()
    }
    /// Subscribe to request events. The callback runs on a proxy thread; keep it cheap
    /// (e.g. push into a channel and wake the UI).
    pub fn on_event(&self, f: Box<dyn Fn(ProxyEvent) + Send + Sync>) {
        self.inner.state.events.subscribe(f)
    }
    /// Most recent events (ring buffer, newest last)
    pub fn recent(&self, n: usize) -> Vec<ProxyEvent> {
        self.inner.state.events.recent(n)
    }
    /// Pool accounts with their v1-compatible usage counters and availability.
    pub fn pool_status(&self, config_dirs: &[PathBuf]) -> Vec<PoolAccountStatus> {
        self.inner.state.pool().status(config_dirs)
    }
    pub fn shutdown(&self) {
        self.inner.shutdown()
    }
}
