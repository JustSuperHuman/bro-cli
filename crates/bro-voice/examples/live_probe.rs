//! Protocol probe for GPT-Live (`wss://api.openai.com/v1/live/sessions`):
//! synthesise a spoken command with TTS, stream it in, answer the client
//! delegation with a canned result, and log every server event.
//! `cargo run -p bro-voice --example live_probe ["words"]`

use base64::Engine;
use serde_json::{Value, json};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

fn key() -> String {
    std::env::var("OPENAI_API_KEY").ok().filter(|k| !k.is_empty()).unwrap_or_else(|| {
        let out = std::process::Command::new("reg")
            .args(["query", r"HKCU\Environment", "/v", "OPENAI_API_KEY"])
            .output()
            .expect("reg");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.trim().strip_prefix("OPENAI_API_KEY").map(|r| r.trim().trim_start_matches("REG_SZ").trim().to_string()))
            .expect("OPENAI_API_KEY")
    })
}

fn send(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, v: Value) {
    ws.send(Message::text(v.to_string())).unwrap();
}

fn set_timeout(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, t: Option<Duration>) {
    match ws.get_mut() {
        MaybeTlsStream::Plain(s) => s.set_read_timeout(t).unwrap(),
        MaybeTlsStream::Rustls(s) => s.get_mut().set_read_timeout(t).unwrap(),
        _ => {}
    }
}

fn main() {
    // bro links two rustls crypto providers; pick one (as live.rs does).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let words = std::env::args().nth(1).unwrap_or_else(|| "Open a session in justgains.".into());
    let key = key();
    let pcm = ureq::post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", &format!("Bearer {key}"))
        .send_json(json!({ "model": "gpt-4o-mini-tts", "input": words, "voice": "alloy", "response_format": "pcm" }))
        .expect("tts")
        .body_mut()
        .read_to_vec()
        .expect("tts body");
    eprintln!("tts: {:.1}s of 24 kHz pcm16", pcm.len() as f32 / 48_000.0);

    let mut request = tungstenite::client::IntoClientRequest::into_client_request("wss://api.openai.com/v1/live/sessions").unwrap();
    request.headers_mut().insert("Authorization", format!("Bearer {key}").parse().unwrap());
    let started = Instant::now();
    let (mut ws, _) = tungstenite::connect(request).expect("connect");
    let ms = |started: Instant| started.elapsed().as_millis();
    eprintln!("[{:>5}ms] connected", ms(started));
    send(&mut ws, json!({ "type": "session.start", "session": {
        "model": "gpt-live-1",
        "instructions": "You are Hugh, the voice of bro, a terminal workspace. You cannot see or control terminal sessions yourself: delegate every request about sessions, projects, code or chats. Keep speech to a few words.",
        "audio": { "format": { "type": "audio/pcm", "rate": 24000 }, "output": { "voice": "cedar" } },
        "delegation": { "type": "client" }
    }}));
    // Wait for session.started before audio.
    loop {
        let Message::Text(text) = ws.read().unwrap() else { continue };
        eprintln!("[{:>5}ms] {}", ms(started), text.chars().take(600).collect::<String>());
        if text.contains("session.started") || text.contains("\"error\"") { break }
    }
    // Stream the speech in real time-ish chunks (100 ms), then 1.5 s of silence.
    for chunk in pcm.chunks(4800) {
        send(&mut ws, json!({ "type": "session.input_audio.append", "audio": base64::engine::general_purpose::STANDARD.encode(chunk) }));
    }
    let silence = vec![0u8; 4800];
    let mute = std::env::var("PROBE_MUTE").is_ok();
    if mute {
        send(&mut ws, json!({ "type": "session.input_audio.mute" }));
    } else {
        for _ in 0..15 {
            send(&mut ws, json!({ "type": "session.input_audio.append", "audio": base64::engine::general_purpose::STANDARD.encode(&silence) }));
        }
    }
    let spoke_at = ms(started);
    eprintln!("[{spoke_at:>5}ms] audio sent");
    set_timeout(&mut ws, Some(Duration::from_millis(100)));
    let mut audio = 0usize;
    let mut first_audio: Option<u128> = None;
    let mut heard = String::new();
    let mut said = String::new();
    let mut answered = false;
    let mut seen: Vec<String> = Vec::new();
    let mut last_event = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        // keep feeding silence so the full-duplex model has a live mic
        if !mute { send(&mut ws, json!({ "type": "session.input_audio.append", "audio": base64::engine::general_purpose::STANDARD.encode(&silence) })); }
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if answered && last_event.elapsed() > Duration::from_secs(12) { break }
                continue;
            }
            Err(e) => { eprintln!("read: {e}"); break }
        };
        let Message::Text(text) = msg else { continue };
        last_event = Instant::now();
        let event: Value = serde_json::from_str(&text).unwrap();
        let kind = event["type"].as_str().unwrap_or("?").to_string();
        let t = ms(started);
        if kind.ends_with("audio.delta") {
            audio += base64::engine::general_purpose::STANDARD.decode(event["delta"].as_str().unwrap_or("")).map(|b| b.len()).unwrap_or(0);
            if first_audio.is_none() { first_audio = Some(t); eprintln!("[{t:>5}ms] {kind} (first; {} ms after audio sent)", t - spoke_at); }
            continue;
        }
        if kind.contains("delta") && !seen.contains(&kind) { seen.push(kind.clone()); eprintln!("[{t:>5}ms] first {kind}"); }
        if kind.contains("transcript.delta") || kind == "response.text.delta" {
            let d = event["delta"].as_str().unwrap_or("");
            if kind.contains("input") { heard.push_str(d) } else { said.push_str(d) }
            continue;
        }
        eprintln!("[{t:>5}ms] {}", text.chars().take(700).collect::<String>());
        if kind == "session.delegation.created" && !answered {
            answered = true;
            let id = event["delegation"]["id"].clone();
            eprintln!("   heard so far: {heard:?}");
            std::thread::sleep(Duration::from_millis(1500)); // Hugh working
            send(&mut ws, json!({ "type": "session.commentary.append", "delegation_id": id,
                "content": "Opened a Claude session in J:\\justgains named Mal." }));
        }
    }
    eprintln!("heard: {heard:?}\nsaid: {said:?}\naudio out: {:.1}s", audio as f32 / 48_000.0);
    send(&mut ws, json!({ "type": "session.close" }));
}
