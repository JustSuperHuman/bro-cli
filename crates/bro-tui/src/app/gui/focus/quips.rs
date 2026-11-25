//! A small colon picker in either session view, and a plain library/editor in Settings.

use super::{Gui, View, advance, range_rects};
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
/// "New…" at the foot of the colon picker.
const CREATE: usize = 606;
const SAVE: usize = 607;
const BACK: usize = 608;
const PREVIEW: usize = 609;
const FIELD: usize = 610;
const ROW: usize = 700;
const REMOVE: usize = 2000;
const TRUNCATE: usize = 3000;
const ROW_H: f32 = 36.0;

/// The definition belongs to this message, rather than the current library entry.
#[derive(Clone)]
pub(super) struct Preview {
    anchor: Rect,
    token: String,
    instructions: String,
}

#[derive(Default)]
pub(super) struct PreviewState {
    pub hovered: Option<Preview>,
    shown: Option<Preview>,
    rect: Option<Rect>,
    scroll: f32,
    max_scroll: f32,
}

/// Strong enough to read in pending messages and in both light and dark themes.
pub(super) fn badge_ink(ui: &Ui) -> bro_gfx::Color {
    ui.t.shine.mix(ui.t.text, if ui.t.light { 0.45 } else { 0.12 })
}

/// Paint a token without changing its text, caret positions, wrapping, or copied content.
pub(super) fn badge(ui: &mut Ui, text: &super::TextBox, at: Point, range: &std::ops::Range<usize>, token: &str, instructions: &str, size: f32) -> Option<Preview> {
    let mut hovered = None;
    for glyphs in range_rects(text, at, range, ui.scale) {
        let height = (size * 1.35).round().min(glyphs.h + 2.0);
        let rect = Rect::new(glyphs.x - 3.0, glyphs.y + (glyphs.h - height) / 2.0, glyphs.w + 6.0, height);
        let hot = ui.hovered(rect);
        let tint = ui.t.shine;
        ui.panel(rect, 6.0, tint.with_alpha(if ui.t.light { if hot { 0.14 } else { 0.09 } } else if hot { 0.23 } else { 0.16 }), tint.with_alpha(if hot { 0.48 } else { 0.26 }));
        // Preserve full-contrast badge text even when the pending bubble's body is faded.
        ui.clip(glyphs, 0.0);
        ui.draw_color(text, at, badge_ink(ui));
        ui.unclip();
        if hot {
            hovered = Some(Preview { anchor: rect, token: token.into(), instructions: instructions.into() });
        }
    }
    hovered
}

impl PreviewState {
    pub(super) fn wheel(&mut self, pointer: Option<Point>, pixels: f32) -> bool {
        if !self.rect.is_some_and(|r| pointer.is_some_and(|p| r.contains(p))) { return false; }
        self.scroll = (self.scroll - pixels).clamp(0.0, self.max_scroll);
        true
    }

    /// Draw after the conversation and composer have released their text clips.
    pub(super) fn draw(&mut self, ui: &mut Ui, id: PaneId) -> bool {
        if let Some(next) = self.hovered.take() {
            if self.shown.as_ref().is_none_or(|old| old.token != next.token || old.instructions != next.instructions) { self.scroll = 0.0; }
            self.shown = Some(next);
        } else if !self.rect.is_some_and(|r| ui.hovered(r.outset(10.0))) {
            self.shown = None;
        }
        self.rect = None;
        let Some(preview) = self.shown.as_ref() else { return false };
        let bounds = ui.screen.intersect(&ui.scene.clip().rect.scale(1.0 / ui.scale));
        if bounds.w < 100.0 || bounds.h < 100.0 { return false; }
        let width = 440.0_f32.min(bounds.w - 16.0);
        let header = ui.line(&preview.token, Style::mono(size::BODY, badge_ink(ui)).weight(weight::SEMIBOLD), Some(width - 32.0));
        let body = ui.para(&preview.instructions, Style::new(size::BODY, weight::REGULAR, ui.t.text), width - 32.0, None);
        let top = header.h + 12.0;
        let max_h = 440.0_f32.min(bounds.h - 16.0);
        if max_h < top + 80.0 { return false; }
        let scrolling = body.h + top + 32.0 > max_h;
        let footer = if scrolling { 24.0 } else { 0.0 };
        let height = (body.h + top + 32.0 + footer).min(max_h);
        let x = (preview.anchor.x + (preview.anchor.w - width) / 2.0).clamp(bounds.x + 8.0, bounds.right() - width - 8.0);
        let above = preview.anchor.y - height - 8.0;
        let y = (if above >= bounds.y + 8.0 { above } else { preview.anchor.bottom() + 8.0 }).clamp(bounds.y + 8.0, bounds.bottom() - height - 8.0);
        let rect = Rect::new(x, y, width, height);
        let inner = Rect::new(x + 16.0, y + 16.0 + top, width - 32.0, height - top - 32.0 - footer);
        self.max_scroll = (body.h - inner.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);
        let layer = ui.layer();
        ui.shadow(rect, 12.0, 18.0, 4.0, ui.t.shadow);
        ui.round(rect, 12.0, ui.t.raised);
        ui.hit_with(rect, Action::Focus(id, PREVIEW), Cursor::Default);
        ui.draw(&header, Point::new(x + 16.0, y + 16.0));
        ui.clip(inner, 0.0);
        ui.draw(&body, Point::new(inner.x, inner.y - self.scroll));
        ui.unclip();
        if scrolling {
            let hint = ui.line("Scroll to read more", Style::new(size::CAPTION, weight::REGULAR, ui.t.muted), None);
            ui.draw(&hint, Point::new(x + 16.0, rect.bottom() - 12.0 - hint.h));
            let thumb = (inner.h * inner.h / body.h).max(18.0);
            let offset = (inner.h - thumb) * self.scroll / self.max_scroll.max(1.0);
            ui.round(Rect::new(rect.right() - 7.0, inner.y + offset, 2.0, thumb), 1.0, ui.t.muted.with_alpha(0.5));
        }
        ui.set_layer(layer);
        self.rect = Some(rect);
        true
    }
}

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
    // The matches, then "New…" (at `rows.len()`).
    let count = rows.len() + 1;
    let selected = if native { d.terminal_quips.selected } else { d.quips.selected }.min(rows.len());
    // With nothing matching, "New…" waits for a click: the keys keep their usual jobs.
    let chosen = (!rows.is_empty()).then_some(selected);
    let query = if native { d.terminal_quips.query.clone().unwrap_or_default() } else { crate::quips::query(d.area.text(), d.area.cursor()).map_or(String::new(), |(_, q)| q) };
    let shown = count.min(6);
    let first = selected.saturating_sub(shown - 1).min(count - shown);
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
    for n in first..first + shown {
        let rr = Rect::new(x + 5.0, y + 30.0 + (n - first) as f32 * ROW_H, w - 10.0, ROW_H - 2.0);
        let Some(q) = rows.get(n).and_then(|i| app.focus.quips.items.get(*i)) else {
            widgets::row(ui, rr, ("quip-pick", id, n), chosen == Some(n), Action::Focus(id, CREATE));
            ui.icon(Rect::new(rr.x + 6.0, rr.y, 18.0, rr.h), "\u{F0415}", size::BODY, t.accent);
            label(ui, "New…", Rect::new(rr.x + 28.0, rr.y, rr.w * 0.5, rr.h), Style::new(size::BODY, weight::MEDIUM, t.text));
            if !query.is_empty() {
                ui.label_in(Rect::new(rr.x + rr.w * 0.5, rr.y, rr.w * 0.5 - 9.0, rr.h), &format!(":{query}"), Style::mono(size::CAPTION, t.faint), Align::Right);
            }
            continue;
        };
        widgets::row(ui, rr, ("quip-pick", id, n), chosen == Some(n), Action::Focus(id, ROW + n));
        label(ui, &q.name, Rect::new(rr.x + 9.0, rr.y, rr.w * 0.5, rr.h), Style::new(size::BODY, weight::MEDIUM, t.text));
        ui.label_in(Rect::new(rr.x + rr.w * 0.5, rr.y, rr.w * 0.5 - 9.0, rr.h), &q.token(), Style::mono(size::CAPTION, t.accent), Align::Right);
    }
    let (note, keys) = match rows.get(selected).and_then(|i| app.focus.quips.items.get(*i)) {
        Some(q) => (crate::quips::summary(&q.trigger).to_string(), "↑ ↓ choose · tab insert · esc close"),
        None if rows.is_empty() && !query.is_empty() => (format!("Nothing is saved as :{query} yet."), "click New… to make it · esc close"),
        None if rows.is_empty() => ("Your library is empty.".into(), "click New… to write one · esc close"),
        None => ("Save your own instructions under a new shortcut.".into(), "↑ ↓ choose · tab create · esc close"),
    };
    label(ui, &note, Rect::new(x + 12.0, r.bottom() - 52.0, w - 24.0, 24.0), Style::new(size::CAPTION, weight::REGULAR, t.muted));
    label(ui, keys, Rect::new(x + 12.0, r.bottom() - 26.0, w - 24.0, 20.0), Style::new(size::CAPTION, weight::REGULAR, t.faint));
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
        let hint = if e.insert { "Saving adds it to your message · tab switches fields · ctrl+s saves" } else { "Saved on this computer · tab switches fields · ctrl+s saves" };
        label(ui, hint, Rect::new(x, r.bottom() - 71.0, w, 20.0), Style::new(size::CAPTION, weight::REGULAR, t.faint));
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
        CREATE => g.app.quip_create(id),
        SAVE => g.app.focus_save_quip(id),
        BACK => g.app.focus_close_quip_editor(id),
        PREVIEW => {}
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
        // down to "New…" past the matches (and nowhere when nothing matches)
        state.selected = (state.selected as isize - dy.round() as isize).clamp(0, rows.len() as isize) as usize;
        return true;
    }
    let rows = g.app.focus_quip_rows(id);
    let last = if matches!(g.app.overlay, Overlay::Quips(_)) { rows.len().saturating_sub(1) } else { rows.len() };
    let p = &mut g.app.focus.draft(id).quips;
    if p.editor.is_some() { return true; }
    if !p.open && rows.is_empty() { return false; }
    p.selected = (p.selected as isize - dy.round() as isize).clamp(0, last as isize) as usize;
    p.delete_armed = false;
    true
}
