//! Computers view: the computers on your Just Superhuman account (JustConnect). Which can run sessions right
//! now, what stands in the way of the ones that can't (link by link, with what fixes it), the sessions on the
//! one you pick, and where `bro ctl` runs.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::alerts::Kind;
use crate::pane::{Cx, Pane};
use crate::services::computers::{Computer, ComputersState, SESSIONS_STALE, STALE, Sessions};
use crate::ui::{self, fg, muted};
use bro_core::justconnect::health::{self, Check, Diagnosis, Hop};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};
use std::time::{Duration, Instant};

/// A look under way is only shown once it has taken this long, so one that comes straight back never flickers.
pub(crate) const SHOW_AFTER: Duration = Duration::from_millis(400);

/// The computers pane.
#[derive(Default)]
pub struct ComputersView {
    /// The computer picked, by id (the list reorders as computers come and go).
    pub(crate) picked: Option<String>,
    /// How far the detail side is scrolled, in lines (terminal UI) or pixels (desktop app).
    pub(crate) scroll: f32,
}

impl ComputersView {
    pub fn new() -> ComputersView {
        ComputersView::default()
    }

    /// The picked computer's place in `list` (the first when nothing is picked or it has gone).
    pub(crate) fn index(&self, list: &[Computer]) -> usize {
        self.picked.as_deref().and_then(|id| list.iter().position(|c| c.id == id)).unwrap_or(0)
    }

    pub(crate) fn pick(&mut self, list: &[Computer], i: usize) {
        if let Some(c) = list.get(i.min(list.len().saturating_sub(1))) {
            if self.picked.as_deref() != Some(c.id.as_str()) {
                self.scroll = 0.0;
            }
            self.picked = Some(c.id.clone());
        }
    }

    /// Keep what's shown fresh while the view is open: the list, and the picked computer's sessions.
    pub(crate) fn keep_fresh(&mut self, cx: &Cx) {
        let (look, sessions) = {
            let st = cx.svc.state();
            let c = &st.computers;
            // a look that failed is tried again half as often: the account being unreachable rarely fixes itself in seconds
            let every = if c.failure.is_some() { STALE * 2 } else { STALE };
            let look = c.loading.is_none() && (!c.checked || c.tried.is_none_or(|at| at.elapsed() >= every));
            let picked = c.list.get(self.index(&c.list)).filter(|x| x.health.ready() && !x.here);
            // a view without the keyboard still shows, so it still refreshes, less often
            let every = if cx.focused { SESSIONS_STALE } else { STALE };
            let sessions = picked.and_then(|x| match c.sessions.get(&x.id) {
                None | Some(Sessions::Unasked) => Some(x.id.clone()),
                Some(Sessions::Ready { at, refreshing: false, .. }) if at.elapsed() >= every => Some(x.id.clone()),
                Some(Sessions::Failed(_, at)) if at.elapsed() >= STALE => Some(x.id.clone()),
                _ => None,
            });
            (look, sessions)
        };
        if look {
            cx.svc.refresh_computers();
        }
        if let Some(id) = sessions {
            cx.svc.refresh_remote_sessions(&id);
        }
    }

    /// Choose the picked computer as where `bro ctl` runs (again: back to this computer).
    pub(crate) fn toggle_ctl(&mut self, cx: &mut Cx) {
        let target = {
            let st = cx.svc.state();
            let c = &st.computers;
            let Some(x) = c.list.get(self.index(&c.list)) else { return };
            if x.here || c.ctl.as_deref() == Some(x.id.as_str()) {
                // back to this computer (nothing to do when it already is)
                if c.ctl.is_none() {
                    return;
                }
                Ok(None)
            } else if x.runs_ctl() {
                Ok(Some(x.id.clone()))
            } else {
                Err(x.health.headline.clone())
            }
        };
        match target {
            Ok(target) => cx.svc.use_computer_for_ctl(target),
            // it can't run anything right now: say why instead of pointing `bro ctl` at it
            Err(why) => cx.toast(Kind::Info, why),
        }
    }

    pub(crate) fn open_desktop(&mut self, cx: &mut Cx) {
        let picked = {
            let st = cx.svc.state();
            st.computers.list.get(self.index(&st.computers.list)).cloned()
        };
        match picked {
            Some(x) if x.here => cx.toast(Kind::Info, "That's this computer: you're already looking at its desktop."),
            Some(x) if x.desktop_reachable() => cx.svc.open_computer_desktop(&x.id),
            Some(x) => cx.toast(Kind::Info, x.health.headline),
            None => {}
        }
    }

    /// Install or update Bro on the picked computer, when that can be done from here.
    pub(crate) fn install_bro(&mut self, cx: &mut Cx) {
        let id = {
            let st = cx.svc.state();
            st.computers.list.get(self.index(&st.computers.list)).filter(|x| x.bro_install().is_some()).map(|x| x.id.clone())
        };
        if let Some(id) = id {
            cx.svc.install_bro_on(&id);
        }
    }

    pub(crate) fn copy_id(&mut self, cx: &mut Cx) {
        let id = {
            let st = cx.svc.state();
            st.computers.list.get(self.index(&st.computers.list)).map(|x| (x.id.clone(), x.name.clone()))
        };
        if let Some((id, name)) = id {
            crate::clip::copy(&id);
            cx.toast(Kind::Info, format!("Copied {name}'s ID, for `bro computers` commands."));
        }
    }
}

/// "as of 12 s ago", "as of 3 min ago".
pub(crate) fn as_of(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    match s {
        0..=4 => "just now".into(),
        5..=89 => format!("{s} s ago"),
        _ => format!("{} min ago", s / 60),
    }
}

/// "0.4 s", "2.1 s", "38 s".
pub(crate) fn span(took: Duration) -> String {
    let seconds = took.as_secs_f64();
    if seconds < 10.0 { format!("{seconds:.1} s") } else { format!("{seconds:.0} s") }
}

/// The line under the title: who the list is for and how old it is, or why there is none.
pub(crate) fn summary(c: &ComputersState) -> String {
    if !c.checked {
        return "Looking for your computers".into();
    }
    if !c.signed_in {
        return "Sign in to Just Superhuman to see your computers".into();
    }
    let n = c.list.len();
    let count = match n {
        0 => "No computers on your account yet".to_string(),
        1 => "1 computer on your account".to_string(),
        n => format!("{n} computers on your account"),
    };
    match c.looked {
        Some(at) if n > 0 => format!("{count} · as of {}", as_of(at)),
        _ => count,
    }
}

/// A few words on one computer for its row in the list.
pub(crate) fn row_status(x: &Computer, c: &ComputersState) -> String {
    if !x.health.ready() {
        return x.health.detail.clone();
    }
    if x.here {
        return "this computer · ready".into();
    }
    match c.sessions.get(&x.id) {
        Some(Sessions::Ready { list, .. }) => match list.len() {
            0 => "ready · no sessions".into(),
            1 => "ready · 1 session".into(),
            n => format!("ready · {n} sessions"),
        },
        _ => "ready".into(),
    }
}

/// What the mark beside a session says about it: needs someone, working, or quiet.
pub(crate) fn session_mark(activity: Option<&str>) -> (&'static str, u8) {
    match activity {
        Some("awaiting") => ("●", 2),
        Some("working") => ("◐", 1),
        _ => ("○", 0),
    }
}

impl Pane for ComputersView {
    fn title(&self) -> String {
        "computers".into()
    }
    fn icon(&self) -> &'static str {
        "remote"
    }
    fn view(&self) -> Option<&'static str> {
        Some("computers")
    }
    fn tick_every(&self) -> Option<Duration> {
        Some(Duration::from_secs(2))
    }
    fn poll(&mut self, cx: &mut Cx) {
        self.keep_fresh(cx);
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        self.keep_fresh(cx);
        let t = cx.theme;
        let st = cx.svc.state();
        let c = &st.computers;
        let sel = self.index(&c.list);
        let picked = c.list.get(sel);
        let on_ctl = picked.is_some_and(|x| c.ctl.as_deref() == Some(x.id.as_str()));
        // only the keys that do something here
        let keys: &[(&str, &str)] = if c.list.is_empty() { &[("r", "look again")] } else { &[("j/k", "pick"), ("enter", if on_ctl { "bro ctl back here" } else { "run bro ctl there" }), ("d", "desktop"), ("c", "copy id"), ("r", "look again")] };
        let area = ui::hint_line(f, area, keys, t);
        let row = |y: u16| Rect { y, height: 1, ..area };
        let mut y = area.y;
        let mut head = vec![Span::styled(" COMPUTERS ", ui::bold_accent(t)), Span::styled(format!(" {}", summary(c)), muted(t))];
        if c.loading.is_some_and(|since| since.elapsed() >= SHOW_AFTER) {
            head.push(Span::styled("  ◌ looking", fg(t.shine)));
        }
        ui::line(f, row(y), head);
        y += 2;
        // the account couldn't be reached: say so above whatever was last known
        if let Some(d) = &c.failure {
            ui::line(f, row(y), vec![Span::styled(format!("  ! {}", d.headline), fg(t.danger).add_modifier(Modifier::BOLD))]);
            y += 1;
            for step in &d.steps {
                if y >= area.bottom() {
                    return;
                }
                ui::line(f, row(y), vec![Span::styled(format!("    {step}"), muted(t))]);
                y += 1;
            }
            y += 1;
        }
        if c.checked && c.list.is_empty() && c.failure.is_none() {
            let lines: &[&str] = if c.signed_in {
                &["  Add this computer under Settings, Remote Control, \"Reach this computer from your account\",", "  or run `bro computers enable`. Do the same on each computer you want to reach from here."]
            } else {
                &["  Run `bro jsh login`, or sign in from the Profiles view. Computers you add to your account", "  show up here, with their sessions."]
            };
            for l in lines {
                ui::line(f, row(y), vec![Span::styled(l.to_string(), muted(t))]);
                y += 1;
            }
            return;
        }
        let name_w = c.list.iter().map(|x| ui::width(&x.name)).max().unwrap_or(8).clamp(8, 32);
        for (i, x) in c.list.iter().enumerate() {
            if y >= area.bottom() {
                return;
            }
            let on = i == sel;
            let ready = x.health.ready();
            let mut spans = vec![
                Span::styled(if on { " ▌" } else { "  " }, ui::accent(t)),
                Span::styled(if ready { "● " } else { "○ " }, fg(if ready { t.good } else { t.muted })),
                Span::styled(ui::pad(&ui::fit(&x.name, name_w), name_w + 2), if on { ui::bold_accent(t) } else { ui::bold() }),
                Span::styled(ui::pad(&x.system, 9), muted(t)),
                Span::styled(row_status(x, c), if ready { fg(t.fg) } else { muted(t) }),
            ];
            if c.ctl.as_deref() == Some(x.id.as_str()) {
                spans.push(Span::styled("   bro ctl runs here", fg(t.shine)));
            }
            ui::line(f, row(y), spans);
            y += 1;
        }
        y += 1;
        let Some(x) = picked else { return };
        if y >= area.bottom() {
            return;
        }
        ui::rule(f, row(y), &x.name, t);
        y += 1;
        let mut lines: Vec<Vec<Span>> = vec![];
        if !x.health.ready() {
            lines.extend(diagnosis_lines(&x.health, t));
        } else if x.here {
            lines.push(vec![Span::styled("  Other computers on your account can start and follow sessions here while Bro is open.", muted(t))]);
        } else {
            match c.sessions.get(&x.id) {
                Some(Sessions::Ready { list, took, .. }) => {
                    lines.push(vec![Span::styled(format!("  Answers in {}", span(*took)), muted(t))]);
                    if list.is_empty() {
                        lines.push(vec![Span::styled("  No sessions are open there. Start one with `bro ctl open_session` after choosing this computer.", muted(t))]);
                    }
                    for s in list {
                        let (mark, level) = session_mark(s.activity.as_deref());
                        let color = [t.muted, t.shine, t.accent][level as usize];
                        lines.push(vec![
                            Span::styled(format!("  {mark} "), fg(color)),
                            Span::styled(ui::pad(&ui::fit(&s.title, 18), 20), ui::bold()),
                            Span::styled(ui::pad(s.agent.as_deref().unwrap_or("shell"), 8), muted(t)),
                            Span::styled(ui::pad(&ui::fit(s.project.as_deref().unwrap_or(""), 18), 20), muted(t)),
                            Span::styled(s.summary.clone(), fg(t.fg)),
                        ]);
                    }
                }
                Some(Sessions::Failed(d, _)) => lines.extend(diagnosis_lines(d, t)),
                Some(Sessions::Loading(since)) if since.elapsed() >= SHOW_AFTER => lines.push(vec![Span::styled("  ◌ Reading its sessions", fg(t.shine))]),
                _ => {}
            }
        }
        for note in &x.health.notes {
            lines.push(vec![]);
            lines.push(vec![Span::styled(format!("  {note}"), muted(t))]);
        }
        let room = area.bottom().saturating_sub(y) as usize;
        let skip = (self.scroll.max(0.0) as usize).min(lines.len().saturating_sub(room.max(1)));
        self.scroll = skip as f32;
        for line in lines.into_iter().skip(skip).take(room) {
            ui::line(f, row(y), line);
            y += 1;
        }
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        let (len, sel) = {
            let st = cx.svc.state();
            (st.computers.list.len(), self.index(&st.computers.list))
        };
        let pick = |v: &mut ComputersView, i: usize, cx: &Cx| {
            let st = cx.svc.state();
            v.pick(&st.computers.list, i);
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => pick(self, (sel + 1).min(len.saturating_sub(1)), cx),
            KeyCode::Char('k') | KeyCode::Up => pick(self, sel.saturating_sub(1), cx),
            KeyCode::PageDown => self.scroll += 6.0,
            KeyCode::PageUp => self.scroll = (self.scroll - 6.0).max(0.0),
            KeyCode::Char('r') => cx.svc.refresh_computers(),
            KeyCode::Enter | KeyCode::Char('u') => self.toggle_ctl(cx),
            KeyCode::Char('d') => self.open_desktop(cx),
            KeyCode::Char('c') => self.copy_id(cx),
            KeyCode::Char('i') => self.install_bro(cx),
            _ => return false,
        }
        true
    }
}

/// A diagnosis as terminal lines: the headline, each link with how it came out, then what to do.
fn diagnosis_lines<'a>(d: &Diagnosis, t: &crate::theme::Theme) -> Vec<Vec<Span<'a>>> {
    let name = d.name.as_deref();
    let width = Hop::ALL.iter().map(|h| ui::width(&h.label(name))).max().unwrap_or(0);
    let mut lines = vec![vec![Span::styled(format!("  {}", d.headline), ui::bold())], vec![]];
    for hop in Hop::ALL {
        let (mark, color, said) = match d.check(hop) {
            Check::Passed => ("✓", t.good, health::passed(hop).to_string()),
            Check::Failed => ("✗", t.danger, d.detail.clone()),
            Check::Unchecked => ("·", t.muted, "not reached".to_string()),
        };
        lines.push(vec![Span::styled(format!("    {mark} "), fg(color)), Span::styled(ui::pad(&hop.label(name), width + 2), fg(t.fg)), Span::styled(said, muted(t))]);
    }
    if !d.steps.is_empty() {
        lines.push(vec![]);
        for (i, step) in d.steps.iter().enumerate() {
            lines.push(vec![Span::styled(format!("  {}. ", i + 1), fg(t.shine)), Span::styled(step.clone(), fg(t.fg))]);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::computers::RemoteSession;
    use bro_core::justconnect::health;
    use serde_json::json;

    fn computer(id: &str, name: &str, here: bool, online: bool) -> Computer {
        let record = json!({"id":id,"name":name,"enabled":true,"provisioned":true,"broOnline":online,"broLastSeen":1_800_000_000_000i64 - 7_200_000});
        Computer { id: id.into(), name: name.into(), system: "Windows".into(), here, bro_version: None, health: health::of_computer(&record, None, 1_800_000_000_000, "2.1.48") }
    }

    #[test]
    fn the_pick_follows_the_computer_not_its_place_in_the_list() {
        let mut list = vec![computer("a", "Studio", true, true), computer("b", "Laptop", false, true), computer("c", "Build box", false, false)];
        let mut v = ComputersView::new();
        assert_eq!(v.index(&list), 0, "nothing picked: the first");
        v.pick(&list, 2);
        assert_eq!(v.picked.as_deref(), Some("c"));
        list.swap(1, 2);
        assert_eq!(v.index(&list), 1, "the list reordered: still Build box");
        list.remove(1);
        assert_eq!(v.index(&list), 0, "it left the account: back to the first");
        v.pick(&list, 99);
        assert_eq!(v.picked.as_deref(), Some("b"), "past the end picks the last");
    }

    #[test]
    fn a_row_says_in_a_few_words_where_the_computer_stands() {
        let mut c = ComputersState { checked: true, signed_in: true, list: vec![computer("a", "Studio", true, true), computer("b", "Laptop", false, true), computer("c", "Build box", false, false)], ..Default::default() };
        assert_eq!(row_status(&c.list[0], &c), "this computer · ready");
        assert_eq!(row_status(&c.list[1], &c), "ready");
        assert_eq!(row_status(&c.list[2], &c), "closed · last open 2 hr ago");
        c.sessions.insert("b".into(), Sessions::Ready { list: vec![RemoteSession::default(), RemoteSession::default()], took: Duration::from_millis(400), at: Instant::now(), refreshing: false });
        assert_eq!(row_status(&c.list[1], &c), "ready · 2 sessions");
    }

    #[test]
    fn the_summary_says_who_the_list_is_for_or_why_there_is_none() {
        let mut c = ComputersState::default();
        assert_eq!(summary(&c), "Looking for your computers");
        c.checked = true;
        assert_eq!(summary(&c), "Sign in to Just Superhuman to see your computers");
        c.signed_in = true;
        assert_eq!(summary(&c), "No computers on your account yet");
        c.list = vec![computer("a", "Studio", true, true)];
        c.looked = Some(Instant::now());
        assert_eq!(summary(&c), "1 computer on your account · as of just now");
        assert_eq!(span(Duration::from_millis(420)), "0.4 s");
        assert_eq!(span(Duration::from_secs(38)), "38 s");
    }
}
