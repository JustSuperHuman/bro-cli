//! Jobs for the Claude Code mod running in a session (bro's mod, `crates/bro-core/claude-mod`, its `apps`
//! part): MCP work only Claude Code can do, because it needs Claude Code's own connections (claude.ai
//! connectors, OAuth, stdio servers). bro asks with [`crate::Bridge::mod_ask`]; the session's mod asks the
//! bridge for work and answers each job.
//!
//! * `GET /api/mod/{session}/jobs` waits up to [`Timing::poll`] (25 s) for work and answers
//!   `{ "jobs": [{ "id": "...", ...the job's fields }] }` with every job queued for the session, now in flight,
//!   or `{ "jobs": [] }`. A job that isn't a JSON object arrives as `{ "id", "job" }`.
//! * `POST /api/mod/{session}/jobs/{id}` with `{ "ok": true, "result": ... }` or `{ "ok": false, "error": "..." }`
//!   answers one; 404 when the session has no such job (never asked, already answered, or timed out). The body
//!   may be up to [`MAX_ANSWER_BYTES`]: a widget's HTML runs to several MB.
//!
//! Only callers on this machine get in, token or not: the mod runs here. A job nobody takes within
//! [`Timing::pickup`] (10 s) fails, because the session's mod isn't there to take it; one taken but not answered
//! within [`Timing::answer`] (90 s) times out. Every reply runs exactly once: answered, failed, or failed when
//! the bridge stops.

use crate::rest::api_error;
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::ExtensionRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// What bro does with the mod's answer, or with why there is none.
pub(crate) type Reply = Box<dyn FnOnce(Result<Value, String>) + Send>;

/// The largest answer the mod may post.
pub(crate) const MAX_ANSWER_BYTES: usize = 32 * 1024 * 1024;

/// Why a job nobody took failed.
pub(crate) const NOT_LISTENING: &str =
    "this session's bro mod isn't taking requests (turn on Settings › bro mod, then start a new Claude session)";

/// Why jobs fail when the bridge stops, or isn't running when asked.
pub(crate) const STOPPED: &str = "bro's bridge stopped before the bro mod answered";

/// The broker's clock. Tests shorten it with [`ModJobs::set_timing`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// A queued job fails when no poll takes it within this.
    pub pickup: Duration,
    /// A job the mod took fails when it isn't answered within this.
    pub answer: Duration,
    /// How long a poll waits for work before answering with none.
    pub poll: Duration,
    /// How long after its last poll a session's mod still counts as listening.
    pub listening: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            pickup: Duration::from_secs(10),
            answer: Duration::from_secs(90),
            poll: Duration::from_secs(25),
            listening: Duration::from_secs(40),
        }
    }
}

/// Every session's queue and the jobs in flight.
#[derive(Default)]
pub(crate) struct ModJobs {
    inner: Mutex<Inner>,
    timing: Mutex<Timing>,
}

#[derive(Default)]
struct Inner {
    /// Per session id.
    lanes: HashMap<String, Lane>,
    /// Queued and in-flight jobs, by job id.
    jobs: HashMap<String, Job>,
    /// The bridge stopped: every job failed, and new ones fail at once.
    closed: bool,
}

/// One session's mod: what waits for it, the bell its polls wait on, and when it last asked for work.
#[derive(Default)]
struct Lane {
    queued: VecDeque<String>,
    bell: Arc<Notify>,
    /// Polls open right now.
    polls: usize,
    polled: Option<Instant>,
}

struct Job {
    session: String,
    body: Value,
    reply: Once,
    /// When it fails unless something happens: the pickup deadline while queued, the answer deadline once taken.
    deadline: Instant,
    taken: bool,
}

/// A reply that runs exactly once: when it's dropped unanswered (the runtime went away with its timer), it
/// answers that the bridge stopped.
struct Once(Option<Reply>);

impl Once {
    fn send(mut self, result: Result<Value, String>) {
        if let Some(reply) = self.0.take() {
            reply(result);
        }
    }
}

impl Drop for Once {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            reply(Err(STOPPED.to_owned()));
        }
    }
}

impl ModJobs {
    pub(crate) fn timing(&self) -> Timing {
        *self.timing.lock()
    }

    #[cfg(test)]
    pub(crate) fn set_timing(&self, timing: Timing) {
        *self.timing.lock() = timing;
    }

    /// The session's mod asked for work within [`Timing::listening`], or is asking right now.
    pub(crate) fn listening(&self, session: &str) -> bool {
        let window = self.timing().listening;
        self.inner.lock().lanes.get(session).is_some_and(|lane| {
            lane.polls > 0 || lane.polled.is_some_and(|at| at.elapsed() < window)
        })
    }

    /// Queues `body` for the session's mod and rings its bell. Returns the job's id and its pickup deadline, or
    /// the reply back once the bridge has stopped.
    fn queue(&self, session: &str, body: Value, reply: Reply) -> Result<(String, Instant), Reply> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let deadline = Instant::now() + self.timing().pickup;
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(reply);
        }
        inner.jobs.insert(
            id.clone(),
            Job { session: session.to_owned(), body, reply: Once(Some(reply)), deadline, taken: false },
        );
        let lane = inner.lanes.entry(session.to_owned()).or_default();
        lane.queued.push_back(id.clone());
        lane.bell.notify_waiters();
        Ok((id, deadline))
    }

    /// A poll from the session's mod begins: it counts as listening until a while after the poll ends.
    fn open_poll(self: &Arc<Self>, session: &str) -> OpenPoll {
        let mut inner = self.inner.lock();
        let lane = inner.lanes.entry(session.to_owned()).or_default();
        lane.polls += 1;
        lane.polled = Some(Instant::now());
        OpenPoll { jobs: self.clone(), session: session.to_owned(), bell: lane.bell.clone() }
    }

    /// Every job queued for the session, as the mod gets them; they are in flight from now on.
    fn take(&self, session: &str) -> Vec<Value> {
        let answer = self.timing().answer;
        let mut inner = self.inner.lock();
        let Inner { lanes, jobs, .. } = &mut *inner;
        let Some(lane) = lanes.get_mut(session) else {
            return Vec::new();
        };
        let now = Instant::now();
        lane.queued
            .drain(..)
            .filter_map(|id| {
                let job = jobs.get_mut(&id)?;
                job.taken = true;
                job.deadline = now + answer;
                Some(flatten(&id, &job.body))
            })
            .collect()
    }

    /// Takes job `id` of `session` out to answer it; None when the session has no such job.
    fn finish(&self, session: &str, id: &str) -> Option<Once> {
        let mut inner = self.inner.lock();
        if inner.jobs.get(id).is_none_or(|job| job.session != session) {
            return None;
        }
        let job = inner.jobs.remove(id)?;
        if !job.taken
            && let Some(lane) = inner.lanes.get_mut(&job.session)
        {
            lane.queued.retain(|queued| queued != id);
        }
        Some(job.reply)
    }

    /// Job `id` once its deadline has passed, taken out, with whether the mod had taken it; `Err` with the
    /// deadline while it still has time, `Ok(None)` once it's gone.
    fn overdue(&self, id: &str) -> Result<Option<(Once, bool)>, Instant> {
        let mut inner = self.inner.lock();
        let Some(job) = inner.jobs.get(id) else {
            return Ok(None);
        };
        if job.deadline > Instant::now() {
            return Err(job.deadline);
        }
        let job = inner.jobs.remove(id).expect("the job was just seen");
        if !job.taken
            && let Some(lane) = inner.lanes.get_mut(&job.session)
        {
            lane.queued.retain(|queued| queued != id);
        }
        Ok(Some((job.reply, job.taken)))
    }

    /// Job `id`'s timer, on the bridge runtime: fails it when a deadline passes without it moving on.
    async fn expire(self: Arc<Self>, id: String, mut deadline: Instant) {
        loop {
            tokio::time::sleep_until(deadline).await;
            match self.overdue(&id) {
                Ok(None) => return,
                Ok(Some((reply, taken))) => {
                    reply.send(Err(if taken {
                        format!(
                            "this session's bro mod took the request but didn't answer within {} s",
                            self.timing().answer.as_secs()
                        )
                    } else {
                        NOT_LISTENING.to_owned()
                    }));
                    return;
                }
                Err(later) => deadline = later,
            }
        }
    }

    /// Every job, failed, and every later one: the bridge stops.
    pub(crate) fn fail_all(&self) {
        let replies: Vec<Once> = {
            let mut inner = self.inner.lock();
            inner.closed = true;
            for lane in inner.lanes.values_mut() {
                lane.queued.clear();
            }
            inner.jobs.drain().map(|(_, job)| job.reply).collect()
        };
        answer_elsewhere(replies, STOPPED);
    }
}

/// An open poll: counts the session's mod as listening, and stamps when it stopped asking once it ends, however
/// it ends (answered, or the mod hung up).
struct OpenPoll {
    jobs: Arc<ModJobs>,
    session: String,
    bell: Arc<Notify>,
}

impl Drop for OpenPoll {
    fn drop(&mut self) {
        if let Some(lane) = self.jobs.inner.lock().lanes.get_mut(&self.session) {
            lane.polls = lane.polls.saturating_sub(1);
            lane.polled = Some(Instant::now());
        }
    }
}

/// Fails `replies` on a short-lived bridge thread, so they never run on bro's own (the caller may hold its locks).
fn answer_elsewhere(replies: Vec<Once>, error: &'static str) {
    if replies.is_empty() {
        return;
    }
    // A thread that can't start drops them here, and `Once` answers that the bridge stopped.
    let _ = std::thread::Builder::new().name("bro-bridge-mod-jobs".into()).spawn(move || {
        for reply in replies {
            reply.send(Err(error.to_owned()));
        }
    });
}

/// The job as the mod gets it: its fields, and its id.
fn flatten(id: &str, body: &Value) -> Value {
    let mut fields = match body {
        Value::Object(fields) => fields.clone(),
        other => {
            let mut fields = Map::new();
            fields.insert("job".into(), other.clone());
            fields
        }
    };
    fields.insert("id".into(), Value::String(id.to_owned()));
    Value::Object(fields)
}

/// [`crate::Bridge::mod_ask`]: queue the job, and start its timer on the bridge runtime.
pub(crate) fn ask(state: &AppState, session: &str, body: Value, reply: Reply) {
    let running = state.running.load(Ordering::Relaxed) && !*state.shutdown.borrow();
    let Some(runtime) = state.runtime.get().cloned().filter(|_| running) else {
        answer_elsewhere(vec![Once(Some(reply))], STOPPED);
        return;
    };
    let jobs = state.mod_jobs.clone();
    match jobs.queue(session, body, reply) {
        Ok((id, deadline)) => {
            runtime.spawn(jobs.expire(id, deadline));
        }
        Err(reply) => answer_elsewhere(vec![Once(Some(reply))], STOPPED),
    }
}

/// The routes, for the router: only callers on this machine, and room for big answers.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/mod/{session}/jobs", get(poll))
        .route("/api/mod/{session}/jobs/{id}", post(answer))
        .route_layer(middleware::from_fn(local_only))
        .layer(DefaultBodyLimit::max(MAX_ANSWER_BYTES + 1024))
}

/// Refuses callers from other machines, token or not. No peer address means an in-process call.
async fn local_only(
    peer: Result<ConnectInfo<SocketAddr>, ExtensionRejection>,
    request: Request,
    next: Next,
) -> Response {
    if peer.ok().is_none_or(|ConnectInfo(peer)| crate::auth::is_loopback(peer)) {
        next.run(request).await
    } else {
        api_error(StatusCode::FORBIDDEN, "Only the bro mod on this machine takes these requests.")
    }
}

/// `GET /api/mod/{session}/jobs`: the session's queued jobs as soon as there are any, else none after
/// [`Timing::poll`] (or at once when the bridge stops).
async fn poll(State(state): State<AppState>, Path(session): Path<String>) -> Json<Value> {
    let jobs = state.mod_jobs.clone();
    let open = jobs.open_poll(&session);
    let until = Instant::now() + jobs.timing().poll;
    let mut shutdown = state.shutdown.subscribe();
    loop {
        let rung = open.bell.notified();
        tokio::pin!(rung);
        // Listen before looking, so a job queued in between still rings.
        rung.as_mut().enable();
        let taken = jobs.take(&session);
        if !taken.is_empty() {
            return Json(json!({ "jobs": taken }));
        }
        tokio::select! {
            _ = &mut rung => {}
            _ = tokio::time::sleep_until(until) => return Json(json!({ "jobs": [] })),
            _ = shutdown.wait_for(|stopped| *stopped) => return Json(json!({ "jobs": [] })),
        }
    }
}

/// `POST /api/mod/{session}/jobs/{id}`: the mod's answer to one job.
async fn answer(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return api_error(StatusCode::BAD_REQUEST, "The answer isn't JSON.");
    };
    let Some(reply) = state.mod_jobs.finish(&session, &id) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "This session has no such job (it was answered, or timed out).",
        );
    };
    let result = if body["ok"] == true {
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(body["error"]
            .as_str()
            .filter(|error| !error.trim().is_empty())
            .unwrap_or("the bro mod couldn't do it")
            .to_owned())
    };
    reply.send(result);
    Json(json!({ "ok": true })).into_response()
}
