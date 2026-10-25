//! The agent team board (prefix M, its members): the team the focused Claude session leads (Claude Code's agent teams,
//! [`bro_core::agent_teams`]) — who's on it, what each teammate is doing, and the task list they share — kept
//! live from the session's transcript worker. Enter goes to the lead (Claude's own agent panel picks a teammate
//! and talks to it); s opens agent teams in the settings.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::layout::PaneId;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::agent_teams::{Member, Task, TaskStatus, Team};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::Span,
};
use std::sync::Arc;

/// What a prompt to the lead looks like, for the empty board.
pub const EXAMPLE: &str = "Spawn three teammates to review this branch: one on security, one on performance, one on tests.";

/// One selectable line: a member or a task (indices into the team's lists).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Member(usize),
    Task(usize),
}

/// Where a task sits on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane {
    Active,
    Next,
    Blocked,
    Done,
}

impl Lane {
    pub fn of(team: &Team, t: &Task) -> Lane {
        match t.status {
            TaskStatus::InProgress => Lane::Active,
            TaskStatus::Completed => Lane::Done,
            TaskStatus::Pending if team.blocked(t) => Lane::Blocked,
            TaskStatus::Pending => Lane::Next,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Lane::Active => "in progress",
            Lane::Next => "up next",
            Lane::Blocked => "blocked",
            Lane::Done => "done",
        }
    }
}

/// The board's lines: the members (lead first), then the tasks by lane (in id order within one).
pub fn items(team: &Team) -> Vec<Item> {
    let mut tasks: Vec<usize> = (0..team.tasks.len()).collect();
    tasks.sort_by_key(|&i| Lane::of(team, &team.tasks[i]));
    (0..team.members.len()).map(Item::Member).chain(tasks.into_iter().map(Item::Task)).collect()
}

/// A member's status in words: its task's "Running the tests", "working", "idle".
pub fn doing(team: &Team, m: &Member) -> String {
    if m.lead {
        return "lead · coordinates the team".into();
    }
    let task = team.task_of(m);
    match (m.active, task) {
        (true, Some(t)) if t.status == TaskStatus::InProgress => t.active_form.clone().unwrap_or_else(|| t.subject.clone()),
        (true, _) => "working".into(),
        (false, Some(t)) if t.status == TaskStatus::InProgress => format!("idle · on #{} {}", t.id, t.subject),
        (false, _) => "idle".into(),
    }
}

/// "security-reviewer · sonnet · plans first": what the member was spawned as.
pub fn kind(m: &Member) -> String {
    let mut parts: Vec<String> = vec![];
    parts.extend(m.agent_type.clone());
    parts.extend(m.model.clone());
    if m.plan_mode {
        parts.push("plans first".into());
    }
    parts.join(" · ")
}

/// A task's owner as a member's name (owners can be written `name@team`).
pub fn owner_name<'a>(team: &'a Team, t: &'a Task) -> Option<&'a str> {
    let o = t.owner.as_deref()?;
    Some(team.member(o).map(|m| m.name.as_str()).unwrap_or(o))
}

/// The colour a member is drawn in: Claude's for it, else the theme's.
pub fn member_color(m: &Member, t: &Theme) -> Color {
    match m.rgb() {
        Some((r, g, b)) => Color::Rgb(r, g, b),
        None if m.lead => ui::harness_color(Some(bro_core::Harness::Claude), t),
        None => t.inline,
    }
}

pub enum Outcome {
    None,
    Close,
    /// go to the lead session
    Go,
    /// open agent teams in the settings
    Settings,
}

pub struct TeamView {
    pub pane: PaneId,
    /// the lead session's name
    pub title: String,
    /// The team as last read (the app refreshes it every frame from the session's transcript worker).
    pub team: Option<Arc<Team>>,
    /// Agent teams are on for new sessions.
    pub enabled: bool,
    /// Why there's no board (not a Claude session).
    pub why: Option<String>,
    pub sel: usize,
    scroll: usize,
    /// (row rect, item index) from the last draw
    hits: Vec<(Rect, usize)>,
    /// list rows that fit, from the last draw
    rows: usize,
}

impl TeamView {
    pub fn new(pane: PaneId, title: String, team: Option<Arc<Team>>, enabled: bool, why: Option<String>) -> TeamView {
        // on the first teammate: the lead is the session you came from
        let sel = usize::from(team.as_deref().is_some_and(|t| !t.is_empty()));
        TeamView { pane, title, team, enabled, why, sel, scroll: 0, hits: vec![], rows: 10 }
    }

    /// The board's lines (empty while there's no team).
    pub fn items(&self) -> Vec<Item> {
        self.team.as_deref().map(items).unwrap_or_default()
    }

    /// No teammates and no tasks: the board explains instead.
    pub fn is_empty(&self) -> bool {
        self.team.as_deref().is_none_or(|t| t.is_empty() && t.tasks.is_empty())
    }

    /// What to say when the board [`is_empty`](Self::is_empty).
    pub fn empty_text(&self) -> String {
        match (&self.why, &self.team) {
            (Some(why), _) => why.clone(),
            (None, Some(_)) => format!("No teammates yet. Ask Claude for some, and they show up here as they join:\n“{EXAMPLE}”"),
            (None, None) if !self.enabled => "Agent teams are off. Turn them on to let Claude spawn teammates in new sessions.".into(),
            (None, None) => "This session has no team. Sessions started since agent teams went on get one: start a new Claude session, or reload this one.".into(),
        }
    }

    pub(crate) fn move_by(&mut self, d: isize) {
        let n = self.items().len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as isize + d).clamp(0, n as isize - 1) as usize;
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let page = self.rows.max(1) as isize;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Home | KeyCode::Char('g') => self.sel = 0,
            KeyCode::End | KeyCode::Char('G') => self.sel = self.items().len().saturating_sub(1),
            KeyCode::Enter if self.why.is_none() => return Outcome::Go,
            KeyCode::Char('s') => return Outcome::Settings,
            _ => {}
        }
        Outcome::None
    }

    pub fn mouse(&mut self, m: MouseEvent) -> Outcome {
        let pos = Position { x: m.column, y: m.row };
        match m.kind {
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, i)) = self.hits.iter().find(|(r, _)| r.contains(pos)) {
                    self.sel = i;
                }
            }
            _ => {}
        }
        Outcome::None
    }

    /// How many rows the list shows (PgUp / PgDn move by that many).
    pub(crate) fn set_rows(&mut self, rows: usize) {
        self.rows = rows.max(1);
    }
}

/// `n` of `of` as a bar `w` cells wide: filled, then thin.
fn bar(n: usize, of: usize, w: usize, t: &Theme) -> Vec<Span<'static>> {
    let full = (n * w + of / 2).checked_div(of).unwrap_or(0).min(w);
    vec![Span::styled("━".repeat(full), fg(t.good)), Span::styled("─".repeat(w - full), fg(t.frame))]
}

/// One line of the drawn board.
enum Line {
    Gap,
    /// "TEAMMATES  3 · 2 working" / "TASKS  3 of 5 done ━━━──"
    Head(&'static str),
    Lane(Lane, usize),
    Item(usize),
    /// the selected task's description, one wrapped line of it
    Detail(String),
}

pub fn draw(f: &mut Frame, area: Rect, v: &mut TeamView, time: f64, t: &Theme) {
    v.hits.clear();
    let list = v.items();
    let w = area.width.saturating_sub(6).clamp(44, 110);
    let want = if v.is_empty() { 12 } else { list.len() as u16 + 14 };
    let h = want.clamp(10, area.height.saturating_sub(2).max(10));
    let inner = ui::popup(f, area, w, h, &format!("{}agent team · {}", ui::lead("team"), v.title), t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let mut hints = vec![];
    if v.why.is_none() {
        hints.push(("⏎", "go to the lead"));
    }
    hints.extend([("s", "team settings"), ("↑↓", "move"), ("esc", "close")]);
    let body = ui::hint_line(f, inner, &hints, t);
    let width = body.width as usize;
    let Some(team) = v.team.clone().filter(|_| !v.is_empty()) else {
        let mut y = body.y + 1;
        for (n, para) in v.empty_text().split('\n').enumerate() {
            let style = if n == 0 { muted(t) } else { fg(t.inline).add_modifier(Modifier::ITALIC) };
            for l in crate::views::settings::wrap(para, width.saturating_sub(4)) {
                if y >= body.bottom() {
                    return;
                }
                ui::line(f, Rect { y, height: 1, ..body }, vec![Span::raw("  "), Span::styled(l, style)]);
                y += 1;
            }
            y += 1;
        }
        return;
    };
    if v.sel >= list.len() {
        v.sel = list.len().saturating_sub(1);
    }

    // members, then the tasks lane by lane
    let mut lines = vec![Line::Head("teammates")];
    let mut lane = None;
    for (i, it) in list.iter().enumerate() {
        if let Item::Task(ti) = it {
            let l = Lane::of(&team, &team.tasks[*ti]);
            if lane.is_none() {
                lines.extend([Line::Gap, Line::Head("tasks")]);
            }
            if lane != Some(l) {
                lane = Some(l);
                lines.push(Line::Lane(l, team.tasks.iter().filter(|x| Lane::of(&team, x) == l).count()));
            }
        }
        lines.push(Line::Item(i));
        if i == v.sel
            && let Item::Task(ti) = it
        {
            let d = team.tasks[*ti].description.trim();
            lines.extend(crate::views::settings::wrap(&d.replace('\n', " "), width.saturating_sub(10)).into_iter().take(3).map(Line::Detail));
        }
    }

    let room = body.height as usize;
    v.rows = room.saturating_sub(2).max(1);
    let first = lines.iter().position(|l| matches!(l, Line::Item(i) if *i == v.sel)).unwrap_or(0);
    let last = lines.iter().rposition(|l| matches!(l, Line::Item(i) if *i == v.sel) || matches!(l, Line::Detail(_))).filter(|&l| l >= first).unwrap_or(first);
    // the selection and its details in view, the line above it too (its lane) when it moves up
    if first < v.scroll + 1 {
        v.scroll = first.saturating_sub(1);
    } else if last >= v.scroll + room {
        v.scroll = (last + 1 - room).min(first);
    }
    let counts = team.counts();
    let mates = team.teammates().count();
    for (row, l) in lines.iter().enumerate().skip(v.scroll).take(room) {
        let r = Rect { y: body.y + (row - v.scroll) as u16, height: 1, ..body };
        match l {
            Line::Gap => {}
            Line::Head("teammates") => {
                let mut spans = vec![Span::styled("TEAMMATES", ui::bold()), Span::styled(format!("  {mates}"), muted(t))];
                if team.working() > 0 {
                    spans.push(Span::styled(format!(" · {} working", team.working()), fg(t.accent)));
                }
                ui::line(f, r, spans);
            }
            Line::Head(_) => {
                let label = format!("  {} of {} done ", counts.done, counts.total);
                let bw = width.saturating_sub(ui::width("TASKS") + ui::width(&label) + 1).min(24);
                let mut spans = vec![Span::styled("TASKS", ui::bold()), Span::styled(label, muted(t))];
                spans.extend(bar(counts.done, counts.total, bw, t));
                ui::line(f, r, spans);
            }
            Line::Lane(l, n) => {
                let c = match l {
                    Lane::Active => t.accent,
                    Lane::Blocked => crate::theme::mix(t.danger, t.muted, 0.4),
                    Lane::Done => t.good,
                    Lane::Next => t.muted,
                };
                ui::line(f, r, vec![Span::raw("  "), Span::styled(l.label().to_string(), fg(c).add_modifier(Modifier::BOLD)), Span::styled(format!("  {n}"), muted(t))]);
            }
            Line::Detail(text) => {
                ui::line(f, r, vec![Span::styled("▌", ui::accent(t)), Span::raw("        "), Span::styled(text.clone(), muted(t))]);
            }
            Line::Item(i) => {
                let on = *i == v.sel;
                let lead = Span::styled(if on { "▌" } else { " " }, ui::accent(t));
                let (left, right) = match list[*i] {
                    Item::Member(mi) => member_line(&team, &team.members[mi], on, width, time, t),
                    Item::Task(ti) => task_line(&team, &team.tasks[ti], on, t),
                };
                let mut left = left;
                left.insert(0, lead);
                ui::line_lr(f, r, left, right);
                v.hits.push((r, *i));
            }
        }
    }
}

fn member_line(team: &Team, m: &Member, on: bool, width: usize, time: f64, t: &Theme) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let c = member_color(m, t);
    let (dot, dot_c) = match (m.lead, m.active) {
        (true, _) => ("★".to_string(), c),
        (false, true) => crate::panes::term::status_look(Some(crate::pane::Activity::Working), false, t, time).map(|(g, _, c)| (g.to_string(), c)).unwrap_or(("●".into(), t.accent)),
        (false, false) => ("○".to_string(), t.muted),
    };
    let name_style = if on { fg(c).add_modifier(Modifier::BOLD) } else { fg(c) };
    let mut left = vec![Span::styled(format!(" {dot} "), fg(dot_c).add_modifier(Modifier::BOLD)), Span::styled(format!("{:<14}", ui::fit(&m.name, 14)), name_style)];
    let what = doing(team, m);
    let style = if m.active && !m.lead { Style::default().fg(t.fg) } else { muted(t) };
    let kind = kind(m);
    let room = width.saturating_sub(22 + ui::width(&kind).min(28));
    left.push(Span::styled(format!(" {}", ui::fit(&what, room)), style));
    (left, vec![Span::styled(format!("{} ", ui::fit(&kind, 28)), muted(t))])
}

fn task_line(team: &Team, task: &Task, on: bool, t: &Theme) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let lane = Lane::of(team, task);
    let (mark, c) = match lane {
        Lane::Active => ("◐", t.accent),
        Lane::Next => ("○", t.muted),
        Lane::Blocked => ("⧗", crate::theme::mix(t.danger, t.muted, 0.4)),
        Lane::Done => ("✓", t.good),
    };
    let mut subject = if on { ui::bold() } else { Style::default().fg(t.fg) };
    if lane == Lane::Done {
        subject = muted(t);
    }
    let left = vec![Span::styled(format!("   {mark} "), fg(c).add_modifier(Modifier::BOLD)), Span::styled(format!("{:>3} ", task.id), muted(t)), Span::styled(task.subject.clone(), subject)];
    let mut right = vec![];
    if lane == Lane::Blocked {
        right.push(Span::styled(format!("after #{} ", task.blocked_by.join(", #")), fg(crate::theme::mix(t.danger, t.muted, 0.4))));
    }
    match owner_name(team, task).and_then(|o| team.member(o).map(|m| (o, member_color(m, t))).or(Some((o, t.inline)))) {
        Some((o, oc)) => right.push(Span::styled(format!("{o} "), fg(oc))),
        None if lane != Lane::Done => right.push(Span::styled("unclaimed ", muted(t))),
        None => {}
    }
    (left, right)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bro_core::agent_teams::Member;

    pub(crate) fn team() -> Team {
        let member = |name: &str, lead: bool, active: bool, color: Option<&str>| Member {
            name: name.into(),
            agent_id: format!("{name}@session-1"),
            color: color.map(String::from),
            lead,
            active,
            ..Member::default()
        };
        let task = |id: &str, subject: &str, status: TaskStatus, owner: Option<&str>, after: &[&str]| Task {
            id: id.into(),
            subject: subject.into(),
            description: format!("{subject}, carefully."),
            active_form: Some(format!("{subject}…")),
            owner: owner.map(String::from),
            status,
            blocked_by: after.iter().map(|s| s.to_string()).collect(),
        };
        Team {
            name: "session-1".into(),
            members: vec![member("team-lead", true, false, None), member("researcher", false, true, Some("blue")), Member { agent_type: Some("security-reviewer".into()), model: Some("sonnet".into()), ..member("auditor", false, false, Some("green")) }],
            tasks: vec![
                task("1", "Map the auth flow", TaskStatus::Completed, Some("researcher"), &[]),
                task("2", "Audit token handling", TaskStatus::InProgress, Some("researcher"), &["1"]),
                task("3", "Write the report", TaskStatus::Pending, None, &["2"]),
                task("4", "Check rate limits", TaskStatus::Pending, Some("auditor@session-1"), &[]),
            ],
        }
    }

    #[test]
    fn members_then_tasks_by_lane() {
        let t = team();
        assert_eq!(items(&t), vec![Item::Member(0), Item::Member(1), Item::Member(2), Item::Task(1), Item::Task(3), Item::Task(2), Item::Task(0)]);
        assert_eq!(Lane::of(&t, &t.tasks[2]), Lane::Blocked);
        assert_eq!(doing(&t, &t.members[1]), "Audit token handling…");
        assert_eq!(doing(&t, &t.members[2]), "idle");
        assert_eq!(kind(&t.members[2]), "security-reviewer · sonnet");
        assert_eq!(owner_name(&t, &t.tasks[3]), Some("auditor"));
    }

    #[test]
    fn keys_move_go_and_open_settings() {
        let mut v = TeamView::new(1, "lead".into(), Some(Arc::new(team())), true, None);
        assert!(!v.is_empty());
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Char('G'))), Outcome::None));
        assert_eq!(v.sel, 6);
        v.move_by(5);
        assert_eq!(v.sel, 6, "stays on the last line");
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Enter)), Outcome::Go));
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Char('s'))), Outcome::Settings));
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Esc)), Outcome::Close));
    }

    #[test]
    fn an_empty_board_says_why() {
        let off = TeamView::new(1, "x".into(), None, false, None);
        assert!(off.is_empty() && off.empty_text().contains("off"));
        let lead_only = Team { members: vec![team().members[0].clone()], ..Team::default() };
        let solo = TeamView::new(1, "x".into(), Some(Arc::new(lead_only)), true, None);
        assert!(solo.is_empty() && solo.empty_text().contains(EXAMPLE));
        let mut shell = TeamView::new(1, "x".into(), None, true, Some("a shell has no team".into()));
        assert_eq!(shell.empty_text(), "a shell has no team");
        assert!(matches!(shell.key(KeyEvent::from(KeyCode::Enter)), Outcome::None), "nothing to go to");
    }
}
