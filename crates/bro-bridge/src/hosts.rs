//! Other bridges on this machine.
//!
//! Every bro runs its own bridge (10001, then 10002… when taken), all sharing
//! `<data_root>` and so the same token. Clients connected to one should see and
//! switch to the others without pairing again:
//!
//! * `GET /api/host` is this bridge's card ([`LocalHost`]); loopback callers
//!   also get its token, so a bridge with another data root can be reached too
//!   (loopback is fully trusted already, see [`crate::auth`]).
//! * Each bridge registers in `<data_root>/hosts/<id>.json` while it runs
//!   (custom ports, specific bind addresses) and every bridge scans loopback
//!   10001..=10021 (and its configured range) for the rest.
//! * `GET /api/hosts` and the `hello` field `hosts` list them all, this one
//!   first with `current: true`; a `hosts` event goes out when peers appear,
//!   leave or change (sessions, needs-you counts) while clients are connected.
//!
//! A peer's `token` is only listed when it differs from this bridge's: clients
//! reuse the one they already hold otherwise.

use crate::insights::AttentionState;
use crate::model::ServerEvent;
use crate::state::AppState;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

/// Registry directory under the data root.
pub(crate) const HOSTS_DIR: &str = "hosts";
/// First port bridges try ([`crate::BridgeConfig::default`]).
const DEFAULT_PORT: u16 = 10001;
/// Ports after the first one a bridge may land on (`server::PORT_ATTEMPTS`).
const SCAN_SPAN: u16 = 20;
/// One probe; a refused loopback connect can take ~2 s on Windows.
const PROBE_TIMEOUT: Duration = Duration::from_millis(2500);
/// A cached peer list younger than this answers `/api/hosts` as is.
const FRESH_FOR: Duration = Duration::from_secs(3);
/// Re-scan cadence while clients are connected.
const REFRESH_EVERY: Duration = Duration::from_secs(5);
/// Registry entries younger than this are never pruned (the bridge may still be starting).
const PRUNE_AFTER: Duration = Duration::from_secs(60);
/// Project names on a card.
const MAX_PROJECTS: usize = 3;

/// One bridge on this machine, as clients see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalHost {
    /// Random per bridge start.
    pub id: String,
    /// "bro"
    pub runtime: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub pid: u32,
    pub port: u16,
    /// Bound address: "0.0.0.0" / "::" (every interface, reach it on the host
    /// you reached this one), a loopback address (this machine only) or one interface.
    pub bind: String,
    #[serde(default)]
    pub started_at: String,
    /// Sessions, not counting bro's orchestrator.
    #[serde(default)]
    pub sessions: usize,
    #[serde(default)]
    pub running: usize,
    /// Running agents waiting on the user.
    #[serde(default)]
    pub awaiting: usize,
    /// Projects of its running sessions, busiest first.
    #[serde(default)]
    pub projects: Vec<String>,
    /// The bridge answering this request.
    #[serde(default)]
    pub current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// What a bridge writes to `<data_root>/hosts/<id>.json`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    id: String,
    pid: u32,
    host: String,
    port: u16,
    #[serde(default)]
    started_at: String,
}

#[derive(Default)]
struct Peers {
    at: Option<Instant>,
    list: Vec<LocalHost>,
}

/// This bridge's identity and the last peer scan.
pub(crate) struct HostDirectory {
    pub id: String,
    peers: Mutex<Peers>,
    /// One scan at a time; callers arriving mid-scan reuse its result.
    scanning: tokio::sync::Mutex<()>,
    /// Built once: a client per scan would reload TLS roots every few seconds.
    client: std::sync::OnceLock<Option<reqwest::Client>>,
}

impl HostDirectory {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            peers: Mutex::new(Peers::default()),
            scanning: tokio::sync::Mutex::new(()),
            client: std::sync::OnceLock::new(),
        }
    }

    fn fresh(&self) -> bool {
        self.peers.lock().at.is_some_and(|at| at.elapsed() < FRESH_FOR)
    }
}

/// Where a client reaches a bridge from this machine.
fn local_address(bind: IpAddr, port: u16) -> SocketAddr {
    if bind.is_unspecified() || bind.is_loopback() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    } else {
        SocketAddr::new(bind, port)
    }
}

impl AppState {
    /// This bridge's card. The token rides along only when `with_token`.
    pub(crate) fn host_card(&self, with_token: bool) -> LocalHost {
        let (bind, port, started_at, sessions) = {
            let inner = self.inner.lock();
            let names: HashMap<&str, &str> = inner
                .published_projects
                .iter()
                .map(|project| (project.id.as_str(), project.name.as_str()))
                .collect();
            let sessions: Vec<(bool, bool, Option<String>)> = inner
                .sessions
                .values()
                .map(|session| &session.summary)
                .filter(|summary| summary.kind.as_deref() != Some("orchestrator"))
                .map(|summary| {
                    let running = summary.status == "running";
                    let awaiting = running
                        && (summary.agent_activity.as_deref() == Some("awaiting")
                            || summary.attention.as_ref().is_some_and(|attention| attention.state == AttentionState::Blocked));
                    let project = summary
                        .project_id
                        .as_deref()
                        .and_then(|id| names.get(id).map(|name| (*name).to_owned()))
                        .or_else(|| (!summary.cwd.trim().is_empty()).then(|| crate::projects::directory_display_name(&summary.cwd)));
                    (running, awaiting, project)
                })
                .collect();
            (inner.host, inner.port, inner.started_at.clone(), sessions)
        };
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for (running, _, project) in &sessions {
            if let Some(project) = project.as_deref().filter(|_| *running) {
                *counts.entry(project).or_default() += 1;
            }
        }
        let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
        ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
        LocalHost {
            id: self.hosts.id.clone(),
            runtime: "bro".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            port,
            bind: bind.to_string(),
            started_at,
            sessions: sessions.len(),
            running: sessions.iter().filter(|session| session.0).count(),
            awaiting: sessions.iter().filter(|session| session.1).count(),
            projects: ranked.into_iter().take(MAX_PROJECTS).map(|(name, _)| name.to_owned()).collect(),
            current: false,
            token: with_token.then(|| self.token.as_ref().clone()),
        }
    }

    /// Every bridge on this machine from the last scan: this one first, then by port.
    pub(crate) fn local_hosts(&self) -> Vec<LocalHost> {
        let mut own = self.host_card(false);
        own.current = true;
        let mut hosts = vec![own];
        hosts.extend(self.hosts.peers.lock().list.iter().cloned());
        hosts
    }
}

/// [`AppState::local_hosts`] after a scan when the last one is stale.
pub(crate) async fn current(state: &AppState) -> Vec<LocalHost> {
    if !state.hosts.fresh() {
        refresh(state).await;
    }
    state.local_hosts()
}

/// Scans for peers (unless a scan just finished) and broadcasts `hosts` when they changed.
pub(crate) async fn refresh(state: &AppState) {
    let _scan = state.hosts.scanning.lock().await;
    if state.hosts.fresh() {
        return;
    }
    let found = discover(state).await;
    let changed = {
        let mut peers = state.hosts.peers.lock();
        peers.at = Some(Instant::now());
        let changed = peers.list != found;
        peers.list = found;
        changed
    };
    if changed {
        publish(state);
    }
}

fn publish(state: &AppState) {
    state.publish(ServerEvent::global(json!({ "type": "hosts", "hosts": state.local_hosts() })));
}

/// Keeps the peer list current while anyone is connected.
pub(crate) fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut shutdown = state.shutdown.subscribe();
        loop {
            if state.clients.load(Ordering::Relaxed) > 0 {
                refresh(&state).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(REFRESH_EVERY) => {}
                _ = shutdown.wait_for(|stopped| *stopped) => return,
            }
        }
    });
}

/// A new client: make sure its first `hosts` event is not a full cadence away.
pub(crate) fn refresh_soon(state: &AppState) {
    if state.hosts.fresh() {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move { refresh(&state).await });
}

fn registry_dir(data_root: &Path) -> PathBuf {
    data_root.join(HOSTS_DIR)
}

/// Announces this bridge in the registry (after it binds).
pub(crate) fn register(state: &AppState) {
    let (host, port, started_at) = {
        let inner = state.inner.lock();
        (inner.host, inner.port, inner.started_at.clone())
    };
    let entry = Registration {
        id: state.hosts.id.clone(),
        pid: std::process::id(),
        host: host.to_string(),
        port,
        started_at,
    };
    let path = registry_dir(&state.data_root).join(format!("{}.json", entry.id));
    let bytes = serde_json::to_vec_pretty(&entry).unwrap_or_default();
    if let Err(error) = crate::project_store::write_atomic(&path, &bytes) {
        tracing::warn!("bro-bridge: could not register in {}: {error}", path.display());
    }
}

/// Leaves the registry (on shutdown).
pub(crate) fn unregister(state: &AppState) {
    let _ = std::fs::remove_file(registry_dir(&state.data_root).join(format!("{}.json", state.hosts.id)));
}

struct Target {
    address: SocketAddr,
    /// Registry file to prune when the bridge it names is gone.
    entry: Option<(PathBuf, String)>,
}

/// Loopback ports bridges land on by default, plus this bridge's own range.
fn scan_ports(configured: u16) -> Vec<u16> {
    let mut ports: Vec<u16> = (DEFAULT_PORT..=DEFAULT_PORT + SCAN_SPAN).collect();
    if configured != 0 {
        ports.extend(configured..=configured.saturating_add(SCAN_SPAN));
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn registry_targets(dir: &Path) -> Vec<Target> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let path = entry.path();
            let registration: Registration = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            let bind: IpAddr = registration.host.parse().ok()?;
            (registration.port != 0).then(|| Target {
                address: local_address(bind, registration.port),
                entry: Some((path, registration.id)),
            })
        })
        .collect()
}

fn old_enough_to_prune(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= PRUNE_AFTER)
}

/// Probes every candidate at once; returns the peers that answered, by port.
async fn discover(state: &AppState) -> Vec<LocalHost> {
    let (own_bind, own_port) = {
        let inner = state.inner.lock();
        (inner.host, inner.port)
    };
    let own_address = local_address(own_bind, own_port);
    let mut targets: Vec<Target> = registry_targets(&registry_dir(&state.data_root));
    // Port 0 (ephemeral, tests) scans nothing: the registry finds its peers.
    if state.config.port != 0 {
        for port in scan_ports(state.config.port) {
            let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
            if !targets.iter().any(|target| target.address == address) {
                targets.push(Target { address, entry: None });
            }
        }
    }
    targets.retain(|target| target.address != own_address);
    let client = state
        .hosts
        .client
        .get_or_init(|| reqwest::Client::builder().no_proxy().timeout(PROBE_TIMEOUT).build().ok());
    let Some(client) = client.clone() else {
        return Vec::new();
    };
    let probes = targets.into_iter().map(|target| {
        let client = client.clone();
        let token = state.token.clone();
        async move {
            let result = probe(&client, target.address, &token).await;
            (target, result)
        }
    });
    let mut found: BTreeMap<String, LocalHost> = BTreeMap::new();
    for (target, result) in futures_util::future::join_all(probes).await {
        let gone = match (&result, &target.entry) {
            (Ok(card), Some((_, id))) => &card.id != id,
            (Err(refused), Some(_)) => *refused,
            _ => false,
        };
        if gone && let Some((path, _)) = &target.entry && old_enough_to_prune(path) {
            let _ = std::fs::remove_file(path);
        }
        let Ok(mut card) = result else { continue };
        if card.id == state.hosts.id {
            continue;
        }
        card.current = false;
        if card.token.as_deref() == Some(state.token.as_str()) {
            card.token = None;
        }
        found.entry(card.id.clone()).or_insert(card);
    }
    let mut peers: Vec<LocalHost> = found.into_values().collect();
    peers.sort_by(|left, right| left.port.cmp(&right.port).then_with(|| left.id.cmp(&right.id)));
    peers
}

/// `GET /api/host` on one address. Err(true) when nothing listens there.
async fn probe(client: &reqwest::Client, address: SocketAddr, token: &str) -> Result<LocalHost, bool> {
    let mut request = client.get(format!("http://{address}/api/host"));
    // Loopback needs no token; another interface is a bridge sharing our data root.
    if !address.ip().is_loopback() {
        request = request.header("x-terminal-web-token", token);
    }
    let response = request.send().await.map_err(|error| error.is_connect())?;
    if !response.status().is_success() {
        return Err(false);
    }
    let card: LocalHost = response.json().await.map_err(|_| false)?;
    (card.runtime == "bro" && !card.id.is_empty() && card.port != 0).then_some(card).ok_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_covers_the_default_range_and_the_configured_one() {
        let ports = scan_ports(10001);
        assert_eq!(ports.first(), Some(&10001));
        assert_eq!(ports.last(), Some(&10021));
        assert_eq!(ports.len(), 21);
        let custom = scan_ports(45000);
        assert!(custom.contains(&10001) && custom.contains(&45000) && custom.contains(&45020));
        assert_eq!(custom.len(), 42);
    }

    #[test]
    fn peers_bound_everywhere_are_reached_on_loopback() {
        let any: IpAddr = "0.0.0.0".parse().unwrap();
        let lan: IpAddr = "192.168.1.5".parse().unwrap();
        assert_eq!(local_address(any, 10002), "127.0.0.1:10002".parse().unwrap());
        assert_eq!(local_address("::".parse().unwrap(), 10002), "127.0.0.1:10002".parse().unwrap());
        assert_eq!(local_address(lan, 10002), "192.168.1.5:10002".parse().unwrap());
    }
}
