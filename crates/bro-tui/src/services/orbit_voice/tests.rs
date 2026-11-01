use super::*;
use bro_bridge::orbit_voice::{StageKind, StagePoint, StageRect, StageSurface, StageViewport};
use bro_core::config::Settings;

fn frame(epoch: &str, x: f64) -> StageFrame {
    StageFrame {
        window: StageWindow {
            handle: "123".into(),
            pid: 123,
            epoch: epoch.into(),
        },
        viewport: StageViewport {
            width: 1200.,
            height: 800.,
            scale_factor: 1.5,
            physical_origin: StagePoint { x: -1200., y: 45. },
        },
        suspended: false,
        surfaces: vec![StageSurface {
            id: "pane-header".into(),
            kind: StageKind::OneWay,
            rect: StageRect {
                x,
                y: 80.,
                w: 700.,
                h: 34.,
            },
        }],
        exclusions: vec![],
        preferred_surface_id: Some("pane-header".into()),
    }
}

#[test]
fn stage_mailbox_is_latest_only_and_clear_retains_window_epoch() {
    crate::testkit::isolate();
    let (tx, _rx) = std::sync::mpsc::channel();
    let svc = Services::with_state(Services::empty_state(Settings::default()), tx, false, false);
    for x in 0..200 {
        svc.queue_orbit_stage(Some(frame("window-one", f64::from(x))));
    }
    let mailbox = &svc.inner.orbit_stage;
    let revision = mailbox.value.lock().revision;
    assert_eq!(revision, 200);
    assert_eq!(
        mailbox.value.lock().frame.as_ref().unwrap().surfaces[0]
            .rect
            .x,
        199.
    );
    svc.queue_orbit_stage(Some(frame("window-one", 199.)));
    assert_eq!(
        mailbox.value.lock().revision,
        revision,
        "text/spinner-only repaints do not publish"
    );
    svc.queue_orbit_stage(None);
    let value = mailbox.value.lock();
    assert!(value.frame.is_none());
    assert_eq!(value.window.as_ref().unwrap().epoch, "window-one");
    assert_eq!(value.revision, revision + 1);
    assert!(
        !mailbox.started.load(Ordering::Acquire),
        "disabled handoff does not start a service"
    );
}

#[test]
#[ignore = "isolated authenticated loopback stage worker; no native window, microphone or provider"]
fn stage_worker_waits_for_host_epoch_and_flushes_the_last_frame() {
    crate::testkit::isolate();
    let root = tempfile::tempdir().unwrap();
    let handle = orbit_voice::start(root.path(), |_| {}).unwrap();
    let descriptor: serde_json::Value =
        serde_json::from_slice(&std::fs::read(handle.descriptor_path()).unwrap()).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    let svc = Services::with_state(Services::empty_state(Settings::default()), tx, false, false);
    svc.attach_orbit_test(handle.clone());
    svc.start_orbit_stage_worker();
    // A first painted frame can precede the listener's initial safe host snapshot.
    for x in 0..200 {
        svc.queue_orbit_stage(Some(frame("first-window", f64::from(x))));
    }
    let host = serde_json::json!({
        "protocol":1,"instanceId":handle.instance_id(),"name":"bro",
        "capabilities":["sessions.read","orchestrator.submit","requests.observe","avatar.stage.v1"],
        "mode":"attached","window":{"handle":"123","pid":123},
        "focusedSessionId":null,"sessions":[]
    });
    svc.publish_orbit_host(host.clone());
    let get = |path: &str| -> serde_json::Value {
        use std::io::{Read, Write};
        let endpoint = url::Url::parse(handle.local_url()).unwrap();
        let host = format!(
            "{}:{}",
            endpoint.host_str().unwrap(),
            endpoint.port().unwrap()
        );
        let mut socket = std::net::TcpStream::connect(&host).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(socket, "GET /orbit/v1{path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n", descriptor["token"].as_str().unwrap()).unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert_eq!(headers.split_whitespace().nth(1), Some("200"));
        serde_json::from_str(body).unwrap()
    };
    let snapshot = get("/host");
    assert_eq!(snapshot["window"]["epoch"], "first-window");
    let staged = get("/stage?after=0");
    assert_eq!(staged["available"], true);
    assert_eq!(staged["frame"]["surfaces"][0]["rect"]["x"], 199.);
    // An ordinary context/focus snapshot must not clear the independent mailbox.
    svc.publish_orbit_host(host);
    assert_eq!(get("/stage?after=0")["revision"], staged["revision"]);
    svc.queue_orbit_stage(None);
    let cleared = get(&format!("/stage?after={}", staged["revision"]));
    assert_eq!(cleared["available"], false);
    assert!(cleared["frame"].is_null());
    assert_eq!(get("/host")["window"]["epoch"], "first-window");
    svc.shutdown();
}

#[test]
fn registered_preview_survives_a_queued_save_and_a_stale_settings_form() {
    crate::testkit::isolate();
    let (tx, _rx) = std::sync::mpsc::channel();
    // Offline suppresses automatic workers; exercise the production disk-write lane explicitly.
    let mut initial = Settings::default();
    initial.theme = "current-live-theme".into();
    initial.orbit_voice.mode = "attached".into();
    let svc = Services::offline(initial, tx);
    let mut stale_form = svc.settings();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let queued = {
        let svc = svc.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            svc.persist_current_settings().unwrap();
        })
    };
    let preview = bro_core::orbit_voice::PreviewInstallation {
        executable: std::env::temp_dir().join("registered-orbit-fixture/electron.exe"),
        application_dir: std::env::temp_dir().join("registered-orbit-fixture/app"),
        version: "1.3.0-bro-preview.1".into(),
    };
    svc.register_orbit_preview(preview.clone()).unwrap();
    stale_form.prefix = "ctrl+a".into();
    svc.save_settings(stale_form);
    barrier.wait();
    queued.join().unwrap();
    let saved = Settings::load();
    assert_eq!(saved.theme, "current-live-theme");
    assert_eq!(saved.prefix, "ctrl+a");
    assert_eq!(saved.orbit_voice.mode, "attached");
    assert!(
        !saved.orbit_voice.enabled,
        "registration is not microphone or connection consent"
    );
    assert_eq!(saved.orbit_voice.executable, Some(preview.executable));
    assert_eq!(
        saved.orbit_voice.application_dir,
        Some(preview.application_dir)
    );
}
