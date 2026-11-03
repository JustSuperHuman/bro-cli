//! Explicit real-Windows acceptance: downloads the published package and captures
//! the local desktop. Kept opt-in because it needs an unlocked interactive session.
#![cfg(windows)]
use bro_bridge::{Bridge, BridgeConfig, StartOptions};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

#[tokio::test]
#[ignore = "downloads the signed RMM package and captures this Windows desktop"]
async fn published_package_streams_through_pairing_and_revokes_every_viewer() {
    let root = tempfile::tempdir().unwrap();
    let data_root = root.path().join("bridge");
    let bridge = Bridge::start_with(
        BridgeConfig {
            port: 0,
            automatic_port: false,
            bind: "127.0.0.1".into(),
            data_root: data_root.clone(),
            web_interface: false,
        },
        StartOptions {
            import_legacy_token: false,
        },
        Box::new(|_| {}),
    )
    .unwrap();
    let base = format!("http://127.0.0.1:{}", bridge.status().port);
    let token = std::fs::read_to_string(data_root.join(".terminal-web-token")).unwrap();
    let token = token.trim();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client
            .get(format!("{base}/api/rmm"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(!root.path().join("rmm/packages").exists());
    bridge.set_rmm_settings(bro_core::rmm::Settings {
        enabled: true,
        ..Default::default()
    });
    let started = Instant::now();
    loop {
        let status = bridge.rmm_status();
        assert_ne!(
            status.phase,
            bro_bridge::rmm::Phase::Error,
            "{:?}",
            status.message
        );
        if status.phase == bro_bridge::rmm::Phase::Ready {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "package installation timed out"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(bro_core::rmm::installed(&root.path().join("rmm")).is_ok());
    let controller: Value = client
        .post(format!("{base}/api/rmm/sessions"))
        .bearer_auth(token)
        .json(&json!({"transport":"websocket"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(controller["readOnly"], false);
    assert_eq!(
        client
            .post(format!("{base}/api/rmm/sessions"))
            .bearer_auth(token)
            .json(&json!({"transport":"websocket"}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let mut request = format!(
        "{}{}",
        base.replacen("http:", "ws:", 1),
        controller["wsPath"].as_str().unwrap()
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket.send(Message::Text("v,1".into())).await.unwrap();
    let mut hello = false;
    let mut keyframe = false;
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(Ok(message)) = socket.next().await {
            match message {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["t"] == "hello" {
                        hello = true;
                        assert!(value["w"].as_u64().unwrap() > 0);
                        assert_eq!(value["readOnly"], false);
                    }
                    if value["t"] == "ping" {
                        socket
                            .send(Message::Text(format!("po,{}", value["s"]).into()))
                            .await
                            .unwrap();
                    }
                }
                Message::Binary(bytes) if bytes.len() > 16 && bytes[0] == 1 => {
                    keyframe |= bytes[1] & 1 == 1;
                    if hello && keyframe {
                        break;
                    }
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(
        hello && keyframe,
        "no native H.264 keyframe reached the paired viewer"
    );
    let screenshot = client
        .get(format!(
            "{base}{}/screenshot",
            controller["basePath"]
                .as_str()
                .unwrap()
                .trim_end_matches('/')
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&screenshot[..8], b"\x89PNG\r\n\x1a\n");
    bridge.set_rmm_settings(bro_core::rmm::Settings {
        enabled: true,
        read_only: true,
        display: 0,
    });
    assert_eq!(bridge.rmm_status().active_viewers, 0);
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await
    .unwrap();
    let watcher: Value = client
        .post(format!("{base}/api/rmm/sessions"))
        .bearer_auth(token)
        .json(&json!({"transport":"websocket","readOnly":false}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        watcher["readOnly"], true,
        "global View only policy must win over a controlling request"
    );
    bridge.set_rmm_settings(Default::default());
    assert_eq!(
        client
            .post(format!(
                "{base}/api/rmm/sessions/{}/heartbeat",
                watcher["id"].as_str().unwrap()
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(bridge.rmm_status().phase, bro_bridge::rmm::Phase::Disabled);
    bridge.shutdown();
    println!(
        "PASS signed download, native capture/H.264, paired gateway, exclusive control, view-only switch and disable revocation"
    );
}
