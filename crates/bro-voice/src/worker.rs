//! The worker thread: turns hook signals into takes. It owns the cpal stream
//! (opened per press, dropped on release), resamples and meters the chunks the
//! audio callback forwards, and hands finished takes to a short-lived
//! transcription thread so the next press is never blocked by the network.

use crate::audio::{self, Capture, Resampler, TARGET_RATE};
use crate::live::{self, LIVE_RATE, Out};
use crate::playback::Player;
use crate::hotkey::machine::Signal;
use crate::transcribe::{self, Request};
use crate::{DEFAULT_MODEL, Shared, VoiceEvent};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

pub enum Msg {
    Key(Signal),
    Audio { take: u64, chunk: Result<Vec<f32>, String> },
    /// Live mode: something for the session (a delegation result).
    LiveOut(live::Out),
    /// Live mode: the session with this generation ended.
    LiveClosed(u64),
    Stop,
}

const LEVEL_EVERY: Duration = Duration::from_millis(66);
/// Takes whose loudest level window stays under this RMS (~-50 dBFS) are silence.
const SILENCE_RMS: f32 = 0.003;

struct Take {
    id: u64,
    capture: Capture,
    resampler: Resampler,
    pcm: Vec<i16>,
    /// Start of the current level window in `pcm`.
    window: usize,
    last_level: Instant,
    loudest: f32,
}

impl Take {
    fn push(&mut self, chunk: &[f32]) -> Option<f32> {
        self.resampler.push(chunk, &mut self.pcm);
        if self.last_level.elapsed() < LEVEL_EVERY || self.window >= self.pcm.len() {
            return None;
        }
        let rms = audio::rms(&self.pcm[self.window..]);
        self.window = self.pcm.len();
        self.last_level = Instant::now();
        self.loudest = self.loudest.max(rms);
        Some(audio::meter(rms))
    }
}

pub fn run(shared: Arc<Shared>, tx: Sender<Msg>, rx: Receiver<Msg>) {
    if let Some(cfg) = shared.cfg.live.clone() {
        return run_live(shared, cfg, tx, rx);
    }
    let host = cpal::default_host();
    // Warm the device enumerator so the first press opens as fast as later ones
    // (~90 ms cold vs ~35 ms warm to first audio on WASAPI).
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        let _ = host.default_input_device().map(|d| d.default_input_config());
    }
    let cap = shared.cfg.max_secs.max(1) as usize * TARGET_RATE as usize;
    let mut take: Option<Take> = None;
    let mut next_id = 0u64;
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Stop => break,
            Msg::Key(Signal::Down) => {
                if take.is_some() {
                    continue;
                }
                next_id += 1;
                let id = next_id;
                let tx = tx.clone();
                let sink: audio::Sink = Arc::new(move |chunk| {
                    let _ = tx.send(Msg::Audio { take: id, chunk });
                });
                match audio::open(&host, sink) {
                    Ok(capture) => {
                        let resampler = Resampler::new(capture.rate);
                        take = Some(Take {
                            id,
                            capture,
                            resampler,
                            pcm: Vec::with_capacity(TARGET_RATE as usize * 10),
                            window: 0,
                            last_level: Instant::now(),
                            loudest: 0.0,
                        });
                        shared.emit(VoiceEvent::Listening);
                    }
                    Err(e) => shared.emit(VoiceEvent::Error(format!("{e:#}"))),
                }
            }
            Msg::Key(Signal::Up { held_ms }) => {
                if let Some(t) = take.take() {
                    if held_ms < shared.cfg.min_ms {
                        drop(t);
                        shared.emit(VoiceEvent::Cancelled);
                    } else {
                        finish(&shared, t);
                    }
                }
            }
            Msg::Key(Signal::Combo) => {
                if take.take().is_some() {
                    shared.emit(VoiceEvent::Cancelled);
                }
            }
            Msg::LiveOut(_) | Msg::LiveClosed(_) => {}
            Msg::Audio { take: id, chunk } => {
                if take.as_ref().is_none_or(|t| t.id != id) {
                    continue; // a straggler from a finished take
                }
                match chunk {
                    Err(e) => {
                        take = None;
                        shared.emit(VoiceEvent::Error(format!("Microphone error: {e}")));
                    }
                    Ok(samples) => {
                        let t = take.as_mut().expect("checked above");
                        if let Some(level) = t.push(&samples) {
                            shared.emit(VoiceEvent::Level(level));
                        }
                        if t.pcm.len() >= cap {
                            // Hit max_secs: transcribe now; the later key-up finds no take.
                            let t = take.take().expect("checked above");
                            finish(&shared, t);
                        }
                    }
                }
            }
        }
    }
}

fn finish(shared: &Arc<Shared>, t: Take) {
    let Take { capture, mut pcm, window, loudest, .. } = t;
    drop(capture);
    let loudest = loudest.max(audio::rms(&pcm[window.min(pcm.len())..]));
    pcm.truncate(shared.cfg.max_secs.max(1) as usize * TARGET_RATE as usize);
    if pcm.len() < shared.cfg.min_ms as usize * TARGET_RATE as usize / 1000 {
        return shared.emit(VoiceEvent::Cancelled);
    }
    if pcm.iter().all(|&s| s == 0) {
        return shared.emit(VoiceEvent::Error("The microphone recorded pure silence: is it muted?".into()));
    }
    if loudest < SILENCE_RMS {
        return shared.emit(VoiceEvent::Cancelled);
    }
    shared.emit(VoiceEvent::Transcribing);
    let owned = shared.clone();
    let spawned = std::thread::Builder::new().name("bro-voice-transcribe".into()).spawn(move || {
        let shared = owned;
        let cfg = &shared.cfg;
        let Some(key) = transcribe::resolve_key(&cfg.api_key_env) else {
            return shared.emit(VoiceEvent::Error(format!(
                "Set {} to your OpenAI API key to use voice input",
                cfg.api_key_env
            )));
        };
        let req = Request {
            model: cfg.model.clone(),
            language: cfg.language.clone(),
            prompt: shared.prompt.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            keywords: shared.keywords.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        };
        let wav = audio::wav(&pcm, TARGET_RATE);
        let fallback = cfg.model == DEFAULT_MODEL;
        match transcribe::transcribe(&cfg.base_url, &key, &cfg.api_key_env, &req, &wav, fallback) {
            Ok(text) if text.is_empty() => shared.emit(VoiceEvent::Cancelled),
            Ok(text) => shared.emit(VoiceEvent::Transcript(text)),
            Err(e) => shared.emit(VoiceEvent::Error(e)),
        }
    });
    if let Err(e) = spawned {
        shared.emit(VoiceEvent::Error(format!("could not start transcription: {e}")));
    }
}

/// 40 ms of 24 kHz audio per `input_audio.append`.
const LIVE_BATCH: usize = LIVE_RATE as usize / 25;

/// An open GPT-Live session as the worker sees it: the continuous capture
/// feeding it and the channel to its socket thread.
struct Link {
    generation: u64,
    out: Sender<Out>,
    _capture: Capture,
    resampler: Resampler,
    batch: Vec<i16>,
    /// Where the samples produced while talking start in `batch` (earlier
    /// ones, produced while the key was up, are silenced).
    talk_from: usize,
    level_window: Vec<i16>,
    last_level: Instant,
}

/// Live mode: each press opens (or reuses) a GPT-Live session. The mic runs
/// for the whole session; only audio captured while the key is held reaches
/// the model, the rest is sent as silence to keep the session's clock going.
fn run_live(shared: Arc<Shared>, cfg: live::LiveConfig, tx: Sender<Msg>, rx: Receiver<Msg>) {
    let host = cpal::default_host();
    let mut player: Option<Arc<Player>> = None;
    let mut link: Option<Link> = None;
    let mut talking = false;
    let mut generation = 0u64;
    while let Ok(msg) = rx.recv() {
        if let Msg::Key(signal) = &msg {
            shared.log(format!("key {signal:?}"));
        }
        match msg {
            Msg::Stop => {
                if let Some(link) = link.take() {
                    let _ = link.out.send(Out::Close);
                }
                break;
            }
            Msg::Key(Signal::Down) => {
                // Talking over the voice cuts it off.
                if let Some(player) = &player {
                    player.clear();
                }
                if link.is_none() {
                    let Some(key) = transcribe::resolve_key(&shared.cfg.api_key_env) else {
                        shared.emit(VoiceEvent::Error(format!(
                            "Set {} to your OpenAI API key to use voice input",
                            shared.cfg.api_key_env
                        )));
                        continue;
                    };
                    if player.is_none() {
                        match Player::open(&host) {
                            Ok(opened) => {
                                shared.log("live: speakers open");
                                player = Some(Arc::new(opened));
                            }
                            Err(e) => shared.emit(VoiceEvent::Error(format!("{e:#} (replies will be text only)"))),
                        }
                    }
                    generation += 1;
                    let id = generation;
                    let audio_tx = tx.clone();
                    let sink: audio::Sink = Arc::new(move |chunk| {
                        let _ = audio_tx.send(Msg::Audio { take: id, chunk });
                    });
                    let capture = match audio::open(&host, sink) {
                        Ok(capture) => {
                            shared.log(format!("live: mic open at {} Hz", capture.rate));
                            capture
                        }
                        Err(e) => {
                            shared.emit(VoiceEvent::Error(format!("{e:#}")));
                            continue;
                        }
                    };
                    let (out, outgoing) = std::sync::mpsc::channel();
                    let (thread_shared, thread_cfg, thread_player, worker) =
                        (shared.clone(), cfg.clone(), player.clone(), tx.clone());
                    let spawned = std::thread::Builder::new().name("bro-voice-live".into()).spawn(move || {
                        live::run(thread_shared, thread_cfg, key, thread_player, outgoing, worker, id)
                    });
                    if let Err(e) = spawned {
                        shared.emit(VoiceEvent::Error(format!("could not start the live session: {e}")));
                        continue;
                    }
                    link = Some(Link {
                        generation: id,
                        out,
                        resampler: Resampler::with_rates(capture.rate, LIVE_RATE),
                        _capture: capture,
                        batch: Vec::with_capacity(LIVE_BATCH * 2),
                        talk_from: 0,
                        level_window: Vec::new(),
                        last_level: Instant::now(),
                    });
                }
                if let Some(link) = link.as_mut() {
                    talking = true;
                    link.talk_from = link.batch.len();
                    let _ = link.out.send(Out::Talking(true));
                    shared.emit(VoiceEvent::Listening);
                }
            }
            Msg::Key(Signal::Up { held_ms }) => {
                if talking {
                    talking = false;
                    if let Some(link) = &link {
                        let _ = link.out.send(Out::Talking(false));
                    }
                    if held_ms < shared.cfg.min_ms {
                        shared.emit(VoiceEvent::Cancelled);
                    }
                }
            }
            Msg::Key(Signal::Combo) => {
                if talking {
                    talking = false;
                    if let Some(link) = &link {
                        let _ = link.out.send(Out::Talking(false));
                    }
                    shared.emit(VoiceEvent::Cancelled);
                }
            }
            Msg::Audio { take, chunk } => {
                let Some(current) = link.as_mut().filter(|link| link.generation == take) else { continue };
                match chunk {
                    Err(e) => {
                        shared.emit(VoiceEvent::Error(format!("Microphone error: {e}")));
                        if let Some(link) = link.take() {
                            let _ = link.out.send(Out::Close);
                        }
                        talking = false;
                    }
                    Ok(samples) => {
                        let before = current.batch.len();
                        current.resampler.push(&samples, &mut current.batch);
                        if talking {
                            current.level_window.extend_from_slice(&current.batch[before.max(current.talk_from)..]);
                            if current.last_level.elapsed() >= LEVEL_EVERY && !current.level_window.is_empty() {
                                shared.emit(VoiceEvent::Level(audio::meter(audio::rms(&current.level_window))));
                                current.level_window.clear();
                                current.last_level = Instant::now();
                            }
                        } else {
                            // Key up: the model hears silence, not the room.
                            let from = before.min(current.batch.len());
                            current.batch[from..].fill(0);
                        }
                        if current.batch.len() >= LIVE_BATCH {
                            let bytes: Vec<u8> = current.batch.iter().flat_map(|s| s.to_le_bytes()).collect();
                            current.batch.clear();
                            current.talk_from = 0;
                            let _ = current.out.send(Out::Audio(bytes));
                        }
                    }
                }
            }
            Msg::LiveOut(out) => match &link {
                Some(link) => {
                    let _ = link.out.send(out);
                }
                None => shared.emit(VoiceEvent::Error("the voice session ended before the answer arrived".into())),
            },
            Msg::LiveClosed(closed) => {
                if link.as_ref().is_some_and(|link| link.generation == closed) {
                    link = None;
                    talking = false;
                }
            }
        }
    }
}
