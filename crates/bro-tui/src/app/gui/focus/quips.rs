//! A small colon picker in either session view, and a plain library/editor in Settings.

use super::{Gui, View, advance};
use crate::app::{App, Overlay};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Ui, size, weight, widgets};
use crate::layout::PaneId;
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};

pub(super) const BASE: usize = 600;
const CLOSE: usize = 601;
const NEW: usize = 602;
const EDIT: usize = 603;
const DELETE: usize = 604;
const INSERT: usize = 605;
const SAVE: usize = 607;
const BACK: usize = 608;
const FIELD: usize = 610;
const ROW: usize = 700;
const REMOVE: usize = 2000;
const TRUNCATE: usize = 3000;
const ROW_H: f32 = 36.0;

thread_local! {
    static MANAGER: std::cell::RefCell<View> = std::cell::RefCell::new(View::default());
}

#[derive(Clone, Copy)]
pub(super) struct FieldGeo {
    rect: Rect,
    cols: usize,
    first: usize,
    adv: f32,
    row_h: f32,
}

fn target(id: PaneId, n: usize, manager: bool) -> Action {
    if manager { Action::Overlay(n) } else { Action::Focus(id, n) }
}

fn label(ui: &mut Ui, text: &str, rect: Rect, style: Style) {
    ui.label_in(rect, text, style, Align::Left);
}

pub(super) fn draw_overlay(app: &mut App, ui: &mut Ui, id: PaneId, window: Rect) {
    let w = 580.0_f32.min(window.w - 32.0);
    let h = 540.0_f32.min(window.h - 40.0);
    let r = Rect::new(window.x + (window.w - w) / 2.0, window.y + (window.h - h) / 2.0, w, h);
    ui.fill(window, bro_gfx::Color::BLACK.with_alpha(0.22));
    let rows = app.focus_quip_rows(id);
    MANAGER.with(|v| manager(app, ui, id, r, &rows, &mut v.borrow_mut()));
}

pub(super) fn draw(app: &mut App, ui: &mut Ui, id: PaneId, over: Rect, rows: &[usize], _view: &mut View) {
    let clip = ui.scene.clip();
    let lifted = clip.rect != Rect::EVERYTHING;
    if lifted { ui.unclip(); }
    let previous = ui.layer();
    picker(app, ui, id, over, rows);
    ui.set_layer(previous);
    if lifted { ui.scene.push_clip(clip.rect, clip.radius); }
}

fn picker(app: &mut App, ui: &mut Ui, id: PaneId, over: Rect, rows: &[usize]) {
    let t = ui.t;
    let native = app.terminal_quips_enabled(id);
    let d = app.focus.draft(id);
    let selected = if native { d.terminal_quips.selected } else { d.quips.selected }.min(rows.len().saturating_sub(1));
    let query = if native { d.terminal_quips.query.clone().unwrap_or_default() } else { crate::quips::query(d.area.text(), d.area.cursor()).map_or(String::new(), |(_, q)| q) };
    let shown = rows.len().clamp(1, 6);
    let first = selected.saturating_sub(shown - 1).min(rows.len().saturating_sub(shown));
    let w = 420.0_f32.min(ui.screen.w - 32.0);
    let h = 30.0 + shown as f32 * ROW_H + 56.0;
    let x = over.x.min(ui.screen.right() - w - 12.0).max(ui.screen.x + 12.0);
    let above = over.y - h - 8.0;
    let y = if above >= ui.screen.y + 12.0 { above } else if over.bottom() + h + 8.0 < ui.screen.bottom() - 12.0 { over.bottom() + 8.0 } else { ui.screen.y + 12.0 };
    let r = Rect::new(x, y, w, h);
    widgets::card(ui, r, 10.0);
    ui.block(r);
    label(ui, "Quips", Rect::new(x + 12.0, y + 5.0, 100.0, 20.0), Style::new(size::SMALL, weight::MEDIUM, t.muted));
    ui.label_in(Rect::new(x + 120.0, y + 5.0, w - 132.0, 20.0), &format!(":{query}"), Style::mono(size::SMALL, t.faint), Align::Right);
    if rows.is_empty() { label(ui, "No matching Quips", Rect::new(x + 12.0, y + 30.0, w - 24.0, ROW_H), Style::new(size::SMALL, weight::REGULAR, t.muted)); }
    for (n, &index) in rows.iter().enumerate().skip(first).take(shown) {
        let q = &app.focus.quips.items[index];
        let rr = Rect::new(x + 5.0, y + 30.0 + (n - first) as f32 * ROW_H, w - 10.0, ROW_H - 2.0);
        widgets::row(ui, rr, ("quip-pick", id, n), n == selected, Action::Focus(id, ROW + n));
        label(ui, &q.name, Rect::new(rr.x + 9.0, rr.y, rr.w * 0.5, rr.h), Style::new(size::BODY, weight::MEDIUM, t.text));
        ui.label_in(Rect::new(rr.x + rr.w * 0.5, rr.y, rr.w * 0.5 - 9.0, rr.h), &q.token(), Style::mono(size::CAPTION, t.accent), Align::Right);
    }
    if let Some(q) = rows.get(selected).and_then(|i| app.focus.quips.items.get(*i)) {
        label(ui, crate::quips::summary(&q.trigger), Rect::new(x + 12.0, r.bottom() - 52.0, w - 24.0, 24.0), Style::new(size::CAPTION, weight::REGULAR, t.muted));
    }
    label(ui, "↑ ↓ choose · tab insert · esc close", Rect::new(x + 12.0, r.bottom() - 26.0, w - 24.0, 20.0), Style::new(size::CAPTION, weight::REGULAR, t.faint));
}

fn manager(app: &mut App, ui: &mut Ui, id: PaneId, r: Rect, rows: &[usize], view: &mut View) {
    let t = ui.t;
    widgets::card(ui, r, 12.0);
    ui.hit_with(ui.screen, Action::Overlay(CLOSE), Cursor::Default);
    ui.block(r);
    ui.clip(r, 12.0);
    let editing = app.focus.draft(id).quips.editor.is_some();
    let title = app.focus.draft(id).quips.editor.as_ref().map_or("Quips", |e| if e.original.is_some() { "Edit Quip" } else { "New Quip" });
    label(ui, title, Rect::new(r.x + 20.0, r.y + 14.0, r.w - 150.0, 30.0), Style::new(22.0, weight::SEMIBOLD, t.text));
    let close = Rect::new(r.right() - 44.0, r.y + 16.0, 26.0, 26.0);
    ui.icon(close, "\u{F0156}", 16.0, t.muted);
    ui.hit(close, Action::Overlay(CLOSE));
    view.quip_fields = [None; 3];
    if editing {
        editor(app, ui, id, r, view);
        ui.unclip();
        return;
    }
    widgets::button(ui, Rect::new(r.right() - 134.0, r.y + 16.0, 76.0, 28.0), "+ New", widgets::Kind::Secondary, Action::Overlay(NEW));
    label(ui, "Type : in any agent session to add instructions.", Rect::new(r.x + 20.0, r.y + 49.0, r.w - 40.0, 22.0), Style::new(size::SMALL, weight::REGULAR, t.muted));
    let p = &app.focus.draft(id).quips;
    let query = p.search.clone();
    let selected = p.selected.min(rows.len().saturating_sub(1));
    let armed = p.delete_armed;
    let error = p.error.clone().or_else(|| app.focus.quips.error.clone());
    let search = Rect::new(r.x + 20.0, r.y + 82.0, r.w - 40.0, 32.0);
    ui.panel(search, 6.0, t.field_focus, t.stroke);
    label(ui, if query.is_empty() { "Search Quips…" } else { &query }, Rect::new(search.x + 10.0, search.y, search.w - 20.0, search.h), Style::new(size::SMALL, weight::REGULAR, if query.is_empty() { t.faint } else { t.text }));
    let list = Rect::new(r.x + 12.0, r.y + 126.0, r.w - 24.0, (r.h - 242.0).max(36.0));
    let shown = (list.h / ROW_H).floor().max(1.0) as usize;
    let first = selected.saturating_sub(shown - 1).min(rows.len().saturating_sub(shown));
    ui.clip(list, 0.0);
    if rows.is_empty() { label(ui, "No matching Quips", list, Style::new(size::BODY, weight::REGULAR, t.muted)); }
    for (n, &index) in rows.iter().enumerate().skip(first).take(shown) {
        let q = &app.focus.quips.items[index];
        let rr = Rect::new(list.x, list.y + (n - first) as f32 * ROW_H, list.w - 4.0, ROW_H - 2.0);
        widgets::row(ui, rr, ("quip-library", id, n), n == selected, Action::Overlay(ROW + n));
        label(ui, &q.name, Rect::new(rr.x + 12.0, rr.y, rr.w * 0.5, rr.h), Style::new(size::BODY, weight::MEDIUM, t.text));
        ui.label_in(Rect::new(rr.x + rr.w * 0.5, rr.y, rr.w * 0.5 - 12.0, rr.h), &q.token(), Style::mono(size::SMALL, t.accent), Align::Right);
    }
    ui.unclip();
    if rows.len() > shown { widgets::scrollbar(ui, list, rows.len() as f32 * ROW_H, list.h, first as f32 * ROW_H, 0.8); }
    let preview = error.as_deref().or_else(|| rows.get(selected).and_then(|i| app.focus.quips.items.get(*i)).map(|q| q.instructions.as_str())).unwrap_or("Create a Quip with your own instructions.");
    let paragraph = ui.para(preview, Style::new(size::SMALL, weight::REGULAR, if error.is_some() { t.danger } else { t.muted }), r.w - 40.0, Some(2));
    ui.draw(&paragraph, Point::new(r.x + 20.0, r.bottom() - 101.0));
    let by = r.bottom() - 45.0;
    if !rows.is_empty() {
        widgets::button(ui, Rect::new(r.x + 20.0, by, 80.0, 30.0), if armed { "Delete?" } else { "Delete" }, if armed { widgets::Kind::Danger } else { widgets::Kind::Secondary }, Action::Overlay(DELETE));
        widgets::button(ui, Rect::new(r.right() - 110.0, by, 90.0, 30.0), "Edit", widgets::Kind::Primary, Action::Overlay(EDIT));
    }
    label(ui, "enter edit · esc close", Rect::new(r.x + 114.0, by, r.w - 238.0, 30.0), Style::new(size::CAPTION, weight::REGULAR, t.faint));
    ui.unclip();
}

fn editor(app: &mut App, ui: &mut Ui, id: PaneId, r: Rect, v: &mut View) {
    let t = ui.t;
    let manager = matches!(app.overlay, crate::app::Overlay::Quips(_));
    let p = &mut app.focus.draft(id).quips;
    let Some(e) = &mut p.editor else { return };
    let x = r.x + 20.0;
    let w = r.w - 40.0;
    let compact = false;
    let split = (w - 16.0) / 2.0;
    let first_y = r.y + 88.0;
    let instructions_y = first_y + 72.0;
    let fields = [
        Rect::new(x, first_y, split, 36.0),
        Rect::new(x + split + 16.0, first_y, split, 36.0),
        Rect::new(x, instructions_y, w, (r.bottom() - 81.0 - instructions_y).max(36.0)),
    ];
    for (i, name) in ["Name", "Shortcut", "Instructions"].iter().enumerate() {
        let rr = fields[i];
        if !compact || i == 2 { label(ui, name, Rect::new(rr.x, rr.y - 24.0, rr.w, 18.0), Style::new(size::SMALL, weight::MEDIUM, t.muted)); }
        let focused = i == e.field;
        ui.panel(rr, 7.0, if focused { t.field_focus } else { t.control }, if focused { t.accent.with_alpha(0.65) } else { t.stroke });
        let inner = Rect::new(rr.x + 12.0, rr.y + 8.0, rr.w - 24.0, rr.h - 16.0);
        let adv = advance(ui, size::BODY);
        let cols = (inner.w / adv).floor().max(4.0) as usize;
        if focused { e.cols = cols; }
        let area = &e.fields[i];
        let rows = area.rows(cols);
        let crow = rows.iter().rposition(|&(s, end)| area.cursor() >= s && area.cursor() <= end).unwrap_or(0);
        let shown = (inner.h / 21.0).floor().max(1.0) as usize;
        let first = crow.saturating_sub(shown - 1);
        v.quip_fields[i] = Some(FieldGeo { rect: inner, cols, first, adv, row_h: 21.0 });
        ui.clip(inner, 0.0);
        if area.text().is_empty() {
            label(ui, ["My Quip", "my-quip", "Tell the AI what to do when you use this Quip…"][i], Rect::new(inner.x, inner.y, inner.w, 20.0), Style::mono(size::BODY, t.faint));
        }
        for (n, &(start, end)) in rows.iter().enumerate().skip(first).take(shown) {
            let b = ui.line(&area.text()[start..end], Style::mono(size::BODY, t.text), None);
            let at = Point::new(inner.x, inner.y + (n - first) as f32 * 21.0);
            ui.draw(&b, at);
            if focused && n == crow {
                let cx = b.layout.caret_x(area.cursor() - start) / ui.scale;
                ui.top_fill(Rect::new(at.x + cx, at.y + 1.0, 1.5, 17.0), 0.5, t.accent);
                v.caret = Some(Rect::new(at.x + cx, at.y, 2.0, 21.0));
            }
        }
        ui.unclip();
        ui.hit_with(rr, target(id, FIELD + i, manager), Cursor::Text);
    }
    if let Some(error) = &p.error {
        label(ui, error, Rect::new(x, r.bottom() - 71.0, w, 20.0), Style::new(size::CAPTION, weight::REGULAR, t.danger));
    } else {
        label(ui, "Saved on this computer · tab switches fields · ctrl+s saves", Rect::new(x, r.bottom() - 71.0, w, 20.0), Style::new(size::CAPTION, weight::REGULAR, t.faint));
    }
    widgets::button(ui, Rect::new(r.right() - 130.0, r.bottom() - 47.0, 110.0, 31.0), "Save", widgets::Kind::Primary, target(id, SAVE, manager));
    widgets::button(ui, Rect::new(r.right() - 216.0, r.bottom() - 47.0, 78.0, 31.0), "Cancel", widgets::Kind::Secondary, target(id, BACK, manager));
}


pub(super) fn draw_terminal(app: &mut App, ui: &mut Ui, id: PaneId, content: Rect, focused: bool, input: Option<Rect>, view: &mut View) {
    if !app.terminal_quips_enabled(id) { return; }
    let t = ui.t;
    let Some(draft) = app.focus.drafts.get(&id) else { return };
    let pending = draft.terminal_quips.pending.clone();
    let query = draft.terminal_quips.query.is_some();
    let clip = ui.scene.clip();
    let lifted = clip.rect != Rect::EVERYTHING;
    if lifted { ui.unclip(); }
    let mut x = content.x + 12.0;
    let mut shown = 0;
    for (index, quip) in pending.iter().enumerate() {
        let text = format!("{}  ×", quip.token());
        let w = ui.measure(&text, Style::mono(size::SMALL, t.accent)) + 18.0;
        let more = if index + 1 < pending.len() { 54.0 } else { 0.0 };
        if x + w + more > content.right() - 12.0 { break; }
        let r = Rect::new(x, content.bottom() - 29.0, w, 23.0);
        ui.round(r, 6.0, t.accent.with_alpha(0.12));
        label(ui, &text, Rect::new(r.x + 9.0, r.y, r.w - 18.0, r.h), Style::mono(size::SMALL, t.accent));
        ui.hit(r, Action::Focus(id, REMOVE + index));
        if ui.hovered(r) { widgets::tooltip(ui, r, &format!("{}\nClick to remove from your next message.", quip.instructions)); }
        x += w + 6.0;
        shown += 1;
    }
    if shown < pending.len() {
        let text = format!("+{} ×", pending.len() - shown);
        let r = Rect::new(x, content.bottom() - 29.0, 48.0, 23.0);
        ui.round(r, 6.0, t.accent.with_alpha(0.12));
        label(ui, &text, Rect::new(r.x + 6.0, r.y, r.w - 12.0, r.h), Style::mono(size::SMALL, t.accent));
        ui.hit(r, Action::Focus(id, TRUNCATE + shown));
        if ui.hovered(r) {
            let names = pending[shown..].iter().map(|q| q.name.as_str()).collect::<Vec<_>>().join(", ");
            widgets::tooltip(ui, r, &format!("Also attached: {names}. Click to remove these Quips."));
        }
    }
    if focused && !app.overlay.is_open() && query {
        let rows = app.terminal_quip_rows(id);
        let anchor = input.unwrap_or_else(|| Rect::new(content.x + 12.0, content.bottom() - if pending.is_empty() { 8.0 } else { 36.0 }, content.w - 24.0, 0.0));
        draw(app, ui, id, anchor, &rows, view);
    }
    if lifted { ui.scene.push_clip(clip.rect, clip.radius); }
}

pub(super) fn click(g: &mut Gui, id: PaneId, n: usize, point: Point, count: u32) -> bool {
    if !(BASE..super::OPEN).contains(&n) { return false; }
    match n {
        CLOSE => g.app.close_quips(id),
        NEW => g.app.focus_edit_quip(id, None),
        SAVE => g.app.focus_save_quip(id),
        BACK => { let p = &mut g.app.focus.draft(id).quips; p.editor = None; p.error = None; }
        EDIT | INSERT => {
            let rows = g.app.focus_quip_rows(id);
            let sel = g.app.focus.draft(id).quips.selected.min(rows.len().saturating_sub(1));
            if let Some(index) = rows.get(sel) {
                if n == EDIT { g.app.focus_edit_quip(id, Some(*index)); } else { g.app.focus_insert_quip(id, *index); }
            }
        }
        DELETE => g.app.focus_delete_quip(id),
        n if (FIELD..FIELD + 3).contains(&n) => {
            let i = n - FIELD;
            let geo = if matches!(g.app.overlay, crate::app::Overlay::Quips(_)) { MANAGER.with(|v| v.borrow().quip_fields[i]) } else { g.panes.get(&id).and_then(|st| st.focus.quip_fields[i]) };
            if let Some(e) = &mut g.app.focus.draft(id).quips.editor {
                e.field = i;
                if let Some(geo) = geo {
                    let rows = e.fields[i].rows(geo.cols);
                    let row = (geo.first + ((point.y - geo.rect.y).max(0.0) / geo.row_h) as usize).min(rows.len().saturating_sub(1));
                    let (start, end) = rows[row];
                    let col = ((point.x - geo.rect.x).max(0.0) / geo.adv).round() as usize;
                    let text = &e.fields[i].text()[start..end];
                    let mut used = 0;
                    let at = text.char_indices().find_map(|(pos, c)| { used += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0); (used > col).then_some(start + pos) }).unwrap_or(end);
                    e.fields[i].set_cursor(at);
                }
            }
        }
        n if n >= TRUNCATE => g.app.focus.draft(id).terminal_quips.pending.truncate(n - TRUNCATE),
        n if n >= REMOVE => g.app.terminal_remove_quip(id, n - REMOVE),
        n if n >= ROW => {
            g.app.focus.draft(id).quips.selected = n - ROW;
            g.app.focus.draft(id).quips.delete_armed = false;
            if count >= 2 || !matches!(g.app.overlay, Overlay::Quips(_)) {
                if let Some(index) = g.app.focus_quip_rows(id).get(n - ROW).copied() { g.app.focus_insert_quip(id, index); }
            }
        }
        _ => {}
    }
    true
}

pub(super) fn wheel(g: &mut Gui, id: PaneId, dy: f32) -> bool {
    if g.app.terminal_quips_enabled(id) && !matches!(g.app.overlay, Overlay::Quips(_)) {
        let rows = g.app.terminal_quip_rows(id);
        let state = &mut g.app.focus.draft(id).terminal_quips;
        if state.query.is_none() { return false; }
        state.selected = (state.selected as isize - dy.round() as isize).clamp(0, rows.len().saturating_sub(1) as isize) as usize;
        return true;
    }
    let rows = g.app.focus_quip_rows(id);
    let p = &mut g.app.focus.draft(id).quips;
    if p.editor.is_some() { return true; }
    if !p.open && rows.is_empty() { return false; }
    p.selected = (p.selected as isize - dy.round() as isize).clamp(0, rows.len().saturating_sub(1) as isize) as usize;
    p.delete_armed = false;
    true
}
