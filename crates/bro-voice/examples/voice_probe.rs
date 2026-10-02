//! Manual probe for bro-voice.
//!
//! cargo run -p bro-voice --example voice_probe              # hold the hotkey, watch events (60 s)
//! cargo run -p bro-voice --example voice_probe -- --secs 120 --hotkey f13
//! cargo run -p bro-voice --example voice_probe -- --check   # 1 s of silence -> API (auth + model)
//! cargo run -p bro-voice --example voice_probe -- --latency # mic open -> first audio, 5 runs
//! cargo run -p bro-voice --example voice_probe -- --selftest # Windows: synthetic F24 hold / tap / chord

use bro_voice::{Voice, VoiceConfig, VoiceEvent};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let mut cfg = VoiceConfig {
        keywords: ["Mal", "Pinky", "Toast", "Minty", "Lilac", "Sky", "Bitty", "Hugh", "bro"].map(String::from).to_vec(),
        ..Default::default()
    };
    if let Some(h) = value("--hotkey") {
        cfg.hotkey = h;
    }
    if let Some(m) = value("--model") {
        cfg.model = m;
    }

    if args.iter().any(|a| a == "--check") {
        let t = Instant::now();
        match Voice::check_api(&cfg) {
            Ok(text) => println!("ok: model {} accepted, {:?} in {:?}", cfg.model, text, t.elapsed()),
            Err(e) => println!("error: {e} ({:?})", t.elapsed()),
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--latency") {
        return latency();
    }
    #[cfg(windows)]
    if args.iter().any(|a| a == "--selftest") {
        return selftest(cfg);
    }

    let secs: u64 = value("--secs").and_then(|s| s.parse().ok()).unwrap_or(60);
    let t0 = Instant::now();
    let voice = Voice::start(
        cfg.clone(),
        Box::new(move |ev| {
            let at = t0.elapsed().as_secs_f32();
            match ev {
                VoiceEvent::Level(l) => {
                    let bar = "#".repeat((l * 40.0) as usize);
                    println!("{at:7.2}s level {l:.2} {bar}");
                }
                other => println!("{at:7.2}s {other:?}"),
            }
        }),
    )?;
    println!("hold {} to talk; exiting in {secs}s", cfg.hotkey);
    std::thread::sleep(Duration::from_secs(secs));
    voice.stop();
    println!("stopped");
    Ok(())
}

/// Time from "open the default input device" to the first audio callback.
fn latency() -> anyhow::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    for run in 1..=5 {
        let t = Instant::now();
        let device = host.default_input_device().ok_or_else(|| anyhow::anyhow!("no input device"))?;
        let config = device.default_input_config()?;
        let (tx, rx) = std::sync::mpsc::channel::<Instant>();
        let stream = device.build_input_stream_raw(
            config.config(),
            config.sample_format(),
            move |_data: &cpal::Data, _: &cpal::InputCallbackInfo| {
                let _ = tx.send(Instant::now());
            },
            |e| eprintln!("stream error: {e}"),
            None,
        )?;
        let built = t.elapsed();
        stream.play()?;
        let played = t.elapsed();
        let first = rx.recv_timeout(Duration::from_secs(2)).map(|i| i - t);
        drop(stream);
        println!(
            "run {run}: {} Hz x{} {:?}: built {built:?}, playing {played:?}, first audio {first:?}",
            config.sample_rate(),
            config.channels(),
            config.sample_format()
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(())
}

/// Drive the real hook with injected F24 (hotkey) / F23 (chord key) presses:
/// a 1 s hold, a 100 ms tap, and a chord. Harmless keys, so nothing on the
/// desktop reacts.
#[cfg(windows)]
fn selftest(cfg: VoiceConfig) -> anyhow::Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput};
    fn key(vk: u16, up: bool) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 } },
        };
        // SAFETY: one valid INPUT.
        unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    }
    const F23: u16 = 0x86;
    const F24: u16 = 0x87;
    let t0 = Instant::now();
    let cfg = VoiceConfig { hotkey: "f24".into(), ..cfg };
    let voice = Voice::start(cfg, Box::new(move |ev| {
        if !matches!(ev, VoiceEvent::Level(_)) {
            println!("{:7.3}s {ev:?}", t0.elapsed().as_secs_f32());
        }
    }))?;
    let pause = |ms| std::thread::sleep(Duration::from_millis(ms));
    println!("-- hold 1s (expect Listening, Transcribing, then a transcript/error)");
    key(F24, false);
    pause(1000);
    key(F24, true);
    pause(1500);
    println!("-- tap 100ms (expect Listening, Cancelled)");
    key(F24, false);
    pause(100);
    key(F24, true);
    pause(700);
    println!("-- chord F24+F23 (expect Listening, Cancelled)");
    key(F24, false);
    pause(150);
    key(F23, false);
    key(F23, true);
    pause(100);
    key(F24, true);
    pause(700);
    voice.stop();
    println!("stopped");
    Ok(())
}
