//! Shared proxy state (one per running proxy).

use crate::errors::{ErrorKind, ProxyError};
use crate::events::EventBus;
use crate::translate::reasoning_cache::ReasoningCache;
use crate::trim::Trimmer;
use crate::upstream::chatgpt::CodexModel;
use crate::upstream::logins::Meter;
use crate::upstream::pool::PoolManager;
use crate::{LoginFamily, PoolLogin, ProxyOptions, Route};
use bro_core::creds::CodexAuth;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// How long a read of the signed-in logins is trusted: a login signed in or out while sessions run
/// joins or leaves its pool within this.
const LOGINS_TTL: Duration = Duration::from_secs(10);

pub(crate) struct AppState {
    routes: RwLock<HashMap<String, Route>>,
    pub events: EventBus,
    pub http: reqwest::Client,
    pub token: Option<String>,
    pub opts: ProxyOptions,
    pool: OnceLock<Arc<PoolManager>>,
    codex_pool: OnceLock<Arc<PoolManager>>,
    logins: Mutex<HashMap<LoginFamily, (Instant, Vec<PoolLogin>)>>,
    /// Each proxy-mode login's latest rate-limit reading (by profile id), from its responses
    pub meters: Mutex<HashMap<String, Meter>>,
    pub reasoning: Arc<ReasoningCache>,
    pub chatgpt_models: Mutex<HashMap<PathBuf, (Instant, Vec<CodexModel>)>>,
    pub upstream_models: Mutex<HashMap<String, (Instant, Vec<String>)>>,
    /// Per-proxy session id (ChatGPT `session_id` header / fallback cache key)
    pub session_id: String,
    /// Optional request compression
    pub trim: Trimmer,
}

impl AppState {
    pub fn new(token: Option<String>, opts: ProxyOptions) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(15 * 60))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()?;
        let trim = Trimmer::new(opts.trim, opts.trim_level);
        Ok(AppState {
            routes: RwLock::new(HashMap::new()),
            events: EventBus::new(),
            http,
            token,
            opts,
            pool: OnceLock::new(),
            codex_pool: OnceLock::new(),
            logins: Mutex::new(HashMap::new()),
            meters: Mutex::new(HashMap::new()),
            reasoning: Arc::new(ReasoningCache::new()),
            chatgpt_models: Mutex::new(HashMap::new()),
            upstream_models: Mutex::new(HashMap::new()),
            session_id: uuid::Uuid::new_v4().to_string(),
            trim,
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

    /// Where a proxy-mode family's request counters, cooldowns and conversations live: Claude logins
    /// share v1's pool file (by v1 account name), Codex logins have their own (by profile id).
    pub fn counters(&self, family: LoginFamily) -> Arc<PoolManager> {
        match family {
            LoginFamily::Claude => self.pool(),
            LoginFamily::Codex => self
                .codex_pool
                .get_or_init(|| {
                    let file = self
                        .opts
                        .codex_pool_usage_file
                        .clone()
                        .unwrap_or_else(|| bro_core::paths::bro_dir().join("codex-pool-usage.json"));
                    Arc::new(PoolManager::new(file))
                })
                .clone(),
        }
    }

    /// The signed-in logins of `family` (cached [`LOGINS_TTL`]; the listing runs off the async workers).
    pub async fn logins(&self, family: LoginFamily) -> Vec<PoolLogin> {
        if let Some(list) = self.cached_logins(family) {
            return list;
        }
        let f = self.opts.logins.clone();
        let list = tokio::task::spawn_blocking(move || read_logins(f.as_ref(), family)).await.unwrap_or_default();
        self.logins.lock().insert(family, (Instant::now(), list.clone()));
        list
    }

    /// [`AppState::logins`] for blocking callers (the Proxy view's status poll).
    pub fn logins_blocking(&self, family: LoginFamily) -> Vec<PoolLogin> {
        if let Some(list) = self.cached_logins(family) {
            return list;
        }
        let list = read_logins(self.opts.logins.as_ref(), family);
        self.logins.lock().insert(family, (Instant::now(), list.clone()));
        list
    }

    fn cached_logins(&self, family: LoginFamily) -> Option<Vec<PoolLogin>> {
        self.logins.lock().get(&family).filter(|(at, _)| at.elapsed() < LOGINS_TTL).map(|(_, list)| list.clone())
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

    pub async fn gemini_auth(&self, file: &Path, force: bool) -> Result<bro_core::gemini::GeminiAuth, ProxyError> {
        let f = self.opts.gemini_auth.clone();
        let file_owned = file.to_path_buf();
        let res = tokio::task::spawn_blocking(move || match f {
            Some(f) => f(&file_owned, force),
            None => bro_core::gemini::auth(&file_owned, force),
        })
        .await;
        match res {
            Ok(Ok(a)) => Ok(a),
            Ok(Err(e)) => Err(ProxyError::new(ErrorKind::Authentication, format!("Gemini sign-in: {e:#}"))),
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

/// Who shares a family's route: [`ProxyOptions::logins`], else bro's signed-in subscription logins, one per
/// account (a Claude login names its `accountUuid`, the first half of its identity).
fn read_logins(f: Option<&crate::LoginsFn>, family: LoginFamily) -> Vec<PoolLogin> {
    if let Some(f) = f {
        return f(family);
    }
    bro_core::profiles::subscription_logins(family == LoginFamily::Claude)
        .into_iter()
        .map(|p| {
            let account = if p.is_claude() { p.identity.as_deref().and_then(|i| i.split(':').next()).filter(|u| !u.is_empty()).map(str::to_string) } else { None };
            PoolLogin { id: p.id, dir: p.dir, account }
        })
        .collect()
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
