//! Durable completion outbox. Browser handoffs are advisory; a completed
//! handoff either has a witnessed agent recheck or receives one submitted wake-up.
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Remote {
    handoff_id: String,
    tab_id: u64,
    bro_session_id: String,
    phase: String,
    #[serde(default)]
    checked: bool,
    #[serde(default)]
    outcome: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    id: String,
    session: String,
    tab: u64,
    ready: bool,
    closed: bool,
    handled: bool,
    seen: i64,
    #[serde(default)]
    claim_until: i64,
}

impl Pending {
    fn message(&self) -> String {
        let state = if self.closed {
            "The browser tab was closed during handoff."
        } else {
            "The user finished the browser handoff with Done."
        };
        format!(
            "[browser handoff {}] {state} Recheck browser tab {} with chrome_wait_for_handoff and inspect its current page before continuing the original task, if it is still active. Respect newer user instructions and cancellations. If the tab was closed, inspect your remaining owned tabs and recover the task as needed. Other pending handoffs do not block your work. Do not repeat actions already completed. This event ID identifies one handoff; ignore a duplicate after you have rechecked it.",
            self.id, self.tab
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
        super::outbox::update(&self.root, "browser-handoffs", change)
    }

    fn observe(
        &self,
        snapshot: &Value,
        sessions: &HashSet<String>,
        now: i64,
    ) -> std::io::Result<Vec<Pending>> {
        let Some(rows) = snapshot.get("handoffs").and_then(Value::as_array) else {
            return Ok(vec![]);
        };
        let remote: Vec<Remote> = rows
            .iter()
            .filter_map(|row| serde_json::from_value(row.clone()).ok())
            .filter(|row: &Remote| {
                !row.handoff_id.is_empty()
                    && row.handoff_id.len() <= 256
                    && row.bro_session_id.len() <= 256
            })
            .collect();
        if remote.is_empty() && !self.root.join("browser-handoffs.json").exists() {
            return Ok(vec![]);
        }
        let tabs: HashSet<u64> = snapshot["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["tabId"].as_u64())
            .collect();
        let seen: HashSet<String> = remote.iter().map(|row| row.handoff_id.clone()).collect();
        self.update(|pending| {
            for row in remote {
                if !sessions.contains(&row.bro_session_id) {
                    continue;
                }
                let entry = pending
                    .entry(row.handoff_id.clone())
                    .or_insert_with(|| Pending {
                        id: row.handoff_id,
                        session: row.bro_session_id,
                        tab: row.tab_id,
                        ready: false,
                        closed: false,
                        handled: false,
                        seen: now,
                        claim_until: 0,
                    });
                if entry.ready != (row.phase == "agent")
                    || entry.closed != (row.outcome == "tab-closed")
                    || row.checked && !entry.handled
                {
                    entry.seen = now;
                }
                entry.ready = row.phase == "agent";
                entry.closed = row.outcome == "tab-closed";
                entry.handled |= row.checked;
            }
            for entry in pending.values_mut() {
                if !entry.ready && !seen.contains(&entry.id) && !tabs.contains(&entry.tab) {
                    entry.ready = true;
                    entry.closed = true;
                }
            }
            pending.retain(|_, entry| !entry.handled || now - entry.seen < 7 * 86_400);
            let mut ready: Vec<_> = pending
                .values()
                .filter(|entry| {
                    entry.ready
                        && !entry.handled
                        && entry.claim_until <= now
                        && sessions.contains(&entry.session)
                })
                .cloned()
                .collect();
            ready.sort_by(|a, b| a.id.cmp(&b.id));
            ready
        })
    }

    fn claim(&self, id: &str, now: i64) -> std::io::Result<bool> {
        self.update(|pending| {
            let Some(entry) = pending
                .get_mut(id)
                .filter(|entry| !entry.handled && entry.ready && entry.claim_until <= now)
            else {
                return false;
            };
            entry.claim_until = now + 45;
            true
        })
    }

    fn delivered(&self, id: &str, submitted: bool) -> std::io::Result<()> {
        self.update(|pending| {
            if let Some(entry) = pending.get_mut(id) {
                entry.handled |= submitted;
                entry.claim_until = 0;
                if submitted {
                    entry.seen = now();
                }
            }
        })
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(crate) fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut shutdown = state.shutdown.subscribe();
        loop {
            let tick = observe_and_wake(&state);
            tokio::select! { _ = shutdown.changed() => break, _ = tick => {} }
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = state.browser_handoff_changed.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(5)) => {},
            }
        }
    });
}

async fn observe_and_wake(state: &AppState) {
    let Ok(snapshot) = super::call(&serde_json::json!({"action":"list"})).await else {
        return;
    };
    super::update_pending(state, &snapshot);
    let sessions = state
        .summaries()
        .into_iter()
        .filter(|s| s.status == "running" && s.agent.is_some())
        .map(|s| s.id)
        .collect();
    let root = state.data_root.clone();
    let Ok(Ok(ready)) =
        tokio::task::spawn_blocking(move || Store::new(&root).observe(&snapshot, &sessions, now()))
            .await
    else {
        return;
    };
    for entry in ready {
        // An agent still waiting in its tool can recheck the page itself. Its
        // next successful browser operation acknowledges this completion.
        if !state
            .summary(&entry.session)
            .is_some_and(|s| s.agent_activity.as_deref() == Some("idle"))
        {
            continue;
        }
        let root = state.data_root.clone();
        let id = entry.id.clone();
        if !matches!(
            tokio::task::spawn_blocking(move || Store::new(&root).claim(&id, now())).await,
            Ok(Ok(true))
        ) {
            continue;
        }
        let (reply, answer) = tokio::sync::oneshot::channel();
        state.send_command(crate::BridgeCommand::BrowserResume {
            id: entry.session.clone(),
            event_id: entry.id.clone(),
            text: entry.message(),
            reply,
        });
        let submitted = matches!(
            tokio::time::timeout(Duration::from_secs(35), answer).await,
            Ok(Ok(Ok(true)))
        );
        let root = state.data_root.clone();
        let _ =
            tokio::task::spawn_blocking(move || Store::new(&root).delivered(&entry.id, submitted))
                .await;
    }
}

#[cfg(test)]
mod tests;
