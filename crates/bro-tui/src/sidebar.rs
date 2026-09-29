//! The sidebar model: a pure function from live sessions + past sessions + UI state (collapsed projects,
//! open "more" lists, filter) to the rows the sidebar draws and navigates. No I/O, no drawing.
//!
//! Order: the projects you opened (your order), then projects that only have running sessions. Earlier
//! sessions aren't rows: "continue session" opens a picker for them.

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
    /// folded projects (projects start open)
    pub collapsed: HashSet<String>,
    pub filter: String,
}

impl SideState {
    pub fn is_collapsed(&self, key: &str) -> bool {
        self.collapsed.contains(key)
    }

    /// Fold or unfold a project.
    pub fn set_collapsed(&mut self, key: &str, fold: bool) {
        if fold {
            self.collapsed.insert(key.to_string());
        } else {
            self.collapsed.remove(key);
        }
    }
}

/// One sidebar row. Earlier sessions aren't listed — "continue session" opens a picker for them.
#[derive(Clone, Debug)]
pub enum Row {
    /// "+ new session" — always first
    New,
    /// "+ open project"
    OpenFolder,
    /// "↻ continue session" (when there are earlier sessions)
    Continue { count: usize },
    Project {
        key: String,
        name: String,
        root: PathBuf,
        live: usize,
        collapsed: bool,
        attention: bool,
        /// seconds since the newest earlier session
        last_age: Option<u64>,
    },
    Live { info: LiveInfo, n: Option<usize> },
}

impl Row {
    /// The project a row belongs to (empty for the action rows).
    pub fn project_key(&self) -> &str {
        match self {
            Row::New | Row::OpenFolder | Row::Continue { .. } => "",
            Row::Project { key, .. } => key,
            Row::Live { info, .. } => &info.project_key,
        }
    }
}

struct Group<'a> {
    key: String,
    name: String,
    root: PathBuf,
    live: Vec<&'a LiveInfo>,
    /// newest earlier session's age
    last_age: Option<u64>,
}

/// Opened projects in your order, then projects that only have running sessions (in launch order).
fn groups<'a>(live: &'a [LiveInfo], past: &[PastInfo], open: &[OpenInfo]) -> Vec<Group<'a>> {
    let mut order: Vec<Group> = open.iter().map(|o| Group { key: o.key.clone(), name: o.name.clone(), root: o.root.clone(), live: vec![], last_age: None }).collect();
    let mut at: HashMap<String, usize> = order.iter().enumerate().map(|(i, g)| (g.key.clone(), i)).collect();
    let mut live_sorted: Vec<&LiveInfo> = live.iter().collect();
    live_sorted.sort_by_key(|l| l.seq);
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
    order
}

/// Live sessions in sidebar order (for alt+1..9 and next/prev), ignoring collapse and filter.
pub fn live_order(live: &[LiveInfo], past: &[PastInfo], open: &[OpenInfo]) -> Vec<PaneId> {
    groups(live, past, open).iter().flat_map(|g| g.live.iter().map(|l| l.pane)).collect()
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
    if q.is_empty() {
        rows.push(Row::New);
        rows.push(Row::OpenFolder);
        if !past.is_empty() {
            rows.push(Row::Continue { count: past.len() });
        }
    }
    for g in groups(live, past, open) {
        let project_hit = !q.is_empty() && matches(&[&g.name], &q);
        let live_rows: Vec<&LiveInfo> = g
            .live
            .iter()
            .copied()
            .filter(|l| q.is_empty() || project_hit || matches(&[l.name.as_deref().unwrap_or(""), l.harness.map(|h| h.label()).unwrap_or("shell"), l.profile.as_deref().unwrap_or(""), l.model.as_deref().unwrap_or("")], &q))
            .collect();
        if !q.is_empty() && live_rows.is_empty() && !project_hit {
            continue;
        }
        let collapsed = q.is_empty() && st.is_collapsed(&g.key);
        let attention = g.live.iter().any(|l| l.activity == Some(Activity::Blocked) || l.done);
        rows.push(Row::Project { key: g.key.clone(), name: g.name.clone(), root: g.root.clone(), live: g.live.len(), collapsed, attention, last_age: g.last_age });
        if collapsed {
            continue;
        }
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
            age_secs: 60,
            seq,
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
                Row::New => "+".to_string(),
                Row::OpenFolder => "o".to_string(),
                Row::Continue { count } => format!("c{count}"),
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
        // earlier sessions aren't rows: one "continue" entry instead
        assert_eq!(kinds(&rows), "+ o c3 P:a P:b L10#1 P:x L11#2");
        assert!(matches!(&rows[3], Row::Project { last_age: Some(50), .. }));
        assert!(matches!(&rows[6], Row::Project { attention: true, .. }));
        assert_eq!(live_order(&l, &p, &opened(&["a", "b"])), vec![10, 11]);
        assert_eq!(project_order(&l, &p, &opened(&["a", "b"])), vec!["a", "b", "x"]);
        assert_eq!(kinds(&build(&l, &[], &opened(&["a"]), &SideState::default())), "+ o P:a P:x L11#1 P:b L10#2", "no continue row without history");
    }

    #[test]
    fn collapse_and_filter() {
        let l = vec![live(1, "a", 1, None), live(2, "b", 2, None)];
        let open = opened(&["a", "b"]);
        let mut st = SideState::default();
        st.set_collapsed("a", true);
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "+ o P:a P:b L2#2");
        st.filter = "b".into();
        assert_eq!(kinds(&build(&l, &[], &open, &st)), "P:b L2#2");
        st.filter = "zzz".into();
        assert!(build(&l, &[], &open, &st).is_empty());
    }

    #[test]
    fn numbering_stops_at_nine() {
        let l: Vec<LiveInfo> = (0..11).map(|i| live(i, "a", i, None)).collect();
        let rows = build(&l, &[], &[], &SideState::default());
        let numbered = rows.iter().filter(|r| matches!(r, Row::Live { n: Some(_), .. })).count();
        assert_eq!(numbered, 9);
    }
}
