//! A throwaway bridge for trying live voice end to end: a few fake agent sessions, the `/voice`
//! page, and every voice event and terminal command printed as it happens.
//!
//! `cargo run -p bro-bridge --example voice_live -- [seconds]` then open the printed URL (or drive
//! it from a headless browser with a fake microphone). Uses a scratch data folder; needs
//! `OPENAI_API_KEY` for real calls.

use bro_bridge::{Bridge, BridgeCommand, BridgeConfig, SessionMeta};
use futures_util::StreamExt;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(120);
    let root = std::env::temp_dir().join(format!("bro-voice-live-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let started = Instant::now();
    let stamp = move || format!("{:>6.2}s", started.elapsed().as_secs_f32());
    let handle: Arc<OnceLock<Bridge>> = Arc::new(OnceLock::new());
    let sessions = handle.clone();
    let bridge = Bridge::start(
        BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: root.join("bridge"), web_interface: false },
        Box::new(move |command| {
            let text = match command {
                BridgeCommand::Input { id, data, .. } => format!("INPUT {id}: {:?}", String::from_utf8_lossy(&data)),
                BridgeCommand::Write { id, data, reply } => {
                    // bro's host confirms each composed write once the session has the bytes.
                    let _ = reply.send(Ok(()));
                    let text = String::from_utf8_lossy(&data).into_owned();
                    // A message ending in Enter: the fake agent works on it for a while, then finishes.
                    if text.contains('\r') && let Some(bridge) = sessions.get().cloned() {
                        let id = id.clone();
                        std::thread::spawn(move || {
                            bridge.output(&id, "\x1b[2J\x1b[H❯ add a dark mode toggle\r\n\r\n· Thinking… (esc to interrupt)\r\n\r\n? for shortcuts".as_bytes());
                            std::thread::sleep(Duration::from_secs(7));
                            bridge.output(&id, "\x1b[2J\x1b[HAdded a dark mode toggle under Settings, Appearance. It follows the system theme by default; all 58 tests pass.\r\n\r\n❯ \r\n\r\n? for shortcuts".as_bytes());
                        });
                    }
                    format!("WRITE {id}: {text:?}")
                }
                other => format!("{other:?}").chars().take(240).collect(),
            };
            println!("[{}] command  {text}", stamp());
        }),
    )?;
    let _ = handle.set(bridge.clone());
    for (id, title, project, screen) in [
        ("toast", "Toast · justgains", "justgains", "Running the test suite…\r\n  ✓ auth flows (12)\r\n  ✓ workouts (29)\r\n41 passed, 0 failed\r\n> "),
        ("pinky", "Pinky · bro-cli", "bro-cli", "Refactored the settings page into three sections.\r\nAll checks pass.\r\n> "),
    ] {
        let dir = root.join(project);
        std::fs::create_dir_all(&dir)?;
        bridge.register(SessionMeta {
            id: id.into(), title: title.into(), shell: "claude".into(), args: vec![], cwd: Some(dir.clone()),
            project: Some(dir), pid: Some(std::process::id()), cols: 100, rows: 30, agent: Some("claude".into()), kind: None,
        });
        bridge.output(id, screen.as_bytes());
    }
    let url = bridge.voice_page_url().expect("the bridge listens");
    println!("[{}] page     {url}", stamp());
    println!("[{}] status   {}", stamp(), bridge.voice_status());
    let ws_url = url.replace("http://", "ws://").replace("/voice", "/ws");
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await.expect("bridge socket");
        let (_, mut read) = socket.split();
        let deadline = tokio::time::sleep(Duration::from_secs(seconds));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                message = read.next() => match message {
                    Some(Ok(message)) => {
                        let Ok(text) = message.to_text() else { continue };
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { continue };
                        if value["type"] == "voice" {
                            let mut value = value.clone();
                            value.as_object_mut().map(|o| { o.remove("type"); o.remove("at"); o.remove("callId"); });
                            println!("[{}] voice    {}", stamp(), value);
                        }
                    }
                    _ => break,
                }
            }
        }
    });
    println!("[{}] status   {}", stamp(), bridge.voice_status());
    bridge.shutdown();
    Ok(())
}
