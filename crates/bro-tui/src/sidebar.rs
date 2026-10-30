//! The sidebar model: a pure function from live sessions + past sessions + UI state (filter) to the rows
//! the sidebar draws and navigates. No I/O, no drawing.
//!
//! Order: the orchestrator (when one runs) pinned on top, then projects with open sessions first,
//! preserving project order within each group. Earlier sessions aren't rows: Continue is available in
//! the session dialog.

use crate::pane::{Activity, PaneId};
use bro_core::Harness;
use bro_core::agent_teams::Team;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Teammates listed under their lead; a bigger team shows this many less one, then "+N more".
pub const TEAM_ROWS: usize = 4;

/// The lines a lead's team takes under its row: a summary, then its teammates (at most [`TEAM_ROWS`]).
pub fn team_lines(team: &Team) -> usize {
    1 + team.teammates().count().min(TEAM_ROWS)
}

/// A live session, as the sidebar needs it.
#[derive(Clone, Debug)]
pub struct LiveInfo {
    pub pane: PaneId,
    pub harness: Option<Harness>,
    /// What to call it: your name, else the agent's own title (Claude Code: the current task)
    pub name: Option<String>,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub project_key: String,
    pub project_name: String,
    pub project_root: PathBuf,
    pub activity: Option<Activity>,
    /// finished while you weren't looking
    pub done: bool,
    /// a second line: `⏸ why it waits` / `↳ recap`
    pub note: Option<String>,
    /// seconds since launch
    pub age_secs: u64,
    /// launch order (smaller = earlier)
    pub seq: u64,
    /// bro's orchestrator: pinned above every project, first in the numbering
    pub orchestrator: bool,
    /// The agent team it leads, while it has teammates (listed under it)
    pub team: Option<Arc<Team>>,
}

/// An earlier session, as the sidebar needs it: which project, and how long ago (for "last used").
#[derive(Clone, Debug)]
pub struct PastInfo {
    pub age_secs: u64,
    pub project_key: String,
    pub project_root: PathBuf,
}

/// A project you've opened (from `OpenProjects`), resolved.
#[derive(Clone, Debug)]
pub struct OpenInfo {
    pub key: String,
    pub name: String,
    pub root: PathBuf,
}

/// Sidebar UI state.
#[derive(Clone, Debug, Default)]
pub struct SideState {
    pub filter: String,
}

/// One sidebar row. Earlier sessions are available from the Continue tab.
#[derive(Clone, Debug)]
pub enum Row {
    Project {
        key: String,
        name: String,
        root: PathBuf,
        live: usize,
        attention: bool,
        /// seconds since the newest earlier session
        last_age: Option<u64>,
    },
    Live { info: LiveInfo, n: Option<usize> },
}

impl Row {
    /// The project a row belongs to.
    pub fn project_key(&self) -> &str {
        match self {
            Row::Project { key, .. } => key,
            Row::Live { info, .. } => &info.project_key,
        }
    }
}

/// Only the workspace coordinator is pinned outside project groups.
fn global_orchestrator(info: &LiveInfo) -> bool {
    info.orchestrator && info.project_key == bro_core::orchestrator::GLOBAL_KEY
}

struct Group<'a> {
    key: String,
    name: String,
    root: PathBuf,
    live: Vec<&'a LiveInfo>,
    /// newest earlier session's age
    last_age: Option<u64>,
}

/// Active projects first; preserve opened-project order and then launch order within each group.
fn groups<'a>(live: &'a [LiveInfo], past: &[PastInfo], open: &[OpenInfo]) -> Vec<Group<'a>> {
    let mut order: Vec<Group> = open.iter().map(|o| Group { key: o.key.clone(), name: o.name.clone(), root: o.root.clone(), live: vec![], last_age: None }).collect();
    let mut at: HashMap<String, usize> = order.iter().enumerate().map(|(i, g)| (g.key.clone(), i)).collect();
    let mut live_sorted: Vec<&LiveInfo> = live.iter().filter(|l| !global_orchestrator(l)).collect();
    live_sorted.sort_by_key(|l| (!l.orchestrator, l.seq));
    for l in live_sorted {
        let i = *at.entry(l.project_key.clone()).or_insert_with(|| {
            order.push(Group { key: l.project_key.clone(), name: l.project_name.clone(), root: l.project_root.clone(), live: vec![], last_age: None });
            order.len() - 1
        });
        order[i].live.push(l);
    }
    for p in past {
        if let Some(&i) = at.get(&p.project_key) {
            let g = &mut order[i];
            g.last_age = Some(g.last_age.map_or(p.age_secs, |a| a.min(p.age_secs)));
        }
    }
    order.sort_by_key(|g| g.live.is_empty());
    order
}

/// The projects in sidebar order: (key, name, root).
pub fn projects(live: &[LiveInfo], open: &[OpenInfo]) -> Vec<(String, String, PathBuf)> {
    groups(live, &[], open).into_iter().map(|g| (g.key, g.name, g.root)).collect()
}

/// Live sessions in sidebar order (for alt+1..9 and next/prev), ignoring the filter.
pub fn live_order(live: &[LiveInfo], past: &[PastInfo], open: &[OpenInfo]) -> Vec<PaneId> {
    let pinned = live.iter().filter(|l| global_orchestrator(l)).map(|l| l.pane);
    pinned.chain(groups(live, past, open).iter().flat_map(|g| g.live.iter().map(|l| l.pane))).collect()
}

/// A project with running sessions, as the project tabs show it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectTab {
    pub key: String,
    pub name: String,
    pub root: PathBuf,
    /// sessions in its grid (minimized ones aside)
    pub live: usize,
    /// minimized sessions, kept out of its grid
    pub hidden: usize,
    /// agents mid-turn
    pub working: usize,
    /// agents waiting on you
    pub blocked: usize,
    /// agents that finished while you weren't looking
    pub done: usize,
}

impl ProjectTab {
    /// Something here wants you: a question to answer or work to look at.
    pub fn attention(&self) -> bool {
        self.blocked + self.done > 0
    }
}

/// Projects with running sessions, including their coordinator, in sidebar order, ignoring the filter;
/// `hidden` sessions (minimized) count apart.
pub fn project_tabs(live: &[LiveInfo], open: &[OpenInfo], hidden: &[PaneId]) -> Vec<ProjectTab> {
    groups(live, &[], open)
        .into_iter()
        .filter(|g| !g.live.is_empty())
        .map(|g| {
            let count = |want: &dyn Fn(&LiveInfo) -> bool| g.live.iter().filter(|l| want(l)).count();
            ProjectTab {
                live: count(&|l| !hidden.contains(&l.pane)),
                hidden: count(&|l| hidden.contains(&l.pane)),
                working: count(&|l| l.activity == Some(Activity::Working)),
                blocked: count(&|l| l.activity == Some(Activity::Blocked)),
                done: count(&|l| l.done && l.activity != Some(Activity::Blocked)),
                key: g.key,
                name: g.name,
                root: g.root,
            }
        })
        .collect()
}

/// Project keys in sidebar order.
#[cfg(test)]
pub fn project_order(live: &[LiveInfo], past: &[PastInfo], open: &[OpenInfo]) -> Vec<String> {
    groups(live, past, open).into_iter().map(|g| g.key).collect()
}

fn matches(hay: &[&str], q: &str) -> bool {
    hay.iter().any(|h| h.to_lowercase().contains(q))
}

/// The rows to draw.
pub fn build(live: &[LiveInfo], past: &[PastInfo], open: &[OpenInfo], st: &SideState) -> Vec<Row> {
    let q = st.filter.trim().to_lowercase();
    let numbering: HashMap<PaneId, usize> = live_order(live, past, open).into_iter().enumerate().map(|(i, p)| (p, i + 1)).collect();
    let mut rows = vec![];
    for l in live.iter().filter(|l| global_orchestrator(l)) {
        if q.is_empty() || matches(&["Orchestrator", l.name.as_deref().unwrap_or(""), l.harness.map(|h| h.label()).unwrap_or(""), l.profile.as_deref().unwrap_or("")], &q) {
            rows.push(Row::Live { info: l.clone(), n: numbering.get(&l.pane).copied().filter(|n| *n <= 9) });
        }
    }
    for g in groups(live, past, open) {
        let project_hit = !q.is_empty() && matches(&[&g.name], &q);
        let live_rows: Vec<&LiveInfo> = g
            .live
            .iter()
            .copied()
            .filter(|l| q.is_empty() || project_hit || matches(&[if l.orchestrator { "orchestrator" } else { "" }, l.name.as_deref().unwrap_or(""), l.harness.map(|h| h.label()).unwrap_or("shell"), l.profile.as_deref().unwrap_or(""), l.model.as_deref().unwrap_or("")], &q))
            .collect();
        if !q.is_empty() && live_rows.is_empty() && !project_hit {
            continue;
        }
        let attention = g.live.iter().any(|l| l.activity == Some(Activity::Blocked) || l.done);
        rows.push(Row::Project { key: g.key.clone(), name: g.name.clone(), root: g.root.clone(), live: g.live.len(), attention, last_age: g.last_age });
        for l in live_rows {
            rows.push(Row::Live { info: l.clone(), n: numbering.get(&l.pane).copied().filter(|n| *n <= 9) });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(pane: PaneId, project: &str, seq: u64, act: Option<Activity>) -> LiveInfo {
        LiveInfo {
            pane,
            harness: Some(Harness::Claude),
            name: None,
            profile: Some("claude:work".into()),
            model: Some("opus".into()),
            project_key: project.into(),
            project_name: project.into(),
            project_root: PathBuf::from(project),
            activity: act,
            done: false,
            note: None,
            age_secs: 60,
            seq,
            orchestrator: false,
            team: None,
        }
    }

    fn past(_idx: usize, project: &str, _title: &str, age: u64) -> PastInfo {
        PastInfo { age_secs: age, project_key: project.into(), project_root: PathBuf::from(project) }
    }

    fn opened(keys: &[&str]) -> Vec<OpenInfo> {
        keys.iter().map(|k| OpenInfo { key: k.to_string(), name: k.to_string(), root: PathBuf::from(k) }).collect()
    }

    fn kinds(rows: &[Row]) -> String {
        rows.iter()
            .map(|r| match r {
                Row::Project { name, .. } => format!("P:{name}"),
                Row::Live { info, n } => format!("L{}#{}", info.pane, n.unwrap_or(0)),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn opened_projects_and_running_sessions_only() {
        let l = vec![live(10, "b", 2, None), live(11, "x", 1, Some(Activity::Blocked))];
        let p = vec![past(0, "c", "never opened", 900), past(1, "b", "fix it", 100), past(2, "a", "newer", 50)];
        let rows = build(&l, &p, &opened(&["a", "b"]), &SideState::default());
        assert_eq!(kinds(&rows), "P:b L10#1 P:x L11#2 P:a");
        assert!(matches!(&rows[4], Row::Project { last_age: Some(50), .. }));
        assert!(matches!(&rows[2], Row::Project { attention: true, .. }));
        assert_eq!(live_order(&l, &p, &opened(&["a", "b"])), vec![10, 11]);
        assert_eq!(project_order(&l, &p, &opened(&["a", "b"])), vec!["b", "x", "a"]);
        assert_eq!(kinds(&build(&l, &[], &opened(&["a"]), &SideState::default())), "P:x L11#1 P:b L10#2 P:a");
    }

    #[test]
    fn project_tabs_are_projects_with_running_sessions_in_sidebar_order() {
        let mut l = vec![live(1, "c", 1, Some(Activity::Working)), live(2, "b", 2, Some(Activity::Blocked)), live(3, "c", 3, None), live(5, "c", 5, None)];
        l[2].done = true;
        let mut boss = live(4, "bro:orchestrator", 4, None);
        boss.orchestrator = true;
        l.push(boss);
        let tabs = project_tabs(&l, &opened(&["a", "b", "c"]), &[5]);
        let summary: Vec<(&str, usize, usize, usize, usize, usize)> = tabs.iter().map(|t| (t.key.as_str(), t.live, t.hidden, t.working, t.blocked, t.done)).collect();
        assert_eq!(summary, vec![("b", 1, 0, 0, 1, 0), ("c", 2, 1, 1, 0, 1)], "no tab for a project without sessions or for the orchestrator");
        assert!(tabs.iter().all(|t| t.attention()));
    }

    #[test]
    fn projects_reorder_when_sessions_open_and_close() {
        let open = opened(&["a", "b", "c", "d"]);
        let mut sessions = vec![live(1, "c", 1, None), live(2, "b", 2, Some(Activity::Blocked))];
        assert_eq!(project_order(&sessions, &[], &open), vec!["b", "c", "a", "d"]);
        sessions.push(live(3, "a", 3, Some(Activity::Working)));
        assert_eq!(project_order(&sessions, &[], &open), vec!["a", "b", "c", "d"]);
        sessions.retain(|l| l.project_key != "b");
        assert_eq!(project_order(&sessions, &[], &open), vec!["a", "c", "b", "d"]);
        assert_eq!(live_order(&sessions, &[], &open), vec![3, 1]);
        sessions.clear();
        assert_eq!(project_order(&sessions, &[], &open), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn filter() {
        let l = vec![live(1, "a", 1, None), live(2, "b", 2, None)];
        let open = opened(&["a", "b"]);
        let mut st = SideState::default();
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "P:a L1#1 P:b L2#2");
        st.filter = "b".into();
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "P:b L2#2");
        st.filter = "zzz".into();
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "", "the project list honors its filter");
    }

    #[test]
    fn orchestrator_is_pinned_on_top_and_numbered_first() {
        let mut boss = live(9, "a", 99, Some(Activity::Working));
        boss.orchestrator = true;
        boss.project_key = "bro:orchestrator".into();
        let l = vec![live(1, "a", 1, None), boss, live(2, "b", 2, None)];
        let open = opened(&["a", "b"]);
        let rows = build(&l, &[], &open, &SideState::default());
        assert_eq!(kinds(&rows), "L9#1 P:a L1#2 P:b L2#3");
        assert!(matches!(&rows[1], Row::Project { live: 1, .. }), "the orchestrator isn't counted in a project");
        assert_eq!(live_order(&l, &[], &open), vec![9, 1, 2]);
        assert_eq!(project_order(&l, &[], &open), vec!["a", "b"], "it never forms a project group");
        let st = SideState { filter: "orch".into() };
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "L9#1");
        let st = SideState { filter: "b".into() };
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "P:b L2#3");
    }

    #[test]
    fn numbering_stops_at_nine() {
        let l: Vec<LiveInfo> = (0..11).map(|i| live(i, "a", i, None)).collect();
        let rows = build(&l, &[], &[], &SideState::default());
        let numbered = rows.iter().filter(|r| matches!(r, Row::Live { n: Some(_), .. })).count();
        assert_eq!(numbered, 9);
    }
}
