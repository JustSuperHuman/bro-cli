//! Network and lifecycle regressions for the optional desktop gateway.
use super::*;
use tokio::task::JoinHandle;

const TOKEN: &str = "rmm-acceptance-pairing-token";

struct Server(JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(router: Router) -> (String, Server) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://{address}"), Server(task))
}

fn state(root: &std::path::Path) -> AppState {
    AppState::new(
        crate::BridgeConfig {
            port: 0,
            automatic_port: false,
            bind: "127.0.0.1".into(),
            data_root: root.join("bridge"),
            web_interface: false,
        },
        TOKEN.into(),
        Arc::new(|_| {}),
    )
}

fn viewer(id: &str, read_only: bool, transport: &str) -> Arc<Lease> {
    Arc::new(Lease {
        id: id.into(),
        read_only,
        transport: transport.into(),
        deadline: Mutex::new(Instant::now() + LEASE_TTL),
        reconnect_at: Instant::now() + Duration::from_secs(50 * 60),
        cancelled: watch::channel(false).0,
        attached: AtomicBool::new(false),
        rtc_id: Mutex::new(None),
        ice_servers: json!([]),
        _control_lock: None,
    })
}

#[tokio::test]
async fn real_loopback_routes_require_pairing_and_never_enable_or_download_the_package() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    let (origin, _server) = serve(crate::server::router(state.clone())).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in ["/api/rmm", "/api/rmm/sessions", "/rmm/unknown/screenshot"] {
        let request = if path.ends_with("/sessions") {
            client.post(format!("{origin}{path}")).json(&json!({}))
        } else {
            client.get(format!("{origin}{path}"))
        };
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    let response = client
        .get(format!("{origin}/api/rmm"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let status: Value = response.json().await.unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["phase"], "disabled");
    for transport in ["webrtc", "websocket"] {
        let response = client
            .post(format!("{origin}/api/rmm/sessions"))
            .bearer_auth(TOKEN)
            .json(&json!({"transport":transport}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    assert!(!state.rmm.installing.load(Ordering::Acquire));
    assert!(state.rmm.inner.lock().process.is_none());
    assert!(state.rmm.inner.lock().leases.is_empty());
    assert!(!state.rmm.root.join("packages").exists());
}

#[tokio::test]
async fn real_routes_refuse_cross_site_requests_even_with_the_correct_pairing_token() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    let (origin, _server) = serve(crate::server::router(state)).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for (name, value) in [
        ("origin", "https://unrelated.example"),
        ("sec-fetch-site", "cross-site"),
    ] {
        let response = client
            .post(format!("{origin}/api/rmm/sessions"))
            .bearer_auth(TOKEN)
            .header(name, value)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{name}");
    }
    // A browser on Bro's own origin can read the status, with the same pairing requirement.
    assert_eq!(
        client
            .get(format!("{origin}/api/rmm"))
            .bearer_auth(TOKEN)
            .header("origin", &origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn cancellation_before_websocket_upgrade_does_not_wait_for_a_second_cancel_signal() {
    let root = tempfile::tempdir().unwrap();
    let rmm = Arc::new(Rmm::new(root.path().to_owned()));
    let lease = viewer("cancelled-before-upgrade", false, "websocket");
    // Set the value before relay() creates its watch receiver. changed() would miss this.
    lease.cancelled.send_replace(true);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    let host = Server(tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        // Stay connected and quiet; only gateway cancellation can end the viewer.
        while let Some(Ok(_)) = socket.next().await {}
    }));
    let (upstream, _) = tokio_tungstenite::connect_async(format!("ws://{upstream_address}"))
        .await
        .unwrap();
    let upstream = Arc::new(tokio::sync::Mutex::new(Some(upstream)));
    let router = Router::new().route(
        "/relay",
        get(move |ws: WebSocketUpgrade| {
            let upstream = upstream.clone();
            let lease = lease.clone();
            let rmm = rmm.clone();
            async move {
                let upstream = upstream.lock().await.take().unwrap();
                ws.on_upgrade(move |client| relay(client, upstream, lease, rmm))
            }
        }),
    );
    let (origin, _server) = serve(router).await;
    let (mut phone, _) =
        tokio_tungstenite::connect_async(format!("{}/relay", origin.replacen("http:", "ws:", 1)))
            .await
            .unwrap();
    // Closing without a WebSocket close frame may report a protocol error; either result
    // proves the revoked connection ended. Remaining open is the security regression.
    let closed = tokio::time::timeout(Duration::from_secs(2), phone.next()).await;
    assert!(
        closed.is_ok(),
        "A viewer cancelled before upgrade remained connected."
    );
    drop(host);
}

#[cfg(windows)]
fn process(origin: String) -> Process {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "rmm::acceptance_tests::owned_process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (child, tree) = ProcessTree::spawn(&mut command).unwrap();
    Process {
        child,
        tree,
        target: Target {
            origin,
            password: "fixture-password".into(),
        },
    }
}

#[cfg(windows)]
#[test]
#[ignore = "Helper process owned by the lifecycle tests; not a standalone test."]
fn owned_process_fixture() {
    std::thread::sleep(Duration::from_secs(60));
}

#[cfg(windows)]
#[tokio::test]
async fn changing_to_view_only_while_admission_waits_cannot_publish_an_old_control_lease() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    state.rmm.config.write().enabled = true;
    {
        let mut inner = state.rmm.inner.lock();
        inner.phase = Phase::Running;
        inner.version = Some("test".into());
        inner.process = Some(process("http://127.0.0.1:9".into()));
    }
    let starting = state.rmm.start_lock.lock().await;
    let pending_state = state.clone();
    let pending = tokio::spawn(async move {
        open(
            State(pending_state),
            Json(Open {
                read_only: false,
                transport: "websocket".into(),
            }),
        )
        .await
    });
    // Wait for admission to own the real exclusive file lock, proving it already
    // captured the old control policy and has reached the blocked startup step.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if package::lock(&state.rmm.root, "control.lock").is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    state.rmm.configure(package::Settings {
        enabled: true,
        read_only: true,
        display: 0,
    });
    // Model a replacement stream under the new policy, without downloading or
    // starting Voxl. Admission must still reject the pre-change request.
    state.rmm.inner.lock().process = Some(process("http://127.0.0.1:9".into()));
    drop(starting);
    let result = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().0, StatusCode::GONE);
    assert!(state.rmm.inner.lock().leases.is_empty());
    assert!(package::lock(&state.rmm.root, "control.lock").is_ok());
    state.rmm.stop();
}

#[cfg(windows)]
#[tokio::test]
async fn an_unconfirmed_rtc_teardown_kills_the_owned_process_and_revokes_other_viewers() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    let router = Router::new().route(
        "/rtc/peer",
        axum::routing::delete(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
    );
    let (origin, _server) = serve(router).await;
    state.rmm.config.write().enabled = true;
    let spectator = viewer("spectator", true, "webrtc");
    let mut controller = viewer("controller", false, "webrtc");
    {
        let controller = Arc::get_mut(&mut controller).unwrap();
        controller._control_lock = Some(package::lock(&state.rmm.root, "control.lock").unwrap());
        *controller.rtc_id.lock() = Some("peer".into());
    }
    {
        let mut inner = state.rmm.inner.lock();
        inner.phase = Phase::Running;
        inner.version = Some("test".into());
        inner.process = Some(process(origin));
        inner.leases.insert(controller.id.clone(), controller);
        inner.leases.insert(spectator.id.clone(), spectator.clone());
    }
    state.rmm.close("controller").await;
    assert!(
        state.rmm.inner.lock().process.is_none(),
        "The peer's process survived a refused teardown."
    );
    assert!(state.rmm.inner.lock().leases.is_empty());
    assert!(*spectator.cancelled.borrow());
    assert!(package::lock(&state.rmm.root, "control.lock").is_ok());
}

#[cfg(windows)]
#[tokio::test]
async fn failed_upstream_websocket_connection_can_be_retried_without_a_stuck_attached_flag() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    // Reserve then release a port so the upstream connection is refused promptly.
    let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = unused.local_addr().unwrap();
    drop(unused);
    state.rmm.config.write().enabled = true;
    let lease = viewer("retry", true, "websocket");
    {
        let mut inner = state.rmm.inner.lock();
        inner.phase = Phase::Running;
        inner.process = Some(process(format!("http://{address}")));
        inner.leases.insert(lease.id.clone(), lease.clone());
    }
    let (origin, _server) = serve(crate::server::router(state.clone())).await;
    let url = format!("{}/rmm/retry/ws", origin.replacen("http:", "ws:", 1));
    for _ in 0..2 {
        let mut request = url.clone().into_client_request().unwrap();
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {TOKEN}").parse().unwrap(),
        );
        let result = tokio_tungstenite::connect_async(request).await;
        match result {
            Err(tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), StatusCode::BAD_GATEWAY)
            }
            other => panic!("Expected refused upstream HTTP502, got {other:?}"),
        }
        assert!(!lease.attached.load(Ordering::Acquire));
    }
    state.rmm.stop();
}

#[cfg(windows)]
#[tokio::test]
async fn confirmed_missing_rtc_peer_releases_control_without_ending_a_live_viewer() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    let deletes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let deleted = deletes.clone();
    let router = Router::new()
        .route(
            "/rtc",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers.get(header::AUTHORIZATION).unwrap(),
                    "Bearer fixture-password"
                );
                Json(json!({"peers":[{"id":"peer-live","state":"connected"}]}))
            }),
        )
        .route(
            "/rtc/peer-gone",
            axum::routing::delete(move || {
                let deleted = deleted.clone();
                async move {
                    deleted.fetch_add(1, Ordering::AcqRel);
                    StatusCode::NOT_FOUND
                }
            }),
        );
    let (origin, _server) = serve(router).await;
    state.rmm.config.write().enabled = true;
    let spectator = viewer("live-viewer", true, "webrtc");
    *spectator.rtc_id.lock() = Some("peer-live".into());
    let mut controller = viewer("lost-controller", false, "webrtc");
    {
        let controller = Arc::get_mut(&mut controller).unwrap();
        controller._control_lock = Some(package::lock(&state.rmm.root, "control.lock").unwrap());
        *controller.rtc_id.lock() = Some("peer-gone".into());
    }
    let cancelled = controller.cancelled.subscribe();
    {
        let mut inner = state.rmm.inner.lock();
        inner.phase = Phase::Running;
        inner.version = Some("test".into());
        inner.process = Some(process(origin));
        inner.leases.insert(controller.id.clone(), controller);
        inner.leases.insert(spectator.id.clone(), spectator.clone());
    }
    assert!(package::lock(&state.rmm.root, "control.lock").is_err());
    state.rmm.reap_closed_peers().await;
    assert!(*cancelled.borrow());
    assert_eq!(deletes.load(Ordering::Acquire), 1);
    assert!(package::lock(&state.rmm.root, "control.lock").is_ok());
    assert!(
        !state
            .rmm
            .inner
            .lock()
            .leases
            .contains_key("lost-controller")
    );
    assert!(state.rmm.inner.lock().leases.contains_key("live-viewer"));
    assert!(!*spectator.cancelled.borrow());
    assert!(state.rmm.inner.lock().process.is_some());
    state.rmm.stop();
}

#[cfg(windows)]
#[tokio::test]
async fn unavailable_or_malformed_rtc_inventory_is_not_evidence_that_the_controller_left() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    let reply = Arc::new(RwLock::new((StatusCode::INTERNAL_SERVER_ERROR, "{}")));
    let current_reply = reply.clone();
    let router = Router::new().route(
        "/rtc",
        get(move || {
            let current_reply = current_reply.clone();
            async move {
                let (code, body) = *current_reply.read();
                (code, [(header::CONTENT_TYPE, "application/json")], body)
            }
        }),
    );
    let (origin, _server) = serve(router).await;
    state.rmm.config.write().enabled = true;
    let mut controller = viewer("controller", false, "webrtc");
    {
        let controller = Arc::get_mut(&mut controller).unwrap();
        controller._control_lock = Some(package::lock(&state.rmm.root, "control.lock").unwrap());
        *controller.rtc_id.lock() = Some("peer-controller".into());
    }
    let cancelled = controller.cancelled.subscribe();
    {
        let mut inner = state.rmm.inner.lock();
        inner.phase = Phase::Running;
        inner.version = Some("test".into());
        inner.process = Some(process(origin));
        inner.leases.insert(controller.id.clone(), controller);
    }
    for (code, body) in [
        (StatusCode::INTERNAL_SERVER_ERROR, "{\"peers\":[]}"),
        (StatusCode::OK, "not JSON"),
        (StatusCode::OK, "{}"),
        (StatusCode::OK, "{\"peers\":null}"),
        (StatusCode::OK, "{\"peers\":[{}]}"),
        (StatusCode::OK, "{\"peers\":[{\"id\":42}]}"),
        (StatusCode::OK, "{\"peers\":[{\"id\":\"\"}]}"),
        (StatusCode::OK, "{\"peers\":[{\"id\":\"../invalid\"}]}"),
        (
            StatusCode::OK,
            "{\"peers\":[{\"id\":\"duplicate\"},{\"id\":\"duplicate\"}]}",
        ),
    ] {
        *reply.write() = (code, body);
        state.rmm.reap_closed_peers().await;
        assert!(
            state.rmm.inner.lock().leases.contains_key("controller"),
            "Inventory {code}: {body}"
        );
        assert!(!*cancelled.borrow(), "Inventory {code}: {body}");
        assert!(package::lock(&state.rmm.root, "control.lock").is_err());
        assert!(state.rmm.inner.lock().process.is_some());
    }
    state.rmm.stop();
}

#[tokio::test]
async fn real_heartbeat_cannot_extend_a_lease_past_its_absolute_reconnect_deadline() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path());
    state.rmm.config.write().enabled = true;
    let mut lease = viewer("capped-heartbeat", true, "webrtc");
    let absolute = Instant::now() + Duration::from_secs(30);
    {
        let lease = Arc::get_mut(&mut lease).unwrap();
        lease.reconnect_at = absolute;
        *lease.deadline.lock() = Instant::now() + Duration::from_secs(10);
    }
    state
        .rmm
        .inner
        .lock()
        .leases
        .insert(lease.id.clone(), lease.clone());
    let (origin, _server) = serve(crate::server::router(state.clone())).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for _ in 0..2 {
        let response = client
            .post(format!(
                "{origin}/api/rmm/sessions/capped-heartbeat/heartbeat"
            ))
            .bearer_auth(TOKEN)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(*lease.deadline.lock(), absolute);
        assert!(response.json::<Value>().await.unwrap()["reconnectAt"].is_string());
    }
    state.rmm.stop();
}
