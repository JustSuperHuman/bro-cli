//! Agent insights (`docs/agent-insights.md` section 4): why an agent is waiting,
//! a one-line recap, and the files it touched or sent. bro computes them by
//! tailing each pane's transcript and hands them over with
//! [`crate::Bridge::set_insights`] / [`crate::Bridge::set_agent_files`]; the
//! bridge publishes them on the session summary (`attention`, `recap`,
//! `agentFiles`) and serves the lists at `GET /api/sessions/{id}/agent-files`.

use crate::file_preview::{ResolveRoots, resolve_path};
use crate::model::iso_now;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Most entries per list served by `agent-files`.
pub const MAX_LISTED: usize = 200;
/// At most two insight-driven `session` events per second per session.
pub(crate) const PUBLISH_INTERVAL: Duration = Duration::from_millis(500);

/// What bro knows about an agent beyond the screen. Both fields are optional;
/// `Default` clears them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInsights {
    pub attention: Option<Attention>,
    pub recap: Option<Recap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttentionState {
    Blocked,
    Done,
    Working,
    Idle,
}

/// The agent's state plus, when it waits on the user, why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    pub state: AttentionState,
    /// "Bash: npm publish", "Question: Which database?" … (≤ 80 chars)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// ISO-8601 time the state began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecapSource {
    /// Claude Code's own `away_summary` record.
    Claude,
    /// bro ran `/recap` for it.
    Generated,
    /// The first sentence of the agent's last message.
    LastMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recap {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    pub source: RecapSource,
}

/// A file the agent deliberately sent the user (Claude's `SendUserFile`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentFile {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub is_image: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchKind {
    Edit,
    Write,
    Create,
    Delete,
    Move,
    Read,
}

/// A file the agent edited, wrote, created, deleted, moved or read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchedFile {
    pub path: PathBuf,
    pub kind: TouchKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Everything bro knows about a session's files, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFiles {
    /// The transcript the lists came from.
    pub transcript: Option<PathBuf>,
    pub sent: Vec<SentFile>,
    pub touched: Vec<TouchedFile>,
}

/// `agentFiles` on the session summary: counts only; clients fetch the lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFilesCount {
    pub sent: usize,
    pub touched: usize,
    pub updated_at: String,
}

/// Forward-slashed path relative to the session's project root or cwd, when
/// the file lies inside one. Existing files go through the resolver (symlinks,
/// case); deleted ones are compared lexically.
pub(crate) fn relative_path(roots: &ResolveRoots, path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        roots.cwd.join(path)
    };
    if absolute.exists() {
        return resolve_path(roots, &absolute.to_string_lossy())
            .ok()
            .map(|resolved| resolved.relative);
    }
    let key = |p: &Path| {
        let text = p.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/').to_owned();
        if cfg!(windows) { text.to_lowercase() } else { text }
    };
    let target = absolute.to_string_lossy().replace('\\', "/");
    let target_key = key(&absolute);
    [Some(roots.cwd.as_path()), roots.project.as_deref()]
        .into_iter()
        .flatten()
        .filter(|root| !root.as_os_str().is_empty())
        .find_map(|root| {
            let prefix = format!("{}/", key(root));
            target_key
                .starts_with(&prefix)
                .then(|| target[prefix.len()..].to_owned())
        })
        .filter(|relative| !relative.split('/').any(|part| part == ".."))
}

/// The `agent-files` response body. Blocking: resolves paths on disk.
pub(crate) fn agent_files_json(session_id: &str, roots: &ResolveRoots, files: &AgentFiles) -> Value {
    let sent: Vec<Value> = files
        .sent
        .iter()
        .take(MAX_LISTED)
        .map(|file| {
            let mut value = serde_json::to_value(file).unwrap_or_else(|_| json!({}));
            if let Some(relative) = relative_path(roots, &file.path) {
                value["relativePath"] = json!(relative);
            }
            value
        })
        .collect();
    let touched: Vec<Value> = files
        .touched
        .iter()
        .take(MAX_LISTED)
        .map(|file| {
            let mut value = serde_json::to_value(file).unwrap_or_else(|_| json!({}));
            if let Some(relative) = relative_path(roots, &file.path) {
                value["relativePath"] = json!(relative);
            }
            value
        })
        .collect();
    let mut body = json!({ "sessionId": session_id, "sent": sent, "touched": touched });
    if let Some(transcript) = &files.transcript {
        body["transcript"] = json!(transcript.to_string_lossy());
    }
    body
}

impl AppState {
    /// Stores the session's insights; publishes (debounced) only on change.
    pub fn set_insights(&self, session_id: &str, insights: SessionInsights) {
        {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            if session.summary.attention == insights.attention && session.summary.recap == insights.recap {
                return;
            }
            session.summary.attention = insights.attention;
            session.summary.recap = insights.recap;
        }
        self.publish_insights(session_id);
    }

    /// Stores the session's file lists; the summary carries their counts.
    pub fn set_agent_files(&self, session_id: &str, files: AgentFiles) {
        {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            if session.agent_files == files {
                return;
            }
            session.summary.agent_files = (!files.sent.is_empty() || !files.touched.is_empty()).then(|| AgentFilesCount {
                sent: files.sent.len(),
                touched: files.touched.len(),
                updated_at: iso_now(),
            });
            session.agent_files = files;
        }
        self.publish_insights(session_id);
    }

    pub fn agent_files(&self, session_id: &str) -> Option<AgentFiles> {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .map(|session| session.agent_files.clone())
    }

    /// `session` event now, or at most [`PUBLISH_INTERVAL`] after the last one.
    fn publish_insights(&self, session_id: &str) {
        let now = Instant::now();
        let delay = {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            if session.insights_scheduled {
                return;
            }
            match session.insights_published {
                Some(last) if now.duration_since(last) < PUBLISH_INTERVAL => {
                    session.insights_scheduled = true;
                    Some(PUBLISH_INTERVAL - now.duration_since(last))
                }
                _ => {
                    session.insights_published = Some(now);
                    None
                }
            }
        };
        let Some(delay) = delay else {
            self.publish_session(session_id);
            return;
        };
        let state = self.clone();
        let id = session_id.to_owned();
        let flush = move || {
            {
                let mut inner = state.inner.lock();
                let Some(session) = inner.sessions.get_mut(&id) else {
                    return;
                };
                session.insights_scheduled = false;
                session.insights_published = Some(Instant::now());
            }
            state.publish_session(&id);
        };
        match self.runtime.get() {
            Some(runtime) => {
                runtime.spawn(async move {
                    tokio::time::sleep(delay).await;
                    flush();
                });
            }
            None => flush(),
        }
    }

    /// Why the agent waits, when bro said so and it is blocked.
    pub(crate) fn blocked_reason(&self, session_id: &str) -> Option<String> {
        let attention = self.summary(session_id)?.attention?;
        (attention.state == AttentionState::Blocked)
            .then_some(attention.reason)
            .flatten()
            .filter(|reason| !reason.trim().is_empty())
    }

    /// What a finished turn amounts to: the question it ended on, else the recap.
    pub(crate) fn finished_summary(&self, session_id: &str) -> Option<String> {
        let summary = self.summary(session_id)?;
        summary
            .attention
            .filter(|attention| attention.state == AttentionState::Done)
            .and_then(|attention| attention.reason)
            .or(summary.recap.map(|recap| recap.text))
            .filter(|text| !text.trim().is_empty())
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn relative_paths_resolve_inside_roots_lexically_for_deleted_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/app.rs"), "fn main() {}").unwrap();
        let roots = ResolveRoots { cwd: root.path().to_path_buf(), project: None };
        assert_eq!(relative_path(&roots, &root.path().join("src/app.rs")).as_deref(), Some("src/app.rs"));
        assert_eq!(relative_path(&roots, Path::new("src/app.rs")).as_deref(), Some("src/app.rs"));
        assert_eq!(relative_path(&roots, &root.path().join("gone/old.rs")).as_deref(), Some("gone/old.rs"));
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(relative_path(&roots, outside.path()), None);
        assert_eq!(relative_path(&roots, &root.path().join("../escape.rs")), None);
    }

    #[test]
    fn wire_shapes_are_camel_case_and_omit_empty_fields() {
        let insights = SessionInsights {
            attention: Some(Attention { state: AttentionState::Blocked, reason: Some("Bash: npm publish".into()), since: None }),
            recap: Some(Recap { text: "Shipped it".into(), at: None, source: RecapSource::LastMessage }),
        };
        let value = serde_json::to_value(&insights).unwrap();
        assert_eq!(value["attention"], json!({ "state": "blocked", "reason": "Bash: npm publish" }));
        assert_eq!(value["recap"], json!({ "text": "Shipped it", "source": "last-message" }));
        let file = SentFile { path: "a.png".into(), caption: None, at: None, mime_type: Some("image/png".into()), is_image: true };
        assert_eq!(serde_json::to_value(file).unwrap(), json!({ "path": "a.png", "mimeType": "image/png", "isImage": true }));
    }
}
