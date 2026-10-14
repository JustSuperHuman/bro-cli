//! Quick Install view: a checklist of the tools a coding machine wants — JustTools first, JustWatch on Windows, JustImagine, then Git,
//! Node.js, Bun, Python, the .NET SDKs, the agent CLIs and a few utilities — with what's already
//! installed. JustTools installs in the background like in settings; the rest run in a new tab
//! (`bro install --wait …`) so each installer's progress and its password / UAC prompts are visible.
//! Once a JustWatch installed from here shows up, its skill is switched on for new sessions; once JustImagine does,
//! bro looks at it again so settings show its version.

use super::settings::{spinner, step_label};
use crate::alerts::Kind;
use crate::pane::{Action, Cx, Pane, Place};
use crate::panes::term::{Meta, Spawn, Term};
use crate::services::Services;
use crate::services::justtools::JobKind;
use crate::ui::{self, fg, muted};
use bro_core::quick_install::{self, Os, Status, Tool};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use parking_lot::Mutex;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often to look again while installs started here may still be running.
const RECHECK: Duration = Duration::from_secs(3);
/// Stop looking again after this long.
const WATCH_FOR: Duration = Duration::from_secs(15 * 60);

/// A tab running `bro install --wait <ids>`: the installers' output and prompts, kept open at the end.
pub fn install_term(svc: &Services, cwd: PathBuf, ids: &[&str]) -> Term {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "bro".into());
    let names: Vec<&str> = ids.iter().copied().filter(|i| !i.starts_with('-')).collect();
    let label = if names.len() == 1 { format!("install {}", names[0]) } else { "quick install".into() };
    let meta = Meta {
        sid: uuid::Uuid::new_v4().to_string(),
        harness: None,
        profile: None,
        store: None,
        model: None,
        label,
        name: None,
        renamed: false,
        project: svc.project_for(&cwd),
        cwd,
        started: Instant::now(),
        route_id: None,
        cleanup: vec![],
        caveman: None,
    };
    let args = ["install", "--wait"].into_iter().chain(ids.iter().copied()).map(String::from).collect();
    Term::new(meta, Spawn { program: exe, args, ..Spawn::default() }, svc.clone())
}

/// What a background look found.
struct Found {
    status: Status,
    os: Os,
    how: HashMap<Tool, String>,
}

/// A selectable row.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Tool(Tool),
    Install,
}

fn rows() -> Vec<Row> {
    Tool::here().into_iter().map(Row::Tool).chain([Row::Install]).collect()
}

#[derive(Default)]
pub struct QuickInstallView {
    sel: usize,
    scroll: usize,
    /// (screen row, selectable row) from the last render, for clicks
    hits: Vec<(u16, usize)>,
    /// what's ticked; None until the first look picks the missing recommended ones
    picked: Option<HashSet<Tool>>,
    found: Option<Found>,
    /// a background look is running; its result lands here
    pending: Option<Arc<Mutex<Option<Found>>>>,
    checked_at: Option<Instant>,
    /// installs were started here at this moment: keep looking for a while
    watching: Option<Instant>,
    /// a JustTools job is running: keep its spinner moving
    busy: bool,
    /// what the last install started here asked for
    started: HashSet<Tool>,
}

impl QuickInstallView {
    pub fn new() -> QuickInstallView {
        QuickInstallView::default()
    }

    fn row(&self) -> Row {
        let all = rows();
        all[self.sel.min(all.len() - 1)]
    }

    /// Look at the machine on a background thread.
    fn check(&mut self, cx: &Cx) {
        if self.pending.is_some() {
            return;
        }
        self.checked_at = Some(Instant::now());
        if cx.svc.is_demo() {
            // the demo never looks at the machine: nothing is installed
            let os = Os::Windows { winget: true };
            let how = Tool::ALL.into_iter().map(|t| (t, "the demo doesn't install anything".to_string())).collect();
            self.found = Some(Found { status: Status::new(), os, how });
            self.pick_defaults();
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        self.pending = Some(slot.clone());
        let waker = cx.waker();
        std::thread::spawn(move || {
            let status = quick_install::detect();
            let os = Os::detect();
            let how = Tool::ALL.into_iter().map(|t| (t, quick_install::how(t, &os))).collect();
            *slot.lock() = Some(Found { status, os, how });
            waker.wake();
        });
    }

    /// The first look ticks the recommended tools that are missing.
    fn pick_defaults(&mut self) {
        if self.picked.is_none() {
            let status = self.found.as_ref().map(|f| &f.status);
            self.picked = Some(Tool::here().into_iter().filter(|t| t.recommended() && !status.is_some_and(|s| s.contains_key(t))).collect());
        }
    }

    fn installed(&self, t: Tool, cx: &Cx) -> bool {
        if t == Tool::JustTools && cx.svc.state().justtools.installed.is_some() {
            return true;
        }
        self.found.as_ref().is_some_and(|f| f.status.contains_key(&t))
    }

    /// Ticked and not installed yet, in install order.
    fn to_install(&self, cx: &Cx) -> Vec<Tool> {
        let picked = self.picked.clone().unwrap_or_default();
        Tool::here().into_iter().filter(|t| picked.contains(t) && !self.installed(*t, cx)).collect()
    }

    fn toggle(&mut self, t: Tool, cx: &mut Cx) {
        if self.installed(t, cx) {
            let how = if matches!(t, Tool::JustTools | Tool::JustImagine) { "settings update it" } else { "`bro install --force` reinstalls it" };
            cx.toast(Kind::Info, format!("{} is already installed · {how}", t.name()));
            return;
        }
        let picked = self.picked.get_or_insert_with(HashSet::new);
        if !picked.remove(&t) {
            picked.insert(t);
        }
    }

    /// Tick every missing tool, or none.
    fn tick_all(&mut self, on: bool, cx: &Cx) {
        let all: HashSet<Tool> = if on { Tool::here().into_iter().filter(|t| !self.installed(*t, cx)).collect() } else { HashSet::new() };
        self.picked = Some(all);
    }

    fn install(&mut self, cx: &mut Cx) {
        if cx.svc.is_demo() {
            return cx.toast(Kind::Info, "the demo doesn't install anything");
        }
        let tools = self.to_install(cx);
        if tools.is_empty() {
            return cx.toast(Kind::Info, "nothing ticked that isn't installed already");
        }
        if tools.contains(&Tool::JustTools) && cx.svc.state().justtools.job.is_none() {
            cx.svc.install_justtools();
        }
        let rest: Vec<&str> = tools.iter().filter(|t| **t != Tool::JustTools).map(|t| t.id()).collect();
        if !rest.is_empty() {
            let cwd = std::env::current_dir().unwrap_or_default();
            cx.act(Action::Open(Box::new(install_term(cx.svc, cwd, &rest)), Place::Tab));
        }
        self.started.extend(tools);
        self.watching = Some(Instant::now());
    }

    /// Enter / space / click on the selected row.
    fn activate(&mut self, cx: &mut Cx) {
        match self.row() {
            Row::Tool(t) => self.toggle(t, cx),
            Row::Install => self.install(cx),
        }
    }
}

fn heading(text: &str, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    vec![Span::styled(format!("──── {text} "), muted(t).add_modifier(Modifier::BOLD))]
}

impl Pane for QuickInstallView {
    fn title(&self) -> String {
        "quick install".into()
    }
    fn icon(&self) -> &'static str {
        "settings"
    }
    fn view(&self) -> Option<&'static str> {
        Some("quick-install")
    }
    fn poll(&mut self, cx: &mut Cx) {
        if let Some(found) = self.pending.as_ref().and_then(|p| p.lock().take()) {
            self.pending = None;
            self.found = Some(found);
            self.pick_defaults();
            // JustWatch landed: new sessions get its skill (once)
            if self.started.contains(&Tool::JustWatch) && self.installed(Tool::JustWatch, cx) {
                self.started.remove(&Tool::JustWatch);
                if !cx.svc.settings().justwatch.skill {
                    cx.svc.set_justwatch_skill_on(true);
                }
            }
            if self.started.contains(&Tool::JustImagine) && self.installed(Tool::JustImagine, cx) {
                self.started.remove(&Tool::JustImagine);
                cx.svc.check_justimagine(true);
            }
        }
        if self.watching.is_some_and(|w| w.elapsed() > WATCH_FOR) {
            self.watching = None;
        }
        if self.watching.is_some() && self.checked_at.is_none_or(|c| c.elapsed() >= RECHECK) {
            self.check(cx);
        }
    }
    fn tick_every(&self) -> Option<Duration> {
        if self.busy {
            Some(Duration::from_millis(100))
        } else {
            self.watching.map(|_| RECHECK)
        }
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        if self.checked_at.is_none() {
            self.check(cx);
        }
        let area = ui::hint_line(f, area, &[("j/k", "move"), ("space", "tick"), ("i", "install"), ("a/n", "all / none"), ("r", "look again")], t);
        let all = rows();
        let jt = cx.svc.state().justtools.clone();
        self.busy = jt.job.is_some();
        let checking = self.found.is_none();
        let picked = self.picked.clone().unwrap_or_default();
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = vec![];
        lines.push((None, vec![
            Span::styled(" QUICK INSTALL ", ui::bold_accent(t)),
            Span::styled(format!("  {}", self.found.as_ref().map_or("looking at this machine…", |f| f.os.describe())), muted(t)),
        ]));
        let mut group = None;
        for (i, row) in all.iter().enumerate() {
            let Row::Tool(tool) = *row else { continue };
            if group != Some(tool.group()) {
                group = Some(tool.group());
                lines.push((None, vec![]));
                lines.push((None, heading(tool.group().label(), t)));
            }
            let sel = self.sel == i;
            let installed = self.installed(tool, cx);
            let jt_job = jt.job.as_ref().filter(|j| tool == Tool::JustTools && j.kind != JobKind::Skill);
            let (boxed, box_style) = match () {
                _ if jt_job.is_some() => (format!("[{}]", spinner(jt_job.unwrap().started)), fg(t.shine)),
                _ if installed => ("[✓]".to_string(), fg(t.good)),
                _ if picked.contains(&tool) => ("[x]".to_string(), fg(t.accent).add_modifier(Modifier::BOLD)),
                _ => ("[ ]".to_string(), muted(t)),
            };
            let detail = match () {
                _ if jt_job.is_some() => step_label(jt_job.unwrap().step).to_string(),
                _ if tool == Tool::JustTools && jt.installed.is_some() => jt.installed.as_ref().and_then(|i| i.version.clone()).unwrap_or_else(|| "installed".into()),
                _ if installed => match self.found.as_ref().and_then(|f| f.status.get(&tool)) {
                    // a version reads better than a path
                    Some(d) if d.starts_with(|c: char| c.is_ascii_digit()) => d.clone(),
                    _ => "installed".into(),
                },
                _ if checking => "checking…".into(),
                _ => "not installed".into(),
            };
            let name_style = match () {
                _ if sel => ui::bold(),
                _ if tool == Tool::JustTools => fg(t.shine).add_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
            let star = if tool.recommended() { "★" } else { " " };
            lines.push((Some(i), vec![
                Span::raw(format!("{} ", if sel { "▌" } else { " " })),
                Span::styled(boxed, box_style),
                Span::styled(format!(" {:<16}", tool.name()), name_style),
                Span::styled(star.to_string(), fg(t.shine)),
                Span::styled(format!(" {}", ui::pad(&detail, 16)), if installed { fg(t.inline) } else { muted(t) }),
                Span::styled(format!(" {}", tool.blurb()), muted(t)),
            ]));
            if tool == Tool::JustTools {
                if let Some(e) = jt.error.as_ref().filter(|_| jt_job.is_none()) {
                    lines.push((None, vec![Span::styled(format!("        ✗ {e}"), fg(t.danger))]));
                } else if let Some(done) = jt.done.as_ref().filter(|_| jt_job.is_none()) {
                    lines.push((None, vec![Span::styled(format!("        ✓ {done}"), fg(t.good))]));
                }
            }
        }

        lines.push((None, vec![]));
        let todo = self.to_install(cx);
        let i = all.len() - 1;
        let sel = self.sel == i;
        let (label, style) = if todo.is_empty() {
            ("  nothing ticked to install".to_string(), muted(t))
        } else {
            let mut names: Vec<&str> = todo.iter().take(6).map(|t| t.name()).collect();
            let more = format!("{} more", todo.len().saturating_sub(6));
            if todo.len() > 6 {
                names.push(&more);
            }
            (format!("  ▶ Install {} · {}", todo.len(), names.join(", ")), ui::bold_accent(t))
        };
        lines.push((Some(i), vec![Span::raw(if sel { "▌" } else { " " }.to_string()), Span::styled(label, if sel { style.add_modifier(Modifier::REVERSED) } else { style })]));
        lines.push((None, vec![Span::styled("    each installer runs in a new tab so you can follow it and answer its prompts", muted(t))]));
        if self.watching.is_some() {
            lines.push((None, vec![Span::styled("    open a new terminal or restart bro afterwards so new sessions find the new tools on PATH", muted(t))]));
        }
        if let (Row::Tool(tool), Some(found)) = (self.row(), &self.found)
            && let Some(how) = found.how.get(&tool)
        {
            lines.push((None, vec![]));
            lines.push((None, vec![Span::styled(format!("  {} · how: ", tool.name()), muted(t)), Span::styled(how.clone(), fg(t.inline))]));
        }

        // keep the selection on screen
        let room = area.height as usize;
        let at = lines.iter().position(|(s, _)| *s == Some(self.sel)).unwrap_or(0);
        if at < self.scroll {
            self.scroll = at.saturating_sub(2);
        } else if at >= self.scroll + room {
            self.scroll = at + 1 - room;
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(room));
        self.hits.clear();
        for (n, (sel, spans)) in lines.into_iter().skip(self.scroll).take(room).enumerate() {
            let r = Rect { y: area.y + n as u16, height: 1, ..area };
            let selected = sel == Some(self.sel);
            if let Some(s) = sel {
                self.hits.push((r.y, s));
            }
            let spans = if selected { spans.into_iter().map(|s| Span::styled(s.content, s.style.bg(t.frame))).collect() } else { spans };
            ui::line(f, r, spans);
        }
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        let last = rows().len() - 1;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.sel = (self.sel + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.sel = last,
            KeyCode::Char(' ' | 'x') | KeyCode::Enter => self.activate(cx),
            KeyCode::Char('i') => self.install(cx),
            KeyCode::Char('a') => self.tick_all(true, cx),
            KeyCode::Char('n') => self.tick_all(false, cx),
            KeyCode::Char('r') => {
                self.check(cx);
                cx.svc.check_justtools();
            }
            _ => return false,
        }
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        let last = rows().len() - 1;
        match ev.kind {
            MouseEventKind::ScrollDown => self.sel = (self.sel + 1).min(last),
            MouseEventKind::ScrollUp => self.sel = self.sel.saturating_sub(1),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, s)) = self.hits.iter().find(|(y, _)| *y == ev.row) {
                    self.sel = s;
                    self.activate(cx);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_every_tool_then_the_install_button() {
        let r = rows();
        assert_eq!(r[0], Row::Tool(Tool::JustTools));
        assert_eq!(r.len(), Tool::here().len() + 1);
        assert_eq!(*r.last().unwrap(), Row::Install);
    }
}
