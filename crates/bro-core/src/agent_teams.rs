//! Claude Code agent teams (https://code.claude.com/docs/en/agent-teams): a lead session, the teammates it
//! spawns (each its own Claude instance) and the task list they share. Claude Code keeps a session's team in
//! its config dir, named after the session — `teams/session-<first 8 of its id>/config.json` (who's in it,
//! removed when the session ends) and `tasks/<team>/<id>.json` (the task list, kept) — and bro reads both to
//! show the team beside its lead. Read-only: the team config is Claude's runtime state.
//!
//! Agent teams are switched on per launch by the `agent_teams` knob ([`crate::claude_env`]).

use serde::Deserialize;
use std::path::Path;

/// The agent type Claude Code gives the lead's own entry.
pub const LEAD: &str = "team-lead";

/// The team session `session_id` leads: `session-` + the first eight characters of its id.
pub fn team_name(session_id: &str) -> String {
    format!("session-{}", session_id.chars().take(8).collect::<String>())
}

/// A session's team as Claude Code last wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Team {
    pub name: String,
    /// the lead first, then teammates as they joined
    pub members: Vec<Member>,
    /// in id order ("1", "2", … "10")
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Member {
    pub name: String,
    /// `name@team`
    pub agent_id: String,
    /// The subagent definition it was spawned from ("security-reviewer", "Explore"); None for a plain teammate
    pub agent_type: Option<String>,
    /// One of Claude Code's agent colours: red, blue, green, yellow, purple, orange, pink, cyan
    pub color: Option<String>,
    pub model: Option<String>,
    pub lead: bool,
    /// Mid-turn: Claude Code marks a teammate idle when its turn ends and active when it starts another.
    pub active: bool,
    /// Plans in read-only plan mode until the lead approves
    pub plan_mode: bool,
    pub joined_ms: Option<i64>,
}

/// In board order: what's moving, what's next, what's done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TaskStatus {
    InProgress,
    #[default]
    Pending,
    Completed,
}

impl TaskStatus {
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in progress",
            TaskStatus::Completed => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    /// "Running the tests": what the owner is doing while it's in progress
    pub active_form: Option<String>,
    /// The teammate working on it or who claimed it (a name, sometimes `name@team`)
    pub owner: Option<String>,
    pub status: TaskStatus,
    /// Tasks that must be completed before this one can be claimed
    pub blocked_by: Vec<String>,
}

/// Tasks by where they stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskCounts {
    pub done: usize,
    pub active: usize,
    pub pending: usize,
    /// pending on a task that isn't done yet
    pub blocked: usize,
    pub total: usize,
}

impl Member {
    /// The colour Claude Code draws this agent in, as (r, g, b).
    pub fn rgb(&self) -> Option<(u8, u8, u8)> {
        color_rgb(self.color.as_deref()?)
    }

    /// Owns `owner` (a task's): its name or its agent id.
    pub fn owns(&self, owner: &str) -> bool {
        owner == self.name || owner == self.agent_id
    }
}

/// Claude Code's agent colours (its light theme's, which read on dark grounds too).
pub fn color_rgb(name: &str) -> Option<(u8, u8, u8)> {
    Some(match name.trim_end_matches("_FOR_SUBAGENTS_ONLY") {
        "red" => (220, 38, 38),
        "blue" => (106, 155, 204),
        "green" => (22, 163, 74),
        "yellow" => (202, 138, 4),
        "purple" => (130, 125, 189),
        "orange" => (217, 119, 87),
        "pink" => (196, 102, 134),
        "cyan" => (8, 145, 178),
        _ => return None,
    })
}

impl Team {
    /// Everyone but the lead.
    pub fn teammates(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| !m.lead)
    }

    /// Teammates mid-turn.
    pub fn working(&self) -> usize {
        self.teammates().filter(|m| m.active).count()
    }

    /// A team only in name: Claude Code sets one up for every session while agent teams are on.
    pub fn is_empty(&self) -> bool {
        self.teammates().next().is_none()
    }

    pub fn counts(&self) -> TaskCounts {
        let mut c = TaskCounts { total: self.tasks.len(), ..TaskCounts::default() };
        for t in &self.tasks {
            match t.status {
                TaskStatus::Completed => c.done += 1,
                TaskStatus::InProgress => c.active += 1,
                TaskStatus::Pending if self.blocked(t) => {
                    c.pending += 1;
                    c.blocked += 1;
                }
                TaskStatus::Pending => c.pending += 1,
            }
        }
        c
    }

    /// `t` waits on a task that isn't done (a task id the list no longer has doesn't hold it up).
    pub fn blocked(&self, t: &Task) -> bool {
        t.status == TaskStatus::Pending && t.blocked_by.iter().any(|id| self.tasks.iter().any(|o| o.id == *id && o.status != TaskStatus::Completed))
    }

    /// What `m` is working on: its task in progress, else its first claimed one still to do.
    pub fn task_of(&self, m: &Member) -> Option<&Task> {
        let mine = || self.tasks.iter().filter(|t| t.owner.as_deref().is_some_and(|o| m.owns(o)));
        mine().find(|t| t.status == TaskStatus::InProgress).or_else(|| mine().find(|t| t.status == TaskStatus::Pending))
    }

    /// The member who owns `owner`.
    pub fn member(&self, owner: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.owns(owner))
    }
}

/// Session `session_id`'s team, from Claude config dir `config_dir`. None while it has none (agent teams off,
/// the session ended, or not set up yet).
pub fn read(config_dir: &Path, session_id: &str) -> Option<Team> {
    let name = team_name(session_id);
    let text = std::fs::read_to_string(config_dir.join("teams").join(&name).join("config.json")).ok()?;
    let cfg: Config = serde_json::from_str(&text).ok()?;
    let mut members: Vec<Member> = cfg.members.into_iter().map(Member::from).collect();
    // the lead first, whatever order the file keeps
    members.sort_by_key(|m| !m.lead);
    Some(Team { tasks: read_tasks(&config_dir.join("tasks").join(&name)), name, members })
}

/// Every `<id>.json` task in `dir`, in id order; a file mid-write or unreadable is skipped.
pub fn read_tasks(dir: &Path) -> Vec<Task> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut tasks: Vec<Task> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str::<RawTask>(&std::fs::read_to_string(e.path()).ok()?).ok())
        .map(Task::from)
        .collect();
    tasks.sort_by(|a, b| match (a.id.parse::<u64>(), b.id.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.id.cmp(&b.id),
    });
    tasks
}

// ------------------------------------------------------------------ Claude Code's files

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    members: Vec<RawMember>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMember {
    #[serde(default)]
    name: String,
    #[serde(default)]
    agent_id: String,
    agent_type: Option<String>,
    color: Option<String>,
    model: Option<String>,
    is_active: Option<bool>,
    #[serde(default)]
    plan_mode_required: bool,
    joined_at: Option<i64>,
}

impl From<RawMember> for Member {
    fn from(r: RawMember) -> Member {
        let lead = r.agent_type.as_deref() == Some(LEAD) || r.name == LEAD;
        Member {
            lead,
            // a teammate is working from the moment it's spawned until Claude marks it idle
            active: r.is_active.unwrap_or(!lead),
            agent_type: r.agent_type.filter(|t| t != LEAD && !t.is_empty()),
            name: r.name,
            agent_id: r.agent_id,
            color: r.color,
            model: r.model.filter(|m| !m.is_empty()),
            plan_mode: r.plan_mode_required,
            joined_ms: r.joined_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTask {
    id: serde_json::Value,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    description: String,
    active_form: Option<String>,
    owner: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    blocked_by: Vec<serde_json::Value>,
}

/// An id written as `"3"` or `3`.
fn id_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl From<RawTask> for Task {
    fn from(r: RawTask) -> Task {
        Task {
            id: id_text(&r.id),
            subject: r.subject,
            description: r.description,
            active_form: r.active_form.filter(|a| !a.trim().is_empty()),
            owner: r.owner.filter(|o| !o.is_empty()),
            status: match r.status.as_str() {
                "in_progress" => TaskStatus::InProgress,
                "completed" => TaskStatus::Completed,
                _ => TaskStatus::Pending,
            },
            blocked_by: r.blocked_by.iter().map(id_text).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SID: &str = "1f0e2d3c-aaaa-bbbb-cccc-000000000000";

    fn write(dir: &Path, rel: &str, v: serde_json::Value) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, v.to_string()).unwrap();
    }

    fn team_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "teams/session-1f0e2d3c/config.json",
            json!({
                "name": "session-1f0e2d3c", "createdAt": 1, "leadAgentId": "team-lead@session-1f0e2d3c", "leadSessionId": SID,
                "members": [
                    {"agentId": "researcher@session-1f0e2d3c", "name": "researcher", "color": "blue", "joinedAt": 5, "tmuxPaneId": "in-process", "subscriptions": [], "backendType": "in-process", "isActive": false},
                    {"agentId": "team-lead@session-1f0e2d3c", "name": "team-lead", "agentType": "team-lead", "joinedAt": 1, "tmuxPaneId": "leader", "cwd": "/w", "subscriptions": [], "backendType": "in-process"},
                    {"agentId": "auditor@session-1f0e2d3c", "name": "auditor", "agentType": "security-reviewer", "color": "green", "model": "sonnet", "planModeRequired": true, "joinedAt": 6, "backendType": "in-process"}
                ]
            }),
        );
        let task = |id: &str, subject: &str, status: &str, owner: Option<&str>, blocked_by: &[&str]| {
            json!({"id": id, "subject": subject, "description": "", "activeForm": format!("{subject}ing"), "owner": owner, "status": status, "blocks": [], "blockedBy": blocked_by})
        };
        write(d.path(), "tasks/session-1f0e2d3c/1.json", task("1", "Map the auth flow", "completed", Some("researcher"), &[]));
        write(d.path(), "tasks/session-1f0e2d3c/2.json", task("2", "Audit token handling", "in_progress", Some("auditor@session-1f0e2d3c"), &["1"]));
        write(d.path(), "tasks/session-1f0e2d3c/10.json", task("10", "Write the report", "pending", None, &["2"]));
        write(d.path(), "tasks/session-1f0e2d3c/3.json", task("3", "Check rate limits", "pending", Some("researcher"), &["1"]));
        std::fs::write(d.path().join("tasks/session-1f0e2d3c/.highwatermark"), "10").unwrap();
        std::fs::write(d.path().join("tasks/session-1f0e2d3c/9.json"), "{ half written").unwrap();
        d
    }

    #[test]
    fn names_the_team_after_the_session() {
        assert_eq!(team_name(SID), "session-1f0e2d3c");
        assert_eq!(team_name("abc"), "session-abc");
    }

    #[test]
    fn reads_members_lead_first_and_tasks_in_id_order() {
        let d = team_dir();
        let team = read(d.path(), SID).expect("a team");
        assert_eq!(team.name, "session-1f0e2d3c");
        let names: Vec<_> = team.members.iter().map(|m| (m.name.as_str(), m.lead, m.active)).collect();
        assert_eq!(names, [("team-lead", true, false), ("researcher", false, false), ("auditor", false, true)]);
        let auditor = &team.members[2];
        assert_eq!((auditor.agent_type.as_deref(), auditor.model.as_deref(), auditor.plan_mode), (Some("security-reviewer"), Some("sonnet"), true));
        assert_eq!(auditor.rgb(), Some((22, 163, 74)));
        assert_eq!(team.members[0].agent_type, None, "the lead's own type isn't a subagent definition");
        assert_eq!(team.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "10"], "numeric order, the broken file skipped");
        assert_eq!(team.working(), 1);
        assert!(!team.is_empty());
    }

    #[test]
    fn counts_and_who_works_on_what() {
        let d = team_dir();
        let team = read(d.path(), SID).unwrap();
        assert_eq!(team.counts(), TaskCounts { done: 1, active: 1, pending: 2, blocked: 1, total: 4 });
        let t10 = team.tasks.iter().find(|t| t.id == "10").unwrap();
        let t3 = team.tasks.iter().find(|t| t.id == "3").unwrap();
        assert!(team.blocked(t10), "waits on task 2");
        assert!(!team.blocked(t3), "task 1 is done");
        let [_, researcher, auditor] = &team.members[..] else { panic!() };
        assert_eq!(team.task_of(auditor).map(|t| t.id.as_str()), Some("2"), "owner given as an agent id");
        assert_eq!(team.task_of(researcher).map(|t| t.id.as_str()), Some("3"), "its next claimed task when none is in progress");
        assert_eq!(team.member("auditor@session-1f0e2d3c").map(|m| m.name.as_str()), Some("auditor"));
    }

    #[test]
    fn no_team_without_its_config() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read(d.path(), SID), None);
        // a session on its own still gets a team (with only its lead) while agent teams are on
        write(d.path(), "teams/session-1f0e2d3c/config.json", json!({"members": [{"name": "team-lead", "agentId": "team-lead@x", "agentType": "team-lead"}]}));
        let solo = read(d.path(), SID).unwrap();
        assert!(solo.is_empty() && solo.tasks.is_empty());
    }
}
