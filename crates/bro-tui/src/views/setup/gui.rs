//! The setup view in the desktop app: a centred onboarding page with the three steps from a fresh machine to a
//! first agent as cards (install an agent, sign in, go). It draws [`SetupView`]'s own state and builds its
//! rows in the terminal UI's order (the keys and `activate` read them); a click selects a row and does it.

use super::{AGENTS, Row, SHOWN, SetupView, harness_of};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand;
use crate::app::gui::kit::widgets::Kind;
use crate::app::gui::kit::{Cursor, Style, Ui, size, weight, widgets};
use crate::pane::{Cx, Pane};
use crate::views::profiles::gui::{BUTTON_H, edge_fade, focus_ring, hints, key_button, key_button_width, page_card};
use bro_core::Harness;
use bro_core::agent_install;
use bro_core::profiles::{Profile, ProfileKind};
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

// targets
const BG: usize = 0;
const LOOK: usize = 1;
/// `ROW + i`: selectable row `i`
const ROW: usize = 100;

const PAGE_W: f32 = 760.0;
const PAD: f32 = 20.0;
/// a step's head (its number and title)
const STEP_H: f32 = 24.0;
const AGENT_H: f32 = 52.0;
const LOGIN_H: f32 = 36.0;
const MORE_H: f32 = 32.0;

const ROCKET: &str = "\u{F14DF}";
const CHECK: &str = "\u{F012C}";
const CROSS: &str = "\u{F0156}";
const PLUS: &str = "\u{F0415}";
const PLAY: &str = "\u{F040A}";
const REFRESH: &str = "\u{F0450}";
const CHEVRON: &str = "\u{F0142}";
const DOWNLOAD: &str = "\u{F01DA}";
/// not signed in: a quiet account outline
const LOGGED_OUT: &str = "\u{F0BE7}";

/// The rows, in the terminal UI's order: the agents, up to six logins (then "more"), the two adds, launch, done.
fn build_rows(profiles: Option<&Vec<Profile>>) -> Vec<Row> {
    let mut rows: Vec<Row> = AGENTS.iter().map(|h| Row::Agent(*h)).collect();
    if let Some(ps) = profiles {
        rows.extend(ps.iter().take(SHOWN).map(|p| Row::Login(p.id.clone())));
        if ps.len() > SHOWN {
            rows.push(Row::More);
        }
    }
    rows.extend([Row::Add(ProfileKind::ClaudeAccount), Row::Add(ProfileKind::CodexProfile), Row::Launch, Row::Done]);
    rows
}

/// How far along a step is.
#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Done,
    Now,
    Later,
}

/// Draw the view inside its card (`area`: the card's content).
pub(crate) fn draw(v: &mut SetupView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let t = ui.t;
    let id = cx.id;
    // what the terminal UI's render does besides drawing: the first look, and the rows its keys walk
    if v.checked_at.is_none() {
        v.check(cx);
    }
    // the window only ticks panes while nothing animates, and an install in another tab shows a spinner: keep
    // the watch going here too (the tick's own `poll`, timed by the view itself)
    if v.watching.is_some() {
        v.poll(cx);
    }
    let (profiles, why) = {
        let st = cx.svc.state();
        (st.profiles.ready().cloned(), st.profiles.why())
    };
    let rows = build_rows(profiles.as_ref());
    v.sel = v.sel.min(rows.len().saturating_sub(1));
    v.rows = rows.clone();
    let any_agent = AGENTS.iter().any(|h| v.installed(*h) == Some(true));
    let any_login = profiles.as_ref().is_some_and(|ps| ps.iter().any(|p| p.authenticated));
    let stages = match (any_agent, any_login) {
        (false, _) => [Stage::Now, if any_login { Stage::Done } else { Stage::Later }, Stage::Later],
        (true, false) => [Stage::Done, Stage::Now, Stage::Later],
        (true, true) => [Stage::Done, Stage::Done, Stage::Now],
    };
    ui.hit_with(area, Action::View(id, BG), Cursor::Default);

    let w = (area.w - if area.w < 480.0 { 24.0 } else { 48.0 }).clamp(0.0, PAGE_W);
    let x = area.x + (area.w - w) / 2.0;
    let view = Rect::new(area.x, area.y, area.w, (area.h - 40.0).max(0.0));
    // `scroll` holds the page's offset in pixels here (the terminal UI keeps lines in it)
    let shown = ui.ease(("setup-scroll", id), v.scroll as f32, 160.0);
    let top = view.y - shown;
    let inner_x = x + PAD;
    let inner_w = w - 2.0 * PAD;
    let mut sel_span: Option<(f32, f32)> = None;
    let row_index = |row: &Row| rows.iter().position(|r| r == row).unwrap_or(usize::MAX);
    ui.clip(view, 0.0);

    // ---- the page's head: a mark (on nothing), the title, where you are
    let mut y = 32.0;
    ui.icon(Rect::new(x + w / 2.0 - 24.0, top + y, 48.0, 48.0), ROCKET, 40.0, t.text.mix(t.muted, 0.15));
    y += 48.0 + 12.0;
    let title = ui.line("Set up bro", Style::new(size::DISPLAY, weight::SEMIBOLD, t.text), None);
    ui.draw(&title, Point::new(x + (w - title.w) / 2.0, top + y));
    y += title.h + 6.0;
    let (status, sc, glyph) = match (any_agent, any_login) {
        (true, true) => ("ready: launch an agent below", t.good, CHECK),
        (false, _) => ("start here: install an agent", t.inline, DOWNLOAD),
        (true, false) => ("next: sign in", t.inline, CHEVRON),
    };
    // the status as an icon in its colour and the words beside it: no badge
    let sst = Style::new(size::BODY, weight::MEDIUM, t.text.mix(t.muted, 0.25));
    let sw = ui.measure(status, sst) + 22.0;
    let sx = x + (w - sw) / 2.0;
    ui.icon(Rect::new(sx, top + y, 16.0, 20.0), glyph, size::TITLE, sc);
    ui.label_in(Rect::new(sx + 22.0, top + y, sw - 22.0, 20.0), status, sst, Align::Left);
    y += 20.0 + 28.0;

    // ---- 1 · install an agent
    let note1 = ui.para("bro runs coding agents in tabs, side by side. Install at least one.", Style::new(size::SMALL, weight::REGULAR, t.muted), inner_w, Some(4));
    let foot1 = ui.para("Pi, omp, Git, Node.js and more: quick install, in the palette.", Style::new(size::SMALL, weight::REGULAR, t.faint), inner_w, Some(4));
    let h1 = PAD + STEP_H + 10.0 + note1.h + 12.0 + AGENTS.len() as f32 * AGENT_H + 10.0 + foot1.h + PAD;
    let c1 = Rect::new(x, top + y, w, h1);
    page_card(ui, c1);
    let mut cy = c1.y + PAD;
    let look = if inner_w >= 420.0 { "Look again" } else { "" };
    let lw = key_button_width(ui, Some(REFRESH), look, Some("r"));
    let lr = Rect::new(c1.right() - PAD - lw + 6.0, cy + (STEP_H - BUTTON_H) / 2.0, lw, BUTTON_H);
    // just the mark: its name on hover
    if key_button(ui, lr, Some(REFRESH), look, Some("r"), Kind::Ghost, Some(Action::View(id, LOOK))) > 0.5 && look.is_empty() {
        widgets::tooltip(ui, lr, "Look again");
    }
    step_head(ui, Point::new(inner_x, cy), inner_w - lw - 8.0, 1, "Install an agent", stages[0]);
    cy += STEP_H + 10.0;
    ui.draw(&note1, Point::new(inner_x, cy));
    cy += note1.h + 12.0;
    for h in AGENTS {
        let i = row_index(&Row::Agent(h));
        let r = Rect::new(inner_x - 10.0, cy, inner_w + 20.0, AGENT_H);
        let on = v.sel == i;
        let hot = list_row(ui, r, ("setup-agent", id, h.label()), on, Action::View(id, ROW + i));
        agent_row(v, ui, r, h, on, hot, id, i);
        if on {
            sel_span = Some((r.y - top, r.bottom() - top));
        }
        cy += AGENT_H;
    }
    cy += 10.0;
    ui.draw(&foot1, Point::new(inner_x, cy));
    y += h1 + 16.0;

    // ---- 2 · sign in
    let explain = "A profile is one login bro launches agents with. This machine's own logins (~/.claude, ~/.codex) are used as they are. Add more Claude accounts or Codex profiles to see every meter in the sidebar and move a session to another login when one runs out.";
    let note2 = ui.para(explain, Style::new(size::SMALL, weight::REGULAR, t.muted), inner_w, Some(12));
    let foot2 = ui.para("Each new profile opens its agent's login in a tab; it shows up here once signed in.", Style::new(size::SMALL, weight::REGULAR, t.faint), inner_w, Some(4));
    let listed = match &profiles {
        None => 26.0,
        Some(ps) => ps.len().min(SHOWN) as f32 * LOGIN_H + if ps.len() > SHOWN { MORE_H } else { 0.0 },
    };
    let add_labels = ["Add a Claude account", "Add a Codex profile"];
    let add_w: Vec<f32> = add_labels.iter().map(|l| key_button_width(ui, Some(PLUS), l, None)).collect();
    let add_stacked = add_w[0] + add_w[1] + 10.0 > inner_w;
    let add_w: Vec<f32> = add_w.iter().map(|w| w.min(inner_w)).collect();
    let add_h = if add_stacked { BUTTON_H * 2.0 + 8.0 } else { BUTTON_H };
    let h2 = PAD + STEP_H + 10.0 + note2.h + 12.0 + listed + 14.0 + add_h + 14.0 + foot2.h + PAD;
    let c2 = Rect::new(x, top + y, w, h2);
    page_card(ui, c2);
    let mut cy = c2.y + PAD;
    step_head(ui, Point::new(inner_x, cy), inner_w, 2, "Sign in", stages[1]);
    cy += STEP_H + 10.0;
    ui.draw(&note2, Point::new(inner_x, cy));
    cy += note2.h + 12.0;
    match &profiles {
        None => {
            ui.label_in(Rect::new(inner_x, cy, inner_w, 26.0), &why, Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Left);
        }
        Some(ps) => {
            let mut ly = cy;
            for p in ps.iter().take(SHOWN) {
                let i = row_index(&Row::Login(p.id.clone()));
                let r = Rect::new(inner_x - 10.0, ly, inner_w + 20.0, LOGIN_H);
                let on = v.sel == i;
                list_row(ui, r, ("setup-login", id, &p.id), on, Action::View(id, ROW + i));
                login_row(v, ui, r, p);
                if on {
                    sel_span = Some((r.y - top, r.bottom() - top));
                }
                ly += LOGIN_H;
            }
            if ps.len() > SHOWN {
                let i = row_index(&Row::More);
                let r = Rect::new(inner_x - 10.0, ly, inner_w + 20.0, MORE_H);
                let on = v.sel == i;
                list_row(ui, r, ("setup-more", id), on, Action::View(id, ROW + i));
                let text = format!("{} more in profiles", ps.len() - SHOWN);
                let st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
                let b = ui.line(&text, st, None);
                ui.draw(&b, Point::new(r.x + LOGIN_TEXT_X, r.y + (r.h - b.h) / 2.0));
                ui.icon(Rect::new(r.x + LOGIN_TEXT_X + b.w + 4.0, r.y, 14.0, r.h), CHEVRON, size::SMALL, t.faint);
                if on {
                    sel_span = Some((r.y - top, r.bottom() - top));
                }
            }
        }
    }
    cy += listed + 14.0;
    let mut ax = inner_x;
    for (k, kind) in [ProfileKind::ClaudeAccount, ProfileKind::CodexProfile].into_iter().enumerate() {
        let i = row_index(&Row::Add(kind));
        let r = Rect::new(ax, cy, add_w[k], BUTTON_H);
        let on = v.sel == i;
        if on {
            focus_ring(ui, r);
            sel_span = Some((r.y - top, r.bottom() - top));
        }
        key_button(ui, r, Some(PLUS), add_labels[k], None, Kind::Secondary, Some(Action::View(id, ROW + i)));
        if add_stacked {
            cy += BUTTON_H + 8.0;
        } else {
            ax = r.right() + 10.0;
        }
    }
    cy = if add_stacked { cy - 8.0 } else { cy + BUTTON_H } + 14.0;
    ui.draw(&foot2, Point::new(inner_x, cy));
    y += h2 + 16.0;

    // ---- 3 · go
    let launch_w = key_button_width(ui, Some(PLAY), "Launch an agent", None) + 12.0;
    let done_label = "Done · don't open this at startup again";
    let done_w = key_button_width(ui, Some(CHECK), done_label, None);
    let go_stacked = launch_w + done_w + 12.0 > inner_w;
    let (launch_w, done_w) = (launch_w.min(inner_w), done_w.min(inner_w));
    let h3 = PAD + STEP_H + 16.0 + if go_stacked { BUTTON_H + 10.0 + BUTTON_H } else { BUTTON_H } + PAD;
    let c3 = Rect::new(x, top + y, w, h3);
    page_card(ui, c3);
    let mut cy = c3.y + PAD;
    step_head(ui, Point::new(inner_x, cy), inner_w, 3, "Go", stages[2]);
    cy += STEP_H + 16.0;
    let li = row_index(&Row::Launch);
    let launch = Rect::new(inner_x, cy, launch_w, BUTTON_H);
    let di = row_index(&Row::Done);
    let done = if go_stacked { Rect::new(inner_x, cy + BUTTON_H + 10.0, done_w, BUTTON_H) } else { Rect::new(launch.right() + 12.0, cy, done_w, BUTTON_H) };
    for (i, r) in [(li, launch), (di, done)] {
        if v.sel == i {
            focus_ring(ui, r);
            sel_span = Some((r.y - top, r.bottom() - top));
        }
    }
    key_button(ui, launch, Some(PLAY), "Launch an agent", None, Kind::Primary, Some(Action::View(id, ROW + li)));
    key_button(ui, done, Some(CHECK), done_label, None, Kind::Ghost, Some(Action::View(id, ROW + di)));
    y += h3 + 28.0;
    // the page's cut edges fade out
    if shown > 0.5 {
        edge_fade(ui, Rect::new(view.x, view.y, view.w, 18.0), t.surface, true);
    }
    if shown + view.h < y - 0.5 {
        edge_fade(ui, Rect::new(view.x, view.bottom() - 28.0, view.w, 28.0), t.surface, false);
    }
    ui.unclip();

    // ---- keep the selection in view (the first row brings the head back)
    let content_h = y;
    let mut target = v.scroll as f32;
    if v.sel == 0 {
        target = 0.0;
    } else if let Some((a, b)) = sel_span {
        // clear of the faded edges
        if a - 22.0 < target {
            target = a - 22.0;
        } else if b + 34.0 > target + view.h {
            target = b + 34.0 - view.h;
        }
    }
    let target = target.clamp(0.0, (content_h - view.h).max(0.0)).round();
    if target != v.scroll as f32 {
        v.scroll = target as usize;
        ui.ticking = true;
    }
    let fade = ui.ease(("setup-bar", id), if ui.hovered(view) { 1.0 } else { 0.3 }, 200.0);
    widgets::scrollbar(ui, view, content_h, view.h, shown, fade);

    // ---- the keys, quietly
    hints(ui, Point::new(x, area.bottom() - 28.0), w, &[("j/k", "move"), ("enter", "do it"), ("r", "look again")]);
    true
}

/// A row in a step: the whole of it clicks; its hover and selection fill stops a pixel short of its neighbours,
/// as WinUI's list items. Returns the hover amount.
fn list_row(ui: &mut Ui, r: Rect, key: impl std::hash::Hash, on: bool, action: Action) -> f32 {
    ui.hit(r, action.clone());
    widgets::row(ui, Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0), key, on, action)
}

/// A step's number in a circle (a check once it's done) and its title, in `w`. Nothing is filled: the circle is
/// an outline, the current step's in the text colour, a done one's in green.
fn step_head(ui: &mut Ui, at: Point, w: f32, n: usize, title: &str, stage: Stage) {
    let t = ui.t;
    let badge = Rect::new(at.x, at.y + (STEP_H - 24.0) / 2.0, 24.0, 24.0);
    let c = badge.center();
    let nst = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    match stage {
        Stage::Done => {
            ui.ring(c, 12.0, 1.0, t.good);
            ui.icon(badge, CHECK, size::TITLE, t.good);
        }
        Stage::Now => {
            ui.ring(c, 12.0, 1.0, t.text.mix(t.muted, 0.2));
            ui.label_in(badge, &n.to_string(), nst, Align::Center);
        }
        Stage::Later => {
            ui.ring(c, 12.0, 1.0, t.border);
            ui.label_in(badge, &n.to_string(), nst.color(t.muted), Align::Center);
        }
    }
    let st = Style::new(size::HEADING, weight::SEMIBOLD, if stage == Stage::Later { t.text.mix(t.muted, 0.35) } else { t.text });
    ui.label_in(Rect::new(at.x + 36.0, at.y, (w - 36.0).max(20.0), STEP_H), title, st, Align::Left);
}

/// An agent: its logo, name and who it's from; then found (where), looking, installing, or an install button.
#[allow(clippy::too_many_arguments)]
fn agent_row(v: &SetupView, ui: &mut Ui, r: Rect, h: Harness, on: bool, hot: f32, id: u64, i: usize) {
    let t = ui.t;
    let logo = Rect::new(r.x + 10.0, r.y + (r.h - 24.0) / 2.0, 24.0, 24.0);
    ui.mark(brand::harness(h), logo, 24.0, None);
    let x = logo.right() + 14.0;
    let name = ui.line(agent_install::display_name(h), Style::new(size::BODY, weight::SEMIBOLD, t.text), None);
    let blurb = if h == Harness::Claude { "Anthropic · needs a Claude subscription or API key" } else { "OpenAI · needs a ChatGPT plan or API key" };
    let ny = r.y + (r.h - name.h - 18.0) / 2.0;
    ui.draw(&name, Point::new(x, ny));
    // right: what bro found
    let mid = r.y + r.h / 2.0;
    let right = r.right() - 12.0;
    let left_edge = match v.installed(h) {
        None => status(ui, right, mid, None, "looking…", t.faint, true),
        Some(true) => {
            let at = v.found.as_ref().and_then(|f| f.iter().find(|(x, _)| *x == h)).and_then(|(_, p)| p.clone()).unwrap_or_default();
            let path = crate::util::short_path(&at, 44);
            let st = Style::mono(size::SMALL, t.muted);
            let b = ui.line(&path, st, Some((r.w * 0.42).min(260.0)));
            ui.draw(&b, Point::new(right - b.w, mid - b.h / 2.0));
            ui.icon(Rect::new(right - b.w - 22.0, mid - 9.0, 16.0, 18.0), CHECK, size::TITLE, t.good);
            right - b.w - 22.0
        }
        Some(false) if v.watching.is_some() => status(ui, right, mid, Some(t.shine), "installing in another tab…", t.shine, true),
        Some(false) => {
            let bw = key_button_width(ui, Some(DOWNLOAD), "Install", None);
            let br = Rect::new(right - bw, mid - BUTTON_H / 2.0, bw, BUTTON_H);
            key_button(ui, br, Some(DOWNLOAD), "Install", None, if on || hot > 0.5 { Kind::Primary } else { Kind::Secondary }, Some(Action::View(id, ROW + i)));
            status(ui, br.x - 14.0, mid, None, "not installed", t.danger, false)
        }
    };
    ui.label_in(Rect::new(x, ny + name.h + 2.0, (left_edge - 16.0 - x).max(20.0), 18.0), blurb, Style::new(size::SMALL, weight::REGULAR, t.muted), Align::Left);
}

/// A status at the right of a row, with a spinner while `spinning` (in `spin`, else its own colour) or a ✗:
/// returns where it starts.
fn status(ui: &mut Ui, right: f32, mid: f32, spin: Option<Color>, text: &str, c: Color, spinning: bool) -> f32 {
    let st = Style::new(size::SMALL, weight::MEDIUM, c);
    let b = ui.line(text, st, None);
    ui.draw(&b, Point::new(right - b.w, mid - b.h / 2.0));
    let mark_x = right - b.w - 14.0;
    if spinning {
        ui.spinner(Point::new(mark_x, mid), 4.6, 1.6, spin.unwrap_or(c));
    } else {
        ui.icon(Rect::new(mark_x - 7.0, mid - 9.0, 14.0, 18.0), CROSS, size::SMALL, c);
    }
    mark_x - 8.0
}

/// Where a login row's id starts (and "more in profiles" lines up with it).
const LOGIN_TEXT_X: f32 = 58.0;

/// A login: signed in (a check) or not (a quiet outline), its agent's logo, its id, its email or what to do,
/// "machine login".
fn login_row(v: &SetupView, ui: &mut Ui, r: Rect, p: &Profile) {
    let t = ui.t;
    let h = harness_of(p);
    let mid = r.y + r.h / 2.0;
    let (glyph, gc) = if p.authenticated { (CHECK, t.good) } else { (LOGGED_OUT, t.muted) };
    ui.icon(Rect::new(r.x + 10.0, mid - 9.0, 16.0, 18.0), glyph, size::TITLE, gc);
    ui.mark(brand::harness(h), Rect::new(r.x + 34.0, mid - 8.0, 16.0, 16.0), 16.0, None);
    let id_st = Style::new(size::BODY, weight::MEDIUM, t.text);
    let id_w = ((r.w - LOGIN_TEXT_X) * 0.4).min(176.0);
    ui.label_in(Rect::new(r.x + LOGIN_TEXT_X, r.y, id_w, r.h), &p.id, id_st, Align::Left);
    let detail_x = r.x + LOGIN_TEXT_X + id_w + 10.0;
    let detail = match () {
        _ if p.authenticated => p.email.clone().unwrap_or_else(|| "signed in".into()),
        _ if v.installed(h) == Some(false) => format!("needs {} first · enter installs it", agent_install::display_name(h)),
        _ => "not signed in · enter opens its login".to_string(),
    };
    let mut right = r.right() - 12.0;
    if p.is_local() && r.w >= 360.0 {
        let pw = widgets::pill_width(ui, "machine login");
        widgets::pill(ui, Point::new(right - pw, mid - 8.0), 16.0, "machine login", t.muted, t.text.with_alpha(if t.light { 0.07 } else { 0.08 }));
        right -= pw + 10.0;
    }
    let dst = Style::new(size::SMALL, weight::REGULAR, if p.authenticated { t.text.mix(t.muted, 0.35) } else { t.muted });
    if right - detail_x >= 40.0 {
        ui.label_in(Rect::new(detail_x, r.y, right - detail_x, r.h), &detail, dst, Align::Left);
    }
}

/// Target `n` was clicked: a row selects and does what Enter does there; "look again" sends r.
pub(crate) fn click(v: &mut SetupView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32, cx: &mut Cx) {
    match n {
        LOOK => {
            v.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE), cx);
        }
        n if n >= ROW && n - ROW < v.rows.len() => {
            v.sel = n - ROW;
            v.activate(cx);
        }
        _ => {}
    }
}

/// The wheel moves the selection, as in the terminal UI.
pub(crate) fn wheel(v: &mut SetupView, _n: Option<usize>, down: bool, cx: &mut Cx) {
    let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
    v.mouse(MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE }, ratatui::layout::Rect::default(), cx);
}
