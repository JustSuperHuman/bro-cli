//! The Tasks pane uses Bro's service-view controls and typography. It stays in the workspace so other
//! sessions keep running while an outcome is inspected or steered.

use super::{CoordinationView, state_label};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Tokens, Ui, radius, size, weight, widgets};
use crate::folder::{self, gui as picker};
use crate::launcher::gui::{Seg, address_bar, segmented};
use crate::pane::Cx;
use crate::views::profiles::gui::{key_button, key_button_width};
use bro_core::coordination::{QualityMode, TaskState};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const ROW: usize = 100;
const FIELD: usize = 1000;
/// The shared project chooser's targets (`picker::CLOSE`, `picker::FIELD`, `picker::ROW + n`) start here.
const PICKER: usize = 10_000;
const ROW_H: f32 = 62.0;

/// What a state looks like in the list.
fn state_color(t: &Tokens, state: TaskState) -> Color {
    match state {
        TaskState::Verified => t.good,
        TaskState::Failed | TaskState::Uncertain => t.danger,
        TaskState::Blocked | TaskState::Submitted | TaskState::Verifying => t.warn,
        TaskState::Running | TaskState::Assigned => t.accent,
        _ => t.muted,
    }
}

/// The project's folder name, as the sidebar and New Session name it.
fn project_name(project: &str) -> &str {
    let trimmed = project.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).find(|part| !part.is_empty()).unwrap_or(trimmed)
}

pub(crate) fn draw(v: &mut CoordinationView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    v.prepare(cx);
    let t = ui.t;
    let area = area.inset(14.0);
    ui.hit_with(area, Action::View(cx.id, 0), Cursor::Default);
    if area.w < 220.0 || area.h < 140.0 {
        ui.label_in(area, "Enlarge this pane to inspect tasks", Style::new(size::BODY, weight::MEDIUM, t.muted), Align::Center);
        return true;
    }
    let title = ui.label(
        Point::new(area.x, area.y),
        if v.demo { "Tasks · demo examples" } else { "Tasks" },
        Style::new(20.0, weight::SEMIBOLD, t.text),
    );
    let mut y = area.y + title.h + 8.0;
    y += draw_counts(v, ui, area, y);
    if v.form.is_some() {
        draw_form(v, ui, Rect::new(area.x, y + 10.0, area.w, (area.bottom() - y - 10.0).max(0.0)), cx);
        return true;
    }
    y += 12.0;
    // Buttons wrap as a group; actions stay available in narrow split panes.
    let buttons = [
        ("New", "n", 3),
        ("Steer", "s", 4),
        ("Cancel", "x", 5),
        if v.current().is_some_and(|t| matches!(t.state, TaskState::Submitted | TaskState::Verifying)) {
            ("Review", "v", 12)
        } else if v.current().is_some_and(|t| t.state == TaskState::Verified && t.attempt_id.is_some()) {
            ("Retire worker", "z", 13)
        } else {
            ("Retry", "r", 6)
        },
        (if v.context_page { "Outcome" } else { "Context" }, "c", 7),
        (if v.show_finished { "Active" } else { "Finished" }, "a", 2),
        ("Refresh", "F5", 1),
        ("Evidence", "e", 14),
    ];
    let mut x = area.x;
    for (label, key, action) in buttons {
        let w = key_button_width(ui, None, label, Some(key));
        if x > area.x && x + w > area.right() {
            x = area.x;
            y += 38.0;
        }
        key_button(
            ui,
            Rect::new(x, y, w.min(area.w), 32.0),
            None,
            label,
            Some(key),
            if action == 3 { widgets::Kind::Primary } else { widgets::Kind::Ghost },
            v.action_enabled(action).then_some(Action::View(cx.id, action)),
        );
        x += w + 4.0;
    }
    y += 42.0;
    y += draw_status(v, ui, Rect::new(area.x, y, area.w, 0.0));
    let body = Rect::new(area.x, y, area.w, (area.bottom() - y).max(0.0));
    if body.h < 36.0 {
        return true;
    }
    if v.row_indices.is_empty() && !v.context_page {
        let text = if v.snapshot.is_none() {
            "Loading tasks. Work in your sessions can continue."
        } else if v.show_finished {
            "No outcomes yet. Choose New to define what should be delivered and how it will be checked."
        } else {
            "No active outcomes. Choose New for another task, or Finished to inspect accepted work."
        };
        let p = ui.para(text, Style::new(size::BODY, weight::REGULAR, t.muted), body.w.min(540.0), Some(4));
        ui.draw(&p, Point::new(body.x, body.y + 16.0));
        return true;
    }
    let wide = body.w >= 700.0;
    let list_w = if wide { (body.w * 0.38).clamp(240.0, 380.0) } else { body.w };
    let list_h = if wide { body.h } else { (body.h * 0.38).clamp(64.0, 210.0).min(body.h) };
    let rows = (list_h / ROW_H).floor().max(1.0) as usize;
    v.list_top = v.list_top.min(v.selected);
    if v.selected >= v.list_top + rows {
        v.list_top = v.selected + 1 - rows;
    }
    draw_list(v, ui, Rect::new(body.x, body.y, list_w, list_h), rows, cx);
    let detail = if wide {
        ui.vline(body.x + list_w + 10.0, body.y, body.h, t.hairline);
        Rect::new(body.x + list_w + 26.0, body.y, (body.w - list_w - 26.0).max(1.0), body.h)
    } else {
        Rect::new(body.x, body.y + list_h + 18.0, body.w, (body.h - list_h - 18.0).max(0.0))
    };
    draw_detail(v, ui, detail);
    true
}

/// The task counts as coloured pills, only for states that have tasks. Returns the height used.
fn draw_counts(v: &CoordinationView, ui: &mut Ui, area: Rect, y: f32) -> f32 {
    let t = ui.t;
    let c = &v.tally;
    let items = [
        (c.running, "working", t.accent),
        (c.review, "review", t.warn),
        (c.blocked, "waiting", t.warn),
        (c.queued, "queued", t.muted),
        (c.failed, "failed", t.danger),
        (c.accepted, "accepted", t.good),
    ];
    let (mut x, mut row) = (area.x, 0.0);
    let mut any = false;
    for (n, label, color) in items {
        if n == 0 {
            continue;
        }
        any = true;
        let text = format!("{n} {label}");
        let w = widgets::pill_width(ui, &text);
        if x > area.x && x + w > area.right() {
            x = area.x;
            row += 26.0;
        }
        widgets::pill(ui, Point::new(x, y + row), 20.0, &text, color, color.with_alpha(0.14));
        x += w + 6.0;
    }
    if !any {
        ui.label(Point::new(area.x, y), "Nothing queued", Style::new(size::SMALL, weight::REGULAR, t.muted));
        return 18.0;
    }
    row + 20.0
}

/// An error or notice in a full-border strip; little when there's none. Returns the height used.
fn draw_status(v: &CoordinationView, ui: &mut Ui, at: Rect) -> f32 {
    let t = ui.t;
    let (text, color) = match (&v.error, &v.notice) {
        (Some(e), _) => (e.as_str(), t.danger),
        (None, Some(n)) => (n.as_str(), t.good),
        _ if v.snapshot.is_none() => ("Loading durable outcomes…", t.muted),
        _ => return 6.0,
    };
    let p = ui.para(text, Style::new(size::SMALL, weight::MEDIUM, color), at.w - 24.0, Some(3));
    let r = Rect::new(at.x, at.y, at.w, p.h + 16.0);
    ui.panel(r, radius::CONTROL, color.with_alpha(0.08), color.with_alpha(0.4));
    ui.draw(&p, Point::new(r.x + 12.0, r.y + 8.0));
    r.h + 14.0
}

fn draw_list(v: &CoordinationView, ui: &mut Ui, list: Rect, rows: usize, cx: &Cx) {
    let t = ui.t;
    ui.clip(list, 0.0);
    for (row, (index, task)) in v.tasks().enumerate().skip(v.list_top).take(rows).enumerate() {
        let r = Rect::new(list.x, list.y + row as f32 * ROW_H, list.w - 8.0, ROW_H - 4.0);
        widgets::row(ui, r, ("coordination-task", index), index == v.selected, Action::View(cx.id, ROW + index));
        let color = state_color(t, task.state);
        ui.circle(Point::new(r.x + 16.0, r.y + 18.0), 4.0, color);
        ui.label_in(Rect::new(r.x + 30.0, r.y + 7.0, r.w - 42.0, 22.0), &task.title, Style::new(size::BODY, weight::MEDIUM, t.text), Align::Left);
        let state = state_label(task.state);
        let st = Style::new(size::SMALL, weight::MEDIUM, color);
        let sw = ui.measure(state, st);
        ui.label_in(Rect::new(r.x + 30.0, r.y + 32.0, r.w - 42.0, 18.0), state, st, Align::Left);
        ui.label_in(
            Rect::new(r.x + 30.0 + sw + 8.0, r.y + 32.0, (r.w - 42.0 - sw - 8.0).max(0.0), 18.0),
            &format!("· {}", project_name(&task.project)),
            Style::new(size::SMALL, weight::REGULAR, t.faint),
            Align::Left,
        );
    }
    widgets::scrollbar(ui, list, v.row_indices.len() as f32 * ROW_H, list.h, v.list_top as f32 * ROW_H, 0.6);
    ui.unclip();
}

fn draw_detail(v: &mut CoordinationView, ui: &mut Ui, detail: Rect) {
    let t = ui.t;
    let key = (detail.w.to_bits(), ui.scale.to_bits());
    let first_is_title = !v.context_page;
    let styles = |i: usize, heading: bool| {
        if heading && i == 0 && first_is_title {
            Style::new(18.0, weight::SEMIBOLD, t.text)
        } else if heading {
            Style::new(size::HEADING, weight::SEMIBOLD, t.text)
        } else {
            Style::new(size::BODY, weight::REGULAR, t.muted)
        }
    };
    v.detail_cached();
    // measure each line once per change; scrolling and idle frames reuse the heights
    if v.detail_heights.as_ref().is_none_or(|(k, _)| *k != key) {
        let lines = v.detail_lines.take().unwrap_or_default();
        let heights = lines.iter().enumerate().map(|(i, (text, heading))| ui.para(text, styles(i, *heading), detail.w, None).h).collect();
        v.detail_lines = Some(lines);
        v.detail_heights = Some((key, heights));
    }
    let (Some(lines), Some((_, heights))) = (&v.detail_lines, &v.detail_heights) else {
        return;
    };
    ui.clip(detail, 0.0);
    let mut dy = detail.y - v.detail_scroll as f32 * 22.0;
    for (i, ((text, heading), h)) in lines.iter().zip(heights).enumerate() {
        if *heading && i > 0 {
            dy += 14.0;
        }
        if dy + h >= detail.y && dy <= detail.bottom() {
            let p = ui.para(text, styles(i, *heading), detail.w, None);
            ui.draw(&p, Point::new(detail.x, dy));
        }
        dy += h + if *heading { 6.0 } else { 10.0 };
        if dy > detail.bottom() {
            break;
        }
    }
    ui.unclip();
}

fn draw_form(v: &mut CoordinationView, ui: &mut Ui, area: Rect, cx: &mut Cx) {
    let Some(form) = &v.form else {
        return;
    };
    let t = ui.t;
    let existing = form.task.is_some();
    ui.clip(area, 0.0);
    ui.label(
        Point::new(area.x, area.y),
        if existing { "Steer this outcome" } else { "Define a new outcome" },
        Style::new(size::HEADING, weight::SEMIBOLD, t.text),
    );
    ui.label_in(
        Rect::new(area.x, area.y + 24.0, area.w, 22.0),
        "Tab fields · Ctrl+Enter saves · Esc back",
        Style::new(size::SMALL, weight::REGULAR, t.muted),
        Align::Left,
    );
    let content = Rect::new(area.x, area.y + 54.0, area.w, (area.h - 54.0).max(0.0));
    ui.clip(content, 0.0);
    let fw = area.w.min(760.0);
    let mut y = content.y - v.detail_scroll as f32 * 22.0;
    let mut bar = None;
    for (i, (label, placeholder)) in [
        ("Outcome", "What should be true when this is finished?"),
        ("Review check", "How can someone verify the outcome?"),
        ("Project", ""),
    ]
    .into_iter()
    .enumerate()
    {
        ui.label(Point::new(area.x, y), label, Style::new(size::SMALL, weight::MEDIUM, t.muted));
        let r = Rect::new(area.x, y + 20.0, fw, 36.0);
        if i == 2 {
            bar = Some(r);
            address_bar(
                ui,
                std::path::Path::new(form.project.trim()),
                form.project.trim().is_empty(),
                r,
                false,
                if existing { Action::None } else { Action::View(cx.id, 15) },
                None,
            );
            if form.focus == 2 && cx.focused && form.picker.is_none() && !existing {
                ui.outline(r, radius::CONTROL, 1.5, t.accent.with_alpha(0.8));
            }
            if existing {
                ui.label(
                    Point::new(area.x, r.bottom() + 6.0),
                    "A task keeps its project. Start a new task to work somewhere else.",
                    Style::new(size::SMALL, weight::REGULAR, t.faint),
                );
                y += 22.0;
            }
        } else {
            let value = if i == 0 { &form.goal } else { &form.acceptance };
            let on = form.focus == i && cx.focused && form.picker.is_none();
            widgets::field(ui, r, value, on.then_some(value.len()), placeholder, on, Action::View(cx.id, FIELD + i));
        }
        y += 74.0;
    }
    ui.label(Point::new(area.x, y), "Quality", Style::new(size::SMALL, weight::MEDIUM, t.muted));
    let chosen = match form.quality {
        QualityMode::Quick => 0,
        QualityMode::Standard => 1,
        QualityMode::Critical => 2,
    };
    let busy = v.busy;
    let id = cx.id;
    segmented(
        ui,
        Point::new(area.x, y + 20.0),
        34.0,
        &[Seg::new("Quick"), Seg::new("Standard"), Seg::new("Critical")],
        Some(chosen),
        size::BODY,
        "coordination-quality",
        |i| if busy { Action::None } else { Action::View(id, 21 + i) },
    );
    y += 64.0;
    let note = ui.para(
        "Quick uses focused checks. Standard verifies the changed behavior. Critical also requires independent review. Existing repository requirements still apply.",
        Style::new(size::SMALL, weight::REGULAR, t.muted),
        fw,
        Some(4),
    );
    ui.draw(&note, Point::new(area.x, y));
    y += note.h + 18.0;
    let save_label = if busy { "Saving…" } else { "Save outcome" };
    let save_key = (key_button_width(ui, None, save_label, Some("Ctrl+Enter")) <= area.w).then_some("Ctrl+Enter");
    let save_w = key_button_width(ui, None, save_label, save_key).ceil().min(area.w);
    key_button(ui, Rect::new(area.x, y, save_w, 32.0), None, save_label, save_key, widgets::Kind::Primary, (!busy).then_some(Action::View(id, 9)));
    let back_w = key_button_width(ui, None, "Back", Some("Esc")).ceil().min(area.w);
    let back_x = if save_w + 10.0 + back_w <= area.w {
        area.x + save_w + 10.0
    } else {
        y += 40.0;
        area.x
    };
    key_button(ui, Rect::new(back_x, y, back_w, 32.0), None, "Back", Some("Esc"), widgets::Kind::Ghost, (!busy).then_some(Action::View(id, 10)));
    if let Some(e) = &v.error {
        let p = ui.para(e, Style::new(size::SMALL, weight::MEDIUM, t.danger), fw - 24.0, Some(4));
        let r = Rect::new(area.x, y + 44.0, fw, p.h + 16.0);
        ui.panel(r, radius::CONTROL, t.danger.with_alpha(0.08), t.danger.with_alpha(0.4));
        ui.draw(&p, Point::new(r.x + 12.0, r.y + 8.0));
    }
    ui.unclip();
    ui.unclip();
    // the shared project chooser hangs from the project field over a dimmed page, as in New Session
    let focused = cx.focused;
    let current = v.form.as_ref().map(|f| std::path::PathBuf::from(f.project.trim())).unwrap_or_default();
    match (v.form.as_mut().and_then(|f| f.picker.as_mut()), bar) {
        (Some(p), Some(bar)) => {
            let prev = ui.layer();
            let k = picker::opening(ui);
            let screen = ui.screen;
            widgets::scrim(ui, screen, k, Action::None);
            address_bar(ui, &current, current.as_os_str().is_empty(), bar, true, Action::None, None);
            picker::draw(p, ui, bar, area, &current, &|n| Action::View(id, PICKER + n), focused, k);
            ui.set_layer(prev);
        }
        _ => picker::closed(ui),
    }
}

pub(crate) fn click(v: &mut CoordinationView, n: usize, _p: Point, _m: KeyModifiers, _count: u32, cx: &mut Cx) {
    if n >= PICKER {
        let k = n - PICKER;
        let Some(p) = v.form.as_mut().and_then(|f| f.picker.as_mut()) else {
            return;
        };
        let outcome = match k {
            picker::CLOSE => folder::Outcome::Close,
            k if k >= picker::ROW => match picker::click_row(p, k - picker::ROW) {
                Some(folder::Outcome::None) if p.error.is_none() => p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                Some(o) => o,
                None => folder::Outcome::None,
            },
            _ => folder::Outcome::None,
        };
        v.picker_outcome(outcome);
    } else if n >= FIELD {
        if let Some(f) = &mut v.form {
            f.focus = (n - FIELD).min(2);
        }
    } else {
        v.activate(n, cx);
    }
}

pub(crate) fn wheel(v: &mut CoordinationView, n: Option<usize>, down: bool, _cx: &mut Cx) {
    if let Some(p) = v.form.as_mut().and_then(|f| f.picker.as_mut()) {
        let code = if down { KeyCode::Down } else { KeyCode::Up };
        for _ in 0..3 {
            p.key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    } else if n.is_some_and(|n| (ROW..FIELD).contains(&n)) {
        v.select(if down { v.selected + 1 } else { v.selected.saturating_sub(1) });
    } else {
        v.detail_scroll = if down { v.detail_scroll + 3 } else { v.detail_scroll.saturating_sub(3) };
    }
}
