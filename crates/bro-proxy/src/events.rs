//! Request events: ring buffer + subscribers, and the per-request recorder that
//! produces exactly one [`ProxyEvent`] (also feeding pool usage counters).

use crate::anthropic::Usage;
use crate::state::AppState;
use crate::util::now_ms;
use crate::{Dialect, ProxyEvent, Route};
use parking_lot::{Mutex, RwLock};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

const RING: usize = 500;

type Subscriber = Arc<dyn Fn(ProxyEvent) + Send + Sync>;

pub(crate) struct EventBus {
    ring: Mutex<VecDeque<ProxyEvent>>,
    subs: RwLock<Vec<Subscriber>>,
}

impl EventBus {
    pub fn new() -> Self {
        EventBus {
            ring: Mutex::new(VecDeque::with_capacity(RING)),
            subs: RwLock::new(vec![]),
        }
    }

    pub fn subscribe(&self, f: Box<dyn Fn(ProxyEvent) + Send + Sync>) {
        self.subs.write().push(Arc::from(f));
    }

    pub fn recent(&self, n: usize) -> Vec<ProxyEvent> {
        let ring = self.ring.lock();
        let skip = ring.len().saturating_sub(n);
        ring.iter().skip(skip).cloned().collect()
    }

    pub fn emit(&self, ev: ProxyEvent) {
        {
            let mut ring = self.ring.lock();
            if ring.len() == RING {
                ring.pop_front();
            }
            ring.push_back(ev.clone());
        }
        let subs: Vec<Subscriber> = self.subs.read().clone();
        for s in subs {
            let ev = ev.clone();
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s(ev))).is_err() {
                tracing::warn!("proxy event subscriber panicked");
            }
        }
    }
}

/// Builds and emits one request's event. Dropping it unfinished (client went away
/// mid-stream) records a cancelled request.
pub(crate) struct Recorder {
    state: Arc<AppState>,
    started: Instant,
    ev: Option<ProxyEvent>,
    usage: Option<Usage>,
    upstream_error: bool,
}

impl Recorder {
    pub fn new(state: Arc<AppState>, route: &Route, inbound: Dialect) -> Self {
        Recorder {
            state,
            started: Instant::now(),
            ev: Some(ProxyEvent {
                at_ms: now_ms(),
                route_id: route.id.clone(),
                inbound: inbound.as_str().into(),
                upstream: route.upstream.kind_str().into(),
                model: String::new(),
                status: 0,
                stream: false,
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                latency_ms: 0,
                error: None,
                account: None,
            }),
            usage: None,
            upstream_error: false,
        }
    }

    fn ev(&mut self) -> Option<&mut ProxyEvent> {
        self.ev.as_mut()
    }
    pub fn set_model(&mut self, m: &str) {
        if let Some(e) = self.ev() {
            e.model = m.to_string();
        }
    }
    pub fn set_stream(&mut self, s: bool) {
        if let Some(e) = self.ev() {
            e.stream = s;
        }
    }
    pub fn set_account(&mut self, a: Option<String>) {
        if let Some(e) = self.ev() {
            e.account = a;
        }
    }
    pub fn set_usage(&mut self, u: &Usage) {
        self.usage = Some(u.clone());
    }
    /// The failure came from the upstream (counts against a pool account).
    pub fn mark_upstream_error(&mut self) {
        self.upstream_error = true;
    }

    pub fn finish(mut self, status: u16, error: Option<String>) {
        self.finish_inner(status, error);
    }

    fn finish_inner(&mut self, status: u16, error: Option<String>) {
        let Some(mut ev) = self.ev.take() else { return };
        ev.status = status;
        ev.latency_ms = self.started.elapsed().as_millis() as u64;
        ev.error = error.clone();
        if let Some(u) = &self.usage {
            ev.input_tokens = Some(crate::translate::total_input(u));
            ev.output_tokens = Some(u.output_tokens);
            ev.cache_read_tokens = u.cache_read_input_tokens;
        }
        if let Some(account) = &ev.account {
            let pool = self.state.pool();
            match (&error, self.upstream_error) {
                (None, _) => pool.record_success(account, self.usage.as_ref()),
                (Some(msg), true) => pool.record_error(account, msg),
                _ => {}
            }
        }
        match &error {
            None => tracing::info!(
                route = %ev.route_id, inbound = %ev.inbound, upstream = %ev.upstream, model = %ev.model,
                status, stream = ev.stream, input = ?ev.input_tokens, output = ?ev.output_tokens,
                ms = ev.latency_ms, account = ?ev.account, "proxy request"
            ),
            Some(e) => tracing::warn!(
                route = %ev.route_id, inbound = %ev.inbound, upstream = %ev.upstream, model = %ev.model,
                status, ms = ev.latency_ms, account = ?ev.account, error = %e, "proxy request failed"
            ),
        }
        self.state.events.emit(ev);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.ev.is_some() {
            self.finish_inner(499, Some("client disconnected".into()));
        }
    }
}
