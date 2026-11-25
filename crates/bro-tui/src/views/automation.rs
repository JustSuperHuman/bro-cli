//! Install and configure the browser and computer tools Bro supplies to every harness.
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::alerts::Kind;
use crate::pane::{Action, Cx, Pane, Place};
use crate::ui::{self, fg, muted};
use bro_core::quick_install::Tool;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use parking_lot::Mutex;
use ratatui::{Frame, layout::Rect, text::Span};
use std::sync::Arc;
use std::time::{Duration, Instant};

const B_INSTALL: usize = 0;
const B_ENABLE: usize = 1;
const B_FOLDER: usize = 2;
const B_CHECK: usize = 3;
const C_INSTALL: usize = 4;
const C_ENABLE: usize = 5;
const C_CHECK: usize = 6;
const C_GUIDE: usize = 7;
const COUNT: usize = 8;
type Found = [Option<String>; 2];
type CheckResult = Result<String, String>;

#[derive(Default)]
pub struct AutomationView {
    sel: usize,
    scroll: f32,
    reveal: bool,
    hits: Vec<(u16, usize)>,
    found: Option<Found>,
    pending: Option<Arc<Mutex<Option<Found>>>>,
    checked_at: Option<Instant>,
    checks: [Option<Arc<Mutex<Option<CheckResult>>>>; 2],
    notice: [Option<CheckResult>; 2],
}

impl AutomationView {
    pub fn new() -> Self {
        Self::default()
    }

    fn check(&mut self, cx: &Cx) {
        if self.pending.is_some() {
            return;
        }
        self.checked_at = Some(Instant::now());
        if cx.svc.is_demo() {
            self.found = Some([None, None]);
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        self.pending = Some(slot.clone());
        let waker = cx.waker();
        std::thread::spawn(move || {
            *slot.lock() = Some([
                bro_core::justbrowse::installed().map(|f| f.version.unwrap_or_else(|| "Installed".into())),
                bro_core::cua::installed().map(|p| p.display().to_string()),
            ]);
            waker.wake();
        });
    }

    fn installed(&self, i: usize) -> bool {
        self.found.as_ref().is_some_and(|f| f[i].is_some())
    }
    fn enabled(&self, i: usize, cx: &Cx) -> bool {
        let settings = cx.svc.settings().automation;
        if i == 0 { settings.justbrowse } else { settings.cua }
    }

    fn available(&self, n: usize) -> bool {
        match n {
            B_INSTALL => self.found.is_some(),
            C_INSTALL => self.found.is_some() && bro_core::cua::supported(),
            B_ENABLE | C_ENABLE | C_GUIDE => true,
            B_FOLDER => self.installed(0),
            B_CHECK => self.installed(0) && self.checks[0].is_none(),
            C_CHECK => self.installed(1) && self.checks[1].is_none(),
            _ => false,
        }
    }

    fn step_selection(&mut self, forward: bool) {
        for _ in 0..COUNT {
            self.sel = if forward {
                (self.sel + 1) % COUNT
            } else {
                (self.sel + COUNT - 1) % COUNT
            };
            if self.available(self.sel) {
                break;
            }
        }
    }
    fn status(&self, i: usize, cx: &Cx) -> &str {
        if self.found.is_none() {
            "Checking installation…"
        } else if !self.installed(i) {
            "Not installed"
        } else if !self.enabled(i, cx) {
            "Installed · off for new sessions"
        } else {
            "Installed · enabled for new sessions"
        }
    }

    fn activate(&mut self, n: usize, cx: &mut Cx) {
        if !self.available(n) {
            return;
        }
        self.sel = n.min(COUNT - 1);
        match n {
            B_INSTALL | C_INSTALL => {
                if cx.svc.is_demo() {
                    return cx.toast(Kind::Info, "The demo doesn't install software.");
                }
                let (i, tool) = if n == B_INSTALL { (0, Tool::JustBrowse) } else { (1, Tool::Cua) };
                if self.found.is_none() {
                    return;
                }
                let ids = if self.installed(i) {
                    vec![tool.id(), "--force"]
                } else if i == 0 {
                    vec!["node", tool.id()]
                } else {
                    vec![tool.id()]
                };
                let cwd = std::env::current_dir().unwrap_or_default();
                cx.act(Action::Open(
                    Box::new(super::quick_install::install_term(cx.svc, cwd, &ids)),
                    Place::Tab,
                ));
            }
            B_ENABLE | C_ENABLE => {
                let mut settings = cx.svc.settings();
                let on = if n == B_ENABLE {
                    &mut settings.automation.justbrowse
                } else {
                    &mut settings.automation.cua
                };
                *on = !*on;
                cx.svc.save_settings(settings);
                cx.toast(
                    Kind::Info,
                    "Browser and computer settings saved. New and resumed sessions pick them up.",
                );
            }
            B_FOLDER if self.installed(0) => {
                if let Err(e) = crate::panes::links::open(&bro_core::justbrowse::shown_folder().to_string_lossy()) {
                    cx.toast(Kind::Error, e);
                }
            }
            B_CHECK | C_CHECK => self.readiness(usize::from(n == C_CHECK), cx),
            C_GUIDE => {
                if let Err(e) = crate::panes::links::open(bro_core::cua::DOCS) {
                    cx.toast(Kind::Error, e);
                }
            }
            _ => {}
        }
    }

    fn readiness(&mut self, i: usize, cx: &Cx) {
        if !self.installed(i) || self.checks[i].is_some() {
            return;
        }
        if cx.svc.is_demo() {
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        self.checks[i] = Some(slot.clone());
        self.notice[i] = None;
        let waker = cx.waker();
        std::thread::spawn(move || {
            let result = if i == 0 {
                bro_core::justbrowse::connected().and_then(|on| {
                    if on {
                        Ok("Connected to your browser.".into())
                    } else {
                        Err("Open the JustBrowse extension and click Connect, then check again.".into())
                    }
                })
            } else {
                bro_core::cua::check_readiness()
            };
            *slot.lock() = Some(result);
            waker.wake();
        });
    }
}

impl Pane for AutomationView {
    fn title(&self) -> String {
        "Browser and computer".into()
    }
    fn icon(&self) -> &'static str {
        "remote"
    }
    fn view(&self) -> Option<&'static str> {
        Some("automation")
    }
    fn tick_every(&self) -> Option<Duration> {
        Some(Duration::from_secs(3))
    }
    fn poll(&mut self, cx: &mut Cx) {
        let found = self.pending.as_ref().and_then(|p| p.lock().take());
        if let Some(found) = found {
            self.found = Some(found);
            self.pending = None;
        }
        for i in 0..2 {
            let result = self.checks[i].as_ref().and_then(|p| p.lock().take());
            if let Some(result) = result {
                self.notice[i] = Some(result);
                self.checks[i] = None;
            }
        }
        if self.checked_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(3)) {
            self.check(cx);
        }
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        self.poll(cx);
        let t = cx.theme;
        let area = ui::hint_line(f, area, &[("j/k", "move"), ("enter", "choose"), ("r", "check installation")], t);
        let labels = [
            if self.installed(0) {
                "Update JustBrowse"
            } else {
                "Install JustBrowse"
            },
            "Use JustBrowse in new sessions",
            "Open extension folder",
            "Check browser connection",
            if self.installed(1) { "Update CUA" } else { "Install CUA" },
            "Use CUA in new sessions",
            "Check CUA readiness",
            "Open CUA setup guide",
        ];
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = vec![
            (None, vec![Span::styled("Browser and computer", ui::bold_accent(t))]),
            (
                None,
                vec![Span::styled("Bro connects these tools to Claude, Codex, Pi and omp.", muted(t))],
            ),
            (
                None,
                vec![Span::styled(
                    "Installed tools replace built-in browser and computer use in new sessions.",
                    muted(t),
                )],
            ),
        ];
        for i in 0..2 {
            lines.push((None, vec![]));
            lines.push((
                None,
                vec![
                    Span::styled(
                        if i == 0 {
                            "JustBrowse · your signed-in browser"
                        } else {
                            "CUA · desktop apps"
                        },
                        ui::bold(),
                    ),
                    Span::styled(format!("  {}", self.status(i, cx)), muted(t)),
                ],
            ));
            if i == 0 {
                for text in [
                    "After installing: open chrome://extensions or edge://extensions.",
                    "Turn on Developer mode, Load unpacked, select the extension folder, then Connect.",
                ] {
                    lines.push((None, vec![Span::styled(text, muted(t))]));
                }
            } else {
                lines.push((None, vec![Span::styled(bro_core::cua::after_install(), muted(t))]));
            }
            for n in i * 4..i * 4 + 4 {
                let suffix = if n == B_ENABLE || n == C_ENABLE {
                    if self.enabled(i, cx) { " · on" } else { " · off" }
                } else {
                    ""
                };
                lines.push((Some(n), vec![Span::styled(format!("{}{}", labels[n], suffix), fg(t.fg))]));
            }
            if let Some(result) = &self.notice[i] {
                let (text, color) = match result {
                    Ok(s) => (s, t.good),
                    Err(e) => (e, t.danger),
                };
                lines.push((None, vec![Span::styled(text.clone(), fg(color))]));
            }
        }
        let room = area.height as usize;
        let at = lines.iter().position(|(n, _)| *n == Some(self.sel)).unwrap_or(0);
        let mut scroll = self.scroll as usize;
        if at < scroll {
            scroll = at;
        } else if at >= scroll + room {
            scroll = at + 1 - room;
        }
        self.scroll = scroll.min(lines.len().saturating_sub(room)) as f32;
        self.hits.clear();
        for (i, (n, spans)) in lines.into_iter().skip(self.scroll as usize).take(room).enumerate() {
            let r = Rect {
                y: area.y + i as u16,
                height: 1,
                ..area
            };
            if let Some(n) = n {
                self.hits.push((r.y, n));
            }
            let on = n == Some(self.sel);
            let spans = std::iter::once(Span::raw(if on { " › " } else { "   " }))
                .chain(spans)
                .map(|span| {
                    if on {
                        Span::styled(span.content, span.style.bg(t.frame))
                    } else {
                        span
                    }
                })
                .collect();
            ui::line(f, r, spans);
        }
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.step_selection(true),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => self.step_selection(false),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(self.sel, cx),
            KeyCode::Char('r') => self.check(cx),
            _ => return false,
        }
        self.reveal = true;
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        if ev.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some((_, n)) = self.hits.iter().find(|(y, _)| *y == ev.row)
        {
            self.activate(*n, cx);
        }
    }
}
