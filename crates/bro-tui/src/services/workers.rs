//! The background threads behind [`Services`]: data loading, the 60 s usage refresher, proxy + bridge startup
//! and the bridge status poller. Every dependency call is guarded.

use super::{Avail, ProxyInfo, Services, UsageEntry, guard, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::config::Config;
use bro_core::sessions::ListOpts;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// Everything a real run starts with.
pub(super) fn spawn_all(svc: &Services) {
    spawn_usage_loop(svc);
    spawn_data(svc);
    let s = svc.settings();
    if s.proxy.enabled {
        spawn_proxy(svc);
    } else {
        svc.update(|st| st.proxy.status = Avail::Unavailable("turned off in settings".into()));
    }
    if s.bridge.enabled {
        spawn_bridge(svc);
    } else {
        svc.update(|st| st.bridge.status = Avail::Unavailable("turned off in settings".into()));
    }
}

pub(super) fn spawn_data(svc: &Services) {
    let svc = svc.clone();
    std::thread::spawn(move || {
        load_data(&svc);
        svc.refresh_usage();
    });
}

/// Config, profiles, providers and past sessions. Blocking; call on a worker thread.
pub(super) fn load_data(svc: &Services) {
    let config = guard_res("config::Config::load", Config::load);
    let cfg_for_providers = config.clone().unwrap_or_default();
    let profiles = guard("profiles::list", bro_core::profiles::list);
    let providers = guard("providers::load", || bro_core::providers::load(&cfg_for_providers));
    svc.update(|st| {
        match &config {
            Ok(c) => st.config = Some(c.clone()),
            Err(e) => note(st, e),
        }
        st.profiles = avail(profiles, &mut st.issues);
        st.providers = avail(providers, &mut st.issues);
    });
    svc.push_bridge_profiles();
    load_models(svc);
    // past sessions can take a while on a cold cache: publish separately
    let past = guard("sessions::list", || bro_core::sessions::list(&ListOpts { limit: 400, harnesses: vec![] }));
    svc.update(|st| st.past = avail(past, &mut st.issues));
}

/// Model lists for the launcher; then, off the critical path, refresh the OpenRouter catalogue when it's
/// older than six hours and publish again.
pub(super) fn load_models(svc: &Services) {
    let publish = |svc: &Services| {
        let providers = svc.state().providers.ready().cloned().unwrap_or_default();
        let models = guard("catalogue", || {
            use bro_core::catalogue;
            let mut m = std::collections::BTreeMap::new();
            for p in &providers {
                m.insert(p.id.clone(), catalogue::provider_models(p));
            }
            m.insert("codex".to_string(), catalogue::codex_models(&bro_core::paths::codex_local_dir()));
            let claude = providers.iter().find(|p| p.id == "anthropic").map(catalogue::provider_models).unwrap_or_default();
            m.insert("claude".to_string(), claude);
            m
        });
        if let Ok(m) = models {
            svc.update(|st| st.models = m);
        }
    };
    publish(svc);
    let stale = guard("catalogue::openrouter_cached", || bro_core::catalogue::openrouter_cached().is_none_or(|(_, age)| age > 6 * 3600)).unwrap_or(false);
    if stale {
        let svc = svc.clone();
        std::thread::spawn(move || {
            if guard_res("catalogue::refresh_openrouter", bro_core::catalogue::refresh_openrouter).is_ok() {
                publish(&svc);
            }
        });
    }
}

fn note(st: &mut super::State, e: &str) {
    if !st.issues.iter().any(|i| i == e) {
        st.issues.push(e.to_string());
    }
}

fn avail<T>(r: Result<T, String>, issues: &mut Vec<String>) -> Avail<T> {
    match r {
        Ok(v) => Avail::Ready(v),
        Err(e) => {
            if !issues.contains(&e) {
                issues.push(e.clone());
            }
            Avail::Unavailable(e)
        }
    }
}

/// Usage every 60 s, or when kicked (`Services::refresh_usage`).
fn spawn_usage_loop(svc: &Services) {
    let (ktx, krx) = channel::<()>();
    svc.set_usage_kick(ktx);
    let svc = svc.clone();
    std::thread::spawn(move || {
        while let Ok(()) | Err(RecvTimeoutError::Timeout) = krx.recv_timeout(Duration::from_secs(60)) {
            while krx.try_recv().is_ok() {} // collapse a burst of kicks
            fetch_usage(&svc);
        }
    });
}

fn fetch_usage(svc: &Services) {
    let profiles: Vec<_> = svc.state().profiles.ready().cloned().unwrap_or_default().into_iter().filter(|p| p.authenticated).collect();
    if profiles.is_empty() {
        return;
    }
    svc.update(|st| {
        for p in &profiles {
            st.usage.entry(p.id.clone()).or_default().fetching = true;
        }
    });
    let mut claude_cands = vec![];
    for p in &profiles {
        let r = guard_res("usage::fetch", || bro_core::usage::fetch(p));
        let entry = match r {
            Ok(u) => {
                let head = guard("usage::headroom", || bro_core::usage::headroom(p, &u)).ok();
                if p.is_claude() {
                    claude_cands.push((p.clone(), u.clone()));
                }
                UsageEntry { usage: Some(u), headroom: head, error: None, fetching: false }
            }
            Err(e) => {
                let old = svc.state().usage.get(&p.id).cloned().unwrap_or_default();
                UsageEntry { error: Some(e), fetching: false, ..old }
            }
        };
        let id = p.id.clone();
        svc.update(|st| {
            st.usage.insert(id, entry);
        });
    }
    let large = guard("usage::pick_by_size", || bro_core::usage::pick_by_size(&claude_cands, true)).ok().flatten();
    let small = guard("usage::pick_by_size", || bro_core::usage::pick_by_size(&claude_cands, false)).ok().flatten();
    svc.update(|st| {
        st.picks = (large, small);
        st.usage_at = Some(Instant::now());
    });
}

/// Start the translating proxy on its own runtime thread.
pub(super) fn spawn_proxy(svc: &Services) {
    let svc = svc.clone();
    std::thread::spawn(move || {
        let s = svc.settings().proxy;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let cfg = bro_proxy::ProxyConfig { bind: "127.0.0.1".into(), port: s.port, token: Some(token.clone()) };
        match guard_res("proxy::start", || bro_proxy::ProxyHandle::start(cfg)) {
            Ok(h) => {
                let port = guard("proxy::port", || h.port()).unwrap_or(s.port);
                let base = guard("proxy::base_url", || h.base_url()).unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));
                let ev_svc = svc.clone();
                let _ = guard("proxy::on_event", || {
                    h.on_event(Box::new(move |e| {
                        ev_svc.update(|st| {
                            st.proxy.events.push_back(e);
                            while st.proxy.events.len() > super::EVENT_CAP {
                                st.proxy.events.pop_front();
                            }
                        })
                    }))
                });
                let recent = guard("proxy::recent", || h.recent(super::EVENT_CAP)).unwrap_or_default();
                svc.set_proxy(Some(h));
                svc.update(|st| {
                    st.proxy.status = Avail::Ready(ProxyInfo { port, base, token });
                    st.proxy.events = recent.into_iter().collect();
                });
                // pool account status, every 5 s while the proxy runs
                loop {
                    let dirs: Vec<std::path::PathBuf> = svc
                        .state()
                        .profiles
                        .ready()
                        .map(|ps| ps.iter().filter(|p| p.kind == bro_core::profiles::ProfileKind::ClaudeAccount && p.authenticated).map(|p| p.dir.clone()).collect())
                        .unwrap_or_default();
                    match svc.with_proxy("proxy::pool_status", |p| p.pool_status(&dirs)) {
                        Some(pool) => svc.update(|st| st.proxy.pool = pool),
                        None => break,
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
            Err(e) => {
                svc.update(|st| st.proxy.status = Avail::Unavailable(e.clone()));
                if !super::guard::is_unimplemented(&e) {
                    svc.send(Event::Toast(Kind::Error, format!("proxy didn't start: {e}")));
                }
            }
        }
    });
}

/// Start the bridge host and its status poller.
pub(super) fn spawn_bridge(svc: &Services) {
    let svc = svc.clone();
    std::thread::spawn(move || {
        let s = svc.settings().bridge;
        let cfg = bro_bridge::BridgeConfig { port: s.port, automatic_port: s.automatic_port, bind: s.bind.clone(), data_root: crate::util::bro_dir().join("bridge"), web_interface: s.web_interface };
        let cmd_svc = svc.clone();
        let started = guard_res("bridge::start", || bro_bridge::Bridge::start(cfg, Box::new(move |c| cmd_svc.send(Event::Bridge(c)))));
        let b = match started {
            Ok(b) => b,
            Err(e) => {
                svc.update(|st| st.bridge.status = Avail::Unavailable(e.clone()));
                if !super::guard::is_unimplemented(&e) {
                    svc.send(Event::Toast(Kind::Error, format!("bridge didn't start: {e}")));
                }
                return;
            }
        };
        let n_svc = svc.clone();
        let _ = guard("bridge::on_notification", || {
            b.on_notification(Box::new(move |n| n_svc.send(Event::Toast(Kind::Bridge, if n.body.is_empty() { n.title } else { format!("{} — {}", n.title, n.body) }))))
        });
        let qr = guard("bridge::pairing_qr", || b.pairing_qr()).ok().flatten();
        svc.set_bridge(Some(b.clone()));
        svc.update(|st| st.bridge.qr = qr);
        svc.push_bridge_profiles();
        svc.send(Event::BridgeStarted);
        // poll status while this bridge is the live one
        loop {
            match guard("bridge::status", || b.status()) {
                Ok(status) => svc.update(|st| st.bridge.status = Avail::Ready(status)),
                Err(e) => {
                    svc.update(|st| st.bridge.status = Avail::Unavailable(e));
                    break;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
            if !svc.bridge_running() || !svc.state().bridge.enabled {
                break;
            }
        }
    });
}
