//! The Just Superhuman account at the bottom of Settings' section list: the knot beside who's signed in and the
//! organization's credits, with **Sign out** under them, or beside "Just Superhuman" and what it gives you, with
//! **Sign in** (on ink: black on light, light on dark, the way the mark is drawn). While the browser approval waits
//! it shows the code to check, with **Open browser** and **Cancel**. A window too short for all of that gets it as
//! one row, and a narrow one (no section list) gets that row under the page.
//!
//! It draws [`bro_core::jsh::Login`]'s state, which the desktop app, the terminal UI, the phone and `bro jsh` share;
//! the requests run on their own threads and the window wakes when the state changes.

use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand::Logo;
use crate::app::gui::kit::widgets::{self, Kind};
use crate::app::gui::kit::{Style, TextBox, Ui, radius, size, weight};
use crate::pane::{Cx, PaneId};
use crate::views::profiles::gui::{BUTTON_H, key_button, key_button_width};
use bro_core::jsh::{self, Account, Snapshot};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use std::sync::atomic::{AtomicBool, Ordering};

/// The settings view's targets for the account (between its sections' and its settings').
pub(super) const SIGN_IN: usize = 900;
pub(super) const SIGN_OUT: usize = 901;
pub(super) const CANCEL: usize = 902;
pub(super) const OPEN: usize = 903;
pub(super) const FIRST: usize = SIGN_IN;
pub(super) const LAST: usize = OPEN;

/// The knot with its two lines beside it.
const BADGE_H: f32 = 38.0;
/// One row: the badge and a button.
const ROW_H: f32 = 36.0;
const KNOT: f32 = 22.0;
const KNOT_SMALL: f32 = 18.0;
const NAME: &str = "Just Superhuman";
const PITCH: &str = "Claude on your team's credits";
const PITCH_LONG: &str = "Sign in once and Claude Code runs on your organization's Just Superhuman credits, with no Claude login. It shows up in the launcher and on your phone.";

/// A sign-in or sign-out request is out (the first request of a sign-in, a revoke): the button shows it.
static BUSY: AtomicBool = AtomicBool::new(false);

/// The sign-in as last read (not on every frame: see [`jsh::Login::snapshot_recent`]). The demo's team account
/// lives only in its state.
fn snapshot(cx: &Cx) -> Snapshot {
    if cx.svc.is_demo() { cx.svc.state().jsh.clone() } else { jsh::login().snapshot_recent() }
}

fn small(c: Color) -> Style {
    Style::new(size::SMALL, weight::REGULAR, c)
}

fn para(ui: &mut Ui, text: &str, st: Style, w: f32, lines: usize) -> TextBox {
    ui.para(text, st, w.max(40.0), Some(lines))
}

/// What went wrong last (a declined or expired sign-in, a failed refresh), to read in full.
fn problem(snap: &Snapshot) -> Option<&str> {
    snap.error.as_deref().map(str::trim).filter(|e| !e.is_empty())
}

/// "Check that the page shows this code, then approve. It works until 4:42 PM." A clock time, so it never goes
/// stale while the window sits still.
fn pending_note(expires_at: i64) -> String {
    let until = chrono::DateTime::from_timestamp_millis(expires_at).map(|t| t.with_timezone(&chrono::Local).format("%-I:%M %p").to_string());
    match until {
        Some(at) => format!("Check that the page shows this code, then approve. It works until {at}."),
        None => "Check that the page shows this code, then approve.".into(),
    }
}

/// The lines under the badge that wrap: why Claude can't run, then what went wrong last.
fn notes(snap: &Snapshot) -> Vec<&str> {
    let reason = snap.account.as_ref().filter(|_| snap.signed_in).and_then(Account::paused_reason);
    reason.into_iter().chain(problem(snap)).collect()
}

/// How tall the account is at `w` wide: whole, or as one row (`compact`).
pub(super) fn height(ui: &mut Ui, w: f32, compact: bool, cx: &Cx) -> f32 {
    if compact {
        return ROW_H;
    }
    let snap = snapshot(cx);
    let danger = small(ui.t.danger);
    if let Some(p) = &snap.pending {
        let note = para(ui, &pending_note(p.expires_at), small(ui.t.muted), w, 3);
        return 20.0 + 6.0 + 28.0 + 4.0 + note.h + 12.0 + BUTTON_H;
    }
    let notes: f32 = notes(&snap).iter().map(|n| para(ui, n, danger, w, 4).h + 8.0).sum();
    BADGE_H + 12.0 + notes + BUTTON_H
}

/// Draw the account in `r` (as tall as [`height`] said).
pub(super) fn draw(ui: &mut Ui, r: Rect, id: PaneId, compact: bool, cx: &Cx) {
    let t = ui.t;
    let snap = snapshot(cx);
    let act = |n: usize| Some(Action::View(id, n));
    let busy = BUSY.load(Ordering::Acquire);
    if let Some(p) = &snap.pending {
        return if compact { pending_row(ui, r, &p.user_code, act) } else { pending(ui, r, p, act) };
    }
    // the badge: who and the credits, or what signing in gives you
    let account = snap.account.as_ref().filter(|_| snap.signed_in);
    let (title, line, ink, tip) = match account {
        Some(a) => {
            let who = Some(a.who().trim()).filter(|w| !w.is_empty()).unwrap_or(NAME).to_string();
            let (email, org) = (a.user.email.trim(), a.organization.name.trim());
            let tip = match (email.is_empty(), org.is_empty()) {
                (false, false) => Some(format!("Signed in as {email}. Sessions on Just Superhuman spend {org}'s credits.")),
                (false, true) => Some(format!("Signed in as {email}. Sessions on Just Superhuman spend your team's credits.")),
                (true, false) => Some(format!("Sessions on Just Superhuman spend {org}'s credits.")),
                (true, true) => None,
            };
            (who, Line::Credits(a), if a.running_low() { t.danger } else { t.muted }, tip)
        }
        None if snap.signed_in => (NAME.to_string(), Line::Text("Loading your credits…"), t.muted, None),
        None => (NAME.to_string(), Line::Text(PITCH), t.muted, Some(PITCH_LONG.to_string())),
    };
    let (label, kind, target) = match (snap.signed_in, busy) {
        (true, false) => ("Sign out", Kind::Secondary, SIGN_OUT),
        (true, true) => ("Signing out…", Kind::Secondary, SIGN_OUT),
        (false, false) => ("Sign in", Kind::Primary, SIGN_IN),
        (false, true) => ("Opening your browser…", Kind::Primary, SIGN_IN),
    };
    let action = (!busy).then(|| act(target)).flatten();
    if compact {
        let w = (key_button_width(ui, None, label, None) + if busy { 22.0 } else { 0.0 }).max(78.0);
        let by = r.y + (r.h - BUTTON_H) / 2.0;
        let room = r.w - w - 10.0 - KNOT_SMALL - 13.0;
        if room >= ui.measure(&title, Style::new(size::BODY, weight::SEMIBOLD, t.text)) {
            let b = Rect::new(r.right() - w, by, w, BUTTON_H);
            account_button(ui, b, label, kind, busy, action);
            badge(ui, Rect::new(r.x, r.y, (b.x - 10.0 - r.x).max(40.0), r.h), KNOT_SMALL, &title, line, ink, tip.as_deref());
        } else {
            // a short window's section list: no room for the name beside the button, so the knot and a wide button
            // (who, or what it's for, on the knot's hover)
            let knot = Rect::new(r.x, r.y, KNOT_SMALL + 12.0, r.h);
            ui.logo(Logo::JustSuperhuman, Rect::new(r.x + 1.0, r.y + (r.h - KNOT_SMALL) / 2.0, KNOT_SMALL, KNOT_SMALL), t.text);
            account_button(ui, Rect::new(knot.right(), by, r.right() - knot.right(), BUTTON_H), label, kind, busy, action);
            let who = match account {
                Some(_) => tip.clone().unwrap_or_else(|| format!("Signed in to Just Superhuman as {title}")),
                None => tip.clone().unwrap_or_else(|| NAME.to_string()),
            };
            if ui.hovered(knot) {
                hover_tip(ui, r, Some(&who));
            }
        }
        return;
    }
    badge(ui, Rect::new(r.x, r.y, r.w, BADGE_H), KNOT, &title, line, ink, tip.as_deref());
    let mut y = r.y + BADGE_H + 12.0;
    for n in notes(&snap) {
        let b = para(ui, n, small(t.danger), r.w, 4);
        ui.draw(&b, Point::new(r.x, y - 4.0));
        y += b.h + 8.0;
    }
    account_button(ui, Rect::new(r.x, y, r.w, BUTTON_H), label, kind, busy, action);
}

/// The badge's second line.
#[derive(Clone, Copy)]
enum Line<'a> {
    Text(&'a str),
    /// "1,840 credits · Rivera Studio", or just the credits when the organization doesn't fit
    Credits(&'a Account),
}

/// The knot, a name and a line under it; `tip` on hover.
fn badge(ui: &mut Ui, r: Rect, knot: f32, title: &str, line: Line, ink: Color, tip: Option<&str>) {
    let t = ui.t;
    ui.logo(Logo::JustSuperhuman, Rect::new(r.x + 1.0, r.y + (r.h - knot) / 2.0, knot, knot), t.text);
    let tx = r.x + knot + 13.0;
    let tw = (r.right() - tx).max(20.0);
    let name = ui.line(title, Style::new(size::BODY, weight::SEMIBOLD, t.text), Some(tw));
    let text = match line {
        Line::Text(s) => s.to_string(),
        Line::Credits(a) if ui.measure(&a.summary(), small(ink)) <= tw => a.summary(),
        Line::Credits(a) => a.credits_text(),
    };
    let sub = ui.line(&text, small(ink), Some(tw));
    let top = r.y + (r.h - name.h - 1.0 - sub.h) / 2.0;
    ui.draw(&name, Point::new(tx, top));
    ui.draw(&sub, Point::new(tx, top + name.h + 1.0));
    hover_tip(ui, r, tip);
}

/// `tip` over `r` while it's hovered, no wider than `r` (the section list clips what's drawn past it).
fn hover_tip(ui: &mut Ui, r: Rect, tip: Option<&str>) {
    if let Some(tip) = tip
        && ui.hovered(r)
    {
        widgets::tooltip_wrapped(ui, r, tip, (r.w - 16.0).max(160.0));
    }
}

/// Sign in (on ink) or Sign out (a standard button). Busy: a spinner before the label, and no clicks.
fn account_button(ui: &mut Ui, r: Rect, label: &str, kind: Kind, busy: bool, action: Option<Action>) {
    if !matches!(kind, Kind::Primary) {
        if busy {
            key_button(ui, r, None, "", None, kind, None);
            spinning_label(ui, r, label, ui.t.muted);
        } else {
            key_button(ui, r, None, label, None, kind, action);
        }
        return;
    }
    let t = ui.t;
    let hot = if action.is_some() { ui.hover_amount(("jsh-sign-in", r.x as i32, r.y as i32), r) } else { 0.0 };
    let down = action.is_some() && ui.pressing && hot > 0.5;
    let base = t.text.with_alpha(1.0);
    let fill = match () {
        _ if busy => base.mix(t.muted, 0.3),
        _ if down => base.mix(t.muted, 0.36),
        _ => base.mix(t.muted, 0.18 * hot),
    };
    ui.round(r, radius::CONTROL, fill);
    let ink = t.ink_on(base);
    if busy {
        spinning_label(ui, r, label, ink);
    } else {
        ui.label_in(r, label, Style::new(size::BODY, weight::SEMIBOLD, ink), Align::Center);
    }
    if let Some(a) = action {
        ui.hit(r, a);
    }
}

/// A spinner and `label`, centred in `r`.
fn spinning_label(ui: &mut Ui, r: Rect, label: &str, ink: Color) {
    let lb = ui.line(label, Style::new(size::BODY, weight::MEDIUM, ink), Some((r.w - 46.0).max(20.0)));
    let x = r.x + ((r.w - 20.0 - lb.w) / 2.0).max(10.0);
    let mid = r.y + r.h / 2.0;
    ui.spinner(Point::new(x + 6.0, mid), 5.5, 2.0, ink);
    ui.draw(&lb, Point::new(x + 20.0, mid - lb.h / 2.0));
}

/// Waiting for the browser, whole: what to do, the code to check, until when, Open browser and Cancel.
fn pending(ui: &mut Ui, r: Rect, p: &jsh::Pending, act: impl Fn(usize) -> Option<Action>) {
    let t = ui.t;
    ui.logo(Logo::JustSuperhuman, Rect::new(r.x + 1.0, r.y + 1.0, KNOT_SMALL, KNOT_SMALL), t.text);
    ui.label_in(Rect::new(r.x + KNOT_SMALL + 10.0, r.y, r.w - KNOT_SMALL - 10.0, 20.0), "Approve bro in your browser", Style::new(size::BODY, weight::SEMIBOLD, t.text), Align::Left);
    ui.label_in(Rect::new(r.x, r.y + 26.0, r.w, 28.0), &p.user_code, Style::mono(20.0, t.text), Align::Left);
    let note = para(ui, &pending_note(p.expires_at), small(t.muted), r.w, 3);
    ui.draw(&note, Point::new(r.x, r.y + 58.0));
    let y = r.y + 58.0 + note.h + 12.0;
    let cw = key_button_width(ui, None, "Cancel", None);
    key_button(ui, Rect::new(r.x, y, r.w - cw - 8.0, BUTTON_H), None, "Open browser", None, Kind::Secondary, act(OPEN));
    key_button(ui, Rect::new(r.right() - cw, y, cw, BUTTON_H), None, "Cancel", None, Kind::Ghost, act(CANCEL));
}

/// Waiting for the browser, one row: the code, Open browser when there's room, Cancel.
fn pending_row(ui: &mut Ui, r: Rect, code: &str, act: impl Fn(usize) -> Option<Action>) {
    let t = ui.t;
    let cw = key_button_width(ui, None, "Cancel", None);
    let ow = key_button_width(ui, None, "Open browser", None);
    let y = r.y + (r.h - BUTTON_H) / 2.0;
    key_button(ui, Rect::new(r.right() - cw, y, cw, BUTTON_H), None, "Cancel", None, Kind::Ghost, act(CANCEL));
    let with_open = r.right() - cw - 8.0 - ow - 10.0 - r.x > 170.0;
    if with_open {
        key_button(ui, Rect::new(r.right() - cw - 8.0 - ow, y, ow, BUTTON_H), None, "Open browser", None, Kind::Secondary, act(OPEN));
    }
    let end = if with_open { r.right() - cw - 8.0 - ow - 10.0 } else { r.right() - cw - 10.0 };
    ui.logo(Logo::JustSuperhuman, Rect::new(r.x + 1.0, r.y + (r.h - KNOT_SMALL) / 2.0, KNOT_SMALL, KNOT_SMALL), t.text);
    let tx = r.x + KNOT_SMALL + 12.0;
    let code_b = ui.line(code, Style::mono(size::TITLE, t.text), None);
    let lead = ui.line("Approve in your browser", Style::new(size::BODY, weight::MEDIUM, t.muted), Some((end - tx - code_b.w - 10.0).max(0.0)));
    let mid = r.y + r.h / 2.0;
    let mut x = tx;
    if lead.w > 30.0 {
        ui.draw(&lead, Point::new(x, mid - lead.h / 2.0));
        x += lead.w + 10.0;
    }
    ui.draw(&code_b, Point::new(x, mid - code_b.h / 2.0));
}

/// A click on the account. The requests run on their own threads; the account follows the shared sign-in, which
/// wakes the window when it changes.
pub(super) fn click(n: usize, cx: &mut Cx) {
    use crate::alerts::Kind as Toast;
    let tx = cx.tx.clone();
    let say = move |kind: Toast, text: String| {
        let _ = tx.send(crate::pane::Event::Toast(kind, text));
    };
    if cx.svc.is_demo() {
        say(Toast::Info, "The demo doesn't sign in to anything.".into());
        return;
    }
    match n {
        SIGN_IN | OPEN => {
            if BUSY.swap(true, Ordering::AcqRel) {
                return;
            }
            std::thread::spawn(move || {
                let started = jsh::login().begin();
                BUSY.store(false, Ordering::Release);
                match started {
                    Ok((p, fresh)) => {
                        if (fresh || n == OPEN)
                            && let Err(e) = crate::panes::links::open(&p.verification_uri_complete)
                        {
                            say(Toast::Error, format!("Couldn't open your browser ({e}). Go to {} and enter {}.", p.verification_uri, p.user_code));
                        }
                    }
                    Err(e) => say(Toast::Error, format!("{e:#}")),
                }
            });
        }
        CANCEL => jsh::login().cancel(),
        SIGN_OUT => {
            if BUSY.swap(true, Ordering::AcqRel) {
                return;
            }
            std::thread::spawn(move || {
                let out = jsh::login().sign_out();
                BUSY.store(false, Ordering::Release);
                if out.revoked {
                    say(Toast::Info, "Signed out of Just Superhuman. This computer can't spend your credits any more.".into());
                } else {
                    say(Toast::Error, "Signed out here, but Just Superhuman couldn't be reached to revoke this computer. Revoke it under Account › Devices on console.justsuperhuman.com.".into());
                }
            });
        }
        _ => {}
    }
}
