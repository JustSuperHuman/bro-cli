//! Notifications in the top-right corner (newest on top, at most two, each for two seconds, as in the terminal
//! UI), and the armed-prefix key table in the bottom-right.
//!
//! As in the terminal UI they sit over the sessions, the alt key tips and the launcher (which fills the body like
//! a view), but under a dialog: [`draw`] runs before the overlays and paints them only while a dialog is open;
//! [`draw_late`] runs last and paints them otherwise.

use super::actions::Action;
use super::kit::{Style, Ui, radius, size, weight};
use crate::alerts::Kind;
use crate::app::{App, Overlay};
use bro_gfx::{Point, Rect};

/// A dialog (not the launcher) is open: notifications go under it.
fn under_dialog(app: &App) -> bool {
    app.overlay.is_open() && !matches!(app.overlay, Overlay::Launcher(_))
}

/// Before the overlays: the notifications, when a dialog will cover them.
pub fn draw(app: &mut App, ui: &mut Ui, window: Rect) {
    if under_dialog(app) {
        paint(app, ui, window);
    }
}

/// After everything else: the notifications over the sessions, the key tips and the launcher.
pub fn draw_late(app: &mut App, ui: &mut Ui, window: Rect) {
    if !under_dialog(app) {
        if app.prefix_armed {
            super::menu::draw(app, ui, window);
            return;
        }
        paint(app, ui, window);
    }
}

fn paint(app: &App, ui: &mut Ui, window: Rect) {
    let t = ui.t;
    let prev = ui.layer();
    if app.prefix_armed {
        // the prefix table, bottom-right
        ui.set_layer(prev);
        return;
    }
    // under the window's buttons
    let mut y = window.y.max(super::caption::bottom()) + 14.0;
    for (i, toast) in app.toasts.items.iter().rev().enumerate() {
        let (icon, c) = match toast.kind {
            Kind::Error => ("\u{F0159}", t.danger),
            Kind::NeedsYou => ("\u{F009A}", t.danger),
            Kind::AgentDone => ("\u{F05E0}", t.good),
            Kind::Usage => ("\u{F029A}", t.inline),
            Kind::Bridge => ("\u{F05A9}", t.shine),
            Kind::Info => ("\u{F02FC}", t.accent),
        };
        let text = toast.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let body = ui.para(&text, Style::new(size::BODY, weight::MEDIUM, t.text), 380.0, Some(3));
        let w = body.w + 58.0;
        let h = (body.h + 22.0).max(42.0);
        // slide in from the right
        let k = ui.ease(("toast", toast.text.as_str(), i), 1.0, 160.0);
        let x = window.right() - w - 16.0 + (1.0 - k) * 40.0;
        let r = Rect::new(x, y, w, h);
        ui.shadow(r, radius::OVERLAY, 28.0, 8.0, t.shadow.fade(0.8 * k));
        ui.panel(r, radius::OVERLAY, t.raised, t.border.fade(k));
        // the icon in its colour, standing on the card (no tile behind it)
        let ir = Rect::new(r.x + 12.0, r.y + (h - 28.0) / 2.0, 24.0, 28.0);
        ui.icon(ir, icon, size::HEADING, c.fade(k));
        ui.draw_faded(&body, Point::new(r.x + 46.0, r.y + (h - body.h) / 2.0), k);
        ui.hit(r, Action::Toast(i));
        y += h + 8.0;
    }
    ui.set_layer(prev);
}
