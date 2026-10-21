//! The confirmation dialog: quit with sessions running, close a busy agent, close a project with live
//! sessions. y / Enter answers yes and any other key no (the workspace handles keys); the buttons take clicks.

use super::super::actions::Action;
use super::super::kit::{Style, Ui, radius, size, weight, widgets};
use super::palette::{CAP_H, appear, button_band, hints, just_opened};
use crate::app::{App, Overlay};
use bro_gfx::{Point, Rect};
use crossterm::event::KeyModifiers;

const YES: usize = 1;
const NO: usize = 0;
/// The dialog's padding, the band along its bottom with the buttons, and a button's height.
const PAD: f32 = 24.0;
const BAND_H: f32 = 64.0;
const BUTTON_H: f32 = 32.0;

pub fn draw(app: &mut App, ui: &mut Ui, window: Rect, _time: f64) -> bool {
    let Overlay::Confirm(c) = &app.overlay else { return false };
    let t = ui.t;
    // fade and lift in (every time one opens, not only the first)
    let opened = just_opened(ui);
    let k = appear(ui, "confirm", opened);
    widgets::scrim(ui, window, k, Action::None);
    let w = 440.0f32.min(window.w - 48.0);
    let text = ui.para(&c.text, Style::new(size::HEADING, weight::SEMIBOLD, t.text), w - 2.0 * PAD, Some(3));
    let detail = ui.para(&c.detail, Style::new(size::BODY, weight::REGULAR, t.muted), w - 2.0 * PAD, Some(4));
    let h = PAD + text.h + 8.0 + detail.h + PAD + BAND_H;
    let r = Rect::new(window.x + (window.w - w) / 2.0, window.y + window.h * 0.3 - h / 2.0 + (1.0 - k) * 12.0, w, h);
    widgets::card(ui, r, radius::OVERLAY);
    // the question, then what it means
    let mut y = r.y + PAD;
    ui.draw(&text, Point::new(r.x + PAD, y));
    y += text.h + 8.0;
    ui.draw(&detail, Point::new(r.x + PAD, y));
    // the band along the bottom: the keys, quietly, and the buttons
    let band = button_band(ui, r, BAND_H);
    let by = band.y + ((band.h - BUTTON_H) / 2.0).round();
    // the terminal UI's words: " Yes  (y) " / " No (esc) " (the keys go in the hints beside them)
    let yes_label = "Yes";
    let no_label = "No";
    let yw = widgets::button_width(ui, yes_label).max(96.0);
    let nw = widgets::button_width(ui, no_label).max(96.0);
    let yes = Rect::new(r.right() - PAD - yw, by, yw, BUTTON_H);
    let no = Rect::new(yes.x - 8.0 - nw, by, nw, BUTTON_H);
    widgets::button(ui, no, no_label, widgets::Kind::Secondary, Action::Overlay(NO));
    widgets::button(ui, yes, yes_label, widgets::Kind::Danger, Action::Overlay(YES));
    let room = no.x - 16.0 - (r.x + PAD);
    if room > 40.0 {
        hints(ui, Some(Point::new(r.x + PAD, by + (BUTTON_H - CAP_H) / 2.0)), &[("y", "yes"), ("esc", "no")], room);
    }
    true
}

pub fn click(app: &mut App, n: usize, _p: Point, _m: KeyModifiers, _count: u32) {
    match n {
        YES => app.confirm_answer(true),
        NO => app.confirm_answer(false),
        _ => {}
    }
}
