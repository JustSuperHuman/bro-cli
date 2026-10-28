//! Discover in the desktop app: the repos bro features (Theo's tstack) as cards over Skill Issue's catalogue, with a
//! search field, its ranking / most clicked / A–Z as a segmented choice, an installed-only switch and the domain tabs
//! as chips with their counts; then the list, each skill with where it stands, its numbers, its install (or enable)
//! button and its ⋯, beside the selected one's details (under the list in a narrow pane). Install asks where to:
//! everywhere, or one of the sidebar's projects. A double-click installs; a right-click opens the skill's menu.

use super::super::{SkillsView, State, clicks_label, open_link, state_of};
use super::{ACCOUNT, ALERT, ARCHIVE, CHECK, CROSS, GITHUB, INFO, MORE, OPEN, PLUS, REFRESH_ICON, RESTORE, SEARCH_ICON, STAR};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::widgets::{self, Kind};
use crate::app::gui::kit::{Style, Ui, radius, size, weight};
use crate::pane::{Cx, Pane, PaneId};
use crate::views::profiles::gui::{BUTTON_H, TITLE, edge_fade, focus_ring, hints, key_button, key_button_width, page_card};
use bro_core::skill_market::{self, DOMAINS, FEATURED, MarketSkill, Sort};
use bro_core::skill_repos;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// targets
pub(crate) const SEARCH: usize = 20;
pub(crate) const CLEAR: usize = 21;
const REFRESH: usize = 22;
pub(crate) const MINE: usize = 23;
const SITE: usize = 24;
const PAGE: usize = 25;
const SOURCE: usize = 26;
const INSTALL_SEL: usize = 27;
const REMOVE_SEL: usize = 28;
/// `SORT + i`: [`Sort::ALL`]`[i]`
pub(crate) const SORT: usize = 30;
/// `DOMAIN`: all; `DOMAIN + 1 + d`: [`DOMAINS`]`[d]`
pub(crate) const DOMAIN: usize = 40;
/// `FEATURE + i`: watch featured repo `i` (or show it); `FEATURE_WEB + i`: its page
const FEATURE: usize = 60;
const FEATURE_WEB: usize = 70;
/// `ROW + i`: list row `i` (select; double-click: install); `INSTALL + i` its button; `MORE_ROW + i` its ⋯
pub(crate) const ROW: usize = 100_000;
const INSTALL: usize = 200_000;
const REMOVE: usize = 300_000;
const MORE_ROW: usize = 400_000;

/// The row a target belongs to (a right-click on it opens that skill's menu).
pub(super) fn row_of(n: usize) -> Option<usize> {
    match n {
        n if n >= MORE_ROW + 100_000 => None,
        n if n >= MORE_ROW => Some(n - MORE_ROW),
        n if n >= REMOVE => Some(n - REMOVE),
        n if n >= INSTALL => Some(n - INSTALL),
        n if n >= ROW => Some(n - ROW),
        _ => None,
    }
}

const ROW_H: f32 = 58.0;
const CHIP_H: f32 = 28.0;
/// the details beside the list, when the page is wide enough for both
const SIDE_W: f32 = 320.0;
const SIDE_MIN: f32 = 800.0;
/// the details under the list otherwise
const UNDER_H: f32 = 132.0;

const DOWNLOAD: &str = "\u{F01DA}";
const CLICK: &str = "\u{F0CFD}";

/// Draw Discover in `col`.
pub(super) fn draw(v: &mut SkillsView, ui: &mut Ui, col: Rect, cx: &mut Cx) {
    let t = ui.t;
    let id = cx.id;
    let (title, sub) = v.heading(cx);

    // ---- title, the site and a refresh beside it, what it is under them
    let tb = ui.line(&title, Style::new(TITLE, weight::SEMIBOLD, t.text), None);
    ui.draw(&tb, Point::new(col.x, col.y));
    let site_w = key_button_width(ui, Some(OPEN), "skill-issue.in", None);
    let refresh = Rect::new(col.right() - BUTTON_H, col.y - 4.0, BUTTON_H, BUTTON_H);
    if v.fetching {
        ui.spinner(refresh.center(), 6.0, 1.7, t.muted);
    } else if widgets::icon_button(ui, refresh, REFRESH_ICON, t.muted, Action::View(id, REFRESH)) {
        widgets::tooltip(ui, refresh, "Fetch the catalogue again");
    }
    key_button(ui, Rect::new(refresh.x - 6.0 - site_w, col.y - 4.0, site_w, BUTTON_H), Some(OPEN), "skill-issue.in", None, Kind::Ghost, Some(Action::View(id, SITE)));
    let sb = ui.para(&sub, Style::new(size::BODY, weight::REGULAR, t.muted), col.w, Some(2));
    let mut y = col.y + tb.h.max(BUTTON_H - 6.0) + 4.0;
    ui.draw(&sb, Point::new(col.x, y));
    y += sb.h + 16.0;

    // ---- the repos bro features, while there's no search
    if v.market.query.trim().is_empty() {
        y = featured(ui, Rect::new(col.x, y, col.w, 0.0), cx) + 16.0;
    }

    // ---- the search, the sort, installed only (the last two under the search when narrow)
    let sorts = ["Ranked", "Most clicked", "A–Z"];
    let sort_w = 300.0f32;
    let mine_label = "Installed only";
    let mine_w = key_button_width(ui, Some(CHECK), mine_label, Some("f"));
    let one_line = col.w >= sort_w + mine_w + 24.0 + 260.0;
    let field_w = if one_line { col.w - sort_w - mine_w - 24.0 } else { col.w };
    let field = Rect::new(col.x, y, field_w, BUTTON_H);
    let typing = v.market.typing && cx.focused;
    widgets::field(ui, field, &v.market.query, Some(v.market.query.len()), "Search Skill Issue: react, figma, fastify, video…", typing, Action::View(id, SEARCH));
    let end = Rect::new(field.right() - 30.0, y + 2.0, 28.0, BUTTON_H - 4.0);
    if !v.market.query.is_empty() {
        widgets::icon_button(ui, end, CROSS, t.muted, Action::View(id, CLEAR));
    } else if !typing {
        ui.icon(end, SEARCH_ICON, size::BODY, t.faint);
    }
    let (sx, sy) = if one_line { (field.right() + 12.0, y) } else { (col.x, y + BUTTON_H + 10.0) };
    let at = Sort::ALL.iter().position(|s| *s == v.sort).unwrap_or(0);
    widgets::segmented(ui, Rect::new(sx, sy, sort_w.min(col.w - mine_w - 12.0), BUTTON_H), &sorts, at, |i| Action::View(id, SORT + i));
    let mine = Rect::new(if one_line { col.right() - mine_w } else { sx + sort_w.min(col.w - mine_w - 12.0) + 12.0 }, sy, mine_w, BUTTON_H);
    key_button(ui, mine, v.mine.then_some(CHECK), mine_label, Some("f"), if v.mine { Kind::Primary } else { Kind::Secondary }, Some(Action::View(id, MINE)));
    y = sy + BUTTON_H + 12.0;

    // ---- the domain tabs, as chips with their counts
    let chips = chip_layout(v, ui, col.w);
    for (r, d, label) in &chips {
        let r = Rect::new(col.x + r.x, y + r.y, r.w, r.h);
        chip(ui, r, label, *d == v.domain, ("skills-domain", id, d.map_or(0, |d| d + 1)), Action::View(id, DOMAIN + d.map_or(0, |d| d + 1)));
    }
    y += chips.last().map_or(CHIP_H, |(r, ..)| r.bottom()) + 16.0;

    // ---- the list and the selected skill's details
    let hints_y = col.bottom() - 18.0;
    let body = Rect::new(col.x, y, col.w, (hints_y - 14.0 - y).max(ROW_H));
    let side = col.w >= SIDE_MIN;
    let (list, detail) = if side {
        (Rect::new(body.x, body.y, body.w - SIDE_W - 20.0, body.h), Rect::new(body.right() - SIDE_W, body.y, SIDE_W, body.h))
    } else {
        let dh = UNDER_H.min(body.h * 0.5);
        (Rect::new(body.x, body.y, body.w, body.h - dh - 12.0), Rect::new(body.x, body.bottom() - dh, body.w, dh))
    };
    let entries = v.discover_list(cx);
    v.market.sel = v.market.sel.min(entries.len().saturating_sub(1));
    draw_list(v, ui, list, &entries, cx);
    match v.selected(cx).and_then(|i| v.skill(i)).cloned() {
        Some(s) => {
            let state = v.state(&s, cx);
            let clicks = v.catalog.as_ref().map_or(0, |c| c.clicks_of(&s.id));
            if side {
                details(ui, detail, &s, &state, clicks, id);
            } else {
                details_under(ui, detail, &s, &state, id);
            }
        }
        None if side => {
            page_card(ui, detail);
            ui.label_in(Rect::new(detail.x + 18.0, detail.y + 18.0, detail.w - 36.0, 20.0), "Pick a skill to see what it does", Style::new(size::BODY, weight::REGULAR, t.faint), Align::Left);
        }
        None => {}
    }

    // ---- the keys, quietly
    hints(ui, Point::new(col.x, hints_y), col.w, &v.hints());
}

/// The repos bro features, a card each: what it is, its skills and what they need, and its add (a repo watched
/// already folds to a line that says so). Returns where the cards end.
fn featured(ui: &mut Ui, at: Rect, cx: &Cx) -> f32 {
    let t = ui.t;
    let id = cx.id;
    let watched = cx.svc.settings().skills.repos;
    let pad = 16.0;
    let mut y = at.y;
    for (i, f) in FEATURED.iter().enumerate() {
        let on = watched.iter().any(|w| skill_repos::same(w, f.repo));
        let (label, icon, kind) = if on { ("Watching", Some(CHECK), Kind::Secondary) } else { ("Add repo", Some(PLUS), Kind::Primary) };
        let aw = key_button_width(ui, icon, label, None);
        let gw = key_button_width(ui, Some(GITHUB), "GitHub", None);
        let x = at.x + pad + 34.0;
        let text_w = (at.w - pad - 34.0 - pad - aw - gw - 24.0).max(80.0);
        let blurb = (!on).then(|| ui.para(f.blurb, Style::new(size::SMALL, weight::REGULAR, t.text.mix(t.muted, 0.2)), text_w, Some(2)));
        let h = match &blurb {
            Some(b) => pad + 20.0 + 6.0 + b.h + 8.0 + 16.0 + 4.0 + 16.0 + pad,
            None => BUTTON_H + 20.0,
        };
        let r = Rect::new(at.x, y, at.w, h);
        page_card(ui, r);
        ui.icon(Rect::new(at.x + pad, r.y + if on { 0.0 } else { pad - 1.0 }, 20.0, if on { h } else { 22.0 }), STAR, size::HEADING, t.shine);
        // its name and who made it
        let ty = if on { r.y + (h - 20.0) / 2.0 } else { r.y + pad };
        let nb = ui.line(f.title, Style::new(size::TITLE, weight::SEMIBOLD, t.text), None);
        ui.draw(&nb, Point::new(x, ty + (20.0 - nb.h) / 2.0));
        let what = if on { format!("by {} · {} · watching: its page lists its skills to install", f.by, f.repo) } else { format!("by {} · {}", f.by, f.repo) };
        let bb = ui.line(&what, Style::new(size::SMALL, weight::REGULAR, t.faint), Some((text_w - nb.w - 10.0).max(10.0)));
        ui.draw(&bb, Point::new(x + nb.w + 10.0, ty + (20.0 - bb.h) / 2.0 + 1.0));
        if let Some(b) = &blurb {
            let mut ly = ty + 20.0 + 6.0;
            ui.draw(b, Point::new(x, ly));
            ly += b.h + 8.0;
            ui.label_in(Rect::new(x, ly, text_w, 16.0), &f.skills.join(" · "), Style::mono(size::CAPTION, t.muted), Align::Left);
            ly += 16.0 + 4.0;
            ui.label_in(Rect::new(x, ly, text_w, 16.0), &format!("needs {}", f.needs), Style::new(size::CAPTION, weight::REGULAR, t.faint), Align::Left);
        }
        // add (or the word it's watched), and its page
        let by = r.y + (h - BUTTON_H) / 2.0;
        let add = Rect::new(r.right() - pad - aw, by, aw, BUTTON_H);
        key_button(ui, add, icon, label, None, kind, Some(Action::View(id, FEATURE + i)));
        key_button(ui, Rect::new(add.x - 8.0 - gw, by, gw, BUTTON_H), Some(GITHUB), "GitHub", None, Kind::Ghost, Some(Action::View(id, FEATURE_WEB + i)));
        y += h + 10.0;
    }
    y - 10.0
}

/// The rows, scrolled; or why there are none, with the way out.
fn draw_list(v: &mut SkillsView, ui: &mut Ui, list: Rect, entries: &[usize], cx: &Cx) {
    let t = ui.t;
    let id = cx.id;
    if entries.is_empty() {
        let (msg, bad, way) = match (&v.catalog, &v.error) {
            (None, Some(e)) => (format!("Couldn't fetch the marketplace: {e}"), true, Some(("Try again", REFRESH))),
            (None, None) => ("Fetching skill-issue.in…".to_string(), false, None),
            _ if v.mine => ("Nothing installed from here yet.".into(), false, Some(("Show them all", MINE))),
            // a search looks in the domain shown: offer the rest
            _ if !v.market.query.trim().is_empty() => match v.domain {
                Some(d) => (format!("Nothing in {} matches “{}”.", DOMAINS[d].1, v.market.query.trim()), false, Some(("Search all domains", DOMAIN))),
                None => (format!("Nothing matches “{}”.", v.market.query.trim()), false, Some(("Clear the search", CLEAR))),
            },
            _ => ("Nothing here.".into(), false, None),
        };
        let mut y = list.y + 18.0;
        if v.catalog.is_none() && v.error.is_none() {
            ui.spinner(Point::new(list.x + 24.0, y + 10.0), 6.0, 1.7, t.muted);
        }
        let x = list.x + if v.catalog.is_none() && v.error.is_none() { 40.0 } else { 16.0 };
        let b = ui.para(&msg, Style::new(size::BODY, weight::REGULAR, if bad { t.danger } else { t.muted }), list.w - 32.0, Some(3));
        ui.draw(&b, Point::new(x, y));
        y += b.h + 12.0;
        if let Some((label, n)) = way {
            let w = key_button_width(ui, None, label, None);
            key_button(ui, Rect::new(x, y, w, BUTTON_H), None, label, None, Kind::Secondary, Some(Action::View(id, n)));
        }
        return;
    }
    let view = Rect::new(list.x - 8.0, list.y, list.w + 8.0, list.h);
    let content_h = entries.len() as f32 * ROW_H;
    // `scroll` holds the list's offset in pixels here; it follows the selection only when the selection moved
    // (the wheel scrolls freely)
    let mut scroll = v.market.scroll as f32;
    if v.market.shown_sel != Some(v.market.sel) {
        let top = v.market.sel as f32 * ROW_H;
        if top - 4.0 < scroll {
            scroll = top - 4.0;
        } else if top + ROW_H + 18.0 > scroll + view.h {
            scroll = top + ROW_H + 18.0 - view.h;
        }
        v.market.shown_sel = Some(v.market.sel);
    }
    scroll = scroll.clamp(0.0, (content_h - view.h).max(0.0));
    v.market.scroll = scroll.round() as usize;
    let shown = ui.ease(("skills-scroll", id), scroll, 150.0);
    let ranked = v.sort == Sort::Ranked && v.market.query.trim().is_empty();
    let st = cx.svc.state();
    let mut sel_at = None;
    ui.clip(view, 0.0);
    for (row, &i) in entries.iter().enumerate() {
        let ry = view.y + row as f32 * ROW_H - shown;
        if ry + ROW_H < view.y || ry > view.bottom() {
            continue;
        }
        let Some(s) = v.catalog.as_ref().and_then(|c| c.skills.get(i)) else { continue };
        let clicks = v.catalog.as_ref().map_or(0, |c| c.clicks_of(&s.id));
        let r = Rect::new(view.x, ry, view.w - 8.0, ROW_H);
        let fill = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
        let on = v.market.sel == row;
        if on {
            // a menu opened with keys opens here
            sel_at = Some((r.x + 48.0, (r.bottom() - 6.0).min(view.bottom() - 12.0)));
        }
        let hot = widgets::row(ui, fill, ("skills", id, row), on, Action::View(id, ROW + row));
        skill_row(ui, r, row, s, &state_of(s, &st), clicks, on, hot, ranked.then_some(row + 1), id);
    }
    drop(st);
    v.sel_at = sel_at;
    if shown > 0.5 {
        edge_fade(ui, Rect::new(view.x, view.y, view.w, 18.0), t.surface, true);
    }
    if shown + view.h < content_h - 0.5 {
        edge_fade(ui, Rect::new(view.x, view.bottom() - 24.0, view.w, 24.0), t.surface, false);
    }
    ui.unclip();
    let fade = ui.ease(("skills-bar", id), if ui.hovered(view) { 1.0 } else { 0.35 }, 200.0);
    widgets::scrollbar(ui, view, content_h, view.h, shown, fade);
}

/// One skill: where it stands (or its place in the ranking), its name and maker, what it's for; on the right its
/// numbers and its button.
#[allow(clippy::too_many_arguments)]
fn skill_row(ui: &mut Ui, r: Rect, row: usize, s: &MarketSkill, state: &State, clicks: u64, on: bool, hot: f32, rank: Option<usize>, id: PaneId) {
    let t = ui.t;
    let mid = r.y + r.h / 2.0;

    // where it stands, or its rank
    let mr = Rect::new(r.x + 8.0, mid - 10.0, 28.0, 20.0);
    match state {
        State::Installing(_) => ui.spinner(mr.center(), 6.0, 1.7, t.shine),
        State::Installed { .. } => ui.icon(mr, CHECK, size::HEADING, t.good),
        State::Failed(_) => ui.icon(mr, ALERT, size::TITLE, t.danger),
        State::Disabled => ui.icon(mr, ARCHIVE, size::TITLE, t.faint),
        State::Yours => ui.icon(mr, ACCOUNT, size::TITLE, t.faint),
        _ => {
            if let Some(n) = rank {
                ui.label_in(mr, &n.to_string(), Style::new(size::SMALL, weight::SEMIBOLD, if n <= 3 { t.shine } else { t.faint }), Align::Center);
            }
        }
    }

    // the buttons, right to left: its ⋯ (while it's hovered or selected), then its main one
    let mut bx = r.right() - 10.0 - BUTTON_H;
    let by = mid - BUTTON_H / 2.0;
    if on || hot > 0.05 {
        let b = Rect::new(bx, by, BUTTON_H, BUTTON_H);
        if widgets::icon_button(ui, b, MORE, t.muted, Action::View(id, MORE_ROW + row)) {
            widgets::tooltip(ui, b, "More (or right-click)");
        }
    }
    bx -= 6.0;
    let (label, icon, kind, act) = button_for(state, on || hot > 0.5);
    let key = (on && act).then_some("enter");
    let w = key_button_width(ui, icon, label, key).max(96.0);
    bx -= w;
    let button = Rect::new(bx, by, w, BUTTON_H);
    if on && act {
        focus_ring(ui, button);
    }
    key_button(ui, button, icon, label, key, kind, act.then_some(Action::View(id, INSTALL + row)));
    bx -= 14.0;

    // its numbers: the site's (stars or installs) over its clicks
    let nums_w = 92.0;
    let room = bx - r.x - 44.0;
    let nums = room > 360.0;
    if nums {
        let nx = bx - nums_w;
        if let Some(p) = s.popularity() {
            let (glyph, value) = metric_parts(&p);
            let st = Style::new(size::SMALL, weight::SEMIBOLD, t.inline);
            let vw = ui.measure(&value, st);
            ui.label(Point::new(nx + nums_w - vw, r.y + 10.0), &value, st);
            ui.icon(Rect::new(nx + nums_w - vw - 18.0, r.y + 9.0, 14.0, 18.0), glyph, size::SMALL, t.inline);
        }
        let c = clicks_label(clicks);
        if !c.is_empty() {
            let st = Style::new(size::CAPTION, weight::REGULAR, t.faint);
            let cw = ui.measure(&c, st);
            ui.label(Point::new(nx + nums_w - cw, r.y + 31.0), &c, st);
        }
        bx = nx - 12.0;
    }

    // name, maker, what it's for
    let x = r.x + 44.0;
    let text_w = (bx - x).max(40.0);
    let name_st = Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::MEDIUM }, if matches!(state, State::Installed { .. }) { t.good.mix(t.text, 0.35) } else { t.text });
    let nb = ui.line(&s.name, name_st, Some(text_w * 0.7));
    ui.draw(&nb, Point::new(x, r.y + 9.0));
    let by_st = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let cb = ui.line(&format!("by {}", s.creator), by_st, Some((text_w - nb.w - 10.0).max(10.0)));
    if nb.w + 10.0 + 30.0 < text_w {
        ui.draw(&cb, Point::new(x + nb.w + 8.0, r.y + 9.0 + (nb.h - cb.h) / 2.0 + 1.0));
    }
    let line2 = match state {
        State::Failed(e) => format!("didn't pull: {e}"),
        _ => s.use_case.clone(),
    };
    let ink = if matches!(state, State::Failed(_)) { t.danger } else { t.muted };
    ui.label_in(Rect::new(x, r.y + 31.0, text_w, 18.0), &line2, Style::new(size::SMALL, weight::REGULAR, ink), Align::Left);
}

/// A row's (and the details') main button for where the skill stands: (label, icon, kind, does something).
fn button_for(state: &State, lit: bool) -> (&'static str, Option<&'static str>, Kind, bool) {
    match state {
        State::Available => ("Install", Some(DOWNLOAD), if lit { Kind::Primary } else { Kind::Secondary }, true),
        State::Installing(_) => ("Installing", None, Kind::Secondary, false),
        State::Failed(_) => ("Retry", Some(REFRESH_ICON), Kind::Secondary, true),
        State::Installed { via: None } => ("Installed", Some(CHECK), Kind::Ghost, false),
        State::Installed { via: Some(_) } => ("With its repo", Some(CHECK), Kind::Ghost, false),
        State::Disabled => ("Enable", Some(RESTORE), Kind::Secondary, true),
        State::Yours => ("Yours", None, Kind::Ghost, false),
        State::NoSource => ("No source", None, Kind::Ghost, false),
    }
}

/// "90.4k stars" → (★, "90.4k"); "694k installs" → (↓, "694k").
fn metric_parts(p: &str) -> (&'static str, String) {
    let (value, what) = p.split_once(' ').unwrap_or((p, ""));
    (if what == "installs" { DOWNLOAD } else { STAR }, value.to_string())
}

/// What the selected skill is, beside the list: its name, maker, domain, numbers, what it's for, where it
/// stands and comes from, and its buttons.
fn details(ui: &mut Ui, r: Rect, s: &MarketSkill, state: &State, clicks: u64, id: PaneId) {
    let t = ui.t;
    page_card(ui, r);
    ui.clip(r, radius::OVERLAY);
    let pad = 18.0;
    let x = r.x + pad;
    let w = r.w - 2.0 * pad;
    let mut y = r.y + pad;

    let nb = ui.para(&s.name, Style::new(size::HEADING + 2.0, weight::SEMIBOLD, t.text), w, Some(2));
    ui.draw(&nb, Point::new(x, y));
    y += nb.h + 2.0;
    let cb = ui.line(&format!("by {}", s.creator), Style::new(size::BODY, weight::REGULAR, t.muted), Some(w));
    ui.draw(&cb, Point::new(x, y));
    y += cb.h + 12.0;

    // domain, niche, an unchecked link
    let mut px = x;
    let mut pill = |ui: &mut Ui, text: &str, c: Color| {
        let pw = widgets::pill_width(ui, text);
        if px + pw <= x + w {
            widgets::pill(ui, Point::new(px, y), 20.0, text, c, c.with_alpha(0.14));
            px += pw + 6.0;
        }
    };
    pill(ui, &s.domain, t.accent);
    if let Some(n) = &s.niche {
        pill(ui, n, t.muted);
    }
    if !s.link_verified {
        pill(ui, "link not checked by hand", t.warn);
    }
    y += 20.0 + 14.0;

    // the numbers
    let mut nx = x;
    if let Some(p) = s.popularity() {
        let (glyph, value) = metric_parts(&p);
        let what = p.split_once(' ').map_or("", |(_, w)| w).to_string();
        nx = stat(ui, Point::new(nx, y), glyph, &value, &what, t.inline) + 22.0;
    }
    if clicks > 0 {
        stat(ui, Point::new(nx, y), CLICK, &skill_market::compact(clicks as f64), "clicks on Skill Issue", t.muted);
    }
    if s.popularity().is_some() || clicks > 0 {
        y += 48.0;
    }

    // what it's for
    let ub = ui.para(&s.use_case, Style::new(size::BODY, weight::REGULAR, t.text.mix(t.muted, 0.15)), w, Some(8));
    ui.draw(&ub, Point::new(x, y));
    y += ub.h + 16.0;

    // where it stands
    if let Some((glyph, text, c)) = standing(s, state, t) {
        let b = ui.para(&text, Style::new(size::SMALL, weight::MEDIUM, c), w - 22.0, Some(4));
        ui.icon(Rect::new(x, y, 16.0, 18.0), glyph, size::BODY, c);
        ui.draw(&b, Point::new(x + 22.0, y + 1.0));
        y += b.h.max(18.0) + 14.0;
    }

    // where it comes from
    if let Some(spec) = skill_market::source(s) {
        widgets::section(ui, Rect::new(x, y, w, 18.0), "Installs from");
        y += 22.0;
        let b = ui.para(&skill_repos::label(&spec), Style::mono(size::SMALL, t.text.mix(t.muted, 0.2)), w, Some(2));
        ui.draw(&b, Point::new(x, y));
        y += b.h + 16.0;
    }

    // the buttons, from the bottom up when there's room, else under the rest
    let rows_h = BUTTON_H * 2.0 + 8.0;
    let by = (r.bottom() - pad - rows_h).max(y);
    detail_buttons(ui, Rect::new(x, by, w, rows_h), s, state, id, false);
    ui.unclip();
}

/// The selected skill under the list (a narrow pane): name, maker, what it's for and where it stands on the
/// left, the buttons on the right.
fn details_under(ui: &mut Ui, r: Rect, s: &MarketSkill, state: &State, id: PaneId) {
    let t = ui.t;
    page_card(ui, r);
    ui.clip(r, radius::OVERLAY);
    let pad = 16.0;
    let buttons_w = 236.0f32.min(r.w * 0.42);
    let x = r.x + pad;
    let w = r.w - 3.0 * pad - buttons_w;
    let mut y = r.y + pad - 2.0;
    let nb = ui.line(&s.name, Style::new(size::TITLE, weight::SEMIBOLD, t.text), Some(w * 0.6));
    ui.draw(&nb, Point::new(x, y));
    let place = s.niche.clone().map_or_else(|| s.domain.clone(), |n| format!("{} · {n}", s.domain));
    let cb = ui.line(&format!("by {} · {place}", s.creator), Style::new(size::SMALL, weight::REGULAR, t.muted), Some((w - nb.w - 10.0).max(10.0)));
    ui.draw(&cb, Point::new(x + nb.w + 10.0, y + (nb.h - cb.h) / 2.0 + 1.0));
    y += nb.h + 6.0;
    let ub = ui.para(&s.use_case, Style::new(size::SMALL, weight::REGULAR, t.text.mix(t.muted, 0.2)), w, Some(2));
    ui.draw(&ub, Point::new(x, y));
    y += ub.h + 6.0;
    let note = standing(s, state, t).map(|(_, text, c)| (text, c)).or_else(|| skill_market::source(s).map(|spec| (format!("installs from {}", skill_repos::label(&spec)), t.faint)));
    if let Some((text, c)) = note {
        ui.label_in(Rect::new(x, y, w, 18.0), &text, Style::new(size::SMALL, weight::MEDIUM, c), Align::Left);
    }
    let rows_h = BUTTON_H * 2.0 + 8.0;
    detail_buttons(ui, Rect::new(r.right() - pad - buttons_w, r.y + (r.h - rows_h).max(0.0) / 2.0, buttons_w, rows_h), s, state, id, true);
    ui.unclip();
}

/// Where the skill stands, in a line, when there's more to say than the button does: (icon, text, colour).
fn standing(s: &MarketSkill, state: &State, t: &crate::app::gui::kit::Tokens) -> Option<(&'static str, String, Color)> {
    Some(match state {
        State::Installed { via: None } => (CHECK, "Installed · bro keeps it in step with its repo".into(), t.good),
        State::Installed { via: Some(repo) } => (CHECK, format!("Installed from {}, which bro watches", skill_repos::label(repo)), t.good),
        State::Installing(_) => (DOWNLOAD, "Installing: fetching just this skill's folder…".into(), t.shine),
        State::Failed(e) => (ALERT, format!("Didn't pull: {e}"), t.danger),
        State::Disabled => (ARCHIVE, "Disabled: enable it to put it back where it was".into(), t.muted),
        State::Yours => (ACCOUNT, format!("You have a skill named {} in ~/.claude/skills already · bro leaves it alone", s.name), t.muted),
        State::NoSource => (INFO, "bro can't tell which repo it lives in · its source link may say".into(), t.muted),
        State::Available => return None,
    })
}

/// The details' buttons in `r` (two rows): install (or enable) and disable first, then its page and its source.
fn detail_buttons(ui: &mut Ui, r: Rect, s: &MarketSkill, state: &State, id: PaneId, narrow: bool) {
    let (label, icon, kind, act) = button_for(state, true);
    let removable = matches!(state, State::Installed { .. });
    let half = (r.w - 8.0) / 2.0;
    // the main action, and disabling beside it when it's installed
    let main_w = if removable { half } else { r.w };
    let key = if *state == State::Disabled { "e" } else { "enter" };
    key_button(ui, Rect::new(r.x, r.y, main_w, BUTTON_H), icon, label, act.then_some(key), kind, act.then_some(Action::View(id, INSTALL_SEL)));
    if removable {
        key_button(ui, Rect::new(r.x + half + 8.0, r.y, half, BUTTON_H), Some(ARCHIVE), "Disable", Some("d"), Kind::Secondary, Some(Action::View(id, REMOVE_SEL)));
    }
    let y = r.y + BUTTON_H + 8.0;
    let github = s.link.contains("github.com");
    let (page_l, src_l) = if narrow { ("Page", "Source") } else { ("Page", if github { "GitHub" } else { "Source" }) };
    key_button(ui, Rect::new(r.x, y, half, BUTTON_H), Some(OPEN), page_l, (!narrow).then_some("o"), Kind::Secondary, Some(Action::View(id, PAGE)));
    key_button(ui, Rect::new(r.x + half + 8.0, y, half, BUTTON_H), Some(if github { GITHUB } else { OPEN }), src_l, (!narrow).then_some("l"), Kind::Secondary, (!s.link.is_empty()).then_some(Action::View(id, SOURCE)));
}

/// A number with its icon over what it counts. Returns its right edge.
fn stat(ui: &mut Ui, at: Point, glyph: &str, value: &str, what: &str, c: Color) -> f32 {
    let t = ui.t;
    ui.icon(Rect::new(at.x, at.y, 16.0, 20.0), glyph, size::TITLE, c);
    let vb = ui.line(value, Style::new(size::HEADING, weight::SEMIBOLD, t.text), None);
    ui.draw(&vb, Point::new(at.x + 20.0, at.y + (20.0 - vb.h) / 2.0));
    let wb = ui.line(what, Style::new(size::CAPTION, weight::REGULAR, t.faint), None);
    ui.draw(&wb, Point::new(at.x, at.y + 22.0));
    (at.x + 20.0 + vb.w).max(at.x + wb.w)
}

/// The domain chips laid out in rows within `w`: (rect relative to the first, domain, label).
fn chip_layout(v: &SkillsView, ui: &mut Ui, w: f32) -> Vec<(Rect, Option<usize>, String)> {
    let mut out = vec![];
    let (mut x, mut y) = (0.0, 0.0);
    for d in std::iter::once(None).chain((0..DOMAINS.len()).map(Some)) {
        let name = d.map_or("All", |d| DOMAINS[d].1);
        let label = match v.catalog.as_ref().map(|c| c.count(d.map(|d| DOMAINS[d].0))) {
            Some(n) => format!("{name}  {n}"),
            None => name.to_string(),
        };
        let cw = ui.measure(&label, Style::new(size::SMALL, weight::SEMIBOLD, ui.t.text)) + 26.0;
        if x > 0.0 && x + cw > w {
            x = 0.0;
            y += CHIP_H + 8.0;
        }
        out.push((Rect::new(x, y, cw, CHIP_H), d, label));
        x += cw + 8.0;
    }
    out
}

/// A domain tab: the accent when it's the one shown, a quiet control otherwise.
fn chip(ui: &mut Ui, r: Rect, label: &str, on: bool, key: impl std::hash::Hash, action: Action) {
    let t = ui.t;
    let hot = ui.hover_amount(key, r);
    let ink = if on {
        ui.round(r, r.h / 2.0, t.accent.fade(1.0 - 0.1 * hot));
        t.on_accent
    } else {
        ui.panel(r, r.h / 2.0, t.control.mix(t.control_hover, hot), t.stroke);
        t.text.mix(t.muted, 0.3 - 0.3 * hot)
    };
    // the count quieter than the name
    match label.split_once("  ") {
        Some((name, n)) => {
            let ns = Style::new(size::SMALL, weight::SEMIBOLD, ink);
            let cs = Style::new(size::SMALL, weight::MEDIUM, ink.fade(0.65));
            let nb = ui.line(name, ns, None);
            let cb = ui.line(n, cs, None);
            let x = r.x + (r.w - nb.w - 6.0 - cb.w) / 2.0;
            ui.draw(&nb, Point::new(x, r.y + (r.h - nb.h) / 2.0));
            ui.draw(&cb, Point::new(x + nb.w + 6.0, r.y + (r.h - cb.h) / 2.0));
        }
        None => {
            ui.label_in(r, label, Style::new(size::SMALL, weight::SEMIBOLD, ink), Align::Center);
        }
    }
    ui.hit(r, action);
}

fn key(v: &mut SkillsView, code: KeyCode, cx: &mut Cx) {
    v.key(KeyEvent::new(code, KeyModifiers::NONE), cx);
}

/// Its install button (or the details'), clicked at `p`: a disabled one is enabled; else where to install it.
fn install_at(v: &mut SkillsView, p: Point, cx: &mut Cx) {
    match v.selected(cx).and_then(|i| v.skill(i)).map(|s| v.state(s, cx)) {
        Some(State::Disabled) => v.enable_market(cx),
        _ => v.install(Some((p.x, p.y)), cx),
    }
}

/// Target `n` was clicked.
pub(super) fn click(v: &mut SkillsView, n: usize, p: Point, _mods: KeyModifiers, count: u32, cx: &mut Cx) {
    if n != SEARCH && n != CLEAR {
        v.market.typing = false;
    }
    match n {
        SEARCH => v.market.typing = true,
        CLEAR => {
            v.market.query.clear();
            v.market.sel = 0;
            v.market.scroll = 0;
            v.market.typing = true;
        }
        REFRESH => key(v, KeyCode::Char('r'), cx),
        MINE => v.toggle_mine(),
        SITE => open_link(skill_market::site(), cx),
        n if (FEATURE..FEATURE + FEATURED.len()).contains(&n) => v.feature(&FEATURED[n - FEATURE], cx),
        n if (FEATURE_WEB..FEATURE_WEB + FEATURED.len()).contains(&n) => open_link(skill_repos::web_url(FEATURED[n - FEATURE_WEB].repo), cx),
        PAGE => v.open(false, cx),
        SOURCE => v.open(true, cx),
        INSTALL_SEL => install_at(v, p, cx),
        REMOVE_SEL => v.remove(cx),
        n if (SORT..SORT + Sort::ALL.len()).contains(&n) => v.set_sort(Sort::ALL[n - SORT]),
        n if (DOMAIN..=DOMAIN + DOMAINS.len()).contains(&n) => v.set_domain((n > DOMAIN).then(|| n - DOMAIN - 1)),
        n if n >= MORE_ROW => {
            v.market.sel = n - MORE_ROW;
            v.open_menu(Some((p.x, p.y)), cx);
        }
        n if n >= REMOVE => {
            v.market.sel = n - REMOVE;
            v.remove(cx);
        }
        n if n >= INSTALL => {
            v.market.sel = n - INSTALL;
            install_at(v, p, cx);
        }
        n if n >= ROW => {
            let row = n - ROW;
            if count >= 2 && v.market.sel == row {
                install_at(v, p, cx);
            }
            v.market.sel = row;
        }
        _ => {}
    }
}
