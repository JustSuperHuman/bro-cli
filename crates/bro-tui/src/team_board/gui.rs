//! The agent team board in the desktop app (prefix M): the lead's teammates as avatars in Claude's colours for
//! them, each with what it's doing, then the task list lane by lane (in progress, up next, blocked, done) with
//! its progress, on a card over the dimmed window. Keys move the selection; a selected task opens up to show
//! its description. As in the terminal UI, enter goes to the lead and s opens agent teams in the settings.

use super::{Item, Lane, Outcome, TeamView, doing, items, kind, owner_name};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{self, Style, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::file_view::gui::{keycap, keycap_w, para_centered};
use bro_core::agent_teams::{Member, Task, Team};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use crossterm::event::KeyModifiers;
use std::cell::RefCell;

// click targets: the hints, then one per line
const GO: usize = 1;
const SETTINGS: usize = 2;
const CLOSE: usize = 3;
const ROW: usize = 100;

const MARGIN: f32 = 48.0;
const MIN_W: f32 = 540.0;
const MAX_W: f32 = 860.0;
const TITLE_H: f32 = 72.0;
const FOOT_H: f32 = 52.0;
const PAD: f32 = 24.0;
const INSET: f32 = 12.0;
const HEAD_H: f32 = 34.0;
const LANE_H: f32 = 26.0;
const GAP_H: f32 = 12.0;
const MEMBER_H: f32 = 52.0;
const TASK_H: f32 = 36.0;
const LIST_PAD: f32 = 4.0;
const EMPTY_H: f32 = 210.0;
const GLIDE: f32 = 0.14;
const OPEN: f64 = 0.16;

const ICON_TEAM: &str = "\u{F0849}";
const ICON_LEAD: &str = "\u{F04CE}";
const ICON_DONE: &str = "\u{F012C}";
const ICON_BLOCKED: &str = "\u{F051F}";
const ICON_PLAN: &str = "\u{F0EBF}";

/// Draw the team board over the whole window.
pub(crate) fn draw(s: &mut TeamView, ui: &mut Ui, area: Rect, _cx: &Cx) -> bool {
    paint(s, ui, area)
}

/// Target `n` was clicked: a line selects; the hints go, open the settings or close.
pub(crate) fn click(s: &mut TeamView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32) -> Outcome {
    match n {
        CLOSE => Outcome::Close,
        GO if s.why.is_none() => Outcome::Go,
        SETTINGS => Outcome::Settings,
        n if n >= ROW && n - ROW < s.items().len() => {
            s.sel = n - ROW;
            Outcome::None
        }
        _ => Outcome::None,
    }
}

/// The wheel moves the selection three lines, as in the terminal UI.
pub(crate) fn wheel(s: &mut TeamView, _n: Option<usize>, down: bool) {
    s.move_by(if down { 3 } else { -3 });
}

// ------------------------------------------------------------------ layout

#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Gap,
    Members,
    Tasks,
    Lane(Lane),
    Item(usize),
}

fn lines(team: &Team, list: &[Item]) -> Vec<Line> {
    let mut out = vec![Line::Members];
    let mut lane = None;
    for (i, it) in list.iter().enumerate() {
        if let Item::Task(ti) = it {
            let l = Lane::of(team, &team.tasks[*ti]);
            if lane.is_none() {
                out.extend([Line::Gap, Line::Tasks]);
            }
            if lane != Some(l) {
                lane = Some(l);
                out.push(Line::Lane(l));
            }
        }
        out.push(Line::Item(i));
    }
    out
}

/// A selected task with a description opens up to show it.
fn opened(s: &TeamView, team: &Team, list: &[Item], i: usize) -> Option<String> {
    match list[i] {
        Item::Task(ti) if i == s.sel => Some(team.tasks[ti].description.trim().to_string()).filter(|d| !d.is_empty()),
        _ => None,
    }
}

fn line_h(ui: &mut Ui, s: &TeamView, team: &Team, list: &[Item], line: Line, w: f32) -> f32 {
    match line {
        Line::Gap => GAP_H,
        Line::Members | Line::Tasks => HEAD_H,
        Line::Lane(_) => LANE_H,
        Line::Item(i) => match list[i] {
            Item::Member(_) => MEMBER_H,
            Item::Task(_) => match opened(s, team, list, i) {
                Some(d) => TASK_H + ui.para(&d, Style::new(size::SMALL, weight::REGULAR, ui.t.muted), task_text_w(w), Some(4)).h + 8.0,
                None => TASK_H,
            },
        },
    }
}

/// Room for a task's description: the row less its mark and the owner's column.
fn task_text_w(w: f32) -> f32 {
    (w - 2.0 * INSET - 72.0 - 150.0).max(120.0)
}

struct Layout {
    card: Rect,
    title: Rect,
    list: Rect,
    foot: Rect,
}

fn layout(window: Rect, content: Option<f32>, lift: f32) -> Layout {
    let w = (window.w - 2.0 * MARGIN).clamp(MIN_W, MAX_W).min(window.w - 24.0).max(1.0).round();
    let room = (window.h - 2.0 * MARGIN - TITLE_H - FOOT_H).max(TASK_H * 3.0);
    let list_h = content.unwrap_or(EMPTY_H).min(room).round();
    let h = TITLE_H + list_h + FOOT_H;
    let card = Rect::new((window.x + (window.w - w) / 2.0).round(), (window.y + ((window.h - h) * 0.4).max(0.0) + lift).round(), w, h);
    let title = Rect::new(card.x, card.y, card.w, TITLE_H);
    let list = Rect::new(card.x, title.bottom(), card.w, list_h);
    let foot = Rect::new(card.x, list.bottom(), card.w, FOOT_H);
    Layout { card, title, list, foot }
}

// ------------------------------------------------------------------ motion

#[derive(Default)]
struct Motion {
    id: u64,
    opened: f64,
    at: f64,
    moving: bool,
    off: f32,
    shown: f32,
}

thread_local! {
    static MOTION: RefCell<Motion> = RefCell::new(Motion::default());
}

/// The open animation (0 → 1) and the offset to draw the list at, gliding to keep line `sel` in view.
fn motion(ui: &mut Ui, s: &TeamView, view: f32, content: f32, tops: &[f32], heights: &[f32], sel: Option<usize>) -> (f32, f32) {
    let id = kit::id((s as *const TeamView as usize, s.pane));
    let now = ui.time;
    let (open, shown, moving) = MOTION.with(|cell| {
        let mut m = cell.borrow_mut();
        if m.id != id {
            *m = Motion { id, opened: now, at: now, ..Motion::default() };
        }
        m.off = crate::agent_files::gui::keep(m.off, view, content, tops, heights, sel);
        let dt = if m.moving { (now - m.at).clamp(0.0, 0.034) as f32 } else { 1.0 / 60.0 };
        m.at = now;
        let to = m.off;
        if (to - m.shown).abs() > view * 1.5 || (to - m.shown).abs() < 0.3 {
            m.shown = to;
        } else {
            m.shown += (to - m.shown) * (1.0 - (-dt * 4.6 / GLIDE).exp());
        }
        m.moving = m.shown != to;
        let open = ((now - m.opened) / OPEN).clamp(0.0, 1.0) as f32;
        (1.0 - (1.0 - open).powi(3), m.shown, m.moving)
    });
    if moving || open < 1.0 {
        ui.ticking = true;
    }
    (open, shown)
}

// ------------------------------------------------------------------ drawing

/// Claude's colour for a member, else the theme's: the lead in Claude's own orange.
fn color_of(ui: &Ui, m: &Member) -> Color {
    match m.rgb() {
        Some((r, g, b)) => Color::rgb8(r, g, b),
        None if m.lead => Color::rgb8(0xD9, 0x77, 0x57),
        None => ui.t.inline,
    }
}

/// [`draw`] without the app's context (the screenshot test draws it too).
pub(crate) fn paint(s: &mut TeamView, ui: &mut Ui, area: Rect) -> bool {
    let team = s.team.clone().filter(|_| !s.is_empty());
    let list = team.as_deref().map(items).unwrap_or_default();
    if s.sel >= list.len() {
        s.sel = list.len().saturating_sub(1);
    }
    let w = layout(area, None, 0.0).card.w;
    let (lines, heights) = match team.as_deref() {
        Some(team) => {
            let lines = lines(team, &list);
            let heights: Vec<f32> = lines.iter().map(|l| line_h(ui, s, team, &list, *l, w)).collect();
            (lines, heights)
        }
        None => (vec![], vec![]),
    };
    let mut tops = Vec::with_capacity(lines.len());
    let mut y = LIST_PAD;
    for h in &heights {
        tops.push(y);
        y += h;
    }
    let content = team.is_some().then_some(y + LIST_PAD);
    let rest = layout(area, content, 0.0);
    s.set_rows(((rest.list.h / TASK_H).floor() as usize).saturating_sub(1));
    let sel = lines.iter().position(|l| *l == Line::Item(s.sel));
    let (open, off) = motion(ui, s, rest.list.h, content.unwrap_or(0.0), &tops, &heights, sel);
    let l = layout(area, content, ((1.0 - open) * 10.0).round());

    widgets::scrim(ui, area, open, Action::None);
    widgets::card(ui, l.card, radius::OVERLAY);
    title(ui, s, team.as_deref(), l.title);
    match (team.as_deref(), content) {
        (Some(team), Some(content)) => {
            let t = ui.t;
            let hair = ui.hair();
            ui.clip(l.list, 0.0);
            for (i, line) in lines.iter().enumerate() {
                let r = Rect::new(l.list.x + INSET, l.list.y + tops[i] - off, l.list.w - 2.0 * INSET, heights[i]);
                if r.bottom() < l.list.y || r.y > l.list.bottom() {
                    continue;
                }
                let label_r = Rect::new(l.list.x + PAD, r.y, l.list.w - 2.0 * PAD, r.h);
                match *line {
                    Line::Gap => {}
                    Line::Members => members_header(ui, label_r, team),
                    Line::Tasks => tasks_header(ui, label_r, team),
                    Line::Lane(lane) => lane_label(ui, label_r, team, lane),
                    Line::Item(n) => match list[n] {
                        Item::Member(mi) => member_row(ui, r, team, &team.members[mi], n, n == s.sel),
                        Item::Task(ti) => task_row(ui, r, team, &team.tasks[ti], n, n == s.sel, opened(s, team, &list, n)),
                    },
                }
            }
            ui.unclip();
            if off > 0.5 {
                ui.hline(l.list.x + hair, l.list.y, l.list.w - 2.0 * hair, t.hairline);
            }
            if off + l.list.h < content - 0.5 {
                ui.hline(l.list.x + hair, l.list.bottom() - hair, l.list.w - 2.0 * hair, t.hairline);
            }
            let hot = ui.hover_amount("team-board-scroll", l.list);
            widgets::scrollbar(ui, Rect::new(l.list.x, l.list.y, l.list.w - 3.0, l.list.h), content, l.list.h, off, 0.6 + 0.4 * hot);
        }
        _ => empty(ui, s, l.list),
    }
    hints(ui, s, l.foot);
    true
}

/// The team mark, "Agent team · {lead}", and on the right how many are working.
fn title(ui: &mut Ui, s: &TeamView, team: Option<&Team>, r: Rect) {
    let t = ui.t;
    let mark = Rect::new(r.x + PAD - 4.0, r.y + ((r.h - 34.0) / 2.0).round() + 2.0, 34.0, 34.0);
    ui.round(mark, 9.0, t.accent.with_alpha(if t.light { 0.12 } else { 0.18 }));
    ui.icon(mark, ICON_TEAM, 19.0, t.accent);
    let mut right = r.right() - PAD;
    if let Some(team) = team.filter(|team| !team.is_empty()) {
        let mates = team.teammates().count();
        let working = team.working();
        let (text, c) = match working {
            0 => (format!("{mates} idle"), t.muted),
            n => (format!("{n} of {mates} working"), t.accent),
        };
        let st = Style::new(size::SMALL, weight::SEMIBOLD, c);
        let tw = ui.measure(&text, st);
        let pill = Rect::new(right - tw - 30.0, r.y + (r.h - 26.0) / 2.0 + 2.0, tw + 30.0, 26.0);
        ui.round(pill, 13.0, c.with_alpha(0.12));
        let dot = Point::new(pill.x + 12.0, pill.y + pill.h / 2.0);
        if working > 0 {
            ui.pulse(dot, 3.2, c);
        } else {
            ui.circle(dot, 3.2, c);
        }
        ui.label(Point::new(pill.x + 21.0, pill.y + (pill.h - 17.0) / 2.0), &text, st);
        right = pill.x - 12.0;
    }
    let x = mark.right() + 12.0;
    let st = Style::new(size::HEADING, weight::SEMIBOLD, t.text);
    let spans = vec![("Agent team".to_string(), st), (" · ".to_string(), st.weight(weight::REGULAR).color(t.faint)), (s.title.clone(), st.weight(weight::MEDIUM).color(t.muted))];
    let b = ui.rich(&spans, size::HEADING, Some((right - x).max(40.0)), false, None);
    ui.draw(&b, Point::new(x, mark.y + (mark.h - b.h) / 2.0));
}

/// A heading as the agent files list titles its sections: sentence case, semibold, a quiet count after it.
fn heading(ui: &mut Ui, r: Rect, label: &str, count: &str) -> f32 {
    let t = ui.t;
    let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.35));
    let b = ui.line(label, st, None);
    let y = (r.bottom() - 8.0 - b.h).round();
    ui.draw(&b, Point::new(r.x, y));
    let c = ui.line(count, Style::new(size::CAPTION, weight::MEDIUM, t.faint), None);
    ui.draw(&c, Point::new(r.x + b.w + 6.0, y + ((b.h - c.h) / 2.0).round()));
    y + b.h / 2.0
}

fn members_header(ui: &mut Ui, r: Rect, team: &Team) {
    let n = team.teammates().count();
    heading(ui, r, "Teammates", &format!("{n} and the lead"));
}

/// "Tasks  3 of 5 done", the progress across to the right.
fn tasks_header(ui: &mut Ui, r: Rect, team: &Team) {
    let t = ui.t;
    let c = team.counts();
    let mid = heading(ui, r, "Tasks", &format!("{} of {} done", c.done, c.total));
    let bw = (r.w * 0.32).clamp(80.0, 220.0);
    let bar = Rect::new(r.right() - bw, mid - 3.0, bw, 6.0);
    let frac = if c.total == 0 { 0.0 } else { c.done as f32 / c.total as f32 };
    ui.round(bar, 3.0, t.text.with_alpha(0.08));
    // done, then in progress after it, softer
    let active = if c.total == 0 { 0.0 } else { c.active as f32 / c.total as f32 };
    if frac + active > 0.0 {
        ui.round(Rect::new(bar.x, bar.y, (bar.w * (frac + active)).max(6.0), bar.h), 3.0, t.accent.with_alpha(0.35));
    }
    if frac > 0.0 {
        ui.round(Rect::new(bar.x, bar.y, (bar.w * frac).max(6.0), bar.h), 3.0, t.good);
    }
}

fn lane_color(ui: &Ui, lane: Lane) -> Color {
    let t = ui.t;
    match lane {
        Lane::Active => t.accent,
        Lane::Next => t.muted,
        Lane::Blocked => t.danger.mix(t.muted, 0.35),
        Lane::Done => t.good,
    }
}

fn lane_label(ui: &mut Ui, r: Rect, team: &Team, lane: Lane) {
    let n = team.tasks.iter().filter(|x| Lane::of(team, x) == lane).count();
    let c = lane_color(ui, lane);
    let st = Style::new(size::CAPTION, weight::SEMIBOLD, c);
    let label = lane.label().to_uppercase();
    let b = ui.line(&label, st, None);
    let y = (r.y + (r.h - b.h) / 2.0 + 2.0).round();
    ui.circle(Point::new(r.x + 3.0, y + b.h / 2.0), 3.0, c);
    ui.draw(&b, Point::new(r.x + 12.0, y));
    let count = ui.line(&n.to_string(), Style::new(size::CAPTION, weight::MEDIUM, ui.t.faint), None);
    ui.draw(&count, Point::new(r.x + 12.0 + b.w + 6.0, y));
}

/// A member: an avatar in its colour with its status on it, its name and what it was spawned as, what it's doing
/// under that, and its status in a word on the right.
fn member_row(ui: &mut Ui, r: Rect, team: &Team, m: &Member, i: usize, selected: bool) {
    let t = ui.t;
    widgets::row(ui, r, ("team-member", i), selected, Action::Overlay(ROW + i));
    let c = color_of(ui, m);
    let center = Point::new(r.x + PAD - INSET + 15.0, r.y + r.h / 2.0);
    ui.circle(center, 15.0, c.with_alpha(if t.light { 0.16 } else { 0.22 }));
    ui.ring(center, 15.0, 1.2, c.with_alpha(0.55));
    let initial: String = m.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
    if m.lead {
        ui.icon(Rect::new(center.x - 10.0, center.y - 10.0, 20.0, 20.0), ICON_LEAD, 15.0, c);
    } else {
        let st = Style::new(size::TITLE, weight::BOLD, c);
        let b = ui.line(&initial, st, None);
        ui.draw(&b, Point::new((center.x - b.w / 2.0).round(), (center.y - b.h / 2.0).round()));
    }
    // the status, on the avatar's corner: a spinner while it works, a quiet dot when idle
    let badge = Point::new(center.x + 11.0, center.y + 11.0);
    if !m.lead {
        ui.circle(badge, 6.5, t.raised);
        if m.active {
            ui.spinner(badge, 3.6, 1.4, t.accent);
        } else {
            ui.circle(badge, 3.2, t.faint);
        }
    }

    // right: the status in a word
    let (word, wc) = match (m.lead, m.active) {
        (true, _) => ("lead", Color::rgb8(0xD9, 0x77, 0x57)),
        (false, true) => ("working", t.accent),
        (false, false) => ("idle", t.faint),
    };
    let wst = Style::new(size::SMALL, weight::MEDIUM, wc);
    let ww = ui.measure(word, wst);
    let right = r.right() - 14.0;
    ui.label(Point::new(right - ww, r.y + (r.h - 17.0) / 2.0), word, wst);

    // the name and what it was spawned as, then what it's doing
    let x0 = center.x + 15.0 + 14.0;
    let room = (right - ww - 16.0 - x0).max(60.0);
    let mut spans = vec![(m.name.clone(), Style::new(size::BODY, weight::SEMIBOLD, if selected { c } else { t.text }))];
    let k = kind(m);
    if !k.is_empty() {
        spans.push((format!("   {k}"), Style::new(size::CAPTION, weight::MEDIUM, t.faint)));
    }
    let name = ui.rich(&spans, size::BODY, Some(room), false, Some(1));
    let what = doing(team, m);
    let busy = m.active && !m.lead;
    let sub = ui.line(&what, Style::new(size::SMALL, weight::REGULAR, if busy { t.text.mix(t.muted, 0.25) } else { t.muted }), Some(room - if m.plan_mode { 18.0 } else { 0.0 }));
    let top = (r.y + (r.h - name.h - 2.0 - sub.h) / 2.0).round();
    ui.draw(&name, Point::new(x0, top));
    let mut sx = x0;
    if m.plan_mode {
        ui.icon(Rect::new(sx - 2.0, top + name.h + 2.0, 16.0, sub.h), ICON_PLAN, size::SMALL, t.faint);
        sx += 16.0;
    }
    ui.draw(&sub, Point::new(sx, top + name.h + 2.0));
}

/// A task: its lane's mark, its number and subject (the description under it when it's selected), and on the
/// right who has it (or what it waits on).
fn task_row(ui: &mut Ui, r: Rect, team: &Team, task: &Task, i: usize, selected: bool, open: Option<String>) {
    let t = ui.t;
    widgets::row(ui, r, ("team-task", i), selected, Action::Overlay(ROW + i));
    let lane = Lane::of(team, task);
    let lc = lane_color(ui, lane);
    let head = Rect::new(r.x, r.y, r.w, TASK_H);
    let mid = head.y + head.h / 2.0;
    let mark = Point::new(r.x + PAD - INSET + 8.0, mid);
    match lane {
        Lane::Active => ui.spinner(mark, 4.4, 1.6, lc),
        Lane::Next => ui.ring(mark, 5.0, 1.4, lc.with_alpha(0.8)),
        Lane::Blocked => ui.icon(Rect::new(mark.x - 8.0, mark.y - 8.0, 16.0, 16.0), ICON_BLOCKED, 13.0, lc),
        Lane::Done => ui.icon(Rect::new(mark.x - 8.0, mark.y - 8.0, 16.0, 16.0), ICON_DONE, 14.0, lc),
    }

    // right: who has it, or what it waits on
    let right = r.right() - 14.0;
    let mut used = 0.0;
    match owner_name(team, task) {
        Some(o) => {
            let oc = team.member(o).map(|m| color_of(ui, m)).unwrap_or(t.inline);
            let st = Style::new(size::SMALL, weight::MEDIUM, if lane == Lane::Done { t.muted } else { t.text.mix(t.muted, 0.15) });
            let ow = ui.measure(o, st).min(120.0);
            let pill = Rect::new(right - ow - 24.0, mid - 11.0, ow + 24.0, 22.0);
            ui.round(pill, 11.0, oc.with_alpha(if t.light { 0.12 } else { 0.16 }));
            ui.circle(Point::new(pill.x + 10.0, mid), 3.5, oc);
            ui.label_in(Rect::new(pill.x + 18.0, pill.y, ow + 2.0, pill.h), o, st, Align::Left);
            used = pill.w + 10.0;
        }
        None if lane != Lane::Done => {
            let st = Style::new(size::SMALL, weight::MEDIUM, t.faint);
            let w = ui.measure("unclaimed", st);
            ui.label(Point::new(right - w, mid - 8.0), "unclaimed", st);
            used = w + 10.0;
        }
        None => {}
    }
    if lane == Lane::Blocked && !task.blocked_by.is_empty() {
        let text = format!("after #{}", task.blocked_by.join(", #"));
        let st = Style::new(size::CAPTION, weight::MEDIUM, lc);
        let w = ui.measure(&text, st);
        ui.label(Point::new(right - used - w, mid - 7.5), &text, st);
        used += w + 10.0;
    }

    // the number and subject
    let x0 = mark.x + 16.0;
    let id_st = Style::mono(size::SMALL, t.faint);
    let id = ui.line(&format!("#{}", task.id), id_st, None);
    ui.draw(&id, Point::new(x0, (mid - id.h / 2.0).round()));
    let sx = x0 + id.w.max(22.0) + 8.0;
    let subject_c = match lane {
        Lane::Done => t.muted,
        _ if selected => t.text,
        _ => t.text.mix(t.muted, 0.1),
    };
    let subject = ui.line(&task.subject, Style::new(size::BODY, if selected { weight::SEMIBOLD } else { weight::MEDIUM }, subject_c), Some((right - used - 12.0 - sx).max(40.0)));
    ui.draw(&subject, Point::new(sx, (mid - subject.h / 2.0).round()));
    if let Some(d) = open {
        let b = ui.para(&d, Style::new(size::SMALL, weight::REGULAR, t.muted), task_text_w(r.w + 2.0 * INSET), Some(4));
        ui.draw(&b, Point::new(sx, head.bottom() - 2.0));
    }
}

/// Why there's no board, in the middle of the card, with the prompt that starts one.
fn empty(ui: &mut Ui, s: &TeamView, r: Rect) {
    let t = ui.t;
    let text = s.empty_text();
    let (why, example) = match text.split_once('\n') {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (text, None),
    };
    let w = (r.w - 96.0).clamp(80.0, 560.0);
    let msg = para_centered(ui, &why, Style::new(size::BODY, weight::MEDIUM, t.muted), w, 3);
    let ex = example.map(|e| para_centered(ui, &e, Style::new(size::BODY, weight::REGULAR, t.text.mix(t.muted, 0.2)), w - 40.0, 3));
    let ex_h = ex.as_ref().map_or(0.0, |b| 16.0 + b.h + 24.0);
    let h = 48.0 + 14.0 + msg.h + ex_h;
    let y = (r.y + (r.h - h) / 2.0).round();
    let cx = r.x + r.w / 2.0;
    let mark = Rect::new((cx - 24.0).round(), y, 48.0, 48.0);
    ui.round(mark, 12.0, t.text.with_alpha(0.05));
    ui.icon(mark, ICON_TEAM, 28.0, t.faint);
    let my = y + 48.0 + 14.0;
    ui.draw(&msg, Point::new((cx - w / 2.0).round(), my));
    if let Some(b) = ex {
        // the prompt, as a quote you'd type to the lead
        let qr = Rect::new((cx - w / 2.0 + 10.0).round(), my + msg.h + 16.0, w - 20.0, b.h + 24.0);
        ui.round(qr, radius::OVERLAY, t.accent.with_alpha(if t.light { 0.06 } else { 0.08 }));
        ui.top_fill(Rect::new(qr.x, qr.y, 3.0, qr.h), 1.5, t.accent.with_alpha(0.6));
        ui.draw(&b, Point::new((cx - (w - 40.0) / 2.0).round(), qr.y + 12.0));
    }
}

/// "⏎ go to the lead · s team settings · ↑↓ move · esc close" bottom-left; all but move take clicks too.
fn hints(ui: &mut Ui, s: &TeamView, r: Rect) {
    let t = ui.t;
    let mut hints = vec![];
    if s.why.is_none() {
        hints.push(("⏎", "go to the lead", Some(GO)));
    }
    hints.extend([("s", "team settings", Some(SETTINGS)), ("↑↓", "move", None), ("esc", "close", Some(CLOSE))]);
    let mut x = r.x + PAD - 8.0;
    for (key, label, target) in hints {
        let kw = keycap_w(ui, key);
        let b = ui.line(label, Style::new(size::SMALL, weight::MEDIUM, t.muted), None);
        let hr = Rect::new(x, r.y + ((r.h - 28.0) / 2.0).round(), (8.0 + kw + 7.0 + b.w + 9.0).round(), 28.0);
        if let Some(n) = target {
            let hot = ui.hover_amount(("team-board-hint", n), hr);
            if hot > 0.0 {
                ui.round(hr, radius::CONTROL, t.hover.fade(hot));
            }
            ui.hit(hr, Action::Overlay(n));
        }
        keycap(ui, Point::new(hr.x + 8.0, hr.y + 5.0), key, t.muted, t.control);
        ui.draw(&b, Point::new(hr.x + 8.0 + kw + 7.0, hr.y + (hr.h - b.h) / 2.0));
        x = hr.right() + 6.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_view::gui::tests::{targets, with_ui};
    use std::sync::Arc;

    fn view() -> TeamView {
        TeamView::new(1, "Refactor auth".into(), Some(Arc::new(crate::team_board::tests::team())), true, None)
    }

    #[test]
    fn lines_head_members_then_lanes() {
        let v = view();
        let team = v.team.clone().unwrap();
        let list = items(&team);
        let l = lines(&team, &list);
        assert_eq!(&l[..4], &[Line::Members, Line::Item(0), Line::Item(1), Line::Item(2)]);
        assert_eq!(&l[4..7], &[Line::Gap, Line::Tasks, Line::Lane(Lane::Active)]);
        assert!(l.contains(&Line::Lane(Lane::Blocked)) && l.contains(&Line::Lane(Lane::Done)));
        assert_eq!(l.iter().filter(|x| matches!(x, Line::Item(_))).count(), list.len());
    }

    #[test]
    fn every_line_is_drawn_and_clicks_select_go_and_close() {
        let mut v = view();
        let (_, hits) = with_ui("graphite", 1440.0, 900.0, |ui| {
            let r = ui.screen;
            assert!(paint(&mut v, ui, r));
        });
        let t = targets(&hits);
        assert!(t.contains(&GO) && t.contains(&SETTINGS) && t.contains(&CLOSE), "{t:?}");
        assert!((0..7).all(|i| t.contains(&(ROW + i))), "every member and task: {t:?}");
        let p = Point::new(0.0, 0.0);
        assert!(matches!(click(&mut v, ROW + 4, p, KeyModifiers::NONE, 1), Outcome::None));
        assert_eq!(v.sel, 4);
        assert!(matches!(click(&mut v, GO, p, KeyModifiers::NONE, 1), Outcome::Go));
        assert!(matches!(click(&mut v, SETTINGS, p, KeyModifiers::NONE, 1), Outcome::Settings));
        assert!(matches!(click(&mut v, CLOSE, p, KeyModifiers::NONE, 1), Outcome::Close));
        wheel(&mut v, None, true);
        assert_eq!(v.sel, 6, "the wheel stops on the last line");
    }

    #[test]
    fn an_empty_board_offers_settings_and_close() {
        let mut v = TeamView::new(1, "x".into(), None, false, None);
        let (_, hits) = with_ui("paper", 1000.0, 700.0, |ui| {
            let r = ui.screen;
            assert!(paint(&mut v, ui, r));
        });
        assert_eq!(targets(&hits), vec![GO, SETTINGS, CLOSE]);
        let mut shell = TeamView::new(1, "x".into(), None, true, Some("a shell has no team".into()));
        let (_, hits) = with_ui("paper", 1000.0, 700.0, |ui| {
            let r = ui.screen;
            assert!(paint(&mut shell, ui, r));
        });
        assert_eq!(targets(&hits), vec![SETTINGS, CLOSE], "nothing to go to");
    }
}
