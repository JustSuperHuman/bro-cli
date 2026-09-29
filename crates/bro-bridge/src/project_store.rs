//! Saved projects, remembered names and the user's ordering, persisted in
//! `<data_root>/.terminal-web-projects.json` (same shape as Just Terminal's:
//! `{ projects, names, order }`; the Node host's `recents` is read too), plus
//! the registry operations the `/api/projects` routes call.

use crate::model::{TerminalProject, TerminalSessionSummary, iso_now};
use crate::projects::{self, RememberedProject};
use crate::state::{AppState, Inner};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

const MAX_PROJECTS: usize = 200;
const PROJECTS_FILE: &str = ".terminal-web-projects.json";

#[derive(Default)]
pub(crate) struct ProjectStore {
    pub saved: Vec<TerminalProject>,
    pub remembered: HashMap<String, RememberedProject>,
    pub order: Vec<String>,
}

impl ProjectStore {
    /// Reads the store; a missing or malformed file is an empty store.
    pub fn load(data_root: &Path) -> Self {
        let Ok(bytes) = std::fs::read(data_root.join(PROJECTS_FILE)) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return Self::default();
        };
        let Some(saved) = value
            .get("projects")
            .and_then(Value::as_array)
            .or_else(|| value.as_array())
        else {
            return Self::default();
        };
        let mut store = Self {
            saved: saved
                .iter()
                .filter_map(|project| {
                    serde_json::from_value::<TerminalProject>(project.clone()).ok()
                })
                .filter(|project| !project.cwd.trim().is_empty())
                .take(MAX_PROJECTS)
                .collect(),
            ..Self::default()
        };
        for field in ["names", "recents"] {
            for entry in value
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Ok(remembered) = serde_json::from_value::<RememberedProject>(entry.clone())
                    && !remembered.cwd.trim().is_empty()
                    && !remembered.name.trim().is_empty()
                {
                    store
                        .remembered
                        .entry(projects::cwd_key(&remembered.cwd))
                        .or_insert(remembered);
                }
            }
        }
        store.order = value
            .get("order")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        store
    }

    /// Visible projects for the given `(session, grouping directory)` pairs.
    pub fn visible<'a>(
        &self,
        sessions: impl Iterator<Item = (&'a TerminalSessionSummary, &'a str)>,
    ) -> Vec<TerminalProject> {
        projects::visible_projects(&self.saved, &self.remembered, &self.order, sessions)
    }

    fn remember(&mut self, cwd: &str, name: &str) {
        let key = projects::cwd_key(cwd);
        if key.is_empty() {
            return;
        }
        self.remembered.insert(
            key,
            RememberedProject {
                name: name.trim().to_owned(),
                cwd: cwd.trim().to_owned(),
                last_used_at: iso_now(),
            },
        );
        while self.remembered.len() > MAX_PROJECTS {
            let oldest = self
                .remembered
                .iter()
                .min_by(|left, right| left.1.last_used_at.cmp(&right.1.last_used_at))
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.remembered.remove(&key),
                None => break,
            };
        }
    }

    fn recents(&self) -> Vec<RememberedProject> {
        let mut recents: Vec<RememberedProject> = self.remembered.values().cloned().collect();
        recents.sort_by(|left, right| right.last_used_at.cmp(&left.last_used_at));
        recents
    }
}

fn visible_locked(inner: &Inner) -> Vec<TerminalProject> {
    inner.projects.visible(
        inner
            .sessions
            .values()
            .map(|session| (&session.summary, session.grouping_dir())),
    )
}

/// temp + rename, so a crash never leaves a truncated file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

impl AppState {
    fn persist_projects(&self) {
        let serialized = {
            let inner = self.inner.lock();
            serde_json::to_vec_pretty(&json!({
                "projects": inner.projects.saved,
                "names": inner.projects.recents(),
                "order": inner.projects.order
            }))
        };
        if let Ok(serialized) = serialized
            && let Err(error) = write_atomic(&self.data_root.join(PROJECTS_FILE), &serialized)
        {
            tracing::warn!("bro-bridge: could not save projects: {error}");
        }
    }

    /// Re-derives the visible project list from saved projects and live
    /// session directories, stamps every running session with the project it
    /// belongs to, and broadcasts whatever changed.
    pub(crate) fn reconcile_projects(&self) -> Vec<TerminalProject> {
        let (projects, projects_changed, changed_sessions) = {
            let mut inner = self.inner.lock();
            let projects = visible_locked(&inner);
            let projects_changed = inner.published_projects != projects;
            let mut changed_sessions = Vec::new();
            for session in inner.sessions.values_mut() {
                let assigned = if session.summary.status == "running" {
                    projects::project_id_for_cwd(&projects, session.grouping_dir())
                        .or_else(|| session.summary.project_id.clone())
                } else {
                    session.summary.project_id.clone()
                };
                if session.summary.project_id != assigned {
                    session.summary.project_id = assigned;
                    changed_sessions.push(session.summary.id.clone());
                }
            }
            inner.published_projects = projects.clone();
            (projects, projects_changed, changed_sessions)
        };
        for id in changed_sessions {
            self.publish_session(&id);
        }
        if projects_changed {
            self.publish(crate::model::ServerEvent::global(
                json!({ "type": "projects", "projects": projects }),
            ));
        }
        projects
    }

    pub(crate) fn projects(&self) -> Vec<TerminalProject> {
        self.reconcile_projects()
    }

    pub(crate) fn create_project(&self, name: Option<&str>, cwd: &str) -> TerminalProject {
        let cwd = cwd.trim();
        let key = projects::cwd_key(cwd);
        let project = {
            let mut inner = self.inner.lock();
            let store = &mut inner.projects;
            let existing = store
                .saved
                .iter()
                .position(|project| projects::cwd_key(&project.cwd) == key);
            let name = name
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| projects::automatic_name(cwd, store.remembered.get(&key)));
            let project = if let Some(index) = existing {
                store.saved[index].name = name.clone();
                store.saved[index].clone()
            } else {
                // Reuse the automatic id for this directory so sessions already
                // filtered to it keep their project when it becomes saved.
                let project = TerminalProject {
                    id: projects::automatic_id(&key),
                    name: name.clone(),
                    cwd: cwd.to_owned(),
                    created_at: iso_now(),
                    automatic: None,
                };
                store.saved.push(project.clone());
                if store.saved.len() > MAX_PROJECTS {
                    store.saved.remove(0);
                }
                project
            };
            store.remember(cwd, &name);
            project
        };
        self.persist_projects();
        self.reconcile_projects();
        project
    }

    pub(crate) fn rename_project(&self, id: &str, name: &str) -> Option<TerminalProject> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        {
            let mut inner = self.inner.lock();
            let published = inner
                .published_projects
                .iter()
                .find(|project| project.id == id)
                .map(|project| project.cwd.clone());
            let store = &mut inner.projects;
            let cwd = if let Some(saved) = store.saved.iter_mut().find(|project| project.id == id) {
                saved.name = name.to_owned();
                saved.cwd.clone()
            } else {
                published?
            };
            store.remember(&cwd, name);
        }
        self.persist_projects();
        self.reconcile_projects()
            .into_iter()
            .find(|project| project.id == id)
    }

    pub(crate) fn delete_project(&self, id: &str) {
        {
            let mut inner = self.inner.lock();
            let store = &mut inner.projects;
            if let Some(index) = store.saved.iter().position(|project| project.id == id) {
                let removed = store.saved.remove(index);
                store.remember(&removed.cwd, &removed.name);
            }
            store.order.retain(|candidate| candidate != id);
        }
        self.persist_projects();
        self.reconcile_projects();
    }

    pub(crate) fn reorder_projects(&self, ids: Vec<String>) {
        {
            let mut inner = self.inner.lock();
            let mut order: Vec<String> = ids.into_iter().filter(|id| !id.is_empty()).collect();
            for project in &inner.published_projects {
                if !order.contains(&project.id) {
                    order.push(project.id.clone());
                }
            }
            inner.projects.order = order;
        }
        self.persist_projects();
        self.reconcile_projects();
    }

    pub(crate) fn recent_projects(&self) -> Vec<RememberedProject> {
        self.inner.lock().projects.recents()
    }
}
