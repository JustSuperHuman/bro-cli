//! The Sounds view in the desktop app: the switch beside the title; a card with the volume and the test, the
//! moments that speak as chips, and a line per harness (its switch, who plays its sounds) with peon-ping's own
//! Claude logins and the button that hands them to bro; then the packs, installed or the registry's, searchable,
//! each row with its preview, its use / install / update button and its removal. It draws [`SoundsView`]'s own
//! state; keys and clicks do what they do in the terminal UI.

use super::{CATEGORIES, Entry, HARNESS_KEYS, HINTS, SoundsView, Tab, harness_name};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::brand;
use crate::app::gui::kit::widgets::{self, Kind, SWITCH_H, SWITCH_W};
use crate::app::gui::kit::{Cursor, Style, Ui, radius, size, weight};
use crate::pane::{Cx, Pane};
use crate::views::profiles::gui::{BUTTON_H, TITLE, edge_fade, focus_ring, hints, key_button, key_button_width, page, page_card};
use bro_gfx::text::Align;
use bro_gfx::{Point, Rect};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// targets
const BG: usize = 0;
const SWITCH: usize = 1;
const VOL_DOWN: usize = 2;
const VOL_UP: usize = 3;
const TEST: usize = 4;
const TAKE_OVER: usize = 5;
const TAB_INSTALLED: usize = 6;
const TAB_BROWSE: usize = 7;
const SEARCH: usize = 8;
const REFRESH: usize = 9;
/// `CAT + i`: category chip `i`
const CAT: usize = 20;
/// `HARNESS + i`: [`HARNESS_KEYS`]`[i]`'s switch
const HARNESS: usize = 40;
/// `ROW + i`: list row `i` (select; again or double-click: use)
const ROW: usize = 1_000;
const PREVIEW: usize = 100_000;
const USE: usize = 200_000;
const REMOVE: usize = 300_000;
const UPDATE: usize = 400_000;

const ROW_H: f32 = 52.0;
const CHIP_H: f32 = 26.0;

const PLAY: &str = "\u{F040A}";
const STOP: &str = "\u{F04DB}";
const MINUS: &str = "\u{F0374}";
const PLUS: &str = "\u{F0415}";
const TRASH: &str = "\u{F01B4}";
const DOWNLOAD: &str = "\u{F01DA}";
const CHECK: &str = "\u{F012C}";
const STAR: &str = "\u{F04CE}";
const REFRESH_ICON: &str = "\u{F0450}";
const SEARCH_ICON: &str = "\u{F0349}";
const UPDATE_ICON: &str = "\u{F06B0}";
const VOLUME: &str = "\u{F057E}";
const MUTED: &str = "\u{F0581}";
const INFO: &str = "\u{F02FD}";

/// Draw the view inside its card (`area`: the card's content).
pub(crate) fn draw(v: &mut SoundsView, ui: &mut Ui, area: Rect, cx: &mut Cx) -> bool {
    let t = ui.t;
    let id = cx.id;
    v.start(cx);
    // the window only ticks panes while nothing animates: take what the workers brought here too
    v.take_inbox(cx);
    if !v.jobs.is_empty() || v.fetching || v.playing.is_some() {
        ui.ticking = true;
    }
    ui.hit_with(area, Action::View(id, BG), Cursor::Default);
    let col = page(area, 980.0);
    let on = v.on(cx);

    // ---- title, what it is, the switch
    let tb = ui.line("Sounds", Style::new(TITLE, weight::SEMIBOLD, t.text), None);
    ui.draw(&tb, Point::new(col.x, col.y));
    let label = if on { "Enabled" } else { "Disabled" };
    let lw = ui.measure(label, Style::new(size::BODY, weight::MEDIUM, t.text));
    let sx = col.right() - SWITCH_W;
    widgets::switch(ui, Point::new(sx, col.y + 2.0), on, Action::View(id, SWITCH));
    ui.label_in(Rect::new(sx - lw - 10.0, col.y, lw, SWITCH_H + 4.0), label, Style::new(size::BODY, weight::MEDIUM, if on { t.text } else { t.muted }), Align::Left);
    let sub = "Voice lines when a session starts, finishes, fails or needs you · provided by PeonPing, https://peonping.com";
    let sub_y = col.y + tb.h + 2.0;
    ui.label_in(Rect::new(col.x, sub_y, col.w - lw - SWITCH_W - 24.0, 18.0), sub, Style::new(size::BODY, weight::REGULAR, t.muted), Align::Left);
    let mut y = sub_y + 18.0 + 16.0;

    // ---- the playback card
    let theirs: Vec<String> = v.peon_ping_logins().iter().map(|l| l.name.clone()).collect();
    let pad = 16.0;
    let inner_w = col.w - 2.0 * pad;
    let chips = chip_layout(ui, inner_w - 92.0);
    let chips_h = chips.last().map_or(CHIP_H, |(r, _)| r.bottom());
    // as many harnesses a line as fit whole: switch, mark, name and who plays
    let widest = HARNESS_KEYS
        .iter()
        .map(|(h, _)| {
            let name = ui.measure(harness_name(*h), Style::new(size::SMALL, weight::SEMIBOLD, t.text));
            let state = ui.measure(&v.harness_state(*h, cx).0, Style::new(size::SMALL, weight::REGULAR, t.text));
            SWITCH_W + 34.0 + name + 8.0 + state + 16.0
        })
        .fold(0.0f32, f32::max);
    let harness_cols = [4usize, 2, 1].into_iter().find(|n| *n as f32 * widest <= inner_w - 92.0).unwrap_or(1);
    let harness_rows = HARNESS_KEYS.len().div_ceil(harness_cols);
    let notice_h = if theirs.is_empty() { 0.0 } else { 12.0 + BUTTON_H };
    let card_h = pad + BUTTON_H + 14.0 + chips_h + 14.0 + harness_rows as f32 * 30.0 + notice_h + pad;
    let card = Rect::new(col.x, y, col.w, card_h);
    page_card(ui, card);
    let x0 = card.x + pad;
    let lab = |ui: &mut Ui, y: f32, h: f32, s: &str| {
        ui.label_in(Rect::new(x0, y, 88.0, h), s, Style::new(size::SMALL, weight::SEMIBOLD, ui.t.text.mix(ui.t.muted, 0.35)), Align::Left);
    };
    let mut cy = card.y + pad;
    let fx = x0 + 92.0;

    // volume: − meter + 50%, then the test
    lab(ui, cy, BUTTON_H, "Volume");
    let vol = v.cfg.volume();
    widgets::icon_button(ui, Rect::new(fx, cy, BUTTON_H, BUTTON_H), MINUS, t.muted, Action::View(id, VOL_DOWN));
    let mw = (inner_w - 92.0 - 2.0 * BUTTON_H - 220.0).clamp(80.0, 260.0);
    let meter = Rect::new(fx + BUTTON_H + 8.0, cy, mw, BUTTON_H);
    widgets::segments(ui, Rect::new(meter.x, meter.y + BUTTON_H / 2.0 - 3.0, meter.w, 6.0), vol, 10, if on { t.accent } else { t.muted });
    ui.hit(meter, Action::View(id, BG));
    widgets::icon_button(ui, Rect::new(meter.right() + 8.0, cy, BUTTON_H, BUTTON_H), PLUS, t.muted, Action::View(id, VOL_UP));
    let pct = format!("{}%", (vol * 100.0).round());
    let px = meter.right() + 16.0 + BUTTON_H;
    ui.icon(Rect::new(px, cy, 18.0, BUTTON_H), if vol <= 0.0 { MUTED } else { VOLUME }, size::BODY, t.muted);
    ui.label_in(Rect::new(px + 22.0, cy, 44.0, BUTTON_H), &pct, Style::new(size::BODY, weight::MEDIUM, t.text), Align::Left);
    let playing_test = v.playing.is_some() && v.selected().is_none_or(|e| v.playing.as_deref() != Some(v.name_of(e).as_str()));
    let tw = key_button_width(ui, Some(if playing_test { STOP } else { PLAY }), "Test", Some("t"));
    key_button(ui, Rect::new(card.right() - pad - tw, cy, tw, BUTTON_H), Some(if playing_test { STOP } else { PLAY }), "Test", Some("t"), Kind::Secondary, Some(Action::View(id, TEST)));
    cy += BUTTON_H + 14.0;

    // the moments that speak
    lab(ui, cy, CHIP_H, "Plays on");
    let active = v.active().map(str::to_string);
    let covered = |cat: &str| active.as_ref().and_then(|a| v.installed.iter().find(|p| &p.name == a)).is_none_or(|p| p.covers(cat));
    for (r, i) in &chips {
        let (cat, label) = CATEGORIES[*i];
        let r = Rect::new(fx + r.x, cy + r.y, r.w, r.h);
        let lit = v.cfg.category_on(cat);
        let hot = ui.hover_amount(("sound-chip", id, *i), r);
        if lit {
            ui.round(r, radius::CONTROL, t.accent.with_alpha(0.16 + 0.06 * hot));
        } else {
            ui.panel(r, radius::CONTROL, t.text.with_alpha(0.02 + 0.04 * hot), t.stroke);
        }
        let ink = match () {
            _ if !lit => t.faint,
            // switched on, but the pack in use has nothing for it
            _ if !covered(cat) => t.muted,
            _ => t.text,
        };
        let key = format!("{}", i + 1);
        ui.label_in(Rect::new(r.x + 8.0, r.y, 12.0, r.h), &key, Style::mono(size::CAPTION, t.faint), Align::Left);
        ui.label_in(Rect::new(r.x + 22.0, r.y, r.w - 28.0, r.h), label, Style::new(size::SMALL, if lit { weight::MEDIUM } else { weight::REGULAR }, ink), Align::Left);
        ui.hit(r, Action::View(id, CAT + i));
        if ui.hovered(r) && lit && !covered(cat) {
            widgets::tooltip(ui, r, "the pack in use has no lines for this");
        }
    }
    cy += chips_h + 14.0;

    // a line per harness: its switch, its mark, who plays for it
    let cw = (inner_w - 92.0) / harness_cols as f32;
    lab(ui, cy, 24.0, "Sessions");
    for (k, (h, _key)) in HARNESS_KEYS.iter().enumerate() {
        let (cxk, cyk) = (k % harness_cols, k / harness_cols);
        let at = Point::new(fx + cxk as f32 * cw, cy + cyk as f32 * 30.0);
        let (state, wired) = v.harness_state(*h, cx);
        let own = cx.svc.settings().peon.harness(*h);
        widgets::switch(ui, Point::new(at.x, at.y + 2.0), own, Action::View(id, HARNESS + k));
        ui.mark(brand::harness(*h), Rect::new(at.x + SWITCH_W + 10.0, at.y + 3.0, 18.0, 18.0), 15.0, None);
        let nx = at.x + SWITCH_W + 34.0;
        let nb = ui.line(harness_name(*h), Style::new(size::SMALL, weight::SEMIBOLD, t.text), None);
        ui.draw(&nb, Point::new(nx, at.y + (24.0 - nb.h) / 2.0));
        let sx = nx + nb.w + 8.0;
        let ink = if wired { t.good.mix(t.text, 0.15) } else { t.faint };
        ui.label_in(Rect::new(sx, at.y, (at.x + cw - sx - 8.0).max(10.0), 24.0), &state, Style::new(size::SMALL, weight::REGULAR, ink), Align::Left);
    }
    cy += harness_rows as f32 * 30.0;

    // peon-ping's own Claude logins, and the hand-over
    if !theirs.is_empty() {
        cy += 12.0;
        let bw = key_button_width(ui, None, "Let bro play them", Some("o"));
        let (login, run) = if theirs.len() == 1 { ("login", "runs") } else { ("logins", "run") };
        // what bro playing them changes: on Windows peon-ping's player is WAV-only and has no volume
        let why = if cfg!(windows) { ", so MP3 packs need ffmpeg there and the volume is ignored" } else { ": bro plays them alike in and out of bro" };
        let text = format!("Claude Code's {} {login} {run} peon-ping's own scripts{why}", theirs.join(", "));
        ui.icon(Rect::new(x0, cy, 16.0, BUTTON_H), INFO, size::BODY, t.shine);
        let note = ui.para(&text, Style::new(size::SMALL, weight::REGULAR, t.text.mix(t.muted, 0.2)), (inner_w - 30.0 - bw - 12.0).max(80.0), Some(2));
        ui.draw(&note, Point::new(x0 + 24.0, cy + (BUTTON_H - note.h) / 2.0));
        key_button(ui, Rect::new(card.right() - pad - bw, cy, bw, BUTTON_H), None, "Let bro play them", Some("o"), Kind::Primary, Some(Action::View(id, TAKE_OVER)));
    }
    y = card.bottom() + 20.0;

    // ---- the tabs, the search, the refresh
    let n_inst = v.installed.len();
    let n_reg = v.registry.as_ref().map(Vec::len);
    let tab_labels = [format!("Installed  {n_inst}"), n_reg.map_or("Browse".to_string(), |n| format!("Browse  {n}"))];
    let tabs = [tab_labels[0].as_str(), tab_labels[1].as_str()];
    let seg = Rect::new(col.x, y, 260.0f32.min(col.w * 0.45), BUTTON_H);
    widgets::segmented(ui, seg, &tabs, if v.tab == Tab::Installed { 0 } else { 1 }, |i| Action::View(id, if i == 0 { TAB_INSTALLED } else { TAB_BROWSE }));
    let rw = BUTTON_H;
    let refresh = Rect::new(col.right() - rw, y, rw, BUTTON_H);
    if v.fetching {
        ui.spinner(refresh.center(), 6.0, 1.7, t.muted);
    } else {
        widgets::icon_button(ui, refresh, REFRESH_ICON, t.muted, Action::View(id, REFRESH));
    }
    let field = Rect::new(seg.right() + 12.0, y, (refresh.x - 8.0 - seg.right() - 12.0).max(60.0), BUTTON_H);
    widgets::field(ui, field, &v.query, Some(v.query.len()), "Search packs: zelda, warcraft, portal…", v.typing && cx.focused, Action::View(id, SEARCH));
    if v.query.is_empty() && !v.typing {
        ui.icon(Rect::new(field.right() - 28.0, y, 20.0, BUTTON_H), SEARCH_ICON, size::BODY, t.faint);
    }
    y += BUTTON_H + 12.0;

    // ---- the list
    let hints_y = col.bottom() - 18.0;
    let view = Rect::new(col.x - 8.0, y, col.w + 16.0, (hints_y - 14.0 - y).max(ROW_H));
    let entries = v.entries();
    v.sel = v.sel.min(entries.len().saturating_sub(1));
    let content_h = entries.len() as f32 * ROW_H;
    let mut scroll = v.scroll as f32;
    if !entries.is_empty() {
        let top = v.sel as f32 * ROW_H;
        if top - 4.0 < scroll {
            scroll = top - 4.0;
        } else if top + ROW_H + 18.0 > scroll + view.h {
            scroll = top + ROW_H + 18.0 - view.h;
        }
    }
    scroll = scroll.clamp(0.0, (content_h - view.h).max(0.0));
    v.scroll = scroll.round() as usize;
    let shown = ui.ease(("sounds-scroll", id), scroll, 150.0);
    if entries.is_empty() {
        let (msg, err) = match v.tab {
            _ if !v.scanned => ("Looking at your packs…".to_string(), false),
            Tab::Installed if v.installed.is_empty() => ("No packs yet. Browse the registry and pick one: enter installs it and uses it.".into(), false),
            Tab::Browse if v.registry.is_none() => match &v.registry_error {
                Some(e) => (format!("Couldn't fetch the PeonPing registry: {e}"), true),
                None => ("Fetching the PeonPing registry…".into(), false),
            },
            _ => (format!("Nothing matches “{}”.", v.query.trim()), false),
        };
        let st = Style::new(size::BODY, weight::REGULAR, if err { t.danger } else { t.muted });
        let b = ui.para(&msg, st, view.w - 32.0, Some(3));
        ui.draw(&b, Point::new(view.x + 16.0, view.y + 18.0));
    }
    ui.clip(view, 0.0);
    for (i, e) in entries.iter().enumerate() {
        let ry = view.y + i as f32 * ROW_H - shown;
        if ry + ROW_H < view.y || ry > view.bottom() {
            continue;
        }
        let r = Rect::new(view.x, ry, view.w, ROW_H);
        let fill = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
        let hot = widgets::row(ui, fill, ("sounds", id, i), v.sel == i, Action::View(id, ROW + i));
        pack_row(v, ui, r, i, *e, v.sel == i, hot, active.as_deref(), id);
    }
    if shown > 0.5 {
        edge_fade(ui, Rect::new(view.x, view.y, view.w, 18.0), t.surface, true);
    }
    if shown + view.h < content_h - 0.5 {
        edge_fade(ui, Rect::new(view.x, view.bottom() - 24.0, view.w, 24.0), t.surface, false);
    }
    ui.unclip();
    let fade = ui.ease(("sounds-bar", id), if ui.hovered(view) { 1.0 } else { 0.35 }, 200.0);
    widgets::scrollbar(ui, view, content_h, view.h, shown, fade);

    hints(ui, Point::new(col.x, hints_y), col.w, HINTS);
    true
}

/// The category chips laid out in rows within `w`: (rect relative to the first, category index).
fn chip_layout(ui: &mut Ui, w: f32) -> Vec<(Rect, usize)> {
    let mut out = vec![];
    let (mut x, mut y) = (0.0, 0.0);
    for (i, (_, label)) in CATEGORIES.iter().enumerate() {
        let cw = ui.measure(label, Style::new(size::SMALL, weight::MEDIUM, ui.t.text)) + 36.0;
        if x > 0.0 && x + cw > w {
            x = 0.0;
            y += CHIP_H + 8.0;
        }
        out.push((Rect::new(x, y, cw, CHIP_H), i));
        x += cw + 8.0;
    }
    out
}

/// One pack: its mark (★ in use, ✓ installed, a spinner while installing), its title and name, what it holds,
/// its description; on the right, its preview, its use / install / update and its removal.
#[allow(clippy::too_many_arguments)]
fn pack_row(v: &SoundsView, ui: &mut Ui, r: Rect, i: usize, e: Entry, on: bool, hot: f32, active: Option<&str>, id: crate::pane::PaneId) {
    let t = ui.t;
    let name = v.name_of(e);
    let job = v.jobs.get(&name).copied();
    let in_use = active == Some(name.as_str());
    let mid = r.y + r.h / 2.0;

    // the mark
    let mr = Rect::new(r.x + 10.0, mid - 10.0, 20.0, 20.0);
    match () {
        _ if job.is_some() => ui.spinner(mr.center(), 6.0, 1.7, t.shine),
        _ if in_use => ui.icon(mr, STAR, size::HEADING, t.shine),
        _ if e.installed.is_some() => ui.icon(mr, CHECK, size::HEADING, t.good),
        _ => ui.icon(mr, DOWNLOAD, size::TITLE, t.faint.mix(t.muted, hot)),
    }

    // the buttons, right to left (measured first: the text takes what's left)
    let mut bx = r.right() - 10.0;
    let by = mid - BUTTON_H / 2.0;
    let mut buttons_w = 0.0;
    if e.installed.is_some() && job.is_none() {
        bx -= BUTTON_H;
        widgets::icon_button(ui, Rect::new(bx, by, BUTTON_H, BUTTON_H), TRASH, t.muted, Action::View(id, REMOVE + i));
        bx -= 6.0;
    }
    let (label, kind, icon, target) = match () {
        _ if job.is_some() => ("Installing", Kind::Secondary, None, None),
        _ if v.update_available(e) => ("Update", Kind::Secondary, Some(UPDATE_ICON), Some(UPDATE + i)),
        _ if in_use => ("In use", Kind::Ghost, None, None),
        _ if e.installed.is_some() => ("Use", Kind::Secondary, None, Some(USE + i)),
        _ => ("Install", if on { Kind::Primary } else { Kind::Secondary }, Some(DOWNLOAD), Some(USE + i)),
    };
    let key = if on && target.is_some() { Some(if label == "Update" { "u" } else { "enter" }) } else { None };
    let w = key_button_width(ui, icon, label, key);
    bx -= w;
    let button = Rect::new(bx, by, w, BUTTON_H);
    if on && target.is_some() {
        focus_ring(ui, button);
    }
    key_button(ui, button, icon, label, key, kind, target.map(|n| Action::View(id, n)));
    bx -= 6.0;
    let playing = v.playing.as_deref() == Some(name.as_str());
    bx -= BUTTON_H;
    widgets::icon_button(ui, Rect::new(bx, by, BUTTON_H, BUTTON_H), if playing { STOP } else { PLAY }, if playing { t.accent } else { t.muted }, Action::View(id, PREVIEW + i));
    buttons_w += r.right() - bx;

    // title · name, then what it holds (or the install's progress) and its description
    let x = r.x + 40.0;
    let text_w = (r.w - 40.0 - buttons_w - 16.0).max(40.0);
    let title_st = Style::new(size::BODY, if on { weight::SEMIBOLD } else { weight::MEDIUM }, if in_use { t.shine } else { t.text });
    let tb = ui.line(&v.title_of(e), title_st, Some(text_w * 0.62));
    ui.draw(&tb, Point::new(x, r.y + 8.0));
    let mut tx = x + tb.w + 8.0;
    let nb = ui.line(&name, Style::mono(size::CAPTION, t.faint), Some((text_w - tb.w - 8.0).max(10.0)));
    ui.draw(&nb, Point::new(tx, r.y + 8.0 + (tb.h - nb.h) / 2.0 + 1.0));
    tx += nb.w + 8.0;
    if let Some(q) = v.listed(e).and_then(|p| p.quality.clone()).filter(|_| tx + 60.0 < x + text_w) {
        let c = match q.as_str() {
            "gold" => t.gold,
            "silver" => t.muted,
            _ => t.warn,
        };
        widgets::pill(ui, Point::new(tx, r.y + 9.0), 16.0, &q, c, c.with_alpha(0.14));
    }
    let line2 = match job {
        Some((d, n)) if n > 0 => format!("installing · {d} of {n} files"),
        Some(_) => "installing…".into(),
        None if v.update_available(e) => format!("version {} out (you have {})", v.listed(e).and_then(|p| p.version.clone()).unwrap_or_default(), e.installed.and_then(|k| v.installed[k].version()).unwrap_or("?")),
        None => {
            let meta = v.meta(e);
            match v.description(e) {
                Some(d) if !meta.is_empty() => format!("{meta}  —  {d}"),
                Some(d) => d,
                None => meta,
            }
        }
    };
    let ink = if job.is_some() { t.shine } else { t.muted };
    ui.label_in(Rect::new(x, r.y + 28.0, text_w, 18.0), &line2, Style::new(size::SMALL, weight::REGULAR, ink), Align::Left);
    if let Some((d, n)) = job.filter(|(_, n)| *n > 0) {
        widgets::meter(ui, Rect::new(x, r.bottom() - 6.0, text_w.min(240.0), 3.0), d as f32 / n as f32, t.shine);
    }
}

fn key(v: &mut SoundsView, code: KeyCode, cx: &mut Cx) {
    v.key(KeyEvent::new(code, KeyModifiers::NONE), cx);
}

/// Target `n` was clicked.
pub(crate) fn click(v: &mut SoundsView, n: usize, _p: Point, _mods: KeyModifiers, count: u32, cx: &mut Cx) {
    if n != SEARCH {
        v.typing = false;
    }
    match n {
        SWITCH => v.toggle(cx),
        VOL_DOWN => v.volume(-0.1, cx),
        VOL_UP => v.volume(0.1, cx),
        TEST => {
            if v.playing.is_some() {
                // a second press stops it
                v.hush();
            } else {
                v.test(cx);
            }
        }
        TAKE_OVER => v.take_over(cx),
        TAB_INSTALLED => v.set_tab(Tab::Installed),
        TAB_BROWSE => v.set_tab(Tab::Browse),
        SEARCH => v.typing = true,
        REFRESH => key(v, KeyCode::Char('r'), cx),
        n if (CAT..CAT + CATEGORIES.len()).contains(&n) => v.toggle_category(n - CAT, cx),
        n if (HARNESS..HARNESS + HARNESS_KEYS.len()).contains(&n) => v.toggle_harness(HARNESS_KEYS[n - HARNESS].0, cx),
        n if n >= UPDATE => {
            v.sel = n - UPDATE;
            v.reinstall(cx);
        }
        n if n >= REMOVE => {
            v.sel = n - REMOVE;
            v.remove(cx);
        }
        n if n >= USE => {
            v.sel = n - USE;
            v.activate(cx);
        }
        n if n >= PREVIEW => {
            v.sel = n - PREVIEW;
            v.preview(cx);
        }
        n if n >= ROW => {
            let i = n - ROW;
            if count >= 2 && v.sel == i {
                v.activate(cx);
            }
            v.sel = i;
        }
        _ => {}
    }
}

/// The wheel moves the selection, three rows a notch.
pub(crate) fn wheel(v: &mut SoundsView, _n: Option<usize>, down: bool, cx: &mut Cx) {
    for _ in 0..3 {
        key(v, if down { KeyCode::Down } else { KeyCode::Up }, cx);
    }
}
