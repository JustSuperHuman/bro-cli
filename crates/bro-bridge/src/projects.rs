//! Project discovery for the bridge.
//!
//! A project is a directory. Saved projects are the ones a user explicitly
//! created; automatic projects are synthesized for every directory a running
//! session belongs to. In bro that directory is the session's project root
//! (`SessionMeta::project`, the git root) when known, else its cwd. Ids are
//! `directory-<sha256(key)[..24]>` of the normalised directory key, identical
//! to Just Terminal's, so a directory keeps its id across hosts and restarts.
//! Names can be customised for either kind and are persisted by directory key.

use crate::model::{TerminalProject, TerminalSessionSummary};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

/// A user-chosen (or previously derived) name for a directory, remembered so
/// automatic projects keep their name across restarts and after closing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RememberedProject {
    pub name: String,
    pub cwd: String,
    #[serde(default)]
    pub last_used_at: String,
}

/// Case-insensitive, separator-normalised identity for a directory.
pub fn cwd_key(cwd: &str) -> String {
    let mut key: String = cwd
        .trim()
        .replace('/', "\\")
        .chars()
        .flat_map(|character| character.to_lowercase())
        .collect();
    while key.len() > 3 && key.ends_with('\\') {
        key.pop();
    }
    key
}

/// Stable identifier for an automatic project; the same directory always
/// resolves to the same id, so native and web clients can keep referring to
/// it between polls and host restarts.
pub fn automatic_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    format!("directory-{}", &format!("{digest:x}")[..24])
}

fn title_case(segment: &str) -> String {
    segment
        .split(['-', '_', '.', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn directory_display_name(cwd: &str) -> String {
    let trimmed = cwd.trim().trim_end_matches(['\\', '/']);
    let segment = trimmed
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(trimmed)
        .trim_end_matches(':');
    let name = title_case(segment);
    if name.is_empty() {
        "Project".into()
    } else {
        name
    }
}

fn project_json_name(cwd: &str) -> Option<String> {
    let bytes = std::fs::read(Path::new(cwd).join("project.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    for field in ["name", "displayName", "projectName", "title"] {
        if let Some(name) = value.get(field).and_then(serde_json::Value::as_str) {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_owned());
            }
        }
    }
    value
        .get("project")
        .and_then(|project| project.get("name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

pub fn automatic_name(cwd: &str, remembered: Option<&RememberedProject>) -> String {
    if let Some(remembered) = remembered.filter(|entry| !entry.name.trim().is_empty()) {
        return remembered.name.clone();
    }
    project_json_name(cwd).unwrap_or_else(|| directory_display_name(cwd))
}

fn session_counts_for_projects(session: &TerminalSessionSummary, directory: &str) -> bool {
    session.status == "running"
        && session.kind.as_deref() != Some("orchestrator")
        && !directory.trim().is_empty()
}

/// Every project a client should see right now: saved projects (always) plus
/// one automatic project per live directory that no saved project covers,
/// arranged by the persisted order with unknown ids at the end. `sessions`
/// yields each session with the directory it is grouped under.
pub fn visible_projects<'a>(
    saved: &[TerminalProject],
    remembered: &HashMap<String, RememberedProject>,
    order: &[String],
    sessions: impl Iterator<Item = (&'a TerminalSessionSummary, &'a str)>,
) -> Vec<TerminalProject> {
    let mut projects: Vec<TerminalProject> = saved.to_vec();
    let saved_keys: Vec<String> = saved.iter().map(|project| cwd_key(&project.cwd)).collect();

    let mut automatic: Vec<(String, TerminalProject)> = Vec::new();
    for (session, directory) in
        sessions.filter(|(session, directory)| session_counts_for_projects(session, directory))
    {
        let key = cwd_key(directory);
        if key.is_empty() || saved_keys.contains(&key) {
            continue;
        }
        if let Some((_, existing)) = automatic.iter_mut().find(|(existing, _)| *existing == key) {
            if session.created_at < existing.created_at {
                existing.created_at = session.created_at.clone();
            }
            continue;
        }
        automatic.push((
            key.clone(),
            TerminalProject {
                id: automatic_id(&key),
                name: automatic_name(directory, remembered.get(&key)),
                cwd: directory.trim().to_owned(),
                created_at: session.created_at.clone(),
                automatic: Some(true),
            },
        ));
    }
    automatic.sort_by(|left, right| left.1.created_at.cmp(&right.1.created_at));
    projects.extend(automatic.into_iter().map(|(_, project)| project));

    if !order.is_empty() {
        let position = |id: &str| order.iter().position(|candidate| candidate == id);
        projects.sort_by(
            |left, right| match (position(&left.id), position(&right.id)) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        );
    }
    projects
}

/// Resolves the project a session belongs to from its live directory.
pub fn project_id_for_cwd(projects: &[TerminalProject], cwd: &str) -> Option<String> {
    let key = cwd_key(cwd);
    if key.is_empty() {
        return None;
    }
    projects
        .iter()
        .find(|project| cwd_key(&project.cwd) == key)
        .map(|project| project.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, cwd: &str, created_at: &str) -> TerminalSessionSummary {
        let mut summary = TerminalSessionSummary::native(
            id.into(),
            "title".into(),
            "pwsh".into(),
            cwd.into(),
            1,
            120,
            32,
        );
        summary.created_at = created_at.into();
        summary
    }

    fn with_cwd(
        sessions: &[TerminalSessionSummary],
    ) -> impl Iterator<Item = (&TerminalSessionSummary, &str)> {
        sessions
            .iter()
            .map(|session| (session, session.cwd.as_str()))
    }

    #[test]
    fn project_ids_are_stable_and_match_just_terminal() {
        // Same directory, any spelling -> same id; the id is the first 24 hex
        // chars of sha256 over the normalised key (Just Terminal's scheme).
        let id = automatic_id(&cwd_key("F:/bro-cli-v2/"));
        assert_eq!(id, automatic_id(&cwd_key(r"f:\BRO-CLI-V2")));
        assert!(id.starts_with("directory-"));
        assert_eq!(id.len(), "directory-".len() + 24);
        let digest = Sha256::digest(r"f:\bro-cli-v2".as_bytes());
        assert_eq!(id, format!("directory-{}", &format!("{digest:x}")[..24]));
        assert_ne!(id, automatic_id(&cwd_key("F:/bro-cli")));
    }

    #[test]
    fn directory_keys_ignore_case_and_separators() {
        assert_eq!(cwd_key("F:/Terminal/"), cwd_key("f:\\terminal"));
        assert_eq!(
            automatic_id(&cwd_key("F:/Terminal/")),
            automatic_id(&cwd_key("f:\\terminal"))
        );
        assert_eq!(cwd_key("C:\\"), "c:\\");
    }

    #[test]
    fn display_names_come_from_the_directory() {
        assert_eq!(
            directory_display_name("F:\\my-cool_project"),
            "My Cool Project"
        );
        assert_eq!(directory_display_name("C:\\"), "C");
        assert_eq!(directory_display_name(""), "Project");
    }

    #[test]
    fn live_directories_become_automatic_projects_once() {
        let sessions = [
            session("a", "F:\\one", "2026-01-02T00:00:00Z"),
            session("b", "f:/one/", "2026-01-01T00:00:00Z"),
            session("c", "F:\\two", "2026-01-03T00:00:00Z"),
        ];
        let projects = visible_projects(&[], &HashMap::new(), &[], with_cwd(&sessions));
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].cwd, "F:\\one");
        assert_eq!(projects[0].created_at, "2026-01-01T00:00:00Z");
        assert_eq!(projects[0].automatic, Some(true));
        assert_eq!(projects[1].name, "Two");
        assert_eq!(
            project_id_for_cwd(&projects, "F:/TWO"),
            Some(projects[1].id.clone())
        );
    }

    #[test]
    fn saved_projects_cover_their_directory_and_remembered_names_win() {
        let saved = vec![TerminalProject {
            id: "saved".into(),
            name: "Saved".into(),
            cwd: "F:\\one".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            automatic: None,
        }];
        let mut remembered = HashMap::new();
        remembered.insert(
            cwd_key("F:\\two"),
            RememberedProject {
                name: "Renamed".into(),
                cwd: "F:\\two".into(),
                last_used_at: String::new(),
            },
        );
        let sessions = [
            session("a", "f:\\ONE", "2026"),
            session("b", "F:\\two", "2026"),
        ];
        let order = vec![automatic_id(&cwd_key("F:\\two")), "saved".into()];
        let projects = visible_projects(&saved, &remembered, &order, with_cwd(&sessions));
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].name, "Renamed");
        assert_eq!(projects[1].id, "saved");
        let exited = {
            let mut summary = session("c", "F:\\three", "2026");
            summary.status = "exited".into();
            summary
        };
        assert_eq!(
            visible_projects(&saved, &remembered, &[], with_cwd(&[exited])).len(),
            1
        );
    }
}
