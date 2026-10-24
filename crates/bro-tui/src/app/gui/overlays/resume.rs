//! The "resume in which login?" / "move to another login" picker in the desktop app.
//!
//! Keys (↑↓ j k, 1-9, ⏎, esc) reach `App::resume_key` through the workspace; this draws that state and takes the
//! mouse as the terminal UI does: a click selects a login and a click on the selected one chooses it. The buttons
//! cancel or choose. Clicks outside and the wheel do nothing, as in the terminal.

use super::super::actions::Action;
use super::super::kit::{Style, Ui, brand, radius, size, weight, widgets};
use super::super::sidebar;
use super::palette::{CAP_H, appear, button_band, current, glide, hints, just_opened, sentence};
use crate::app::overlays::{ResumeFrom, ResumeTarget};
use crate::app::{App, Overlay};
use bro_gfx::{Point, Rect};
use crossterm::event::KeyModifiers;
use std::cell::Cell;

const CANCEL: usize = 1;
const RESUME: usize = 2;
/// Targets: `ROW + i` is `targets[i]`.
const ROW: usize = 100;
/// (wide enough for the four key hints beside the buttons)
const WIDTH: f32 = 620.0;
/// A login's row (two lines), the dialog's padding, the header (the agent's logo beside two lines of text).
const ROW_H: f32 = 48.0;
const PAD: f32 = 24.0;
const HEAD_H: f32 = 40.0;
const LOGO: f32 = 30.0;
/// The band along the bottom with the buttons, and a button's height.
const BAND_H: f32 = 64.0;
const BUTTON_H: f32 = 32.0;
/// The "% left" column: the figure over its segments.
const USAGE_W: f32 = 88.0;

thread_local! {
    /// Which picker (its address) and the first login in view when they don't all fit.
    static TOP: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// Draw the picker over the window; register targets with `ui.hit(r, Action::Overlay(n))`. False = use the
/// terminal UI's drawing.
pub fn draw(app: &mut App, ui: &mut Ui, window: Rect, _time: f64) -> bool {
    let Overlay::Resume(p) = &app.overlay else { return false };
    let t = ui.t;
    let opened = just_opened(ui);
    let id = &**p as *const crate::app::overlays::ResumePicker as usize;
    let (was, top) = TOP.get();
    let fresh = opened || was != id;
    let k = appear(ui, "resume", opened);
    widgets::scrim(ui, window, k, Action::None);
    let w = WIDTH.min(window.w - 32.0).max(300.0);
    let live = matches!(p.from, ResumeFrom::Live { .. });
    let reload = matches!(p.from, ResumeFrom::Reload { .. });
    let (caption, note) = if reload {
        ("reload with profile", "reload the conversation with the selected profile's current settings")
    } else if live {
        ("move to another login", "the running session closes and continues in the login you pick")
    } else {
        ("resume in", "another login continues from a copy of the conversation")
    };
    let note = ui.para(note, Style::new(size::BODY, weight::REGULAR, t.muted), w - 2.0 * PAD, Some(2));
    // the logins that fit, the selected one kept in view
    let fixed = PAD + HEAD_H + 12.0 + note.h + 14.0 + 12.0 + BAND_H;
    let fit = ((window.h - 48.0 - fixed) / ROW_H).floor().max(1.0) as usize;
    let n = p.targets.len();
    let rows = n.min(fit);
    let sel = p.sel.min(n.saturating_sub(1));
    let mut top = if fresh { 0 } else { top };
    if sel < top {
        top = sel;
    }
    if rows > 0 && sel >= top + rows {
        top = sel + 1 - rows;
    }
    top = top.min(n.saturating_sub(rows));
    TOP.set((id, top));
    let h = fixed + rows as f32 * ROW_H;
    let y = (window.y + window.h * 0.3 - h / 2.0).max(window.y + 16.0).round();
    let r = Rect::new((window.x + (window.w - w) / 2.0).round(), y + (1.0 - k) * 10.0, w, h);
    widgets::card(ui, r, radius::OVERLAY);

    // the agent's logo, what this does, the session
    let head = Rect::new(r.x + PAD, r.y + PAD, r.w - 2.0 * PAD, HEAD_H);
    let mark = Rect::new(head.x, head.y + (HEAD_H - LOGO) / 2.0, LOGO, LOGO);
    ui.mark(brand::harness(p.from.harness()), mark, LOGO - 4.0, None);
    let tx = mark.right() + 14.0;
    let tw = head.right() - tx;
    let title = ui.line(p.from.title(), Style::new(size::HEADING, weight::SEMIBOLD, t.text), Some(tw));
    let cap_h = 16.0;
    let gap = 2.0;
    let ty = head.y + (HEAD_H - cap_h - gap - title.h) / 2.0;
    widgets::section(ui, Rect::new(tx, ty, tw, cap_h), &sentence(caption));
    ui.draw(&title, Point::new(tx, ty + cap_h + gap));
    let mut y = head.bottom() + 12.0;
    ui.draw(&note, Point::new(r.x + PAD, y));
    y += note.h + 14.0;

    // the logins
    let list = Rect::new(r.x + 12.0, y, r.w - 24.0, rows as f32 * ROW_H);
    let shown = glide(ui, "resume", top as f32, fresh);
    ui.clip(list, 0.0);
    for (i, tg) in p.targets.iter().enumerate().skip(shown.floor().max(0.0) as usize).take(rows + 1) {
        let rr = Rect::new(list.x, list.y + (i as f32 - shown) * ROW_H, list.w, ROW_H);
        row(ui, rr, i, tg, i == sel);
    }
    ui.unclip();
    if n > rows {
        widgets::scrollbar(ui, Rect::new(list.x, list.y, list.w + 10.0, list.h), n as f32 * ROW_H, list.h, shown * ROW_H, 0.6);
    }

    // the band along the bottom: the keys, quietly, and the buttons
    let band = button_band(ui, r, BAND_H);
    let by = band.y + ((band.h - BUTTON_H) / 2.0).round();
    let verb = if reload { "Reload" } else { "Resume" };
    let rw = widgets::button_width(ui, verb).max(96.0);
    let cw = widgets::button_width(ui, "Cancel").max(88.0);
    let resume = Rect::new(r.right() - PAD - rw, by, rw, BUTTON_H);
    let cancel = Rect::new(resume.x - 8.0 - cw, by, cw, BUTTON_H);
    widgets::button(ui, cancel, "Cancel", widgets::Kind::Secondary, Action::Overlay(CANCEL));
    widgets::button(ui, resume, verb, widgets::Kind::Primary, Action::Overlay(RESUME));
    let room = cancel.x - 16.0 - (r.x + PAD);
    if room > 40.0 {
        let pairs = [("⏎", if reload { "reload" } else { "resume" }), ("1-9", "pick + go"), ("↑↓", "move"), ("esc", "cancel")];
        hints(ui, Some(Point::new(r.x + PAD, by + (BUTTON_H - CAP_H) / 2.0)), &pairs, room);
    }
    true
}

/// One login: its number (1-9 picks it), name, "current", plan · email, and what's left as a figure over
/// segments (the terminal UI's ▰▰▱).
fn row(ui: &mut Ui, r: Rect, i: usize, tg: &ResumeTarget, selected: bool) {
    let t = ui.t;
    widgets::row(ui, r, ("resume", i), selected, Action::Overlay(ROW + i));
    if i < 9 {
        widgets::kbd(ui, Point::new(r.x + 12.0, r.y + (r.h - CAP_H) / 2.0), &(i + 1).to_string());
    }
    let x = r.x + 44.0;
    let mut right = r.right() - 14.0;
    // two lines: the name over the detail (and the figure over its segments)
    let (line1, line2) = (r.y + 15.0, r.y + 34.0);
    if let Some(left) = tg.left {
        // ≤ 20% danger, ≤ 50% amber, else good (the sidebar's usage colours)
        let c = sidebar::left_color(left, t);
        let s = ui.line(&format!("{left:.0}% left"), Style::new(size::SMALL, weight::SEMIBOLD, c), None);
        ui.draw(&s, Point::new(right - s.w, line1 - s.h / 2.0));
        let seg = Rect::new(right - USAGE_W, line2 - 3.0, USAGE_W, 6.0);
        widgets::segments(ui, seg, (left / 100.0) as f32, widgets::segment_count(USAGE_W, 7.0), c);
        right -= USAGE_W + 20.0;
    }
    let room = (right - x).max(1.0);
    let name = ui.line(&tg.name, Style::new(size::BODY, weight::SEMIBOLD, if selected { t.text } else { t.text.mix(t.muted, 0.15) }), Some(room));
    let detail = (!tg.detail.is_empty()).then(|| ui.line(&tg.detail, Style::new(size::SMALL, weight::REGULAR, t.muted), Some(room)));
    let ny = if detail.is_some() { line1 - name.h / 2.0 } else { r.y + (r.h - name.h) / 2.0 };
    ui.draw(&name, Point::new(x, ny));
    if tg.current {
        let cw = current(ui, None);
        if x + name.w + 10.0 + cw <= right {
            current(ui, Some(Point::new(x + name.w + 10.0, ny + name.h / 2.0)));
        }
    }
    if let Some(d) = detail {
        ui.draw(&d, Point::new(x, line2 - d.h / 2.0));
    }
}

/// Target `n` was clicked: a login selects it, the selected one chooses it; the buttons cancel or choose.
pub fn click(app: &mut App, n: usize, _p: Point, _m: KeyModifiers, _count: u32) {
    match n {
        CANCEL => app.overlay = Overlay::None,
        RESUME => app.resume_chosen(),
        n if n >= ROW => {
            let Overlay::Resume(p) = &mut app.overlay else { return };
            let i = n - ROW;
            if i >= p.targets.len() {
                return;
            }
            if i == p.sel {
                app.resume_chosen();
            } else {
                p.sel = i;
            }
        }
        _ => {}
    }
}

/// The wheel does nothing here, as in the terminal UI.
pub fn wheel(_app: &mut App, _n: Option<usize>, _down: bool) {}
