//! Shared proxy state (one per running proxy).

use crate::errors::{ErrorKind, ProxyError};
use crate::events::EventBus;
use crate::translate::reasoning_cache::ReasoningCache;
use crate::upstream::chatgpt::CodexModel;
use crate::upstream::pool::PoolManager;
use crate::{ProxyOptions, Route};
use bro_core::creds::CodexAuth;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub(crate) struct AppState {
    routes: RwLock<HashMap<String, Route>>,
    pub events: EventBus,
    pub http: reqwest::Client,
    pub token: Option<String>,
    pub opts: ProxyOptions,
    pool: OnceLock<Arc<PoolManager>>,
    pub reasoning: Arc<ReasoningCache>,
    pub chatgpt_models: Mutex<HashMap<PathBuf, (Instant, Vec<CodexModel>)>>,
    pub upstream_models: Mutex<HashMap<String, (Instant, Vec<String>)>>,
    /// Per-proxy session id (ChatGPT `session_id` header / fallback cache key)
    pub session_id: String,
}

impl AppState {
    pub fn new(token: Option<String>, opts: ProxyOptions) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(15 * 60))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()?;
        Ok(AppState {
            routes: RwLock::new(HashMap::new()),
            events: EventBus::new(),
            http,
            token,
            opts,
            pool: OnceLock::new(),
            reasoning: Arc::new(ReasoningCache::new()),
            chatgpt_models: Mutex::new(HashMap::new()),
            upstream_models: Mutex::new(HashMap::new()),
            session_id: uuid::Uuid::new_v4().to_string(),
        })
    }

    pub fn upsert_route(&self, route: Route) {
        tracing::info!(route = %route.id, upstream = route.upstream.kind_str(), "proxy route upserted");
        self.routes.write().insert(route.id.clone(), route);
    }
    pub fn remove_route(&self, id: &str) {
        self.routes.write().remove(id);
    }
    pub fn routes(&self) -> Vec<Route> {
        let mut v: Vec<Route> = self.routes.read().values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
    pub fn route(&self, id: &str) -> Option<Route> {
        self.routes.read().get(id).cloned()
    }

    pub fn pool(&self) -> Arc<PoolManager> {
        self.pool
            .get_or_init(|| {
                let file = self
                    .opts
                    .pool_usage_file
                    .clone()
                    .unwrap_or_else(default_pool_usage_file);
                Arc::new(PoolManager::new(file))
            })
            .clone()
    }

    /// Claude OAuth token (blocking credential code runs off the async workers).
    pub async fn claude_token(&self, dir: &Path, force: bool) -> Result<String, ProxyError> {
        let f = self.opts.claude_token.clone();
        let dir_owned = dir.to_path_buf();
        let res = tokio::task::spawn_blocking(move || match f {
            Some(f) => f(&dir_owned, force),
            None => bro_core::creds::claude_access_token(&dir_owned, force),
        })
        .await;
        match res {
            Ok(Ok(t)) => Ok(t),
            Ok(Err(e)) => Err(ProxyError::new(
                ErrorKind::Authentication,
                format!("Claude login {}: {e:#}", dir.display()),
            )),
            Err(e) => Err(ProxyError::api(format!("credential task failed: {e}"))),
        }
    }

    pub async fn codex_auth(&self, home: &Path, force: bool) -> Result<CodexAuth, ProxyError> {
        let f = self.opts.codex_auth.clone();
        let home_owned = home.to_path_buf();
        let res = tokio::task::spawn_blocking(move || match f {
            Some(f) => f(&home_owned, force),
            None => bro_core::creds::codex_auth(&home_owned, force),
        })
        .await;
        match res {
            Ok(Ok(a)) => Ok(a),
            Ok(Err(e)) => Err(ProxyError::new(
                ErrorKind::Authentication,
                format!("Codex login {}: {e:#}", home.display()),
            )),
            Err(e) => Err(ProxyError::api(format!("credential task failed: {e}"))),
        }
    }
}

fn default_pool_usage_file() -> PathBuf {
    let dir = std::env::var_os("CLAUDE_POOL_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".claude-max-pool")
        });
    dir.join("usage.json")
}
