//! The sidebar model: a pure function from live sessions + past sessions + UI state (filter) to the rows
//! the sidebar draws and navigates. No I/O, no drawing.
//!
//! Order: the orchestrator (when one runs) pinned on top, then projects with open sessions first,
//! preserving project order within each group. Earlier sessions aren't rows: Continue is available in
//! the session dialog.
//!
//! Compact (the window's sidebar, beside the session tabs): no session rows, only the sessions that need you (a
//! question to answer, a permission to give), newest first, then the projects with sessions running, and the rest
//! folded into Recent (the last used first) until you open it.

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

/// What a session still has at work once its own turn is over ([`LiveInfo::background`]): "1 subagent still
/// working", "2 background tasks still running", "1 subagent and 2 background tasks still working".
pub fn background_words(subs: usize, tasks: usize) -> String {
    let n = |k: usize, one: &str| if k == 1 { format!("1 {one}") } else { format!("{k} {one}s") };
    match (subs, tasks) {
        (s, 0) => format!("{} still working", n(s, "subagent")),
        (0, t) => format!("{} still running", n(t, "background task")),
        (s, t) => format!("{} and {} still working", n(s, "subagent"), n(t, "background task")),
    }
}

#[cfg(test)]
#[test]
fn background_work_reads_as_what_is_still_going() {
    assert_eq!(background_words(1, 0), "1 subagent still working");
    assert_eq!(background_words(0, 2), "2 background tasks still running");
    assert_eq!(background_words(2, 1), "2 subagents and 1 background task still working");
}

/// A live session, as the sidebar needs it.
#[derive(Clone, Debug, PartialEq)]
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
    /// its subagents and background tasks still at work (its own turn may be over)
    pub background: (usize, usize),
    /// a second line: `⏸ why it waits` / `↳ recap`
    pub note: Option<String>,
    /// when it started waiting on you (unix seconds), while it does
    pub waiting_since: Option<u64>,
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
#[derive(Clone, Debug, PartialEq)]
pub struct OpenInfo {
    pub key: String,
    pub name: String,
    pub root: PathBuf,
}

/// Sidebar UI state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SideState {
    pub filter: String,
    /// projects and what needs you only (the window's sidebar beside the session tabs)
    pub compact: bool,
    /// the compact list's Recent group is open: the projects with no session running
    pub recent: bool,
}

/// Where the compact list's projects with no session running are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recent {
    /// listed with the rest: nothing runs, or the filter is looking
    Listed,
    /// in the Recent group, open or folded away
    Folded { open: bool },
}

/// The projects with no session running, the last used first, and where the compact list puts them.
pub fn recent(live: &[LiveInfo], past: &[PastInfo], open: &[OpenInfo], st: &SideState) -> (Vec<OpenInfo>, Recent) {
    let (running, idle) = split(groups(live, past, open));
    let at = if running.is_empty() || !st.filter.trim().is_empty() { Recent::Listed } else { Recent::Folded { open: st.recent } };
    (idle.into_iter().map(|g| OpenInfo { key: g.key, name: g.name, root: g.root }).collect(), at)
}

/// Projects with sessions running, in their order, and the rest, the last used first (never used last).
fn split(gs: Vec<Group<'_>>) -> (Vec<Group<'_>>, Vec<Group<'_>>) {
    let (running, mut idle): (Vec<Group>, Vec<Group>) = gs.into_iter().partition(|g| !g.live.is_empty());
    idle.sort_by_key(|g| g.last_age.unwrap_or(u64::MAX));
    (running, idle)
}

/// The session waits on you: a permission to give, a question its turn ended on.
pub fn needs_you(l: &LiveInfo) -> bool {
    l.activity == Some(Activity::Blocked) || l.note.as_deref().is_some_and(|n| n.starts_with('⏸'))
}

/// What a session's tab calls it: the orchestrator; your name for it or its agent's title; else its login ·
/// model; else its agent, or "shell".
pub fn session_label(l: &LiveInfo) -> String {
    if l.orchestrator {
        return "Orchestrator".into();
    }
    if let Some(name) = l.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        return name.to_string();
    }
    let who = l.profile.as_deref().map(|p| p.split(':').next_back().unwrap_or(p).to_string()).unwrap_or_default();
    let model = l.model.as_deref().map(|m| crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m))).unwrap_or_default();
    let detail = [who, model].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    match l.harness {
        Some(_) if !detail.is_empty() => detail,
        Some(h) => h.label().to_string(),
        None => "shell".into(),
    }
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

/// What a session is searched by: its name, agent, login, model (and its note and project in the compact list).
fn session_hay(l: &LiveInfo) -> [&str; 5] {
    [if l.orchestrator { "orchestrator" } else { "" }, l.name.as_deref().unwrap_or(""), l.harness.map(|h| h.label()).unwrap_or("shell"), l.profile.as_deref().unwrap_or(""), l.model.as_deref().unwrap_or("")]
}

/// The rows to draw.
pub fn build(live: &[LiveInfo], past: &[PastInfo], open: &[OpenInfo], st: &SideState) -> Vec<Row> {
    let q = st.filter.trim().to_lowercase();
    let numbering: HashMap<PaneId, usize> = live_order(live, past, open).into_iter().enumerate().map(|(i, p)| (p, i + 1)).collect();
    if st.compact {
        let shown = recent(live, past, open, st).1 != Recent::Folded { open: false };
        return compact(live, past, open, &q, &numbering, shown);
    }
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
            .filter(|l| q.is_empty() || project_hit || matches(&session_hay(l), &q))
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

/// The compact rows: what needs you, newest first, then the projects with sessions running, then (when `recent`
/// shows them) the rest, the last used first (the filter finds a project by its name or by its sessions').
fn compact(live: &[LiveInfo], past: &[PastInfo], open: &[OpenInfo], q: &str, numbering: &HashMap<PaneId, usize>, recent: bool) -> Vec<Row> {
    let hit = |l: &LiveInfo| q.is_empty() || matches(&session_hay(l), q) || matches(&[&l.project_name, l.note.as_deref().unwrap_or("")], q);
    let mut asks: Vec<&LiveInfo> = live.iter().filter(|l| needs_you(l) && hit(l)).collect();
    asks.sort_by_key(|l| (std::cmp::Reverse(l.waiting_since.unwrap_or(0)), l.seq));
    let mut rows: Vec<Row> = asks.into_iter().map(|l| Row::Live { info: l.clone(), n: numbering.get(&l.pane).copied().filter(|n| *n <= 9) }).collect();
    let found = groups(live, past, open).into_iter().filter(|g| q.is_empty() || matches(&[&g.name], q) || g.live.iter().any(|l| hit(l))).collect();
    let (running, mut idle) = split(found);
    if !recent {
        idle.clear();
    }
    for g in running.into_iter().chain(idle) {
        let attention = g.live.iter().any(|l| l.activity == Some(Activity::Blocked) || l.done);
        rows.push(Row::Project { key: g.key, name: g.name, root: g.root, live: g.live.len(), attention, last_age: g.last_age });
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
            background: (0, 0),
            note: None,
            waiting_since: None,
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
        assert!(tabs.iter().all(|t| t.blocked + t.done > 0));
    }

    #[test]
    fn a_session_tab_says_what_it_is_called_or_what_it_runs_on() {
        let mut l = live(1, "a", 1, None);
        assert_eq!(session_label(&l), "work · opus", "no name: its login and model");
        l.name = Some("  Fix the login bug ".into());
        assert_eq!(session_label(&l), "Fix the login bug");
        l.orchestrator = true;
        assert_eq!(session_label(&l), "Orchestrator");
        let mut bare = live(2, "a", 2, None);
        (bare.profile, bare.model) = (None, None);
        assert_eq!(session_label(&bare), "claude");
        bare.harness = None;
        assert_eq!(session_label(&bare), "shell");
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
        let st = SideState { filter: "orch".into(), ..Default::default() };
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "L9#1");
        let st = SideState { filter: "b".into(), ..Default::default() };
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "P:b L2#3");
    }

    #[test]
    fn compact_lists_what_needs_you_newest_first_then_the_projects() {
        let mut l = vec![live(1, "a", 1, Some(Activity::Working)), live(2, "b", 2, Some(Activity::Blocked)), live(3, "a", 3, Some(Activity::Idle)), live(4, "c", 4, Some(Activity::Idle))];
        l[1].waiting_since = Some(100);
        l[1].note = Some("⏸ Bash: git push".into());
        // its turn ended on a question
        l[2].note = Some("⏸ Which database should I use?".into());
        l[2].waiting_since = Some(200);
        // a recap isn't a question
        l[3].note = Some("↳ shipped the fix".into());
        l[3].done = true;
        let mut boss = live(9, "bro:orchestrator", 9, Some(Activity::Blocked));
        boss.orchestrator = true;
        l.push(boss);
        let open = opened(&["a", "b", "c", "d"]);
        let st = SideState { compact: true, ..Default::default() };
        let rows = build(&l, &[], &open, &st);
        assert_eq!(kinds(&rows), "L3#3 L2#4 L9#1 P:a P:b P:c", "the orchestrator asks too, but has no project row; d runs nothing, so it waits in Recent");
        assert!(matches!(&rows[3], Row::Project { live: 2, .. }));
        assert!(l.iter().filter(|l| needs_you(l)).count() == 3);
        // the filter: a project by its name or its sessions', what needs you by its question or project
        let st = SideState { compact: true, filter: "database".into(), ..Default::default() };
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "L3#3 P:a");
        let st = SideState { compact: true, filter: "push".into(), ..Default::default() };
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "L2#4 P:b");
        // the full list is unchanged
        assert_eq!(kinds(&build(&l, &[], &open, &SideState::default())), "L9#1 P:a L1#2 L3#3 P:b L2#4 P:c L4#5 P:d");
    }

    #[test]
    fn projects_with_nothing_running_wait_in_recent_the_last_used_first() {
        let names = |(idle, at): (Vec<OpenInfo>, Recent)| (idle.iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join(" "), at);
        let l = vec![live(1, "b", 1, None)];
        let p = vec![past(0, "a", "", 900), past(1, "c", "", 60), past(2, "z", "", 3000)];
        let open = opened(&["a", "b", "c", "z", "e"]);
        let mut st = SideState { compact: true, ..Default::default() };
        assert_eq!(kinds(&build(&l, &p, &open, &st)), "P:b", "folded: only what runs");
        assert_eq!(names(recent(&l, &p, &open, &st)), ("c a z e".into(), Recent::Folded { open: false }));
        st.recent = true;
        assert_eq!(kinds(&build(&l, &p, &open, &st)), "P:b P:c P:a P:z P:e", "open: the last used first, never used last");
        assert_eq!(recent(&l, &p, &open, &st).1, Recent::Folded { open: true });
        // the filter looks in Recent too, folded or not
        st.recent = false;
        st.filter = "z".into();
        assert_eq!(kinds(&build(&l, &p, &open, &st)), "P:z");
        assert_eq!(recent(&l, &p, &open, &st).1, Recent::Listed);
        // with nothing running there's nothing to fold them under
        st.filter.clear();
        assert_eq!(kinds(&build(&[], &p, &open, &st)), "P:c P:a P:z P:b P:e");
        assert_eq!(names(recent(&[], &p, &open, &st)), ("c a z b e".into(), Recent::Listed));
        // the full list is as it was
        assert_eq!(kinds(&build(&l, &p, &open, &SideState::default())), "P:b L1#1 P:a P:c P:z P:e");
    }

    #[test]
    fn numbering_stops_at_nine() {
        let l: Vec<LiveInfo> = (0..11).map(|i| live(i, "a", i, None)).collect();
        let rows = build(&l, &[], &[], &SideState::default());
        let numbered = rows.iter().filter(|r| matches!(r, Row::Live { n: Some(_), .. })).count();
        assert_eq!(numbered, 9);
    }
}
