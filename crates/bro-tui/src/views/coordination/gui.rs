//! The Tasks pane uses Bro's service-view controls and typography. It stays in the workspace so other
//! sessions keep running while an outcome is inspected or steered.

use super::{CoordinationView, state_label};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Ui, size, weight, widgets};
use crate::pane::Cx;
use crate::views::profiles::gui::{key_button, key_button_width};
use bro_core::coordination::TaskState;
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use crossterm::event::KeyModifiers;

const ROW: usize = 100;
const FIELD: usize = 1000;

pub(crate) fn draw(v: &mut CoordinationView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    v.prepare(cx);
    let t = ui.t;
    let area = area.inset(14.0);
    ui.hit_with(area, Action::View(cx.id, 0), Cursor::Default);
    if area.w < 220.0 || area.h < 140.0 {
        ui.label_in(
            area,
            "Enlarge this pane to inspect tasks",
            Style::new(size::BODY, weight::MEDIUM, t.muted),
            Align::Center,
        );
        return true;
    }
    ui.label(
        Point::new(area.x, area.y),
        if v.demo {
            "Tasks · demo examples"
        } else {
            "Tasks"
        },
        Style::new(20.0, weight::SEMIBOLD, t.text),
    );
    let summary = ui.para(
        &v.counts().full(),
        Style::new(size::SMALL, weight::REGULAR, t.muted),
        area.w,
        Some(2),
    );
    ui.draw(&summary, Point::new(area.x, area.y + 30.0));
    let mut y = area.y + 36.0 + summary.h;
    if v.form.is_some() {
        draw_form(
            v,
            ui,
            Rect::new(
                area.x,
                y + 10.0,
                area.w,
                (area.bottom() - y - 10.0).max(0.0),
            ),
            cx,
        );
        return true;
    }
    // Buttons wrap as a group; actions stay available in narrow split panes.
    let buttons = [
        ("New", "n", 3),
        ("Steer", "s", 4),
        ("Cancel", "x", 5),
        if v.current()
            .is_some_and(|t| matches!(t.state, TaskState::Submitted | TaskState::Verifying))
        {
            ("Review", "v", 12)
        } else if v
            .current()
            .is_some_and(|t| t.state == TaskState::Verified && t.attempt_id.is_some())
        {
            ("Retire worker", "z", 13)
        } else {
            ("Retry", "r", 6)
        },
        (if v.context_page { "Outcome" } else { "Context" }, "c", 7),
        (
            if v.show_finished {
                "Active"
            } else {
                "Finished"
            },
            "a",
            2,
        ),
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
        let enabled = v.action_enabled(action);
        key_button(
            ui,
            Rect::new(x, y, w.min(area.w), 32.0),
            None,
            label,
            Some(key),
            if action == 3 {
                widgets::Kind::Primary
            } else {
                widgets::Kind::Ghost
            },
            enabled.then_some(Action::View(cx.id, action)),
        );
        x += w + 4.0;
    }
    y += 42.0;
    let status = v
        .error
        .as_deref()
        .or(v.notice.as_deref())
        .unwrap_or(if v.snapshot.is_none() {
            "Loading durable outcomes…"
        } else {
            "Worker activity and accepted outcomes are tracked separately."
        });
    let message = ui.para(
        status,
        Style::new(
            size::SMALL,
            weight::REGULAR,
            if v.error.is_some() { t.danger } else { t.muted },
        ),
        area.w,
        Some(3),
    );
    ui.draw(&message, Point::new(area.x, y));
    y += message.h + 14.0;
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
        let p = ui.para(
            text,
            Style::new(size::BODY, weight::REGULAR, t.muted),
            body.w.min(540.0),
            Some(4),
        );
        ui.draw(&p, Point::new(body.x, body.y + 16.0));
        return true;
    }
    let wide = body.w >= 700.0;
    let list_w = if wide {
        (body.w * 0.38).clamp(240.0, 380.0)
    } else {
        body.w
    };
    let list_h = if wide {
        body.h
    } else {
        (body.h * 0.38).clamp(64.0, 210.0).min(body.h)
    };
    let rows = (list_h / 58.0).floor().max(1.0) as usize;
    v.list_top = v.list_top.min(v.selected);
    if v.selected >= v.list_top + rows {
        v.list_top = v.selected + 1 - rows;
    }
    let entries: Vec<_> = v
        .tasks()
        .enumerate()
        .skip(v.list_top)
        .take(rows)
        .map(|(i, t)| (i, t.title.clone(), t.project.clone(), t.state))
        .collect();
    ui.clip(Rect::new(body.x, body.y, list_w, list_h), 0.0);
    for (row, (index, title, project, state)) in entries.into_iter().enumerate() {
        let r = Rect::new(body.x, body.y + row as f32 * 58.0, list_w, 54.0);
        widgets::row(
            ui,
            r,
            ("coordination-task", index),
            index == v.selected,
            Action::View(cx.id, ROW + index),
        );
        ui.label_in(
            Rect::new(r.x + 10.0, r.y + 5.0, r.w - 20.0, 22.0),
            &title,
            Style::new(size::BODY, weight::MEDIUM, t.text),
            Align::Left,
        );
        let color = match state {
            TaskState::Verified => t.good,
            TaskState::Failed | TaskState::Uncertain => t.danger,
            TaskState::Blocked | TaskState::Submitted => t.warn,
            _ => t.muted,
        };
        ui.label_in(
            Rect::new(r.x + 10.0, r.y + 29.0, r.w - 20.0, 18.0),
            &format!("{} · {project}", state_label(state)),
            Style::new(size::SMALL, weight::REGULAR, color),
            Align::Left,
        );
    }
    ui.unclip();
    let detail = if wide {
        ui.vline(body.x + list_w + 10.0, body.y, body.h, t.hairline);
        Rect::new(
            body.x + list_w + 26.0,
            body.y,
            (body.w - list_w - 26.0).max(1.0),
            body.h,
        )
    } else {
        Rect::new(
            body.x,
            body.y + list_h + 18.0,
            body.w,
            (body.h - list_h - 18.0).max(0.0),
        )
    };
    ui.clip(detail, 0.0);
    let mut dy = detail.y - v.detail_scroll as f32 * 22.0;
    for (text, heading) in v.detail() {
        if heading {
            dy += 8.0;
        }
        let p = ui.para(
            &text,
            Style::new(
                if heading { size::HEADING } else { size::BODY },
                if heading {
                    weight::SEMIBOLD
                } else {
                    weight::REGULAR
                },
                if heading { t.text } else { t.muted },
            ),
            detail.w,
            None,
        );
        if dy + p.h >= detail.y && dy <= detail.bottom() {
            ui.draw(&p, Point::new(detail.x, dy));
        }
        dy += p.h + if heading { 8.0 } else { 10.0 };
    }
    ui.unclip();
    true
}

fn draw_form(v: &mut CoordinationView, ui: &mut Ui, area: Rect, cx: &mut Cx) {
    let Some(form) = &v.form else {
        return;
    };
    let t = ui.t;
    ui.clip(area, 0.0);
    ui.label(
        Point::new(area.x, area.y),
        if form.task.is_some() {
            "Steer this outcome"
        } else {
            "Define a new outcome"
        },
        Style::new(size::HEADING, weight::SEMIBOLD, t.text),
    );
    ui.label_in(
        Rect::new(area.x, area.y + 24.0, area.w, 22.0),
        "Tab fields · Ctrl+Enter saves · Esc back · PgUp/PgDn scroll",
        Style::new(size::SMALL, weight::REGULAR, t.muted),
        Align::Left,
    );
    let content = Rect::new(area.x, area.y + 50.0, area.w, (area.h - 50.0).max(0.0));
    ui.clip(content, 0.0);
    let mut y = content.y - v.detail_scroll as f32 * 22.0;
    for (i, (label, value, placeholder)) in [
        (
            "Outcome",
            &form.goal,
            "What should be true when this is finished?",
        ),
        (
            "Review check",
            &form.acceptance,
            "How can someone verify the outcome?",
        ),
        (
            "Project folder",
            &form.project,
            "Absolute path to the project",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        ui.label(
            Point::new(area.x, y),
            label,
            Style::new(size::SMALL, weight::MEDIUM, t.muted),
        );
        widgets::field(
            ui,
            Rect::new(area.x, y + 20.0, area.w.min(760.0), 34.0),
            value,
            (form.focus == i && cx.focused).then_some(value.len()),
            placeholder,
            form.focus == i && cx.focused,
            Action::View(cx.id, FIELD + i),
        );
        y += 70.0;
    }
    let label = format!(
        "Quality: {}",
        match form.quality {
            bro_core::coordination::QualityMode::Quick => "Quick",
            bro_core::coordination::QualityMode::Standard => "Standard",
            bro_core::coordination::QualityMode::Critical => "Critical",
        }
    );
    let quality_key =
        (key_button_width(ui, None, &label, Some("Ctrl+q")) <= area.w).then_some("Ctrl+q");
    let quality_w = key_button_width(ui, None, &label, quality_key)
        .ceil()
        .min(area.w);
    key_button(
        ui,
        Rect::new(area.x, y, quality_w, 32.0),
        None,
        &label,
        quality_key,
        widgets::Kind::Secondary,
        (!v.busy).then_some(Action::View(cx.id, 11)),
    );
    y += 42.0;
    let note=ui.para("Quick uses focused checks. Standard verifies the changed behavior. Critical also requires independent review. Existing repository requirements still apply.",Style::new(size::SMALL,weight::REGULAR,t.muted),area.w.min(760.0),Some(4));
    ui.draw(&note, Point::new(area.x, y));
    y += note.h + 18.0;
    let save_label = if v.busy { "Saving…" } else { "Save outcome" };
    let save_key = (key_button_width(ui, None, save_label, Some("Ctrl+Enter")) <= area.w)
        .then_some("Ctrl+Enter");
    let save_w = key_button_width(ui, None, save_label, save_key)
        .ceil()
        .min(area.w);
    key_button(
        ui,
        Rect::new(area.x, y, save_w, 32.0),
        None,
        save_label,
        save_key,
        widgets::Kind::Primary,
        (!v.busy).then_some(Action::View(cx.id, 9)),
    );
    let back_w = key_button_width(ui, None, "Back", Some("Esc"))
        .ceil()
        .min(area.w);
    let back_x = if save_w + 10.0 + back_w <= area.w {
        area.x + save_w + 10.0
    } else {
        y += 40.0;
        area.x
    };
    key_button(
        ui,
        Rect::new(back_x, y, back_w, 32.0),
        None,
        "Back",
        Some("Esc"),
        widgets::Kind::Ghost,
        (!v.busy).then_some(Action::View(cx.id, 10)),
    );
    if let Some(e) = &v.error {
        let p = ui.para(
            e,
            Style::new(size::SMALL, weight::REGULAR, t.danger),
            area.w,
            Some(4),
        );
        ui.draw(&p, Point::new(area.x, y + 44.0));
    }
    ui.unclip();
    ui.unclip();
}

pub(crate) fn click(
    v: &mut CoordinationView,
    n: usize,
    _p: Point,
    _m: KeyModifiers,
    _count: u32,
    cx: &mut Cx,
) {
    if n >= FIELD {
        if let Some(f) = &mut v.form {
            f.focus = (n - FIELD).min(2);
        }
    } else {
        v.activate(n, cx);
    }
}
pub(crate) fn wheel(v: &mut CoordinationView, n: Option<usize>, down: bool, _cx: &mut Cx) {
    if n.is_some_and(|n| (ROW..FIELD).contains(&n)) {
        v.select(if down {
            v.selected + 1
        } else {
            v.selected.saturating_sub(1)
        });
    } else {
        v.detail_scroll = if down {
            v.detail_scroll + 3
        } else {
            v.detail_scroll.saturating_sub(3)
        };
    }
}
