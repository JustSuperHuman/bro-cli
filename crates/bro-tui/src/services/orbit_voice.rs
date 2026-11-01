//! Orbit owns realtime speech and its native character; bro owns durable work.
//! Enabled startup launches a verified hidden companion, never a microphone/provider session.

#[cfg(test)]
mod tests;
pub(super) mod relay;

use super::{Services, guard_res};
use crate::alerts::Kind;
use crate::pane::Event;
use bro_bridge::orbit_voice::{self, Handle, StageFrame, StageWindow};
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
struct StageValue {
    frame: Option<StageFrame>,
    revision: u64,
    window: Option<StageWindow>,
    published_window: Option<StageWindow>,
}

/// One bounded latest value, independent of session snapshots and request history.
/// GUI handoff does no serialization or I/O; the trailing worker delivers the final
/// resize/scroll update even when there is no subsequent paint.
#[derive(Default)]
pub(super) struct StageMailbox {
    value: Mutex<StageValue>,
    changed: Condvar,
    started: AtomicBool,
    stopped: AtomicBool,
}

impl StageMailbox {
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.changed.notify_all();
    }
}

#[derive(Clone, Debug, Default)]
pub struct OrbitState {
    pub checking: bool,
    pub connected: bool,
    pub version: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub runtime: Option<orbit_voice::RuntimeStatus>,
    pub owned_elsewhere: bool,
    pub starting: bool,
}

fn wait_audio_off(handle: &Handle) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if handle.runtime_status().is_none_or(|r| r.microphone == orbit_voice::Microphone::Off && r.phase == orbit_voice::Phase::Off) { break; }
        std::thread::sleep(Duration::from_millis(40));
    }
}

impl Services {
    /// Scripted visual fixtures cannot launch audio, providers or a companion.
    #[cfg(feature = "gui")]
    pub(crate) fn orbit_demo_fixture(&self, fixture: &str) {
        if !self.is_demo() { return; }
        let connected = matches!(fixture, "setup" | "ready" | "talking");
        let credential = matches!(fixture, "ready" | "talking");
        self.update(|state| {
            state.settings.orbit_voice.enabled = fixture != "off";
            state.orbit_voice = OrbitState {
                connected, checking: fixture == "connecting", version: Some("1.3.0-bro-preview.1".into()),
                status: if fixture == "talking" { "Listening" } else if connected { "Ready · microphone off" } else { "Waiting for Orbit" }.into(),
                error: (fixture == "error").then(|| "Orbit’s preview checksum failed for src/external-host.js; rebuild and regenerate its compatibility manifest".into()),
                runtime: connected.then(|| orbit_voice::RuntimeStatus { runtime_id: "demo".into(), pid: 42, build_id: "a".repeat(64), renderer_ready: true,
                    preferences: orbit_voice::Preferences { provider: orbit_voice::Provider::Openai, model: "gpt-realtime".into(), voice: "cedar".into(), volume: 75, listen_mode: "always".into() },
                    credential_available: credential, credential_source: if credential { orbit_voice::CredentialSource::Process } else { orbit_voice::CredentialSource::None },
                    microphone: if fixture == "talking" { orbit_voice::Microphone::Active } else { orbit_voice::Microphone::Off },
                    phase: if fixture == "talking" { orbit_voice::Phase::Listening } else { orbit_voice::Phase::Off }, muted: false, failure: None }),
                ..Default::default()
            };
        });
    }

    #[cfg(test)]
    pub(crate) fn attach_orbit_test(&self, handle: Handle) {
        *self.inner.orbit_voice.lock() = Some(handle);
        self.update(|state| state.settings.orbit_voice.enabled = true);
    }

    pub(crate) fn orbit_handle(&self) -> Option<Handle> {
        self.inner.orbit_voice.lock().clone()
    }

    pub(crate) fn orbit_owned_elsewhere(&self) -> bool {
        self.inner.guest || self.state().orbit_voice.owned_elsewhere
    }

    pub(crate) fn orbit_generation(&self) -> u64 {
        self.inner.orbit_generation.load(Ordering::Acquire)
    }

    pub(crate) fn queue_orbit_stage(&self, frame: Option<StageFrame>) {
        if self.inner.guest || self.is_demo() {
            return;
        }
        let mailbox = &self.inner.orbit_stage;
        let mut value = mailbox.value.lock();
        if value.frame == frame {
            return;
        }
        if let Some(frame) = &frame {
            value.window = Some(frame.window.clone());
        }
        value.frame = frame;
        value.revision = value.revision.wrapping_add(1);
        drop(value);
        mailbox.changed.notify_one();
    }

    fn start_orbit_stage_worker(&self) {
        let mailbox = self.inner.orbit_stage.clone();
        if mailbox.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        std::thread::spawn(move || {
            let mut sent: Option<(String, u64)> = None;
            let mut next = Instant::now();
            while !mailbox.stopped.load(Ordering::Acquire) {
                let Some(inner) = weak.upgrade() else { break };
                let handle = inner.orbit_voice.lock().clone();
                drop(inner); // No service ownership or lock survives the independent wait.
                let ready = handle.as_ref().is_some_and(Handle::ready);
                let mut value = mailbox.value.lock();
                let identity = handle
                    .as_ref()
                    .map(|h| (h.instance_id().to_owned(), value.revision));
                let window_ready = value
                    .frame
                    .as_ref()
                    .is_none_or(|f| value.published_window.as_ref() == Some(&f.window));
                if !ready || !window_ready || identity == sent {
                    mailbox.changed.wait_for(&mut value, Duration::from_secs(1));
                    continue;
                }
                if let Some(delay) = next.checked_duration_since(Instant::now()) {
                    mailbox.changed.wait_for(&mut value, delay);
                    continue;
                }
                let frame = value.frame.clone();
                drop(value);
                if let Some(handle) = handle {
                    // Validation, JSON encoding, and watch publication all run off the UI.
                    handle.publish_stage(frame);
                    sent = identity;
                    next = Instant::now() + Duration::from_millis(34);
                }
            }
        });
    }

    /// Reconcile the one managed owner off-thread. Enabled startup is hidden and
    /// device-free; an explicit resume may recover a deliberately stopped runtime.
    pub(crate) fn start_orbit_voice(&self, resume: bool) {
        if self.inner.guest || self.is_demo() { return; }
        if !self.settings().orbit_voice.enabled && let Some(handle) = self.orbit_handle() { handle.revoke_work(); }
        let svc = self.clone();
        let resume_generation = self.inner.orbit_control_generation.load(Ordering::Acquire);
        std::thread::spawn(move || {
            let reconcile = svc.inner.orbit_reconcile.lock();
            if svc.inner.stopping.load(Ordering::Acquire) { return; }
            if !svc.settings().orbit_voice.enabled {
                svc.inner.orbit_generation.fetch_add(1, Ordering::AcqRel);
                let handle = { svc.inner.orbit_voice.lock().take() };
                if let Some(handle) = handle {
                    svc.stop_owned_orbit(&handle);
                }
                svc.update(|st| st.orbit_voice = OrbitState { status: "Disconnected".into(), ..Default::default() });
                return;
            }
            if svc.orbit_handle().is_some_and(|h| !h.work_enabled()) {
                let old = { svc.inner.orbit_voice.lock().take() };
                if let Some(old) = old { svc.stop_owned_orbit(&old); }
                svc.inner.orbit_generation.fetch_add(1, Ordering::AcqRel);
            }
            svc.update(|st| { st.orbit_voice.checking = true; st.orbit_voice.error = None; st.orbit_voice.status = "Connecting Orbit Voice…".into(); });
            // The independent installer handoff survives queued old whole-settings saves.
            if let Ok(preview) = bro_core::orbit_voice::installed_preview()
                && let Err(error) = svc.register_orbit_preview(preview) {
                svc.orbit_error(error.to_string()); return;
            }
            let settings = svc.settings().orbit_voice;
            if !svc.bridge_running() { super::workers::spawn_bridge(&svc); }
            let generation = if svc.orbit_handle().is_some() { svc.orbit_generation() }
                else { svc.inner.orbit_generation.fetch_add(1, Ordering::AcqRel) + 1 };
            let handle = if let Some(handle) = svc.orbit_handle() { handle } else {
                let callback = std::sync::Arc::downgrade(&svc.inner);
                let owner_until = Instant::now() + Duration::from_secs(10);
                loop {
                    if svc.inner.stopping.load(Ordering::Acquire) || !svc.settings().orbit_voice.enabled { return; }
                    let callback = callback.clone();
                    let mut occupied = false;
                    let result = guard_res("orbit::start", || {
                        let result = orbit_voice::start_managed(crate::util::bro_dir(), move |command| {
                            if let Some(inner) = callback.upgrade() { Services { inner }.send(Event::OrbitVoice(generation, command)); }
                        });
                        occupied = result.as_ref().is_err_and(|error| error.is::<orbit_voice::ManagedOwnerBusy>());
                        result
                    });
                    match result {
                        Ok(handle) => { handle.attach_workspace(svc.inner.bridge.read().clone()); *svc.inner.orbit_voice.lock() = Some(handle.clone()); break handle; }
                        Err(_) if occupied && Instant::now() < owner_until => {
                            // A former window may still be closing its listener after PTY-driver takeover.
                            // Wait only for the kernel lease; never adopt or terminate another runtime.
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Err(error) => {
                            svc.update(|st| { st.orbit_voice.checking = false; st.orbit_voice.owned_elsewhere = occupied;
                                st.orbit_voice.status = if occupied { "Your primary bro controls Orbit Voice" } else { "Unable to connect Orbit Voice" }.into();
                                st.orbit_voice.error = Some(error.to_string()); });
                            return;
                        }
                    }
                }
            };
            svc.start_orbit_stage_worker();
            svc.update(|st| st.orbit_voice.owned_elsewhere = false); // wake host publication before handshake
            if resume && svc.inner.orbit_control_generation.load(Ordering::Acquire) == resume_generation
                && let Err(error) = handle.resume_runtime() { svc.orbit_error(error); return; }
            // Give a same-lineage runtime time to observe descriptor rotation before considering launch.
            let handover_until = Instant::now() + Duration::from_millis(1800);
            while handle.previously_owned_runtime() && handle.runtime_status().is_none() && Instant::now() < handover_until {
                if svc.orbit_generation() != generation || !svc.settings().orbit_voice.enabled { return; }
                std::thread::sleep(Duration::from_millis(50));
            }
            let detection = bro_core::orbit_voice::detect(&settings);
            let mut launch_error = None;
            if handle.runtime_status().is_none() && !handle.intentionally_stopped() && !handle.launch_pending() && !svc.inner.stopping.load(Ordering::Acquire) {
                match bro_core::orbit_voice::launch_managed(&settings, handle.descriptor_path()) {
                    Ok(launched) => {
                        handle.expect_runtime_process(&launched);
                        // A surviving companion watches this exact pairing file. Give it
                        // time to authenticate before recovering a stuck/closing process.
                        {
                            let until = Instant::now() + Duration::from_secs(if launched.reused { 6 } else { 12 });
                            while handle.runtime_status().is_none() && Instant::now() < until {
                                if svc.inner.stopping.load(Ordering::Acquire) || !svc.settings().orbit_voice.enabled || svc.orbit_generation() != generation || handle.intentionally_stopped() { return; }
                                std::thread::sleep(Duration::from_millis(50));
                            }
                            if handle.runtime_status().is_none() {
                                svc.update(|st| st.orbit_voice.status = "Restarting Orbit connection…".into());
                                match bro_core::orbit_voice::restart_managed(&settings, handle.descriptor_path(), &launched, || {
                                    handle.runtime_status().is_none() && !handle.intentionally_stopped()
                                        && !svc.inner.stopping.load(Ordering::Acquire) && svc.settings().orbit_voice.enabled
                                        && svc.orbit_generation() == generation
                                }) {
                                    Ok(Some(restarted)) => handle.expect_runtime_process(&restarted),
                                    Ok(None) => {},
                                    Err(error) => launch_error = Some(error.to_string()),
                                }
                            }
                        }
                    }
                    Err(error) => launch_error = Some(error.to_string()),
                }
            }
            svc.update(|st| { st.orbit_voice.version = detection.version().map(str::to_owned); st.orbit_voice.error = launch_error.clone(); });
            if svc.inner.orbit_observer_generation.swap(generation, Ordering::AcqRel) == generation { return; }
            drop(reconcile);
            // Weak, generation-bound observer: it never respawns a missing heartbeat.
            let weak = Arc::downgrade(&svc.inner);
            drop(svc);
            loop {
                let Some(inner) = weak.upgrade() else { break };
                let svc = Services { inner };
                if svc.orbit_generation() != generation || !svc.settings().orbit_voice.enabled { break; }
                let runtime = handle.runtime_status();
                let connecting = runtime.is_none() && handle.launch_pending() && !handle.intentionally_stopped();
                svc.update(|st| {
                    st.orbit_voice.checking = connecting;
                    st.orbit_voice.connected = runtime.is_some();
                    if runtime.is_some() { st.orbit_voice.error = None; }
                    st.orbit_voice.status = match &runtime {
                        Some(r) if !r.renderer_ready => "Preparing voice controls…",
                        Some(r) if r.failure.is_some() => match r.failure.unwrap() {
                            orbit_voice::RuntimeFailure::MicrophoneDenied => "Microphone access denied · check Advanced settings",
                            orbit_voice::RuntimeFailure::MicrophoneFailed => "Microphone unavailable · check your device and retry",
                            orbit_voice::RuntimeFailure::ProviderConnectionFailed => "Voice connection failed · check the selected provider and key source",
                        },
                        Some(r) if r.microphone == orbit_voice::Microphone::Denied => "Microphone access was denied. Open Advanced settings to try again.",
                        Some(r) if r.phase != orbit_voice::Phase::Off => match r.phase {
                            orbit_voice::Phase::Connecting => "Connecting voice…", orbit_voice::Phase::Listening => "Listening",
                            orbit_voice::Phase::Thinking => "Thinking", orbit_voice::Phase::Speaking => "Speaking", _ => "Ready",
                        },
                        Some(r) if !r.credential_available => "Setup required · choose a provider and key source",
                        Some(_) if handle.intentionally_stopped() => "Stopped · choose Start to resume",
                        Some(_) => "Ready · microphone off",
                        None if handle.intentionally_stopped() => "Stopped · choose Start to resume",
                        None if connecting => "Connecting Orbit Voice…",
                        None if st.orbit_voice.error.is_some() => "Unable to connect · retry when ready",
                        None => "Disconnected · choose Start to reconnect",
                    }.into();
                    st.orbit_voice.runtime = runtime;
                });
                drop(svc);
                std::thread::sleep(Duration::from_millis(750));
            }
        });
    }

    fn orbit_error(&self, error: String) {
        self.update(|st| { st.orbit_voice.checking = false; st.orbit_voice.starting = false; st.orbit_voice.status = "Unable to connect".into(); st.orbit_voice.error = Some(error); });
    }

    pub(crate) fn toggle_orbit_voice(&self) {
        let state = self.state().orbit_voice.clone();
        let talking = state.starting || state.runtime.as_ref().is_some_and(|r| r.phase != orbit_voice::Phase::Off);
        self.orbit_control(if talking { orbit_voice::VoiceControl::Stop {} } else { orbit_voice::VoiceControl::Start {} });
    }

    pub(crate) fn mute_orbit_voice(&self) {
        let runtime = self.state().orbit_voice.runtime.clone();
        if let Some(runtime) = runtime.filter(|r| r.phase != orbit_voice::Phase::Off) {
            self.orbit_control(orbit_voice::VoiceControl::SetMute { muted: !runtime.muted });
        }
    }

    pub(crate) fn open_orbit_voice(&self) {
        self.orbit_control(orbit_voice::VoiceControl::ShowSettings {});
    }

    /// A native user action, deliberately separate from work/model tools.
    pub(crate) fn orbit_control(&self, command: orbit_voice::VoiceControl) {
        if self.inner.stopping.load(Ordering::Acquire) { return; }
        if self.is_demo() { self.send(Event::Toast(Kind::Info, "The demo never opens a microphone or provider connection.".into())); return; }
        if self.orbit_owned_elsewhere() { self.relay_orbit_control(command); return; }
        let control_generation = self.inner.orbit_control_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let starting = matches!(command, orbit_voice::VoiceControl::Start {});
        self.update(|st| st.orbit_voice.starting = starting);
        let connect = matches!(command, orbit_voice::VoiceControl::Start {} | orbit_voice::VoiceControl::ShowSettings {});
        if connect {
            let mut settings = self.settings(); settings.orbit_voice.enabled = true; self.save_settings(settings);
            self.start_orbit_voice(true);
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let quitting = matches!(command, orbit_voice::VoiceControl::Quit {});
            let mut saved_quit = false;
            let deadline = Instant::now() + Duration::from_secs(if connect { 30 } else if quitting { 15 } else { 2 });
            loop {
                if svc.inner.orbit_control_generation.load(Ordering::Acquire) != control_generation { return; }
                if let Some(handle) = svc.orbit_handle() {
                    if quitting && !saved_quit {
                        if let Err(error) = handle.suppress_runtime() { svc.orbit_error(error); return; }
                        saved_quit = true;
                    }
                    if let Some(runtime) = handle.runtime_status() {
                        if (runtime.renderer_ready && (!matches!(command, orbit_voice::VoiceControl::Start {}) || !handle.intentionally_stopped())) || !connect {
                            if matches!(command, orbit_voice::VoiceControl::Start {}) && !runtime.credential_available {
                                svc.update(|st| st.orbit_voice.starting = false);
                                svc.send(Event::Toast(Kind::Info, "Choose a realtime provider and key source in Advanced Orbit settings first.".into())); return;
                            }
                            if let Err(error) = handle.control(command) {
                                svc.update(|st| st.orbit_voice.starting = false);
                                svc.send(Event::Toast(Kind::Error, error));
                            } else if starting {
                                let until = Instant::now() + Duration::from_secs(11);
                                while svc.inner.orbit_control_generation.load(Ordering::Acquire) == control_generation
                                    && handle.runtime_status().is_none_or(|r| r.phase == orbit_voice::Phase::Off)
                                    && Instant::now() < until { std::thread::sleep(Duration::from_millis(50)); }
                                if svc.inner.orbit_control_generation.load(Ordering::Acquire) == control_generation {
                                    svc.update(|st| st.orbit_voice.starting = false);
                                }
                            }
                            return;
                        }
                    } else if matches!(command, orbit_voice::VoiceControl::Stop {}) { return; }
                } else if !connect && !quitting { return; }
                if Instant::now() >= deadline {
                    svc.update(|st| st.orbit_voice.starting = false);
                    if quitting { return; }
                    svc.send(Event::Toast(Kind::Error, "Orbit’s voice controls are not ready. Check the connection and retry; no microphone was started.".into())); return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }

    pub(crate) fn orbit_background_wanted(&self) -> bool {
        let s = self.settings();
        s.orbit_voice.enabled && s.orbit_voice.keep_in_background && s.server.keep_sessions && s.server.keep_working
    }

    pub(crate) fn prepare_orbit_handover(&self) { self.inner.orbit_handover.store(true, Ordering::Release); }

    pub(crate) fn stop_owned_orbit(&self, handle: &Handle) {
        handle.revoke_work();
        if !self.inner.orbit_handover.load(Ordering::Acquire) {
            let ready_until = Instant::now() + Duration::from_secs(3);
            while handle.launch_pending() && handle.runtime_status().is_none() && Instant::now() < ready_until { std::thread::sleep(Duration::from_millis(40)); }
            let _ = handle.control(orbit_voice::VoiceControl::Shutdown {});
            wait_audio_off(handle);
            let deadline = Instant::now() + Duration::from_secs(2);
            while handle.runtime_status().is_some() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(40)); }
        }
        handle.shutdown();
    }

    /// Explicit installed-fork reload: validate off-thread, merge only launch paths
    /// into the live settings owner, persist, then use the ordinary scoped launcher.
    pub(crate) fn reload_installed_orbit_preview(&self) {
        if self.is_demo() || self.inner.guest || self.state().orbit_voice.checking {
            return;
        }
        self.update(|st| {
            st.orbit_voice.checking = true;
            st.orbit_voice.error = None;
        });
        let svc = self.clone();
        std::thread::spawn(move || {
            let result = guard_res("orbit::register_preview", || {
                let preview = bro_core::orbit_voice::installed_preview()?;
                svc.register_orbit_preview(preview)
            });
            svc.update(|st| {
                st.orbit_voice.checking = false;
                if let Err(error) = &result {
                    st.orbit_voice.error = Some(error.to_string());
                }
            });
            match result {
                Ok(()) => svc.start_orbit_voice(true),
                Err(error) => svc.send(Event::Toast(Kind::Error, error.to_string())),
            }
        });
    }

    // Blocking: shares the disk-write lane with every queued settings save.
    fn register_orbit_preview(
        &self,
        preview: bro_core::orbit_voice::PreviewInstallation,
    ) -> anyhow::Result<()> {
        let _write = self.inner.settings_io.lock();
        let (previous, previous_pin, latest) = {
            let mut state = self.inner.state.write();
            let mut pin = self.inner.orbit_preview.write();
            let previous = state.settings.orbit_voice.clone();
            let previous_pin = pin.replace(preview.clone());
            preview.apply(&mut state.settings.orbit_voice);
            (previous, previous_pin, state.settings.clone())
        };
        if let Err(error) = latest.save() {
            // A failed write must not change the selected runtime. Preserve unrelated
            // options that the UI may have edited while the atomic write was pending.
            let mut state = self.inner.state.write();
            state.settings.orbit_voice.executable = previous.executable;
            state.settings.orbit_voice.application_dir = previous.application_dir;
            *self.inner.orbit_preview.write() = previous_pin;
            return Err(error);
        }
        self.wake();
        Ok(())
    }

    pub(crate) fn check_orbit_voice_release(&self) {
        self.send(Event::Toast(Kind::Info, "Orbit Voice is a Windows compatibility preview. Install a newer tested preview to update it; your connection stays available.".into()));
    }

    pub(crate) fn orbit_repository(&self, upstream: bool) {
        if self.is_demo() {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            if let Err(error) = bro_core::orbit_voice::open_repository(upstream) {
                svc.send(Event::Toast(Kind::Error, error.to_string()));
            }
        });
    }

    pub(crate) fn publish_orbit_host(&self, mut snapshot: serde_json::Value) {
        if let Some(handle) = self.orbit_handle() {
            let window = {
                let value = self.inner.orbit_stage.value.lock();
                value
                    .window
                    .as_ref()
                    .filter(|w| {
                        snapshot["window"]["handle"].as_str() == Some(w.handle.as_str())
                            && snapshot["window"]["pid"].as_u64() == Some(u64::from(w.pid))
                    })
                    .cloned()
            };
            if let Some(window) = &window {
                snapshot["window"]["epoch"] = window.epoch.clone().into();
            }
            if let Some(sessions) = snapshot["sessions"].as_array() {
                let generations = sessions.iter().filter_map(|s| Some((s["id"].as_str()?, s["generation"].as_str()?))).collect::<Vec<_>>();
                self.with_bridge("bridge::voice_generations", |workspace| workspace.bind_voice_generations(&generations));
            }
            if handle.publish_host(snapshot) {
                self.inner.orbit_stage.value.lock().published_window = window;
                self.inner.orbit_stage.changed.notify_one();
            }
        }
    }
}
