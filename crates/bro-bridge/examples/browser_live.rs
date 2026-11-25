//! Isolated live browser gateway for end-to-end testing; never replaces an existing Bro host.
//! `cargo run --locked -p bro-bridge --example browser_live -- <connection-file> [seconds]`
use bro_bridge::{Bridge, BridgeCommand, BridgeConfig, SessionMeta, StartOptions};
use std::io::Write;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let info = std::path::PathBuf::from(args.next().expect("connection-file argument"));
    let seconds = args
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(600);
    let wake_test = args.next().as_deref() == Some("--wake-test");
    let fixture_session =
        wake_test.then(|| format!("browser-wake-fixture-{}", uuid::Uuid::new_v4()));
    let wake_log = info.with_extension("wakes.jsonl");
    if wake_test {
        std::fs::write(&wake_log, "")?;
    }
    let log_path = wake_log.clone();
    let allowed_session = fixture_session.clone();
    let root = tempfile::tempdir()?;
    let previous = std::fs::read(&info)
        .ok()
        .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok());
    let port = previous
        .as_ref()
        .and_then(|v| v["url"].as_str())
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.port())
        .unwrap_or(0);
    if let Some(token) = previous
        .as_ref()
        .and_then(|v| v["token"].as_str())
        .filter(|v| v.len() <= 256)
    {
        std::fs::write(root.path().join(".terminal-web-token"), token)?;
    }
    let bridge = Bridge::start_with(
        BridgeConfig {
            port,
            automatic_port: false,
            bind: "127.0.0.1".into(),
            data_root: root.path().to_owned(),
            web_interface: false,
        },
        StartOptions {
            import_legacy_token: false,
        },
        Box::new(move |command| {
            if let BridgeCommand::BrowserResume {
                id,
                event_id,
                text,
                reply,
            } = command
            {
                let result = if allowed_session.as_deref() != Some(&id) {
                    Err("This is not the fixture session.".into())
                } else {
                    std::fs::OpenOptions::new()
                        .append(true)
                        .open(&log_path)
                        .and_then(|mut file| {
                            writeln!(
                                file,
                                "{}",
                                serde_json::json!({"sessionId":id,"eventId":event_id,"text":text})
                            )
                        })
                        .map(|_| true)
                        .map_err(|error| error.to_string())
                };
                let _ = reply.send(result);
            }
        }),
    )?;
    if let Some(id) = &fixture_session {
        bridge.register(SessionMeta {
            id: id.clone(),
            title: "Browser wake test agent".into(),
            shell: "codex".into(),
            args: vec![],
            cwd: Some(root.path().to_owned()),
            project: None,
            pid: None,
            cols: 100,
            rows: 30,
            agent: Some("codex".into()),
            kind: None,
        });
        bridge.output(id, "\x1b[2J\x1b[HOpenAI Codex\r\n› ".as_bytes());
    }
    let url = bridge.local_url().expect("bridge listener");
    std::fs::write(
        &info,
        serde_json::to_vec(
            &serde_json::json!({"url":url,"token":bridge.status().token,"pid":std::process::id(),"fixtureSessionId":fixture_session,"wakeLog":wake_log,"activityFile":info.with_extension("activity")}),
        )?,
    )?;
    println!(
        "Browser test gateway ready on {url}; connection details saved to {}",
        info.display()
    );
    if let Some(id) = fixture_session {
        let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
        let mut last = String::new();
        while std::time::Instant::now() < deadline {
            let activity =
                std::fs::read_to_string(info.with_extension("activity")).unwrap_or_default();
            if activity != last {
                let frame = if activity.trim() == "working" {
                    "\x1b[2J\x1b[HOpenAI Codex\r\nWorking (esc to interrupt)"
                } else {
                    "\x1b[2J\x1b[HOpenAI Codex\r\n› "
                };
                bridge.output(&id, frame.as_bytes());
                last = activity;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    } else {
        std::thread::sleep(Duration::from_secs(seconds));
    }
    bridge.shutdown();
    Ok(())
}
