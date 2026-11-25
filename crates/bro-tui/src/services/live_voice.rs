//! Live voice calls through bro's bridge: the phone app's voice mode and the `/voice` page. The
//! bridge owns the calls (OpenAI Realtime over WebRTC, with bro's tools on a sideband socket); bro's
//! window only configures them, shows who is on a call, and can end them.

use super::Services;
use crate::alerts::Kind;
use crate::pane::Event;
use bro_core::config::VoiceSettings;

/// What the bridge serves live calls with.
pub(crate) fn bridge_config(voice: &VoiceSettings) -> bro_bridge::voice::VoiceConfig {
    let defaults = bro_bridge::voice::VoiceConfig::default();
    bro_bridge::voice::VoiceConfig {
        enabled: voice.calls,
        model: voice.realtime_model.clone().filter(|m| !m.trim().is_empty()).unwrap_or(defaults.model),
        voice: voice
            .realtime_voice
            .clone()
            .filter(|v| bro_bridge::voice::VOICES.contains(&v.as_str()))
            .unwrap_or(defaults.voice),
        api_key_env: if voice.api_key_env.trim().is_empty() { defaults.api_key_env } else { voice.api_key_env.clone() },
        announce: bro_bridge::voice::Announce::parse(&voice.announce).unwrap_or_default(),
        ..defaults
    }
}

/// Spoken-word labels for the announce setting.
pub(crate) fn announce_label(mode: &str) -> &'static str {
    match bro_bridge::voice::Announce::parse(mode).unwrap_or_default() {
        bro_bridge::voice::Announce::All => "Any agent finishes or needs you",
        bro_bridge::voice::Announce::Asked => "Agents you talked to",
        bro_bridge::voice::Announce::Off => "Only when you ask",
    }
}

/// "On your phone", "In a browser".
pub(crate) fn client_label(client: &str) -> &'static str {
    if client == "phone" { "On your phone" } else { "In a browser" }
}

/// "Listening", "Speaking", … for a live call's state.
pub(crate) fn state_label(state: &str) -> &'static str {
    match state {
        "connecting" => "Connecting",
        "hearing" => "Hearing you",
        "thinking" => "On it",
        "speaking" => "Speaking",
        _ => "Listening",
    }
}

impl Services {
    /// The bridge takes new live voice settings at once (calls already running keep theirs).
    pub(crate) fn push_voice_config(&self, voice: &VoiceSettings) {
        let config = bridge_config(voice);
        self.with_bridge("bridge::set_voice_config", |bridge| bridge.set_voice_config(config));
    }

    /// Opens the voice page in the default browser: talk to bro on this computer.
    pub(crate) fn open_voice_page(&self) {
        if self.is_demo() {
            self.send(Event::Toast(Kind::Info, "The demo doesn't open a microphone or a voice call.".into()));
            return;
        }
        let url = { self.inner.bridge.read().as_ref().and_then(|bridge| bridge.voice_page_url()) };
        match url {
            Some(url) => self.send(Event::OpenLink(url)),
            None => self.send(Event::Toast(
                Kind::Info,
                "Turn on the phone & web bridge in Settings → Server to talk to bro from a browser or your phone.".into(),
            )),
        }
    }

    /// Ends every live call (phone and browser).
    pub(crate) fn hangup_live_voice(&self) {
        if self.is_demo() {
            self.update(|state| state.bridge.voice_calls.clear());
            return;
        }
        self.with_bridge("bridge::voice_hangup_all", |bridge| bridge.voice_hangup_all());
    }

    /// Scripted visual fixtures: live voice states without a bridge, microphone or provider.
    #[cfg(feature = "gui")]
    pub(crate) fn live_voice_demo_fixture(&self, fixture: &str) {
        if !self.is_demo() {
            return;
        }
        let call = |client: &str, state: &str, minutes: i64| bro_bridge::voice::LiveCall {
            id: format!("rtc_demo_{client}"),
            client: client.into(),
            started_at: (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339(),
            state: state.into(),
        };
        self.update(|state| {
            state.bridge.enabled = fixture != "off";
            state.bridge.voice_ready = match fixture {
                "off" => None,
                "nokey" => Some(Err("bro needs an OpenAI key for live voice. Set OPENAI_API_KEY on this computer, then try again.".into())),
                _ => Some(Ok(())),
            };
            state.bridge.voice_calls = match fixture {
                "phone" => vec![call("phone", "speaking", 4)],
                "two" => vec![call("phone", "listening", 12), call("web", "thinking", 1)],
                _ => vec![],
            };
        });
    }
}
