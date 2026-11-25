//! The Computers view in the desktop app: the account's computers down the left (a dot for whether each can
//! run sessions now, and a few words on where it stands), the picked one on the right. A computer that can't be
//! reached shows each link between here and it with how that link came out and what fixes the first that fails;
//! one that can shows its sessions, the ones that need someone first. Keys (j/k, enter, d, c, r) reach
//! [`super::ComputersView`] as in the terminal UI.

use super::{ComputersView, SHOW_AFTER, as_of, row_status, span, summary};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::keymap::Act;
use crate::pane::Cx;
use crate::services::computers::{Computer, ComputersState, RemoteSession, Sessions};
use crate::views::usage::gui::{card, hints, icon_text_button, icon_text_button_width, title};
use bro_core::justconnect::health::{self, Check, Diagnosis, Hop};
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use crossterm::event::KeyModifiers;

/// The view's background (wheel, focus).
const BODY: usize = 0;
/// Look at the account again.
const REFRESH: usize = 1;
/// Run `bro ctl` on the picked computer (or back on this one).
const CTL: usize = 2;
/// Open the picked computer's desktop.
const DESKTOP: usize = 3;
/// Copy the picked computer's ID.
const COPY: usize = 4;
/// The picked computer's side (wheel).
const DETAIL: usize = 5;
/// Open Profiles to sign in.
const SIGN_IN: usize = 6;
/// Open Settings to add this computer.
const SETTINGS: usize = 7;
/// Install or update Bro on the picked computer.
const INSTALL: usize = 8;
/// `ROW + i`: pick computer `i`.
const ROW: usize = 100;

const PAD: f32 = 28.0;
const MAX_W: f32 = 1180.0;
const LIST_W: f32 = 330.0;
const ROW_H: f32 = 56.0;

const REFRESH_ICON: &str = "\u{F0450}";
const CHECK: &str = "\u{F012C}";
const CROSS: &str = "\u{F0156}";
const ALERT: &str = "\u{F0026}";
const COPY_ICON: &str = "\u{F018F}";
const DESKTOP_ICON: &str = "\u{F0989}";
const CTL_ICON: &str = "\u{F048B}";
const ACCOUNT_ICON: &str = "\u{F0004}";
const INSTALL_ICON: &str = "\u{F0463}";

pub(crate) fn draw(v: &mut ComputersView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    v.keep_fresh(cx);
    let t = ui.t;
    // what this frame shows, copied out (drawing never holds the services' lock)
    let c = cx.svc.state().computers.clone();
    ui.hit_with(area, Action::View(cx.id, BODY), Cursor::Default);
    let page = Rect::new(area.x + PAD, area.y, (area.w - 2.0 * PAD).clamp(0.0, MAX_W), area.h);

    // title, how old the list is, and "Look again"
    let head = Rect::new(page.x, area.y + 18.0, page.w, 36.0);
    let bw = icon_text_button_width(ui, "Look again");
    let br = Rect::new(head.right() - bw, head.y + 2.0, bw, 32.0);
    icon_text_button(ui, br, REFRESH_ICON, "Look again", widgets::Kind::Secondary, Action::View(cx.id, REFRESH));
    // a look that comes straight back is never announced
    if c.loading.is_some_and(|since| since.elapsed() >= SHOW_AFTER) {
        ui.spinner(Point::new(br.x - 20.0, head.y + head.h / 2.0), 7.0, 2.0, t.accent);
    }
    // with nothing to list, the card below says why: the title stands alone
    let listed = !c.list.is_empty();
    let meta = if c.checked && !listed && c.failure.is_none() { String::new() } else { summary(&c) };
    title(ui, Rect { w: (br.x - 44.0 - head.x).max(1.0), ..head }, "Computers", &[(meta, Style::new(size::SMALL, weight::REGULAR, t.muted))]);

    let foot_y = area.bottom() - 30.0;
    let picked = c.list.get(v.index(&c.list)).cloned();
    let on_ctl = picked.as_ref().is_some_and(|x| c.ctl.as_deref() == Some(x.id.as_str()));
    // only the keys that do something here
    if listed {
        hints(ui, Point::new(page.x, foot_y), &[("j/k", "pick"), ("enter", if on_ctl { "bro ctl back here" } else { "run bro ctl there" }), ("d", "desktop"), ("c", "copy id"), ("r", "look again")]);
    } else {
        hints(ui, Point::new(page.x, foot_y), &[("r", "look again")]);
    }

    let mut top = head.bottom() + 18.0;
    // the account couldn't be reached: said above whatever was last known, which stays
    if let Some(d) = &c.failure {
        top += notice(ui, Rect::new(page.x, top, page.w, 0.0), d) + 14.0;
    }
    let body = Rect::new(page.x, top, page.w, (foot_y - 14.0 - top).max(0.0));
    if c.checked && c.list.is_empty() {
        if c.failure.is_none() {
            empty(ui, body, &c, cx);
        }
        return true;
    }
    if !c.checked {
        return true;
    }

    // the list beside the picked computer; in a narrow pane, above it
    let beside = body.w >= 760.0;
    let list_h = c.list.len() as f32 * (ROW_H + 4.0);
    let (list, detail) = if beside {
        (Rect::new(body.x, body.y, LIST_W, body.h), Rect::new(body.x + LIST_W + 24.0, body.y, body.w - LIST_W - 24.0, body.h))
    } else {
        let h = list_h.min(body.h * 0.45);
        (Rect::new(body.x, body.y, body.w, h), Rect::new(body.x, body.y + h + 16.0, body.w, (body.h - h - 16.0).max(0.0)))
    };
    ui.clip(list, 0.0);
    let sel = v.index(&c.list);
    for (i, x) in c.list.iter().enumerate() {
        let r = Rect::new(list.x, list.y + i as f32 * (ROW_H + 4.0), list.w, ROW_H);
        if r.y > list.bottom() {
            break;
        }
        computer_row(ui, r, i, x, &c, i == sel, cx);
    }
    ui.unclip();
    if let Some(x) = &picked
        && detail.h > 80.0
    {
        detail_card(v, ui, detail, x, &c, cx);
    }
    true
}

/// One computer in the list: whether it can run sessions (a dot), its name, and where it stands in a few words.
fn computer_row(ui: &mut Ui, r: Rect, i: usize, x: &Computer, c: &ComputersState, on: bool, cx: &Cx) {
    let t = ui.t;
    widgets::row(ui, r, ("computer", cx.id, i), on, Action::View(cx.id, ROW + i));
    let ready = x.health.ready();
    let dot = Point::new(r.x + 18.0, r.y + 20.0);
    if ready {
        ui.circle(dot, 4.5, t.good);
    } else {
        ui.ring(dot, 4.5, 1.5, t.faint);
    }
    let x0 = r.x + 34.0;
    let room = (r.right() - x0 - 12.0).max(1.0);
    let name = Style::new(size::TITLE, if on { weight::SEMIBOLD } else { weight::MEDIUM }, t.text);
    ui.label_in(Rect::new(x0, r.y + 9.0, room, 20.0), &x.name, name, Align::Left);
    let mut status = format!("{} · {}", x.system, row_status(x, c));
    if c.ctl.as_deref() == Some(x.id.as_str()) {
        status.push_str(" · bro ctl runs here");
    }
    let quiet = Style::new(size::SMALL, weight::REGULAR, if ready { t.muted } else { t.muted.mix(t.faint, 0.3) });
    ui.label_in(Rect::new(x0, r.y + 30.0, room, 18.0), &status, quiet, Align::Left);
}

/// The account couldn't be reached: what happened and what to do, on a card of its own. Returns its height.
fn notice(ui: &mut Ui, at: Rect, d: &Diagnosis) -> f32 {
    let t = ui.t;
    let text_w = (at.w - 64.0).max(1.0);
    let head = ui.para(&d.headline, Style::new(size::BODY, weight::SEMIBOLD, t.text), text_w, Some(2));
    let step = d.steps.first().map(|s| ui.para(s, Style::new(size::SMALL, weight::REGULAR, t.muted), text_w, Some(3)));
    let h = 14.0 + head.h + step.as_ref().map_or(0.0, |s| 4.0 + s.h) + 14.0;
    let r = Rect::new(at.x, at.y, at.w, h);
    card(ui, r, 0.0, false);
    ui.icon(Rect::new(r.x + 16.0, r.y + 12.0, 20.0, 20.0), ALERT, size::HEADING, t.danger);
    ui.draw(&head, Point::new(r.x + 48.0, r.y + 14.0));
    if let Some(step) = &step {
        ui.draw(step, Point::new(r.x + 48.0, r.y + 14.0 + head.h + 4.0));
    }
    h
}

/// Nothing to list: signed out, or no computer added yet. What it is for, and the one thing to do next.
fn empty(ui: &mut Ui, body: Rect, c: &ComputersState, cx: &Cx) {
    let t = ui.t;
    let (icon, head, text, label, target) = if c.signed_in {
        (
            DESKTOP_ICON,
            "Add a computer to your account",
            "Every computer you add shows up here with its sessions, and you can start work on it from any other. Turn on \"Reach this computer from your account\" under Remote Control in Settings, here and on each computer you want to reach. Each one asks for administrator permission once.",
            "Open Settings",
            SETTINGS,
        )
    } else {
        (
            ACCOUNT_ICON,
            "Sign in to see your computers",
            "Your Just Superhuman account keeps the list of your computers. Sign in and the ones you've added appear here, with the sessions running on them, wherever they are.",
            "Sign in from Profiles",
            SIGN_IN,
        )
    };
    let w = body.w.min(620.0);
    let text_w = w - 104.0;
    let hb = ui.line(head, Style::new(size::HEADING, weight::SEMIBOLD, t.text), Some(text_w));
    let pb = ui.para(text, Style::new(size::BODY, weight::REGULAR, t.muted), text_w, Some(6));
    let h = 24.0 + hb.h + 8.0 + pb.h + 16.0 + 32.0 + 24.0;
    let r = Rect::new(body.x, body.y, w, h);
    card(ui, r, 0.0, false);
    ui.icon(Rect::new(r.x + 24.0, r.y + 24.0, 40.0, 40.0), icon, 30.0, t.faint);
    let x = r.x + 80.0;
    ui.draw(&hb, Point::new(x, r.y + 24.0));
    ui.draw(&pb, Point::new(x, r.y + 24.0 + hb.h + 8.0));
    let bw = icon_text_button_width(ui, label);
    icon_text_button(ui, Rect::new(x, r.y + 24.0 + hb.h + 8.0 + pb.h + 16.0, bw, 32.0), icon, label, widgets::Kind::Primary, Action::View(cx.id, target));
}

/// The picked computer: its name and what can be done with it, then why it can't be reached or what runs there.
fn detail_card(v: &mut ComputersView, ui: &mut Ui, r: Rect, x: &Computer, c: &ComputersState, cx: &Cx) {
    let t = ui.t;
    card(ui, r, 0.0, false);
    ui.hit_with(r, Action::View(cx.id, DETAIL), Cursor::Default);
    let inner = Rect::new(r.x + 22.0, r.y + 18.0, r.w - 44.0, r.h - 36.0);

    // name, with the actions at the end of its line
    let mut right = inner.right();
    let mut action = |ui: &mut Ui, glyph: &str, label: &str, kind: widgets::Kind, target: usize| {
        let w = icon_text_button_width(ui, label);
        right -= w;
        icon_text_button(ui, Rect::new(right, inner.y, w, 32.0), glyph, label, kind, Action::View(cx.id, target));
        right -= 8.0;
    };
    let on_ctl = c.ctl.as_deref() == Some(x.id.as_str());
    let busy = c.busy.as_ref().filter(|(id, _, _)| id == &x.id || id.is_empty() && x.here);
    // only what can be done with this computer right now
    if x.desktop_reachable() {
        action(ui, DESKTOP_ICON, "Open desktop", widgets::Kind::Secondary, DESKTOP);
    }
    if let Some(label) = x.bro_install() {
        action(ui, INSTALL_ICON, label, widgets::Kind::Secondary, INSTALL);
    }
    if on_ctl || x.here && c.ctl.is_some() {
        action(ui, CTL_ICON, "Run bro ctl here again", widgets::Kind::Secondary, CTL);
    } else if x.runs_ctl() {
        action(ui, CTL_ICON, "Run bro ctl there", widgets::Kind::Primary, CTL);
    }
    let copy = Rect::new(right - 32.0, inner.y, 32.0, 32.0);
    if widgets::icon_button(ui, copy, COPY_ICON, t.muted, Action::View(cx.id, COPY)) {
        widgets::tooltip(ui, copy, "copy this computer's ID");
    }
    right = copy.x - 12.0;
    if let Some((_, _, since)) = busy
        && since.elapsed() >= SHOW_AFTER
    {
        ui.spinner(Point::new(right - 10.0, inner.y + 16.0), 7.0, 2.0, t.accent);
        right -= 28.0;
    }
    let name_w = (right - inner.x).max(1.0);
    ui.label_in(Rect::new(inner.x, inner.y - 1.0, name_w, 22.0), &x.name, Style::new(size::HEADING, weight::SEMIBOLD, t.text), Align::Left);
    let mut about = x.system.clone();
    if x.here {
        about.push_str(" · this computer");
    }
    if let Some(version) = &x.bro_version {
        about.push_str(&format!(" · Bro {version}"));
    }
    if on_ctl {
        about.push_str(" · bro ctl runs here");
    }
    ui.label_in(Rect::new(inner.x, inner.y + 20.0, name_w, 18.0), &about, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);

    // the rest scrolls
    let view = Rect::new(inner.x, inner.y + 52.0, inner.w, (inner.bottom() - inner.y - 52.0).max(0.0));
    ui.clip(view, 0.0);
    let mut y = view.y - v.scroll;
    if !x.health.ready() {
        y = diagnosis(ui, view.x, y, view.w, &x.health);
    } else if x.here {
        let b = ui.para("Other computers on your account can start and follow sessions here while Bro is open. Pick one of them to see what runs there.", Style::new(size::BODY, weight::REGULAR, t.muted), view.w.min(640.0), Some(4));
        ui.draw(&b, Point::new(view.x, y));
        y += b.h + 16.0;
    } else {
        match c.sessions.get(&x.id) {
            Some(Sessions::Ready { list, took, at, .. }) => {
                let meta = format!("Answers in {} · read {}", span(*took), as_of(*at));
                widgets::section(ui, Rect::new(view.x, y, view.w, 20.0), "Sessions");
                let mb = ui.line(&meta, Style::new(size::SMALL, weight::REGULAR, t.faint), None);
                ui.draw(&mb, Point::new(view.right() - mb.w, y + 2.0));
                y += 30.0;
                if list.is_empty() {
                    let b = ui.para("Nothing is open there. Choose \"Run bro ctl there\", then `bro ctl open_session` starts one on it from this computer.", Style::new(size::BODY, weight::REGULAR, t.muted), view.w.min(640.0), Some(3));
                    ui.draw(&b, Point::new(view.x, y));
                    y += b.h + 16.0;
                }
                for (i, s) in list.iter().enumerate() {
                    y = session_row(ui, Rect::new(view.x, y, view.w, 50.0), (cx.id, i), s);
                }
            }
            Some(Sessions::Failed(d, _)) => y = diagnosis(ui, view.x, y, view.w, d),
            Some(Sessions::Loading(since)) if since.elapsed() >= SHOW_AFTER => {
                ui.spinner(Point::new(view.x + 8.0, y + 10.0), 7.0, 2.0, t.accent);
                ui.label_in(Rect::new(view.x + 26.0, y, view.w - 26.0, 20.0), "Reading its sessions", Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
                y += 30.0;
            }
            _ => {}
        }
    }
    for note in &x.health.notes {
        y += 6.0;
        let b = ui.para(note, Style::new(size::SMALL, weight::REGULAR, t.muted), view.w.min(680.0), Some(4));
        ui.draw(&b, Point::new(view.x, y));
        y += b.h + 8.0;
    }
    ui.unclip();
    // how far there is to scroll
    let content = y + v.scroll - view.y;
    v.scroll = v.scroll.clamp(0.0, (content - view.h).max(0.0));
    if content > view.h {
        widgets::scrollbar(ui, Rect::new(r.right() - 10.0, view.y, 6.0, view.h), content, view.h, v.scroll, 1.0);
    }
}

/// A diagnosis drawn at (`x`, `y`): the headline, each link with how it came out, then what to do. Returns the
/// y after it.
fn diagnosis(ui: &mut Ui, x: f32, mut y: f32, w: f32, d: &Diagnosis) -> f32 {
    let t = ui.t;
    let head = ui.para(&d.headline, Style::new(size::TITLE, weight::SEMIBOLD, t.text), w, Some(2));
    ui.draw(&head, Point::new(x, y));
    y += head.h + 14.0;
    let name = d.name.as_deref();
    let label_w = Hop::ALL.iter().map(|h| ui.measure(&h.label(name), Style::new(size::BODY, weight::MEDIUM, t.text))).fold(0.0, f32::max) + 24.0;
    for hop in Hop::ALL {
        let row = Rect::new(x, y, w, 26.0);
        let icon = Rect::new(row.x, row.y, 18.0, row.h);
        let (said, ink) = match d.check(hop) {
            Check::Passed => {
                ui.icon(icon, CHECK, size::TITLE, t.good);
                (health::passed(hop).to_string(), t.muted)
            }
            Check::Failed => {
                ui.icon(icon, CROSS, size::TITLE, t.danger);
                (d.detail.clone(), t.text)
            }
            Check::Unchecked => {
                ui.ring(Point::new(icon.x + 9.0, icon.y + 13.0), 3.5, 1.5, t.faint);
                ("not reached".to_string(), t.faint)
            }
        };
        let passed = d.check(hop) != Check::Unchecked;
        ui.label_in(Rect::new(row.x + 28.0, row.y, label_w, row.h), &hop.label(name), Style::new(size::BODY, weight::MEDIUM, if passed { t.text } else { t.muted }), Align::Left);
        ui.label_in(Rect::new(row.x + 28.0 + label_w, row.y, (w - 28.0 - label_w).max(1.0), row.h), &said, Style::new(size::BODY, weight::REGULAR, ink), Align::Left);
        y += 26.0;
    }
    if !d.steps.is_empty() {
        y += 16.0;
        widgets::section(ui, Rect::new(x, y, w, 20.0), "What to do");
        y += 30.0;
        for (i, step) in d.steps.iter().enumerate() {
            let n = ui.line(&format!("{}.", i + 1), Style::new(size::BODY, weight::SEMIBOLD, t.accent), None);
            ui.draw(&n, Point::new(x, y));
            let b = ui.para(step, Style::new(size::BODY, weight::REGULAR, t.text), (w - 26.0).min(660.0), Some(4));
            ui.draw(&b, Point::new(x + 26.0, y));
            y += b.h + 10.0;
        }
    }
    y + 6.0
}

/// One session on the picked computer: a mark for whether it needs someone, is working or is quiet, its name
/// with the agent and project, what it is doing, and when it last printed. Returns the y after it.
fn session_row(ui: &mut Ui, r: Rect, key: impl std::hash::Hash, s: &RemoteSession) -> f32 {
    let t = ui.t;
    let hot = ui.hover_amount(("remote-session", key), r);
    if hot > 0.0 {
        ui.round(r, radius::CONTROL, t.text.with_alpha(0.04 * hot));
    }
    let mark = Point::new(r.x + 12.0, r.y + 17.0);
    match s.activity.as_deref() {
        // waiting on someone: the accent, still
        Some("awaiting") => ui.circle(mark, 4.5, t.accent),
        Some("working") => ui.pulse(mark, 4.5, t.shine),
        _ => ui.ring(mark, 4.5, 1.5, t.faint),
    }
    let x0 = r.x + 30.0;
    let when = ui.line(&s.last_output, Style::new(size::SMALL, weight::REGULAR, t.faint), None);
    ui.draw(&when, Point::new(r.right() - when.w - 8.0, r.y + 8.0));
    let room = (r.right() - when.w - 24.0 - x0).max(1.0);
    let mut who = s.agent.clone().unwrap_or_else(|| "shell".into());
    if let Some(project) = &s.project {
        who.push_str(&format!(" · {project}"));
    }
    let name = ui.line(&s.title, Style::new(size::BODY, weight::SEMIBOLD, t.text), Some(room));
    ui.draw(&name, Point::new(x0, r.y + 7.0));
    ui.label_in(Rect::new(x0 + name.w + 10.0, r.y + 7.0, (room - name.w - 10.0).max(1.0), 18.0), &who, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
    let needs = s.activity.as_deref() == Some("awaiting");
    ui.label_in(Rect::new(x0, r.y + 27.0, (r.right() - x0 - 8.0).max(1.0), 18.0), &s.summary, Style::new(size::SMALL, weight::REGULAR, if needs { t.text } else { t.muted }), Align::Left);
    r.bottom() + 2.0
}

/// Target `n` was clicked.
pub(crate) fn click(v: &mut ComputersView, n: usize, _p: Point, _mods: KeyModifiers, count: u32, cx: &mut Cx) {
    match n {
        REFRESH => cx.svc.refresh_computers(),
        CTL => v.toggle_ctl(cx),
        DESKTOP => v.open_desktop(cx),
        COPY => v.copy_id(cx),
        INSTALL => v.install_bro(cx),
        SIGN_IN => cx.act(crate::pane::Action::Run(Act::Profiles)),
        SETTINGS => cx.act(crate::pane::Action::Run(Act::Settings)),
        n if (ROW..ROW + 1000).contains(&n) => {
            {
                let st = cx.svc.state();
                v.pick(&st.computers.list, n - ROW);
            }
            // a double click on another computer opens its desktop
            if count >= 2 {
                v.open_desktop(cx);
            }
        }
        _ => {}
    }
}

/// The wheel over target `n` (None: elsewhere in the view): the picked computer's side scrolls.
pub(crate) fn wheel(v: &mut ComputersView, _n: Option<usize>, down: bool, _cx: &mut Cx) {
    v.scroll = (v.scroll + if down { 48.0 } else { -48.0 }).max(0.0);
}
