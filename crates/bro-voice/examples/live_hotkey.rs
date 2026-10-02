//! Live mode through the real hotkey path, with the diagnostic log on:
//! `cargo run -p bro-voice --example live_hotkey [seconds] [log]`. Hold the
//! hotkey and talk; delegations are answered with a canned line.

use bro_voice::{LiveConfig, Voice, VoiceConfig, VoiceEvent};
use std::sync::{Arc, Mutex};

fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    let log = std::env::args().nth(2).map(std::path::PathBuf::from);
    let slot: Arc<Mutex<Option<Voice>>> = Arc::new(Mutex::new(None));
    let for_events = slot.clone();
    let cfg = VoiceConfig { live: Some(LiveConfig { idle_secs: 20, ..LiveConfig::default() }), log, ..VoiceConfig::default() };
    let voice = Voice::start(
        cfg,
        Box::new(move |event| {
            if !matches!(event, VoiceEvent::Level(_)) {
                eprintln!("{event:?}");
            }
            if let VoiceEvent::Delegation { id, .. } = event
                && let Some(voice) = for_events.lock().unwrap().as_ref()
            {
                voice.delegation_result(&id, "This is a test answer from the probe.");
            }
        }),
    )
    .expect("start");
    *slot.lock().unwrap() = Some(voice);
    eprintln!("hold {} to talk; running {secs}s", bro_voice::default_hotkey());
    std::thread::sleep(std::time::Duration::from_secs(secs));
}
