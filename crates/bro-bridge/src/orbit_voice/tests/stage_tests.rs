use super::*;

fn frame() -> StageFrame {
    StageFrame {
        window: StageWindow {
            handle: "123".into(),
            pid: 456,
            epoch: "window-1".into(),
        },
        viewport: StageViewport {
            width: 1200.0,
            height: 800.0,
            scale_factor: 1.5,
            physical_origin: StagePoint {
                x: -1200.0,
                y: 45.0,
            },
        },
        surfaces: vec![StageSurface {
            id: "pane-example-header".into(),
            kind: StageKind::OneWay,
            rect: StageRect {
                x: 250.0,
                y: 80.0,
                w: 700.0,
                h: 34.0,
            },
        }],
        exclusions: vec![StageRect {
            x: 0.0,
            y: 100.0,
            w: 200.0,
            h: 500.0,
        }],
        preferred_surface_id: Some("pane-example-header".into()),
        suspended: false,
    }
}

fn host_snapshot() -> Value {
    let mut host = snapshot(Some("a"), "g");
    host["window"] = json!({"handle":"123","pid":456,"epoch":"window-1"});
    host
}

fn published() -> Fixture {
    let fixture = Fixture::new();
    assert!(fixture.handle.publish_host(host_snapshot()));
    assert!(fixture.handle.publish_stage(Some(frame())));
    fixture
}

async fn current(fixture: &Fixture) -> Value {
    stage::get(State(fixture.shared()), Query(stage_query(None)))
        .await
        .unwrap()
        .0
}

fn stage_query(after: Option<u64>) -> stage::StageQuery {
    serde_json::from_value(match after {
        Some(after) => json!({"after":after}),
        None => json!({}),
    })
    .unwrap()
}

fn response_json(response: &str) -> Value {
    serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[test]
#[ignore = "actual loopback network test; explicit temporary data root only"]
fn independent_stage_network_heartbeat_auth_and_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let handle = start(root.path(), |_| {}).unwrap();
    assert!(handle.publish_host(host_snapshot()));
    assert!(handle.publish_stage(Some(frame())));
    let descriptor: Descriptor =
        serde_json::from_slice(&std::fs::read(handle.descriptor_path()).unwrap()).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let url = format!("{}/orbit/v1/stage", handle.local_url());
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let first: Value = client
            .get(&url)
            .bearer_auth(&descriptor.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(first["available"], true);
        let heartbeat: Value = client
            .get(format!("{url}?after={}", handle.stage_revision()))
            .bearer_auth(&descriptor.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(first, heartbeat);
        assert_eq!(
            client
                .get(&url)
                .header(header::ORIGIN, "null")
                .bearer_auth(&descriptor.token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    });
    let started = Instant::now();
    handle.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!handle.descriptor_path().exists());
}

#[test]
fn stage_contract_roundtrips_exact_camel_case_and_rejects_unknown_shapes() {
    let input = json!({
        "window":{"handle":"123","pid":456,"epoch":"window-1"},
        "viewport":{"width":1200.0,"height":800.0,"scaleFactor":1.5,"physicalOrigin":{"x":-1200.0,"y":45.0}},
        "surfaces":[{"id":"pane-example-header","kind":"oneWay","rect":{"x":250.0,"y":80.0,"w":700.0,"h":34.0}}],
        "exclusions":[{"x":0.0,"y":100.0,"w":200.0,"h":500.0}],
        "preferredSurfaceId":"pane-example-header","suspended":false
    });
    assert_eq!(
        serde_json::from_value::<StageFrame>(input.clone()).unwrap(),
        frame()
    );
    assert_eq!(serde_json::to_value(frame()).unwrap(), input);
    assert_eq!(serde_json::to_value(StageKind::Solid).unwrap(), "solid");
    let mut optional = input.clone();
    optional
        .as_object_mut()
        .unwrap()
        .remove("preferredSurfaceId");
    assert!(
        serde_json::from_value::<StageFrame>(optional)
            .unwrap()
            .preferred_surface_id
            .is_none()
    );
    for malformed in [
        {
            let mut value = input.clone();
            value["schema"] = json!(2);
            value
        },
        {
            let mut value = input.clone();
            value["surfaces"][0]["kind"] = json!("oneway");
            value
        },
        {
            let mut value = input.clone();
            value["viewport"]["physicalOrigin"] = Value::Null;
            value
        },
    ] {
        assert!(serde_json::from_value::<StageFrame>(malformed).is_err());
    }
}

#[test]
fn stage_accepts_boundary_geometry_and_maximum_safe_counts() {
    let fixture = published();
    let mut frame = frame();
    frame.surfaces = (0..96)
        .map(|index| StageSurface {
            id: format!("{index}:{}", "s".repeat(157)),
            kind: if index % 2 == 0 {
                StageKind::Solid
            } else {
                StageKind::OneWay
            },
            rect: StageRect {
                x: 0.0,
                y: 0.0,
                w: 1200.0,
                h: 800.0,
            },
        })
        .collect();
    frame.exclusions = vec![
        StageRect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 800.0
        };
        32
    ];
    frame.preferred_surface_id = Some(frame.surfaces[95].id.clone());
    assert!(serde_json::to_vec(&frame).unwrap().len() <= 64 * 1024);
    assert!(fixture.handle.publish_stage(Some(frame)));
}

#[test]
fn invalid_geometry_clears_stage_independently_of_valid_session_context() {
    let invalid: Vec<Box<dyn Fn(&mut StageFrame)>> = vec![
        Box::new(|frame| frame.window.handle = "0".into()),
        Box::new(|frame| frame.window.handle = "18446744073709551616".into()),
        Box::new(|frame| frame.window.pid = 0),
        Box::new(|frame| frame.window.pid += 1),
        Box::new(|frame| frame.window.epoch = "window-0".into()),
        Box::new(|frame| frame.window.epoch = "bad epoch".into()),
        Box::new(|frame| frame.viewport.width = f64::NAN),
        Box::new(|frame| frame.viewport.width = f64::INFINITY),
        Box::new(|frame| frame.viewport.height = 0.0),
        Box::new(|frame| frame.viewport.width = 100_001.0),
        Box::new(|frame| frame.viewport.scale_factor = 0.0),
        Box::new(|frame| frame.viewport.scale_factor = 17.0),
        Box::new(|frame| frame.viewport.physical_origin.x = f64::NEG_INFINITY),
        Box::new(|frame| frame.viewport.physical_origin.y = 10_000_001.0),
        Box::new(|frame| frame.surfaces[0].rect.x = -0.1),
        Box::new(|frame| frame.surfaces[0].rect.y = f64::NAN),
        Box::new(|frame| frame.surfaces[0].rect.w = -1.0),
        Box::new(|frame| frame.surfaces[0].rect.h = 0.0),
        Box::new(|frame| frame.surfaces[0].rect.w = 1200.0),
        Box::new(|frame| frame.surfaces[0].rect.h = 800.0),
        Box::new(|frame| frame.surfaces[0].rect.w = f64::INFINITY),
        Box::new(|frame| frame.surfaces[0].id.clear()),
        Box::new(|frame| frame.surfaces[0].id = "x".repeat(161)),
        Box::new(|frame| frame.surfaces[0].id = "unsafe/id".into()),
        Box::new(|frame| frame.surfaces[0].id = "not-ASCII-é".into()),
        Box::new(|frame| frame.surfaces.push(frame.surfaces[0].clone())),
        Box::new(|frame| frame.surfaces = vec![frame.surfaces[0].clone(); 97]),
        Box::new(|frame| frame.exclusions = vec![frame.exclusions[0].clone(); 33]),
        Box::new(|frame| frame.exclusions[0].x = -1.0),
        Box::new(|frame| frame.exclusions[0].h = 900.0),
        Box::new(|frame| frame.preferred_surface_id = Some("removed".into())),
    ];
    let fixture = published();
    for (index, mutate) in invalid.iter().enumerate() {
        assert!(fixture.handle.publish_stage(Some(frame())));
        let before = fixture.handle.stage_revision();
        let host = fixture.shared().data.lock().host.clone();
        let mut bad = frame();
        mutate(&mut bad);
        assert!(
            !fixture.handle.publish_stage(Some(bad)),
            "invalid frame {index}"
        );
        assert_eq!(
            fixture.handle.stage_revision(),
            before + 1,
            "invalid frame {index}"
        );
        assert_eq!(
            fixture.shared().stage.borrow().metadata()["available"],
            false
        );
        assert_eq!(fixture.shared().data.lock().host, host);
        assert!(fixture.handle.ready());
    }
}

#[tokio::test]
async fn optional_stage_is_safe_without_changing_old_control_capabilities() {
    let fixture = Fixture::new();
    fixture.publish(None, "g");
    let empty = current(&fixture).await;
    assert_eq!(
        empty,
        json!({"schema":1,"instanceId":fixture.handle.instance_id(),"revision":0,"available":false,"frame":null})
    );
    assert!(!fixture.handle.publish_stage(Some(frame()))); // No host native window.
    let mut input = host_snapshot();
    input["window"].as_object_mut().unwrap().remove("epoch"); // Older host producer.
    input["stage"] = json!({"schema":2,"revision":999,"available":true,"surfaces":["injected"]});
    input["capabilities"] = json!(["arbitrary.execute"]);
    assert!(fixture.handle.publish_host(input));
    assert!(fixture.handle.publish_stage(Some(frame())));
    let normalized = host(State(fixture.shared())).await.0;
    let mut expected: Vec<_> = CAPABILITIES.iter().copied().collect();
    expected.push("avatar.stage.v1");
    expected.push("avatar.roam.v1");
    assert_eq!(normalized["capabilities"], json!(expected));
    assert_eq!(
        normalized["stage"],
        json!({"schema":1,"revision":1,"available":true})
    );
    assert!(normalized["stage"].get("frame").is_none());
    assert!(normalized.get("surfaces").is_none());
    assert!(!normalized.to_string().contains("arbitrary.execute"));
    assert!(fixture.handle.publish_stage(None));
    assert_eq!(current(&fixture).await["frame"], Value::Null);
    assert!(fixture.handle.ready());
}

#[tokio::test]
async fn optional_roaming_policy_survives_snapshots_without_changing_fixed_fallback_or_stage() {
    let fixture = published();
    let original = current(&fixture).await;
    let policy = json!({"schema":1,"policy":"auto"});
    let mut input = host_snapshot();
    input["mode"] = json!("attached");
    input["avatarRoam"] = policy.clone();
    assert!(fixture.handle.publish_host(input.clone()));
    let normalized = host(State(fixture.shared())).await.0;
    assert_eq!(normalized["mode"], "attached", "old clients keep the fixed fallback");
    assert_eq!(normalized["avatarRoam"], policy);
    assert_eq!(current(&fixture).await, original, "placement metadata never replaces geometry");
    assert!(!CAPABILITIES.contains(&"avatar.roam.v1"), "roaming is optional, not a required capability");
    input["focusedSessionId"] = json!("b");
    assert!(fixture.handle.publish_host(input.clone()));
    assert_eq!(host(State(fixture.shared())).await.0["avatarRoam"], policy);
    for invalid in [
        Value::Null,
        json!({"schema":2,"policy":"auto"}),
        json!({"schema":1,"policy":"execute"}),
        json!({"schema":1,"policy":"auto","command":"not-authority"}),
        json!({"schema":"1","policy":"auto"}),
        json!("auto"),
    ] {
        input["avatarRoam"] = invalid;
        assert!(fixture.handle.publish_host(input.clone()), "bad optional policy must not discard session context");
        let safe = host(State(fixture.shared())).await.0;
        assert!(safe.get("avatarRoam").is_none());
        assert_eq!(safe["sessions"].as_array().unwrap().len(), 2);
        assert_eq!(current(&fixture).await, original);
    }
    input["avatarRoam"] = policy;
    input["window"] = Value::Null;
    assert!(fixture.handle.publish_host(input));
    assert!(host(State(fixture.shared())).await.0.get("avatarRoam").is_none());
}

#[tokio::test]
async fn host_replacement_and_epoch_changes_invalidate_stale_geometry() {
    let fixture = published();
    let original = current(&fixture).await;
    let mut ordinary = host_snapshot();
    ordinary["focusedSessionId"] = json!("b");
    assert!(fixture.handle.publish_host(ordinary.clone()));
    assert_eq!(current(&fixture).await, original);
    assert!(fixture.handle.publish_host(ordinary));
    assert_eq!(current(&fixture).await, original);
    for field in ["handle", "pid", "epoch"] {
        assert!(fixture.handle.publish_host(host_snapshot()));
        assert!(fixture.handle.publish_stage(Some(frame())));
        let revision = fixture.handle.stage_revision();
        let mut replacement = host_snapshot();
        replacement["window"][field] = match field {
            "handle" => json!("124"),
            "pid" => json!(457),
            _ => json!("window-2"),
        };
        assert!(fixture.handle.publish_host(replacement));
        assert_eq!(fixture.handle.stage_revision(), revision + 1);
        assert_eq!(current(&fixture).await["frame"], Value::Null);
        assert!(!fixture.handle.publish_stage(Some(frame())));
    }
    assert!(fixture.handle.publish_host(host_snapshot()));
    assert!(fixture.handle.publish_stage(Some(frame())));
    fixture.publish(None, "g");
    assert_eq!(current(&fixture).await["frame"], Value::Null);
    // Malformed epoch fails host normalization rather than stripping its binding.
    let mut bad = host_snapshot();
    bad["window"]["epoch"] = json!("bad epoch");
    assert!(!fixture.handle.publish_host(bad));
}

#[tokio::test]
async fn stage_is_latest_only_monotonic_and_does_not_flood_request_ring() {
    let fixture = published();
    latch(&fixture, "u").await;
    send(&fixture, "r", "u", "Work").await.unwrap();
    assert!(fixture.handle.queued("r"));
    let (sequence, events) = {
        let shared = fixture.shared();
        let data = shared.data.lock();
        (data.sequence, data.events.clone())
    };
    let mut frame = frame();
    for index in 1..600 {
        frame.surfaces[0].rect.x = index as f64 * 0.5;
        assert!(fixture.handle.publish_stage(Some(frame.clone())));
    }
    let revision = fixture.handle.stage_revision();
    let latest = current(&fixture).await;
    assert_eq!(latest["revision"], revision);
    assert_eq!(latest["frame"]["surfaces"][0]["rect"]["x"], 299.5);
    assert!(fixture.handle.publish_stage(Some(frame.clone())));
    assert_eq!(fixture.handle.stage_revision(), revision);
    let shared = fixture.shared();
    assert_eq!(shared.data.lock().sequence, sequence);
    assert_eq!(shared.data.lock().events, events);
    assert!(shared.data.lock().requests["r"].state == RequestState::Queued);
    frame.suspended = true;
    assert!(fixture.handle.publish_stage(Some(frame)));
    let suspended = current(&fixture).await;
    assert_eq!(suspended["available"], false);
    assert_eq!(suspended["frame"]["suspended"], true);
    assert_eq!(suspended["revision"], revision + 1);
    assert!(fixture.handle.publish_stage(None));
    assert_eq!(current(&fixture).await["frame"], Value::Null);
}

#[tokio::test]
async fn unchanged_heartbeats_renew_current_revision_and_future_cursors_reset() {
    let fixture = published();
    let revision = fixture.handle.stage_revision();
    let started = Instant::now();
    let heartbeat = stage::get(State(fixture.shared()), Query(stage_query(Some(revision))))
        .await
        .unwrap()
        .0;
    assert_eq!(heartbeat, current(&fixture).await);
    assert!(started.elapsed() >= Duration::from_millis(800));
    assert!(started.elapsed() < Duration::from_millis(1400));
    assert_eq!(fixture.handle.stage_revision(), revision);
    let started = Instant::now();
    let reset = stage::get(
        State(fixture.shared()),
        Query(stage_query(Some(revision + 100))),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(reset, heartbeat);
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[tokio::test]
async fn watch_wakes_on_publication_and_aborts_on_shutdown_without_holding_data() {
    let fixture = published();
    let shared = fixture.shared();
    let watch = stage::get(
        State(shared.clone()),
        Query(stage_query(Some(fixture.handle.stage_revision()))),
    );
    tokio::pin!(watch);
    assert!(futures_util::poll!(&mut watch).is_pending());
    assert!(shared.data.try_lock().is_some());
    let mut moved = frame();
    moved.surfaces[0].rect.y += 1.0;
    assert!(fixture.handle.publish_stage(Some(moved)));
    let updated = tokio::time::timeout(Duration::from_millis(200), watch)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(updated["frame"]["surfaces"][0]["rect"]["y"], 81.0);
    let watch = stage::get(
        State(shared.clone()),
        Query(stage_query(Some(fixture.handle.stage_revision()))),
    );
    tokio::pin!(watch);
    assert!(futures_util::poll!(&mut watch).is_pending());
    fixture.handle.shutdown();
    let stopped = tokio::time::timeout(Duration::from_millis(200), watch)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(stopped.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!fixture.handle.publish_stage(Some(frame())));
    assert_eq!(shared.stage.borrow().metadata()["available"], false);
}

#[tokio::test]
async fn router_stage_uses_exact_auth_peer_host_origin_and_bounds_queries() {
    let fixture = published();
    assert!(
        http(&fixture, "GET", "/orbit/v1/stage", "", "", false)
            .await
            .starts_with("HTTP/1.1 401")
    );
    assert!(
        http(
            &fixture,
            "GET",
            "/orbit/v1/stage?token=legacy",
            "x-terminal-web-token: legacy\r\n",
            "",
            false
        )
        .await
        .starts_with("HTTP/1.1 401")
    );
    for headers in [
        "Origin: null\r\n",
        "Host: localhost:12345\r\n",
        "Host: 127.0.0.1:12345\r\n",
    ] {
        assert!(
            http(&fixture, "GET", "/orbit/v1/stage", headers, "", true)
                .await
                .starts_with("HTTP/1.1 403")
        );
    }
    for query in [
        "after=-1",
        "after=wat",
        "after=18446744073709551616",
        "after=1&after=1",
        "schema=2",
    ] {
        let response = http(
            &fixture,
            "GET",
            &format!("/orbit/v1/stage?{query}"),
            "",
            "",
            true,
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400"), "{query}: {response}");
    }
    let response = http(&fixture, "GET", "/orbit/v1/stage", "", "", true).await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("cache-control: no-store"));
    assert!(!response.contains("access-control-allow-origin"));
    assert!(!response.contains(&fixture.shared().token));
    assert_eq!(response_json(&response), current(&fixture).await);
    let heartbeat = http(
        &fixture,
        "GET",
        &format!("/orbit/v1/stage?after={}", fixture.handle.stage_revision()),
        "",
        "",
        true,
    )
    .await;
    assert!(heartbeat.starts_with("HTTP/1.1 200"), "{heartbeat}");
    assert_eq!(response_json(&response), response_json(&heartbeat));
    let oversized = http(
        &fixture,
        "GET",
        "/orbit/v1/stage",
        "",
        &"x".repeat(MAX_BODY + 1),
        true,
    )
    .await;
    assert!(oversized.starts_with("HTTP/1.1 413"));
}

#[tokio::test]
async fn stage_capacity_rate_and_cancellation_are_isolated_from_control() {
    let fixture = published();
    let shared = fixture.shared();
    // Even all 32 held control permits cannot prevent the independent stage lane.
    let control = shared
        .in_flight
        .clone()
        .acquire_many_owned(32)
        .await
        .unwrap();
    assert!(
        http(&fixture, "GET", "/orbit/v1/stage", "", "", true)
            .await
            .starts_with("HTTP/1.1 200")
    );
    drop(control);
    // Saturated stage capacity and rate budget never consume control resources.
    let stage_capacity = shared
        .stage_in_flight
        .clone()
        .acquire_many_owned(stage::MAX_WATCHERS as u32)
        .await
        .unwrap();
    assert!(
        http(&fixture, "GET", "/orbit/v1/stage", "", "", true)
            .await
            .starts_with("HTTP/1.1 429")
    );
    assert!(
        http(&fixture, "GET", "/orbit/v1/host", "", "", true)
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert_eq!(shared.in_flight.available_permits(), 32);
    drop(stage_capacity);
    while shared.stage_rate.lock().admit() {}
    assert!(
        http(&fixture, "GET", "/orbit/v1/stage", "", "", true)
            .await
            .starts_with("HTTP/1.1 429")
    );
    assert!(
        http(
            &fixture,
            "POST",
            "/orbit/v1/utterances",
            "",
            r#"{"id":"u"}"#,
            true
        )
        .await
        .starts_with("HTTP/1.1 200")
    );
    assert!(
        http(
            &fixture,
            "POST",
            "/orbit/v1/requests",
            "",
            r#"{"id":"r","utteranceId":"u","text":"Work"}"#,
            true
        )
        .await
        .starts_with("HTTP/1.1 200")
    );
    assert_eq!(fixture.commands.lock().len(), 1);
    *shared.stage_rate.lock() = stage::Rate::new();
    let path = format!("/orbit/v1/stage?after={}", fixture.handle.stage_revision());
    let request = http(&fixture, "GET", &path, "", "", true);
    tokio::pin!(request);
    // Drive the actual router until its authenticated watch holds a stage permit.
    tokio::select! {
        biased;
        result = &mut request => panic!("watch returned early: {result}"),
        _ = async {
            loop {
                tokio::task::yield_now().await;
                if shared.stage_in_flight.available_permits() < stage::MAX_WATCHERS { break; }
            }
        } => {},
    }
    assert!(shared.data.try_lock().is_some());
    assert_eq!(shared.in_flight.available_permits(), 32);
    assert!(
        http(&fixture, "GET", "/orbit/v1/events", "", "", true)
            .await
            .starts_with("HTTP/1.1 200")
    );
    fixture.handle.shutdown();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), request)
            .await
            .unwrap()
            .starts_with("HTTP/1.1 503")
    );
    assert_eq!(
        shared.stage_in_flight.available_permits(),
        stage::MAX_WATCHERS
    );
}
