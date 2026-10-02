//! Records every GPT-Live voice saying the same line and writes a
//! self-contained preview page (audio embedded as WAV data URIs):
//! `cargo run -p bro-voice --example voice_samples -- <out.html> [model]`

use base64::Engine;
use serde_json::{Value, json};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const VOICES: [&str; 22] = [
    "alloy", "ash", "ballad", "beacon", "bossa", "cedar", "cinder", "coral", "delta", "echo", "gleam", "marin", "meridian",
    "quartz", "ripple", "sage", "shimmer", "stone", "tempo", "verse", "vesper", "willow",
];
const LINE: &str = "Hey, it's Hugh. I opened a Claude session in justgains, and it's called Toast. Want me to send it the failing tests?";
const RATE: usize = 24_000;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn key() -> String {
    std::env::var("OPENAI_API_KEY").ok().filter(|k| !k.is_empty()).unwrap_or_else(|| {
        let out = std::process::Command::new("reg").args(["query", r"HKCU\Environment", "/v", "OPENAI_API_KEY"]).output().expect("reg");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.trim().strip_prefix("OPENAI_API_KEY").map(|r| r.trim().trim_start_matches("REG_SZ").trim().to_string()))
            .expect("OPENAI_API_KEY")
    })
}

fn send(ws: &mut Socket, v: Value) -> Result<(), String> {
    ws.send(Message::text(v.to_string())).map_err(|e| e.to_string())
}

struct Sample {
    voice: String,
    pcm: Vec<i16>,
    said: String,
    error: Option<String>,
}

fn record(voice: &str, model: &str, key: &str) -> Sample {
    let mut sample = Sample { voice: voice.into(), pcm: Vec::new(), said: String::new(), error: None };
    if let Err(e) = record_into(&mut sample, model, key) {
        sample.error = Some(e);
    }
    sample
}

fn record_into(sample: &mut Sample, model: &str, key: &str) -> Result<(), String> {
    let mut request = tungstenite::client::IntoClientRequest::into_client_request("wss://api.openai.com/v1/live/sessions").unwrap();
    request.headers_mut().insert("Authorization", format!("Bearer {key}").parse().unwrap());
    let (mut ws, _) = tungstenite::connect(request).map_err(|e| e.to_string())?;
    if let MaybeTlsStream::Rustls(stream) = ws.get_mut() {
        let _ = stream.get_mut().set_read_timeout(Some(Duration::from_millis(10)));
    }
    send(&mut ws, json!({ "type": "session.start", "session": {
        "model": model,
        "instructions": "You are recording a voice sample. When asked to say a line, say exactly that line, warmly and naturally, then stop.",
        "audio": { "format": { "type": "audio/pcm", "rate": RATE }, "output": { "voice": sample.voice } }
    }}))?;
    let engine = base64::engine::general_purpose::STANDARD;
    let silence = engine.encode(vec![0u8; RATE / 25 * 2]);
    let started = Instant::now();
    let mut asked = false;
    let mut last_said: Option<Instant> = None;
    let mut next_frame = Instant::now();
    while started.elapsed() < Duration::from_secs(40) {
        // A voice that ignored the instruction won't come round: retry instead.
        if last_said.is_none() && started.elapsed() > Duration::from_secs(12) {
            break;
        }
        if asked && Instant::now() >= next_frame {
            // GPT-Live paces itself by the input stream: keep a (silent) mic running.
            send(&mut ws, json!({ "type": "session.input_audio.append", "audio": silence }))?;
            next_frame += Duration::from_millis(40);
        }
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if last_said.is_some_and(|at| at.elapsed() > Duration::from_millis(2500)) {
                    break;
                }
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let Message::Text(text) = msg else { continue };
        let event: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if std::env::var("SAMPLES_DEBUG").is_ok() && !event["type"].as_str().unwrap_or("").contains("delta") {
            eprintln!("[{} {:>5}ms] {}", sample.voice, started.elapsed().as_millis(), text.chars().take(300).collect::<String>());
        }
        match event["type"].as_str().unwrap_or("") {
            "session.started" => {
                send(&mut ws, json!({ "type": "session.instructions.append", "delegation_id": null,
                    "content": format!("Say this line now, exactly: \"{LINE}\"") }))?;
                asked = true;
                next_frame = Instant::now();
            }
            "session.output_audio.delta" | "response.audio.delta" => {
                let bytes = engine.decode(event["delta"].as_str().unwrap_or("")).unwrap_or_default();
                sample.pcm.extend(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
            }
            "session.output_transcript.delta" | "response.text.delta" => {
                sample.said.push_str(event["delta"].as_str().unwrap_or(""));
                last_said = Some(Instant::now());
            }
            "error" => return Err(event.pointer("/error/message").and_then(Value::as_str).unwrap_or("error").to_string()),
            _ => {}
        }
    }
    let _ = send(&mut ws, json!({ "type": "session.close" }));
    trim(&mut sample.pcm);
    if sample.pcm.is_empty() {
        return Err("no speech came back".into());
    }
    Ok(())
}

/// Cut leading and trailing silence, keeping a short pad.
fn trim(pcm: &mut Vec<i16>) {
    let loud = |s: &i16| s.unsigned_abs() > 500;
    let pad = RATE / 8;
    let (Some(first), Some(last)) = (pcm.iter().position(loud), pcm.iter().rposition(loud)) else {
        pcm.clear();
        return;
    };
    let end = (last + pad).min(pcm.len());
    pcm.truncate(end);
    pcm.drain(..first.saturating_sub(pad));
}

fn wav(pcm: &[i16]) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn main() {
    // bro links two rustls crypto providers; pick one (as live.rs does).
    let _ = rustls::crypto::ring::default_provider().install_default();
    if let Ok(voice) = std::env::var("SAMPLES_ONE") {
        let key = key();
        for run in 1..=4 {
            let s = record(&voice, "gpt-live-1", &key);
            eprintln!("run {run}: {:.1}s {:?} {:?}", s.pcm.len() as f32 / RATE as f32, s.error, s.said);
        }
        return;
    }
    let out = std::env::args().nth(1).unwrap_or_else(|| "gpt-live-voices.html".into());
    let model = std::env::args().nth(2).unwrap_or_else(|| "gpt-live-1".into());
    let key = key();
    let started = Instant::now();
    // A few sessions at a time: many at once come back silent.
    let mut samples: Vec<Sample> = VOICES.iter().map(|v| Sample { voice: v.to_string(), pcm: Vec::new(), said: String::new(), error: Some("not recorded".into()) }).collect();
    for attempt in 1..=8 {
        let todo: Vec<usize> = (0..samples.len()).filter(|&i| samples[i].error.is_some()).collect();
        if todo.is_empty() {
            break;
        }
        eprintln!("attempt {attempt}: {} voices", todo.len());
        for batch in todo.chunks(4) {
            let recorded: Vec<(usize, Sample)> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|&i| {
                        let (key, model, voice) = (&key, &model, samples[i].voice.clone());
                        scope.spawn(move || (i, record(&voice, model, key)))
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().expect("thread")).collect()
            });
            for (i, sample) in recorded {
                samples[i] = sample;
            }
        }
    }
    for s in &samples {
        eprintln!(
            "{:>9}: {:>5.1}s {}",
            s.voice,
            s.pcm.len() as f32 / RATE as f32,
            s.error.as_deref().map(|e| format!("ERROR {e}")).unwrap_or_else(|| format!("{:?}", s.said.trim()))
        );
    }
    eprintln!("recorded in {:.1}s", started.elapsed().as_secs_f32());

    let engine = base64::engine::general_purpose::STANDARD;
    let mut cards = String::new();
    for s in &samples {
        let body = match &s.error {
            Some(error) => format!(r#"<p class="error">Couldn't record: {}</p>"#, escape(error)),
            None => format!(
                r#"<audio preload="auto" src="data:audio/wav;base64,{}"></audio>
      <button class="play" aria-label="Play {voice}"><span class="icon">▶</span><span class="bar"><span class="fill"></span></span><span class="time">{secs:.1}s</span></button>
      <p class="said">“{said}”</p>"#,
                engine.encode(wav(&s.pcm)),
                voice = s.voice,
                secs = s.pcm.len() as f32 / RATE as f32,
                said = escape(s.said.trim()),
            ),
        };
        let current = if s.voice == "cedar" { r#"<span class="tag">bro default</span>"# } else { "" };
        cards.push_str(&format!(
            r#"
    <article class="card" data-voice="{voice}">
      <header><h2>{voice}</h2>{current}</header>
      {body}
      <button class="use" data-snippet='live_voice = "{voice}"'>Copy <code>live_voice = "{voice}"</code></button>
    </article>"#,
            voice = s.voice,
        ));
    }
    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>GPT-Live 1 voices · bro</title>
<style>
  :root {{ color-scheme: light dark; --bg:#f6f5f2; --card:#fff; --ink:#1d1d1f; --muted:#6b6b70; --line:#e4e2dd; --accent:#3b6fe0; --accent-soft:#3b6fe01a; }}
  @media (prefers-color-scheme: dark) {{ :root {{ --bg:#141416; --card:#1d1d20; --ink:#ececf0; --muted:#9a9aa3; --line:#2c2c31; --accent:#7ea2ff; --accent-soft:#7ea2ff22; }} }}
  * {{ box-sizing:border-box; }}
  body {{ margin:0; background:var(--bg); color:var(--ink); font:15px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }}
  main {{ max-width:1100px; margin:0 auto; padding:40px 24px 64px; }}
  h1 {{ font-size:26px; margin:0 0 6px; letter-spacing:-0.01em; }}
  .lede {{ color:var(--muted); margin:0 0 20px; max-width:70ch; }}
  .line {{ border-left:3px solid var(--accent); padding:6px 14px; margin:0 0 24px; background:var(--accent-soft); border-radius:0 8px 8px 0; }}
  .toolbar {{ display:flex; gap:10px; align-items:center; margin-bottom:22px; flex-wrap:wrap; }}
  .toolbar button {{ font:inherit; padding:8px 14px; border-radius:8px; border:1px solid var(--line); background:var(--card); color:var(--ink); cursor:pointer; }}
  .toolbar button.primary {{ background:var(--accent); border-color:var(--accent); color:#fff; }}
  .grid {{ display:grid; grid-template-columns:repeat(auto-fill, minmax(240px, 1fr)); gap:14px; }}
  .card {{ background:var(--card); border:1px solid var(--line); border-radius:12px; padding:14px 14px 12px; display:flex; flex-direction:column; gap:10px; transition:border-color .15s, box-shadow .15s; }}
  .card.playing {{ border-color:var(--accent); box-shadow:0 0 0 3px var(--accent-soft); }}
  .card header {{ display:flex; align-items:center; justify-content:space-between; }}
  .card h2 {{ font-size:17px; margin:0; text-transform:capitalize; }}
  .tag {{ font-size:11px; padding:2px 8px; border-radius:99px; background:var(--accent-soft); color:var(--accent); font-weight:600; }}
  .play {{ display:flex; align-items:center; gap:10px; width:100%; font:inherit; padding:8px 10px; border-radius:8px; border:1px solid var(--line); background:transparent; color:var(--ink); cursor:pointer; }}
  .play:hover {{ border-color:var(--accent); }}
  .icon {{ width:18px; text-align:center; color:var(--accent); }}
  .bar {{ flex:1; height:4px; border-radius:2px; background:var(--line); overflow:hidden; }}
  .fill {{ display:block; height:100%; width:0; background:var(--accent); }}
  .time {{ color:var(--muted); font-size:12px; font-variant-numeric:tabular-nums; }}
  .said {{ margin:0; color:var(--muted); font-size:13px; }}
  .use {{ align-self:flex-start; font:inherit; font-size:12px; padding:4px 8px; border-radius:6px; border:1px dashed var(--line); background:transparent; color:var(--muted); cursor:pointer; }}
  .use:hover {{ color:var(--ink); border-color:var(--muted); }}
  .use code {{ font-size:12px; }}
  .error {{ color:#c0392b; margin:0; font-size:13px; }}
  footer {{ margin-top:28px; color:var(--muted); font-size:13px; }}
  code {{ font-family:ui-monospace, "Cascadia Code", Consolas, monospace; }}
</style>
</head>
<body>
<main>
  <h1>GPT-Live 1 voices</h1>
  <p class="lede">Every voice <code>{model}</code> offers, recorded from a live session saying the same line. Pick one and put it under <code>[voice]</code> in <code>~/.bro/v2.toml</code>, then restart bro.</p>
  <p class="line">“{line}”</p>
  <div class="toolbar">
    <button class="primary" id="all">▶ Play all in order</button>
    <button id="stop">■ Stop</button>
  </div>
  <section class="grid">{cards}
  </section>
  <footer>Recorded {count} voices on {date}. Each sample is the voice's own delivery of the line, so wording can vary slightly.</footer>
</main>
<script>
  const cards = [...document.querySelectorAll('.card')].filter(c => c.querySelector('audio'));
  let queue = [];
  function stopAll() {{
    for (const c of cards) {{ const a = c.querySelector('audio'); a.pause(); a.currentTime = 0; c.classList.remove('playing'); c.querySelector('.icon').textContent = '▶'; c.querySelector('.fill').style.width = '0'; }}
  }}
  function play(card) {{
    stopAll();
    const a = card.querySelector('audio');
    card.classList.add('playing'); card.querySelector('.icon').textContent = '❚❚';
    a.play();
  }}
  for (const card of cards) {{
    const a = card.querySelector('audio');
    card.querySelector('.play').addEventListener('click', () => {{ queue = []; a.paused ? play(card) : stopAll(); }});
    a.addEventListener('timeupdate', () => {{ card.querySelector('.fill').style.width = (100 * a.currentTime / (a.duration || 1)) + '%'; }});
    a.addEventListener('ended', () => {{ card.classList.remove('playing'); card.querySelector('.icon').textContent = '▶'; card.querySelector('.fill').style.width = '0';
      const next = queue.shift(); if (next) setTimeout(() => play(next), 350); }});
  }}
  document.getElementById('all').addEventListener('click', () => {{ queue = cards.slice(1); play(cards[0]); }});
  document.getElementById('stop').addEventListener('click', () => {{ queue = []; stopAll(); }});
  for (const b of document.querySelectorAll('.use')) b.addEventListener('click', async () => {{
    await navigator.clipboard.writeText(b.dataset.snippet);
    const was = b.innerHTML; b.textContent = 'Copied'; setTimeout(() => b.innerHTML = was, 1200);
  }});
</script>
</body>
</html>
"#,
        model = escape(&model),
        line = escape(LINE),
        count = samples.iter().filter(|s| s.error.is_none()).count(),
        date = chrono_free_date(),
    );
    std::fs::write(&out, html).expect("write page");
    eprintln!("wrote {out}");
}

/// Today's date as YYYY-MM-DD without a date crate.
fn chrono_free_date() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}
