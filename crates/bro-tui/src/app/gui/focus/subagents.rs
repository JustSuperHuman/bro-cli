//! Focus mode's subagents ([`bro_core::sessions::subagents`]). Where the session started them, a card each says what
//! it's doing or what it reported; a strip over the reply box keeps the ones at work in sight while the
//! conversation goes on; a quiet line marks where each reported back. Any of them opens its own conversation in
//! place of the session's, under a head with the way back (esc goes back too).

use super::markdown::{self, Block};
use super::{BACK, Paint, SUBAGENT, Typo, clock};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::motion::Spring;
use crate::app::gui::kit::{Style, Ui, radius, weight, widgets};
use crate::focus::model_name;
use crate::layout::PaneId;
use bro_core::sessions::conversation::{Conversation, Item};
use bro_core::sessions::subagents::{Subagent, SubagentState, read_note};
use bro_core::sessions::tasks::{Task, TaskKind};
use bro_gfx::{Point, Rect};
use std::collections::HashMap;
use std::sync::Arc;

/// A card row's padding above and below its two lines.
const PAD_Y: f32 = 9.0;
/// The strip of subagents at work: its height, and the most a chip in it takes.
pub(super) const DOCK_H: f32 = 30.0;
const CHIP_MAX: f32 = 300.0;
/// A chip's name when the strip shows names alone: at most, and at least before the rest go to "+N".
const NAME_MAX: f32 = 170.0;
const NAME_MIN: f32 = 64.0;

/// The subagents a frame knows: each one the session's worker follows, and any the conversations shown name that
/// it doesn't follow yet (their cards show all the same).
#[derive(Default)]
pub(super) struct Subs {
    /// every one, the oldest first
    pub all: Vec<Arc<Subagent>>,
    by_call: HashMap<String, usize>,
}

impl Subs {
    pub fn new(followed: Option<&Arc<Vec<Arc<Subagent>>>>, shown: &[Option<&Conversation>]) -> Subs {
        let mut s = Subs::default();
        for sub in followed.into_iter().flat_map(|l| l.iter()) {
            s.add(sub.clone());
        }
        for sub in shown.iter().flatten().flat_map(|c| c.subagents.iter()) {
            if !s.by_call.contains_key(&sub.call) {
                s.add(Arc::new(sub.clone()));
            }
        }
        s
    }

    fn add(&mut self, sub: Arc<Subagent>) {
        self.by_call.insert(sub.call.clone(), self.all.len());
        self.all.push(sub);
    }

    pub fn get(&self, call: &str) -> Option<&Arc<Subagent>> {
        self.by_call.get(call).map(|i| &self.all[*i])
    }

    /// Those at work now, the oldest first.
    pub fn running(&self) -> Vec<Arc<Subagent>> {
        self.all.iter().filter(|s| s.state.running()).cloned().collect()
    }

    /// Of the subagents `call`'s started in turn: how many are at work, and how many there are.
    fn children(&self, call: &str) -> (usize, usize) {
        self.all.iter().filter(|s| s.parent.as_deref() == Some(call)).fold((0, 0), |(busy, all), s| (busy + usize::from(s.state.running()), all + 1))
    }
}

/// The click target that opens the subagent `call` started, registered for this frame.
pub(super) fn open_target(opened: &mut Vec<String>, call: &str) -> usize {
    let n = opened.iter().position(|c| c == call).unwrap_or_else(|| {
        opened.push(call.to_string());
        opened.len() - 1
    });
    SUBAGENT + n
}

/// The first thing `text` (markdown) says, as plain words on one line: its first paragraph (a heading over it, as
/// "## Summary", says less), else its first heading.
pub(super) fn first_line(text: &str) -> String {
    // the start is enough: a report can run to pages
    let end = text.char_indices().nth(800).map_or(text.len(), |(i, _)| i);
    let blocks = markdown::parse(&text[..end]);
    let flat = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let said = blocks.iter().find_map(|b| {
        let words = match b {
            Block::Para(spans) | Block::Item { spans, .. } | Block::Quote(spans) => markdown::plain(spans),
            Block::Code { lines, .. } => lines.first().cloned().unwrap_or_default(),
            _ => return None,
        };
        Some(flat(words)).filter(|w| !w.is_empty())
    });
    said.or_else(|| blocks.iter().find_map(|b| if let Block::Heading(_, spans) = b { Some(flat(markdown::plain(spans))).filter(|w| !w.is_empty()) } else { None })).unwrap_or_default()
}

/// What a subagent is doing now, in a few words: the step it's running, else its own heading for its thinking,
/// else the last thing it did or said. Once it's finished: the start of what it reported.
pub(super) fn activity(s: &Subagent) -> String {
    let conv = s.conversation.as_deref();
    let reported = || s.result.as_deref().map(first_line).filter(|l| !l.is_empty());
    match s.state {
        SubagentState::Running => {
            if let Some(step) = conv.and_then(Conversation::running) {
                return step.label.clone();
            }
            if let Some(t) = conv.and_then(|c| c.thought.as_ref()) {
                return t.text.clone();
            }
            conv.and_then(|c| {
                c.items.iter().rev().find_map(|it| match it.as_ref() {
                    Item::Did { steps } => steps.last().map(|s| s.label.clone()),
                    Item::Said { text, .. } => Some(first_line(text)).filter(|l| !l.is_empty()),
                    _ => None,
                })
            })
            .unwrap_or_else(|| "Getting started".to_string())
        }
        SubagentState::Done => reported().unwrap_or_else(|| "Finished".to_string()),
        SubagentState::Failed => reported().unwrap_or_else(|| "Stopped on an error".to_string()),
        SubagentState::Stopped => "Stopped before it finished".to_string(),
    }
}

/// How its thinking is going while it runs no step: its own heading for it, else how long it's been quiet.
pub(super) fn thinking(s: &Subagent, now_ms: i64) -> String {
    let conv = s.conversation.as_deref();
    let secs = conv.and_then(|c| c.last_ms).or(s.started_ms).map_or(0, |t| ((now_ms - t) / 1000).max(0));
    let heading = conv.and_then(|c| c.thought.as_ref()).map(|t| t.text.clone());
    let words = heading.unwrap_or_else(|| {
        match secs {
            0..=11 => "Thinking",
            12..=44 => "Still thinking",
            _ => "Deep in thought",
        }
        .to_string()
    });
    if secs >= 12 { format!("{words} · {}", clock(secs)) } else { words }
}

/// "2 subagents working", "3 subagents", "1 subagent".
fn crew(busy: usize, all: usize) -> String {
    match (busy, all) {
        (0, 1) => "1 subagent".to_string(),
        (0, n) => format!("{n} subagents"),
        (1, _) => "1 subagent working".to_string(),
        (b, _) => format!("{b} subagents working"),
    }
}

/// What the header says while subagents or background tasks work and the session itself doesn't: "1 subagent
/// working", "Build the release · 4m 10s" (one task alone says what it is), "2 background tasks running",
/// "1 subagent and 2 tasks working".
pub(super) fn working(subs: usize, tasks: &[Task], now_ms: i64) -> String {
    let n = |k: usize, one: &str, many: &str| if k == 1 { format!("1 {one}") } else { format!("{k} {many}") };
    match (subs, tasks) {
        (s, []) => crew(s, s),
        (0, [t]) => match t.elapsed_ms(now_ms).filter(|ms| *ms >= 1000) {
            Some(ms) => format!("{} · {}", t.title, clock(ms / 1000)),
            None => t.title.clone(),
        },
        (0, ts) => format!("{} running", n(ts.len(), "background task", "background tasks")),
        (s, ts) => format!("{} and {} working", n(s, "subagent", "subagents"), n(ts.len(), "task", "tasks")),
    }
}

/// What a card says on its right: what the subagent goes by (when that isn't what it was asked) or its sort, then
/// how long it's been at it, or took.
fn tag(s: &Subagent, now_ms: i64) -> String {
    let name = s.name.as_deref().filter(|n| !n.trim().is_empty() && !n.eq_ignore_ascii_case(&s.title)).or(s.kind.as_deref());
    let time = s.elapsed_ms(now_ms).filter(|ms| *ms >= 1000).map(|ms| clock(ms / 1000));
    [name.map(str::to_string), time].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// A subagent's state as a mark centred on `center`: a spinner while it works, a check once it's done, a warning
/// when it failed, a stop when it was stopped.
fn mark(ui: &mut Ui, s: &Subagent, center: Point, size: f32) {
    let t = ui.t;
    let r = Rect::new(center.x - 8.0, center.y - 8.0, 16.0, 16.0);
    // it just stopped working: the new mark pops in (springs past its size and settles)
    let ended = ui.since_on(("sub-mark", &s.call), !s.state.running());
    let pop = ui.pop_scale(ended);
    let fade = pop.min(1.0);
    match s.state {
        SubagentState::Running => ui.spinner(center, 4.4, 1.5, t.accent),
        SubagentState::Done => ui.icon(r, "\u{F012C}", size * pop, t.good.fade(fade)),
        SubagentState::Failed => ui.icon(r, "\u{F0028}", size * pop, t.danger.fade(fade)),
        SubagentState::Stopped => ui.icon(r, "\u{F04DB}", (size - 1.0) * pop, t.muted.fade(fade)),
    }
}

fn lines(c: &Typo) -> (f32, f32) {
    ((c.small * 1.5).round(), (c.small * 1.45).round())
}

/// How tall the cards of `n` subagents are.
pub(super) fn cards_height(c: &Typo, n: usize) -> f32 {
    let (a, b) = lines(c);
    n as f32 * (a + b + 2.0 * PAD_Y)
}

/// The cards of the subagents a turn started, at `at`, `w` wide: one panel, a row each (its mark, what it was asked,
/// what it goes by and for how long, and under that what it's doing, or what it reported). A row opens it.
pub(super) fn cards(ui: &mut Ui, c: &Typo, p: &mut Paint, subs: &[Arc<Subagent>], at: Point, w: f32) {
    let t = ui.t;
    let (l1, l2) = lines(c);
    let rh = l1 + l2 + 2.0 * PAD_Y;
    let r = Rect::new(at.x, at.y, w, rh * subs.len() as f32);
    ui.panel(r, radius::OVERLAY, t.text.with_alpha(if t.light { 0.025 } else { 0.035 }), t.hairline);
    for (n, s) in subs.iter().enumerate() {
        let row = Rect::new(r.x, r.y + n as f32 * rh, r.w, rh);
        if n > 0 {
            ui.hline(row.x + 14.0, row.y, row.w - 28.0, t.hairline);
        }
        let hot = ui.hover_amount(("focus-sub", p.id, &s.call), row);
        if hot > 0.01 {
            ui.round(Rect::new(row.x + 3.0, row.y + 3.0, row.w - 6.0, row.h - 6.0), radius::CONTROL, t.text.with_alpha(0.045 * hot));
        }
        let target = open_target(&mut p.opened, &s.call);
        ui.hit(row, Action::Focus(p.id, target));
        let top = row.y + PAD_Y;
        let m1 = top + l1 / 2.0;
        mark(ui, s, Point::new(row.x + 21.0, m1), c.small + 1.0);
        // on the right: what it goes by and for how long, then the way in
        let mut right = row.right() - 10.0;
        ui.icon(Rect::new(right - 16.0, row.y + (rh - 16.0) / 2.0, 16.0, 16.0), "\u{F0142}", 14.0, c.faint.mix(c.text, 0.7 * hot));
        right -= 26.0;
        let said = tag(s, c.now_ms);
        if !said.is_empty() {
            let b = ui.line(&said, Style::mono(c.small, c.faint), Some((w * 0.38).max(60.0)));
            ui.draw(&b, Point::new(right - b.w, m1 - b.h / 2.0));
            right -= b.w + 14.0;
        }
        let x = row.x + 38.0;
        let title = ui.line(&s.title, Style::mono(c.small, c.text).weight(weight::MEDIUM), Some((right - x).max(30.0)));
        ui.draw(&title, Point::new(x, m1 - title.h / 2.0));
        // under it: what it's doing or what it reported, and the subagents it started in turn
        let mut words = activity(s);
        let (busy, all) = p.subs.map_or((0, 0), |subs| subs.children(&s.call));
        if all > 0 {
            words = format!("{words} · {}", crew(busy, all));
        }
        let ink = if s.state == SubagentState::Failed { t.danger.mix(c.muted, 0.35) } else { c.muted };
        let b = ui.line(&words, Style::mono(c.small, ink), Some((row.right() - 36.0 - x).max(30.0)));
        ui.draw(&b, Point::new(x, top + l1 + (l2 - b.h) / 2.0));
        if hot > 0.6 && (title.layout.truncated || b.layout.truncated) {
            widgets::tooltip(ui, row, &format!("{}\n{words}", s.title));
        }
    }
}

/// Where a subagent reported back (a conversation's note, `text` as [`read_note`] reads it), at `at` across `w`: a
/// quiet line through the middle and on it its mark and "… finished" (failed, stopped), which opens it. `i` is the
/// note's place in the conversation.
pub(super) fn note(ui: &mut Ui, c: &Typo, text: &str, i: usize, at: Point, w: f32, p: &mut Paint) -> f32 {
    let h = (c.small * 1.9).round();
    if !p.draw {
        return h;
    }
    let Some((call, state)) = read_note(text) else { return h };
    let t = ui.t;
    let sub = p.subs.and_then(|s| s.get(call)).cloned();
    let who = sub.as_ref().map_or_else(|| "A subagent".to_string(), |s| s.title.clone());
    let verb = match state {
        SubagentState::Done => "finished",
        SubagentState::Failed => "failed",
        SubagentState::Stopped => "stopped",
        SubagentState::Running => "is working again",
    };
    let b = ui.line(&format!("{who} {verb}"), Style::mono(c.small, c.faint), Some((w - 120.0).max(60.0)));
    let width = 22.0 + b.w;
    let x = at.x + (w - width) / 2.0;
    let mid = at.y + h / 2.0;
    let r = Rect::new(x - 10.0, mid - 12.0, width + 20.0, 24.0);
    let hot = if sub.is_some() { ui.hover_amount(("focus-sub-note", p.id, i), r) } else { 0.0 };
    if hot > 0.01 {
        ui.round(r, radius::CONTROL, t.text.with_alpha(0.05 * hot));
    }
    let (glyph, ink) = match state {
        SubagentState::Done => ("\u{F012C}", t.good),
        SubagentState::Failed => ("\u{F0028}", t.danger),
        _ => ("\u{F04DB}", c.faint),
    };
    ui.icon(Rect::new(x, mid - 8.0, 16.0, 16.0), glyph, c.small, ink.fade(0.85));
    ui.draw_color(&b, Point::new(x + 22.0, mid - b.h / 2.0), c.faint.mix(c.text, 0.75 * hot));
    let line_w = ((w - width) / 2.0 - 22.0).clamp(0.0, 120.0);
    ui.hline(r.x - 6.0 - line_w, mid, line_w, t.hairline);
    ui.hline(r.right() + 6.0, mid, line_w, t.hairline);
    if sub.is_some() {
        let target = open_target(&mut p.opened, call);
        ui.hit(r, Action::Focus(p.id, target));
    }
    h
}

/// The head of a subagent's own conversation, where the session's folder heads the session's (as tall): the way
/// back, the subagent (its mark and what it was asked), and on the right what it goes by, its model and how long it
/// has taken. Under it a hairline.
pub(super) fn head(ui: &mut Ui, c: &Typo, id: PaneId, s: &Subagent, at: Point, w: f32) {
    let t = ui.t;
    let row_h = (c.small * 1.7).round();
    let mid = at.y + row_h / 2.0;
    // the way back
    let label = "Conversation";
    let st = Style::mono(c.small, c.muted);
    let lw = ui.measure(label, st);
    let back = Rect::new(at.x - 6.0, mid - 13.0, 6.0 + 18.0 + lw + 10.0, 26.0);
    let hot = ui.hover_amount(("focus-sub-back", id), back);
    if hot > 0.01 {
        ui.round(back, radius::CONTROL, t.text.with_alpha(0.06 * hot));
    }
    let ink = c.muted.mix(c.text, hot);
    ui.icon(Rect::new(at.x - 2.0, mid - 8.0, 16.0, 16.0), "\u{F0141}", c.small + 2.0, ink);
    let b = ui.line(label, st.color(ink), None);
    ui.draw(&b, Point::new(at.x + 18.0, mid - b.h / 2.0));
    ui.hit(back, Action::Focus(id, BACK));
    if hot > 0.6 {
        widgets::tooltip(ui, back, "Back to the conversation · esc");
    }
    let mut x = back.right() + 6.0;
    let slash = ui.line("/", Style::mono(c.small, c.faint), None);
    ui.draw(&slash, Point::new(x, mid - slash.h / 2.0));
    x += slash.w + 12.0;
    mark(ui, s, Point::new(x + 7.0, mid), c.small + 1.0);
    x += 22.0;
    // on the right: what it goes by, its model, how long
    let mut parts: Vec<String> = vec![];
    if let Some(n) = s.name.as_deref().filter(|n| !n.eq_ignore_ascii_case(&s.title)).or(s.kind.as_deref()) {
        parts.push(n.to_string());
    }
    if let Some(m) = &s.model {
        parts.push(model_name(m));
    }
    if let Some(ms) = s.elapsed_ms(c.now_ms).filter(|ms| *ms >= 1000) {
        parts.push(clock(ms / 1000));
    }
    // as much of that as fits beside the whole title (what it goes by goes first, how long last); a title too long
    // for the room keeps just how long, when there's room for that
    let right = at.x + w;
    let mut room = right - x;
    let title_st = Style::mono(c.small, c.text).weight(weight::MEDIUM);
    let faint = Style::mono(c.small, c.faint);
    let tw = ui.measure(&s.title, title_st);
    let mut meta = String::new();
    for skip in 0..parts.len() {
        let m = parts[skip..].join(" · ");
        if tw + 16.0 + ui.measure(&m, faint) <= room {
            meta = m;
            break;
        }
    }
    if meta.is_empty()
        && let Some(last) = parts.last()
        && room - ui.measure(last, faint) - 16.0 >= 160.0
    {
        meta = last.clone();
    }
    if !meta.is_empty() {
        let mb = ui.line(&meta, faint, None);
        ui.draw(&mb, Point::new(right - mb.w, mid - mb.h / 2.0));
        room -= mb.w + 16.0;
    }
    let title = ui.line(&s.title, title_st, Some(room.max(30.0)));
    ui.draw(&title, Point::new(x, mid - title.h / 2.0));
    if title.layout.truncated && ui.hovered(Rect::new(x, mid - 12.0, title.w, 24.0)) {
        widgets::tooltip(ui, Rect::new(x, mid - 12.0, title.w, 24.0), &s.title);
    }
    ui.hline(at.x, at.y + row_h + 8.0, w, t.hairline);
}

/// How a subagent's own conversation ends once it's over: "Finished in 9m 40s · 41 tools · 1.2M tokens".
pub(super) fn ending(s: &Subagent, now_ms: i64) -> Option<String> {
    let verb = match s.state {
        SubagentState::Running => return None,
        SubagentState::Done => "Finished in",
        SubagentState::Failed => "Failed after",
        SubagentState::Stopped => "Stopped after",
    };
    let mut parts: Vec<String> = vec![];
    match s.elapsed_ms(now_ms).filter(|ms| *ms >= 1000) {
        Some(ms) => parts.push(format!("{verb} {}", clock(ms / 1000))),
        None => parts.push(verb.trim_end_matches(" in").trim_end_matches(" after").to_string()),
    }
    if let Some(n) = s.tool_uses.filter(|n| *n > 0) {
        parts.push(if n == 1 { "1 tool call".to_string() } else { format!("{n} tool calls") });
    }
    if let Some(n) = s.tokens.filter(|n| *n > 0) {
        parts.push(match n {
            0..=999 => format!("{n} tokens"),
            1_000..=999_999 => format!("{}K tokens", n / 1000),
            _ => format!("{:.1}M tokens", n as f64 / 1e6),
        });
    }
    Some(parts.join(" · "))
}

/// What a background task's chip says after its name: what a watch saw last, else how long it's been going.
fn task_news(task: &Task, now_ms: i64) -> String {
    let time = task.elapsed_ms(now_ms).filter(|ms| *ms >= 1000).map(|ms| clock(ms / 1000));
    match (&task.news, time) {
        (Some(news), _) if task.kind == TaskKind::Watch => news.clone(),
        (_, Some(time)) => time,
        _ => "Just started".to_string(),
    }
}

/// A background task's tooltip: what it is, what it runs, for how long, and where its output goes.
fn task_tip(task: &Task, now_ms: i64) -> String {
    let what = match task.kind {
        TaskKind::Command => "Command running in the background",
        TaskKind::Watch => "Watching for output",
        TaskKind::Workflow => "Workflow running in the background",
    };
    let mut lines = vec![task.title.clone()];
    if let Some(cmd) = task.detail.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        let cut: String = cmd.chars().take(160).collect();
        lines.push(if cut.len() < cmd.len() { format!("{cut}…") } else { cut });
    }
    let mut last = what.to_string();
    if let Some(ms) = task.elapsed_ms(now_ms).filter(|ms| *ms >= 1000) {
        last = format!("{last} · {}", clock(ms / 1000));
    }
    if let Some(news) = task.news.as_deref().filter(|_| task.kind == TaskKind::Watch) {
        last = format!("{last}\nLast seen: {news}");
    }
    lines.push(last);
    if task.output.is_some() {
        lines.push("Click to read its output".to_string());
    }
    lines.join("\n")
}

/// The most any one of `widths` gets for all of them to fit in `room`: the narrow ones keep what they need, the rest
/// share what's left evenly.
fn share(widths: &[f32], room: f32) -> f32 {
    let mut sorted = widths.to_vec();
    sorted.sort_by(f32::total_cmp);
    let mut left = room;
    for (i, w) in sorted.iter().enumerate() {
        let even = left / (sorted.len() - i) as f32;
        if *w > even {
            return even;
        }
        left -= w;
    }
    f32::MAX
}

/// One chip in the strip: a subagent at work, or a background task.
enum Chip<'a> {
    Sub(&'a Arc<Subagent>),
    Task(&'a Task),
}

impl Chip<'_> {
    fn label(&self) -> &str {
        match self {
            Chip::Sub(s) => s.label(),
            Chip::Task(t) => &t.title,
        }
    }
}

/// The strip over the reply box (`r`): what's at work while the conversation goes on, a chip each. A subagent's says
/// what it goes by and what it's doing (the one showing lit) and opens it; a background task's (a command, a watch,
/// a workflow) says what it is and how long it's been going, or what it saw last, and opens its output. When they
/// don't all fit that way, their names alone (each as short as it takes, down to a point; the rest on hover); then
/// as many as fit, and how many more. Returns the tooltip of the chip under the pointer, for the caller to draw once
/// the strip's clip is off (it shows above the strip).
#[allow(clippy::too_many_arguments)]
pub(super) fn dock(ui: &mut Ui, c: &Typo, id: PaneId, running: &[Arc<Subagent>], tasks: &[Task], r: Rect, shown: Option<&str>, opened: &mut Vec<String>, links: &mut Vec<String>) -> Option<(Rect, String)> {
    let t = ui.t;
    let mut tip = None;
    let h = 28.0;
    let mid = r.y + r.h / 2.0;
    let gap = 8.0;
    let mut x = r.x;
    let name_st = Style::mono(c.small, c.text).weight(weight::MEDIUM);
    let what_st = Style::mono(c.small, c.muted);
    let chips: Vec<Chip> = running.iter().map(Chip::Sub).chain(tasks.iter().map(Chip::Task)).collect();
    let whats: Vec<String> = chips
        .iter()
        .map(|chip| match chip {
            Chip::Sub(s) => activity(s),
            Chip::Task(task) => task_news(task, c.now_ms),
        })
        .collect();
    let count = chips.len() as f32;
    let full: f32 = chips.iter().zip(&whats).map(|(chip, what)| (42.0 + ui.measure(chip.label(), name_st) + ui.measure(&format!(" · {what}"), what_st)).min(CHIP_MAX)).sum::<f32>() + gap * (count - 1.0);
    let names_only = full > r.w;
    let names: Vec<f32> = chips.iter().map(|chip| ui.measure(chip.label(), name_st)).collect();
    let name_cap = share(&names, r.w - count * 42.0 - gap * (count - 1.0)).clamp(NAME_MIN, NAME_MAX);
    for (n, (chip, what)) in chips.iter().zip(&whats).enumerate() {
        let left = chips.len() - n;
        // room for "+N" after this one, unless it's the last
        let reserve = if left > 1 { 52.0 } else { 0.0 };
        let room = (r.right() - x - reserve).min(CHIP_MAX);
        if room < if names_only { 42.0 + NAME_MIN } else { 120.0 } {
            let more = format!("+{left}");
            let b = ui.line(&more, Style::mono(c.small, c.muted), None);
            let mr = Rect::new(x, mid - h / 2.0, b.w + 20.0, h);
            ui.panel(mr, h / 2.0, t.text.with_alpha(0.04), t.hairline);
            ui.draw(&b, Point::new(mr.x + 10.0, mid - b.h / 2.0));
            if ui.hovered(mr) {
                let names: Vec<String> = chips[n..].iter().map(|c| c.label().to_string()).collect();
                tip = Some((mr, names.join("\n")));
            }
            break;
        }
        let key = match chip {
            Chip::Sub(s) => s.call.as_str(),
            Chip::Task(task) => task.id.as_str(),
        };
        let (name, said) = if names_only {
            (ui.line(chip.label(), name_st, Some(name_cap.min(room - 42.0))), None)
        } else {
            // the name takes what what it's doing leaves (that gets up to 45% of the chip)
            let said_text = format!(" · {what}");
            let said_w = ui.measure(&said_text, what_st).min((room - 40.0) * 0.45);
            let name = ui.line(chip.label(), name_st, Some((room - 40.0 - said_w).max(30.0)));
            let said = ui.line(&said_text, what_st, Some((room - 40.0 - name.w).max(20.0)));
            (name, Some(said))
        };
        let said_w = said.as_ref().map_or(0.0, |b| b.w);
        let want = Rect::new(x, mid - h / 2.0, 30.0 + name.w + said_w + 12.0, h);
        // a chip comes in fading and rising a little, and glides when the ones before it come and go
        let key_s = match chip {
            Chip::Sub(s) => s.call.as_str(),
            Chip::Task(task) => task.id.as_str(),
        };
        let slid = ui.spring(("dock-x", id, key_s), want.x, Spring::GLIDE, want.x);
        let came = ui.spring(("dock-in", id, key_s), 1.0, Spring::ARRIVE, 0.0);
        let cr = want.translate(slid - want.x, (1.0 - came) * 6.0);
        let on = matches!(chip, Chip::Sub(s) if shown == Some(s.call.as_str()));
        let hot = ui.hover_amount(("focus-dock", id, key), cr);
        let fill = if on { t.selected } else { t.text.with_alpha(0.045 + 0.05 * hot) };
        ui.panel(cr, h / 2.0, fill, if on { t.accent.with_alpha(0.5) } else { t.hairline });
        ui.spinner(Point::new(cr.x + 15.0, mid), 4.0, 1.4, t.accent);
        ui.draw(&name, Point::new(cr.x + 28.0, mid - name.h / 2.0));
        if let Some(said) = &said {
            ui.draw(said, Point::new(cr.x + 28.0 + name.w, mid - said.h / 2.0));
        }
        match chip {
            Chip::Sub(s) => {
                let target = open_target(opened, &s.call);
                ui.hit(cr, Action::Focus(id, target));
                if hot > 0.6 && (name.layout.truncated || said.as_ref().is_none_or(|b| b.layout.truncated)) {
                    tip = Some((cr, format!("{}\n{what}", s.title)));
                }
            }
            Chip::Task(task) => {
                if let Some(uri) = task.output.as_deref().and_then(|p| crate::panes::links::file_uri(std::path::Path::new(p), None, None)) {
                    ui.hit(cr, Action::Focus(id, super::LINK + links.len()));
                    links.push(uri);
                }
                if hot > 0.6 {
                    tip = Some((cr, task_tip(task, c.now_ms)));
                }
            }
        }
        ui.veil(cr.outset(1.0), h / 2.0, came);
        x = want.right() + gap;
    }
    tip
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(state: SubagentState) -> Subagent {
        Subagent { state, ..Subagent::started("t1", "Add swipe between media".into(), Some(1_000)) }
    }

    #[test]
    fn a_subagent_says_what_its_doing_or_what_it_reported() {
        assert_eq!(activity(&sub(SubagentState::Running)), "Getting started");
        let mut done = sub(SubagentState::Done);
        done.result = Some("## Summary\n\nSwiping between **pictures** and [videos](https://x.y) works now.\n\nTypecheck passes.".into());
        assert_eq!(activity(&done), "Swiping between pictures and videos works now.", "its first words, not its heading");
        done.result = Some("# All done".into());
        assert_eq!(activity(&done), "All done");
        assert_eq!(activity(&sub(SubagentState::Stopped)), "Stopped before it finished");
        assert_eq!(activity(&sub(SubagentState::Failed)), "Stopped on an error");
        assert_eq!(first_line("```\ncargo test\n```"), "cargo test");
    }

    #[test]
    fn its_card_says_what_it_goes_by_and_for_how_long() {
        let mut s = sub(SubagentState::Running);
        assert_eq!(tag(&s, 43_000), "42s");
        s.name = Some("media-swipe".into());
        s.kind = Some("fork".into());
        assert_eq!(tag(&s, 3_601_000), "media-swipe · 1h 00m", "its own name over its sort");
        s.name = None;
        s.state = SubagentState::Done;
        s.ended_ms = Some(66_000);
        assert_eq!(tag(&s, 999_999), "fork · 1m 05s", "a finished one says how long it took");
        assert_eq!(crew(2, 3), "2 subagents working");
        assert_eq!(crew(0, 1), "1 subagent");
        assert_eq!(working(1, &[], 0), "1 subagent working");
    }

    #[test]
    fn background_tasks_say_what_they_are_and_how_long_theyve_gone() {
        let build = Task { id: "b1".into(), title: "Build the release".into(), started_ms: Some(1_000), ..Task::default() };
        assert_eq!(working(0, std::slice::from_ref(&build), 251_000), "Build the release · 4m 10s", "one task alone says what it is");
        assert_eq!(working(0, &[build.clone(), build.clone()], 0), "2 background tasks running");
        assert_eq!(working(2, std::slice::from_ref(&build), 0), "2 subagents and 1 task working");
        assert_eq!(task_news(&build, 61_000), "1m 00s");
        assert_eq!(share(&[40.0, 200.0, 300.0], 340.0), 150.0, "a short name keeps its room, the long ones share the rest");
        assert_eq!(share(&[40.0, 50.0], 340.0), f32::MAX, "all of them fit");
        let watch = Task { kind: TaskKind::Watch, news: Some("sheet ready: b.png".into()), ..build.clone() };
        assert_eq!(task_news(&watch, 61_000), "sheet ready: b.png", "a watch says what it saw last");
        let tip = task_tip(&Task { detail: Some("cargo build --release".into()), output: Some(r"C:\t\b1.output".into()), ..build }, 61_000);
        assert_eq!(tip, "Build the release\ncargo build --release\nCommand running in the background · 1m 00s\nClick to read its output");
    }

    #[test]
    fn its_end_says_how_it_went() {
        let mut s = sub(SubagentState::Done);
        s.ended_ms = Some(581_000);
        s.tool_uses = Some(41);
        s.tokens = Some(1_234_000);
        assert_eq!(ending(&s, 0).as_deref(), Some("Finished in 9m 40s · 41 tool calls · 1.2M tokens"));
        assert_eq!(ending(&sub(SubagentState::Running), 0), None);
        let mut stopped = sub(SubagentState::Stopped);
        stopped.ended_ms = Some(500);
        assert_eq!(ending(&stopped, 0).as_deref(), Some("Stopped"));
    }

    #[test]
    fn the_frames_subagents_are_found_by_the_call_that_started_them() {
        let followed = Arc::new(vec![Arc::new(sub(SubagentState::Running))]);
        let mut conv = Conversation::new(bro_core::Harness::Claude);
        conv.subagents.push(Subagent::started("t2", "Find the routes".into(), Some(2_000)));
        conv.subagents.push(Subagent::started("t1", "stale".into(), None));
        let subs = Subs::new(Some(&followed), &[Some(&conv)]);
        assert_eq!(subs.all.len(), 2);
        assert_eq!(subs.get("t1").map(|s| s.title.as_str()), Some("Add swipe between media"), "what's followed wins");
        assert_eq!(subs.get("t2").map(|s| s.title.as_str()), Some("Find the routes"), "one not followed yet still has its card");
        assert_eq!(subs.running().len(), 2);
        let mut opened = vec![];
        assert_eq!((open_target(&mut opened, "t1"), open_target(&mut opened, "t2"), open_target(&mut opened, "t1")), (SUBAGENT, SUBAGENT + 1, SUBAGENT));
    }
}
