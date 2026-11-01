//! Native bro-to-bro controls over the existing authenticated PTY-host relay.
//! No Orbit descriptor/token, transcript, account secret or model tool uses this channel.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const KEY: &str = "bro.native.orbit-status.v1";
const MAX_BYTES: usize = 16 * 1024;
const FRESH_MS: u64 = 5000;
fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64) }
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Status {
    schema: u32,
    owner: u32,
    epoch: String,
    at: u64,
    runtime: Option<orbit_voice::RuntimeStatus>,
    status: String,
    starting: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: u32,
    epoch: String,
    id: String,
    at: u64,
    command: orbit_voice::VoiceControl,
}
pub(in crate::services) struct RelayState {
    epoch: String,
    current: Mutex<Option<Status>>,
    seen: Mutex<VecDeque<(String, u64)>>,
}
impl Default for RelayState {
    fn default() -> Self { Self { epoch: uuid::Uuid::new_v4().to_string(), current: Mutex::new(None), seen: Mutex::new(VecDeque::new()) } }
}
fn parse(info: &bro_host::HostInfo) -> Option<Status> {
    let bytes = info.values.get(KEY)?;
    if bytes.len() > MAX_BYTES { return None; }
    let value: Status = serde_json::from_str(bytes).ok()?;
    if value.schema != 1 || value.owner != info.peers.primary?.pid || value.at > now() || now().saturating_sub(value.at) > FRESH_MS
        || uuid::Uuid::parse_str(&value.epoch).is_err() || value.status.len() > 512 { return None; }
    Some(value)
}
fn allowed(command: &orbit_voice::VoiceControl) -> bool {
    !matches!(command, orbit_voice::VoiceControl::Shutdown {})
}
impl Services {
    pub(in crate::services) fn start_orbit_relay(&self) {
        if self.is_demo() { return; }
        let weak = Arc::downgrade(&self.inner);
        std::thread::spawn(move || loop {
            let Some(inner) = weak.upgrade() else { break };
            let svc = Services { inner };
            if svc.inner.stopping.load(Ordering::Acquire) { break; }
            if svc.inner.guest || svc.state().orbit_voice.owned_elsewhere {
                // Query only. A --new/solo window never connects as a driver or launches a runtime.
                let value = bro_host::client::status(&crate::util::bro_dir()).ok().as_ref().and_then(parse);
                svc.update(|state| {
                    state.orbit_voice.owned_elsewhere = true;
                    state.orbit_voice.connected = value.as_ref().is_some_and(|s| s.runtime.is_some());
                    state.orbit_voice.runtime = value.as_ref().and_then(|s| s.runtime.clone());
                    state.orbit_voice.starting = value.as_ref().is_some_and(|s| s.starting);
                    state.orbit_voice.status = value.as_ref().map_or_else(|| "Primary voice controls unavailable".into(), |s| s.status.clone());
                });
                *svc.inner.orbit_relay.current.lock() = value;
            } else if let Some(host) = svc.host().filter(|h| h.peers().primary.is_some_and(|p| p.pid == std::process::id())) {
                let state = svc.state().orbit_voice.clone();
                let value = Status { schema: 1, owner: std::process::id(), epoch: svc.inner.orbit_relay.epoch.clone(), at: now(),
                    runtime: state.runtime, status: state.status, starting: state.starting };
                if let Ok(encoded) = serde_json::to_string(&value) && encoded.len() <= MAX_BYTES { host.set_value(KEY, &encoded); }
            }
            drop(svc);
            std::thread::sleep(Duration::from_millis(750));
        });
    }

    pub(super) fn relay_orbit_control(&self, command: orbit_voice::VoiceControl) {
        if !allowed(&command) { return; }
        let latest = self.inner.orbit_relay.current.lock().clone();
        let Some(latest) = latest.filter(|s| s.at <= now() && now() - s.at <= FRESH_MS) else {
            self.send(Event::Toast(Kind::Info, "The primary voice controls are unavailable. Open the primary window and try again.".into())); return;
        };
        let request = Request { schema: 1, epoch: latest.epoch.clone(), id: uuid::Uuid::new_v4().to_string(), at: now(), command };
        let svc = self.clone();
        std::thread::spawn(move || {
            let result = bro_host::Client::connect(&crate::util::bro_dir(), bro_host::Role::Query, |_| {});
            if let Ok(client) = result && client.peers().primary.is_some_and(|p| p.pid == latest.owner)
                && parse(client.info()).is_some_and(|s| s.epoch == latest.epoch)
                && let Ok(body) = serde_json::to_value(request) {
                client.relay(serde_json::json!({"orbitControl":body}));
                // Sending is not an applied acknowledgement; authoritative status arrives separately.
            } else { svc.send(Event::Toast(Kind::Info, "The primary window changed. Try the voice control again.".into())); }
        });
    }

    /// Returns true for this closed native envelope, including rejected stale/malformed requests.
    pub(crate) fn accept_orbit_relay(&self, body: &serde_json::Value) -> bool {
        let Some(value) = body.get("orbitControl") else { return false; };
        if self.inner.guest || self.orbit_owned_elsewhere() || body.as_object().is_none_or(|v| v.len() != 1)
            || serde_json::to_vec(value).map_or(true, |v| v.len() > MAX_BYTES) { return true; }
        let Ok(request) = serde_json::from_value::<Request>(value.clone()) else { return true; };
        if request.schema != 1 || request.epoch != self.inner.orbit_relay.epoch || request.at > now() || now() - request.at > FRESH_MS
            || uuid::Uuid::parse_str(&request.id).is_err() || !allowed(&request.command) { return true; }
        let mut seen = self.inner.orbit_relay.seen.lock();
        seen.retain(|(_, at)| *at <= now() && now() - *at <= FRESH_MS);
        if seen.iter().any(|(id, _)| id == &request.id) { return true; }
        if seen.len() >= 256 { return true; }
        seen.push_back((request.id, request.at)); // expired entries cannot be replayed: the timestamp is also checked
        drop(seen);
        self.orbit_control(request.command);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_status_is_fresh_primary_bound_and_never_authorizes_shutdown() {
        let status = Status { schema: 1, owner: 42, epoch: uuid::Uuid::new_v4().to_string(), at: now(), runtime: None, status: "Microphone off".into(), starting: false };
        let mut info = bro_host::HostInfo::default();
        info.peers.primary = Some(bro_host::Peer { pid: 42, role: bro_host::Role::Window });
        info.values.insert(KEY.into(), serde_json::to_string(&status).unwrap());
        assert!(parse(&info).is_some());
        info.peers.primary.as_mut().unwrap().pid = 43;
        assert!(parse(&info).is_none());
        info.peers.primary.as_mut().unwrap().pid = 42;
        info.values.insert(KEY.into(), serde_json::to_string(&Status { at: now().saturating_sub(FRESH_MS + 1), ..status }).unwrap());
        assert!(parse(&info).is_none());
        assert!(!allowed(&orbit_voice::VoiceControl::Shutdown {}));
        assert!(allowed(&orbit_voice::VoiceControl::Start {}));
    }
}
