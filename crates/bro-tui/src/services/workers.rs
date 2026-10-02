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
        spawn_voice(svc);
    } else {
        svc.update(|st| st.bridge.status = Avail::Unavailable("turned off in settings".into()));
    }
}

pub(super) fn spawn_data(svc: &Services) {
    let svc = svc.clone();
    std::thread::spawn(move || {
        load_data(&svc);
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
    // Meter requests must not wait for model loading or a cold past-session scan.
    svc.refresh_usage();
    svc.push_bridge_profiles();
    load_models(svc);
    // past sessions can take a while on a cold cache: publish separately
    load_past(svc);
}

/// Re-scan earlier sessions (cheap once the on-disk caches are warm) and publish them.
pub(super) fn load_past(svc: &Services) {
    let past = guard("sessions::list", || bro_core::sessions::list(&ListOpts { limit: 400, harnesses: vec![] }));
    svc.update(|st| st.past = avail(past, &mut st.issues));
    svc.push_known_folders();
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
    fetch_usage_with(svc, &|p| guard_res("usage::fetch", || bro_core::usage::fetch(p)));
}

fn fetch_usage_with(svc: &Services, fetch: &(impl Fn(&bro_core::profiles::Profile) -> Result<bro_core::usage::Usage, String> + Sync)) {
    let profiles: Vec<_> = svc.state().profiles.ready().cloned().unwrap_or_default().into_iter().filter(|p| p.authenticated).collect();
    if profiles.is_empty() {
        return;
    }
    svc.update(|st| {
        for p in &profiles {
            st.usage.entry(p.id.clone()).or_default().fetching = true;
        }
    });
    // Publish each account as it finishes; another account's timeout cannot hold it up.
    let pending = parking_lot::Mutex::new(profiles.iter());
    let claude_cands: Vec<_> = std::thread::scope(|scope| {
        // Bound simultaneous CLI processes / HTTP requests even with a large account pool.
        let jobs: Vec<_> = (0..profiles.len().min(4)).map(|_| scope.spawn(|| {
            let mut candidates = vec![];
            loop {
                let Some(p) = pending.lock().next() else { break };
                let r = fetch(p);
                let entry = match r {
                    Ok(u) => {
                        let head = guard("usage::headroom", || bro_core::usage::headroom(p, &u)).ok();
                        if p.is_claude() {
                            candidates.push((p.clone(), u.clone()));
                        }
                        UsageEntry { usage: Some(u), headroom: head, error: None, fetching: false }
                    }
                    Err(e) => {
                        let old = svc.state().usage.get(&p.id).cloned().unwrap_or_default();
                        UsageEntry { error: Some(e), fetching: false, ..old }
                    }
                };
                svc.update(|st| {
                    st.usage.insert(p.id.clone(), entry);
                });
            }
            candidates
        })).collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap_or_default()).collect()
    });
    let large = guard("usage::pick_by_size", || bro_core::usage::pick_by_size(&claude_cands, true)).ok().flatten();
    let small = guard("usage::pick_by_size", || bro_core::usage::pick_by_size(&claude_cands, false)).ok().flatten();
    svc.update(|st| {
        st.picks = (large, small);
        st.usage_at = Some(Instant::now());
    });
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use bro_core::profiles::{Profile, ProfileKind};
    use bro_core::usage::{Usage, Window};

    #[test]
    fn slow_account_does_not_delay_other_readings_and_failed_refresh_recovers() {
        crate::testkit::isolate();
        let (tx, events) = channel();
        let svc = Services::offline(super::super::fallback_settings(), tx);
        let profile = |name: &str, authenticated: bool| Profile {
            id: format!("codex:{name}"), kind: ProfileKind::CodexProfile, name: name.into(), dir: std::env::temp_dir().join(name),
            authenticated, plan: None, tier: None, identity: None, email: None,
        };
        let usage = |pct| Usage { five_hour: Some(Window { used_pct: pct, resets_at: None, window_mins: Some(300) }), fetched_at: 123, ..Default::default() };
        svc.update(|st| {
            st.profiles = Avail::Ready(vec![profile("slow", true), profile("fast", true), profile("logged-out", false)]);
            st.usage.clear();
            st.usage.insert("codex:slow".into(), UsageEntry { usage: Some(usage(10.0)), ..Default::default() });
        });
        let (release, blocked) = channel();
        let blocked = parking_lot::Mutex::new(blocked);
        let (started, ready) = channel();
        std::thread::scope(|scope| {
            let job = scope.spawn(|| fetch_usage_with(&svc, &|p| {
                match p.name.as_str() {
                    "slow" => {
                        started.send(()).unwrap();
                        blocked.lock().recv().unwrap();
                        Err("timeout".into())
                    }
                    "fast" => Ok(usage(63.0)),
                    _ => panic!("must not fetch an unauthenticated account"),
                }
            }));
            let started = ready.recv_timeout(Duration::from_secs(2)).is_ok();
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut fast_arrived = false;
            while Instant::now() < deadline {
                if svc.state().usage.get("codex:fast").is_some_and(|e| !e.fetching && e.usage.is_some()) {
                    fast_arrived = true;
                    break;
                }
                let _ = events.recv_timeout(Duration::from_millis(10));
            }
            // Release before assertions so a sequential-fetch regression cannot deadlock the test.
            release.send(()).unwrap();
            job.join().unwrap();
            assert!(started && fast_arrived, "fast usage must publish while slow usage is blocked");
        });
        {
            let st = svc.state();
            let slow = &st.usage["codex:slow"];
            assert!(!slow.fetching);
            assert_eq!(slow.error.as_deref(), Some("timeout"));
            assert_eq!(slow.usage.as_ref().unwrap().five_hour.as_ref().unwrap().used_pct, 10.0);
            assert_eq!(st.usage["codex:fast"].usage.as_ref().unwrap().five_hour.as_ref().unwrap().used_pct, 63.0);
            assert!(!st.usage.contains_key("codex:logged-out"));
            assert!(st.usage_at.is_some());
        }
        fetch_usage_with(&svc, &|_| Ok(usage(40.0)));
        let st = svc.state();
        assert!(st.usage.values().all(|e| e.error.is_none() && !e.fetching));
        assert_eq!(st.usage["codex:slow"].usage.as_ref().unwrap().five_hour.as_ref().unwrap().used_pct, 40.0);
    }
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

/// Push-to-talk to the orchestrator: hold the hotkey anywhere, speak, release; the transcript goes to Hugh.
/// Only starts when an API key for transcription is set, so the hotkey is never taken for nothing.
pub(super) fn spawn_voice(svc: &Services) {
    let svc = svc.clone();
    std::thread::spawn(move || {
        let s = svc.settings().voice;
        if !s.enabled || svc.is_demo() {
            return;
        }
        if !bro_voice::Voice::api_key_available(&s.api_key_env) {
            // the hotkey stays free until there is something to transcribe with
            svc.send(Event::Toast(Kind::Info, format!("voice is off: set {} to use push-to-talk with Hugh", s.api_key_env)));
            return;
        }
        let defaults = bro_voice::VoiceConfig::default();
        let cfg = bro_voice::VoiceConfig {
            hotkey: s.hotkey.clone().unwrap_or(defaults.hotkey.clone()),
            api_key_env: s.api_key_env.clone(),
            model: s.model.clone().unwrap_or(defaults.model.clone()),
            language: s.language.clone(),
            keywords: super::voice_keywords(),
            prompt: Some("Voice commands for bro, a terminal workspace: opening agent sessions in projects, sending them tasks, asking about their chats.".into()),
            ..defaults
        };
        let live = s.mode != "transcribe";
        let cfg = bro_voice::VoiceConfig {
            live: live.then(|| bro_voice::LiveConfig {
                model: s.live_model.clone().unwrap_or_else(|| bro_voice::DEFAULT_LIVE_MODEL.into()),
                voice: s.live_voice.clone().unwrap_or_else(|| bro_voice::DEFAULT_LIVE_VOICE.into()),
                instructions: super::LIVE_INSTRUCTIONS.into(),
                ..bro_voice::LiveConfig::default()
            }),
            log: Some(crate::util::bro_dir().join("voice.log")),
            ..cfg
        };
        let hotkey = cfg.hotkey.clone();
        let v_svc = svc.clone();
        // the pane in focus when the key went down is the one the words are for
        let spoken_to = parking_lot::Mutex::new(None::<String>);
        let started = guard_res("voice::start", || {
            bro_voice::Voice::start(
                cfg,
                Box::new(move |ev| match ev {
                    bro_voice::VoiceEvent::Listening => {
                        *spoken_to.lock() = v_svc.focused_session();
                        v_svc.send(Event::Toast(Kind::Info, if live { "listening…" } else { "listening… release to send to Hugh" }.into()));
                    }
                    bro_voice::VoiceEvent::Heard(text) => v_svc.send(Event::Toast(Kind::Info, format!("you: {text}"))),
                    bro_voice::VoiceEvent::Said(text) => v_svc.send(Event::Toast(Kind::Info, format!("Hugh: {text}"))),
                    // GPT-Live hands the words to Hugh; his answer goes back to be spoken
                    bro_voice::VoiceEvent::Delegation { id, request } => {
                        if request.trim().is_empty() {
                            v_svc.voice_delegation_result(&id, "The words didn't come through; ask the user to say that again.");
                        } else {
                            v_svc.orchestrator_send_voice(request, spoken_to.lock().clone(), Some(id));
                        }
                    }
                    bro_voice::VoiceEvent::Transcript(text) => {
                        v_svc.send(Event::Toast(Kind::Info, format!("you: {text}")));
                        v_svc.orchestrator_send_voice(text, spoken_to.lock().take(), None);
                    }
                    bro_voice::VoiceEvent::Error(e) => v_svc.send(Event::Toast(Kind::Error, format!("voice: {e}"))),
                    bro_voice::VoiceEvent::Level(_) | bro_voice::VoiceEvent::Transcribing | bro_voice::VoiceEvent::Cancelled => {}
                }),
            )
        });
        match started {
            Ok(voice) => {
                svc.set_voice(Some(voice));
                svc.push_known_folders();
            }
            Err(e) => svc.send(Event::Toast(Kind::Error, format!("voice ({hotkey}) didn't start: {e}"))),
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
        let o_svc = svc.clone();
        let _ = guard("bridge::on_orchestrator_update", || {
            b.on_orchestrator_update(Box::new(move |u| match u {
                // a spoken question (tagged with its GPT-Live delegation): the voice says the answer
                bro_bridge::OrchestratorUpdate::Reply { text, tag: Some(id) } => o_svc.voice_delegation_result(&id, if text.trim().is_empty() { "Done." } else { text.trim() }),
                bro_bridge::OrchestratorUpdate::Failed { message, tag: Some(id) } => o_svc.voice_delegation_result(&id, &format!("That didn't work: {message}")),
                bro_bridge::OrchestratorUpdate::Step { summary, tag: Some(id) } => o_svc.voice_delegation_progress(&id, &summary),
                bro_bridge::OrchestratorUpdate::Reply { text, tag: None } if !text.trim().is_empty() => o_svc.send(Event::Toast(Kind::Info, format!("Hugh: {}", super::brief(&text)))),
                bro_bridge::OrchestratorUpdate::Failed { message, tag: None } => o_svc.send(Event::Toast(Kind::Error, format!("Hugh: {message}"))),
                _ => {}
            }))
        });
        let qr = guard("bridge::pairing_qr", || b.pairing_qr()).ok().flatten();
        svc.set_bridge(Some(b.clone()));
        svc.update(|st| st.bridge.qr = qr);
        svc.push_bridge_profiles();
        svc.push_known_folders();
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
