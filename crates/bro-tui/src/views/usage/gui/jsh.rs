//! The Just Superhuman account on the usage view, the first card while signed in: the credits left and what
//! they're worth, this month's allowance as a meter (as a login's windows are) with when it renews, and under the
//! rule what's been spent, granted and held, and anything stopping Claude. A click starts a session on the account
//! (the launcher, on it, to pick a model).

use super::{INSET, LOGO, METER_H, card, meter, pct_color, session_chip};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::Logo;
use crate::app::gui::kit::{Style, TextBox, Ui, size, weight};
use crate::pane::PaneId;
use bro_core::jsh::{Account, Snapshot, grouped};
use bro_core::usage::Window;
use bro_gfx::{Point, Rect};

/// The card's click target in the usage view.
pub(super) const TARGET: usize = 6;
/// The credits left, large.
const FIGURE: f32 = 28.0;
const FIGURE_H: f32 = 36.0;
const ALERT: &str = "\u{F0026}";

/// Under the rule: this month's spend (or that the credits are one-time), what's granted and held, then what's
/// wrong.
struct Foot {
    spent: TextBox,
    notes: Vec<TextBox>,
    problems: Vec<TextBox>,
}

impl Foot {
    fn lay(ui: &mut Ui, snap: &Snapshot, a: &Account, inner: f32) -> Foot {
        let t = ui.t;
        let c = &a.credits;
        let quiet = Style::new(size::SMALL, weight::MEDIUM, t.muted);
        let spans = match c.month_used_pct() {
            Some(pct) => vec![
                ("this month  ".to_string(), quiet),
                (format!("{} spent", grouped(c.used_this_month())), Style::new(size::SMALL, weight::SEMIBOLD, pct_color(pct, t))),
                ("  ·  ".into(), Style::new(size::SMALL, weight::REGULAR, t.faint)),
                (format!("{} a month", grouped(c.monthly_allowance)), quiet),
            ],
            None => vec![("one-time credits  ".to_string(), quiet), ("they never expire".into(), Style::new(size::SMALL, weight::SEMIBOLD, t.text))],
        };
        let spent = ui.rich(&spans, size::SMALL, Some(inner), false, Some(1));
        let faint = Style::new(size::SMALL, weight::REGULAR, t.faint);
        let mut notes = vec![];
        if c.granted > 0 && c.monthly_allowance > 0 {
            notes.push(ui.para(&format!("Includes {} one-time credits that never expire.", grouped(c.granted)), faint, inner, Some(2)));
        }
        if c.reserved > 0 {
            notes.push(ui.para(&format!("{} held for replies still being written.", grouped(c.reserved)), faint, inner, Some(2)));
        }
        let danger = Style::new(size::SMALL, weight::MEDIUM, t.danger);
        let problems = problems(snap, Some(a)).iter().map(|p| ui.para(p, danger, inner - 18.0, Some(3))).collect();
        Foot { spent, notes, problems }
    }

    fn height(&self) -> f32 {
        self.spent.h + self.notes.iter().map(|b| b.h).sum::<f32>() + self.problems.iter().map(|b| 8.0 + b.h).sum::<f32>()
    }
}

/// Why Claude can't run on the account, then the last refresh's failure.
fn problems(snap: &Snapshot, a: Option<&Account>) -> Vec<String> {
    let mut out: Vec<String> = a.and_then(Account::paused_reason).map(str::to_string).into_iter().collect();
    if let Some(e) = snap.error.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        out.push(if a.is_some() { format!("last refresh failed: {e}") } else { e.to_string() });
    }
    out
}

/// Signed in, but no reading of the account yet: that it's being read, or why it couldn't be.
fn waiting(ui: &mut Ui, snap: &Snapshot, inner: f32) -> TextBox {
    let (text, c) = match problems(snap, None).into_iter().next() {
        Some(p) => (p, ui.t.danger),
        None => ("Reading your credits…".to_string(), ui.t.muted),
    };
    ui.para(&text, Style::new(size::SMALL, weight::MEDIUM, c), inner - 24.0, Some(3))
}

/// How tall the card wants to be at width `w`.
pub(super) fn height(ui: &mut Ui, snap: &Snapshot, w: f32) -> f32 {
    let inner = w - 2.0 * INSET;
    let mut h = INSET + 36.0;
    match &snap.account {
        Some(a) => {
            h += 18.0 + FIGURE_H;
            if a.credits.month_used_pct().is_some() {
                h += 12.0 + METER_H;
            }
            h += 18.0 + 12.0 + Foot::lay(ui, snap, a, inner).height() + INSET;
        }
        None => h += 18.0 + waiting(ui, snap, inner).h.max(20.0) + INSET,
    }
    h
}

pub(super) fn draw(ui: &mut Ui, r: Rect, snap: &Snapshot, id: PaneId) {
    let t = ui.t;
    let hot = ui.hover_amount(("usage-jsh", id), r);
    // it starts a session: its fill steps up under the pointer, down while pressed
    let fill = card(ui, r, hot, ui.pressing && hot > 0.5);
    ui.hit(r, Action::View(id, TARGET));

    // header: the knot, the name; the organization that pays and who's signed in under it
    let x0 = r.x + INSET;
    let right = r.right() - INSET;
    let inner = right - x0;
    let head = Rect::new(x0, r.y + INSET, inner, 36.0);
    ui.logo(Logo::JustSuperhuman, Rect::new(x0, head.y + (head.h - LOGO) / 2.0, LOGO, LOGO), t.text);
    let tx = x0 + LOGO + 14.0;
    let row1 = Rect::new(tx, head.y - 1.0, right - tx, 20.0);
    let spin = if snap.refreshing { 20.0 } else { 0.0 };
    let name = ui.line(bro_core::jsh::PROVIDER_NAME, Style::new(size::TITLE, weight::SEMIBOLD, t.text), Some((row1.w - spin - 4.0).max(24.0)));
    ui.draw(&name, Point::new(tx, row1.y + (row1.h - name.h) / 2.0));
    if snap.refreshing {
        ui.spinner(Point::new(tx + name.w + 15.0, row1.y + 10.0), 4.6, 1.6, t.accent);
    }
    session_chip(ui, right, row1.y, fill, hot);
    let a = snap.account.as_ref();
    let sub = match a {
        Some(a) => {
            let org = a.organization.name.trim();
            let mut s = vec![(if org.is_empty() { "Your team".to_string() } else { org.to_string() }, Style::new(size::SMALL, weight::SEMIBOLD, t.shine))];
            let email = a.user.email.trim();
            if !email.is_empty() {
                s.push((format!("  ·  {email}"), Style::new(size::SMALL, weight::REGULAR, t.muted)));
            }
            s
        }
        None => vec![("Your team's credits".to_string(), Style::new(size::SMALL, weight::REGULAR, t.muted))],
    };
    let sb = ui.rich(&sub, size::SMALL, Some(right - tx), false, Some(1));
    ui.draw(&sb, Point::new(tx, head.bottom() - sb.h + 1.0));

    let mut y = head.bottom() + 18.0;
    let Some(a) = a else {
        let b = waiting(ui, snap, inner);
        let icon = Rect::new(x0, y, 16.0, 18.0);
        if snap.error.is_some() {
            ui.icon(icon, ALERT, size::BODY, t.danger);
        } else {
            ui.spinner(Point::new(icon.x + 7.0, icon.y + 9.0), 4.6, 1.6, t.accent);
        }
        ui.draw(&b, Point::new(x0 + 24.0, y + 1.0));
        return;
    };
    figure(ui, Rect::new(x0, y, inner, FIGURE_H), a);
    y += FIGURE_H;
    if let Some(pct) = a.credits.month_used_pct() {
        y += 12.0;
        let w = Window { used_pct: pct, resets_at: a.credits.resets_at.map(|ms| ms / 1000), window_mins: None };
        meter(ui, Rect::new(x0, y, inner, METER_H), "month", Some(&w), 8.0);
    }

    // the footer sits on the card's bottom edge so it lines up with the logins' across a row
    let foot = Foot::lay(ui, snap, a, inner);
    let mut fy = r.bottom() - INSET - foot.height();
    ui.hline(x0, fy - 12.0, inner, t.hairline);
    ui.draw(&foot.spent, Point::new(x0, fy));
    fy += foot.spent.h;
    for b in &foot.notes {
        ui.draw(b, Point::new(x0, fy));
        fy += b.h;
    }
    for b in &foot.problems {
        fy += 8.0;
        ui.icon(Rect::new(x0 - 1.0, fy, 14.0, 16.0), ALERT, size::SMALL, t.danger);
        ui.draw(b, Point::new(x0 + 18.0, fy));
        fy += b.h;
    }
}

/// The credits left, large, "credits left" on their baseline, and what they're worth at the right when it fits.
fn figure(ui: &mut Ui, r: Rect, a: &Account) {
    let t = ui.t;
    let c = &a.credits;
    let fb = ui.line(&grouped(c.available), Style::new(FIGURE, weight::SEMIBOLD, if a.running_low() { t.danger } else { t.text }), None);
    let fy = r.y + (r.h - fb.h) / 2.0;
    let base = fy + fb.h * 0.735;
    ui.draw(&fb, Point::new(r.x, fy));
    let unit = ui.line(if c.available == 1 { "credit left" } else { "credits left" }, Style::new(size::BODY, weight::MEDIUM, t.muted), Some((r.w - fb.w - 8.0).max(1.0)));
    ui.draw(&unit, Point::new(r.x + fb.w + 8.0, base - unit.h * 0.735));
    let usd = ui.line(&c.dollars(c.available), Style::new(size::BODY, weight::SEMIBOLD, t.text.mix(t.muted, 0.3)), None);
    if fb.w + 8.0 + unit.w + 16.0 + usd.w <= r.w {
        ui.draw(&usd, Point::new(r.right() - usd.w, base - usd.h * 0.735));
    }
}
