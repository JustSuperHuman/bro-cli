//! Outbound, account-authenticated work delivery. No listener or bridge token is exposed to the cloud.
//!
//! The worker asks the account server for work and is kept waiting until some arrives, so a request made on
//! another computer starts here a round trip later. A server that answers at once instead is asked on a clock
//! that speeds up while this computer is being used and relaxes when it isn't ([`gap`]). Reads (a list, a
//! session's screen or chat) run side by side and never queue behind a slow start; what changes a session runs
//! in the order it was asked, one at a time per session ([`Order`]). When the server can't be reached the worker
//! stays away longer each time, with some chance in the wait ([`jc::retry_after`]), and says where it stands
//! ([`link`]) so the app can show it.
//!
//! The same asks carry one more piece of news: someone signed in to the account wants to open this computer from
//! their phone. The worker then dials the account relay's tunnel ([`crate::relay`]) and keeps it while it is used.
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bro_core::{jsh, justconnect as jc};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, oneshot, watch};

/// The tools another computer may run here.
const TOOLS: [&str; 9] = [
    "list_sessions",
    "list_projects",
    "open_session",
    "read_session",
    "read_chat",
    "send_input",
    "send_keys",
    "answer_prompt",
    "close_session",
];
/// Jobs run at once.
const PARALLEL: usize = 4;
/// Between asks when the server answers at once and nothing has been asked of this computer lately.
const IDLE: Duration = Duration::from_secs(5);
/// A request the server doesn't hold: the round trip and its own bookkeeping.
const QUICK: Duration = Duration::from_secs(20);

fn allowed(tool: &str) -> bool {
    TOOLS.contains(&tool)
}

fn arguments_for(tool: &str, input: &Value) -> Value {
    let mut args = input.clone();
    if tool == "open_session" && args.get("permission").is_none_or(Value::is_null) {
        args["permission"] = json!("default");
    }
    args
}

/// How long to wait before asking again a server that answered at once with nothing, `since_work` after the
/// last job arrived. Someone following a session from another computer sends a few requests a second; a five
/// second wait between each makes their page feel dead. An idle computer goes back to a slow clock.
fn gap(since_work: Option<Duration>) -> Duration {
    match since_work {
        Some(d) if d < Duration::from_secs(10) => Duration::from_millis(250),
        Some(d) if d < Duration::from_secs(60) => Duration::from_secs(1),
        Some(d) if d < Duration::from_secs(300) => Duration::from_secs(2),
        _ => IDLE,
    }
}

/// The session a tool changes: its jobs run one at a time, in the order they were asked. None for tools that
/// only read, and for a new session (nothing to be in order with yet).
fn lane(tool: &str, args: &Value) -> Option<String> {
    matches!(tool, "send_input" | "send_keys" | "answer_prompt" | "close_session")
        .then(|| args["sessionId"].as_str().unwrap_or_default().trim().to_lowercase())
}

/// Keeps what changes a session in the order it was asked while everything else runs side by side.
#[derive(Default)]
struct Order {
    /// Per session: closes when the last job queued for it has finished.
    tails: HashMap<String, oneshot::Receiver<()>>,
}

/// A job's place in its session's order: what to wait for before running, and what to let go of when done.
#[derive(Default)]
struct Turn {
    before: Option<oneshot::Receiver<()>>,
    done: Option<oneshot::Sender<()>>,
}

impl Order {
    fn join(&mut self, tool: &str, args: &Value) -> Turn {
        // sessions whose last job has finished need no entry
        self.tails.retain(|_, done| matches!(done.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
        let Some(key) = lane(tool, args) else {
            return Turn::default();
        };
        let (done, finished) = oneshot::channel();
        Turn { before: self.tails.insert(key, finished), done: Some(done) }
    }
}

/// Where the worker stands with the account server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkPhase {
    /// Not asked to: remote access is off here, or nobody is signed in.
    #[default]
    Off,
    /// The first ask is on its way.
    Connecting,
    /// The server answers.
    Connected,
    /// The server stopped answering; the worker is trying again.
    Reconnecting,
    /// The server answers, and refuses this sign-in for this computer.
    Refused,
}

/// The worker's connection to the account server, for the app to show.
#[derive(Clone, Copy, Debug, Default)]
pub struct Link {
    pub phase: LinkPhase,
    /// When it entered this phase.
    pub since: Option<Instant>,
    /// The server holds the worker's requests until there is work: what's asked of this computer starts at once.
    /// False: it is asked on a clock (every few seconds, faster while in use).
    pub held: bool,
    /// When the next try is due, while reconnecting.
    pub retry_at: Option<Instant>,
    /// Jobs running here now.
    pub running: usize,
    /// The account relay's tunnel is open: someone is reaching this computer through the account right now, or
    /// was a moment ago.
    pub tunnel: bool,
}

static LINK: Mutex<Link> = Mutex::new(Link { phase: LinkPhase::Off, since: None, held: false, retry_at: None, running: 0, tunnel: false });

/// Where this process's worker stands with the account server.
pub fn link() -> Link {
    *LINK.lock()
}

fn set_phase(phase: LinkPhase, retry_at: Option<Instant>) {
    let mut link = LINK.lock();
    if link.phase != phase {
        link.phase = phase;
        link.since = Some(Instant::now());
    }
    link.retry_at = retry_at;
}

/// Why an ask came back with nothing to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failed {
    /// The server answered with this error status.
    Refused(u16),
    /// No answer: offline, a restart, a timeout.
    NoAnswer,
}

async fn ask(http: &reqwest::Client, stored: &jsh::Stored, route: &str, body: &Value, limit: Duration) -> Result<Value, Failed> {
    let response = http
        .post(jc::endpoint(stored, route))
        .bearer_auth(&stored.token)
        .timeout(limit)
        .json(body)
        .send()
        .await
        .map_err(|_| Failed::NoAnswer)?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        // a proxy's own 502/504 page is the server not answering, not the server refusing
        return Err(if matches!(status, 502..=504) { Failed::NoAnswer } else { Failed::Refused(status) });
    }
    response.json().await.map_err(|_| Failed::NoAnswer)
}

/// Sleeps `rest`; false when bro is shutting down.
async fn pause(shutdown: &mut watch::Receiver<bool>, rest: Duration) -> bool {
    if *shutdown.borrow() {
        return false;
    }
    tokio::select! {
        changed = shutdown.changed() => changed.is_ok() && !*shutdown.borrow(),
        _ = tokio::time::sleep(rest) => true,
    }
}

/// Who the worker is: this computer's enrollment and sign-in, read afresh each time they matter (they change
/// under a running Bro: `bro computers disable`, a sign-out). Bro reads the files under `~/.bro/jsh`; a test
/// hands the worker its own.
#[derive(Clone)]
pub(crate) struct Identity {
    pub config: Arc<dyn Fn() -> jc::Connection + Send + Sync>,
    pub sign_in: Arc<dyn Fn() -> Option<jsh::Stored> + Send + Sync>,
    /// Where the one-worker-per-user lock lives.
    pub lock_dir: std::path::PathBuf,
}

impl Identity {
    fn disk() -> Identity {
        Identity { config: Arc::new(jc::load), sign_in: Arc::new(jsh::stored), lock_dir: bro_core::paths::bro_dir().join("jsh") }
    }
}

/// Returns once remote access is switched off here or the sign-in is no longer `stored`'s: a request the
/// server is holding for this computer is hung up on, so it can't hand over work nobody will run.
async fn withdrawn(who: &Identity, computer: &str, stored: &jsh::Stored) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let c = (who.config)();
        if !c.enabled || c.computer_id != computer || (who.sign_in)().as_ref() != Some(stored) {
            return;
        }
    }
}

/// One job, from its turn to its result on the server.
#[allow(clippy::too_many_arguments)]
async fn run(state: AppState, http: reqwest::Client, stored: jsh::Stored, computer: String, id: String, lease: String, tool: String, arguments: Value, turn: Turn) {
    if let Some(before) = turn.before {
        let _ = before.await;
    }
    LINK.lock().running += 1;
    let result = if allowed(&tool) {
        crate::orchestrator::tools::execute(&state, &tool, &arguments_for(&tool, &arguments)).await
    } else {
        Err("Unsupported remote Bro tool.".into())
    };
    {
        let mut link = LINK.lock();
        link.running = link.running.saturating_sub(1);
    }
    // the next job for this session may start; the upload doesn't hold it up
    drop(turn.done);
    let ok = result.is_ok();
    let mut output = result.unwrap_or_else(|e| e);
    if output.len() > 262144 {
        let mut end = 262144;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
    }
    let route = format!("/computers/{computer}/results/{id}");
    let body = json!({"lease":lease,"ok":ok,"output":output});
    // Repeating a result upload cannot execute work again. The server never re-leases a claimed job. Any answer
    // settles it (stored, or the lease no longer stands); only silence is worth another try.
    for attempt in 0..4 {
        match ask(&http, &stored, &route, &body, QUICK).await {
            Ok(_) | Err(Failed::Refused(_)) => break,
            Err(Failed::NoAnswer) => tokio::time::sleep(jc::retry_after(attempt, jc::roll()).min(Duration::from_secs(8))).await,
        }
    }
}

/// The account relay's tunnel, from the dial to its end: open while phones use it, closed by the server when
/// they stop, and hung up here the moment remote access is switched off, the sign-in changes or Bro shuts down.
pub(crate) async fn carry(door: crate::relay::Door, who: Identity, computer: String, stored: jsh::Stored, mut shutdown: watch::Receiver<bool>) {
    LINK.lock().tunnel = true;
    let stop = async {
        tokio::select! {
            _ = withdrawn(&who, &computer, &stored) => {}
            _ = shutdown.wait_for(|stopped| *stopped) => {}
        }
    };
    let ended = crate::relay::run(&door, &stored, &computer, stop).await;
    LINK.lock().tunnel = false;
    if ended != crate::relay::Ended::Closed {
        tracing::info!("bro-bridge: the account relay did not open: {ended:?}");
    }
}

/// Dials the tunnel when the server's answer asks for it and none is open.
fn dial_if_asked(answer: &Value, tunnel: &mut Option<tokio::task::JoinHandle<()>>, dial: impl FnOnce() -> tokio::task::JoinHandle<()>) {
    if answer["relay"] == true && tunnel.as_ref().is_none_or(|task| task.is_finished()) {
        *tunnel = Some(dial());
    }
}

pub(crate) fn spawn(state: &AppState) {
    // A test's bridge is never this computer's worker: it would read the real enrollment and take real work.
    // Tests that mean to run one call `work` with an identity of their own.
    if cfg!(test) {
        return;
    }
    tokio::spawn(work(state.clone(), Identity::disk()));
}

/// The worker: asks for work as `who`, runs it, uploads the results, until Bro shuts down.
pub(crate) async fn work(state: AppState, who: Identity) {
    {
        let mut shutdown = state.shutdown.subscribe();
        // one client for the worker's life: every ask rides the same connection
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_idle_timeout(Duration::from_secs(50)).build().unwrap_or_default();
        let slots = Arc::new(Semaphore::new(PARALLEL));
        let mut order = Order::default();
        // where the account relay's streams are served, and the tunnel that brings them (dialed when asked for)
        let door = crate::relay::open_door(&state);
        let mut tunnel: Option<tokio::task::JoinHandle<()>> = None;
        // Only one bridge per OS user drains the queue; a file lock is released on crashes. Held while this one
        // is the worker, so jobs are never split between two.
        let mut lock: Option<std::fs::File> = None;
        let mut failures = 0u32;
        let mut last_work: Option<Instant> = None;
        let mut rest = Duration::from_secs(1);
        loop {
            if !pause(&mut shutdown, rest).await {
                break;
            }
            rest = IDLE;
            let c = (who.config)();
            // the Bro service answers for this computer (`bro agent`): a Bro someone opened here leaves it to it
            let stored = (who.sign_in)().filter(|s| c.enabled && jc::valid_id(&c.computer_id) && c.origin == s.origin && !jc::agent::owns(&c.computer_id));
            let Some(stored) = stored else {
                lock = None;
                failures = 0;
                set_phase(LinkPhase::Off, None);
                continue;
            };
            if lock.is_none() {
                lock = bro_core::rmm::lock(&who.lock_dir, "justconnect-worker").ok();
            }
            if lock.is_none() {
                // another Bro of this user is the worker
                set_phase(LinkPhase::Off, None);
                continue;
            }
            if link().phase == LinkPhase::Off {
                set_phase(LinkPhase::Connecting, None);
            }
            let route = format!("/computers/{}/poll", c.computer_id);
            let version = env!("CARGO_PKG_VERSION");
            let dial = || tokio::spawn(carry(door.clone(), who.clone(), c.computer_id.clone(), stored.clone(), state.shutdown.subscribe()));
            // No room for another job: say this computer is alive and busy, then wait for one to finish.
            let Ok(slot) = slots.clone().try_acquire_owned() else {
                // busy with jobs is no reason to keep a phone waiting: the tunnel is dialed all the same
                if let Ok(answer) = ask(&http, &stored, &route, &json!({"busy":true,"version":version,"capabilities":{"relay":true}}), QUICK).await {
                    dial_if_asked(&answer, &mut tunnel, dial);
                }
                tokio::select! {
                    _ = shutdown.changed() => {}
                    _ = slots.clone().acquire_owned() => {}
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
                rest = Duration::ZERO;
                continue;
            };
            let asked = Instant::now();
            let body = json!({"version":version,"wait":jc::WAIT_SECS,"capabilities":{"tools":TOOLS,"parallel":PARALLEL,"relay":true}});
            let polled = tokio::select! {
                _ = shutdown.changed() => break,
                // switched off or signed out while the server held the request: hang up, the top of the loop reads why
                _ = withdrawn(&who, &c.computer_id, &stored) => { rest = Duration::ZERO; continue }
                answer = ask(&http, &stored, &route, &body, Duration::from_secs(jc::WAIT_SECS + 15)) => answer,
            };
            let answer = match polled {
                Ok(answer) => answer,
                // this sign-in isn't this computer's (enrolled under another, or access removed on the account):
                // asking faster won't change the answer
                Err(Failed::Refused(401 | 403)) => {
                    rest = Duration::from_secs(60);
                    set_phase(LinkPhase::Refused, Some(Instant::now() + rest));
                    continue;
                }
                Err(_) => {
                    rest = jc::retry_after(failures, jc::roll());
                    failures += 1;
                    set_phase(LinkPhase::Reconnecting, Some(Instant::now() + rest));
                    continue;
                }
            };
            failures = 0;
            // the server says when it holds requests; one that kept this one a while plainly does
            let held = answer["waited"] == true || asked.elapsed() >= Duration::from_secs(2);
            set_phase(LinkPhase::Connected, None);
            LINK.lock().held = held;
            dial_if_asked(&answer, &mut tunnel, dial);
            let job = &answer["job"];
            let claimed = job["id"].as_str().filter(|id| jc::valid_id(id)).zip(job["lease"].as_str());
            let Some((id, lease)) = claimed else {
                rest = if held { Duration::from_millis(50) } else { gap(last_work.map(|at| at.elapsed())) };
                continue;
            };
            // Re-check the local switch and sign-in after the network wait.
            if !(who.config)().enabled || (who.sign_in)().as_ref() != Some(&stored) {
                continue;
            }
            last_work = Some(Instant::now());
            // more may be waiting behind it
            rest = Duration::ZERO;
            let tool = job["tool"].as_str().unwrap_or_default().to_string();
            let arguments = job["arguments"].clone();
            let turn = order.join(&tool, &arguments);
            let job = run(state.clone(), http.clone(), stored.clone(), c.computer_id.clone(), id.to_string(), lease.to_string(), tool, arguments, turn);
            tokio::spawn(async move {
                job.await;
                drop(slot);
            });
        }
        set_phase(LinkPhase::Off, None);
    }
}

pub(crate) async fn status() -> Json<Value> {
    let c = jc::load();
    let link = link();
    Json(json!({
        "enabled": c.enabled,
        "computerId": c.computer_id,
        "signedIn": jsh::signed_in(),
        "selected": jc::selected(),
        "link": { "phase": link.phase, "held": link.held, "running": link.running, "tunnel": link.tunnel },
    }))
}

pub(crate) async fn action(
    State(_state): State<AppState>,
    Path(action): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || match action.as_str() {
        "enable" => jc::enable(body["name"].as_str()),
        "disable" => jc::disable(),
        "select" => jc::select(body["computerId"].as_str().unwrap_or("local")),
        "list" => jc::call("GET", "/computers", json!({})),
        _ => anyhow::bail!("Unknown JustConnect action."),
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"JustConnect could not finish that action."})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_launches_keep_normal_approval_prompts_unless_explicitly_selected() {
        let input = json!({"project":"C:/work","agent":"jsh:claude"});
        assert_eq!(arguments_for("open_session", &input)["permission"], "default");
        assert!(input.get("permission").is_none());
        assert_eq!(arguments_for("open_session", &json!({"permission":"auto"}))["permission"], "auto");
        assert_eq!(arguments_for("list_sessions", &json!({})), json!({}));
        assert!(allowed("send_keys"));
        assert!(allowed("answer_prompt"));
    }

    #[tokio::test]
    async fn the_tunnel_is_dialed_when_the_server_asks_and_only_one_at_a_time() {
        let dials = std::sync::atomic::AtomicUsize::new(0);
        let dial = || {
            dials.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(std::future::pending::<()>())
        };
        let mut tunnel = None;
        dial_if_asked(&json!({"job":null}), &mut tunnel, dial);
        assert!(tunnel.is_none(), "nobody asked");
        dial_if_asked(&json!({"job":null,"relay":true}), &mut tunnel, dial);
        dial_if_asked(&json!({"job":null,"relay":true}), &mut tunnel, dial);
        assert_eq!(dials.load(std::sync::atomic::Ordering::SeqCst), 1, "one is already open");
        // it closed (idle, or the line dropped): the next ask dials again
        let open = tunnel.take().unwrap();
        open.abort();
        let _ = (&mut tunnel.insert(open)).await;
        dial_if_asked(&json!({"relay":true}), &mut tunnel, dial);
        assert_eq!(dials.load(std::sync::atomic::Ordering::SeqCst), 2);
        tunnel.unwrap().abort();
    }

    #[test]
    fn cloud_jobs_cannot_call_configuration_tools() {
        assert!(allowed("open_session"));
        for name in ["justconnect_enable", "list_computers", "set_settings", "run_shell", ""] {
            assert!(!allowed(name));
        }
    }

    #[test]
    fn the_clock_is_quick_while_this_computer_is_in_use_and_slow_when_it_is_not() {
        assert_eq!(gap(None), IDLE, "nothing asked yet");
        assert_eq!(gap(Some(Duration::from_secs(1))), Duration::from_millis(250));
        assert_eq!(gap(Some(Duration::from_secs(30))), Duration::from_secs(1));
        assert_eq!(gap(Some(Duration::from_secs(120))), Duration::from_secs(2));
        assert_eq!(gap(Some(Duration::from_secs(900))), IDLE);
    }

    #[test]
    fn only_what_changes_a_session_takes_a_turn() {
        let session = json!({"sessionId":"Mal"});
        for tool in ["send_input", "send_keys", "answer_prompt", "close_session"] {
            assert_eq!(lane(tool, &session).as_deref(), Some("mal"), "{tool}");
        }
        for tool in ["list_sessions", "list_projects", "read_session", "read_chat", "open_session"] {
            assert_eq!(lane(tool, &session), None, "{tool}");
        }
    }

    #[tokio::test]
    async fn a_sessions_changes_run_in_the_order_they_were_asked() {
        let mut order = Order::default();
        let first = order.join("send_input", &json!({"sessionId":"Mal"}));
        let second = order.join("send_keys", &json!({"sessionId":"mal"}));
        let other = order.join("send_input", &json!({"sessionId":"Hugh"}));
        let read = order.join("read_chat", &json!({"sessionId":"Mal"}));
        assert!(first.before.is_none(), "nothing ahead of the first");
        assert!(other.before.is_none(), "another session doesn't wait for this one");
        assert!(read.before.is_none() && read.done.is_none(), "a read never waits");
        let mut waiting = second.before.expect("the second waits for the first");
        assert!(matches!(waiting.try_recv(), Err(oneshot::error::TryRecvError::Empty)), "still waiting while the first runs");
        drop(first.done);
        assert!(waiting.await.is_err(), "released when the first finishes");
        // a finished session's entry goes; one still running stays
        drop(second.done);
        let _later = order.join("list_sessions", &json!({}));
        assert_eq!(order.tails.len(), 1);
        assert!(order.tails.contains_key("hugh"));
    }
}
