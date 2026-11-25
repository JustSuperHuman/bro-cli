//! Native rendering of the workspace's prefix-key menu.

use super::actions::Action;
use super::kit::{Style, Ui, radius, size, weight, widgets};
use crate::app::App;
use bro_gfx::{Point, Rect};

pub fn draw(app: &mut App, ui: &mut Ui, window: Rect) {
    let prev = ui.layer();
    let items = app.menu_actions();
    let w = 560.0_f32.min((window.w - 16.0).max(1.0));
    let row_h = 30.0;
    let rows = items.len().min(((window.h - 100.0) / row_h).floor().clamp(1.0, 16.0) as usize);
    app.menu.keep_visible(rows, items.len());
    let h = (rows as f32 * row_h + 76.0).min(window.h);
    let (x, y) = app.menu.anchor.unwrap_or((window.x + (window.w - w) / 2.0, window.y + 48.0));
    let r = Rect::new(x.clamp(window.x, window.right() - w), y.clamp(window.y, window.bottom() - h), w, h);
    let t = ui.t;
    widgets::card(ui, r, radius::OVERLAY);
    ui.hit(r, Action::None);
    ui.label(Point::new(r.x + 14.0, r.y + 12.0), &format!("Session menu · {}", app.keymap.prefix.display()), Style::new(size::SMALL, weight::SEMIBOLD, t.accent));
    for (row, (index, (key, act))) in items.iter().enumerate().skip(app.menu.top).take(rows).enumerate() {
        let rr = Rect::new(r.x + 6.0, r.y + 36.0 + row as f32 * row_h, r.w - 12.0, row_h);
        widgets::row(ui, rr, ("session-menu", index), index == app.menu.sel, Action::Menu(index));
        if !key.is_empty() {
            widgets::kbd(ui, Point::new(rr.right() - 64.0, rr.y + 6.0), key);
        }
        let text = ui.line(&crate::app::menu::label(*act), Style::new(size::BODY, weight::MEDIUM, t.text), Some((rr.w - 94.0).max(1.0)));
        ui.draw(&text, Point::new(rr.x + 10.0, rr.y + (row_h - text.h) / 2.0));
    }
    ui.label(Point::new(r.x + 14.0, r.bottom() - 25.0), "↑↓ Select   Enter Run   Esc Cancel", Style::new(size::SMALL, weight::REGULAR, t.muted));
    if items.len() > rows {
        widgets::scrollbar(ui, Rect::new(r.right() - 6.0, r.y + 36.0, 4.0, rows as f32 * row_h), items.len() as f32 * row_h, rows as f32 * row_h, app.menu.top as f32 * row_h, 0.7);
    }
    ui.set_layer(prev);
}
