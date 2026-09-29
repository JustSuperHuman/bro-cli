//! The sidebar model: a pure function from live sessions + past sessions + UI state (collapsed projects,
//! open "past" groups, filter) to the rows the sidebar draws and navigates. No I/O, no drawing.
//!
//! Order: projects with live sessions first (in the order their first session started, so rows don't jump),
//! then projects that only have past sessions, most recent first.

use crate::pane::{Activity, PaneId};
use bro_core::Harness;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// A live session, as the sidebar needs it.
#[derive(Clone, Debug)]
pub struct LiveInfo {
    pub pane: PaneId,
    pub harness: Option<Harness>,
    /// Your name for it, else the launch label's first part
    pub name: Option<String>,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub project_key: String,
    pub project_name: String,
    pub project_root: PathBuf,
    pub activity: Option<Activity>,
    /// finished while you weren't looking
    pub done: bool,
    /// seconds since launch
    pub age_secs: u64,
    /// launch order (smaller = earlier)
    pub seq: u64,
}

/// A resumable past session.
#[derive(Clone, Debug)]
pub struct PastInfo {
    /// index into the services' past-session list
    pub idx: usize,
    pub harness: Harness,
    pub profile: Option<String>,
    pub title: String,
    pub age_secs: u64,
    pub project_key: String,
    pub project_name: String,
    pub project_root: PathBuf,
}

/// Sidebar UI state.
#[derive(Clone, Debug, Default)]
pub struct SideState {
    /// project keys whose sessions are hidden
    pub collapsed: HashSet<String>,
    /// project keys whose "past" group is open
    pub past_open: HashSet<String>,
    pub filter: String,
}

/// One sidebar row.
#[derive(Clone, Debug)]
pub enum Row {
    Project { key: String, name: String, root: PathBuf, live: usize, past: usize, collapsed: bool, attention: bool },
    Live { info: LiveInfo, n: Option<usize> },
    PastHeader { key: String, count: usize, open: bool },
    Past { info: PastInfo },
}

impl Row {
    /// The project a row belongs to.
    pub fn project_key(&self) -> &str {
        match self {
            Row::Project { key, .. } | Row::PastHeader { key, .. } => key,
            Row::Live { info, .. } => &info.project_key,
            Row::Past { info } => &info.project_key,
        }
    }
}

/// Past-session rows shown per open project group.
pub const PAST_PER_PROJECT: usize = 8;
/// Projects shown that only have past sessions.
pub const PAST_PROJECTS: usize = 12;

struct Group<'a> {
    key: String,
    name: String,
    root: PathBuf,
    live: Vec<&'a LiveInfo>,
    past: Vec<&'a PastInfo>,
}

fn groups<'a>(live: &'a [LiveInfo], past: &'a [PastInfo]) -> Vec<Group<'a>> {
    let mut order: Vec<Group> = vec![];
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut live_sorted: Vec<&LiveInfo> = live.iter().collect();
    live_sorted.sort_by_key(|l| l.seq);
    for l in live_sorted {
        let i = *at.entry(l.project_key.clone()).or_insert_with(|| {
            order.push(Group { key: l.project_key.clone(), name: l.project_name.clone(), root: l.project_root.clone(), live: vec![], past: vec![] });
            order.len() - 1
        });
        order[i].live.push(l);
    }
    let with_live = order.len();
    let mut past_sorted: Vec<&PastInfo> = past.iter().collect();
    past_sorted.sort_by_key(|p| p.age_secs);
    for p in past_sorted {
        let i = match at.get(&p.project_key) {
            Some(&i) => i,
            None => {
                if order.len() - with_live >= PAST_PROJECTS {
                    continue;
                }
                order.push(Group { key: p.project_key.clone(), name: p.project_name.clone(), root: p.project_root.clone(), live: vec![], past: vec![] });
                at.insert(p.project_key.clone(), order.len() - 1);
                order.len() - 1
            }
        };
        order[i].past.push(p);
    }
    order
}

/// Live sessions in sidebar order (for alt+1..9 and next/prev), ignoring collapse and filter.
pub fn live_order(live: &[LiveInfo], past: &[PastInfo]) -> Vec<PaneId> {
    groups(live, past).iter().flat_map(|g| g.live.iter().map(|l| l.pane)).collect()
}

/// Project keys in sidebar order.
#[cfg(test)]
pub fn project_order(live: &[LiveInfo], past: &[PastInfo]) -> Vec<String> {
    groups(live, past).into_iter().map(|g| g.key).collect()
}

fn matches(hay: &[&str], q: &str) -> bool {
    hay.iter().any(|h| h.to_lowercase().contains(q))
}

/// The rows to draw.
pub fn build(live: &[LiveInfo], past: &[PastInfo], st: &SideState) -> Vec<Row> {
    let q = st.filter.trim().to_lowercase();
    let numbering: HashMap<PaneId, usize> = live_order(live, past).into_iter().enumerate().map(|(i, p)| (p, i + 1)).collect();
    let mut rows = vec![];
    for g in groups(live, past) {
        let project_hit = !q.is_empty() && matches(&[&g.name], &q);
        let live_rows: Vec<&LiveInfo> = g
            .live
            .iter()
            .copied()
            .filter(|l| q.is_empty() || project_hit || matches(&[l.name.as_deref().unwrap_or(""), l.harness.map(|h| h.label()).unwrap_or("shell"), l.profile.as_deref().unwrap_or(""), l.model.as_deref().unwrap_or("")], &q))
            .collect();
        let past_rows: Vec<&PastInfo> = g.past.iter().copied().filter(|p| q.is_empty() || project_hit || matches(&[&p.title, p.harness.label(), p.profile.as_deref().unwrap_or("")], &q)).collect();
        if !q.is_empty() && live_rows.is_empty() && past_rows.is_empty() && !project_hit {
            continue;
        }
        let collapsed = q.is_empty() && st.collapsed.contains(&g.key);
        let attention = g.live.iter().any(|l| l.activity == Some(Activity::Blocked) || l.done);
        rows.push(Row::Project { key: g.key.clone(), name: g.name.clone(), root: g.root.clone(), live: g.live.len(), past: g.past.len(), collapsed, attention });
        if collapsed {
            continue;
        }
        for l in live_rows {
            rows.push(Row::Live { info: l.clone(), n: numbering.get(&l.pane).copied().filter(|n| *n <= 9) });
        }
        if !past_rows.is_empty() {
            let open = st.past_open.contains(&g.key) || !q.is_empty();
            rows.push(Row::PastHeader { key: g.key.clone(), count: past_rows.len(), open });
            if open {
                for p in past_rows.into_iter().take(PAST_PER_PROJECT) {
                    rows.push(Row::Past { info: p.clone() });
                }
            }
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
            age_secs: 60,
            seq,
        }
    }

    fn past(idx: usize, project: &str, title: &str, age: u64) -> PastInfo {
        PastInfo { idx, harness: Harness::Codex, profile: Some("codex:local".into()), title: title.into(), age_secs: age, project_key: project.into(), project_name: project.into(), project_root: PathBuf::from(project) }
    }

    fn kinds(rows: &[Row]) -> String {
        rows.iter()
            .map(|r| match r {
                Row::Project { name, .. } => format!("P:{name}"),
                Row::Live { info, n } => format!("L{}#{}", info.pane, n.unwrap_or(0)),
                Row::PastHeader { count, open, .. } => format!("H{count}{}", if *open { "o" } else { "" }),
                Row::Past { info } => format!("p{}", info.idx),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn groups_live_first_then_recent_past() {
        let l = vec![live(10, "b", 2, None), live(11, "a", 1, Some(Activity::Blocked)), live(12, "b", 3, None)];
        let p = vec![past(0, "c", "old", 900), past(1, "b", "fix it", 100), past(2, "d", "newer", 50), past(3, "b", "other", 10)];
        let rows = build(&l, &p, &SideState::default());
        assert_eq!(kinds(&rows), "P:a L11#1 P:b L10#2 L12#3 H2 P:d H1 P:c H1");
        assert!(matches!(&rows[0], Row::Project { attention: true, live: 1, .. }));
        assert_eq!(live_order(&l, &p), vec![11, 10, 12]);
        assert_eq!(project_order(&l, &p), vec!["a", "b", "d", "c"]);
    }

    #[test]
    fn collapse_open_and_filter() {
        let l = vec![live(1, "a", 1, None), live(2, "b", 2, None)];
        let p = vec![past(0, "a", "port the loop", 10), past(1, "a", "fix tests", 20), past(2, "b", "qr pairing", 5)];
        let mut st = SideState::default();
        st.collapsed.insert("b".into());
        st.past_open.insert("a".into());
        assert_eq!(kinds(&build(&l, &p, &st)), "P:a L1#1 H2o p0 p1 P:b");
        // numbering ignores collapse: b's session is still #2
        st.collapsed.clear();
        assert_eq!(kinds(&build(&l, &p, &st)), "P:a L1#1 H2o p0 p1 P:b L2#2 H1");
        // filter: only matching past sessions, groups auto-open, collapse ignored
        st.collapsed.insert("a".into());
        st.filter = "QR".into();
        assert_eq!(kinds(&build(&l, &p, &st)), "P:b H1o p2");
        // filter on a project name shows all of it
        st.filter = "a".into();
        let rows = build(&l, &p, &st);
        assert!(kinds(&rows).starts_with("P:a L1#1 H2o p0 p1"), "{}", kinds(&rows));
        st.filter = "zzz".into();
        assert!(build(&l, &p, &st).is_empty());
    }

    #[test]
    fn numbering_stops_at_nine_and_past_is_capped() {
        let l: Vec<LiveInfo> = (0..11).map(|i| live(i, "a", i, None)).collect();
        let p: Vec<PastInfo> = (0..20).map(|i| past(i, "a", "t", i as u64)).collect();
        let mut st = SideState::default();
        st.past_open.insert("a".into());
        let rows = build(&l, &p, &st);
        let numbered = rows.iter().filter(|r| matches!(r, Row::Live { n: Some(_), .. })).count();
        assert_eq!(numbered, 9);
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Past { .. })).count(), PAST_PER_PROJECT);
        assert!(rows.iter().any(|r| matches!(r, Row::PastHeader { count: 20, .. })));
    }
}
