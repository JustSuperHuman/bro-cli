//! User-authored page edit requests. Independent of handoff phases and tab lifetime.
use crate::{BridgeCommand, state::AppState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const BACKEND: &str = "http://127.0.0.1:12306/browser/page-review";
const MAX_REQUEST: usize = 256 * 1024;
const MAX_PROMPT: usize = 200 * 1024;
const MAX_RESPONSE: usize = 8 * 1024 * 1024 + 64 * 1024;
const MAX_REGISTRATION: usize = 30 * 1024;
const OUTBOX: &str = "browser-page-requests";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Remote {
    id: String,
    created_at: u64,
    bro_session_id: String,
    prompt: String,
    pages: Vec<Value>,
    status: String,
}

fn identity(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

impl Remote {
    fn parse(row: &Value) -> Option<Self> {
        if serde_json::to_vec(row).ok()?.len() > MAX_REQUEST {
            return None;
        }
        let mut remote: Self = serde_json::from_value(row.clone()).ok()?;
        if !identity(&remote.id)
            || !identity(&remote.bro_session_id)
            || remote.created_at > 8_640_000_000_000_000
            || !matches!(remote.status.as_str(), "pending" | "delivered")
            || remote.pages.is_empty()
            || remote.pages.len() > 100
            || remote.pages.iter().any(|page| !page.is_object())
            || remote.prompt.len() > MAX_PROMPT
            || remote.prompt.trim().is_empty()
        {
            return None;
        }
        remote.prompt = remote.prompt.replace("\r\n", "\n").replace('\r', "\n");
        if remote
            .prompt
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return None;
        }
        Some(remote)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    id: String,
    session: String,
    prompt: String,
    created_at: u64,
    handled: bool,
    acknowledged: bool,
    seen: i64,
    claim_until: i64,
}

impl Pending {
    fn event_id(&self) -> String {
        format!("page-request/{}", self.id)
    }

    fn message(&self) -> String {
        format!(
            "[browser page edit request {}]\nThe user sent this page edit request from JustBrowse. Implement it in the project's source files and respect newer user instructions.\n\n{}",
            self.event_id(),
            self.prompt
        )
    }
}

struct Store {
    root: PathBuf,
}

impl Store {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }

    fn update<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, Pending>) -> T,
    ) -> std::io::Result<T> {
        super::outbox::update(&self.root, OUTBOX, change)
    }

    fn observe(
        &self,
        snapshot: Option<&Value>,
        sessions: &HashSet<String>,
        now: i64,
    ) -> std::io::Result<Vec<Pending>> {
        let remote: Vec<_> = snapshot
            .filter(|value| value["success"] == true)
            .and_then(|value| value["requests"].as_array())
            .filter(|rows| rows.len() <= 200)
            .into_iter()
            .flatten()
            .filter_map(Remote::parse)
            .filter(|row| sessions.contains(&row.bro_session_id))
            .collect();
        if remote.is_empty() && !self.root.join(format!("{OUTBOX}.json")).exists() {
            return Ok(vec![]);
        }
        self.update(|pending| {
            for row in remote {
                // Only a live, known agent can acquire a new local request. Unknown
                // and closed owners remain pending in the extension; never retarget.
                if !sessions.contains(&row.bro_session_id) {
                    continue;
                }
                if let Some(entry) = pending.get_mut(&row.id) {
                    // A stable ID cannot replace an already queued body or its owner.
                    if entry.session == row.bro_session_id && row.status == "delivered" {
                        entry.handled = true;
                        entry.acknowledged = true;
                        entry.prompt.clear();
                        entry.seen = now;
                    }
                } else if row.status == "pending" {
                    pending.insert(
                        row.id.clone(),
                        Pending {
                            id: row.id,
                            session: row.bro_session_id,
                            prompt: row.prompt,
                            created_at: row.created_at,
                            handled: false,
                            acknowledged: false,
                            seen: now,
                            claim_until: 0,
                        },
                    );
                }
            }
            pending.retain(|_, entry| !entry.acknowledged || now - entry.seen < 7 * 86_400);
            let mut ready: Vec<_> = pending
                .values()
                .filter(|entry| {
                    !entry.acknowledged
                        && entry.claim_until <= now
                        && (entry.handled || sessions.contains(&entry.session))
                })
                .cloned()
                .collect();
            ready.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            ready
        })
    }

    fn claim(&self, id: &str, now: i64) -> std::io::Result<Option<Pending>> {
        self.update(|pending| {
            let entry = pending.get(id)?;
            if entry.acknowledged
                || entry.claim_until > now
                || !entry.handled
                    && pending.values().any(|other| {
                        other.id != id
                            && other.session == entry.session
                            && !other.handled
                            && other.claim_until > now
                    })
            {
                return None;
            }
            let entry = pending.get_mut(id)?;
            entry.claim_until = now + 45;
            Some(entry.clone())
        })
    }

    fn delivered(&self, id: &str, now: i64) -> std::io::Result<()> {
        self.update(|pending| {
            if let Some(entry) = pending.get_mut(id) {
                entry.handled = true;
                entry.prompt.clear();
                entry.seen = now;
                entry.claim_until = now + 45;
            }
        })
    }

    fn release(&self, id: &str, acknowledged: bool) -> std::io::Result<()> {
        self.update(|pending| {
            if let Some(entry) = pending.get_mut(id) {
                entry.acknowledged |= acknowledged && entry.handled;
                entry.claim_until = 0;
            }
        })
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// This transport is intentionally private: request bodies never join /api/browser.
async fn call(endpoint: &str, args: &Value) -> Option<Value> {
    let mut response = super::client()
        .post(endpoint)
        .json(args)
        .send()
        .await
        .ok()?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_RESPONSE {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    let result: Value = serde_json::from_slice(&bytes).ok()?;
    (result["success"] == true).then_some(result)
}

fn metadata(text: &str, max: usize) -> String {
    let mut clean = String::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        if clean.len() + c.len_utf8() > max {
            break;
        }
        clean.push(c);
    }
    clean
}

fn targets(state: &AppState) -> Vec<Value> {
    let mut sessions = state.summaries();
    sessions.sort_by(|a, b| a.id.cmp(&b.id));
    sessions.into_iter().filter(|s| s.status == "running" && s.agent.is_some() && identity(&s.id))
        .map(|s| {
            let title = metadata(&s.title, 512);
            json!({"id":s.id, "title":if title.trim().is_empty() { "Agent session" } else { &title }, "folder":metadata(&s.cwd, 1024)})
        }).collect()
}

async fn publish_targets(endpoint: &str, targets: Vec<Value>) {
    // Registrations merge across hosts. Batch without losing long session lists
    // or exceeding the native route's 32 KiB request limit.
    let mut batch = vec![];
    let mut size = 64;
    for target in targets {
        let bytes = serde_json::to_vec(&target).map_or(MAX_REGISTRATION, |v| v.len() + 1);
        if !batch.is_empty() && (size + bytes > MAX_REGISTRATION || batch.len() == 200) {
            let _ = call(endpoint, &json!({"action":"targets", "targets":batch})).await;
            batch.clear();
            size = 64;
        }
        size += bytes;
        batch.push(target);
    }
    let _ = call(endpoint, &json!({"action":"targets", "targets":batch})).await;
}

fn can_submit(state: &AppState, session: &str) -> bool {
    state.summary(session).is_some_and(|s| {
        s.status == "running" && s.agent.is_some() && s.agent_activity.as_deref() == Some("idle")
    })
}

async fn process(state: AppState, entry: Pending, endpoint: String) {
    if !entry.handled && !can_submit(&state, &entry.session) {
        return;
    }
    let root = state.data_root.clone();
    let id = entry.id.clone();
    let Ok(Ok(Some(entry))) =
        tokio::task::spawn_blocking(move || Store::new(&root).claim(&id, now())).await
    else {
        return;
    };
    if !entry.handled {
        let submitted = if can_submit(&state, &entry.session) {
            let (reply, answer) = tokio::sync::oneshot::channel();
            state.send_command(BridgeCommand::BrowserResume {
                id: entry.session.clone(),
                event_id: entry.event_id(),
                text: entry.message(),
                reply,
            });
            matches!(
                tokio::time::timeout(Duration::from_secs(35), answer).await,
                Ok(Ok(Ok(true)))
            )
        } else {
            false
        };
        if !submitted {
            let root = state.data_root.clone();
            let _ =
                tokio::task::spawn_blocking(move || Store::new(&root).release(&entry.id, false))
                    .await;
            return;
        }
        let root = state.data_root.clone();
        let id = entry.id.clone();
        // Commit handled before any remote ACK. Failure leaves the claim for a
        // retry with the same BrowserResume event ID, never an early ACK.
        if !matches!(
            tokio::task::spawn_blocking(move || Store::new(&root).delivered(&id, now())).await,
            Ok(Ok(()))
        ) {
            return;
        }
    }
    let acknowledged = call(
        &endpoint,
        &json!({"action":"ack", "id":entry.id, "broSessionId":entry.session}),
    )
    .await
    .is_some_and(|result| {
        result["request"]["id"] == entry.id
            && result["request"]["broSessionId"] == entry.session
            && result["request"]["status"] == "delivered"
    });
    let root = state.data_root.clone();
    let _ = tokio::task::spawn_blocking(move || Store::new(&root).release(&entry.id, acknowledged))
        .await;
}

async fn poll(state: &AppState, endpoint: &str) {
    let registered = targets(state);
    let sessions = registered
        .iter()
        .filter_map(|s| s["id"].as_str().map(str::to_owned))
        .collect();
    let list = json!({"action":"list"});
    let (_, snapshot) = tokio::join!(publish_targets(endpoint, registered), call(endpoint, &list));
    let root = state.data_root.clone();
    let Ok(Ok(ready)) = tokio::task::spawn_blocking(move || {
        Store::new(&root).observe(snapshot.as_ref(), &sessions, now())
    })
    .await
    else {
        return;
    };
    for entry in ready {
        // Delivery may wait for paste confirmation; target leases still refresh
        // every poll, and another request cannot claim this event meanwhile.
        tokio::spawn(process(state.clone(), entry, endpoint.to_owned()));
    }
}

pub(super) fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut shutdown = state.shutdown.subscribe();
        loop {
            tokio::select! { _ = shutdown.changed() => break, _ = poll(&state, BACKEND) => {} }
            tokio::select! { _ = shutdown.changed() => break, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
        }
    });
}

#[cfg(test)]
mod tests;
