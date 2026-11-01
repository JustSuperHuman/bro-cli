//! Actual hidden Electron main/React/server, fake devices/provider, real authenticated native controls.
use super::*;
use std::process::{Child, Command as ProcessCommand, Stdio};

#[track_caller]
fn wait_for(label: &str, mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < until, "managed acceptance timed out: {label}");
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn report(directory: &Path) -> Value {
    std::fs::read(directory.join("report.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null)
}
struct OwnedFixture(Child);
impl Drop for OwnedFixture {
    fn drop(&mut self) {
        // This is only the exact child spawned by this fixture, never process-name discovery.
        if self.0.try_wait().ok().flatten().is_none() { let _ = self.0.kill(); let _ = self.0.wait(); }
    }
}
#[test]
#[ignore = "actual isolated Electron and loopback host; explicit fixture paths required; fake audio/provider only"]
fn hidden_managed_app_uses_native_controls_and_survives_owner_handover() {
    let electron = PathBuf::from(std::env::var_os("BRO_ORBIT_ACCEPTANCE_ELECTRON").expect("fixture Electron path"));
    let application = PathBuf::from(std::env::var_os("BRO_ORBIT_ACCEPTANCE_APP").expect("fixture source preview path"));
    let settings = bro_core::config::OrbitSettings { enabled: true, executable: Some(electron.clone()), application_dir: Some(application.clone()), ..Default::default() };
    let build = bro_core::orbit_voice::managed_build_id(&settings).unwrap();
    let root = tempfile::tempdir().unwrap().keep();
    let evidence = root.join("acceptance");
    std::fs::create_dir(&evidence).unwrap();
    println!("Isolated managed fixture: {}", evidence.display());
    let workspace = crate::Bridge::start_local(crate::BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(),
        data_root: root.join("workspace"), web_interface: false }, Box::new(|_| {})).unwrap();
    let make_host = || {
        let host = start_managed(&root, |_| {}).unwrap();
        host.attach_workspace(Some(workspace.clone()));
        assert!(host.publish_host(json!({"protocol":1,"instanceId":host.instance_id(),"name":"bro", "mode":"attached",
            "avatarPlacement":{"schema":1,"allowEscape":false}, "focusedSessionId":null,"sessions":[]})));
        host
    };
    let first = make_host();
    let child = ProcessCommand::new(&electron).arg(application.join("scripts/managed-runtime-e2e.mjs"))
        .arg("--bro-managed").arg("--external-host-file").arg(first.descriptor_path())
        .arg("--acceptance-dir").arg(&evidence)
        .stdout(Stdio::from(std::fs::File::create(evidence.join("stdout.log")).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(evidence.join("stderr.log")).unwrap())).spawn().unwrap();
    let mut child = OwnedFixture(child);
    first.expect_runtime(child.0.id(), build.clone());
    wait_for("native lifecycle transition", || first.runtime_status().is_some_and(|s| s.renderer_ready) && report(&evidence)["startup"].is_object());
    let initial = report(&evidence);
    assert_eq!(initial["startup"]["microphoneOpens"], 0);
    assert_eq!(initial["startup"]["contexts"], 0);
    assert_eq!(initial["startup"]["providerStarts"], 0);
    assert_eq!(initial["startup"]["visible"], false);
    first.control(VoiceControl::Start {}).unwrap();
    wait_for("native lifecycle transition", || first.runtime_status().is_some_and(|s| s.phase == Phase::Listening && s.microphone == Microphone::Active));
    first.control(VoiceControl::SetMute { muted: true }).unwrap();
    wait_for("native lifecycle transition", || first.runtime_status().is_some_and(|s| s.muted && s.microphone == Microphone::Active));
    first.control(VoiceControl::SetMute { muted: false }).unwrap();
    wait_for("native lifecycle transition", || first.runtime_status().is_some_and(|s| !s.muted));
    first.control(VoiceControl::ShowSettings {}).unwrap();
    wait_for("native lifecycle transition", || report(&evidence)["settingsHidden"] == true);
    assert!(first.runtime_status().is_some_and(|s| s.phase == Phase::Listening));
    let runtime = first.runtime_status().unwrap().runtime_id;
    let epoch = first.instance_id().to_owned();
    first.shutdown();
    let next = make_host();
    assert_ne!(next.instance_id(), epoch);
    wait_for("native lifecycle transition", || next.runtime_status().is_some_and(|s| s.runtime_id == runtime && s.phase == Phase::Listening && s.microphone == Microphone::Active));
    let continued = report(&evidence);
    assert_eq!(continued["renderer"]["contexts"], 1);
    assert_eq!(continued["renderer"]["voiceSockets"], 1);
    assert_eq!(continued["providerStarts"], 1);
    assert_eq!(continued["renderer"]["tracksStopped"], 0);
    assert_eq!(continued["rendererIds"].as_array().unwrap().len(), 1);
    next.shutdown();
    // Reproduce an interrupted handoff that lost its runtime record. Verified
    // native discovery can authorize the same process without another voice socket.
    std::fs::write(root.join("orbit/managed-runtime.json"), br#"{"intentionalStop":false,"runtime":null}"#).unwrap();
    let next = make_host();
    assert!(!next.previously_owned_runtime());
    next.expect_runtime_process(&bro_core::orbit_voice::ManagedLaunch {
        pid: child.0.id(), build_id: build,
        process_identity: bro_core::orbit_voice::managed_process_identity(child.0.id()).unwrap(), reused: true,
    });
    wait_for("missing-record recovery", || next.runtime_status().is_some_and(|s| s.runtime_id == runtime && s.phase == Phase::Listening && s.microphone == Microphone::Active));
    assert_eq!(report(&evidence)["providerStarts"], 1);
    assert_eq!(report(&evidence)["rendererIds"].as_array().unwrap().len(), 1);
    next.control(VoiceControl::Stop {}).unwrap();
    wait_for("native lifecycle transition", || next.runtime_status().is_some_and(|s| s.phase == Phase::Off && s.microphone == Microphone::Off));
    wait_for("native lifecycle transition", || report(&evidence)["renderer"]["tracksStopped"] == 1 && report(&evidence)["providerCloses"] == 1);
    next.control(VoiceControl::Quit {}).unwrap();
    wait_for("native lifecycle transition", || child.0.try_wait().unwrap().is_some());
    assert!(child.0.wait().unwrap().success());
    assert!(next.intentionally_stopped());
    let final_report = report(&evidence);
    assert_eq!(final_report["errors"], json!([]), "{final_report}");
    assert_eq!(final_report["blockedChildren"], 0);
    assert_eq!(final_report["clipboardWrites"], 0);
    assert_eq!(final_report["desktopCalls"], 0);
    assert_eq!(final_report["renderer"]["contextsClosed"], 1);
    let names = final_report["tools"].as_array().unwrap();
    for name in ["read_session", "read_chat", "coordination_snapshot", "coordination_task"] {
        assert!(names.contains(&json!(name)), "canonical tools must recover on the same live provider socket");
    }
    next.shutdown();
    workspace.shutdown();
    // Retain only this fixture's evidence for visual inspection, never descriptors or profile secrets.
    let retained = std::env::temp_dir().join(format!("bro-orbit-managed-evidence-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&retained).unwrap();
    std::fs::copy(evidence.join("advanced-settings.png"), retained.join("advanced-settings.png")).unwrap();
    std::fs::write(retained.join("report.json"), serde_json::to_vec_pretty(&final_report).unwrap()).unwrap();
    println!("Managed fixture evidence: {}", retained.display());
}
