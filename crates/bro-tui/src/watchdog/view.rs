//! The watchdog overlay (prefix w): a setup form for a session's milestones and how the watchdog behaves,
//! and, once one runs, its live status page: milestones as the reviewer marks them, the wake-up in
//! progress, the log, and wake now / pause / edit / stop.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::layout::PaneId;
use crate::textarea::TextArea;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_bridge::{Milestone, MilestoneStatus, WatchdogPlan, WatchdogSnapshot};
use bro_core::config::WatchdogSettings;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::Span,
};

/// Rows a milestone field may grow to.
const FIELD_ROWS: usize = 6;
/// Left gutter of a milestone row: marker, number, status.
const GUTTER: u16 = 6;
const LABEL: usize = 18;
const VALUE: usize = 20;

/// What the app should do.
#[derive(Debug, PartialEq)]
pub enum Out {
    None,
    Close,
    Start(WatchdogPlan),
    Save(WatchdogPlan),
    Wake,
    Pause(bool),
    Stop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Milestone(usize),
    Wake,
    Fresh,
    Second,
    Budget,
    Primary,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Button {
    Wake,
    Pause,
    Edit,
    Stop,
    Close,
}

const BUTTONS: [Button; 5] = [Button::Wake, Button::Pause, Button::Edit, Button::Stop, Button::Close];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    Field(Field),
    Button(Button),
}

pub struct WatchdogView {
    pub pane: PaneId,
    pub sid: String,
    /// the window title: the session's full name
    title: String,
    /// what the copy calls it: "Mal", or "this session" when it has no name of its own
    name: String,
    /// "codex · personal · gpt-5.5"
    agent: String,
    editing: bool,
    milestones: Vec<TextArea>,
    /// per milestone: the status and note it had (kept through edits)
    kept: Vec<(MilestoneStatus, Option<String>)>,
    idle_secs: u64,
    max_wakes: u32,
    fresh: bool,
    second: bool,
    focus: Field,
    button: Button,
    /// who reviews next: "claude · home · opus · 72% left"
    reviewer: String,
    hits: Vec<(Rect, Hit)>,
    scroll: usize,
    /// width of the milestone fields as last drawn (keys move by visual rows)
    field_width: usize,
    /// s was pressed once on the status page
    stop_armed: bool,
    error: Option<String>,
}

impl WatchdogView {
    /// The status page when the session has a watchdog, else the setup form with `defaults`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(pane: PaneId, sid: String, title: String, name: Option<String>, agent: String, existing: Option<&WatchdogSnapshot>, defaults: &WatchdogSettings, reviewer: String) -> WatchdogView {
        let mut v = WatchdogView {
            pane,
            sid,
            title,
            name: name.unwrap_or_else(|| "this session".into()),
            agent,
            editing: existing.is_none(),
            milestones: vec![TextArea::new("")],
            kept: vec![(MilestoneStatus::Pending, None)],
            idle_secs: defaults.idle_secs,
            max_wakes: defaults.max_wakes,
            fresh: true,
            second: true,
            focus: Field::Milestone(0),
            button: Button::Wake,
            reviewer,
            hits: vec![],
            scroll: 0,
            field_width: 60,
            stop_armed: false,
            error: None,
        };
        if let Some(dog) = existing {
            v.load(&dog.plan);
        }
        v
    }

    /// Fill the form from a plan.
    fn load(&mut self, plan: &WatchdogPlan) {
        self.milestones = plan.milestones.iter().map(|m| TextArea::new(&m.title)).collect();
        self.kept = plan.milestones.iter().map(|m| (m.status, m.note.clone())).collect();
        if self.milestones.is_empty() {
            self.milestones.push(TextArea::new(""));
            self.kept.push((MilestoneStatus::Pending, None));
        }
        self.idle_secs = plan.idle_secs;
        self.max_wakes = plan.max_wakes;
        self.fresh = plan.fresh_context;
        self.second = plan.second_opinions;
        self.focus = Field::Milestone(0);
    }

    #[cfg(test)]
    pub fn editing(&self) -> bool {
        self.editing
    }

    /// Why the last action didn't go through (shown above the buttons).
    pub fn set_error(&mut self, e: String) {
        self.error = Some(e);
    }

    /// The plan the form describes: blank milestones dropped, kept statuses carried along.
    pub fn plan(&self) -> WatchdogPlan {
        let milestones = self
            .milestones
            .iter()
            .zip(&self.kept)
            .filter(|(ta, _)| !ta.is_blank())
            .map(|(ta, (status, note))| Milestone { title: ta.text().trim().to_string(), status: *status, note: note.clone() })
            .collect();
        WatchdogPlan { milestones, fresh_context: self.fresh, second_opinions: self.second, max_wakes: self.max_wakes, idle_secs: self.idle_secs }
    }

    fn order(&self) -> Vec<Field> {
        let mut v: Vec<Field> = (0..self.milestones.len()).map(Field::Milestone).collect();
        v.extend([Field::Wake, Field::Fresh, Field::Second, Field::Budget, Field::Primary, Field::Cancel]);
        v
    }

    fn move_focus(&mut self, dir: isize) {
        let order = self.order();
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        // up from the buttons goes to the budget, not from Cancel to Start
        let to = match (self.focus, dir) {
            (Field::Cancel, -1) => Field::Budget,
            (Field::Primary, 1) => Field::Primary,
            _ => order[(i + dir).clamp(0, order.len() as isize - 1) as usize],
        };
        self.set_focus(to, dir > 0);
    }

    fn set_focus(&mut self, to: Field, from_above: bool) {
        if let Field::Milestone(i) = to
            && let Some(ta) = self.milestones.get_mut(i)
        {
            ta.cursor_to(from_above);
        }
        self.focus = to;
    }

    fn submit(&mut self, existing: bool) -> Out {
        let plan = self.plan();
        if plan.milestones.is_empty() {
            self.error = Some("Add a milestone first. The reviewer checks every handoff against them.".into());
            self.set_focus(Field::Milestone(0), true);
            return Out::None;
        }
        if existing {
            self.editing = false;
            Out::Save(plan)
        } else {
            Out::Start(plan)
        }
    }

    fn cancel(&mut self, dog: Option<&WatchdogSnapshot>) -> Out {
        match dog {
            Some(d) => {
                // back to the status page, the form as the plan stands
                self.load(&d.plan);
                self.editing = false;
                self.error = None;
                Out::None
            }
            None => Out::Close,
        }
    }

    pub fn paste(&mut self, text: &str) {
        if !self.editing {
            return;
        }
        if let Field::Milestone(i) = self.focus {
            // one pasted line per milestone when it's a list
            let lines: Vec<&str> = text.lines().map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim()).filter(|l| !l.is_empty()).collect();
            if lines.len() > 1 && self.milestones[i].is_blank() && lines.iter().all(|l| l.chars().count() < 200) {
                self.milestones.remove(i);
                self.kept.remove(i);
                for (n, line) in lines.iter().enumerate() {
                    self.milestones.insert(i + n, TextArea::new(line));
                    self.kept.insert(i + n, (MilestoneStatus::Pending, None));
                }
                self.focus = Field::Milestone(i + lines.len() - 1);
            } else {
                self.milestones[i].insert(text);
            }
        }
    }

    pub fn key(&mut self, k: KeyEvent, dog: Option<&WatchdogSnapshot>) -> Out {
        if self.editing { self.key_edit(k, dog) } else { self.key_status(k, dog) }
    }

    fn key_edit(&mut self, k: KeyEvent, dog: Option<&WatchdogSnapshot>) -> Out {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        self.error = None;
        match k.code {
            KeyCode::Char('s') if ctrl => return self.submit(dog.is_some()),
            KeyCode::Esc => return self.cancel(dog),
            KeyCode::Tab => {
                self.move_focus(1);
                return Out::None;
            }
            KeyCode::BackTab => {
                self.move_focus(-1);
                return Out::None;
            }
            _ => {}
        }
        match self.focus {
            Field::Milestone(i) => {
                let n = self.milestones.len();
                match k.code {
                    KeyCode::Up if alt || ctrl => {
                        if i > 0 {
                            self.milestones.swap(i, i - 1);
                            self.kept.swap(i, i - 1);
                            self.focus = Field::Milestone(i - 1);
                        }
                    }
                    KeyCode::Down if alt || ctrl => {
                        if i + 1 < n {
                            self.milestones.swap(i, i + 1);
                            self.kept.swap(i, i + 1);
                            self.focus = Field::Milestone(i + 1);
                        }
                    }
                    KeyCode::Char('d') if ctrl => self.remove(i),
                    KeyCode::Backspace if self.milestones[i].text().is_empty() && n > 1 => {
                        self.remove(i);
                        if let Field::Milestone(j) = self.focus {
                            self.milestones[j].cursor_to(false);
                        }
                    }
                    KeyCode::Enter if !alt && !k.modifiers.contains(KeyModifiers::SHIFT) => {
                        if !self.milestones[i].is_blank() {
                            self.milestones.insert(i + 1, TextArea::new(""));
                            self.kept.insert(i + 1, (MilestoneStatus::Pending, None));
                            self.focus = Field::Milestone(i + 1);
                        } else if n > 1 {
                            // an empty line ends the list
                            self.milestones.remove(i);
                            self.kept.remove(i);
                            self.focus = Field::Wake;
                        } else {
                            self.focus = Field::Wake;
                        }
                    }
                    _ => {
                        if !self.milestones[i].key(k, self.field_width) {
                            match k.code {
                                KeyCode::Up => self.move_focus(-1),
                                KeyCode::Down => self.move_focus(1),
                                _ => {}
                            }
                        }
                    }
                }
            }
            f => {
                let dir = match k.code {
                    KeyCode::Left | KeyCode::Char('h') => -1,
                    KeyCode::Right | KeyCode::Char('l') => 1,
                    KeyCode::Enter | KeyCode::Char(' ') => 0,
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.move_focus(-1);
                        return Out::None;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.move_focus(1);
                        return Out::None;
                    }
                    _ => return Out::None,
                };
                return self.change(f, dir, dog);
            }
        }
        Out::None
    }

    /// ←/→ (dir ±1) or enter (0) on an option row or a button.
    fn change(&mut self, f: Field, dir: isize, dog: Option<&WatchdogSnapshot>) -> Out {
        match f {
            Field::Wake => self.idle_secs = super::step_choice(&super::IDLE_CHOICES, self.idle_secs, if dir == 0 { 1 } else { dir }),
            Field::Budget => self.max_wakes = super::step_choice(&super::BUDGET_CHOICES, self.max_wakes, if dir == 0 { 1 } else { dir }),
            Field::Fresh => self.fresh = !self.fresh,
            Field::Second => self.second = !self.second,
            Field::Primary if dir == 0 => return self.submit(dog.is_some()),
            Field::Cancel if dir == 0 => return self.cancel(dog),
            Field::Primary | Field::Cancel => self.focus = if dir > 0 { Field::Cancel } else { Field::Primary },
            Field::Milestone(_) => {}
        }
        Out::None
    }

    fn remove(&mut self, i: usize) {
        if self.milestones.len() == 1 {
            self.milestones[0].set_text("");
            self.kept[0] = (MilestoneStatus::Pending, None);
            return;
        }
        self.milestones.remove(i);
        self.kept.remove(i);
        self.focus = Field::Milestone(i.saturating_sub(1).min(self.milestones.len() - 1));
    }

    fn key_status(&mut self, k: KeyEvent, dog: Option<&WatchdogSnapshot>) -> Out {
        let i = BUTTONS.iter().position(|b| *b == self.button).unwrap_or(0);
        let pressed = match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Out::Close,
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                self.button = BUTTONS[i.saturating_sub(1)];
                None
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.button = BUTTONS[(i + 1).min(BUTTONS.len() - 1)];
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') => Some(self.button),
            KeyCode::Char('w') => Some(Button::Wake),
            KeyCode::Char('p') => Some(Button::Pause),
            KeyCode::Char('e') => Some(Button::Edit),
            KeyCode::Char('s') | KeyCode::Char('x') => Some(Button::Stop),
            _ => None,
        };
        match pressed {
            Some(b) => self.press(b, dog),
            None => Out::None,
        }
    }

    fn press(&mut self, b: Button, dog: Option<&WatchdogSnapshot>) -> Out {
        self.button = b;
        if b != Button::Stop {
            self.stop_armed = false;
        }
        match b {
            Button::Wake => Out::Wake,
            Button::Pause => Out::Pause(!dog.is_some_and(|d| d.paused)),
            Button::Edit => {
                if let Some(d) = dog {
                    self.load(&d.plan);
                }
                self.editing = true;
                Out::None
            }
            Button::Stop if self.stop_armed => Out::Stop,
            Button::Stop => {
                self.stop_armed = true;
                Out::None
            }
            Button::Close => Out::Close,
        }
    }

    pub fn click(&mut self, pos: Position, dog: Option<&WatchdogSnapshot>) -> Out {
        let Some(&(_, hit)) = self.hits.iter().find(|(r, _)| r.contains(pos)) else { return Out::None };
        match hit {
            Hit::Button(b) => self.press(b, dog),
            Hit::Field(f) if f == self.focus && !matches!(f, Field::Milestone(_)) => self.change(f, 0, dog),
            Hit::Field(f) => {
                self.set_focus(f, false);
                Out::None
            }
        }
    }

    /// Hints for the bottom row.
    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if !self.editing {
            return vec![("w", "wake now"), ("p", "pause"), ("e", "edit"), ("s s", "stop"), ("esc", "close")];
        }
        match self.focus {
            Field::Milestone(_) => vec![("⏎", "next milestone"), ("alt+⏎", "new line"), ("alt+↑↓", "reorder"), ("ctrl+d", "delete"), ("tab", "options"), ("ctrl+s", "start")],
            Field::Primary | Field::Cancel => vec![("⏎", "choose"), ("←→", "switch"), ("ctrl+s", "start"), ("esc", "cancel")],
            _ => vec![("←→", "change"), ("↑↓", "move"), ("ctrl+s", "start"), ("esc", "cancel")],
        }
    }
}

/// Wrapped display rows of `text` at `width`.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let ta = TextArea::new(text);
    ta.rows(width).into_iter().map(|(s, e)| ta.text()[s..e].trim_end().to_string()).collect()
}

/// Local "2:14pm" for an ISO time.
fn clock(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso).map(|t| t.with_timezone(&chrono::Local).format("%-I:%M%P").to_string()).unwrap_or_default()
}

/// "1m 12s" since an ISO time.
fn since(iso: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso) else { return String::new() };
    let s = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
    if s < 60 { format!("{s}s") } else { format!("{}m {:02}s", s / 60, s % 60) }
}

fn spinner() -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    FRAMES[(ms / 100) as usize % FRAMES.len()]
}

fn status_glyph(s: MilestoneStatus, t: &Theme) -> Span<'static> {
    match s {
        MilestoneStatus::Done => Span::styled("✓", fg(t.good).add_modifier(Modifier::BOLD)),
        MilestoneStatus::Active => Span::styled("▸", fg(t.accent).add_modifier(Modifier::BOLD)),
        MilestoneStatus::Pending => Span::styled("○", muted(t)),
    }
}

fn heading(f: &mut Frame, r: Rect, label: &str, right: String, t: &Theme) {
    ui::line_lr(f, r, vec![Span::styled(label.to_string(), muted(t).add_modifier(Modifier::BOLD))], vec![Span::styled(right, muted(t))]);
}

fn button(label: &str, on: bool, primary: bool, t: &Theme) -> Span<'static> {
    let ink = if t.is_light() { ratatui::style::Color::Rgb(255, 255, 255) } else { t.deep() };
    let style = match (on, primary) {
        (true, _) => fg(ink).bg(t.accent).add_modifier(Modifier::BOLD),
        (false, true) => fg(t.accent).bg(t.frame).add_modifier(Modifier::BOLD),
        (false, false) => fg(t.fg).bg(t.frame),
    };
    Span::styled(format!(" {label} "), style)
}

/// Draw the overlay. `dog` is the session's watchdog as the bridge has it now (None: not started).
pub fn draw(f: &mut Frame, area: Rect, v: &mut WatchdogView, dog: Option<&WatchdogSnapshot>, t: &Theme) {
    v.hits.clear();
    let w = area.width.saturating_sub(4).min(104);
    // as tall as what it shows (it grows as milestones are added), within the screen
    let text_w = w.saturating_sub(6) as usize;
    let h = (needed_rows(v, dog, text_w) as u16 + 4).max(20).min(area.height.saturating_sub(2));
    let title = format!("{}watchdog · {}", ui::lead("watchdog"), v.title);
    let inner = ui::popup(f, area, w, h, &title, t);
    let inner = Rect { x: inner.x + 2, width: inner.width.saturating_sub(4), ..inner };
    if inner.height < 8 || inner.width < 30 {
        return;
    }
    let body = ui::hint_line(f, inner, &v.hints(), t);
    match (v.editing, dog) {
        (false, Some(d)) => draw_status(f, body, v, d, t),
        _ => draw_form(f, body, v, dog, t),
    }
}

/// Rows the page needs at `width` (inside the frame's padding, hint line included).
fn needed_rows(v: &WatchdogView, dog: Option<&WatchdogSnapshot>, width: usize) -> usize {
    match (v.editing, dog) {
        (false, Some(d)) => {
            let mut n = 2 + 1 + 1;
            for m in &d.plan.milestones {
                n += wrap(&m.title, width.saturating_sub(6)).len().min(3) + usize::from(m.note.is_some());
            }
            if let Some(r) = &d.running {
                n += 3 + r.steps.len().clamp(1, 6);
            }
            n += 2;
            n += if d.runs.is_empty() { 1 } else { d.runs.iter().map(|r| 1 + wrap(&r.summary, width.saturating_sub(12)).len().min(3)).sum() };
            n + 4
        }
        _ => {
            let field = width.saturating_sub(GUTTER as usize + 1).max(1);
            let intro = wrap(&intro(v, dog.is_some()), width).len();
            let fields: usize = v.milestones.iter().map(|m| m.height(field).clamp(1, FIELD_ROWS)).sum();
            intro + 3 + fields + 2 + 5 + 4
        }
    }
}

fn intro(v: &WatchdogView, existing: bool) -> String {
    let name = &v.name;
    if existing {
        format!("Edit what {name} works toward. Milestones the reviewer already marked keep their status, and the log and wake-up count carry over.")
    } else {
        format!("When {name} finishes a turn and goes quiet, bro wakes a fresh reviewer on the login with the most usage left. It reads the chat and the code, checks the work against these milestones and hands {name} its next task.")
    }
}

fn draw_form(f: &mut Frame, area: Rect, v: &mut WatchdogView, dog: Option<&WatchdogSnapshot>, t: &Theme) {
    let existing = dog.is_some();
    // the buttons and any error stay at the bottom
    let bottom = Rect { y: area.bottom().saturating_sub(2), height: 1, ..area };
    let content = Rect { height: area.height.saturating_sub(3), ..area };
    let field_w = content.width.saturating_sub(GUTTER) as usize;
    v.field_width = field_w.saturating_sub(1).max(1);
    let name = v.name.clone();

    // (height, what) per block, top to bottom
    enum Block {
        Text(Vec<Span<'static>>),
        Gap,
        Heading(&'static str, String),
        Milestone(usize),
        Opt(Field),
    }
    let mut blocks: Vec<(usize, Block)> = vec![];
    for row in wrap(&intro(v, existing), content.width as usize) {
        blocks.push((1, Block::Text(vec![Span::styled(row, muted(t))])));
    }
    blocks.push((1, Block::Text(vec![Span::styled("watches  ", muted(t)), Span::styled(v.agent.clone(), fg(t.inline))])));
    blocks.push((1, Block::Gap));
    let count = v.milestones.iter().filter(|m| !m.is_blank()).count();
    blocks.push((1, Block::Heading("MILESTONES", format!("{count} · in order · the first one starts now"))));
    for i in 0..v.milestones.len() {
        let rows = v.milestones[i].height(v.field_width).clamp(1, FIELD_ROWS);
        blocks.push((rows, Block::Milestone(i)));
    }
    blocks.push((1, Block::Gap));
    blocks.push((1, Block::Heading("HOW IT WORKS", String::new())));
    for o in [Field::Wake, Field::Fresh, Field::Second, Field::Budget] {
        blocks.push((1, Block::Opt(o)));
    }
    blocks.push((1, Block::Text(vec![
        Span::styled(format!("  {:<LABEL$}", "Reviewer"), Style::default()),
        Span::styled(v.reviewer.clone(), fg(t.inline)),
        Span::styled("  picked fresh each wake-up", muted(t)),
    ])));

    // keep the focused block in view
    let tops: Vec<usize> = blocks.iter().scan(0, |y, (hgt, _)| {
        let top = *y;
        *y += hgt;
        Some(top)
    }).collect();
    let total: usize = blocks.iter().map(|b| b.0).sum();
    let room = content.height as usize;
    if let Some(i) = blocks.iter().position(|(_, b)| matches!((b, v.focus), (Block::Milestone(a), Field::Milestone(b)) if *a == b) || matches!(b, Block::Opt(o) if *o == v.focus)) {
        let (top, hgt) = (tops[i], blocks[i].0);
        if top < v.scroll {
            v.scroll = top.saturating_sub(1);
        } else if top + hgt > v.scroll + room {
            v.scroll = top + hgt - room;
        }
    }
    if matches!(v.focus, Field::Primary | Field::Cancel) {
        v.scroll = total.saturating_sub(room);
    }
    v.scroll = v.scroll.min(total.saturating_sub(room));

    for ((hgt, block), top) in blocks.iter().zip(&tops) {
        if *top < v.scroll || top + hgt > v.scroll + room {
            continue;
        }
        let r = Rect { y: content.y + (top - v.scroll) as u16, height: *hgt as u16, ..content };
        match block {
            Block::Gap => {}
            Block::Text(spans) => ui::line(f, r, spans.clone()),
            Block::Heading(label, right) => heading(f, r, label, right.clone(), t),
            Block::Milestone(i) => {
                let i = *i;
                let on = v.focus == Field::Milestone(i);
                let (status, _) = v.kept[i];
                let mut gutter = vec![Span::styled(if on { "▌" } else { " " }, ui::accent(t)), Span::styled(format!("{:>2} ", i + 1), if on { ui::bold_accent(t) } else { muted(t) })];
                if existing {
                    gutter.push(status_glyph(status, t));
                }
                ui::line(f, Rect { height: 1, width: GUTTER, ..r }, gutter);
                let fr = Rect { x: r.x + GUTTER, width: r.width.saturating_sub(GUTTER), ..r };
                let placeholder = if i == 0 { "the first milestone and what done looks like, e.g. offline sync: writes queue, retry with backoff, tests cover it" } else { "the next milestone · enter on an empty one finishes the list" };
                let style = if status == MilestoneStatus::Done { muted(t) } else { Style::default() };
                v.milestones[i].render(f, fr, style, on, placeholder, t);
                v.hits.push((r, Hit::Field(Field::Milestone(i))));
            }
            Block::Opt(o) => {
                let on = v.focus == *o;
                let (label, value, help) = match o {
                    Field::Wake => ("Wake after", format!("‹ {} idle ›", super::human(v.idle_secs)), "after each stop · a question waits 2 min or more".to_string()),
                    Field::Fresh => ("Fresh context", (if v.fresh { "on" } else { "off" }).to_string(), if v.fresh { "may clear the conversation before a new milestone".into() } else { "keeps one conversation going".to_string() }),
                    Field::Second => ("Second opinions", (if v.second { "on" } else { "off" }).to_string(), if v.second { "may open a helper, say an Opus pass on Codex UI work".into() } else { format!("only ever talks to {name}") }),
                    Field::Budget => ("Budget", format!("‹ {} wake-ups ›", v.max_wakes), "then it stands down and tells you".to_string()),
                    _ => continue,
                };
                let vstyle = if value == "off" { muted(t) } else if on { ui::bold_accent(t) } else { fg(t.inline) };
                ui::line(f, r, vec![
                    Span::styled(if on { "▌ " } else { "  " }, ui::accent(t)),
                    Span::styled(format!("{label:<LABEL$}"), if on { ui::bold() } else { Style::default() }),
                    Span::styled(ui::pad(&value, VALUE), vstyle),
                    Span::styled(help, muted(t)),
                ]);
                v.hits.push((r, Hit::Field(*o)));
            }
        }
    }
    if let Some(e) = &v.error {
        ui::line(f, Rect { y: bottom.y.saturating_sub(1), ..bottom }, vec![Span::styled(e.clone(), fg(t.danger))]);
    }
    let primary = if existing { "Save plan" } else { "Start watchdog" };
    let a = button(primary, v.focus == Field::Primary, true, t);
    let b = button("Cancel", v.focus == Field::Cancel, false, t);
    let (aw, bw) = (ui::width(&a.content) as u16, ui::width(&b.content) as u16);
    ui::line(f, bottom, vec![a, Span::raw("  "), b]);
    v.hits.push((Rect { width: aw, ..bottom }, Hit::Field(Field::Primary)));
    v.hits.push((Rect { x: bottom.x + aw + 2, width: bw, ..bottom }, Hit::Field(Field::Cancel)));
}

fn draw_status(f: &mut Frame, area: Rect, v: &mut WatchdogView, d: &WatchdogSnapshot, t: &Theme) {
    let width = area.width as usize;
    let limit = area.bottom().saturating_sub(2);
    let mut y = area.y;
    let mut put = |f: &mut Frame, spans: Vec<Span<'static>>, right: Vec<Span<'static>>| {
        if y < limit {
            ui::line_lr(f, Rect { y, height: 1, ..area }, spans, right);
        }
        y += 1;
    };
    let name = v.name.clone();
    let wakes = vec![Span::styled(format!("wake-up {} of {}", d.wakes, d.plan.max_wakes), muted(t))];
    match (&d.running, &d.finished, d.paused) {
        (Some(r), _, _) => {
            put(f, vec![Span::styled(format!("{} reviewing", spinner()), ui::bold_accent(t)), Span::styled(format!("  {} · {}", r.reviewer, since(&r.started_at)), fg(t.inline))], wakes);
            put(f, vec![Span::styled(format!("woke because {}", r.reason), muted(t))], vec![]);
        }
        (None, Some(why), _) => {
            put(f, vec![Span::styled("✓ stood down", fg(t.good).add_modifier(Modifier::BOLD)), Span::styled(format!("  {why}"), muted(t))], wakes);
            put(f, vec![Span::styled("e adds milestones or wake-ups and it watches again", muted(t))], vec![]);
        }
        (None, None, true) => {
            put(f, vec![Span::styled("‖ paused", fg(t.inline).add_modifier(Modifier::BOLD)), Span::styled(format!("  {name} works on without it"), muted(t))], wakes);
            put(f, vec![Span::styled("p resumes watching", muted(t))], vec![]);
        }
        (None, None, false) => {
            put(f, vec![Span::styled("● watching", fg(t.good).add_modifier(Modifier::BOLD)), Span::styled(format!("  {}", v.agent), fg(t.inline))], wakes);
            put(f, vec![Span::styled(format!("wakes when {name} stops and sits idle {} · fresh context {} · second opinions {}", super::human(d.plan.idle_secs), if d.plan.fresh_context { "on" } else { "off" }, if d.plan.second_opinions { "on" } else { "off" }), muted(t))], vec![]);
        }
    }
    put(f, vec![], vec![]);
    let (done, total) = d.plan.progress();
    put(f, vec![Span::styled("MILESTONES", muted(t).add_modifier(Modifier::BOLD))], vec![Span::styled(format!("{done} of {total} done"), muted(t))]);
    for (i, m) in d.plan.milestones.iter().enumerate() {
        let rows = wrap(&m.title, width.saturating_sub(6));
        let shown = rows.len().min(3);
        for (n, row) in rows.into_iter().take(3).enumerate() {
            let text = if n + 1 == shown && shown == 3 && wrap(&m.title, width.saturating_sub(6)).len() > 3 { format!("{row}…") } else { row };
            let lead = if n == 0 { vec![Span::raw(" "), status_glyph(m.status, t), Span::styled(format!(" {:>2} ", i + 1), muted(t))] } else { vec![Span::raw("      ")] };
            let style = match m.status {
                MilestoneStatus::Done => muted(t),
                MilestoneStatus::Active => ui::bold(),
                MilestoneStatus::Pending => Style::default(),
            };
            let mut spans = lead;
            spans.push(Span::styled(text, style));
            put(f, spans, vec![]);
        }
        if let Some(note) = &m.note {
            put(f, vec![Span::raw("      "), Span::styled(format!("↳ {}", ui::fit(note, width.saturating_sub(8))), fg(t.inline))], vec![]);
        }
    }
    if let Some(r) = &d.running {
        put(f, vec![], vec![]);
        put(f, vec![Span::styled("RIGHT NOW", muted(t).add_modifier(Modifier::BOLD))], vec![]);
        if r.steps.is_empty() {
            put(f, vec![Span::styled("  starting the reviewer…", muted(t))], vec![]);
        }
        let skip = r.steps.len().saturating_sub(6);
        for (n, step) in r.steps.iter().enumerate().skip(skip) {
            let last = n + 1 == r.steps.len();
            put(f, vec![Span::styled(if last { format!("  {} ", spinner()) } else { "  · ".into() }, ui::accent(t)), Span::styled(ui::fit(step, width.saturating_sub(4)), if last { Style::default() } else { muted(t) })], vec![]);
        }
    }
    put(f, vec![], vec![]);
    put(f, vec![Span::styled("LAST WAKE-UPS", muted(t).add_modifier(Modifier::BOLD))], vec![]);
    if d.runs.is_empty() && d.running.is_none() {
        put(f, vec![Span::styled(format!("  None yet. The first comes once {name} stops and sits idle for {}.", super::human(d.plan.idle_secs)), muted(t))], vec![]);
    }
    for run in d.runs.iter().rev() {
        let mark = if run.ok { Span::styled("✓", fg(t.good)) } else { Span::styled("✗", fg(t.danger)) };
        put(f, vec![Span::styled(format!("  {:>7}  ", clock(&run.started_at)), muted(t)), mark, Span::styled(format!("  {}", run.reviewer), fg(t.inline))], vec![Span::styled(format!("{} steps", run.steps.len()), muted(t))]);
        let summary = run.summary.split_whitespace().collect::<Vec<_>>().join(" ");
        for row in wrap(&summary, width.saturating_sub(12)).into_iter().take(3) {
            put(f, vec![Span::raw("            "), Span::styled(row, if run.ok { Style::default() } else { fg(t.danger) })], vec![]);
        }
    }
    // the buttons
    let row = Rect { y: area.bottom().saturating_sub(1), height: 1, ..area };
    if v.stop_armed {
        ui::line(f, Rect { y: row.y.saturating_sub(1), ..row }, vec![Span::styled("press s again to stop the watchdog; the session keeps running", fg(t.danger))]);
    }
    let mut x = row.x;
    let mut spans = vec![];
    for b in BUTTONS {
        let label = match b {
            Button::Wake => "Wake now",
            Button::Pause if d.paused => "Resume",
            Button::Pause => "Pause",
            Button::Edit => "Edit plan",
            Button::Stop => "Stop watchdog",
            Button::Close => "Close",
        };
        let span = button(label, v.button == b, b == Button::Wake, t);
        let w = ui::width(&span.content) as u16;
        v.hits.push((Rect { x, width: w, ..row }, Hit::Button(b)));
        x += w + 2;
        spans.push(span);
        spans.push(Span::raw("  "));
    }
    ui::line(f, row, spans);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(v: &mut WatchdogView, s: &str) {
        for c in s.chars() {
            v.key(key(KeyCode::Char(c)), None);
        }
    }

    fn view() -> WatchdogView {
        WatchdogView::new(1, "sid".into(), "Mal".into(), Some("Mal".into()), "codex · me".into(), None, &WatchdogSettings::default(), "claude · home · opus".into())
    }

    #[test]
    fn milestones_are_typed_added_reordered_and_started() {
        let mut v = view();
        assert!(matches!(v.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL), None), Out::None), "nothing to start yet");
        assert!(v.error.is_some());
        typed(&mut v, "offline sync");
        v.key(key(KeyCode::Enter), None);
        typed(&mut v, "settings redesign");
        v.key(key(KeyCode::Enter), None);
        typed(&mut v, "docs");
        v.key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT), None);
        assert_eq!(v.plan().milestones.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["offline sync", "docs", "settings redesign"]);
        // enter on an empty milestone finishes the list and moves to the options
        v.focus = Field::Milestone(2);
        v.key(key(KeyCode::Enter), None);
        v.key(key(KeyCode::Enter), None);
        assert_eq!(v.focus, Field::Wake);
        assert_eq!(v.milestones.len(), 3);
        v.key(key(KeyCode::Right), None);
        assert_eq!(v.idle_secs, 120);
        v.key(key(KeyCode::Down), None);
        v.key(key(KeyCode::Enter), None);
        assert!(!v.fresh);
        match v.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL), None) {
            Out::Start(plan) => {
                assert_eq!(plan.milestones.len(), 3);
                assert_eq!(plan.idle_secs, 120);
                assert!(!plan.fresh_context);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_pasted_list_becomes_milestones() {
        let mut v = view();
        v.paste("- offline sync\n- settings redesign\n\n* docs\n");
        assert_eq!(v.plan().milestones.len(), 3);
        assert_eq!(v.focus, Field::Milestone(2));
        v.paste(" and the changelog");
        assert_eq!(v.plan().milestones[2].title, "docs and the changelog");
    }

    #[test]
    fn the_status_page_wakes_pauses_and_stops_after_a_second_press() {
        let dog = WatchdogSnapshot { plan: WatchdogPlan { milestones: vec![Milestone::new("a")], ..WatchdogPlan::default() }, ..WatchdogSnapshot::default() };
        let mut v = WatchdogView::new(1, "sid".into(), "Mal".into(), Some("Mal".into()), "claude".into(), Some(&dog), &WatchdogSettings::default(), String::new());
        assert!(!v.editing());
        assert_eq!(v.key(key(KeyCode::Char('w')), Some(&dog)), Out::Wake);
        assert_eq!(v.key(key(KeyCode::Char('p')), Some(&dog)), Out::Pause(true));
        assert_eq!(v.key(key(KeyCode::Char('s')), Some(&dog)), Out::None);
        assert_eq!(v.key(key(KeyCode::Char('s')), Some(&dog)), Out::Stop);
        v.key(key(KeyCode::Char('e')), Some(&dog));
        assert!(v.editing());
        typed(&mut v, " more");
        assert!(matches!(v.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL), Some(&dog)), Out::Save(p) if p.milestones[0].title == "a more"));
        assert!(!v.editing());
    }
}
