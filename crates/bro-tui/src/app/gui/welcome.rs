//! The empty workspace: the wordmark, what to do first, a "Did you know?" tip, the keys that matter, and whether
//! the logins, the proxy and the phone bridge are up, under a turning world with the agents for moons when the
//! window has the room. Same content as the terminal UI's welcome screen.

use super::actions::Action;
use super::kit::rune as kit_rune;
use super::kit::motion::Spring;
use super::kit::{Style, Ui, radius, size, weight, widgets};
use super::layout::{Presence, flow_begin, flow_enter};
use crate::app::App;
use crate::app::tips::{self, Part, Tip};
use crate::keymap::Act;
use crate::rune::Scene;
use crate::services::Avail;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};

thread_local! {
    /// The world's own clock: (the frame time it last moved at, how far it has turned). It turns faster under the
    /// pointer, and stands still while the window is in the background.
    static WORLD_CLOCK: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

thread_local! {
    /// When the welcome screen last began arriving.
    static ENTERED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

pub fn draw(app: &mut App, ui: &mut Ui, area: Rect, focused_window: bool) {
    let t = ui.t;
    // Everything arrives one after another: each piece rises a few pixels and fades in on a spring, a beat after
    // the one before. It starts when the screen does (the first run, or the last session closing).
    flow_begin("welcome", ui.now, 0);
    let began = {
        let fresh = flow_enter("welcome", 0) != Presence::Known;
        ENTERED.with(|e| {
            if fresh || e.get().is_none() {
                e.set(Some(ui.now));
            }
            e.get().unwrap_or(ui.now)
        })
    };
    let since = ui.now.saturating_duration_since(began).as_secs_f32();
    if since < 0.07 * 14.0 + 0.9 {
        ui.anim.moving = true;
    }
    // how far piece `n` has arrived (0 → 1, a touch over as it lands)
    let arrived = |n: usize| Spring::ARRIVE.step((since - 0.07 * n as f32).max(0.0));
    const RISE: f32 = 14.0;
    // the world above it all, as tall as the window spares (none when it's short)
    let narrow = area.w < 580.0;
    let compact = area.h < 620.0;
    let tight = area.h < 500.0;
    let mark_size = if tight { 28.0 } else if compact { 40.0 } else { 64.0 };
    let title_gap = if tight { 14.0 } else if compact { 32.0 } else { 44.0 };
    let cta_gap = if tight { 16.0 } else if compact { 24.0 } else { 40.0 };
    let tools_gap = if tight { 8.0 } else if compact { 12.0 } else { 20.0 };
    let tools_h = if tight { if narrow { 164.0 } else { 92.0 } } else if narrow { 224.0 } else { 126.0 };
    let base_h = mark_size * 1.35 + title_gap + 36.0 + cta_gap + tools_h + tools_gap + 42.0;
    // "Did you know?" under the tools when there's the height, before the keys (it teaches more of them)
    let tips_h = if compact { TIPS_COMPACT_H } else { TIPS_H };
    let tips_room = if area.h - base_h >= tips_h + tools_gap { tips_h + tools_gap } else { 0.0 };
    let spare = area.h - base_h - tips_room;
    let key_count = if narrow { 0 } else if spare >= 136.0 { 8 } else if spare >= 68.0 { 4 } else { 0 };
    let card_h = base_h + tips_room + key_count as f32 / 2.0 * 34.0;
    let hero_h = (area.h - card_h - 40.0).clamp(0.0, 270.0);
    let hero_h = if hero_h < 140.0 || area.w < 420.0 { 0.0 } else { hero_h };
    let w = 640.0f32.min(area.w - 48.0).max(120.0);
    let h = card_h.min(area.h - 32.0);
    let top = area.y + (area.h - h - hero_h).max(0.0) / 2.0;
    if hero_h > 0.0 {
        let hero = Rect::new(area.x + 24.0, top, area.w - 48.0, hero_h);
        let boost = ui.hover_amount("welcome-world", hero);
        let now = ui.time;
        let clock = WORLD_CLOCK.with(|c| {
            let (last, turned) = c.get();
            let step = if focused_window { (now - last).clamp(0.0, 0.1) * (1.0 + 5.0 * boost as f64) } else { 0.0 };
            c.set((now, turned + step));
            turned + step
        });
        // about 30 rows: fine enough to read as a picture, as Rune's are
        let font = (hero_h / 30.0 / 1.12).clamp(6.5, 10.0);
        let inks = kit_rune::inks(t, t.layer);
        let ticking = ui.ticking;
        kit_rune::draw(ui, hero, font, Scene::World, 40.0 + clock, &inks, arrived(0).clamp(0.0, 1.0));
        // a still world doesn't keep the window drawing
        if !focused_window {
            ui.ticking = ticking;
        }
    }
    let r = Rect::new(area.x + (area.w - w) / 2.0, top + hero_h, w, h);
    // the wordmark in the terminal UI's logo colours: a rainbow moving across it on animated themes, else the
    // accent fading to the theme's shine
    let time = ui.time;
    let letters = ["b", "r", "o"];
    let colors: Vec<Color> = (0..letters.len())
        .map(|i| {
            // where the letter sits along the terminal UI's 25-column logo
            let col = 4.0 + 8.0 * i as f64;
            if app.theme.animated {
                let c = crate::theme::rainbow_at(col * 0.018 - time * 0.06, 0.5);
                super::kit::color(c, t.accent, &t.ansi)
            } else {
                t.accent.mix(t.shine, (col / 25.0) as f32 * 0.8)
            }
        })
        .collect();
    let a = arrived(1);
    let fade = |c: Color, a: f32| c.fade(a.clamp(0.0, 1.0));
    let spans: Vec<(String, Style)> = letters.iter().zip(&colors).map(|(l, c)| (l.to_string(), Style::new(mark_size, weight::BOLD, fade(*c, a)))).collect();
    let mark = ui.rich(&spans, mark_size, None, false, Some(1));
    let mx = r.x + (r.w - mark.w - 18.0) / 2.0;
    let dy = (1.0 - a) * RISE;
    ui.draw(&mark, Point::new(mx, r.y + dy));
    ui.circle(Point::new(mx + mark.w + 10.0, r.y + dy + mark.h - 22.0), 7.0, fade(colors[colors.len() - 1], a));
    if app.theme.animated {
        ui.ticking = true;
    }
    let a = arrived(2);
    let tag = ui.line(if tight { "" } else { "The agentic terminal workspace" }, Style::new(size::TITLE, weight::MEDIUM, fade(t.muted, a)), None);
    let dy = (1.0 - a) * RISE;
    ui.draw(&tag, Point::new(r.x + (r.w - tag.w) / 2.0, r.y + mark.h + 4.0 + dy));
    let mut y = r.y + mark.h + title_gap;
    // call to action: nothing signed in yet (once the profiles have loaded), setting up comes first
    let nobody = app.svc.state().profiles.ready().is_some_and(|ps| !ps.iter().any(|p| p.authenticated));
    let (label, act) = if nobody { ("Install an agent and sign in", Act::Setup) } else { ("Launch your first agent", Act::NewSession) };
    let bw = widgets::button_width(ui, label) + 24.0;
    let a = arrived(3);
    let br = Rect::new(r.x + (r.w - bw) / 2.0, y + (1.0 - a) * RISE, bw, 36.0);
    // (a button has no alpha: it appears a few frames in, rising the rest of the way)
    if a > 0.4 {
        widgets::button(ui, br, label, widgets::Kind::Primary, Action::Welcome(if nobody { 1 } else { 0 }));
    }
    let k = app.keymap.primary(act);
    if !k.is_empty() && !tight {
        let st = Style::new(size::SMALL, weight::MEDIUM, fade(t.faint, a));
        let kw = ui.measure(&k, st);
        ui.label(Point::new(r.x + (r.w - kw) / 2.0, br.bottom() + 8.0), &k, st);
    }
    let br = Rect { y, ..br };
    y = br.bottom() + cta_gap;
    // Two complementary tools, using the same native surfaces and type as the rest of Bro.
    // Whole rows are targets; the welcome keys w/c reach the same setup from a keyboard.
    let gap = if tight { 8.0 } else { 12.0 };
    let cw = if narrow { r.w } else { (r.w - gap) / 2.0 };
    let ch = if tight { if narrow { 78.0 } else { 92.0 } } else if narrow { 106.0 } else { 126.0 };
    for (i, (name, detail, glyph, key)) in [
        ("JustBrowse", "Your signed-in browser. Each agent gets its own tabs.", "\u{F059F}", "w"),
        ("CUA", "Desktop apps, through Cua Driver.", "\u{F0379}", "c"),
    ].iter().enumerate() {
        let card = Rect::new(r.x + if narrow { 0.0 } else { i as f32 * (cw + gap) }, y + if narrow { i as f32 * (ch + gap) } else { 0.0 }, cw, ch);
        let hot = ui.hover_amount(("welcome-automation", i), card);
        ui.glass_panel(card, radius::OVERLAY, t.surface.mix(t.hover, hot * 0.5), t.hairline.mix(t.stroke, hot));
        let pad_y = if tight { 10.0 } else { 16.0 };
        ui.icon(Rect::new(card.x + 16.0, card.y + pad_y, 22.0, 22.0), glyph, 22.0, t.text);
        ui.label(Point::new(card.x + 48.0, card.y + pad_y - 1.0), name, Style::new(size::TITLE, weight::SEMIBOLD, t.text));
        let detail = if tight { if i == 0 { "Your signed-in browser" } else { "Desktop apps" } } else { detail };
        let description = ui.para(detail, Style::new(size::SMALL, weight::REGULAR, t.muted), (cw - 32.0).max(60.0), Some(if tight { 1 } else { 2 }));
        ui.draw(&description, Point::new(card.x + 16.0, card.y + if tight { 34.0 } else { 46.0 }));
        ui.label(Point::new(card.x + 16.0, card.bottom() - if tight { 23.0 } else { 27.0 }), if i == 0 { "Set up browser" } else { "Set up computer" }, Style::new(size::SMALL, weight::SEMIBOLD, t.accent));
        widgets::kbd(ui, Point::new(card.right() - 36.0, card.bottom() - if tight { 25.0 } else { 29.0 }), key);
        ui.hit(card, Action::Welcome(2 + i));
    }
    y += tools_h + tools_gap;
    if tips_room > 0.0 {
        let a = arrived(4);
        tips_card(app, ui, Rect::new(r.x, y + (1.0 - a) * RISE, r.w, tips_h), a.clamp(0.0, 1.0), focused_window, compact);
        y += tips_room;
    }
    // the keys that matter
    let keys: [(Act, &str); 8] = [
        (Act::NewSession, "Launch an agent"),
        (Act::FocusSidebar, "Browse projects"),
        (Act::Palette, "Command palette"),
        (Act::Usage, "Usage meters"),
        (Act::Profiles, "Profiles and logins"),
        (Act::Setup, "Setup guide"),
        (Act::Bridge, "Pair your phone"),
        (Act::Help, "All keys"),
    ];
    let col_w = (r.w - 24.0) / 2.0;
    for (i, (act, what)) in keys.iter().take(key_count).enumerate() {
        let cx = r.x + 12.0 + (i % 2) as f32 * col_w;
        let a = arrived(5 + i);
        let cy = y + (i / 2) as f32 * 34.0 + (1.0 - a) * RISE;
        let row = Rect::new(cx, cy, col_w - 8.0, 30.0);
        let h = ui.hover_amount(("welcome-key", i), row);
        ui.round(row, radius::CONTROL, t.hover.fade(h));
        ui.label_in(Rect::new(row.x + 10.0, row.y, row.w - 120.0, row.h), what, Style::new(size::BODY, weight::MEDIUM, fade(t.text.mix(t.muted, 0.25), a)), Align::Left);
        let chord = app.keymap.primary(*act);
        if !chord.is_empty() && a > 0.5 {
            // measure the caps off-screen first, then right-align them
            let kw = widgets::kbd_width(ui, &chord);
            widgets::kbd(ui, Point::new(row.right() - kw - 8.0, row.y + 6.0), &chord);
        }
        ui.hit(row, Action::Welcome(10 + i));
    }
    y += key_count as f32 / 2.0 * 34.0 + if tight { 4.0 } else { 22.0 };
    let a = arrived(14);
    y += (1.0 - a) * RISE;
    // status line
    let st = app.svc.state();
    let profiles = match &st.profiles {
        Avail::Ready(ps) => {
            let n = ps.iter().filter(|p| p.authenticated).count();
            (if n > 0 { t.good } else { t.inline }, format!("{n} logged-in profile{}", if n == 1 { "" } else { "s" }))
        }
        Avail::Loading => (t.muted, "profiles loading".into()),
        Avail::Unavailable(_) => (t.danger, "profiles unavailable".into()),
    };
    let proxy = match &st.proxy.status {
        Avail::Ready(i) => (t.good, format!("proxy :{}", i.port)),
        Avail::Loading => (t.muted, "proxy starting".into()),
        Avail::Unavailable(_) => (t.danger, "proxy off".into()),
    };
    let bridge = match &st.bridge.status {
        Avail::Ready(s) if s.running => (t.good, format!("bridge :{} · {} dev", s.port, s.clients)),
        Avail::Loading if st.bridge.enabled => (t.muted, "bridge starting".into()),
        _ => (t.muted, "bridge off".into()),
    };
    drop(st);
    let items = [profiles, proxy, bridge];
    let items = &items[..if r.w < 280.0 { 1 } else if r.w < 430.0 { 2 } else { 3 }];
    let sst = Style::new(size::SMALL, weight::MEDIUM, fade(t.muted, a));
    let widths: Vec<f32> = items.iter().map(|(_, s)| ui.measure(s, sst) + 16.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 16.0 * (items.len() - 1) as f32;
    let mut x = r.x + (r.w - total) / 2.0;
    for ((c, s), w) in items.iter().zip(widths) {
        let ir = Rect::new(x - 2.0, y, 14.0, 16.0);
        if *c == t.good {
            ui.icon(ir, "\u{F012C}", size::SMALL, fade(t.good, a));
        } else if *c == t.danger {
            ui.icon(ir, "\u{F0156}", size::SMALL, fade(t.danger, a));
        } else if *c == t.inline {
            ui.icon(ir, "\u{F0028}", size::SMALL, fade(t.inline, a));
        } else {
            ui.icon(ir, "\u{F0374}", size::SMALL, fade(t.faint, a));
        }
        ui.label(Point::new(x + 14.0, y), s, sst);
        x += w + 16.0;
    }
}

/// The tips card's height, and its height in a short window (a line less of detail).
const TIPS_H: f32 = 116.0;
const TIPS_COMPACT_H: f32 = 98.0;
/// The tips card's targets: back, on, the card itself (on), and from `TIP_DOT` the dots.
const TIP_BACK: usize = 20;
const TIP_ON: usize = 21;
const TIP_CARD: usize = 22;
const TIP_DOT: usize = 30;

/// "Did you know?": one tip at a time in a card. The next slides in after a few seconds (held while the pointer rests
/// on the card or the window is in the background); the arrows, the dots, the wheel and `[` `]` move through them.
fn tips_card(app: &mut App, ui: &mut Ui, card: Rect, a: f32, focused_window: bool, compact: bool) {
    let t = ui.t;
    let list = app.tip_list();
    let n = list.len();
    if n == 0 {
        return;
    }
    let resting = ui.hovered(card);
    let i = app.tips.current(n, ui.now, resting || !focused_window);
    let hot = ui.hover_amount("welcome-tips", card);
    ui.glass_panel(card, radius::OVERLAY, t.surface.mix(t.hover, hot * 0.35).fade(a), t.hairline.mix(t.stroke, hot * 0.6).fade(a));
    ui.hit(card, Action::Welcome(TIP_CARD));
    let pad = 16.0;
    // the heading, and on the right where this one is among them between the arrows
    let head = Rect::new(card.x + pad, card.y + 10.0, card.w - 2.0 * pad, 26.0);
    ui.icon(Rect::new(head.x - 2.0, head.y, 18.0, head.h), "\u{F06E8}", size::TITLE, t.gold.fade(a));
    ui.label_in(Rect::new(head.x + 20.0, head.y, 140.0, head.h), "Did you know?", Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.2).fade(a)), Align::Left);
    let on = Rect::new(card.right() - 8.0 - 28.0, head.y - 1.0, 28.0, 28.0);
    widgets::icon_button(ui, on, "\u{F0142}", t.muted.fade(a), Action::Welcome(TIP_ON));
    let mut x = on.x - 4.0;
    let cy = head.y + head.h / 2.0;
    if card.w >= 300.0 + n as f32 * 11.0 {
        // a dot each; the one showing is long, and fills as its time runs out
        const DOT: f32 = 6.0;
        const GAP: f32 = 5.0;
        const LONG: f32 = 20.0;
        let grown: Vec<f32> = (0..n).map(|k| ui.spring(("welcome-tip-dot", k), if k == i { 1.0 } else { 0.0 }, Spring::GLIDE, if k == i { 1.0 } else { 0.0 }).clamp(0.0, 1.0)).collect();
        let widths: Vec<f32> = grown.iter().map(|g| DOT + (LONG - DOT) * g).collect();
        let total = widths.iter().sum::<f32>() + GAP * (n - 1) as f32;
        let progress = app.tips.progress(ui.now);
        let mut dx = x - total;
        for (k, (w, g)) in widths.iter().zip(&grown).enumerate() {
            let dot = Rect::new(dx, cy - DOT / 2.0, *w, DOT);
            let target = Rect::new(dx - GAP / 2.0, cy - 10.0, w + GAP, 20.0);
            let h = ui.hover_amount(("welcome-tip-dot-hover", k), target);
            ui.round(dot, DOT / 2.0, t.faint.mix(t.muted, h.max(*g)).fade(a * (0.5 + 0.5 * h.max(*g))));
            if k == i {
                ui.round(Rect::new(dot.x, dot.y, DOT + (dot.w - DOT) * progress, DOT), DOT / 2.0, t.accent.fade(a));
            }
            ui.hit(target, Action::Welcome(TIP_DOT + k));
            dx += w + GAP;
        }
        x -= total + 4.0;
    } else {
        let label = format!("{} / {n}", i + 1);
        let st = Style::new(size::SMALL, weight::MEDIUM, t.faint.fade(a));
        let lw = ui.measure(&label, st);
        ui.label_in(Rect::new(x - lw - 2.0, head.y, lw, head.h), &label, st, Align::Right);
        x -= lw + 6.0;
    }
    widgets::icon_button(ui, Rect::new(x - 28.0, on.y, 28.0, 28.0), "\u{F0141}", t.muted.fade(a), Action::Welcome(TIP_BACK));
    // the tip, sliding in from the side it came from as the last slides out the other
    let body = Rect::new(card.x + 1.0, head.bottom() + 2.0, card.w - 2.0, card.bottom() - head.bottom() - 3.0);
    let at = Point::new(card.x + pad, body.y + 4.0);
    let w = card.w - 2.0 * pad;
    ui.clip(body, radius::OVERLAY - 1.0);
    let slide = app.tips.from.map(|(prev, dir, since)| (prev, dir as f32, Spring::GLIDE.step(ui.now.saturating_duration_since(since).as_secs_f32()).clamp(0.0, 1.0)));
    match slide {
        Some((prev, dir, p)) if p < 0.995 && prev < n => {
            ui.anim.moving = true;
            const SHIFT: f32 = 40.0;
            tip(ui, &list[prev], Point::new(at.x - dir * SHIFT * p, at.y), w, a * (1.0 - 1.6 * p).max(0.0), compact);
            tip(ui, &list[i], Point::new(at.x + dir * SHIFT * (1.0 - p), at.y), w, a * ((p - 0.2) / 0.8).clamp(0.0, 1.0), compact);
        }
        _ => tip(ui, &list[i], at, w, a, compact),
    }
    ui.unclip();
}

/// One tip at `at` (top-left) across `w`: its keys as keycaps (and what the mouse does) beside the headline, the
/// detail under them.
fn tip(ui: &mut Ui, tip: &Tip, at: Point, w: f32, alpha: f32, compact: bool) {
    if alpha <= 0.01 {
        return;
    }
    let t = ui.t;
    const CAP_H: f32 = 22.0;
    let cap_st = Style::new(size::SMALL, weight::SEMIBOLD, t.text.fade(alpha));
    let word_st = Style::new(size::SMALL, weight::MEDIUM, t.muted.fade(alpha));
    let plus_st = Style::new(size::SMALL, weight::MEDIUM, t.faint.fade(alpha));
    let shade = if t.light { Color::rgba(0.0, 0.0, 0.0, 0.18) } else { Color::rgba(0.0, 0.0, 0.0, 0.45) };
    let parts = tips::gesture(&tip.how);
    let mut x = at.x;
    for (k, part) in parts.iter().enumerate() {
        if k > 0 {
            if tips::joined(&parts[k - 1], part) {
                let b = ui.line("+", plus_st, None);
                ui.draw(&b, Point::new(x + 5.0, at.y + (CAP_H - b.h) / 2.0));
                x += b.w + 10.0;
            } else {
                x += 6.0;
            }
        }
        match part {
            Part::Key(key) => {
                let b = ui.line(key, cap_st, None);
                let r = Rect::new(x, at.y, (b.w + 14.0).max(CAP_H), CAP_H);
                ui.panel(r, radius::CONTROL, t.control.fade(alpha), t.stroke.fade(alpha));
                ui.hline(r.x + 2.0, r.bottom() - ui.hair(), r.w - 4.0, shade.fade(alpha));
                ui.draw(&b, Point::new(r.x + (r.w - b.w) / 2.0, r.y + (CAP_H - b.h) / 2.0 - 0.5));
                x = r.right();
            }
            Part::Word(word) => {
                let b = ui.line(word, word_st, None);
                ui.draw(&b, Point::new(x, at.y + (CAP_H - b.h) / 2.0));
                x += b.w;
            }
        }
    }
    let title_st = Style::new(size::TITLE, weight::SEMIBOLD, t.text.fade(alpha));
    let tx = x + 14.0;
    let lines = if compact { 1 } else { 2 };
    let (detail_y, lines) = if ui.measure(tip.title, title_st) <= at.x + w - tx {
        let b = ui.line(tip.title, title_st, None);
        ui.draw(&b, Point::new(tx, at.y + (CAP_H - b.h) / 2.0));
        (at.y + CAP_H + 8.0, lines)
    } else {
        // no room beside the keys: the headline goes under them and the detail gives up a line
        let b = ui.line(tip.title, title_st, Some(w));
        ui.draw(&b, Point::new(at.x, at.y + CAP_H + 5.0));
        (at.y + CAP_H + 9.0 + b.h, lines - 1)
    };
    if lines > 0 {
        let d = ui.para(&tip.detail, Style::new(size::SMALL, weight::REGULAR, t.muted.fade(alpha)), w, Some(lines));
        ui.draw(&d, Point::new(at.x, detail_y));
    }
}

/// The wheel over the tips card moves through them, a tip a burst. False when `n` isn't one of its targets.
pub fn wheel(app: &mut App, n: usize, down: bool) -> bool {
    if !(TIP_BACK..=TIP_CARD).contains(&n) && n < TIP_DOT {
        return false;
    }
    app.tip_wheel(if down { 1 } else { -1 });
    true
}

/// The welcome screen's targets: 0 launch, 1 set up, 2–3 the tools, 10.. the key rows, 20–22 and 30.. the tips.
pub fn click(app: &mut App, n: usize) {
    let acts = [Act::NewSession, Act::FocusSidebar, Act::Palette, Act::Usage, Act::Profiles, Act::Setup, Act::Bridge, Act::Help];
    match n {
        0 => app.run_act(Act::NewSession),
        1 => app.run_act(Act::Setup),
        2 | 3 => app.open_view("automation"),
        TIP_BACK => app.tip_step(-1),
        TIP_ON | TIP_CARD => app.tip_step(1),
        n if n >= TIP_DOT => app.tip_show(n - TIP_DOT),
        n if n >= 10 => {
            if let Some(a) = acts.get(n - 10) {
                app.run_act(*a);
            }
        }
        _ => {}
    }
}
