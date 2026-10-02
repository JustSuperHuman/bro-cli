//! bro-voice — push-to-talk voice input for bro.
//!
//! Hold the hotkey (Right Win / Right Option / Right Super by default on
//! Windows / macOS / Linux) to record from the default microphone;
//! release to transcribe it through an OpenAI-compatible
//! `/audio/transcriptions` endpoint. Three kinds of background thread do the
//! work: a global `WH_KEYBOARD_LL` hook with its own message loop (decides
//! swallow/pass in microseconds and forwards signals), a worker that owns the
//! audio stream, and one short-lived thread per transcription. Results arrive
//! through the `on_event` callback on those threads; nothing blocks the caller
//! of [`Voice::start`] beyond installing the hook.
//!
//! Cross-platform: the hotkey backends live in [`hotkey`] (Windows hook, macOS
//! event tap, Linux evdev); capture (cpal) and transcription (ureq) are
//! portable. Swallowing the hotkey is best-effort: Windows yes, macOS for
//! non-modifier keys, Linux never.
//!
//! Contract rule: the public items declared here are the API bro-tui builds
//! against. Add freely; do not rename or remove.

mod audio;
mod hotkey;
mod live;
mod playback;
mod transcribe;
mod worker;

pub use live::{DEFAULT_LIVE_MODEL, DEFAULT_LIVE_URL, DEFAULT_LIVE_VOICE, LiveConfig};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// OpenAI's recommended speech-to-text model for `/v1/audio/transcriptions`
/// (accepts `prompt`, `keywords[]`, `languages[]`). If a server rejects it the
/// request is retried once with `whisper-1`.
pub const DEFAULT_MODEL: &str = "gpt-transcribe";
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_API_KEY_ENV: &str = "OPENAI_API_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceConfig {
    /// `rwin` (= `rsuper` / `rcmd` / `rmeta`), `lwin`, `rctrl`, `ralt` (=
    /// `ropt` / `roption`), `rshift`, `capslock`, `scrolllock`, `pause`,
    /// `insert` (= `help`), `f13`..`f24`, `apps`, or `vk:0xNN` (Windows VK).
    /// Not every key exists on every platform; `start` says so.
    pub hotkey: String,
    /// The hotkey never reaches other apps while it is used for talking (a tap
    /// or a chord is replayed so they still work).
    pub swallow: bool,
    /// Environment variable holding the API key (process env, then registry).
    pub api_key_env: String,
    pub base_url: String,
    pub model: String,
    /// ISO-639-1 hint (`en`); `None` = auto-detect.
    pub language: Option<String>,
    /// Free-text context / vocabulary bias.
    pub prompt: Option<String>,
    /// Words to spell exactly (session and project names). Sent as
    /// `keywords[]` to `gpt-transcribe`, folded into the prompt for other models.
    pub keywords: Vec<String>,
    /// Holds shorter than this are taps (the key does its normal thing).
    pub min_ms: u32,
    /// Recording stops and is transcribed after this long.
    pub max_secs: u32,
    /// GPT-Live: talk to a live voice that answers out loud and delegates work
    /// to the host ([`VoiceEvent::Delegation`]). `None` = transcribe only.
    pub live: Option<LiveConfig>,
    /// Append a diagnostic trace (sessions, audio counts, server events) here.
    pub log: Option<std::path::PathBuf>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        VoiceConfig {
            hotkey: default_hotkey().into(),
            swallow: true,
            api_key_env: DEFAULT_API_KEY_ENV.into(),
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            language: None,
            prompt: None,
            keywords: Vec::new(),
            min_ms: 300,
            max_secs: 90,
            live: None,
            log: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEvent {
    /// The microphone is open and recording.
    Listening,
    /// Input level, 0..1 (RMS mapped from -60..0 dBFS), ~15 per second.
    Level(f32),
    /// Released; the take is being transcribed.
    Transcribing,
    Transcript(String),
    /// Nothing to deliver: a tap, a chord, silence, or an empty transcript.
    Cancelled,
    /// One readable line.
    Error(String),
    /// Live mode: what the user said (one utterance, transcribed by GPT-Live).
    Heard(String),
    /// Live mode: what the voice said out loud.
    Said(String),
    /// Live mode: the voice handed work to the host. Run `request` (the user's
    /// words since the previous delegation) and answer with
    /// [`Voice::delegation_result`]; the voice speaks the answer.
    Delegation { id: String, request: String },
}

/// The platform's default hotkey name (`rwin`, `ropt` on macOS, `rsuper` on
/// Linux).
pub fn default_hotkey() -> &'static str {
    hotkey::default_hotkey()
}

type OnEvent = Box<dyn Fn(VoiceEvent) + Send + Sync>;

/// State shared with the background threads.
pub(crate) struct Shared {
    pub(crate) cfg: VoiceConfig,
    pub(crate) prompt: Mutex<Option<String>>,
    pub(crate) keywords: Mutex<Vec<String>>,
    stopped: AtomicBool,
    on_event: OnEvent,
}

impl Shared {
    /// One line in the diagnostic trace, if one is configured. Never fails.
    pub(crate) fn log(&self, line: impl AsRef<str>) {
        let Some(path) = &self.cfg.log else { return };
        use std::io::Write;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("{}.{:03}", d.as_secs(), d.subsec_millis()))
            .unwrap_or_default();
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{stamp} {}", line.as_ref());
        }
    }

    pub(crate) fn emit(&self, event: VoiceEvent) {
        match &event {
            VoiceEvent::Level(_) => {}
            other => self.log(format!("event {other:?}")),
        }
        if !self.stopped.load(Ordering::Acquire) {
            (self.on_event)(event);
        }
    }
}

struct Inner {
    shared: Arc<Shared>,
    listener: Mutex<Option<hotkey::Listener>>,
    worker: std::sync::mpsc::Sender<worker::Msg>,
}

impl Inner {
    fn stop(&self) {
        if self.shared.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.worker.send(worker::Msg::Stop);
        if let Some(mut listener) = self.listener.lock().unwrap_or_else(|p| p.into_inner()).take() {
            listener.stop();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A running push-to-talk listener. Cheap to clone; stops when the last clone
/// drops (or on [`Voice::stop`]).
#[derive(Clone)]
pub struct Voice {
    inner: Arc<Inner>,
}

impl Voice {
    /// Install the global hotkey and start listening. Fails fast on a bad hotkey
    /// name, a key the platform lacks, or a hook the OS refuses (with how to
    /// grant access on macOS/Linux). The API
    /// key and microphone are checked per take and reported as `Error` events.
    pub fn start(cfg: VoiceConfig, on_event: Box<dyn Fn(VoiceEvent) + Send + Sync>) -> anyhow::Result<Voice> {
        let vk = Self::parse_hotkey(&cfg.hotkey).ok_or_else(|| anyhow::anyhow!("unknown voice hotkey {:?}", cfg.hotkey))?;
        let shared = Arc::new(Shared {
            prompt: Mutex::new(cfg.prompt.clone()),
            keywords: Mutex::new(cfg.keywords.clone()),
            cfg,
            stopped: AtomicBool::new(false),
            on_event,
        });
        Self::start_platform(shared, vk)
    }

    fn start_platform(shared: Arc<Shared>, vk: u32) -> anyhow::Result<Voice> {
        let (tx, rx) = std::sync::mpsc::channel();
        let signal_tx = tx.clone();
        let listener = hotkey::listen(
            vk,
            shared.cfg.swallow,
            shared.cfg.min_ms,
            Box::new(move |signal| {
                let _ = signal_tx.send(worker::Msg::Key(signal));
            }),
        )?;
        let worker_shared = shared.clone();
        let worker_tx = tx.clone();
        std::thread::Builder::new()
            .name("bro-voice-worker".into())
            .spawn(move || worker::run(worker_shared, worker_tx, rx))?;
        Ok(Voice { inner: Arc::new(Inner { shared, listener: Mutex::new(Some(listener)), worker: tx }) })
    }

    /// Live mode: the outcome of a delegation, for the voice to tell the user
    /// (keep it to a sentence or two). Ends that delegation.
    pub fn delegation_result(&self, id: &str, text: &str) {
        let _ = self.inner.worker.send(worker::Msg::LiveOut(live::Out::Json(live::commentary(id, text))));
    }

    /// Live mode: progress on a running delegation. The voice knows it but
    /// doesn't read it out (it may mention it if asked).
    pub fn delegation_progress(&self, id: &str, text: &str) {
        let _ = self.inner.worker.send(worker::Msg::LiveOut(live::Out::Json(live::thinking(id, text))));
    }

    /// Replace the free-text prompt used for the next transcriptions.
    pub fn set_prompt(&self, prompt: Option<String>) {
        *self.inner.shared.prompt.lock().unwrap_or_else(|p| p.into_inner()) = prompt;
    }

    /// Replace the exact-spelling vocabulary used for the next transcriptions.
    pub fn set_keywords(&self, keywords: Vec<String>) {
        *self.inner.shared.keywords.lock().unwrap_or_else(|p| p.into_inner()) = keywords;
    }

    /// Unhook and stop the threads. No events are delivered afterwards (an
    /// in-flight transcription finishes silently). Idempotent.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Blocking: send 1 s of silence to the configured endpoint to verify the
    /// key, URL and model are accepted. Returns whatever text came back
    /// (usually empty). Run it off the UI thread.
    pub fn check_api(cfg: &VoiceConfig) -> Result<String, String> {
        let key = transcribe::resolve_key(&cfg.api_key_env)
            .ok_or_else(|| format!("Set {} to your OpenAI API key to use voice input", cfg.api_key_env))?;
        let wav = audio::wav(&vec![0i16; audio::TARGET_RATE as usize], audio::TARGET_RATE);
        let req = transcribe::Request { model: cfg.model.clone(), language: cfg.language.clone(), ..Default::default() };
        transcribe::transcribe(&cfg.base_url, &key, &cfg.api_key_env, &req, &wav, false)
    }

    /// Whether an API key is set in `api_key_env` (process env, then the
    /// registry on Windows). Cheap enough to call before [`Voice::start`].
    pub fn api_key_available(api_key_env: &str) -> bool {
        transcribe::resolve_key(api_key_env).is_some()
    }

    /// Hotkey name -> Windows VK code (`None` if unknown), for config validation.
    pub fn parse_hotkey(s: &str) -> Option<u32> {
        hotkey::parse_hotkey(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = VoiceConfig::default();
        assert!(Voice::parse_hotkey(&c.hotkey).is_some());
        #[cfg(windows)]
        assert_eq!(c.hotkey, "rwin");
        assert!(c.swallow);
        assert_eq!(c.api_key_env, "OPENAI_API_KEY");
        assert_eq!(c.base_url, "https://api.openai.com/v1");
        assert_eq!(c.model, DEFAULT_MODEL);
        assert_eq!((c.min_ms, c.max_secs), (300, 90));
    }

    #[test]
    fn bad_hotkey_fails_fast() {
        let cfg = VoiceConfig { hotkey: "nope".into(), ..Default::default() };
        let err = Voice::start(cfg, Box::new(|_| {})).err().expect("error");
        assert!(err.to_string().contains("unknown voice hotkey"));
    }
}
