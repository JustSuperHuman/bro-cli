//! The sign-in flow, storage, provider injection and the Claude Code launch, against a scripted local server.
use super::*;
use crate::Harness;
use crate::browser::BrowserMode;
use crate::launch::{LaunchCtx, LaunchSpec, Permission, UpstreamTarget, build, resolve_upstream};
use crate::util::test_env::sandbox;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

type Reply = (u16, Value);
type Handler = Box<dyn FnMut(&str, &str, Option<&str>, &Value) -> Reply + Send>;
type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Value)>>>;

/// A one-thread HTTP server: every request goes to `handler(method, path, bearer, json body)`.
struct Mock {
    origin: String,
    seen: Seen,
    stop: Arc<AtomicBool>,
}

impl Mock {
    fn start(mut handler: Handler) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::new(Mutex::new(vec![]));
        let stop = Arc::new(AtomicBool::new(false));
        let (log, halt) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            while !halt.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let (mut length, mut bearer) = (0usize, None);
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    if lower.starts_with("authorization:") {
                        bearer = line.split_once(':').map(|(_, v)| v.trim().trim_start_matches("Bearer ").to_string());
                    }
                }
                let mut raw = vec![0; length];
                let _ = reader.read_exact(&mut raw);
                let body: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
                log.lock().push((method.clone(), path.clone(), bearer.clone(), body.clone()));
                let (status, reply) = handler(&method, &path, bearer.as_deref(), &body);
                let text = reply.to_string();
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        Mock { origin, seen, stop }
    }

    fn paths(&self) -> Vec<String> {
        self.seen.lock().iter().map(|(m, p, _, _)| format!("{m} {p}")).collect()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn account_json() -> Value {
    json!({
        "user": { "id": "u1", "name": "Sam Rivera", "email": "sam@example.com", "image": null },
        "organization": { "id": "o1", "name": "Rivera Studio", "slug": "rivera-studio" },
        "credits": { "available": 1840, "granted": 1840, "monthlyAllowance": 0, "used": 0, "reserved": 0, "resetsAt": 1795000000000i64, "creditsPerUsd": 100 },
        "claude": {
            "baseUrl": "https://console.justsuperhuman.com/api/claude", "available": true, "reason": "",
            "models": [{ "id": "claude-opus-5-5", "name": "Opus 5.5" }, { "id": "claude-sonnet-5-5", "name": "Sonnet 5.5" }]
        },
        "host": { "url": "https://rivera-studio.justsuperhuman.com", "status": "ready" },
        "token": { "id": "t1", "name": "bro on JAMES-PC", "createdAt": 1795000000000i64 }
    })
}

fn account() -> Account {
    serde_json::from_value(account_json()).unwrap()
}

fn device_json() -> Value {
    json!({
        "device_code": "dev-secret", "user_code": "WDJB-MJHT",
        "verification_uri": "https://console.justsuperhuman.com/connect",
        "verification_uri_complete": "https://console.justsuperhuman.com/connect?code=WDJB-MJHT",
        "expires_in": 600, "interval": 1
    })
}

fn err(code: &str) -> Reply {
    (400, json!({ "error": code }))
}

fn sign_in_files(origin: &str) {
    save_token("jsh_user_secret", origin).unwrap();
    save_account(&account()).unwrap();
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn origin_defaults_to_production_and_honours_the_override() {
    let _sb = sandbox();
    assert_eq!(origin(), "https://console.justsuperhuman.com");
    unsafe { std::env::set_var("BRO_JSH_ORIGIN", "http://auth.localhost:8790/") };
    assert_eq!(origin(), "http://auth.localhost:8790");
}

#[test]
fn start_device_names_the_app_and_the_computer() {
    let _sb = sandbox();
    let mock = Mock::start(Box::new(|_, _, _, _| (200, device_json())));
    let start = start_device(&mock.origin).unwrap();
    assert_eq!((start.user_code.as_str(), start.interval, start.expires_in), ("WDJB-MJHT", 1, 600));
    let seen = mock.seen.lock();
    assert_eq!(seen[0].1, "/api/connect/v1/device");
    assert_eq!(seen[0].3["client"], "bro");
    assert_eq!(seen[0].3["version"], env!("CARGO_PKG_VERSION"));
    assert!(!seen[0].3["device"].as_str().unwrap().is_empty());
}

#[test]
fn start_device_failures_read_as_plain_english() {
    let _sb = sandbox();
    let mock = Mock::start(Box::new(|_, _, _, _| (503, json!({ "message": "Sign-in is paused for maintenance." }))));
    let e = start_device(&mock.origin).unwrap_err().to_string();
    assert!(e.contains("Sign-in is paused for maintenance."), "{e}");
    // nothing listening
    let e = start_device("http://127.0.0.1:9").unwrap_err().to_string();
    assert!(e.starts_with("Couldn't reach Just Superhuman"), "{e}");
}

#[test]
fn every_token_reply_maps_to_a_poll_state() {
    let _sb = sandbox();
    let queue = Arc::new(Mutex::new(vec![
        err("authorization_pending"),
        err("slow_down"),
        err("access_denied"),
        err("expired_token"),
        (200, json!({ "token": "jsh_user_abc", "token_type": "Bearer", "account": account_json() })),
    ]));
    let replies = queue.clone();
    let mock = Mock::start(Box::new(move |_, _, _, body| {
        assert_eq!(body["device_code"], "dev-secret");
        replies.lock().remove(0)
    }));
    let polled: Vec<Poll> = (0..5).map(|_| poll_token(&mock.origin, "dev-secret").unwrap()).collect();
    assert_eq!(&polled[..4], [Poll::Pending, Poll::SlowDown, Poll::Denied, Poll::Expired]);
    assert_eq!(polled[4], Poll::Done { token: "jsh_user_abc".into(), account: Box::new(account()) });
}

#[test]
fn an_unknown_token_error_is_an_error_not_a_state() {
    let _sb = sandbox();
    let mock = Mock::start(Box::new(|_, _, _, _| (500, json!({ "message": "database is down" }))));
    assert!(poll_token(&mock.origin, "x").unwrap_err().to_string().contains("database is down"));
}

/// Runs `drive` with scripted replies; returns the outcome and every wait it asked for.
fn run_drive(start: &DeviceStart, script: Vec<anyhow::Result<Poll>>, cancel_after: Option<usize>) -> (Outcome, Vec<u64>) {
    let mut script = script.into_iter();
    let mut waits = vec![];
    let polls = std::cell::Cell::new(0usize);
    let outcome = {
        let mut poll = || {
            polls.set(polls.get() + 1);
            script.next().unwrap_or(Ok(Poll::Pending))
        };
        let mut sleep = |d: Duration| waits.push(d.as_secs());
        drive(start, &mut poll, &mut sleep, &|| cancel_after.is_some_and(|n| polls.get() >= n))
    };
    (outcome, waits)
}

fn start_of(interval: u64, expires_in: u64) -> DeviceStart {
    DeviceStart { device_code: "d".into(), user_code: "AAAA-BBBB".into(), verification_uri: "u".into(), verification_uri_complete: "uc".into(), expires_in, interval }
}

#[test]
fn drive_waits_the_interval_and_adds_five_seconds_on_slow_down() {
    let done = Poll::Done { token: "jsh_user_t".into(), account: Box::new(account()) };
    let (outcome, waits) = run_drive(&start_of(3, 600), vec![Ok(Poll::Pending), Ok(Poll::SlowDown), Ok(Poll::Pending), Ok(done.clone())], None);
    assert_eq!(waits, [3, 3, 8, 8]);
    assert!(matches!(outcome, Outcome::Done { token, .. } if token == "jsh_user_t"));
    // an interval of zero is not a busy loop
    assert_eq!(run_drive(&start_of(0, 600), vec![Ok(Poll::Pending), Ok(done)], None).1, [1, 1]);
}

#[test]
fn drive_stops_on_denied_expired_and_cancel() {
    assert_eq!(run_drive(&start_of(3, 600), vec![Ok(Poll::Pending), Ok(Poll::Denied)], None).0, Outcome::Denied);
    assert_eq!(run_drive(&start_of(3, 600), vec![Ok(Poll::Expired)], None).0, Outcome::Expired);
    // never approved: gives up when the code's lifetime has been waited out
    let (outcome, waits) = run_drive(&start_of(5, 20), vec![], None);
    assert_eq!((outcome, waits.len()), (Outcome::Expired, 4));
    assert_eq!(run_drive(&start_of(3, 600), vec![], Some(2)).0, Outcome::Cancelled);
}

#[test]
fn drive_rides_out_a_flaky_connection_but_not_a_dead_one() {
    let done = Poll::Done { token: "jsh_user_t".into(), account: Box::new(account()) };
    let flaky = vec![Err(anyhow!("offline")), Err(anyhow!("offline")), Ok(Poll::Pending), Err(anyhow!("offline")), Ok(done)];
    assert!(matches!(run_drive(&start_of(1, 600), flaky, None).0, Outcome::Done { .. }));
    let dead: Vec<_> = (0..MAX_POLL_FAILURES).map(|_| Err(anyhow!("offline"))).collect();
    assert!(matches!(run_drive(&start_of(1, 600), dead, None).0, Outcome::Failed(m) if m.contains("offline")));
}

#[test]
fn fetch_account_sends_the_bearer_and_401_means_revoked() {
    let _sb = sandbox();
    let mock = Mock::start(Box::new(|_, _, bearer, _| match bearer {
        Some("good") => (200, account_json()),
        _ => (401, json!({ "error": "revoked" })),
    }));
    assert_eq!(fetch_account(&mock.origin, "good").unwrap(), Fetched::Account(Box::new(account())));
    assert_eq!(fetch_account(&mock.origin, "bad").unwrap(), Fetched::Revoked);
    let a = account();
    assert_eq!((a.credits.available, a.credits.granted, a.credits.resets_at), (1840, 1840, Some(1795000000000)));
    assert_eq!(a.summary(), "1,840 credits · Rivera Studio");
}

#[test]
fn credits_formatting() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1000), "1,000");
    assert_eq!(grouped(1234567), "1,234,567");
    assert_eq!(grouped(-2500), "-2,500");
    let mut a = account();
    a.credits.available = 1;
    assert_eq!(a.credits_text(), "1 credit");
}

#[test]
fn the_token_gets_its_own_file_and_never_reaches_the_config_or_the_account_cache() {
    let sb = sandbox();
    let mut cfg = crate::config::Config::default();
    cfg.set_key("zai", Some("z-key"));
    cfg.save().unwrap();
    sign_in_files("https://console.justsuperhuman.com");
    let stored = stored().unwrap();
    assert_eq!((stored.token.as_str(), stored.origin.as_str()), ("jsh_user_secret", "https://console.justsuperhuman.com"));
    assert_eq!(token_path(), sb.bro().join("jsh").join("token.json"));
    for file in [crate::paths::config_path(), account_path()] {
        assert!(!std::fs::read_to_string(&file).unwrap().contains("jsh_user_secret"), "{}", file.display());
    }
    // key_for answers from the token file
    assert_eq!(crate::config::Config::load().unwrap().key_for(PROVIDER_ID, None).as_deref(), Some("jsh_user_secret"));
    assert_eq!(cached_account().unwrap().0, account());
}

#[test]
fn signed_in_adds_the_provider_and_signed_out_removes_it() {
    let _sb = sandbox();
    let cfg = crate::config::Config::default();
    assert!(crate::providers::find(&crate::providers::load(&cfg), PROVIDER_ID).is_none());
    assert_eq!(cfg.key_for(PROVIDER_ID, None), None);
    sign_in_files(PRODUCTION_ORIGIN);
    let all = crate::providers::load(&cfg);
    let p = crate::providers::find(&all, PROVIDER_ID).unwrap();
    assert_eq!((p.name.as_str(), p.mode), ("Just Superhuman", ProviderMode::Anthropic));
    assert_eq!(p.base_url.as_deref(), Some("https://console.justsuperhuman.com/api/claude"));
    assert_eq!(p.model_ids(), ["claude-opus-5-5", "claude-sonnet-5-5"]);
    assert_eq!(p.model("claude-sonnet-5-5").unwrap().name.as_deref(), Some("Sonnet 5.5"));
    assert_eq!(default_model().as_deref(), Some("claude-opus-5-5"));
    assert_eq!(cfg.key_for(PROVIDER_ID, None).as_deref(), Some("jsh_user_secret"));
    forget();
    assert!(crate::providers::find(&crate::providers::load(&cfg), PROVIDER_ID).is_none());
}

#[test]
fn a_token_without_a_cached_account_still_gives_a_working_provider() {
    let _sb = sandbox();
    save_token("jsh_user_x", "http://auth.localhost:8790/").unwrap();
    let p = provider().unwrap();
    assert_eq!(p.base_url.as_deref(), Some("http://auth.localhost:8790/api/claude"));
    assert!(p.models.is_empty());
}

fn spec(harness: Harness, model: Option<&str>) -> LaunchSpec {
    LaunchSpec {
        harness,
        profile_id: None,
        provider_id: Some(PROVIDER_ID.into()),
        model: model.map(str::to_string),
        cwd: std::path::PathBuf::from("/work"),
        resume: None,
        permission: Permission::Default,
        browser: BrowserMode::Off,
        extra_args: vec![],
        orchestrator: false,
    }
}

fn env<'a>(c: &'a crate::launch::CommandSpec, k: &str) -> Option<&'a str> {
    c.env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
}

#[test]
fn claude_code_runs_on_the_account_with_no_login_in_the_way() {
    let _sb = sandbox();
    let launch = || build(&spec(Harness::Claude, Some("claude-opus-5-5")), &LaunchCtx::default());
    assert!(format!("{:#}", launch().unwrap_err()).contains("unknown provider"), "signed out");
    sign_in_files(PRODUCTION_ORIGIN);
    // a Claude login's token in the shell must not ride along
    unsafe { std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat-shell") };
    let c = launch().unwrap();
    assert_eq!(env(&c, "ANTHROPIC_BASE_URL"), Some("https://console.justsuperhuman.com/api/claude"));
    assert_eq!(env(&c, "ANTHROPIC_AUTH_TOKEN"), Some("jsh_user_secret"));
    assert_eq!(env(&c, "ANTHROPIC_API_KEY"), Some(""));
    assert_eq!(env(&c, "ANTHROPIC_MODEL"), Some("claude-opus-5-5"));
    assert!(env(&c, "_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL").is_none());
    for var in ["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_REFRESH_TOKEN", "CLAUDE_CONFIG_DIR"] {
        assert!(c.env_remove.iter().any(|r| r == var), "{var} must be removed");
        assert!(env(&c, var).is_none());
    }
    assert!(c.route.is_none(), "an Anthropic provider goes direct");
    assert!(c.args.windows(2).any(|w| w == ["--model", "claude-opus-5-5"]));
    assert_eq!(c.label, "claude · justsuperhuman · claude-opus-5-5");
    unsafe { std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN") };
}

#[test]
fn on_a_claude_profile_the_account_token_still_wins() {
    let _sb = sandbox();
    let dir = crate::paths::claude_accounts_dir().join("work");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"profile-oauth"}}"#).unwrap();
    sign_in_files(PRODUCTION_ORIGIN);
    let mut s = spec(Harness::Claude, None);
    s.profile_id = Some("claude:work".into());
    let c = build(&s, &LaunchCtx::default()).unwrap();
    assert_eq!(env(&c, "ANTHROPIC_AUTH_TOKEN"), Some("jsh_user_secret"));
    assert!(!format!("{c:?}").contains("profile-oauth"));
}

#[test]
fn codex_and_pi_reach_the_account_through_the_proxy() {
    let _sb = sandbox();
    sign_in_files(PRODUCTION_ORIGIN);
    let ctx = LaunchCtx { proxy_base: Some("http://127.0.0.1:3458".into()), proxy_token: Some("tok".into()), ..Default::default() };
    // Codex has no Anthropic endpoint of its own: bro-proxy translates, signed in with the token
    let c = build(&spec(Harness::Codex, Some("claude-sonnet-5-5")), &ctx).unwrap();
    let route = c.route.as_ref().unwrap_or_else(|| panic!("codex goes through bro-proxy: {c:?}"));
    assert_eq!(route.upstream, PROVIDER_ID);
    let target = resolve_upstream(&route.upstream, route.model.as_deref(), &crate::config::Config::default()).unwrap();
    assert_eq!(
        target,
        UpstreamTarget::Anthropic { base_url: "https://console.justsuperhuman.com/api/claude".into(), api_key: Some("jsh_user_secret".into()), bearer: true }
    );
    assert!(!format!("{:?}", c.args).contains("jsh_user_secret"));
    // Pi speaks Anthropic Messages itself: its own provider entry, the key in its environment, not its arguments
    let pi = build(&spec(Harness::Pi, Some("claude-sonnet-5-5")), &ctx).unwrap();
    assert_eq!(env(&pi, "BRO_PI_API_KEY"), Some("jsh_user_secret"));
    assert!(!format!("{:?}", pi.args).contains("jsh_user_secret"));
    assert!(pi.args.iter().any(|a| a.contains("justsuperhuman")));
}

#[test]
fn a_full_sign_in_stores_the_account_and_a_second_start_returns_the_waiting_one() {
    let _sb = sandbox();
    let approved = Arc::new(AtomicBool::new(false));
    let flag = approved.clone();
    let mock = Mock::start(Box::new(move |_, path, _, _| match path {
        "/api/connect/v1/device" => (200, device_json()),
        "/api/connect/v1/token" if flag.load(Ordering::Acquire) => (200, json!({ "token": "jsh_user_flow", "token_type": "Bearer", "account": account_json() })),
        "/api/connect/v1/token" => err("authorization_pending"),
        "/api/connect/v1/revoke" => (200, json!({ "ok": true })),
        _ => (404, json!({})),
    }));
    let changes = Arc::new(Mutex::new(0u32));
    let counter = changes.clone();
    let me = login();
    me.on_change(move || *counter.lock() += 1);
    let (pending, fresh) = me.begin_at(&mock.origin).unwrap();
    assert!(fresh);
    assert_eq!((pending.user_code.as_str(), pending.verification_uri_complete.as_str()), ("WDJB-MJHT", "https://console.justsuperhuman.com/connect?code=WDJB-MJHT"));
    let (again, fresh) = me.begin_at(&mock.origin).unwrap();
    assert!(!fresh && again == pending);
    assert_eq!(mock.paths().iter().filter(|p| p.ends_with("/device")).count(), 1);
    let snap = me.snapshot();
    assert!(!snap.signed_in && snap.pending.is_some());
    approved.store(true, Ordering::Release);
    wait_until("the sign-in", || me.snapshot().signed_in);
    let snap = me.snapshot();
    assert!(snap.pending.is_none() && snap.error.is_none(), "{snap:?}");
    assert_eq!(snap.account.unwrap().user.email, "sam@example.com");
    let saved = stored().unwrap();
    assert_eq!((saved.token.as_str(), saved.origin.as_str()), ("jsh_user_flow", mock.origin.as_str()));
    assert!(*changes.lock() >= 2);

    // sign out revokes with the token, at the origin that issued it, and forgets everything
    assert_eq!(me.sign_out(), SignedOut { revoked: true, was_signed_in: true });
    assert!(mock.seen.lock().iter().any(|(_, p, bearer, _)| p.ends_with("/revoke") && bearer.as_deref() == Some("jsh_user_flow")));
    assert!(!signed_in() && cached_account().is_none());
    assert!(crate::providers::find(&crate::providers::load(&crate::config::Config::default()), PROVIDER_ID).is_none());
    assert_eq!(me.sign_out(), SignedOut { revoked: true, was_signed_in: false });
}

#[test]
fn a_declined_sign_in_says_so_and_can_start_again() {
    let _sb = sandbox();
    let mock = Mock::start(Box::new(|_, path, _, _| if path.ends_with("/device") { (200, device_json()) } else { err("access_denied") }));
    let me = login();
    me.begin_at(&mock.origin).unwrap();
    wait_until("the decline", || me.snapshot().pending.is_none());
    let snap = me.snapshot();
    assert!(!snap.signed_in);
    assert!(snap.error.unwrap().contains("declined"));
    assert!(me.begin_at(&mock.origin).unwrap().1, "a fresh code, not the dead one");
    me.cancel();
    assert!(me.snapshot().pending.is_none());
}

#[test]
fn a_revoked_device_signs_out_locally_and_says_why() {
    let _sb = sandbox();
    let mock = Mock::start(Box::new(|_, _, _, _| (401, json!({ "error": "revoked" }))));
    sign_in_files(&mock.origin);
    let me = login();
    me.refresh().unwrap();
    let snap = me.snapshot();
    assert!(!snap.signed_in && snap.account.is_none());
    assert!(snap.error.unwrap().contains("revoked"));
    assert!(crate::providers::find(&crate::providers::load(&crate::config::Config::default()), PROVIDER_ID).is_none());
}

#[test]
fn a_refresh_updates_the_credits_and_a_failed_one_keeps_the_last_reading() {
    let _sb = sandbox();
    let up = Arc::new(AtomicBool::new(true));
    let flag = up.clone();
    let mock = Mock::start(Box::new(move |_, _, _, _| {
        if !flag.load(Ordering::Acquire) {
            return (500, json!({ "message": "try later" }));
        }
        let mut a = account_json();
        a["credits"]["available"] = json!(1200);
        (200, a)
    }));
    sign_in_files(&mock.origin);
    let me = login();
    me.refresh().unwrap();
    assert_eq!(me.snapshot().account.unwrap().credits.available, 1200);
    up.store(false, Ordering::Release);
    assert!(me.refresh().is_err());
    let snap = me.snapshot();
    assert!(snap.signed_in, "a server hiccup is not a sign-out");
    assert_eq!(snap.account.unwrap().credits.available, 1200);
    assert!(snap.error.unwrap().contains("try later"));
    // a fresh reading isn't asked for again
    let before = mock.paths().len();
    me.refresh_if_stale(Duration::from_secs(3600));
    assert_eq!(mock.paths().len(), before);
}

#[test]
fn sign_out_without_a_server_still_forgets_the_token() {
    let _sb = sandbox();
    sign_in_files("http://127.0.0.1:9");
    let out = login().sign_out();
    assert_eq!(out, SignedOut { revoked: false, was_signed_in: true });
    assert!(!signed_in());
    assert!(login().snapshot().error.unwrap().contains("revoke"));
}

#[test]
#[ignore = "network: needs the accounts server at BRO_JSH_ORIGIN"]
fn the_real_server_starts_a_device_flow() {
    let start = start_device(&origin()).unwrap();
    assert!(!start.user_code.is_empty());
}
